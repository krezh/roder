use std::collections::HashMap;
use std::sync::Arc;

use std::pin::Pin;

use futures::future::join_all;
use futures::io::AsyncBufReadExt;
use futures::{Stream, StreamExt};
use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use k8s_openapi::api::core::v1::{Event, Namespace, Node, Pod};
use kube::api::{
    Api, DeleteParams, DynamicObject, ListParams, LogParams, Patch, PatchParams, PostParams,
};
use roder_core::{
    Category, ClusterOverview, HealthRollup, MetricsPoint, NodeSummary, ObjectDetail, ObjectEvent,
    ResourceKind,
};
use serde_json::json;

use crate::client::{make_api, ClusterAccess, K8sError};
use crate::discovery::{build_catalog, CatalogEntry};
use crate::informers::{InformerRegistry, WatchHandle};
use crate::metrics::{node_usage, parse_cpu, parse_mem};
use crate::project::ts_string;

type CanCacheKey = (String, String, Option<String>);

/// The server-side façade over a connected cluster: the token-passthrough client,
/// the discovered resource catalog, and the shared-informer registry.
pub struct Backend {
    cluster: Arc<ClusterAccess>,
    catalog: Vec<CatalogEntry>,
    by_key: HashMap<String, CatalogEntry>,
    registry: Arc<InformerRegistry>,
    /// Short-lived cache so rapid (re)connects don't each re-LIST the cluster.
    /// RwLock allows concurrent reads when the cache is fresh.
    overview_cache: tokio::sync::RwLock<Option<(std::time::Instant, ClusterOverview)>>,
    /// Short-TTL SelfSubjectAccessReview cache keyed (verb, key, namespace).
    /// RwLock so concurrent permission reads don't serialize.
    can_cache: tokio::sync::RwLock<HashMap<CanCacheKey, (std::time::Instant, bool)>>,
}

impl Backend {
    pub async fn connect_with_token(id_token: &str) -> Result<Self, K8sError> {
        let cluster = Arc::new(ClusterAccess::connect_with_token(id_token).await?);
        Self::build(cluster).await
    }

    pub async fn connect_with_default() -> Result<Self, K8sError> {
        let cluster = Arc::new(ClusterAccess::connect_with_default().await?);
        Self::build(cluster).await
    }

    async fn build(cluster: Arc<ClusterAccess>) -> Result<Self, K8sError> {
        let client = cluster.client();
        // Harvest CRD-declared printer columns once; shared by the catalog (headers)
        // and the informers (cell projection). Empty if CRDs aren't listable.
        let columns = Arc::new(crate::printer_columns::load(&client).await);
        let catalog = build_catalog(&client, &columns).await?;
        let by_key = catalog
            .iter()
            .map(|e| (e.kind.key.clone(), e.clone()))
            .collect();
        let registry = InformerRegistry::new(cluster.clone(), columns);
        Ok(Self {
            cluster,
            catalog,
            by_key,
            registry,
            overview_cache: tokio::sync::RwLock::new(None),
            can_cache: tokio::sync::RwLock::new(HashMap::new()),
        })
    }

    /// Swap in a refreshed ID token (called by the refresh task).
    pub fn set_token(&self, id_token: &str) -> Result<(), K8sError> {
        self.cluster.set_token(id_token)
    }

    /// The current `kube::Client` (a cheap clone of the hot-swappable inner Arc).
    fn client(&self) -> kube::Client {
        (*self.cluster.client()).clone()
    }

    /// The browsable resource catalog as surfaced to the UI.
    pub fn kinds(&self) -> Vec<ResourceKind> {
        self.catalog.iter().map(|e| e.kind.clone()).collect()
    }

    pub async fn namespaces(&self) -> Result<Vec<String>, K8sError> {
        let api: Api<Namespace> = Api::all(self.client());
        let list = api.list(&ListParams::default()).await.map_err(api_err)?;
        let mut names: Vec<String> = list
            .items
            .into_iter()
            .filter_map(|n| n.metadata.name)
            .collect();
        names.sort();
        Ok(names)
    }

