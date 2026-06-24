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

    /// Resolve the list of main (non-init) container names for a pod.
    /// Tries the informer cache first, falls back to a live API call.
    async fn pod_containers(&self, ns: &str, pod: &str) -> Vec<String> {
        if let Some(obj) = self.registry.cached_object("/v1/Pod", Some(ns), pod).await {
            if let Some(arr) = obj
                .data
                .get("spec")
                .and_then(|s| s.get("containers"))
                .and_then(|c| c.as_array())
            {
                return arr
                    .iter()
                    .filter_map(|c| c.get("name")?.as_str().map(str::to_string))
                    .collect();
            }
        }
        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        api.get(pod)
            .await
            .ok()
            .and_then(|p| p.spec)
            .map(|s| s.containers.into_iter().map(|c| c.name).collect())
            .unwrap_or_default()
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
    pub async fn inject_debug_container(
        &self,
        ns: &str,
        pod: &str,
    ) -> Result<String, K8sError> {
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
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(60);
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
                            && cs
                                .state
                                .as_ref()
                                .and_then(|s| s.running.as_ref())
                                .is_some()
                    });
                if running {
                    return Ok(name);
                }
            }
        }
    }


    /// Live pod logs as SSE. When a specific container is requested it is streamed
    /// without a prefix. When the pod has multiple containers and none was specified,
    /// all containers are streamed in parallel with `container │ ` line prefixes so
    /// the frontend can show a container pill per line.
    pub async fn logs(
        &self,
        ns: &str,
        pod: &str,
        container: Option<String>,
        follow: bool,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, K8sError> {
        let api: Api<Pod> = Api::namespaced(self.client(), ns);

        if let Some(name) = container {
            // Explicit container selection — stream it without any prefix.
            let lp = LogParams {
                follow,
                tail_lines: Some(500),
                timestamps: false,
                container: Some(name),
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

        let containers = self.pod_containers(ns, pod).await;

        if containers.len() <= 1 {
            // Single container — stream without prefix.
            let lp = LogParams {
                follow,
                tail_lines: Some(500),
                timestamps: false,
                container: containers.into_iter().next(),
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

        // Multiple containers — stream all with `container │ ` prefix.
        let mut streams: Vec<Pin<Box<dyn Stream<Item = String> + Send>>> = Vec::new();
        for name in containers {
            let lp = LogParams {
                follow,
                tail_lines: Some(500),
                timestamps: false,
                container: Some(name.clone()),
                ..Default::default()
            };
            match api.log_stream(pod, &lp).await {
                Ok(reader) => {
                    let s = reader
                        .lines()
                        .filter_map(|r| async move { r.ok() })
                        .map(move |line| format!("{name} │ {line}"));
                    streams.push(Box::pin(s));
                }
                Err(e) => {
                    tracing::debug!("failed to open log stream for container {name}: {e}");
                    let msg = format!("{name} │ [roder] failed to stream logs: {e}");
                    streams.push(Box::pin(futures::stream::once(async move { msg })));
                }
            }
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

    /// Delete all "dead" pods (matching k9s's `toastPhases`) and finished Jobs.
    /// Best-effort: individual delete failures are silently skipped.
    pub async fn sanitize(
        &self,
        namespace: Option<String>,
    ) -> Result<CleanupSummary, K8sError> {
    let pod_api: Api<Pod> = match namespace.as_deref() {
        Some(ns) => Api::namespaced(self.client(), ns),
        None => Api::all(self.client()),
    };
    let pods = pod_api.list(&ListParams::default()).await.map_err(api_err)?;
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
    let jobs = job_api.list(&ListParams::default()).await.map_err(api_err)?;
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
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            probe.join(),
        )
        .await
        {
            Ok(Ok(_)) => return shell.to_string(),
            _ => continue,
        }
    }
    "/bin/sh".to_string()
}

fn api_err<E: std::fmt::Display>(e: E) -> K8sError {
    K8sError::Api(e.to_string())
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
    match status.and_then(|s| s.phase.as_deref()) {
        Some("Succeeded") => return true,
        _ => {}
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
