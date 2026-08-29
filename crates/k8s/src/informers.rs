use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use futures::StreamExt;
use kube::api::{DynamicObject, ListParams, WatchParams};
use kube::core::ApiResource;
use roder_core::{MetricsPoint, ResourceRow, Trend, WatchEvent};
use tokio::sync::{broadcast, Mutex, Notify, RwLock};

use crate::client::ClusterAccess;
use crate::metrics::PvcUsage;
use crate::project::{project_table_row, reproject_table_row, table_layout, TableLayout};
use crate::table::{TableApi, TableError, TableWatchEvent};

/// Broadcast backlog per informer. Each slot can hold a full `Snapshot` (every
/// row), so a deep buffer multiplies memory on busy kinds. Kept modest: when a
/// slow SSE consumer lags past it, the watch handler re-snapshots from the live
/// `rows` cache (see `api::watch`), so a small backlog costs a resync, not data.
const CHANNEL_CAP: usize = 512;
/// How long an informer with no subscribers lingers before being evicted. Kept
/// generous so reconnecting to a recently-viewed resource serves the warm cache
/// instead of re-LISTing — and fewer LIST-then-WATCH restarts is kinder to etcd.
const IDLE_GRACE: Duration = Duration::from_secs(600);

#[derive(Clone, PartialEq, Eq, Hash)]
struct WatchKey {
    resource_key: String,
    namespace: Option<String>,
    /// Label selector (e.g. a workload's `spec.selector`) — filtered watches get
    /// their own shared informer, distinct from the unfiltered list.
    selector: Option<String>,
}

/// Per-pod current + previous metrics sample for trend arrows.
#[derive(Clone, Copy, Default)]
pub(crate) struct UsageEntry {
    pub(crate) cpu: f64,
    pub(crate) mem: f64,
    prev_cpu: Option<f64>,
    prev_mem: Option<f64>,
    /// Consecutive scrape cycles in which this pod was absent from the
    /// metrics-server response. Pods are only evicted after several misses so
    /// a single partial response (e.g. one node temporarily unreachable) does
    /// not permanently wipe their usage history.
    misses: u8,
}

impl UsageEntry {
    pub(crate) fn trend_cpu(&self) -> Trend {
        match self.prev_cpu {
            Some(p) if self.cpu > p + 0.001 => Trend::Up,
            Some(p) if self.cpu < p - 0.001 => Trend::Down,
            _ => Trend::None,
        }
    }

    pub(crate) fn trend_mem(&self) -> Trend {
        match self.prev_mem {
            Some(p) if self.mem > p + 1024.0 * 1024.0 => Trend::Up,
            Some(p) if self.mem < p - 1024.0 * 1024.0 => Trend::Down,
            _ => Trend::None,
        }
    }

    /// Whether a previous sample exists (first sample has no trend).
    pub(crate) fn has_prev(&self) -> bool {
        self.prev_cpu.is_some() && self.prev_mem.is_some()
    }
}

/// Combined current + previous usage map, updated atomically by the metrics refresh.
pub(crate) type UsageWithTrend = HashMap<String, UsageEntry>;

/// Time-series history for a single pod (namespace/name).
/// Stores up to MAX_HISTORY_POINTS data points.
pub type PodMetricsHistory = VecDeque<MetricsPoint>;

/// Maximum number of data points to keep per pod (roughly 1 hour at 15s intervals).
const MAX_HISTORY_POINTS: usize = 240;

/// Map of PVC key (namespace/name) to its current filesystem usage.
pub(crate) type PvcUsageMap = HashMap<String, PvcUsage>;

/// Map of pod key (namespace/name) to its metrics history.
pub(crate) type MetricsHistory = HashMap<String, PodMetricsHistory>;

/// Secondary index key: (namespace, name) → uid for O(1) object lookup by name.
type NameKey = (Option<String>, String);
type NameIndex = Arc<RwLock<HashMap<NameKey, String>>>;

type Headers = Arc<ArcSwap<Vec<String>>>;

struct Active {
    tx: broadcast::Sender<WatchEvent>,
    rows: Arc<RwLock<HashMap<String, ResourceRow>>>,
    handle: tokio::task::JoinHandle<()>,
    /// Last time a subscriber was present (for idle eviction).
    idle_since: Arc<RwLock<Option<Instant>>>,
    /// Whether this informer watches pods (gets live CPU/mem enrichment).
    is_pod: bool,
    /// Whether this informer watches PVCs (gets live filesystem usage enrichment).
    is_pvc: bool,
    /// Resource key (`group/version/kind`) and namespace this informer covers, so
    /// the detail handler can locate the right cache.
    resource_key: String,
    namespace: Option<String>,
    /// (group, kind) for relisting when CRDs change.
    group: String,
    kind: String,
    /// Last-seen full objects by uid, so detail is served from cache (and metric
    /// refreshes re-project pods) without an extra apiserver round-trip.
    objects: Arc<RwLock<HashMap<String, DynamicObject>>>,
    /// Secondary index: (namespace, name) → uid for O(1) object lookup by name.
    by_name: NameIndex,
    /// Current server Table layout, replaced atomically with each successful relist.
    layout: Arc<RwLock<Option<TableLayout>>>,
    headers: Headers,
    schema_lock: Arc<RwLock<()>>,
    /// Poked by `refresh_schema` to make the task relist and publish a fresh snapshot.
    reproject: Arc<Notify>,
    /// Terminal event retained after the watch task exits so a later subscriber
    /// does not attach to a silent, dead informer.
    terminal: Arc<RwLock<Option<WatchEvent>>>,
}

