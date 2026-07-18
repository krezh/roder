//! The server-side façade over a connected cluster: connection/catalog bootstrap,
//! object detail/watch subscription, and the mutation/read entry points used by
//! `src/server`. Each concern beyond that core lives in its own submodule, all as
//! additional `impl Backend` blocks on the one struct defined here: [`overview`]
//! (dashboard), [`mutations`] (delete/scale/restart/apply), [`flux`] (Flux-specific
//! reconcile actions), [`logs`] (pod/workload log streaming), [`exec`] (interactive
//! shell + debug containers), [`permissions`] (RBAC checks), [`sanitize`]
//! (dead pod/job sweep), and [`drain`] (cordon + evict a node's pods).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use futures::StreamExt;
use k8s_openapi::api::core::v1::{Event, Namespace};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, DynamicObject, ListParams, Patch, PatchParams};
use kube::core::PartialObjectMeta;
use kube::runtime::watcher;
use roder_core::{ClusterOverview, MetricsPoint, ObjectDetail, ObjectEvent, ResourceKind};

use crate::client::{make_api, ClusterAccess, K8sError};
use crate::discovery::{build_catalog, CatalogEntry};
use crate::informers::{InformerRegistry, WatchHandle};
use crate::project::ts_string;

mod drain;
pub use drain::DrainSession;
mod exec;
mod flux;
mod helm_release;
mod logs;
mod mutations;
mod overview;
mod permissions;
mod sanitize;
mod tree;

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

    /// Verify that the Kubernetes API is reachable with the current identity.
    pub async fn probe(&self) -> Result<String, K8sError> {
        self.cluster.probe().await
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
        let mut backoff_attempt = 0;
        loop {
            let client = (*cluster.client()).clone();
            let api: Api<PartialObjectMeta<CustomResourceDefinition>> = Api::all(client);
            // Metadata-only watch: we just need to know a CRD changed, not its
            // (potentially megabyte) schema — so this never deserializes the
            // heavy CRD bodies. The actual column harvest is `printer_columns::load`.
            let stream = watcher::watcher(api, watcher::Config::default());
            futures::pin_mut!(stream);
            while let Some(event) = stream.next().await {
                match event {
                    Err(e) => {
                        tracing::debug!("CRD watch error (self-healing): {e}");
                        let delay = crate::informers::rebuild_backoff(backoff_attempt);
                        backoff_attempt = backoff_attempt.saturating_add(1);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    // `Init` is emitted before the LIST request, so it does not
                    // prove connectivity. Any other event does.
                    Ok(watcher::Event::Init) => {}
                    Ok(_) => backoff_attempt = 0,
                }
                // Coalesce a burst of CRD events: keep draining until the stream
                // goes quiet for 2s, then rebuild once.
                loop {
                    match tokio::time::timeout(Duration::from_secs(2), stream.next()).await {
                        Ok(Some(Err(e))) => {
                            tracing::debug!("CRD watch error (self-healing): {e}");
                            let delay = crate::informers::rebuild_backoff(backoff_attempt);
                            backoff_attempt = backoff_attempt.saturating_add(1);
                            tokio::time::sleep(delay).await;
                        }
                        Ok(Some(Ok(watcher::Event::Init))) => {}
                        Ok(Some(Ok(_))) => backoff_attempt = 0,
                        Ok(None) | Err(_) => break,
                    }
                }
                rebuild_catalog(&cluster, &registry, &catalog).await;
            }
            // Stream ended; pause before rebuilding the CRD watch.
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}