    pub async fn subscribe(
        &self,
        key: &str,
        namespace: Option<String>,
        selector: Option<String>,
    ) -> Result<WatchHandle, K8sError> {
        let entry = self.entry(key)?;
        Ok(self
            .registry
            .subscribe(
                &entry.api_resource,
                &entry.kind.group,
                &entry.kind.kind,
                entry.kind.namespaced,
                namespace,
                selector,
            )
            .await)
    }

    pub async fn detail(
        &self,
        key: &str,
        namespace: Option<&str>,
        name: &str,
    ) -> Result<ObjectDetail, K8sError> {
        // Prefer the live informer cache (instant, no apiserver round-trip); only
        // GET on a miss (e.g. the kind isn't currently being watched).
        let mut obj = match self.registry.cached_object(key, namespace, name).await {
            Some(o) => o,
            None => {
                let entry = self.entry(key)?.clone();
                let api: Api<DynamicObject> = make_api(
                    self.client(),
                    &entry.api_resource,
                    entry.kind.namespaced,
                    namespace,
                );
                api.get(name).await.map_err(api_err)?
            }
        };
        obj.metadata.managed_fields = None; // declutter
        let yaml = serde_yaml::to_string(&obj).map_err(api_err)?;
        let object = serde_json::to_value(&obj).unwrap_or_default();
        let events = self.events_for(namespace, name).await.unwrap_or_default();

        Ok(ObjectDetail {
            name: name.to_string(),
            namespace: namespace.map(|s| s.to_string()),
            object,
            yaml,
            events,
        })
    }

    async fn events_for(
        &self,
        namespace: Option<&str>,
        name: &str,
    ) -> Result<Vec<ObjectEvent>, K8sError> {
        let client = self.client();
        let api: Api<Event> = match namespace {
            Some(ns) => Api::namespaced(client, ns),
            None => Api::all(client),
        };
        let lp = ListParams::default().fields(&format!("involvedObject.name={name}"));
        let list = api.list(&lp).await.map_err(api_err)?;
        let mut events: Vec<ObjectEvent> = list
            .items
            .into_iter()
            .map(|e| ObjectEvent {
                type_: e.type_.unwrap_or_default(),
                reason: e.reason.unwrap_or_default(),
                message: e.message.unwrap_or_default(),
                age: e.last_timestamp.as_ref().and_then(ts_string),
                count: e.count.unwrap_or(0),
            })
            .collect();
        events.sort_by(|a, b| b.age.cmp(&a.age));
        Ok(events)
    }

    /// Dashboard overview. Served from a short-lived cache so frequent page
    /// (re)connects reuse one snapshot instead of each re-listing the cluster.
    pub async fn overview(&self) -> Result<ClusterOverview, K8sError> {
        const TTL: std::time::Duration = std::time::Duration::from_secs(8);
        {
            let cache = self.overview_cache.read().await;
            if let Some((at, ov)) = cache.as_ref() {
                if at.elapsed() < TTL {
                    return Ok(ov.clone());
                }
            }
        }
        let fresh = self.compute_overview().await?;
        let mut cache = self.overview_cache.write().await;
        // Re-check under the write lock: another concurrent caller may have
        // already populated a fresh entry while we were computing.
        if cache.as_ref().map(|(at, _)| at.elapsed() >= TTL).unwrap_or(true) {
            *cache = Some((std::time::Instant::now(), fresh.clone()));
        }
        Ok(fresh)
    }

