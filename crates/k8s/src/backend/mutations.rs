//! Generic (non-Flux-specific) mutations: delete, scale, rollout restart,
//! server-side apply, ESO force-sync, and Job creation actions.

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

    pub async fn certificate_renew(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        let entry = self.entry(key)?;
        if entry.kind.group != "cert-manager.io" || entry.kind.kind != "Certificate" {
            return Err(K8sError::Api(
                "renewal requires a cert-manager.io Certificate".into(),
            ));
        }
        let namespace = ns
            .filter(|namespace| !namespace.is_empty())
            .ok_or_else(|| K8sError::Api("Certificate namespace is required".into()))?;
        let api = self.dyn_api(key, Some(namespace))?;
        let certificate = api.get(name).await.map_err(api_err)?;
        let mut data = serde_json::to_value(certificate).map_err(api_err)?;
        mark_certificate_for_renewal(&mut data, &now_rfc3339())?;
        let certificate: DynamicObject = serde_json::from_value(data).map_err(api_err)?;
        api.replace_status(name, &PostParams::default(), &certificate)
            .await
            .map_err(api_err)?;
        Ok(())
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

    pub async fn job_rerun(&self, key: &str, ns: Option<&str>, name: &str) -> Result<(), K8sError> {
        if key != "batch/v1/Job" {
            return Err(K8sError::Api("re-run requires a batch/v1 Job".into()));
        }
        let namespace = ns.ok_or_else(|| K8sError::Api("Job namespace is required".into()))?;
        let source = self
            .dyn_api(key, Some(namespace))?
            .get(name)
            .await
            .map_err(api_err)?;
        let job = build_rerun_job(&source, namespace, name)?;
        self.dyn_api(key, Some(namespace))?
            .create(&PostParams::default(), &job)
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

fn build_rerun_job(
    source: &DynamicObject,
    namespace: &str,
    source_name: &str,
) -> Result<DynamicObject, K8sError> {
    let data = serde_json::to_value(source).map_err(api_err)?;
    let terminal = data
        .get("status")
        .and_then(|status| status.get("conditions"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                matches!(
                    condition.get("type").and_then(serde_json::Value::as_str),
                    Some("Complete" | "Failed")
                ) && condition.get("status").and_then(serde_json::Value::as_str) == Some("True")
            })
        });
    if !terminal {
        return Err(K8sError::Api(
            "only completed or failed Jobs can be re-run".into(),
        ));
    }

    let mut spec = data
        .get("spec")
        .cloned()
        .ok_or_else(|| K8sError::Api("Job has no spec".into()))?;
    let spec_object = spec
        .as_object_mut()
        .ok_or_else(|| K8sError::Api("Job spec is not an object".into()))?;
    spec_object.remove("selector");
    spec_object.remove("manualSelector");
    if let Some(labels) = spec_object
        .get_mut("template")
        .and_then(|template| template.get_mut("metadata"))
        .and_then(|metadata| metadata.get_mut("labels"))
        .and_then(serde_json::Value::as_object_mut)
    {
        for generated in [
            "batch.kubernetes.io/controller-uid",
            "batch.kubernetes.io/job-name",
            "controller-uid",
            "job-name",
        ] {
            labels.remove(generated);
        }
    }

    let base: String = source_name
        .to_ascii_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(48)
        .collect();
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "job" } else { base };
    let job = json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {
            "generateName": format!("{base}-rerun-"),
            "namespace": namespace,
        },
        "spec": spec,
    });
    serde_json::from_value(job).map_err(api_err)
}

