use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use std::pin::Pin;

use arc_swap::ArcSwap;
use futures::future::join_all;
use futures::io::AsyncBufReadExt;
use futures::{Stream, StreamExt};
use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Event, Namespace, Node, Pod};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{
    Api, DeleteParams, DynamicObject, ListParams, LogParams, Patch, PatchParams, PostParams,
};
use kube::core::PartialObjectMeta;
use kube::runtime::watcher;
use roder_core::{
    Category, CleanupSummary, ClusterOverview, HealthRollup, MetricsPoint, NodeSummary,
    ObjectDetail, ObjectEvent, ResourceKind,
};
use serde_json::json;

use crate::client::{make_api, ClusterAccess, K8sError};
use crate::discovery::{build_catalog, CatalogEntry};
use crate::informers::{InformerRegistry, WatchHandle};
use crate::metrics::{node_usage, parse_cpu, parse_mem};
use crate::project::ts_string;

/// The resource catalog, hot-swapped when CRDs change so newly-installed
/// operators appear (and removed ones disappear) without restarting roder.
struct CatalogData {
    entries: Vec<CatalogEntry>,
    by_key: HashMap<String, CatalogEntry>,
}

impl CatalogData {
    fn new(entries: Vec<CatalogEntry>) -> Self {
        let by_key = entries
            .iter()
            .map(|e| (e.kind.key.clone(), e.clone()))
            .collect();
        Self { entries, by_key }
    }
}

type CanCacheKey = (String, String, Option<String>);