    /// Dashboard overview, computed from a handful of list calls + metrics-server.
    async fn compute_overview(&self) -> Result<ClusterOverview, K8sError> {
        let client = self.client();
        let kubernetes_version = self.cluster.probe().await.unwrap_or_default();

        // Nodes + (best-effort) usage.
        let usage = node_usage(&client).await;
        let node_list = Api::<Node>::all(client.clone())
            .list(&ListParams::default())
            .await
            .map_err(api_err)?;
        let nodes = node_list
            .items
            .iter()
            .map(|n| {
                let name = n.metadata.name.clone().unwrap_or_default();
                let ready = n
                    .status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .map(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
                    .unwrap_or(false);
                let cap = n.status.as_ref().and_then(|s| s.capacity.as_ref());
                let cpu_cores = cap.and_then(|c| c.get("cpu")).map(|q| parse_cpu(&q.0));
                let mem_bytes = cap.and_then(|c| c.get("memory")).map(|q| parse_mem(&q.0));
                let (cpu_used, mem_used) = match usage.get(&name) {
                    Some((c, m)) => (Some(*c), Some(*m)),
                    None => (None, None),
                };
                let info = n.status.as_ref().and_then(|s| s.node_info.as_ref());
                let kubelet_version = info
                    .map(|i| i.kubelet_version.clone())
                    .filter(|s| !s.is_empty());
                let os_image = info.map(|i| i.os_image.clone()).filter(|s| !s.is_empty());
                NodeSummary {
                    name,
                    ready,
                    cpu_cores,
                    cpu_used,
                    mem_bytes,
                    mem_used,
                    kubelet_version,
                    os_image,
                }
            })
            .collect();

        // Pods by phase.
        let pods = Api::<Pod>::all(client.clone())
            .list(&ListParams::default())
            .await
            .map_err(api_err)?;
        let (mut pod_running, mut pod_pending, mut pod_failed) = (0u32, 0u32, 0u32);
        for p in &pods.items {
            match p.status.as_ref().and_then(|s| s.phase.as_deref()) {
                Some("Running") => pod_running += 1,
                Some("Pending") => pod_pending += 1,
                Some("Failed") => pod_failed += 1,
                _ => {}
            }
        }
        let pod_total = pods.items.len() as u32;

        // Namespaces.
        let namespace_count = Api::<Namespace>::all(client.clone())
            .list(&ListParams::default())
            .await
            .map(|l| l.items.len() as u32)
            .unwrap_or(0);

        // Recent warning events.
        let warnings = self.recent_warnings().await.unwrap_or_default();

        let (flux, external_secrets) = tokio::join!(
            self.rollup(Category::Flux),
            self.rollup(Category::ExternalSecrets),
        );

        Ok(ClusterOverview {
            kubernetes_version,
            nodes,
            namespace_count,
            pod_total,
            pod_running,
            pod_pending,
            pod_failed,
            warnings,
            flux,
            external_secrets,
        })
    }

    async fn recent_warnings(&self) -> Result<Vec<String>, K8sError> {
        let client = self.client();
        let lp = ListParams::default().fields("type=Warning");
        let list = Api::<Event>::all(client).list(&lp).await.map_err(api_err)?;
        let mut events: Vec<(Option<String>, String)> = list
            .items
            .into_iter()
            .map(|e| {
                let ts = e.last_timestamp.as_ref().and_then(ts_string);
                let ns = e
                    .involved_object
                    .namespace
                    .clone()
                    .map(|n| format!("{n}/"))
                    .unwrap_or_default();
                let obj = e.involved_object.name.clone().unwrap_or_default();
                (
                    ts,
                    format!(
                        "{ns}{obj}: {} {}",
                        e.reason.unwrap_or_default(),
                        e.message.unwrap_or_default()
                    ),
                )
            })
            .collect();
        events.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(events.into_iter().take(8).map(|(_, m)| m).collect())
    }

    /// Count Ready/suspended/failing for every kind in a CRD family (Flux, ESO).
    /// Lists all matching kinds concurrently (instead of sequentially) so the
    /// dashboard overview doesn't accumulate per-kind latency.
    async fn rollup(&self, category: Category) -> HealthRollup {
        let client = self.client();
        let futs = self
            .catalog
            .iter()
            .filter(|e| e.kind.category == category)
            .map(|entry| {
                let api: Api<DynamicObject> = Api::all_with(client.clone(), &entry.api_resource);
                async move {
                    let Ok(list) = api.list(&ListParams::default()).await else {
                        return Vec::new();
                    };
                    list.items
                }
            });
        let results = join_all(futs).await;
        let mut rollup = HealthRollup::default();
        for items in results {
            for obj in items {
                rollup.total += 1;
                let suspended = obj
                    .data
                    .get("spec")
                    .and_then(|s| s.get("suspend"))
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                if suspended {
                    rollup.suspended += 1;
                }
                match ready_condition(&obj.data) {
                    Some(true) => rollup.ready += 1,
                    Some(false) => rollup.failing += 1,
                    None => {}
                }
            }
        }
        rollup
    }

    // ---- mutations (M6) ---------------------------------------------------

    fn dyn_api(&self, key: &str, ns: Option<&str>) -> Result<Api<DynamicObject>, K8sError> {
        let entry = self.entry(key)?;
        Ok(make_api(
            self.client(),
            &entry.api_resource,
            entry.kind.namespaced,
            ns,
        ))
    }

    async fn merge_patch(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
        patch: serde_json::Value,
    ) -> Result<(), K8sError> {
        self.dyn_api(key, ns)?
            .patch(name, &PatchParams::default(), &Patch::Merge(patch))
            .await
            .map_err(api_err)?;
        Ok(())
    }

    pub async fn delete(&self, key: &str, ns: Option<&str>, name: &str) -> Result<(), K8sError> {
        self.dyn_api(key, ns)?
            .delete(name, &DeleteParams::default())
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

    pub async fn flux_suspend(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
        suspend: bool,
    ) -> Result<(), K8sError> {
        self.merge_patch(key, ns, name, json!({ "spec": { "suspend": suspend } }))
            .await
    }

    pub async fn flux_reconcile(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        let patch = json!({ "metadata": { "annotations": {
            "reconcile.fluxcd.io/requestedAt": now_rfc3339()
        }}});
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

    /// Whether a pod is currently Running — so its logs should be *followed* (a live
    /// stream) rather than fetched once (a finished pod produces no more output, and
    /// following it would end immediately and trigger an SSE reconnect loop). Unknown
    /// pods (not in any cache) default to following, which is safe for live ones.
    pub async fn pod_running(&self, ns: &str, name: &str) -> bool {
        match self.registry.cached_object("/v1/Pod", Some(ns), name).await {
            Some(obj) => {
                obj.data
                    .get("status")
                    .and_then(|s| s.get("phase"))
                    .and_then(|p| p.as_str())
                    == Some("Running")
            }
            None => true,
        }
    }

    /// Recent (tail) pod logs.
    pub async fn logs(
        &self,
        ns: &str,
        pod: &str,
        container: Option<String>,
        follow: bool,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, K8sError> {
        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        let lp = LogParams {
            follow,
            tail_lines: Some(500),
            timestamps: false,
            container,
            ..Default::default()
        };
        let reader = api.log_stream(pod, &lp).await.map_err(api_err)?;
        let lines = reader.lines().filter_map(|r| async move { r.ok() });
        Ok(Box::pin(lines))
    }

    /// Aggregated logs for a workload: resolve its pods by `spec.selector` and merge
    /// every pod's log stream into one, each line prefixed `pod │ `.
    pub async fn logs_workload(
        &self,
        key: &str,
        ns: &str,
        name: &str,
        follow: bool,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, K8sError> {
        let obj = self
            .dyn_api(key, Some(ns))?
            .get(name)
            .await
            .map_err(api_err)?;
        let data = serde_json::to_value(&obj).map_err(api_err)?;
        let selector = data
            .get("spec")
            .and_then(|s| s.get("selector"))
            .and_then(|sel| sel.get("matchLabels"))
            .and_then(|m| m.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| format!("{k}={v}")))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();

        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        let pods = api
            .list(&ListParams::default().labels(&selector))
            .await
            .map_err(api_err)?;

        let mut streams: Vec<Pin<Box<dyn Stream<Item = String> + Send>>> = Vec::new();
        for p in pods.items {
            let pod = p.metadata.name.unwrap_or_default();
            let lp = LogParams {
                follow,
                tail_lines: Some(200),
                timestamps: false,
                ..Default::default()
            };
            match api.log_stream(&pod, &lp).await {
                Ok(reader) => {
                    let s = reader
                        .lines()
                        .filter_map(|r| async move { r.ok() })
                        .map(move |line| format!("{pod} │ {line}"));
                    streams.push(Box::pin(s));
                }
                Err(e) => {
                    tracing::debug!("failed to open log stream for pod {pod}: {e}");
                    let msg = format!("{pod} │ [roder] failed to stream logs: {e}");
                    streams.push(Box::pin(futures::stream::once(async move { msg })));
                }
            }
        }
        Ok(Box::pin(futures::stream::select_all(streams)))
    }

    /// Get historical metrics data for a pod (CPU and memory over time).
    pub async fn pod_metrics_history(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<MetricsPoint>, K8sError> {
        Ok(self
            .registry
            .pod_metrics_history(namespace, name)
            .await
            .unwrap_or_default())
    }

    /// RBAC: which actions may the current identity take on this kind/namespace.
    /// Cached briefly so the per-detail-open `patch`+`delete` checks don't each SSAR.
    pub async fn can(&self, verb: &str, key: &str, ns: Option<&str>) -> bool {
        const TTL: std::time::Duration = std::time::Duration::from_secs(30);
        let ck = (verb.to_string(), key.to_string(), ns.map(|s| s.to_string()));
        {
            let cache = self.can_cache.read().await;
            if let Some((at, allowed)) = cache.get(&ck) {
                if at.elapsed() < TTL {
                    return *allowed;
                }
            }
        }
        let Ok(entry) = self.entry(key) else {
            return false;
        };
        let ssar = SelfSubjectAccessReview {
            spec: SelfSubjectAccessReviewSpec {
                resource_attributes: Some(ResourceAttributes {
                    verb: Some(verb.to_string()),
                    group: Some(entry.kind.group.clone()),
                    resource: Some(entry.kind.plural.clone()),
                    namespace: ns.map(|s| s.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let api: Api<SelfSubjectAccessReview> = Api::all(self.client());
        let allowed = match api.create(&PostParams::default(), &ssar).await {
            Ok(r) => r.status.map(|s| s.allowed).unwrap_or(false),
            Err(e) => {
                // Don't cache transient failures — a network blip would hide
                // all action buttons for 30 seconds on every affected resource.
                tracing::warn!("SSAR failed for {verb} on {key}: {e}");
                return false;
            }
        };
        let mut cache = self.can_cache.write().await;
        // Evict stale entries so the map doesn't grow O(verbs × kinds × namespaces).
        cache.retain(|_, (at, _)| at.elapsed() < TTL * 10);
        cache.insert(ck, (std::time::Instant::now(), allowed));
        allowed
    }

    fn entry(&self, key: &str) -> Result<&CatalogEntry, K8sError> {
        self.by_key
            .get(key)
            .ok_or_else(|| K8sError::Api(format!("unknown resource kind: {key}")))
    }
}

/// Map any displayable error into a generic `K8sError::Api`.
fn api_err<E: std::fmt::Display>(e: E) -> K8sError {
    K8sError::Api(e.to_string())
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Read a `Ready` status condition: Some(true)=Ready, Some(false)=not Ready, None=absent/unknown.
fn ready_condition(data: &serde_json::Value) -> Option<bool> {
    let conds = data.get("status")?.get("conditions")?.as_array()?;
    let ready = conds.iter().find(|c| c["type"] == "Ready")?;
    match ready["status"].as_str()? {
        "True" => Some(true),
        "False" => Some(false),
        _ => None,
    }
}