/// Aborts the underlying watch task when this entry is dropped. Dropping a
/// `tokio::task::JoinHandle` on its own does NOT stop the task — tokio just
/// detaches it, leaving it running against the user's client forever. This
/// makes removal from `InformerRegistry::active` (idle reaper) and the whole
/// registry dropping (see the `Weak`-holding background loops in
/// [`spawn_reaper`]/[`spawn_reproject`]/[`spawn_columns_watch`]) actually free
/// the watch. `abort()` is idempotent, so an explicit `entry.handle.abort()`
/// elsewhere before drop is harmless.
impl Drop for Active {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// What a caller gets when subscribing to a live list: the current contents plus
/// a receiver for subsequent deltas.
pub struct WatchHandle {
    pub snapshot: Vec<ResourceRow>,
    pub initial_columns: Vec<String>,
    pub rx: broadcast::Receiver<WatchEvent>,
    /// Live row cache (used by the SSE handler to send a re-snapshot on broadcast lag).
    pub rows: Arc<RwLock<HashMap<String, ResourceRow>>>,
    /// Current column headers, hot-swapped on CRD change — read for the initial
    /// snapshot and any lag-resync so headers always match the rows' cells.
    pub columns: Headers,
    pub schema_lock: Arc<RwLock<()>>,
}

/// Shared cluster-metadata + metrics enrichment layer, owned once per cluster
/// (by the SA-credentialed access) regardless of how many per-user
/// `InformerRegistry`s are watching. Holds the pod-usage/metrics-history/PVC-usage
/// caches and runs the periodic scrapes that fill them, so N users watching the
/// same cluster still costs one metrics-server scrape and one kubelet scan, not N.
pub struct Enrichment {
    cluster: Arc<ClusterAccess>,
    /// Shared pod usage cache (current + previous), refreshed once for all pod informers.
    pub pod_usage: Arc<RwLock<UsageWithTrend>>,
    /// Time-series metrics history for pods, used for graphs.
    pub metrics_history: Arc<RwLock<MetricsHistory>>,
    /// Shared PVC filesystem usage cache, refreshed once for all PVC informers.
    pub pvc_usage: Arc<RwLock<PvcUsageMap>>,
}

impl Enrichment {
    /// Start the shared cache + spawn the scrape loops once, driven by the
    /// SA `cluster`. Per-user `InformerRegistry`s only read these caches.
    pub fn new(cluster: Arc<ClusterAccess>) -> Arc<Self> {
        let enrichment = Arc::new(Self {
            cluster,
            pod_usage: Arc::new(RwLock::new(HashMap::new())),
            metrics_history: Arc::new(RwLock::new(HashMap::new())),
            pvc_usage: Arc::new(RwLock::new(PvcUsageMap::default())),
        });
        spawn_metrics_scrape(enrichment.clone());
        spawn_pvc_usage_scrape(enrichment.clone());
        enrichment
    }

    /// Get the metrics history for a specific pod (namespace/name).
    pub async fn pod_metrics_history(
        &self,
        namespace: &str,
        name: &str,
    ) -> Option<Vec<MetricsPoint>> {
        let key = format!("{namespace}/{name}");
        let history = self.metrics_history.read().await;
        history.get(&key).map(|h| h.iter().copied().collect())
    }
}

/// Registry of shared informers. One watch per (resource, namespace) is kept on
/// the API server regardless of how many UI clients subscribe — that is the
/// mechanism that keeps roder kind to etcd.
pub struct InformerRegistry {
    cluster: Arc<ClusterAccess>,
    active: Mutex<HashMap<WatchKey, Active>>,
    /// Shared metrics/PVC caches, injected — one `Enrichment` (and its scrape
    /// loops) serves every per-user registry watching the same cluster.
    enrich: Arc<Enrichment>,
}

impl InformerRegistry {
    /// `schema_rx` notifies this registry when active Table informers must relist.
    pub fn new(
        cluster: Arc<ClusterAccess>,
        enrich: Arc<Enrichment>,
        schema_rx: broadcast::Receiver<()>,
    ) -> Arc<Self> {
        let registry = Arc::new(Self {
            cluster,
            active: Mutex::new(HashMap::new()),
            enrich,
        });
        // The 3 background loops hold only `Weak` — the registry's sole strong
        // owner is `Backend` (see `crates/k8s/src/backend/mod.rs`), so dropping
        // a `Backend` actually frees this registry, its `active` watch tasks
        // (see `impl Drop for Active`), and the user's `ClusterAccess`
        // connection pool, instead of leaking them forever.
        spawn_reaper(Arc::downgrade(&registry));
        spawn_reproject(Arc::downgrade(&registry));
        spawn_schema_watch(Arc::downgrade(&registry), schema_rx);
        registry
    }

    /// Subscribe to a live list, starting the shared informer if needed.
    pub async fn subscribe(
        &self,
        ar: &ApiResource,
        group: &str,
        kind: &str,
        namespaced: bool,
        namespace: Option<String>,
        selector: Option<String>,
    ) -> WatchHandle {
        let selector = selector.filter(|s| !s.is_empty());
        let key = WatchKey {
            resource_key: format!("{group}/{}/{kind}", ar.version),
            namespace: if namespaced { namespace.clone() } else { None },
            selector: selector.clone(),
        };

        let mut active = self.active.lock().await;
        let entry = active.entry(key.clone()).or_insert_with(|| {
            start_informer(
                self.cluster.clone(),
                ar.clone(),
                group.to_string(),
                kind.to_string(),
                namespaced,
                key.namespace.clone(),
                selector,
                self.enrich.pod_usage.clone(),
                self.enrich.pvc_usage.clone(),
            )
        });

        // Subscribe first so no delta is missed between snapshot and stream.
        let mut rx = entry.tx.subscribe();
        *entry.idle_since.write().await = None;
        let schema_guard = entry.schema_lock.read().await;
        let snapshot = entry.rows.read().await.values().cloned().collect();
        let initial_columns = (**entry.headers.load()).clone();
        drop(schema_guard);
        if let Some(event) = entry.terminal.read().await.clone() {
            let (terminal_tx, terminal_rx) = broadcast::channel(1);
            let _ = terminal_tx.send(event);
            rx = terminal_rx;
        }
        WatchHandle {
            snapshot,
            initial_columns,
            rx,
            rows: entry.rows.clone(),
            columns: entry.headers.clone(),
            schema_lock: entry.schema_lock.clone(),
        }
    }

    /// A changed CRD may change the server's Table schema. Relist active
    /// informers so rows and headers are replaced atomically from a fresh Table.
    pub async fn refresh_schema(&self) {
        let active = self.active.lock().await;
        for entry in active.values() {
            entry.reproject.notify_one();
        }
    }

    /// Whether any active informer currently has a live SSE subscriber
    /// (broadcast receiver). Used by the per-subject `BackendRegistry`'s idle
    /// reaper/soft-cap eviction (roder's server crate) to decide whether a
    /// subject's whole `Backend` can be dropped: even if the subject hasn't
    /// issued a new HTTP request in a while, an open dashboard tab still
    /// streaming a live view must not be evicted out from under it.
    pub async fn has_active_subscribers(&self) -> bool {
        self.active
            .lock()
            .await
            .values()
            .any(|e| e.tx.receiver_count() > 0)
    }

