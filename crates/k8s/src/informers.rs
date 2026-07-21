use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use futures::StreamExt;
use kube::api::{Api, DynamicObject};
use kube::core::ApiResource;
use kube::runtime::watcher::{self, Event};
use roder_core::{MetricsPoint, ResourceRow, Trend, WatchEvent};
use tokio::sync::{broadcast, Mutex, Notify, RwLock};

use crate::client::{make_api, ClusterAccess};
use crate::metrics::PvcUsage;
use crate::printer_columns::{self, ColumnMap, PrinterCol};
use crate::project::{columns_for, project_row, should_hide};

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

/// Hot-swappable column state shared between the informer task, the registry's
/// `refresh_columns`, and the `WatchHandle`: the CRD printer columns used to
/// project cells, and the matching header names. Swapping both atomically when a
/// CRD changes is what lets an open table reflow its columns live.
type Crd = Arc<ArcSwap<Vec<PrinterCol>>>;
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
    /// (group, kind) for re-deriving columns when CRDs change.
    group: String,
    kind: String,
    /// Last-seen full objects by uid, so detail is served from cache (and metric
    /// refreshes re-project pods) without an extra apiserver round-trip.
    objects: Arc<RwLock<HashMap<String, DynamicObject>>>,
    /// Secondary index: (namespace, name) → uid for O(1) object lookup by name.
    by_name: NameIndex,
    /// Hot-swappable printer columns + header names (see [`Crd`]).
    crd: Crd,
    headers: Headers,
    /// Poked by `refresh_columns` to make the task re-list and re-project with the
    /// freshly-swapped columns (a fresh `Snapshot` down the live channel — the
    /// client never reconnects).
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
    pub rx: broadcast::Receiver<WatchEvent>,
    /// Live row cache (used by the SSE handler to send a re-snapshot on broadcast lag).
    pub rows: Arc<RwLock<HashMap<String, ResourceRow>>>,
    /// Current column headers, hot-swapped on CRD change — read for the initial
    /// snapshot and any lag-resync so headers always match the rows' cells.
    pub columns: Headers,
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
    /// CRD-declared printer columns, indexed by (group, kind). Injected/shared
    /// so every per-user registry sees the same hot-swapped columns; swapped by
    /// [`InformerRegistry::refresh_columns`] when CRDs change.
    columns: Arc<ArcSwap<ColumnMap>>,
}