/// The server-side façade over a connected cluster: the token-passthrough client,
/// the discovered resource catalog, and the shared-informer registry.
pub struct Backend {
    cluster: Arc<ClusterAccess>,
    /// Hot-swappable catalog (entries + by-key index), rebuilt by the CRD watch.
    catalog: Arc<ArcSwap<CatalogData>>,
    registry: Arc<InformerRegistry>,
    /// Short-lived cache so rapid (re)connects don't each re-LIST the cluster.
    /// RwLock allows concurrent reads when the cache is fresh.
    overview_cache: tokio::sync::RwLock<Option<(std::time::Instant, ClusterOverview)>>,
    /// Short-TTL SelfSubjectAccessReview cache keyed (verb, key, namespace).
    /// RwLock so concurrent permission reads don't serialize.
    can_cache: tokio::sync::RwLock<HashMap<CanCacheKey, (std::time::Instant, bool)>>,
    /// Signature of the last-streamed attempt per (namespace, pod, container), so a
    /// crashed/stuck container's static output is shown once per attempt instead of
    /// being replayed every time a client reconnects to a still-broken pod.
    log_seen: tokio::sync::RwLock<HashMap<(String, String, String), String>>,
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
        // Harvest CRD-declared printer columns; shared by the catalog (headers)
        // and the informers (cell projection). Empty if CRDs aren't listable.
        let columns = Arc::new(crate::printer_columns::load(&client).await);
        let entries = build_catalog(&client, &columns).await?;
        let catalog = Arc::new(ArcSwap::from_pointee(CatalogData::new(entries)));
        let registry = InformerRegistry::new(cluster.clone(), columns);
        // Keep the catalog + columns live: watch CRDs and rebuild on change, so
        // new operators show up and changed printer columns reflow without a
        // restart (active tables re-render in place).
        spawn_crd_watch(cluster.clone(), registry.clone(), catalog.clone());
        Ok(Self {
            cluster,
            catalog,
            registry,
            overview_cache: tokio::sync::RwLock::new(None),
            can_cache: tokio::sync::RwLock::new(HashMap::new()),
            log_seen: tokio::sync::RwLock::new(HashMap::new()),
        })
    }

    /// Swap in a refreshed ID token (called by the refresh task).
    pub fn set_token(&self, id_token: &str) -> Result<(), K8sError> {
        self.cluster.set_token(id_token)
    }

    /// The current `kube::Client` (a cheap clone of the hot-swappable inner Arc).
    pub fn client(&self) -> kube::Client {
        (*self.cluster.client()).clone()
    }

    /// The browsable resource catalog as surfaced to the UI.
    pub fn kinds(&self) -> Vec<ResourceKind> {
        self.catalog
            .load()
            .entries
            .iter()
            .map(|e| e.kind.clone())
            .collect()
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
                let entry = self.entry(key)?;
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
        if cache
            .as_ref()
            .map(|(at, _)| at.elapsed() >= TTL)
            .unwrap_or(true)
        {
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
        let catalog = self.catalog.load();
        let futs = catalog
            .entries
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

    /// `flux reconcile helmrelease --force`: force a one-off Helm install/upgrade.
    pub async fn flux_force(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        let ts = now_rfc3339();
        let patch = json!({ "metadata": { "annotations": {
            "reconcile.fluxcd.io/requestedAt": ts,
            "reconcile.fluxcd.io/forceAt": ts,
        }}});
        self.merge_patch(key, ns, name, patch).await
    }

    /// `flux reconcile helmrelease --reset`: reset the failure count on a stuck HelmRelease.
    pub async fn flux_reset(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        let ts = now_rfc3339();
        let patch = json!({ "metadata": { "annotations": {
            "reconcile.fluxcd.io/requestedAt": ts,
            "reconcile.fluxcd.io/resetAt": ts,
        }}});
        self.merge_patch(key, ns, name, patch).await
    }

    /// `flux reconcile <kind> --with-source`: reconcile the referenced source
    /// (GitRepository/OCIRepository/HelmRepository/Bucket/HelmChart) first,
    /// then the object itself.
    pub async fn flux_reconcile_with_source(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        let obj = self.dyn_api(key, ns)?.get(name).await.map_err(api_err)?;
        let data = serde_json::to_value(&obj).map_err(api_err)?;
        let spec = data
            .get("spec")
            .ok_or_else(|| K8sError::Api("resource has no spec".into()))?;
        let source = extract_source_ref(spec)
            .ok_or_else(|| K8sError::Api("resource has no sourceRef to reconcile".into()))?;
        let source_entry = self.entry_by_kind(&source.kind)?;
        let source_ns = source.namespace.as_deref().or(ns);
        self.flux_reconcile(&source_entry.kind.key, source_ns, &source.name)
            .await?;
        self.flux_reconcile(key, ns, name).await
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

    /// Whether a pod is not (yet) in a terminal phase, so its logs should be
    /// *followed* (a live stream) rather than fetched once. Only `Succeeded`/`Failed`
    /// pods are done for good; everything else — including `Pending` (image still
    /// pulling, init containers running) — may still produce its first log line, or
    /// crash, the moment the container starts. Unknown pods (not in any cache)
    /// default to following, which is safe for live ones.
    pub async fn pod_active(&self, ns: &str, name: &str) -> bool {
        match self.registry.cached_object("/v1/Pod", Some(ns), name).await {
            Some(obj) => !matches!(
                obj.data
                    .get("status")
                    .and_then(|s| s.get("phase"))
                    .and_then(|p| p.as_str()),
                Some("Succeeded") | Some("Failed")
            ),
            None => true,
        }
    }

    /// A pod's full object as JSON. Tries the informer cache first, falls back to
    /// a live API call — used for the `spec`/`status` introspection the log
    /// container list needs (informer objects and `Pod` don't share a type).
    async fn pod_json(&self, ns: &str, pod: &str) -> Option<serde_json::Value> {
        if let Some(obj) = self.registry.cached_object("/v1/Pod", Some(ns), pod).await {
            return Some(obj.data);
        }
        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        serde_json::to_value(api.get(pod).await.ok()?).ok()
    }

    /// Every container to stream logs for, in the order the pod runs them: init
    /// containers first, then main containers — paired with whether that
    /// container's output can only be reached via `previous`. Containers that
    /// haven't run at all yet (still waiting their turn) are omitted, and a
    /// container whose crash/attempt was already streamed before is omitted too —
    /// see [`Backend::already_reported`].
    async fn pod_log_containers(&self, ns: &str, pod: &str) -> Vec<(String, bool)> {
        let Some(data) = self.pod_json(ns, pod).await else {
            return Vec::new();
        };
        let names = |field: &str| -> Vec<String> {
            data.get("spec")
                .and_then(|s| s.get(field))
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| c.get("name")?.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut out = Vec::new();
        for name in names("initContainers")
            .into_iter()
            .chain(names("containers"))
        {
            match container_log_plan(&data, &name) {
                LogPlan::Skip => {}
                LogPlan::Live => out.push((name, false)),
                LogPlan::Static {
                    previous,
                    signature,
                } => {
                    let key = (ns.to_string(), pod.to_string(), name.clone());
                    if !self.already_reported(key, signature).await {
                        out.push((name, previous));
                    }
                }
            }
        }
        out
    }

    /// Whether `signature` for `(namespace, pod, container)` was already reported
    /// last time — and if not, remembers it. Used so a crashed/stuck container's
    /// static output isn't re-sent on every reconnect to a still-broken pod.
    async fn already_reported(&self, key: (String, String, String), signature: String) -> bool {
        let mut seen = self.log_seen.write().await;
        if seen.get(&key) == Some(&signature) {
            true
        } else {
            seen.insert(key, signature);
            false
        }
    }

    /// Open one container's log stream, tagging each line with `prefix` (empty for
    /// a single-container view). On failure, produce a one-line placeholder
    /// instead of erroring the whole pane — deduped the same way as
    /// [`Backend::pod_log_containers`], so a container that's still stuck doesn't
    /// repeat the same message on every reconnect.
    async fn open_container_log(
        &self,
        ns: &str,
        pod: &str,
        name: &str,
        previous: bool,
        follow: bool,
        prefix: &str,
    ) -> Pin<Box<dyn Stream<Item = String> + Send>> {
        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        let lp = LogParams {
            follow,
            tail_lines: Some(500),
            timestamps: false,
            container: Some(name.to_string()),
            previous,
            ..Default::default()
        };
        match api.log_stream(pod, &lp).await {
            Ok(reader) => {
                let prefix = prefix.to_string();
                Box::pin(
                    reader
                        .lines()
                        .filter_map(|r| async move { r.ok() })
                        .map(move |line| format!("{prefix}{line}")),
                )
            }
            Err(e) => {
                let key = (ns.to_string(), pod.to_string(), name.to_string());
                let msg = e.to_string();
                if self.already_reported(key, msg.clone()).await {
                    return Box::pin(futures::stream::empty());
                }
                tracing::debug!("failed to open log stream for container {name}: {msg}");
                let prefix = prefix.to_string();
                Box::pin(futures::stream::once(async move {
                    format!("{prefix}[roder] failed to stream logs: {msg}")
                }))
            }
        }
    }

    /// Open an interactive exec session into a pod container. Returns an
    /// `AttachedProcess` whose stdin/stdout can be proxied over a WebSocket.
    /// Probes for the best available shell (bash › sh › ash) before opening
    /// the interactive session.
    pub async fn exec(
        &self,
        ns: &str,
        pod: &str,
        container: Option<&str>,
    ) -> Result<kube::api::AttachedProcess, K8sError> {
        use kube::api::AttachParams;
        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        let shell = detect_shell(&api, pod, container).await;
        let mut ap = AttachParams::default()
            .stdin(true)
            .stdout(true)
            .stderr(false)
            .tty(true);
        if let Some(c) = container {
            ap = ap.container(c);
        }
        api.exec(pod, vec![shell.as_str()], &ap)
            .await
            .map_err(api_err)
    }

    /// Inject a `nicolaka/netshoot` ephemeral container into `pod`, wait for it
    /// to reach Running, and return its name for use with [`exec`].
    pub async fn inject_debug_container(&self, ns: &str, pod: &str) -> Result<String, K8sError> {
        let api: Api<Pod> = Api::namespaced(self.client(), ns);

        // Fetch current ephemeral containers; the patch replaces the whole
        // array, so we must include any that already exist.
        let pod_obj = api.get(pod).await.map_err(api_err)?;
        let mut containers: Vec<serde_json::Value> = pod_obj
            .spec
            .and_then(|s| s.ephemeral_containers)
            .map(|ecs| {
                ecs.into_iter()
                    .filter_map(|ec| serde_json::to_value(ec).ok())
                    .collect()
            })
            .unwrap_or_default();

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        // Build the set of names already in use so we never push a duplicate.
        // Ephemeral containers are permanent once added; collisions cause a 422.
        let existing: std::collections::HashSet<String> = containers
            .iter()
            .filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(str::to_owned))
            .collect();
        let name = (0u32..)
            .map(|i| {
                if i == 0 {
                    format!("debug-{:06x}", ts & 0x00ff_ffff)
                } else {
                    format!("debug-{:06x}-{i}", ts & 0x00ff_ffff)
                }
            })
            .find(|n| !existing.contains(n))
            .expect("infinite iterator always yields a free name");

        containers.push(serde_json::json!({
            "name": name,
            "image": "nicolaka/netshoot",
            "stdin": true,
            "tty": true,
            "terminationMessagePolicy": "File"
        }));

        api.patch_subresource::<serde_json::Value>(
            "ephemeralcontainers",
            pod,
            &PatchParams::default(),
            &Patch::Merge(serde_json::json!({
                "spec": { "ephemeralContainers": containers }
            })),
        )
        .await
        .map_err(api_err)?;

        // Poll until the container reaches Running (up to 60 s).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            if std::time::Instant::now() > deadline {
                return Err(api_err(format!(
                    "debug container {name} did not start within 60s"
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            // Swallow transient API errors (429, 503, blip) rather than aborting;
            // the container was already injected and cannot be removed, so a brief
            // apiserver hiccup during polling should not strand it permanently.
            if let Ok(p) = api.get(pod).await {
                let running = p
                    .status
                    .and_then(|s| s.ephemeral_container_statuses)
                    .unwrap_or_default()
                    .iter()
                    .any(|cs| {
                        cs.name == name
                            && cs.state.as_ref().and_then(|s| s.running.as_ref()).is_some()
                    });
                if running {
                    return Ok(name);
                }
            }
        }
    }

    /// Live pod logs as SSE. When a specific container is requested it is streamed
    /// without a prefix. Otherwise every container in the pod is streamed — init
    /// containers first (in spec order), then main containers — with `container │ `
    /// line prefixes, so an init error (or a container that hasn't started yet) is
    /// visible immediately instead of only once the main container finally starts.
    /// A container that hasn't run at all yet is omitted rather than reported as an
    /// error (it's just waiting its turn), and a crashed/stuck container's output
    /// is only ever streamed once per attempt — see [`Backend::pod_log_containers`].
    pub async fn logs(
        &self,
        ns: &str,
        pod: &str,
        container: Option<String>,
        follow: bool,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, K8sError> {
        if let Some(name) = container {
            // Explicit container selection — stream it without any prefix. (No UI
            // path sends this today; kept for completeness / direct API use.)
            let previous = match self.pod_json(ns, pod).await {
                Some(data) => {
                    matches!(container_log_plan(&data, &name), LogPlan::Static { previous, .. } if previous)
                }
                None => false,
            };
            let api: Api<Pod> = Api::namespaced(self.client(), ns);
            let lp = LogParams {
                follow,
                tail_lines: Some(500),
                timestamps: false,
                container: Some(name),
                previous,
                ..Default::default()
            };
            let lines = api
                .log_stream(pod, &lp)
                .await
                .map_err(api_err)?
                .lines()
                .filter_map(|r| async move { r.ok() });
            return Ok(Box::pin(lines));
        }

        let containers = self.pod_log_containers(ns, pod).await;
        if containers.is_empty() {
            // Nothing to show right now — every container is either still waiting
            // its turn, or its last attempt was already streamed. An empty stream
            // ends at once; the caller's `follow`/`eof` handling decides whether to
            // retry (see `server::api::logs`).
            return Ok(Box::pin(futures::stream::empty()));
        }

        // Prefix lines only when merging more than one container, matching the
        // pre-existing single-container (no pill) presentation.
        let prefixed = containers.len() > 1;
        let mut streams: Vec<Pin<Box<dyn Stream<Item = String> + Send>>> = Vec::new();
        for (name, previous) in containers {
            let prefix = if prefixed {
                format!("{name} │ ")
            } else {
                String::new()
            };
            streams.push(
                self.open_container_log(ns, pod, &name, previous, follow, &prefix)
                    .await,
            );
        }
        Ok(Box::pin(futures::stream::select_all(streams)))
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

        // matchExpressions-only selectors (no matchLabels) yield an empty string,
        // which kube treats as "no filter" and would list every pod in the namespace.
        if selector.is_empty() {
            return Err(K8sError::Api(
                "workload uses a matchExpressions-only selector; per-workload log aggregation is not supported for this resource".into(),
            ));
        }

        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        let pods = api
            .list(&ListParams::default().labels(&selector))
            .await
            .map_err(api_err)?;

        let mut streams: Vec<Pin<Box<dyn Stream<Item = String> + Send>>> = Vec::new();
        for p in pods.items {
            let pod = p.metadata.name.unwrap_or_default();
            let container = if p.spec.as_ref().map(|s| s.containers.len()).unwrap_or(0) > 1 {
                p.spec
                    .and_then(|s| s.containers.into_iter().next())
                    .map(|c| c.name)
            } else {
                None
            };
            let lp = LogParams {
                follow,
                tail_lines: Some(200),
                timestamps: false,
                container,
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

    pub async fn kind_stats(
        &self,
        namespace: Option<&str>,
    ) -> std::collections::HashMap<String, roder_core::KindStats> {
        self.registry.kind_stats(namespace).await
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

    /// Look up a catalog entry by key. Returns an owned clone because the catalog
    /// is hot-swappable (the `ArcSwap` guard is only valid transiently).
    fn entry(&self, key: &str) -> Result<CatalogEntry, K8sError> {
        self.catalog
            .load()
            .by_key
            .get(key)
            .cloned()
            .ok_or_else(|| K8sError::Api(format!("unknown resource kind: {key}")))
    }

    /// Resolve a Flux sourceRef's `kind` (e.g. "GitRepository") to a catalog
    /// entry, without needing its `apiVersion` — the same way Flux's own
    /// controllers resolve sourceRef generically across `source.toolkit.fluxcd.io`.
    fn entry_by_kind(&self, kind: &str) -> Result<CatalogEntry, K8sError> {
        self.catalog
            .load()
            .entries
            .iter()
            .find(|e| e.kind.group.ends_with("fluxcd.io") && e.kind.kind == kind)
            .cloned()
            .ok_or_else(|| K8sError::Api(format!("no Flux source kind found: {kind}")))
    }

    /// Delete all "dead" pods (matching k9s's `toastPhases`) and finished Jobs.
    /// Best-effort: individual delete failures are silently skipped.
    pub async fn sanitize(&self, namespace: Option<String>) -> Result<CleanupSummary, K8sError> {
        let pod_api: Api<Pod> = match namespace.as_deref() {
            Some(ns) => Api::namespaced(self.client(), ns),
            None => Api::all(self.client()),
        };
        let pods = pod_api
            .list(&ListParams::default())
            .await
            .map_err(api_err)?;
        let mut pods_deleted = 0usize;
        for pod in pods.items.iter().filter(|p| is_toast_pod(p)) {
            let name = pod.metadata.name.as_deref().unwrap_or_default();
            let ns = pod.metadata.namespace.as_deref().unwrap_or_default();
            if Api::<Pod>::namespaced(self.client(), ns)
                .delete(name, &DeleteParams::default())
                .await
                .is_ok()
            {
                pods_deleted += 1;
            }
        }

        let job_api: Api<Job> = match namespace.as_deref() {
            Some(ns) => Api::namespaced(self.client(), ns),
            None => Api::all(self.client()),
        };
        let jobs = job_api
            .list(&ListParams::default())
            .await
            .map_err(api_err)?;
        let mut jobs_deleted = 0usize;
        for job in jobs.items.iter().filter(|j| is_finished_job(j)) {
            let name = job.metadata.name.as_deref().unwrap_or_default();
            let ns = job.metadata.namespace.as_deref().unwrap_or_default();
            if Api::<Job>::namespaced(self.client(), ns)
                .delete(name, &DeleteParams::default())
                .await
                .is_ok()
            {
                jobs_deleted += 1;
            }
        }

        Ok(CleanupSummary {
            pods_deleted,
            jobs_deleted,
        })
    }
}

/// Map any displayable error into a generic `K8sError::Api`.
/// Probe the container to find the best available interactive shell.
/// Starts each candidate with no arguments, no stdin, no TTY — the shell reads
/// EOF and exits immediately without loading interactive configs (avoids
/// oh-my-zsh and similar frameworks breaking the probe). Whichever join()
/// returns Ok first wins. Falls back to `/bin/sh` if all probes fail or time out.
async fn detect_shell(api: &Api<Pod>, pod: &str, container: Option<&str>) -> String {
    use kube::api::AttachParams;
    for shell in ["/bin/bash", "/bin/ash", "/bin/zsh", "/bin/sh"] {
        let mut ap = AttachParams::default()
            .stdin(false)
            .stdout(false)
            .stderr(false)
            .tty(false);
        if let Some(c) = container {
            ap = ap.container(c);
        }
        let Ok(probe) = api.exec(pod, vec![shell], &ap).await else {
            continue;
        };
        match tokio::time::timeout(std::time::Duration::from_secs(2), probe.join()).await {
            Ok(Ok(_)) => return shell.to_string(),
            _ => continue,
        }
    }
    "/bin/sh".to_string()
}

fn api_err<E: std::fmt::Display>(e: E) -> K8sError {
    K8sError::Api(e.to_string())
}

/// Whether `name` (an init or main container) is currently `waiting` — i.e. not
/// running and not freshly terminated, so the kubelet has nothing to show for the
/// *current* attempt and `previous` logs are the only way to see its last output.
/// Whether/how `name` (an init or main container) should be logged: `None` if it
/// simply hasn't had its turn yet — no status reported, or `waiting` with no prior
/// attempt — which isn't an error, just sequencing, so it shouldn't synthesize one.
/// `Some(previous)` if it has run at least once, with `previous = true` when the
/// *current* attempt has nothing to show (mid-backoff after a crash) and the last
/// attempt's output is only reachable via the `previous` log.
/// How to log a container given its current status.
enum LogPlan {
    /// Hasn't run at all yet — just waiting its turn; not an error, don't report it.
    Skip,
    /// Currently running — stream it live.
    Live,
    /// A finished attempt: terminated (and not yet superseded by a new attempt),
    /// or — if already backing off toward a restart — only reachable via
    /// `previous`. `signature` fingerprints the attempt so a repeat fetch of the
    /// same crash can be recognized and skipped.
    Static { previous: bool, signature: String },
}

fn container_log_plan(pod_data: &serde_json::Value, name: &str) -> LogPlan {
    let Some(status) = ["initContainerStatuses", "containerStatuses"]
        .iter()
        .find_map(|field| {
            pod_data
                .get("status")?
                .get(*field)?
                .as_array()?
                .iter()
                .find(|c| c.get("name").and_then(|n| n.as_str()) == Some(name))
        })
    else {
        return LogPlan::Skip;
    };
    let state = status.get("state");
    if state.and_then(|s| s.get("running")).is_some() {
        return LogPlan::Live;
    }
    if let Some(terminated) = state.and_then(|s| s.get("terminated")) {
        return LogPlan::Static {
            previous: false,
            signature: attempt_signature(terminated),
        };
    }
    // `waiting`: nothing current to show; fall back to the last completed attempt.
    match status.get("lastState").and_then(|s| s.get("terminated")) {
        Some(terminated) => LogPlan::Static {
            previous: true,
            signature: attempt_signature(terminated),
        },
        None => LogPlan::Skip,
    }
}

/// A stable fingerprint for one container attempt (exit code + time it ended), so
/// a repeat fetch of the same crash can be recognized and skipped.
fn attempt_signature(terminated: &serde_json::Value) -> String {
    let code = terminated
        .get("exitCode")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let at = terminated
        .get("finishedAt")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    format!("{code}@{at}")
}

/// Matches k9s's `toastPhases`: pods that are dead or permanently stuck.
/// Skips pods that already have a deletion timestamp (already being removed).
fn is_toast_pod(pod: &Pod) -> bool {
    if pod.metadata.deletion_timestamp.is_some() {
        return false;
    }
    let status = pod.status.as_ref();
    if status.and_then(|s| s.reason.as_deref()) == Some("Evicted") {
        return true;
    }
    if let Some("Succeeded") = status.and_then(|s| s.phase.as_deref()) {
        return true;
    }
    let cs = status
        .and_then(|s| s.container_statuses.as_deref())
        .unwrap_or(&[]);
    let ics = status
        .and_then(|s| s.init_container_statuses.as_deref())
        .unwrap_or(&[]);
    cs.iter().chain(ics).any(|c| {
        if let Some(w) = c.state.as_ref().and_then(|s| s.waiting.as_ref()) {
            matches!(
                w.reason.as_deref(),
                Some(
                    "CrashLoopBackOff"
                        | "Error"
                        | "ImagePullBackOff"
                        | "ErrImagePull"
                        | "ContainerStatusUnknown"
                )
            )
        } else if let Some(t) = c.state.as_ref().and_then(|s| s.terminated.as_ref()) {
            t.reason.as_deref() == Some("OOMKilled")
        } else {
            false
        }
    })
}

fn is_finished_job(job: &Job) -> bool {
    job.status
        .as_ref()
        .and_then(|s| s.conditions.as_deref())
        .unwrap_or(&[])
        .iter()
        .any(|c| matches!(c.type_.as_str(), "Complete" | "Failed") && c.status == "True")
}

/// Re-discover the catalog and re-harvest CRD printer columns, swapping both in
/// and asking the registry to re-project any active informer whose columns
/// changed. Best-effort: a failed rebuild leaves the previous catalog in place.
async fn rebuild_catalog(
    cluster: &Arc<ClusterAccess>,
    registry: &Arc<InformerRegistry>,
    catalog: &Arc<ArcSwap<CatalogData>>,
) {
    let client = (*cluster.client()).clone();
    let columns = Arc::new(crate::printer_columns::load(&client).await);
    match build_catalog(&client, &columns).await {
        Ok(entries) => {
            catalog.store(Arc::new(CatalogData::new(entries)));
            registry.refresh_columns(columns).await;
            tracing::debug!("catalog + printer columns refreshed after CRD change");
        }
        Err(e) => tracing::debug!("catalog refresh skipped: {e}"),
    }
}

/// Watch `CustomResourceDefinition`s and rebuild the catalog/columns on change,
/// so new operators appear and changed printer columns reflow live — no restart.
/// Events are debounced (operators often churn CRDs in bursts) into one rebuild.
fn spawn_crd_watch(
    cluster: Arc<ClusterAccess>,
    registry: Arc<InformerRegistry>,
    catalog: Arc<ArcSwap<CatalogData>>,
) {
    tokio::spawn(async move {
        loop {
            let client = (*cluster.client()).clone();
            let api: Api<PartialObjectMeta<CustomResourceDefinition>> = Api::all(client);
            // Metadata-only watch: we just need to know a CRD changed, not its
            // (potentially megabyte) schema — so this never deserializes the
            // heavy CRD bodies. The actual column harvest is `printer_columns::load`.
            let stream = watcher::watcher(api, watcher::Config::default());
            futures::pin_mut!(stream);
            while let Some(event) = stream.next().await {
                if let Err(e) = event {
                    tracing::debug!("CRD watch error (self-healing): {e}");
                    continue;
                }
                // Coalesce a burst of CRD events: keep draining until the stream
                // goes quiet for 2s, then rebuild once.
                while let Ok(Some(_)) =
                    tokio::time::timeout(Duration::from_secs(2), stream.next()).await
                {
                }
                rebuild_catalog(&cluster, &registry, &catalog).await;
            }
            // Stream ended; pause before rebuilding the CRD watch.
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
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

/// A Flux source reference resolved from a reconciling object's spec.
struct SourceRef {
    kind: String,
    name: String,
    namespace: Option<String>,
}

/// Find the sourceRef a Kustomization or HelmRelease reconciles against, trying
/// each field Flux supports in turn: `spec.sourceRef` (Kustomization),
/// `spec.chart.spec.sourceRef` (HelmRelease templated chart), and
/// `spec.chartRef` (HelmRelease direct OCIRepository/HelmChart reference).
fn extract_source_ref(spec: &serde_json::Value) -> Option<SourceRef> {
    const PATHS: &[&[&str]] = &[
        &["sourceRef"],
        &["chart", "spec", "sourceRef"],
        &["chartRef"],
    ];
    for path in PATHS {
        let mut cur = Some(spec);
        for seg in *path {
            cur = cur.and_then(|c| c.get(seg));
        }
        let Some(cur) = cur else { continue };
        let kind = cur.get("kind").and_then(|v| v.as_str());
        let name = cur.get("name").and_then(|v| v.as_str());
        if let (Some(kind), Some(name)) = (kind, name) {
            return Some(SourceRef {
                kind: kind.to_string(),
                name: name.to_string(),
                namespace: cur
                    .get("namespace")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            });
        }
    }
    None
}

#[cfg(test)]
mod source_ref_tests {
    use super::extract_source_ref;
    use serde_json::json;

    #[test]
    fn kustomization_source_ref() {
        let spec = json!({ "sourceRef": { "kind": "GitRepository", "name": "flux-system" } });
        let sr = extract_source_ref(&spec).expect("source ref");
        assert_eq!(sr.kind, "GitRepository");
        assert_eq!(sr.name, "flux-system");
        assert_eq!(sr.namespace, None);
    }

    #[test]
    fn kustomization_source_ref_with_namespace() {
        let spec = json!({ "sourceRef": {
            "kind": "GitRepository", "name": "podinfo", "namespace": "apps"
        }});
        let sr = extract_source_ref(&spec).expect("source ref");
        assert_eq!(sr.namespace.as_deref(), Some("apps"));
    }

    #[test]
    fn helmrelease_templated_chart_source_ref() {
        let spec = json!({ "chart": { "spec": {
            "chart": "podinfo",
            "sourceRef": { "kind": "HelmRepository", "name": "podinfo" }
        }}});
        let sr = extract_source_ref(&spec).expect("source ref");
        assert_eq!(sr.kind, "HelmRepository");
        assert_eq!(sr.name, "podinfo");
    }

    #[test]
    fn helmrelease_direct_chart_ref() {
        let spec = json!({ "chartRef": { "kind": "OCIRepository", "name": "podinfo" } });
        let sr = extract_source_ref(&spec).expect("source ref");
        assert_eq!(sr.kind, "OCIRepository");
        assert_eq!(sr.name, "podinfo");
    }

    #[test]
    fn prefers_source_ref_over_chart_ref_when_both_present() {
        // Shouldn't happen in practice, but sourceRef (Kustomization's own
        // field) should win if a spec somehow has both.
        let spec = json!({
            "sourceRef": { "kind": "GitRepository", "name": "a" },
            "chartRef": { "kind": "OCIRepository", "name": "b" },
        });
        let sr = extract_source_ref(&spec).expect("source ref");
        assert_eq!(sr.kind, "GitRepository");
    }

    #[test]
    fn none_when_no_source_ref_present() {
        assert!(extract_source_ref(&json!({ "suspend": false })).is_none());
    }

    #[test]
    fn none_when_source_ref_missing_required_fields() {
        assert!(extract_source_ref(&json!({ "sourceRef": { "kind": "GitRepository" } })).is_none());
    }
}