    /// Serve an object from a live informer cache (no apiserver round-trip) when one
    /// of the active informers for this resource already holds it. Uses a secondary
    /// name-based index for O(1) lookup instead of scanning all cached objects.
    pub async fn cached_object(
        &self,
        resource_key: &str,
        namespace: Option<&str>,
        name: &str,
    ) -> Option<DynamicObject> {
        // Clone the Arc handles under the lock so we can drop the Mutex before
        // awaiting the inner RwLocks — holding a Mutex across an async await
        // blocks every other subscriber, reaper, and refresh task.
        let (by_name, objects) = {
            let active = self.active.lock().await;
            let entry = active.values().find(|e| {
                if e.resource_key != resource_key {
                    return false;
                }
                // A namespace-scoped informer can't hold an object from another namespace.
                if let (Some(en), Some(want)) = (e.namespace.as_deref(), namespace) {
                    if en != want {
                        return false;
                    }
                }
                true
            })?;
            (entry.by_name.clone(), entry.objects.clone())
        };
        // O(1) name lookup using the secondary index.
        let lookup = (namespace.map(|s| s.to_string()), name.to_string());
        let uid = by_name.read().await.get(&lookup).cloned()?;
        let objs = objects.read().await;
        objs.get(&uid).cloned()
    }
}

#[allow(clippy::too_many_arguments)]
fn start_informer(
    cluster: Arc<ClusterAccess>,
    ar: ApiResource,
    group: String,
    kind: String,
    namespaced: bool,
    namespace: Option<String>,
    selector: Option<String>,
    pod_usage: Arc<RwLock<UsageWithTrend>>,
    pvc_usage: Arc<RwLock<PvcUsageMap>>,
) -> Active {
    let is_pod = group.is_empty() && kind == "Pod";
    let is_pvc = group.is_empty() && kind == "PersistentVolumeClaim";
    let resource_key = format!("{group}/{}/{kind}", ar.version);
    let active_ns = namespace.clone();
    // Clones for the `Active` record; the originals are moved into the task.
    let active_group = group.clone();
    let active_kind = kind.clone();
    let (tx, _rx) = broadcast::channel(CHANNEL_CAP);
    let rows: Arc<RwLock<HashMap<String, ResourceRow>>> = Arc::new(RwLock::new(HashMap::new()));
    let objects: Arc<RwLock<HashMap<String, DynamicObject>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let by_name: NameIndex = Arc::new(RwLock::new(HashMap::new()));
    // Start as None: the informer is being subscribed to immediately in
    // subscribe(), which sets idle_since = None. Initialising to Some(now())
    // would make the reaper start the eviction clock before any subscription.
    let idle_since: Arc<RwLock<Option<Instant>>> = Arc::new(RwLock::new(None));

    let headers: Headers = Arc::new(ArcSwap::from_pointee(Vec::new()));
    let layout = Arc::new(RwLock::new(None));
    let schema_lock = Arc::new(RwLock::new(()));
    let reproject = Arc::new(Notify::new());
    let terminal = Arc::new(RwLock::new(None));

    let task_tx = tx.clone();
    let task_rows = rows.clone();
    let task_objects = objects.clone();
    let task_by_name = by_name.clone();
    let task_pvc_usage = pvc_usage.clone();
    let task_layout = layout.clone();
    let task_schema_lock = schema_lock.clone();
    let task_headers = headers.clone();
    let task_reproject = reproject.clone();
    let task_terminal = terminal.clone();
    // Only pods and PVCs are re-projected from their full object body (live
    // metrics / filesystem usage) — and only those need cached objects for the
    // detail view. For every other kind we keep just the lightweight rows and
    // let the detail handler fall back to a single GET, so large Secrets /
    // ConfigMaps / CRDs don't pin their full bodies in memory for the cluster's
    // lifetime. This is the main steady-state memory reduction.
    let cache_objects = is_pod || is_pvc;
    let handle = tokio::spawn(async move {
        let mut backoff_attempt: u32 = 0;
        'relist: loop {
            let client = (*cluster.client()).clone();
            let api = TableApi::new(
                client,
                &ar,
                if namespaced {
                    namespace.as_deref()
                } else {
                    None
                },
            );
            let mut building: HashMap<String, ResourceRow> = HashMap::new();
            let mut building_objs: HashMap<String, DynamicObject> = HashMap::new();
            let mut continue_token = String::new();
            let mut resource_version = String::new();
            let mut current_layout: Option<TableLayout> = None;

            // A complete paginated LIST is built off to the side and committed
            // only after the final page, so subscribers never observe a partial
            // replacement when the apiserver pages a large resource collection.
            loop {
                let mut params = ListParams::default().limit(500);
                if let Some(selector) = &selector {
                    params = params.labels(selector);
                }
                if !continue_token.is_empty() {
                    params = params.continue_token(&continue_token);
                }
                let table = tokio::select! {
                    result = api.list(&params) => match result {
                        Ok(table) => table,
                        Err(error) => {
                            if is_forbidden_error(&error) {
                                publish_terminal(&task_tx, &task_terminal, error.to_string()).await;
                                return;
                            }
                            if is_auth_error(&error) {
                                tokio::time::sleep(rebuild_backoff(backoff_attempt)).await;
                                backoff_attempt = backoff_attempt.saturating_add(1);
                                continue 'relist;
                            }
                            if error.is_permanent() {
                                publish_error(&task_tx, &task_terminal, error.to_string()).await;
                                return;
                            }
                            tracing::debug!("Table list error for {group}/{kind}: {error}");
                            let delay = rebuild_backoff(backoff_attempt);
                            backoff_attempt = backoff_attempt.saturating_add(1);
                            tokio::time::sleep(delay).await;
                            continue 'relist;
                        }
                    },
                    _ = task_reproject.notified() => continue 'relist,
                };
                if table.kind != "Table" {
                    publish_error(
                        &task_tx,
                        &task_terminal,
                        format!("expected Kubernetes Table, got {}", table.kind),
                    )
                    .await;
                    return;
                }
                if current_layout.is_none() {
                    current_layout = Some(table_layout(
                        &group,
                        &kind,
                        namespaced && namespace.is_none(),
                        &table.column_definitions,
                    ));
                }
                let layout = current_layout.as_ref().expect("Table layout initialized");
                for table_row in &table.rows {
                    let Some(object) = table_row.object.as_ref() else {
                        publish_error(
                            &task_tx,
                            &task_terminal,
                            format!("Table row for {group}/{kind} omitted its object"),
                        )
                        .await;
                        return;
                    };
                    let (usage, pvc_u) = enrichments(
                        object,
                        is_pod,
                        is_pvc,
                        &*pod_usage.read().await,
                        &*task_pvc_usage.read().await,
                    );
                    if let Some((row, object)) =
                        project_table_row(&group, &kind, layout, table_row, usage, pvc_u)
                    {
                        if cache_objects {
                            building_objs.insert(row.uid.clone(), object);
                        }
                        building.insert(row.uid.clone(), row);
                    }
                }
                if resource_version.is_empty() {
                    resource_version.clone_from(&table.metadata.resource_version);
                }
                continue_token = table.metadata.continue_;
                if continue_token.is_empty() {
                    break;
                }
            }

            let Some(layout) = current_layout else {
                tracing::warn!("Table list for {group}/{kind} had no schema");
                continue;
            };
            let rows_vec: Vec<ResourceRow> = building.values().cloned().collect();
            let mut name_idx = HashMap::new();
            if cache_objects {
                for (uid, object) in &building_objs {
                    name_idx.insert(name_key(object), uid.clone());
                }
            }
            let schema_guard = task_schema_lock.write().await;
            task_headers.store(Arc::new(layout.columns.clone()));
            *task_layout.write().await = Some(layout.clone());
            *task_rows.write().await = std::mem::take(&mut building);
            *task_objects.write().await = std::mem::take(&mut building_objs);
            *task_by_name.write().await = name_idx;
            let _ = task_tx.send(WatchEvent::Snapshot {
                columns: layout.columns.clone(),
                rows: rows_vec,
            });
            drop(schema_guard);
            backoff_attempt = 0;

            // Reconnect a cleanly-ended or transiently-failed watch from its
            // last resource version. Only a 410 or schema notification relists.
            loop {
                let mut params = WatchParams::default().timeout(290);
                if let Some(selector) = &selector {
                    params = params.labels(selector);
                }
                let stream = tokio::select! {
                    result = api.watch(&params, &resource_version) => match result {
                        Ok(stream) => stream,
                        Err(error) => {
                            if is_forbidden_error(&error) {
                                publish_terminal(&task_tx, &task_terminal, error.to_string()).await;
                                return;
                            }
                            if is_auth_error(&error) {
                                tokio::time::sleep(rebuild_backoff(backoff_attempt)).await;
                                backoff_attempt = backoff_attempt.saturating_add(1);
                                continue 'relist;
                            }
                            if error.is_permanent() {
                                publish_error(&task_tx, &task_terminal, error.to_string()).await;
                                return;
                            }
                            tracing::debug!("Table watch connect error for {group}/{kind}: {error}");
                            tokio::time::sleep(rebuild_backoff(backoff_attempt)).await;
                            backoff_attempt = backoff_attempt.saturating_add(1);
                            continue;
                        }
                    },
                    _ = task_reproject.notified() => continue 'relist,
                };
                futures::pin_mut!(stream);
                loop {
                    let event = tokio::select! {
                        event = stream.next() => event,
                        _ = task_reproject.notified() => continue 'relist,
                    };
                    let Some(event) = event else {
                        break;
                    };
                    match event {
                        Ok(TableWatchEvent::Added(table) | TableWatchEvent::Modified(table)) => {
                            if let Some(version) = table.resource_version() {
                                resource_version = version.to_string();
                            }
                            for table_row in &table.rows {
                                let Some(object) = table_row.object.as_ref() else {
                                    publish_error(
                                        &task_tx,
                                        &task_terminal,
                                        format!(
                                            "Table watch row for {group}/{kind} omitted its object"
                                        ),
                                    )
                                    .await;
                                    return;
                                };
                                let (usage, pvc_u) = enrichments(
                                    object,
                                    is_pod,
                                    is_pvc,
                                    &*pod_usage.read().await,
                                    &*task_pvc_usage.read().await,
                                );
                                if let Some((row, object)) = project_table_row(
                                    &group, &kind, &layout, table_row, usage, pvc_u,
                                ) {
                                    let uid = row.uid.clone();
                                    if cache_objects {
                                        task_by_name
                                            .write()
                                            .await
                                            .insert(name_key(&object), uid.clone());
                                        task_objects.write().await.insert(uid.clone(), object);
                                    }
                                    task_rows.write().await.insert(uid, row.clone());
                                    let _ = task_tx.send(WatchEvent::Applied { row });
                                } else {
                                    let uid = object.metadata.uid.clone().unwrap_or_else(|| {
                                        format!(
                                            "{}/{}",
                                            object.metadata.namespace.clone().unwrap_or_default(),
                                            object.metadata.name.clone().unwrap_or_default()
                                        )
                                    });
                                    if task_rows.write().await.remove(&uid).is_some() {
                                        task_objects.write().await.remove(&uid);
                                        task_by_name.write().await.remove(&name_key(object));
                                        let _ = task_tx.send(WatchEvent::Deleted { uid });
                                    }
                                }
                            }
                            backoff_attempt = 0;
                        }
                        Ok(TableWatchEvent::Deleted(table)) => {
                            if let Some(version) = table.resource_version() {
                                resource_version = version.to_string();
                            }
                            for table_row in table.rows {
                                let Some(object) = table_row.object else {
                                    continue;
                                };
                                let uid = object.metadata.uid.clone().unwrap_or_else(|| {
                                    format!(
                                        "{}/{}",
                                        object.metadata.namespace.clone().unwrap_or_default(),
                                        object.metadata.name.clone().unwrap_or_default()
                                    )
                                });
                                task_rows.write().await.remove(&uid);
                                if cache_objects {
                                    task_objects.write().await.remove(&uid);
                                    task_by_name.write().await.remove(&name_key(&object));
                                }
                                let _ = task_tx.send(WatchEvent::Deleted { uid });
                            }
                        }
                        Ok(TableWatchEvent::Bookmark(table)) => {
                            if let Some(version) = table.resource_version() {
                                resource_version = version.to_string();
                            }
                        }
                        Ok(TableWatchEvent::Error(status)) if status.code == 410 => {
                            continue 'relist
                        }
                        Ok(TableWatchEvent::Error(status)) if status.code == 401 => {
                            tokio::time::sleep(rebuild_backoff(backoff_attempt)).await;
                            backoff_attempt = backoff_attempt.saturating_add(1);
                            continue 'relist;
                        }
                        Ok(TableWatchEvent::Error(status)) if status.code == 403 => {
                            publish_terminal(&task_tx, &task_terminal, status.message).await;
                            return;
                        }
                        Ok(TableWatchEvent::Error(status)) => {
                            if (400..500).contains(&status.code) {
                                publish_error(&task_tx, &task_terminal, status.message).await;
                                return;
                            }
                            tracing::debug!(
                                "Table watch status for {group}/{kind}: {}",
                                status.message
                            );
                            break;
                        }
                        Err(error) => {
                            if is_forbidden_error(&error) {
                                publish_terminal(&task_tx, &task_terminal, error.to_string()).await;
                                return;
                            }
                            if is_auth_error(&error) {
                                tokio::time::sleep(rebuild_backoff(backoff_attempt)).await;
                                backoff_attempt = backoff_attempt.saturating_add(1);
                                continue 'relist;
                            }
                            if error.is_permanent() {
                                publish_error(&task_tx, &task_terminal, error.to_string()).await;
                                return;
                            }
                            tracing::debug!("Table watch error for {group}/{kind}: {error}");
                            break;
                        }
                    }
                }
                tokio::time::sleep(rebuild_backoff(backoff_attempt)).await;
                backoff_attempt = backoff_attempt.saturating_add(1);
            }
        }
    });

