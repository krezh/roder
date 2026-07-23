//! Generic (non-Flux-specific) mutations: delete, scale, rollout restart,
//! server-side apply, ESO force-sync, and manual CronJob triggering.

use k8s_openapi::api::core::v1::Pod;
use kube::api::{
    Api, DeleteParams, DynamicObject, EvictParams, Patch, PatchParams, PostParams,
    PropagationPolicy,
};
use roder_core::{DeletePropagation, ResourceKind};
use serde_json::json;

use super::{api_err, now_rfc3339, Backend};
use crate::client::K8sError;

impl Backend {
    /// `force` maps to `kubectl delete --force` (grace period of zero, i.e.
    /// no graceful termination wait); `propagation` maps to `--cascade`.
    pub async fn delete(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
        force: bool,
        propagation: Option<DeletePropagation>,
    ) -> Result<(), K8sError> {
        let params = DeleteParams {
            grace_period_seconds: force.then_some(0),
            propagation_policy: propagation.map(|p| match p {
                DeletePropagation::Orphan => PropagationPolicy::Orphan,
                DeletePropagation::Background => PropagationPolicy::Background,
                DeletePropagation::Foreground => PropagationPolicy::Foreground,
            }),
            ..DeleteParams::default()
        };
        self.dyn_api(key, ns)?
            .delete(name, &params)
            .await
            .map_err(api_err)?;
        Ok(())
    }

    /// Evict a pod via the eviction subresource, so the server enforces any
    /// PodDisruptionBudget instead of the pod just being deleted outright.
    pub async fn evict_pod(&self, ns: &str, name: &str) -> Result<(), K8sError> {
        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        api.evict(name, &EvictParams::default())
            .await
            .map_err(api_err)?;
        Ok(())
    }

    pub async fn scale(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
        replicas: i32,
    ) -> Result<(), K8sError> {
        self.merge_patch(key, ns, name, json!({ "spec": { "replicas": replicas } }))
            .await
    }

    /// Cordon (`unschedulable: true`) or uncordon a Node. Node is cluster-scoped,
    /// so there's no namespace to pass through.
    pub async fn cordon(&self, key: &str, name: &str, unschedulable: bool) -> Result<(), K8sError> {
        self.merge_patch(
            key,
            None,
            name,
            json!({ "spec": { "unschedulable": unschedulable } }),
        )
        .await
    }

    pub async fn rollout_restart(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        let patch = json!({ "spec": { "template": { "metadata": { "annotations": {
            "kubectl.kubernetes.io/restartedAt": now_rfc3339()
        }}}}});
        self.merge_patch(key, ns, name, patch).await
    }

    pub async fn eso_refresh(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        let patch = json!({ "metadata": { "annotations": {
            "force-sync": now_rfc3339()
        }}});
        self.merge_patch(key, ns, name, patch).await
    }

    /// Manually trigger a CronJob: create a Job from its `spec.jobTemplate`
    /// (the same thing `kubectl create job --from=cronjob/<name>` does).
    pub async fn cronjob_trigger(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        let cj = self.dyn_api(key, ns)?.get(name).await.map_err(api_err)?;
        let data = serde_json::to_value(&cj).map_err(api_err)?;
        let tmpl = data
            .get("spec")
            .and_then(|s| s.get("jobTemplate"))
            .ok_or_else(|| K8sError::Api("CronJob has no spec.jobTemplate".into()))?;
        let job_spec = tmpl.get("spec").cloned().unwrap_or_else(|| json!({}));
        let uid = data
            .get("metadata")
            .and_then(|m| m.get("uid"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        // DNS-1123 subdomain: lowercase alphanumeric and hyphens only.
        let ts = time::OffsetDateTime::now_utc().unix_timestamp();
        let base: String = name
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(40)
            .collect();
        let base = base.trim_matches('-');
        let base = if base.is_empty() { "job" } else { base };
        let job_name = format!("{base}-manual-{ts}");

        let job = json!({
            "apiVersion": "batch/v1",
            "kind": "Job",
            "metadata": {
                "name": job_name,
                "namespace": ns,
                "annotations": { "cronjob.kubernetes.io/instantiate": "manual" },
                "ownerReferences": [{
                    "apiVersion": "batch/v1",
                    "kind": "CronJob",
                    "name": name,
                    "uid": uid,
                    "controller": false,
                    "blockOwnerDeletion": true,
                }],
            },
            "spec": job_spec,
        });
        let obj: DynamicObject = serde_json::from_value(job).map_err(api_err)?;
        self.dyn_api("batch/v1/Job", ns)?
            .create(&PostParams::default(), &obj)
            .await
            .map_err(api_err)?;
        Ok(())
    }

    /// Server-side apply an edited YAML document.
    pub async fn apply_yaml(&self, yaml: &str) -> Result<(), K8sError> {
        let obj: DynamicObject =
            serde_yaml::from_str(yaml).map_err(|e| K8sError::Api(format!("invalid YAML: {e}")))?;
        let types = obj
            .types
            .as_ref()
            .ok_or_else(|| K8sError::Api("document is missing apiVersion/kind".into()))?;
        let (group, version) = match types.api_version.split_once('/') {
            Some((g, v)) => (g.to_string(), v.to_string()),
            None => (String::new(), types.api_version.clone()),
        };
        let key = ResourceKind::make_key(&group, &version, &types.kind);
        let name = obj
            .metadata
            .name
            .clone()
            .ok_or_else(|| K8sError::Api("document is missing metadata.name".into()))?;
        let ns = obj.metadata.namespace.clone();

        self.dyn_api(&key, ns.as_deref())?
            .patch(
                &name,
                &PatchParams::apply("roder").force(),
                &Patch::Apply(&obj),
            )
            .await
            .map_err(api_err)?;
        Ok(())
    }
}
