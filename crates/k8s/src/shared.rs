//! The ServiceAccount-owned cluster layer, shared across every per-user session
//! on the same cluster: the SA [`ClusterAccess`], the hot-swapped resource
//! catalog + CRD printer columns (rebuilt on CRD change), and the shared
//! [`Enrichment`] metrics/PVC caches. There is one `SharedCluster` per cluster;
//! per-user `InformerRegistry`s (owned by `backend::Backend`) read
//! discovery/columns/enrichment from here instead of each re-discovering and
//! re-scraping independently.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use futures::StreamExt;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::Api;
use kube::core::PartialObjectMeta;
use kube::runtime::watcher;
use roder_core::ResourceKind;
use tokio::sync::broadcast;

use crate::client::{ClusterAccess, K8sError};
use crate::discovery::{build_catalog, CatalogEntry};
use crate::informers::{rebuild_backoff, Enrichment};
use crate::printer_columns::ColumnMap;

/// The resource catalog, hot-swapped when CRDs change so newly-installed
/// operators appear (and removed ones disappear) without restarting roder.
///
/// Lives here (rather than in `backend`) because catalog ownership is hoisted
/// onto the shared, SA-owned layer; `backend::Backend` reads it via
/// [`SharedCluster::entry`]/[`SharedCluster::entry_by_kind`]/[`SharedCluster::catalog`]
/// instead of keeping its own copy.
pub(crate) struct CatalogData {
    pub(crate) entries: Vec<CatalogEntry>,
    pub(crate) by_key: HashMap<String, CatalogEntry>,
}

impl CatalogData {
    pub(crate) fn new(entries: Vec<CatalogEntry>) -> Self {
        let by_key = entries
            .iter()
            .map(|e| (e.kind.key.clone(), e.clone()))
            .collect();
        Self { entries, by_key }
    }
}

/// The ServiceAccount-owned cluster connection shared by every per-user
/// session on a cluster: discovery catalog, CRD printer columns, and the
/// metrics/PVC enrichment caches. Built once via [`SharedCluster::connect_default`].
pub struct SharedCluster {
    cluster: Arc<ClusterAccess>,
    catalog: Arc<ArcSwap<CatalogData>>,
    columns: Arc<ArcSwap<ColumnMap>>,
    enrich: Arc<Enrichment>,
    /// Fires after the CRD watch stores freshly-rebuilt columns, so every
    /// per-user `InformerRegistry` (subscribed via [`subscribe_columns`])
    /// re-projects its active informers against the new columns without a
    /// client reconnect. A small capacity is enough: it only ever carries a
    /// wake-up, never data (subscribers always re-read `columns` itself).
    columns_changed: broadcast::Sender<()>,
}

impl SharedCluster {
    /// Connect with the ServiceAccount / default credentials (in-cluster
    /// config or local kubeconfig), harvest CRD printer columns, build the
    /// discovery catalog, and start the CRD watch + enrichment scrape loops.
    pub async fn connect_default() -> Result<Arc<Self>, K8sError> {
        let cluster = Arc::new(ClusterAccess::connect_with_default().await?);
        Self::build(cluster).await
    }

    async fn build(cluster: Arc<ClusterAccess>) -> Result<Arc<Self>, K8sError> {
        let client = (*cluster.client()).clone();
        // Harvest CRD-declared printer columns; shared by the catalog (headers)
        // and every per-user informer registry (cell projection).
        let loaded_columns = crate::printer_columns::load(&client).await;
        let entries = build_catalog(&client, &loaded_columns).await?;
        let catalog = Arc::new(ArcSwap::from_pointee(CatalogData::new(entries)));
        let columns = Arc::new(ArcSwap::from_pointee(loaded_columns));
        let enrich = Enrichment::new(cluster.clone());
        let (columns_changed, _) = broadcast::channel(4);
        let shared = Arc::new(Self {
            cluster,
            catalog,
            columns,
            enrich,
            columns_changed,
        });
        // Keep the catalog + columns live: watch CRDs and rebuild on change, so
        // new operators show up and changed printer columns are visible to
        // future subscriptions without a restart.
        spawn_crd_watch_shared(shared.clone());
        Ok(shared)
    }