    Active {
        tx,
        rows,
        handle,
        idle_since,
        is_pod,
        is_pvc,
        resource_key,
        namespace: active_ns,
        group: active_group,
        kind: active_kind,
        objects,
        by_name,
        layout,
        headers,
        schema_lock,
        reproject,
        terminal,
    }
}

/// Background task: stop informers that have had no subscribers for a while, so
/// we don't hold watches for views nobody is looking at.
fn spawn_reaper(registry: std::sync::Weak<InformerRegistry>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tick.tick().await;
            let Some(registry) = registry.upgrade() else {
                break;
            };

            // Phase 1: Snapshot (key, idle_since arc, receiver count) while
            // holding the Mutex for the minimum time — no inner awaits here.
            #[allow(clippy::type_complexity)]
            let entries: Vec<(WatchKey, Arc<RwLock<Option<Instant>>>, usize)> = {
                let active = registry.active.lock().await;
                active
                    .iter()
                    .map(|(k, e)| (k.clone(), e.idle_since.clone(), e.tx.receiver_count()))
                    .collect()
            };

            // Phase 2: Update idle_since outside the Mutex so subscribe() isn't
            // blocked while we await each entry's inner RwLock.
            let mut to_remove = Vec::new();
            for (key, idle_since, receivers) in entries {
                if receivers == 0 {
                    let mut idle = idle_since.write().await;
                    match *idle {
                        Some(since) if since.elapsed() >= IDLE_GRACE => {
                            to_remove.push(key);
                        }
                        None => *idle = Some(Instant::now()),
                        _ => {}
                    }
                } else {
                    *idle_since.write().await = None;
                }
            }

            // Phase 3: Re-acquire to remove evicted entries.
            if !to_remove.is_empty() {
                let mut active = registry.active.lock().await;
                for key in to_remove {
                    // A subscriber may have attached after phase 1. Recheck
                    // while holding the same lock used by subscribe() before
                    // dropping and aborting the informer.
                    let still_idle = active
                        .get(&key)
                        .is_some_and(|entry| entry.tx.receiver_count() == 0);
                    if still_idle && active.remove(&key).is_some() {
                        tracing::debug!("evicted idle informer {}", key.resource_key);
                    }
                }
            }
        }
    });
}