impl InformerRegistry {
    /// `columns_rx` is a [`SharedCluster::subscribe_columns`](crate::shared::SharedCluster::subscribe_columns)
    /// receiver: fired whenever the shared CRD watch swaps in freshly-rebuilt
    /// columns, so this registry re-projects its own active informers against
    /// them without waiting for its next subscribe.
    pub fn new(
        cluster: Arc<ClusterAccess>,
        enrich: Arc<Enrichment>,
        columns: Arc<ArcSwap<ColumnMap>>,
        columns_rx: broadcast::Receiver<()>,
    ) -> Arc<Self> {
        let registry = Arc::new(Self {
            cluster,
            active: Mutex::new(HashMap::new()),
            enrich,
            columns,
        });
        // The 3 background loops hold only `Weak` — the registry's sole strong
        // owner is `Backend` (see `crates/k8s/src/backend/mod.rs`), so dropping
        // a `Backend` actually frees this registry, its `active` watch tasks
        // (see `impl Drop for Active`), and the user's `ClusterAccess`
        // connection pool, instead of leaking them forever.
        spawn_reaper(Arc::downgrade(&registry));
        spawn_reproject(Arc::downgrade(&registry));
        spawn_columns_watch(Arc::downgrade(&registry), columns_rx);
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

        let columns = self.columns.load();
        let crd = printer_columns::cols_for(&columns, group, kind).to_vec();
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
                crd,
            )
        });

        // Subscribe first so no delta is missed between snapshot and stream.
        let mut rx = entry.tx.subscribe();
        *entry.idle_since.write().await = None;
        let snapshot = entry.rows.read().await.values().cloned().collect();
        if let Some(event) = entry.terminal.read().await.clone() {
            let (terminal_tx, terminal_rx) = broadcast::channel(1);
            let _ = terminal_tx.send(event);
            rx = terminal_rx;
        }
        WatchHandle {
            snapshot,
            rx,
            rows: entry.rows.clone(),
            columns: entry.headers.clone(),
        }
    }

    /// For every active informer whose columns actually changed against
    /// `new_columns`, swap its columns and poke it to re-list + re-project.
    /// The connected clients keep their streams and just receive a new snapshot.
    ///
    /// Does NOT store `new_columns` into `self.columns`: that `ArcSwap` is the
    /// same shared instance owned and written by `SharedCluster` (see
    /// [`InformerRegistry::new`]), so the shared layer is the sole writer —
    /// this only reprojects each active informer against what's already there.
    pub async fn refresh_columns(&self, new_columns: Arc<ColumnMap>) {
        // Collect the pokes under the lock (no awaits), then they fire on the
        // informer tasks asynchronously.
        let new_map: &ColumnMap = &new_columns;
        let active = self.active.lock().await;
        for entry in active.values() {
            let new_crd = printer_columns::cols_for(new_map, &entry.group, &entry.kind).to_vec();
            let new_headers = columns_for(&entry.group, &entry.kind, &new_crd);
            if **entry.headers.load() == new_headers {
                continue; // columns unchanged for this kind
            }
            entry.crd.store(Arc::new(new_crd));
            entry.headers.store(Arc::new(new_headers));
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
    crd: Vec<PrinterCol>,
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

    // Hot-swappable columns: derive the initial headers from the starting CRD,
    // then wrap both so `refresh_columns` can swap them and poke `reproject`.
    let initial_headers = columns_for(&group, &kind, &crd);
    let crd: Crd = Arc::new(ArcSwap::from_pointee(crd));
    let headers: Headers = Arc::new(ArcSwap::from_pointee(initial_headers));
    let reproject = Arc::new(Notify::new());
    let terminal = Arc::new(RwLock::new(None));

    let task_tx = tx.clone();
    let task_rows = rows.clone();
    let task_objects = objects.clone();
    let task_by_name = by_name.clone();
    let task_pvc_usage = pvc_usage.clone();
    let task_crd = crd.clone();
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
        // `kube`'s `watcher` is self-healing: on a transient failure it yields
        // an `Err`, then re-lists and resumes on the same stream. Back off here
        // until a complete relist succeeds; WatchStreamExt::default_backoff resets
        // on the synthetic `Init` event emitted before each LIST, so it otherwise
        // stays at its minimum delay throughout an outage. We only rebuild the
        // stream when the client itself must change — i.e. the bearer token
        // rotated and the old client now 401s.
        // Rebuilding on *every* error would force a fresh LIST each time, which
        // both hammers etcd and doubles memory while the new list is buffered.
        let mut backoff_attempt: u32 = 0;
        loop {
            let client = (*cluster.client()).clone();
            let api: Api<DynamicObject> = make_api(client, &ar, namespaced, namespace.as_deref());

            let mut building: HashMap<String, ResourceRow> = HashMap::new();
            let mut building_objs: HashMap<String, DynamicObject> = HashMap::new();
            let cfg = match &selector {
                Some(s) => watcher::Config::default().labels(s),
                None => watcher::Config::default(),
            };
            let stream = watcher::watcher(api, cfg);
            futures::pin_mut!(stream);

            // Reusable helper to derive the name-based index key from an object.
            let name_key = |obj: &DynamicObject| {
                (
                    obj.metadata.namespace.clone(),
                    obj.metadata.name.clone().unwrap_or_default(),
                )
            };

            // Set when an auth error means we must rebuild with the refreshed client.
            let mut rebuild_for_auth = false;
            // Set when `refresh_columns` poked us to re-list with new columns.
            let mut reproject_now = false;
            loop {
                tokio::select! {
                    maybe = stream.next() => {
                        let Some(event) = maybe else { break };
                        match event {
                            Ok(Event::Init) => {
                                building.clear();
                                building_objs.clear();
                            }
                            Ok(Event::InitApply(mut obj)) => {
                                obj.metadata.managed_fields = None;
                                if should_hide(&group, &kind, &obj) {
                                    continue;
                                }
                                let (usage, pvc_u) = enrichments(
                                    &obj,
                                    is_pod,
                                    is_pvc,
                                    &*pod_usage.read().await,
                                    &*task_pvc_usage.read().await,
                                );
                                let guard = task_crd.load();
                                let crd_now: &[PrinterCol] = &guard;
                                let row = project_row(&group, &kind, &obj, usage, pvc_u, crd_now);
                                if cache_objects {
                                    building_objs.insert(row.uid.clone(), obj);
                                }
                                building.insert(row.uid.clone(), row);
                            }
                            Ok(Event::InitDone) => {
                                let rows_vec: Vec<ResourceRow> = building.values().cloned().collect();
                                // Rebuild the name index from the full init set.
                                let mut name_idx: HashMap<NameKey, String> = HashMap::new();
                                if cache_objects {
                                    for (uid, obj) in building_objs.iter() {
                                        name_idx.insert(name_key(obj), uid.clone());
                                    }
                                }
                                *task_rows.write().await = std::mem::take(&mut building);
                                *task_objects.write().await = std::mem::take(&mut building_objs);
                                *task_by_name.write().await = name_idx;
                                let columns = (**task_headers.load()).clone();
                                let _ = task_tx.send(WatchEvent::Snapshot { columns, rows: rows_vec });
                                // A completed (re)list means the stream is healthy
                                // again; reset the rebuild backoff.
                                backoff_attempt = 0;
                            }
                            Ok(Event::Apply(mut obj)) => {
                                obj.metadata.managed_fields = None;
                                if should_hide(&group, &kind, &obj) {
                                    continue;
                                }
                                let (usage, pvc_u) = enrichments(
                                    &obj,
                                    is_pod,
                                    is_pvc,
                                    &*pod_usage.read().await,
                                    &*task_pvc_usage.read().await,
                                );
                                let guard = task_crd.load();
                                let crd_now: &[PrinterCol] = &guard;
                                let row = project_row(&group, &kind, &obj, usage, pvc_u, crd_now);
                                let uid = row.uid.clone();
                                if cache_objects {
                                    let name_k = name_key(&obj);
                                    task_objects.write().await.insert(uid.clone(), obj);
                                    task_by_name.write().await.insert(name_k, uid.clone());
                                }
                                task_rows.write().await.insert(uid, row.clone());
                                let _ = task_tx.send(WatchEvent::Applied { row });
                            }
                            Ok(Event::Delete(obj)) => {
                                let guard = task_crd.load();
                                let crd_now: &[PrinterCol] = &guard;
                                let row = project_row(&group, &kind, &obj, None, None, crd_now);
                                task_rows.write().await.remove(&row.uid);
                                if cache_objects {
                                    task_objects.write().await.remove(&row.uid);
                                    task_by_name.write().await.remove(&name_key(&obj));
                                }
                                let _ = task_tx.send(WatchEvent::Deleted { uid: row.uid });
                            }
                            Err(e) => {
                                // A 403 means the subject genuinely lacks RBAC for
                                // this kind/namespace (this is expected under token
                                // passthrough) — retrying, rebuilt client or not,
                                // can never succeed and would just hammer the
                                // apiserver forever. Tell this user's stream once
                                // and end the task; the idle reaper cleans up the
                                // `Active` entry once its subscribers drop off.
                                if is_forbidden_error(&e) {
                                    tracing::debug!("watch forbidden (403) for {group}/{kind}; stopping: {e}");
                                    let event = WatchEvent::Forbidden {
                                        message: e.to_string(),
                                    };
                                    *task_terminal.write().await = Some(event.clone());
                                    let _ = task_tx.send(event);
                                    return;
                                }
                                // A 401 means the token rotated: the watcher would
                                // keep retrying with the stale client forever, so
                                // break to rebuild with the hot-swapped one. Every
                                // other error is transient — let the watcher
                                // self-heal in place (no LIST, no memory spike).
                                if is_auth_error(&e) {
                                    tracing::debug!("watch auth error for {group}/{kind}; rebuilding: {e}");
                                    rebuild_for_auth = true;
                                    break;
                                }
                                tracing::debug!("watch error for {group}/{kind} (self-healing): {e}");
                                let delay = rebuild_backoff(backoff_attempt);
                                backoff_attempt = backoff_attempt.saturating_add(1);
                                tokio::select! {
                                    _ = tokio::time::sleep(delay) => {}
                                    _ = task_reproject.notified() => {
                                        reproject_now = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    _ = task_reproject.notified() => {
                        // Columns changed: tear the watch down and re-list, so the
                        // next InitDone emits a fresh Snapshot carrying the new
                        // columns. The clients keep their streams — no reconnect.
                        reproject_now = true;
                        break;
                    }
                }
            }

            // A reproject re-lists immediately — it isn't a failure, so no backoff.
            if reproject_now {
                continue;
            }
            // We only get here on an auth rebuild or (in practice never) a truly
            // ended stream. Back off — exponentially, with jitter, capped — so a
            // sustained outage or token problem doesn't have every informer
            // reconnecting in lockstep.
            if !rebuild_for_auth {
                // Defensive: an unexpected clean stream end shouldn't hot-loop.
                backoff_attempt = backoff_attempt.max(1);
            }
            let delay = rebuild_backoff(backoff_attempt);
            backoff_attempt = backoff_attempt.saturating_add(1);
            tokio::time::sleep(delay).await;
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
        crd,
        headers,
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

/// Background task: on each notification from the shared CRD watch (see
/// [`InformerRegistry::new`]'s `columns_rx`), re-run [`InformerRegistry::refresh_columns`]
/// with the (already-swapped-in) shared columns, so this registry's active
/// informers reflow live along with every other per-user registry on the
/// cluster. A lagged receiver (registry busy, several rebuilds coalesced)
/// still catches up to the latest columns on the next tick, so it's skipped
/// rather than treated as an error; only a closed sender (the `SharedCluster`
/// is gone) ends the loop.
fn spawn_columns_watch(
    registry: std::sync::Weak<InformerRegistry>,
    mut columns_rx: broadcast::Receiver<()>,
) {
    tokio::spawn(async move {
        // `columns_rx.recv()` can block indefinitely when no CRD changes
        // occur, so a 30s liveness tick is raced alongside it purely so this
        // loop still notices — and exits — once the registry (held only
        // `Weak` here) has been dropped, rather than blocking on `recv()`
        // forever after the last real notification.
        let mut liveness = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                recv = columns_rx.recv() => {
                    match recv {
                        Err(broadcast::error::RecvError::Closed) => break,
                        // Lagged: fall through and do a catch-up refresh below.
                        Err(broadcast::error::RecvError::Lagged(_)) | Ok(()) => {}
                    }
                }
                _ = liveness.tick() => {}
            }
            let Some(registry) = registry.upgrade() else {
                break;
            };
            registry.refresh_columns(registry.columns.load_full()).await;
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
}

/// One tick of per-user re-projection: re-derive pod/PVC rows from the shared
/// `Enrichment` caches and re-broadcast whichever changed.
///
/// Phased locking mirrors `spawn_reaper` ("holding the Mutex for the minimum
/// time — no inner awaits here") and `cached_object` (clone the Arc handles
/// under the lock, drop it, THEN await the inner RwLocks): `active` is only
/// ever held long enough to clone handles into a `Vec`, never across an
/// await, so this never blocks `subscribe()`, `refresh_columns()`, or
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
            })
            .collect()
    };

    // Phase 2: outside the Mutex, briefly read the shared cache per entry and
    // reproject/broadcast changed rows — the `pod_usage`/`pvc_usage` read
    // guard is only held for one entry's reproject call, not the whole loop.
    for entry in entries {
        if entry.is_pod {
            let usage = registry.enrich.pod_usage.read().await;
            reproject_entry(&entry.tx, &entry.objects, &entry.rows, |obj| {
                project_row("", "Pod", obj, usage_for(obj, &usage), None, &[])
            })
            .await;
        } else if entry.is_pvc {
            let pvc = registry.enrich.pvc_usage.read().await;
            reproject_entry(&entry.tx, &entry.objects, &entry.rows, |obj| {
                project_row(
                    "",
                    "PersistentVolumeClaim",
                    obj,
                    None,
                    pvc_usage_for(obj, &pvc),
                    &[],
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
            crd: Arc::new(ArcSwap::from_pointee(Vec::new())),
            headers: Arc::new(ArcSwap::from_pointee(Vec::new())),
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
    project: impl Fn(&DynamicObject) -> ResourceRow,
) {
    let objs = objects.read().await;
    let mut rows = rows.write().await;
    for (uid, obj) in objs.iter() {
        let new_row = project(obj);
        if rows.get(uid) != Some(&new_row) {
            rows.insert(uid.clone(), new_row.clone());
            let _ = tx.send(WatchEvent::Applied { row: new_row });
        }
    }
}

/// Walks an error's source chain, stringifying each link, and reports whether
/// any of them contains one of `needles`. Shared core for [`is_auth_error`]
/// and [`is_forbidden_error`] so both stay robust against `kube`'s error-enum
/// shape changes in the same way. Delegates to [`matches_status_str`] once the
/// chain is stringified, so that pure string-matching logic is unit-testable
/// without constructing a real `watcher::Error`.
fn matches_status(e: &watcher::Error, needles: &[&str]) -> bool {
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
fn is_auth_error(e: &watcher::Error) -> bool {
    matches_status(e, &["Unauthorized", "401"])
}

/// Whether a watcher error is an authorization failure (HTTP 403): under token
/// passthrough this means the subject genuinely lacks RBAC for this
/// kind/namespace, so retrying (with or without a rebuilt client) can never
/// succeed. Callers should surface this to the user once and stop the watch,
/// rather than let it self-heal/retry forever.
fn is_forbidden_error(e: &watcher::Error) -> bool {
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

    /// A `columns_changed` notification (the `SharedCluster` counterpart, sent
    /// after a CRD-driven columns rebuild) makes the registry re-derive an
    /// active informer's headers from the freshly-swapped `ColumnMap` — this
    /// is the live column-reflow wiring, exercised end to end via the
    /// broadcast channel rather than by calling `refresh_columns` directly.
    #[tokio::test]
    async fn columns_changed_notification_reflows_active_informer_headers() {
        use kube::core::ApiResource;
        let cluster = crate::client::ClusterAccess::for_test();
        let enrich = Enrichment::new(cluster.clone());
        let columns: Arc<ArcSwap<ColumnMap>> =
            Arc::new(ArcSwap::from_pointee(ColumnMap::default()));
        let (columns_tx, columns_rx) = broadcast::channel::<()>(4);
        let registry = InformerRegistry::new(cluster, enrich, columns.clone(), columns_rx);

        let ar = ApiResource {
            group: "example.com".into(),
            version: "v1".into(),
            api_version: "example.com/v1".into(),
            kind: "Widget".into(),
            plural: "widgets".into(),
        };
        let handle = registry
            .subscribe(&ar, "example.com", "Widget", true, Some("ns".into()), None)
            .await;
        let headers_before = (**handle.columns.load()).clone();

        // Swap in a ColumnMap that declares a printer column for this CRD kind,
        // then notify — mirroring what `spawn_crd_watch_shared` does after a
        // rebuild: store the new columns, then broadcast `()`.
        let mut new_map = ColumnMap::default();
        new_map.insert(
            ("example.com".to_string(), "Widget".to_string()),
            vec![PrinterCol {
                name: "Phase".to_string(),
                json_path: "$.status.phase".to_string(),
                col_type: "string".to_string(),
            }],
        );
        columns.store(Arc::new(new_map));
        columns_tx.send(()).expect("registry is subscribed");

        // The reflow happens on a background task; poll briefly instead of a
        // fixed sleep so the test isn't flaky under load.
        let mut headers_after = headers_before.clone();
        for _ in 0..50 {
            headers_after = (**handle.columns.load()).clone();
            if headers_after != headers_before {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            headers_after,
            vec!["Phase".to_string()],
            "active informer's headers should reflect the newly-swapped CRD column"
        );
    }

    #[tokio::test]
    async fn distinct_user_registries_track_subscriptions_independently() {
        use kube::core::ApiResource;
        let enrich = Enrichment::new(crate::client::ClusterAccess::for_test());
        let columns: Arc<ArcSwap<ColumnMap>> =
            Arc::new(ArcSwap::from_pointee(ColumnMap::default()));
        let (columns_tx, _) = broadcast::channel::<()>(4);

        let reg_a = InformerRegistry::new(
            crate::client::ClusterAccess::for_test(),
            enrich.clone(),
            columns.clone(),
            columns_tx.subscribe(),
        );
        let reg_b = InformerRegistry::new(
            crate::client::ClusterAccess::for_test(),
            enrich.clone(),
            columns.clone(),
            columns_tx.subscribe(),
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
        let columns = Arc::new(ArcSwap::from_pointee(ColumnMap::default()));
        let (_columns_tx, columns_rx) = broadcast::channel(4);
        let registry = InformerRegistry::new(cluster, enrich, columns, columns_rx);
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
    fn test_pod_object(uid: &str, name: &str, namespace: &str) -> (DynamicObject, ResourceRow) {
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
        let row = project_row("", "Pod", &pod, None, None, &[]);
        (pod, row)
    }

    #[tokio::test]
    async fn reproject_once_rebroadcasts_only_on_change() {
        let cluster = crate::client::ClusterAccess::for_test();
        let enrich = Enrichment::new(cluster.clone());
        let columns: Arc<ArcSwap<ColumnMap>> =
            Arc::new(ArcSwap::from_pointee(ColumnMap::default()));
        let (_columns_tx, columns_rx) = broadcast::channel::<()>(4);
        let registry = InformerRegistry::new(cluster, enrich.clone(), columns, columns_rx);

        let (pod, row) = test_pod_object("uid-1", "pod1", "ns");
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
                assert_eq!(row.cells[3], "500", "CPU cell should reflect seeded usage");
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
        let columns: Arc<ArcSwap<ColumnMap>> =
            Arc::new(ArcSwap::from_pointee(ColumnMap::default()));
        let (_columns_tx, columns_rx) = broadcast::channel::<()>(4);
        let registry = InformerRegistry::new(cluster, enrich, columns, columns_rx);

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
            crd: Arc::new(ArcSwap::from_pointee(Vec::new())),
            headers: Arc::new(ArcSwap::from_pointee(Vec::new())),
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
