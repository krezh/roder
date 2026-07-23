//! The per-user façade over a connected cluster: the user's token-passthrough
//! `ClusterAccess`, a per-user `InformerRegistry`, and per-user small caches
//! (overview, `can`, log-dedup). Discovery/catalog/CRD printer columns/metrics
//! enrichment are NOT owned here — they live once per cluster on
//! [`crate::shared::SharedCluster`] and are shared by every user's `Backend`.
//! Each concern beyond that core lives in its own submodule, all as
//! additional `impl Backend` blocks on the one struct defined here: [`overview`]
//! (dashboard), [`mutations`] (delete/scale/restart/apply), [`flux`] (Flux-specific
//! reconcile actions), [`logs`] (pod/workload log streaming), [`exec`] (interactive
//! shell + debug containers), [`permissions`] (RBAC checks), [`sanitize`]
//! (dead pod/job sweep), and [`drain`] (cordon + evict a node's pods).

use std::collections::HashMap;
use std::sync::Arc;

use k8s_openapi::api::core::v1::{Event, Namespace};
use kube::api::{Api, DynamicObject, ListParams, Patch, PatchParams};
use roder_core::{ClusterOverview, MetricsPoint, ObjectDetail, ObjectEvent, ResourceKind};

use crate::client::{make_api, ClusterAccess, K8sError};
use crate::discovery::CatalogEntry;
use crate::informers::{InformerRegistry, WatchHandle};
use crate::project::ts_string;
use crate::shared::SharedCluster;

mod drain;
pub use drain::DrainSession;
mod exec;
mod flux;
mod helm_release;
mod kopiur;
mod logs;
mod mutations;
mod overview;
mod permissions;
mod sanitize;
mod tree;

type CanCacheKey = (String, String, Option<String>);

/// The per-user façade over a connected cluster: the user's token-passthrough
/// client, its own informer registry, and small per-user caches. Catalog,
/// CRD printer columns, and metrics/PVC enrichment are read from `shared`.
pub struct Backend {
    /// The USER's token-passthrough client (RBAC is the calling user's identity).
    cluster: Arc<ClusterAccess>,
    /// The cluster's SA-owned catalog/columns/enrichment, shared with every
    /// other user's `Backend` on this cluster.
    shared: Arc<SharedCluster>,
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
    fn from_parts(cluster: Arc<ClusterAccess>, shared: Arc<SharedCluster>) -> Self {
        let registry = InformerRegistry::new(
            cluster.clone(),
            shared.enrichment(),
            shared.columns(),
            shared.subscribe_columns(),
        );
        Self {
            cluster,
            shared,
            registry,
            overview_cache: tokio::sync::RwLock::new(None),
            can_cache: tokio::sync::RwLock::new(HashMap::new()),
            log_seen: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Connect as the given user (OIDC ID token) against a cluster whose
    /// discovery/columns/enrichment are already running on `shared`.
    pub async fn connect_with_token(
        id_token: &str,
        shared: Arc<SharedCluster>,
    ) -> Result<Self, K8sError> {
        let cluster = Arc::new(ClusterAccess::connect_with_token(id_token).await?);
        Ok(Self::from_parts(cluster, shared))
    }

    /// Connect using the inferred kubeconfig/in-cluster default credentials
    /// (no OIDC token). Used only for the dev-mode backend, where auth is
    /// bypassed entirely and there is no per-user bearer token to pass
    /// through.
    pub async fn connect_with_default(shared: Arc<SharedCluster>) -> Result<Self, K8sError> {
        let cluster = Arc::new(ClusterAccess::connect_with_default().await?);
        Ok(Self::from_parts(cluster, shared))
    }

    /// Build a `Backend` from already-connected parts, for tests that don't
    /// need real cluster I/O. `pub` (not `#[cfg(test)]`) so the `roder` server
    /// crate's own tests (e.g. `server::backends`) can build one across the
    /// crate boundary — `cfg(test)` is per-crate and wouldn't be visible there.
    #[doc(hidden)]
    pub fn from_parts_for_test(cluster: Arc<ClusterAccess>, shared: Arc<SharedCluster>) -> Self {
        Self::from_parts(cluster, shared)
    }

    /// Swap in a refreshed ID token (called by the refresh task).
    pub fn set_token(&self, id_token: &str) -> Result<(), K8sError> {
        self.cluster.set_token(id_token)
    }

    /// Whether this user currently has any live SSE subscribers on any of
    /// their active informers. Used by `BackendRegistry`'s idle-eviction
    /// reaper and soft LRU cap: a subject with an open, actively-streaming
    /// dashboard tab must never be evicted, regardless of how long it's been
    /// since their last HTTP request.
    pub async fn has_active_subscribers(&self) -> bool {
        self.registry.has_active_subscribers().await
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
        self.shared.kinds()
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
            .shared
            .enrichment()
            .pod_metrics_history(namespace, name)
            .await
            .unwrap_or_default())
    }

    /// Look up a catalog entry by key. Returns an owned clone because the catalog
    /// is hot-swappable (the `ArcSwap` guard is only valid transiently).
    fn entry(&self, key: &str) -> Result<CatalogEntry, K8sError> {
        self.shared.entry(key)
    }

    /// Resolve a Flux sourceRef's `kind` (e.g. "GitRepository") to a catalog
    /// entry, without needing its `apiVersion` — the same way Flux's own
    /// controllers resolve sourceRef generically across `source.toolkit.fluxcd.io`.
    fn entry_by_kind(&self, kind: &str) -> Result<CatalogEntry, K8sError> {
        self.shared.entry_by_kind(kind)
    }
}

/// Map any displayable error into a generic `K8sError::Api`.
fn api_err<E: std::fmt::Display>(e: E) -> K8sError {
    K8sError::Api(e.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiErrorDisposition {
    NotFound,
    Retryable,
    Permanent,
}

fn classify_status(code: u16) -> ApiErrorDisposition {
    match code {
        404 => ApiErrorDisposition::NotFound,
        429 | 500..=599 => ApiErrorDisposition::Retryable,
        _ => ApiErrorDisposition::Permanent,
    }
}

fn classify_kube_error(error: &kube::Error) -> ApiErrorDisposition {
    match error {
        kube::Error::Api(status) => classify_status(status.code),
        kube::Error::HyperError(_) | kube::Error::Service(_) => ApiErrorDisposition::Retryable,
        _ => ApiErrorDisposition::Permanent,
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ClusterAccess;
    use crate::shared::SharedCluster;

    #[tokio::test]
    async fn backend_constructor_reuses_supplied_cluster_and_shared_state() {
        let shared = SharedCluster::for_test();
        let cluster = ClusterAccess::for_test();
        let backend = Backend::from_parts_for_test(cluster.clone(), shared.clone());
        assert!(Arc::ptr_eq(&backend.cluster, &cluster));
        assert!(Arc::ptr_eq(&backend.shared, &shared));
        assert!(backend.overview_cache.read().await.is_none());
        assert!(backend.can_cache.read().await.is_empty());
        assert!(backend.log_seen.read().await.is_empty());
    }
}