/// Background task: re-project this registry's active pod/PVC informers from
/// the shared `Enrichment` caches every 15s, and re-broadcast any row that
/// changed. This is the per-user counterpart of the (now cache-only) scrape
/// loops on `Enrichment` — one scrape fills the shared cache, and each
/// per-user registry re-projects its own subscribers' rows from it on its own
/// timer, so N users watching the same pod still cost one metrics-server
/// scrape but each get their own broadcast stream.
fn spawn_reproject(registry: std::sync::Weak<InformerRegistry>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tick.tick().await;
            let Some(registry) = registry.upgrade() else {
                break;
            };
            reproject_once(&registry).await;
        }
    });
}

/// Relist active Table informers when the shared CRD watch reports a schema change.
fn spawn_schema_watch(
    registry: std::sync::Weak<InformerRegistry>,
    mut schema_rx: broadcast::Receiver<()>,
) {
    tokio::spawn(async move {
        // `schema_rx.recv()` can block indefinitely when no CRD changes
        // occur, so a 30s liveness tick is raced alongside it purely so this
        // loop still notices — and exits — once the registry (held only
        // `Weak` here) has been dropped, rather than blocking on `recv()`
        // forever after the last real notification.
        let mut liveness = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                recv = schema_rx.recv() => {
                    match recv {
                        Err(broadcast::error::RecvError::Closed) => break,
                        // Lagged: fall through and do a catch-up refresh below.
                        Err(broadcast::error::RecvError::Lagged(_)) | Ok(()) => {}
                    }
                }
                _ = liveness.tick() => {
                    if registry.upgrade().is_none() {
                        break;
                    }
                    continue;
                }
            }
            let Some(registry) = registry.upgrade() else {
                break;
            };
            registry.refresh_schema().await;
        }
    });
}

/// A snapshot of the Arc handles a single active informer needs for
/// re-projection, taken while `InformerRegistry::active` is locked so the
/// lock can be dropped before any inner-lock awaits (see [`reproject_once`]).
struct ReprojectTarget {
    tx: broadcast::Sender<WatchEvent>,
    objects: Arc<RwLock<HashMap<String, DynamicObject>>>,
    rows: Arc<RwLock<HashMap<String, ResourceRow>>>,
    is_pod: bool,
    is_pvc: bool,
    group: String,
    kind: String,
    layout: Arc<RwLock<Option<TableLayout>>>,
    schema_lock: Arc<RwLock<()>>,
}

/// One tick of per-user re-projection: re-derive pod/PVC rows from the shared
/// `Enrichment` caches and re-broadcast whichever changed.
///
/// Phased locking mirrors `spawn_reaper` ("holding the Mutex for the minimum
/// time — no inner awaits here") and `cached_object` (clone the Arc handles
/// under the lock, drop it, THEN await the inner RwLocks): `active` is only
/// ever held long enough to clone handles into a `Vec`, never across an
/// await, so this never blocks `subscribe()`, `refresh_schema()`, or
/// `cached_object()`. The `pod_usage`/`pvc_usage` read guards are similarly
/// held only briefly, per entry, rather than for the whole tick, so a
/// concurrent scrape-loop write is blocked for at most one entry's reproject,
/// not the whole registry's.
async fn reproject_once(registry: &InformerRegistry) {
    // Phase 1: snapshot the entries that actually need re-projecting (pod/PVC
    // informers with at least one live subscriber) while holding the Mutex
    // for the minimum time — no inner awaits here.
    let entries: Vec<ReprojectTarget> = {
        let active = registry.active.lock().await;
        active
            .values()
            .filter(|e| (e.is_pod || e.is_pvc) && e.tx.receiver_count() > 0)
            .map(|e| ReprojectTarget {
                tx: e.tx.clone(),
                objects: e.objects.clone(),
                rows: e.rows.clone(),
                is_pod: e.is_pod,
                is_pvc: e.is_pvc,
                group: e.group.clone(),
                kind: e.kind.clone(),
                layout: e.layout.clone(),
                schema_lock: e.schema_lock.clone(),
            })
            .collect()
    };

    // Phase 2: outside the Mutex, briefly read the shared cache per entry and
    // reproject/broadcast changed rows — the `pod_usage`/`pvc_usage` read
    // guard is only held for one entry's reproject call, not the whole loop.
    for entry in entries {
        let _schema_guard = entry.schema_lock.read().await;
        let Some(layout) = entry.layout.read().await.clone() else {
            continue;
        };
        if entry.is_pod {
            let usage = registry.enrich.pod_usage.read().await;
            reproject_entry(&entry.tx, &entry.objects, &entry.rows, |obj, current| {
                reproject_table_row(
                    &entry.group,
                    &entry.kind,
                    &layout,
                    obj,
                    current,
                    usage_for(obj, &usage),
                    None,
                )
            })
            .await;
        } else if entry.is_pvc {
            let pvc = registry.enrich.pvc_usage.read().await;
            reproject_entry(&entry.tx, &entry.objects, &entry.rows, |obj, current| {
                reproject_table_row(
                    &entry.group,
                    &entry.kind,
                    &layout,
                    obj,
                    current,
                    None,
                    pvc_usage_for(obj, &pvc),
                )
            })
            .await;
        }
    }
}