    /// A `SharedCluster` backed by an unreachable loopback apiserver and an
    /// empty catalog, for unit tests that don't need real cluster I/O. `pub`
    /// (not `#[cfg(test)]`/`pub(crate)`) so the `roder` server crate's own
    /// test fixtures (`AppState`'s `shared` field) can build one — `cfg(test)`
    /// is per-crate and a dependency is never compiled with it on behalf of a
    /// dependent crate's tests.
    #[doc(hidden)]
    pub fn for_test() -> Arc<Self> {
        let cluster = ClusterAccess::for_test();
        let catalog = Arc::new(ArcSwap::from_pointee(CatalogData::new(Vec::new())));
        let columns = Arc::new(ArcSwap::from_pointee(ColumnMap::default()));
        let enrich = Enrichment::new(cluster.clone());
        let (columns_changed, _) = broadcast::channel(4);
        Arc::new(Self {
            cluster,
            catalog,
            columns,
            enrich,
            columns_changed,
        })
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

    /// The hot-swappable catalog store itself, for callers (e.g. `Backend`'s
    /// Flux/tree lookups) that need to iterate `entries`/`by_key` directly
    /// rather than through [`entry`]/[`entry_by_kind`].
    pub(crate) fn catalog(&self) -> Arc<ArcSwap<CatalogData>> {
        self.catalog.clone()
    }

    /// Look up a catalog entry by key. Returns an owned clone because the
    /// catalog is hot-swappable (the `ArcSwap` guard is only valid transiently).
    pub(crate) fn entry(&self, key: &str) -> Result<CatalogEntry, K8sError> {
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
    pub(crate) fn entry_by_kind(&self, kind: &str) -> Result<CatalogEntry, K8sError> {
        self.catalog
            .load()
            .entries
            .iter()
            .find(|e| e.kind.group.ends_with("fluxcd.io") && e.kind.kind == kind)
            .cloned()
            .ok_or_else(|| K8sError::Api(format!("no Flux source kind found: {kind}")))
    }

    /// The shared metrics/PVC enrichment caches + scrape loops.
    pub(crate) fn enrichment(&self) -> Arc<Enrichment> {
        self.enrich.clone()
    }

    /// The shared, hot-swapped CRD printer-column map.
    pub(crate) fn columns(&self) -> Arc<ArcSwap<ColumnMap>> {
        self.columns.clone()
    }

    /// Subscribe to column-rebuild notifications. Fired once per CRD-driven
    /// catalog/columns rebuild (see [`spawn_crd_watch_shared`]); each per-user
    /// `InformerRegistry` holds its own receiver so it can re-project its
    /// active informers against the freshly-swapped `columns()`.
    pub(crate) fn subscribe_columns(&self) -> broadcast::Receiver<()> {
        self.columns_changed.subscribe()
    }

    /// The ServiceAccount `kube::Client`, for handlers that legitimately need
    /// SA-level reads (e.g. alertmanager discovery) rather than the calling
    /// user's own permissions.
    pub fn sa_client(&self) -> kube::Client {
        (*self.cluster.client()).clone()
    }
}

/// Re-discover the catalog and re-harvest CRD printer columns, swapping both
/// into the shared stores, then notify every per-user `InformerRegistry`
/// (via [`SharedCluster::subscribe_columns`]) to re-project its active
/// informers against the new columns. Best-effort: a failed rebuild leaves
/// the previous catalog/columns in place (and no notification is sent).
async fn rebuild_catalog_shared(
    cluster: &Arc<ClusterAccess>,
    catalog: &Arc<ArcSwap<CatalogData>>,
    columns: &Arc<ArcSwap<ColumnMap>>,
    columns_changed: &broadcast::Sender<()>,
) {
    let client = (*cluster.client()).clone();
    let loaded_columns = crate::printer_columns::load(&client).await;
    match build_catalog(&client, &loaded_columns).await {
        Ok(entries) => {
            catalog.store(Arc::new(CatalogData::new(entries)));
            columns.store(Arc::new(loaded_columns));
            // No receivers (e.g. no user session active yet) is not an error.
            let _ = columns_changed.send(());
            tracing::debug!("shared catalog + printer columns refreshed after CRD change");
        }
        Err(e) => tracing::debug!("shared catalog refresh skipped: {e}"),
    }
}

/// Watch `CustomResourceDefinition`s and rebuild the shared catalog/columns on
/// change, so new operators appear and changed printer columns are visible —
/// no restart. Events are debounced (operators often churn CRDs in bursts)
/// into one rebuild. This is the `SharedCluster` counterpart of
/// `backend::spawn_crd_watch`; see that function's doc comment for why the
/// watch is metadata-only.
fn spawn_crd_watch_shared(shared: Arc<SharedCluster>) {
    tokio::spawn(async move {
        let mut backoff_attempt = 0;
        loop {
            let client = (*shared.cluster.client()).clone();
            let api: Api<PartialObjectMeta<CustomResourceDefinition>> = Api::all(client);
            let stream = watcher::watcher(api, watcher::Config::default());
            futures::pin_mut!(stream);
            while let Some(event) = stream.next().await {
                match event {
                    Err(e) => {
                        tracing::debug!("CRD watch error (self-healing): {e}");
                        let delay = rebuild_backoff(backoff_attempt);
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
                            let delay = rebuild_backoff(backoff_attempt);
                            backoff_attempt = backoff_attempt.saturating_add(1);
                            tokio::time::sleep(delay).await;
                        }
                        Ok(Some(Ok(watcher::Event::Init))) => {}
                        Ok(Some(Ok(_))) => backoff_attempt = 0,
                        Ok(None) | Err(_) => break,
                    }
                }
                rebuild_catalog_shared(
                    &shared.cluster,
                    &shared.catalog,
                    &shared.columns,
                    &shared.columns_changed,
                )
                .await;
            }
            // Stream ended; pause before rebuilding the CRD watch.
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn column_notifications_reach_each_shared_subscriber() {
        let shared = SharedCluster::for_test();
        let mut first = shared.subscribe_columns();
        let mut second = shared.subscribe_columns();

        shared.columns_changed.send(()).unwrap();
        assert_eq!(first.recv().await, Ok(()));
        assert_eq!(second.recv().await, Ok(()));
    }

    #[test]
    fn catalog_data_indexes_entries_by_resource_key() {
        let api_resource = kube::core::ApiResource {
            group: "apps".into(),
            version: "v1".into(),
            api_version: "apps/v1".into(),
            kind: "Deployment".into(),
            plural: "deployments".into(),
        };
        let entry = CatalogEntry {
            kind: roder_core::ResourceKind {
                key: "apps/v1/Deployment".into(),
                group: "apps".into(),
                version: "v1".into(),
                kind: "Deployment".into(),
                plural: "deployments".into(),
                namespaced: true,
                category: roder_core::Category::Workloads,
                columns: vec!["Ready".into()],
            },
            api_resource,
        };

        let catalog = CatalogData::new(vec![entry.clone()]);
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(
            catalog.by_key["apps/v1/Deployment"].api_resource,
            entry.api_resource
        );
    }
}