fn mark_certificate_for_renewal(
    certificate: &mut serde_json::Value,
    transition_time: &str,
) -> Result<(), K8sError> {
    let generation = certificate
        .pointer("/metadata/generation")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default();
    let root = certificate
        .as_object_mut()
        .ok_or_else(|| K8sError::Api("Certificate is not an object".into()))?;
    let status = root
        .entry("status")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| K8sError::Api("Certificate status is not an object".into()))?;
    let conditions = status
        .entry("conditions")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| K8sError::Api("Certificate status.conditions is not an array".into()))?;
    let manual = json!({
        "lastTransitionTime": transition_time,
        "message": "Certificate re-issuance manually triggered",
        "observedGeneration": generation,
        "reason": "ManuallyTriggered",
        "status": "True",
        "type": "Issuing",
    });
    if let Some(condition) = conditions.iter_mut().find(|condition| {
        condition.get("type").and_then(serde_json::Value::as_str) == Some("Issuing")
    }) {
        *condition = manual;
    } else {
        conditions.push(manual);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn source_job(condition: Option<&str>) -> DynamicObject {
        let conditions = condition.map_or_else(Vec::new, |condition| {
            vec![json!({"type": condition, "status": "True"})]
        });
        serde_json::from_value(json!({
            "apiVersion": "batch/v1",
            "kind": "Job",
            "metadata": {"name": "Database_Backup", "namespace": "default"},
            "spec": {
                "manualSelector": true,
                "selector": {"matchLabels": {"controller-uid": "old-uid"}},
                "backoffLimit": 4,
                "template": {
                    "metadata": {
                        "annotations": {"example.com/keep": "yes"},
                        "labels": {
                            "app": "backup",
                            "batch.kubernetes.io/controller-uid": "old-uid",
                            "batch.kubernetes.io/job-name": "Database_Backup",
                            "controller-uid": "old-uid",
                            "job-name": "Database_Backup"
                        }
                    },
                    "spec": {
                        "restartPolicy": "Never",
                        "containers": [{"name": "backup", "image": "backup:latest"}]
                    }
                }
            },
            "status": {"conditions": conditions}
        }))
        .unwrap()
    }

    #[test]
    fn rerun_rejects_non_terminal_job() {
        let error = build_rerun_job(&source_job(None), "default", "backup").unwrap_err();
        assert!(error.to_string().contains("only completed or failed"));
    }

    #[test]
    fn rerun_clones_spec_without_controller_identity() {
        let job =
            build_rerun_job(&source_job(Some("Complete")), "default", "Database_Backup").unwrap();
        let data = serde_json::to_value(job).unwrap();

        assert_eq!(
            data.pointer("/metadata/generateName")
                .and_then(Value::as_str),
            Some("databasebackup-rerun-")
        );
        assert_eq!(
            data.pointer("/spec/backoffLimit").and_then(Value::as_i64),
            Some(4)
        );
        assert!(data.pointer("/spec/selector").is_none());
        assert!(data.pointer("/spec/manualSelector").is_none());
        assert_eq!(
            data.pointer("/spec/template/metadata/labels/app")
                .and_then(Value::as_str),
            Some("backup")
        );
        assert_eq!(
            data.pointer("/spec/template/metadata/labels")
                .and_then(Value::as_object)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            data.pointer("/spec/template/metadata/annotations/example.com~1keep")
                .and_then(Value::as_str),
            Some("yes")
        );
    }

    #[test]
    fn failed_job_can_be_rerun_with_bounded_generate_name() {
        let source_name = format!("{}!", "A".repeat(100));
        let job = build_rerun_job(&source_job(Some("Failed")), "default", &source_name).unwrap();
        let generate_name = job.metadata.generate_name.unwrap();
        assert!(generate_name.ends_with("-rerun-"));
        assert!(generate_name.len() <= 55);
    }

    #[test]
    fn manual_certificate_renewal_preserves_other_conditions() {
        let mut certificate = json!({
            "metadata": {"generation": 7},
            "status": {"conditions": [
                {"type": "Ready", "status": "True"},
                {"type": "Issuing", "status": "False", "reason": "Completed"}
            ]}
        });
        mark_certificate_for_renewal(&mut certificate, "2026-08-27T12:00:00Z").unwrap();
        let conditions = certificate
            .pointer("/status/conditions")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(conditions.len(), 2);
        assert_eq!(conditions[0]["type"], "Ready");
        assert_eq!(conditions[1]["status"], "True");
        assert_eq!(conditions[1]["reason"], "ManuallyTriggered");
        assert_eq!(conditions[1]["observedGeneration"], 7);
    }
}