#[cfg(test)]
impl Active {
    /// Build a minimal pod `Active` entry for tests, seeded with one cached
    /// object + matching row, so `reproject_once` can be driven deterministically
    /// without starting a real informer task.
    fn test_pod(
        tx: broadcast::Sender<WatchEvent>,
        objects: HashMap<String, DynamicObject>,
        rows: HashMap<String, ResourceRow>,
    ) -> Self {
        Active {
            tx,
            rows: Arc::new(RwLock::new(rows)),
            handle: tokio::spawn(async {}),
            idle_since: Arc::new(RwLock::new(None)),
            is_pod: true,
            is_pvc: false,
            resource_key: "/v1/Pod".to_string(),
            namespace: Some("ns".to_string()),
            group: String::new(),
            kind: "Pod".to_string(),
            objects: Arc::new(RwLock::new(objects)),
            by_name: Arc::new(RwLock::new(HashMap::new())),
            layout: Arc::new(RwLock::new(Some(table_layout("", "Pod", false, &[])))),
            headers: Arc::new(ArcSwap::from_pointee(Vec::new())),
            schema_lock: Arc::new(RwLock::new(())),
            reproject: Arc::new(Notify::new()),
            terminal: Arc::new(RwLock::new(None)),
        }
    }
}

/// Re-project every cached object in `objects` through `project`, updating
/// the live row cache and broadcasting only the rows that actually changed.
async fn reproject_entry(
    tx: &broadcast::Sender<WatchEvent>,
    objects: &RwLock<HashMap<String, DynamicObject>>,
    rows: &RwLock<HashMap<String, ResourceRow>>,
    project: impl Fn(&DynamicObject, &ResourceRow) -> ResourceRow,
) {
    let objs = objects.read().await;
    let mut rows = rows.write().await;
    for (uid, obj) in objs.iter() {
        let Some(current) = rows.get(uid) else {
            continue;
        };
        let new_row = project(obj, current);
        if current != &new_row {
            rows.insert(uid.clone(), new_row.clone());
            let _ = tx.send(WatchEvent::Applied { row: new_row });
        }
    }
}

fn name_key(object: &DynamicObject) -> NameKey {
    (
        object.metadata.namespace.clone(),
        object.metadata.name.clone().unwrap_or_default(),
    )
}

async fn publish_terminal(
    tx: &broadcast::Sender<WatchEvent>,
    terminal: &RwLock<Option<WatchEvent>>,
    message: String,
) {
    let event = WatchEvent::Forbidden { message };
    *terminal.write().await = Some(event.clone());
    let _ = tx.send(event);
}

async fn publish_error(
    tx: &broadcast::Sender<WatchEvent>,
    terminal: &RwLock<Option<WatchEvent>>,
    message: String,
) {
    let event = WatchEvent::Error { message };
    *terminal.write().await = Some(event.clone());
    let _ = tx.send(event);
}

/// Walks an error's source chain, stringifying each link, and reports whether
/// any of them contains one of `needles`. Shared core for [`is_auth_error`]
/// and [`is_forbidden_error`] so both stay robust against `kube`'s error-enum
/// shape changes in the same way. Delegates to [`matches_status_str`] once the
/// chain is stringified, so that pure string-matching logic is unit-testable
/// without constructing a real `watcher::Error`.
fn matches_status(e: &(dyn std::error::Error + 'static), needles: &[&str]) -> bool {
    use std::error::Error;
    let mut src: Option<&dyn Error> = Some(e);
    while let Some(err) = src {
        if matches_status_str(&err.to_string(), needles) {
            return true;
        }
        src = err.source();
    }
    false
}

/// Pure string-matching core of [`matches_status`]: does `s` contain any of
/// `needles`? Extracted so the status-matching behaviour can be unit tested
/// directly on plain strings, without needing to construct a real
/// `watcher::Error` source chain.
fn matches_status_str(s: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| s.contains(needle))
}

/// Whether a watcher error is an authentication failure (HTTP 401), which means
/// the bearer token has rotated and the watch must be rebuilt with the
/// hot-swapped client. Walks the error source chain and matches on the status
/// text so it stays robust across `kube`'s error-enum shape changes. A 403
/// (RBAC) is handled separately by [`is_forbidden_error`]: rebuilding wouldn't
/// help there, so the watch is stopped instead of retried.
fn is_auth_error(e: &TableError) -> bool {
    matches_status(e, &["Unauthorized", "401"])
}

/// Whether a watcher error is an authorization failure (HTTP 403): under token
/// passthrough this means the subject genuinely lacks RBAC for this
/// kind/namespace, so retrying (with or without a rebuilt client) can never
/// succeed. Callers should surface this to the user once and stop the watch,
/// rather than let it self-heal/retry forever.
fn is_forbidden_error(e: &TableError) -> bool {
    matches_status(e, &["Forbidden", "403"])
}

/// Backoff before retrying or rebuilding a watch stream: 1, 2, 4, 8, 16, 30s
/// (capped) plus up to ~1s of jitter so many informers don't reconnect in
/// lockstep after a shared outage.
pub(crate) fn rebuild_backoff(attempt: u32) -> Duration {
    let secs = (1u64 << attempt.min(5)).min(30);
    let jitter_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_millis()))
        .unwrap_or(0);
    Duration::from_secs(secs) + Duration::from_millis(jitter_ms)
}

fn usage_for(obj: &DynamicObject, cache: &UsageWithTrend) -> Option<UsageEntry> {
    let ns = obj.metadata.namespace.as_deref().unwrap_or("");
    let name = obj.metadata.name.as_deref()?;
    cache.get(&format!("{ns}/{name}")).copied()
}

fn pvc_usage_for(obj: &DynamicObject, cache: &PvcUsageMap) -> Option<PvcUsage> {
    let ns = obj.metadata.namespace.as_deref()?;
    let name = obj.metadata.name.as_deref()?;
    cache.get(&format!("{ns}/{name}")).copied()
}

/// Pick the right enrichment for an incoming object based on which caches apply
/// to this kind. Most kinds pass (None, None); Pods pass a usage entry; PVCs
/// pass a PvcUsage.
fn enrichments(
    obj: &DynamicObject,
    is_pod: bool,
    is_pvc: bool,
    pod_cache: &UsageWithTrend,
    pvc_cache: &PvcUsageMap,
) -> (Option<UsageEntry>, Option<PvcUsage>) {
    (
        is_pod.then(|| usage_for(obj, pod_cache)).flatten(),
        is_pvc.then(|| pvc_usage_for(obj, pvc_cache)).flatten(),
    )
}

/// Background task: refresh pod usage from metrics-server. One fetch serves every
/// pod informer across every per-user registry sharing this `Enrichment`. No-op
/// when metrics-server isn't installed. Cache-only — re-projecting cached rows
/// into each per-user registry's broadcast streams is done by a per-user timer
/// (see the per-registry re-project loop), not here.
fn spawn_metrics_scrape(enrichment: Arc<Enrichment>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tick.tick().await;
            let client = (*enrichment.cluster.client()).clone();
            let fresh = crate::metrics::pod_usage(&client).await;
            if fresh.is_empty() {
                continue;
            }

            // Get current timestamp for metrics history
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            // Preserve previous samples for trend detection, then merge new data.
            // Hold the lock for the entire update to avoid race conditions.
            let mut usage = enrichment.pod_usage.write().await;
            let mut history = enrichment.metrics_history.write().await;

            // Increment miss counter for all existing entries; reset below
            // for pods that appear in this scrape.
            for entry in usage.values_mut() {
                entry.misses = entry.misses.saturating_add(1);
                entry.prev_cpu = Some(entry.cpu);
                entry.prev_mem = Some(entry.mem);
            }
            // Update current values for fresh entries and record history
            for (k, (cpu, mem)) in &fresh {
                let entry = usage.entry(k.clone()).or_default();
                entry.misses = 0; // seen this scrape — reset
                entry.cpu = *cpu;
                entry.mem = *mem;
                // For new entries, set prev to current (no trend on first sample)
                if !entry.has_prev() {
                    entry.prev_cpu = Some(*cpu);
                    entry.prev_mem = Some(*mem);
                }

                // Record metrics history
                let pod_history = history.entry(k.clone()).or_insert_with(VecDeque::new);
                pod_history.push_back(MetricsPoint {
                    timestamp,
                    cpu: *cpu,
                    mem: *mem,
                });
                // Keep only the last MAX_HISTORY_POINTS
                while pod_history.len() > MAX_HISTORY_POINTS {
                    pod_history.pop_front();
                }
            }
            // Evict entries absent for >3 consecutive scrapes (~45s). A single
            // partial response (e.g. one node temporarily unreachable) only
            // increments the miss counter, preserving history across transient gaps.
            usage.retain(|_, v| v.misses <= 3);
            history.retain(|k, _| usage.contains_key(k));
        }
    });
}

/// Background task: refresh PVC filesystem usage from each kubelet's
/// `/proxy/stats/summary`. No-op when RBAC lacks `nodes/proxy`. Cache-only — see
/// [`spawn_metrics_scrape`] for why re-projection is not done here.
fn spawn_pvc_usage_scrape(enrichment: Arc<Enrichment>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            let client = (*enrichment.cluster.client()).clone();
            let fresh = crate::metrics::pvc_usage(&client).await;

            // Always replace the cache; the kubelet scan is authoritative
            // about which PVCs are currently mounted and how full they are.
            *enrichment.pvc_usage.write().await = fresh;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn distinct_user_registries_track_subscriptions_independently() {
        use kube::core::ApiResource;
        let enrich = Enrichment::new(crate::client::ClusterAccess::for_test());
        let (schema_tx, _) = broadcast::channel::<()>(4);

        let reg_a = InformerRegistry::new(
            crate::client::ClusterAccess::for_test(),
            enrich.clone(),
            schema_tx.subscribe(),
        );
        let reg_b = InformerRegistry::new(
            crate::client::ClusterAccess::for_test(),
            enrich.clone(),
            schema_tx.subscribe(),
        );

        let ar = ApiResource {
            group: "".into(),
            version: "v1".into(),
            api_version: "v1".into(),
            kind: "ConfigMap".into(),
            plural: "configmaps".into(),
        };
        let ha = reg_a
            .subscribe(&ar, "", "ConfigMap", true, Some("ns".into()), None)
            .await;
        let hb = reg_b
            .subscribe(&ar, "", "ConfigMap", true, Some("ns".into()), None)
            .await;

        assert!(reg_a.has_active_subscribers().await);
        assert!(reg_b.has_active_subscribers().await);
        drop(ha);
        assert!(!reg_a.has_active_subscribers().await);
        assert!(reg_b.has_active_subscribers().await);
        drop(hb);
        assert!(!reg_b.has_active_subscribers().await);
    }

    #[tokio::test]
    async fn terminal_forbidden_is_replayed_to_new_subscribers() {
        use kube::core::ApiResource;

        let cluster = crate::client::ClusterAccess::for_test();
        let enrich = Enrichment::new(cluster.clone());
        let (_schema_tx, schema_rx) = broadcast::channel(4);
        let registry = InformerRegistry::new(cluster, enrich, schema_rx);
        let (tx, _rx) = broadcast::channel(4);
        let entry = Active::test_pod(tx, HashMap::new(), HashMap::new());
        *entry.terminal.write().await = Some(WatchEvent::Forbidden {
            message: "pods is forbidden".into(),
        });
        registry.active.lock().await.insert(
            WatchKey {
                resource_key: "/v1/Pod".into(),
                namespace: Some("ns".into()),
                selector: None,
            },
            entry,
        );
        let ar = ApiResource {
            group: "".into(),
            version: "v1".into(),
            api_version: "v1".into(),
            kind: "Pod".into(),
            plural: "pods".into(),
        };

        let mut handle = registry
            .subscribe(&ar, "", "Pod", true, Some("ns".into()), None)
            .await;
        let event = tokio::time::timeout(Duration::from_millis(100), handle.rx.recv())
            .await
            .expect("terminal event should be sent immediately")
            .expect("terminal sender remains alive");
        assert_eq!(
            event,
            WatchEvent::Forbidden {
                message: "pods is forbidden".into()
            }
        );
        assert_eq!(
            handle.rx.recv().await,
            Err(broadcast::error::RecvError::Closed)
        );
    }

    /// Build a minimal cached pod `DynamicObject` + its initial (usage-less)
    /// projected row, keyed by `uid`.
    fn test_pod_object(
        uid: &str,
        name: &str,
        namespace: &str,
    ) -> (DynamicObject, ResourceRow, usize) {
        let pod: DynamicObject = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": name,
                "namespace": namespace,
                "uid": uid,
            },
            "spec": {},
            "status": {},
        }))
        .expect("valid pod manifest");
        let layout = table_layout("", "Pod", false, &[]);
        let cpu_index = layout
            .columns
            .iter()
            .position(|column| column == "CPU")
            .expect("pod layout has a CPU column");
        let table_row = crate::table::TableRow {
            object: Some(pod.clone()),
            ..Default::default()
        };
        let row = project_table_row("", "Pod", &layout, &table_row, None, None)
            .expect("visible pod")
            .0;
        (pod, row, cpu_index)
    }

    #[tokio::test]
    async fn reproject_once_rebroadcasts_only_on_change() {
        let cluster = crate::client::ClusterAccess::for_test();
        let enrich = Enrichment::new(cluster.clone());
        let (_schema_tx, schema_rx) = broadcast::channel::<()>(4);
        let registry = InformerRegistry::new(cluster, enrich.clone(), schema_rx);

        let (pod, row, cpu_index) = test_pod_object("uid-1", "pod1", "ns");
        let uid = row.uid.clone();

        let (tx, mut rx) = broadcast::channel(16);
        let entry = Active::test_pod(
            tx,
            HashMap::from([(uid.clone(), pod)]),
            HashMap::from([(uid.clone(), row)]),
        );
        let key = WatchKey {
            resource_key: "/v1/Pod".to_string(),
            namespace: Some("ns".to_string()),
            selector: None,
        };
        registry.active.lock().await.insert(key, entry);

        // No subscribers other than `rx` are watching yet, so before any usage
        // is seeded the initial CPU cell is "n/a" (asserted implicitly below
        // via the change it becomes once usage lands).

        // Seed usage that will change the projected CPU cell.
        registry.enrich.pod_usage.write().await.insert(
            "ns/pod1".to_string(),
            UsageEntry {
                cpu: 0.5,
                mem: 100.0 * 1024.0 * 1024.0,
                ..Default::default()
            },
        );

        reproject_once(&registry).await;

        let event = rx
            .try_recv()
            .expect("row changed by usage — should broadcast");
        match event {
            WatchEvent::Applied { row } => {
                assert_eq!(row.uid, uid);
                assert_eq!(
                    row.cells[cpu_index], "500",
                    "CPU cell should reflect seeded usage"
                );
            }
            other => panic!("expected Applied, got {other:?}"),
        }

        // No further change: a second tick with identical usage must not
        // re-broadcast (change-detection).
        reproject_once(&registry).await;
        assert!(
            matches!(
                rx.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "unchanged usage must not trigger another broadcast"
        );
    }

    /// The registry's background loops must hold `Weak<InformerRegistry>`, not
    /// `Arc`, or dropping the last strong
    /// `Arc` (held by `Backend`) never frees the registry — leaking the user's
    /// `ClusterAccess` connection pool and every active watch task forever.
    #[tokio::test]
    async fn dropping_registry_frees_it_no_strong_ref_cycle() {
        let cluster = crate::client::ClusterAccess::for_test();
        let enrich = Enrichment::new(cluster.clone());
        let (_schema_tx, schema_rx) = broadcast::channel::<()>(4);
        let registry = InformerRegistry::new(cluster, enrich, schema_rx);

        let weak = Arc::downgrade(&registry);
        drop(registry);

        // Give any mid-tick upgrade a moment to release, then assert no strong
        // refs remain.
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            weak.upgrade().is_none(),
            "background loops must hold Weak, not Arc — registry leaked"
        );
    }

    /// `Active`'s `Drop` impl must abort its watch task: a bare
    /// `JoinHandle` drop detaches the task in tokio rather than stopping it,
    /// which would leave the watch running against the user's client forever
    /// after the entry is removed (idle reaper) or the whole registry drops.
    #[tokio::test]
    async fn active_drop_aborts_its_watch_task() {
        let (tx, _rx) = broadcast::channel(4);
        let handle = tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });
        let active = Active {
            tx,
            rows: Arc::new(RwLock::new(HashMap::new())),
            handle,
            idle_since: Arc::new(RwLock::new(None)),
            is_pod: false,
            is_pvc: false,
            resource_key: "/v1/Pod".to_string(),
            namespace: None,
            group: String::new(),
            kind: "Pod".to_string(),
            objects: Arc::new(RwLock::new(HashMap::new())),
            by_name: Arc::new(RwLock::new(HashMap::new())),
            layout: Arc::new(RwLock::new(None)),
            headers: Arc::new(ArcSwap::from_pointee(Vec::new())),
            schema_lock: Arc::new(RwLock::new(())),
            reproject: Arc::new(Notify::new()),
            terminal: Arc::new(RwLock::new(None)),
        };
        // We can't hold `active.handle` after moving `active` into drop, so
        // grab a second handle via `abort_handle` before dropping.
        let abort_handle = active.handle.abort_handle();
        drop(active);
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            abort_handle.is_finished(),
            "dropping Active must abort its watch task"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn live_service_snapshot_uses_server_table_schema() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cluster = Arc::new(
            crate::client::ClusterAccess::connect_with_default()
                .await
                .expect("connect to current cluster"),
        );
        let enrich = Enrichment::new(cluster.clone());
        let (_schema_tx, schema_rx) = broadcast::channel(4);
        let registry = InformerRegistry::new(cluster, enrich, schema_rx);
        let ar = kube::core::ApiResource {
            group: String::new(),
            version: "v1".into(),
            api_version: "v1".into(),
            kind: "Service".into(),
            plural: "services".into(),
        };
        let mut handle = registry
            .subscribe(&ar, "", "Service", true, Some("kube-system".into()), None)
            .await;
        let event = tokio::time::timeout(Duration::from_secs(10), handle.rx.recv())
            .await
            .expect("Table snapshot timeout")
            .expect("Table snapshot stream closed");
        let WatchEvent::Snapshot { columns, rows } = event else {
            panic!("expected initial snapshot");
        };
        assert_eq!(
            columns,
            [
                "Name",
                "Type",
                "Cluster-IP",
                "External-IP",
                "Port(s)",
                "Age"
            ]
        );
        assert!(rows.iter().any(|row| row.name == "roder"));
        assert!(rows.iter().all(|row| row.cells.len() == columns.len()));
    }

    /// The pure string-walk core used by `is_auth_error` and
    /// `is_forbidden_error`. Exercised directly on plain strings
    /// since constructing a real `watcher::Error` source chain isn't practical
    /// in a unit test.
    #[test]
    fn forbidden_error_is_detected() {
        let forbidden_needles = ["Forbidden", "403"];
        assert!(matches_status_str(
            "ApiError: pods is forbidden: 403",
            &forbidden_needles
        ));
        assert!(!matches_status_str(
            "connection refused",
            &forbidden_needles
        ));
    }

    #[test]
    fn auth_error_is_detected() {
        let auth_needles = ["Unauthorized", "401"];
        assert!(matches_status_str("ApiError: Unauthorized", &auth_needles));
        assert!(matches_status_str(
            "ApiError: request failed: 401",
            &auth_needles
        ));
        assert!(!matches_status_str("connection refused", &auth_needles));
    }

    #[test]
    fn matches_status_str_is_needle_disjoint() {
        // A forbidden-style message must not match the auth needles and
        // vice versa — the two error paths must not accidentally overlap.
        assert!(!matches_status_str(
            "pods is forbidden: 403",
            &["Unauthorized", "401"]
        ));
        assert!(!matches_status_str(
            "Unauthorized: 401",
            &["Forbidden", "403"]
        ));
    }
}
