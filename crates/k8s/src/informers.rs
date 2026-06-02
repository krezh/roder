use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use kube::api::{Api, DynamicObject};
use kube::runtime::watcher::{self, Event};
use kube::core::ApiResource;
use roder_core::{ResourceRow, Trend, WatchEvent};
use serde::Serialize;
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::client::{make_api, ClusterAccess};
use crate::printer_columns::{self, ColumnMap, PrinterCol};
use crate::project::project_row;

const CHANNEL_CAP: usize = 4096;
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

/// A single metrics data point with timestamp.
#[derive(Clone, Copy, Serialize)]
pub struct MetricsPoint {
    pub timestamp: u64, // Unix timestamp in seconds
    pub cpu: f64,
    pub mem: f64,
}

/// Time-series history for a single pod (namespace/name).
/// Stores up to MAX_HISTORY_POINTS data points.
pub type PodMetricsHistory = VecDeque<MetricsPoint>;

/// Maximum number of data points to keep per pod (roughly 1 hour at 15s intervals).
const MAX_HISTORY_POINTS: usize = 240;

/// Map of pod key (namespace/name) to its metrics history.
pub type MetricsHistory = HashMap<String, PodMetricsHistory>;

/// Secondary index key: (namespace, name) → uid for O(1) object lookup by name.
type NameKey = (Option<String>, String);
type NameIndex = Arc<RwLock<HashMap<NameKey, String>>>;

struct Active {
    tx: broadcast::Sender<WatchEvent>,
    rows: Arc<RwLock<HashMap<String, ResourceRow>>>,
    handle: tokio::task::JoinHandle<()>,
    /// Last time a subscriber was present (for idle eviction).
    idle_since: Arc<RwLock<Option<Instant>>>,
    /// Whether this informer watches pods (gets live CPU/mem enrichment).
    is_pod: bool,
    /// Resource key (`group/version/kind`) and namespace this informer covers, so
    /// the detail handler can locate the right cache.
    resource_key: String,
    namespace: Option<String>,
    /// Last-seen full objects by uid, so detail is served from cache (and metric
    /// refreshes re-project pods) without an extra apiserver round-trip.
    objects: Arc<RwLock<HashMap<String, DynamicObject>>>,
    /// Secondary index: (namespace, name) → uid for O(1) object lookup by name.
    by_name: NameIndex,
}

/// What a caller gets when subscribing to a live list: the current contents plus
/// a receiver for subsequent deltas.
pub struct WatchHandle {
    pub snapshot: Vec<ResourceRow>,
    pub rx: broadcast::Receiver<WatchEvent>,
    /// Live row cache (used by the SSE handler to send a re-snapshot on broadcast lag).
    pub rows: Arc<RwLock<HashMap<String, ResourceRow>>>,
}

/// Registry of shared informers. One watch per (resource, namespace) is kept on
/// the API server regardless of how many UI clients subscribe — that is the
/// mechanism that keeps roder kind to etcd.
pub struct InformerRegistry {
    cluster: Arc<ClusterAccess>,
    active: Mutex<HashMap<WatchKey, Active>>,
    /// Shared pod usage cache (current + previous), refreshed once for all pod informers.
    pod_usage: Arc<RwLock<UsageWithTrend>>,
    /// Time-series metrics history for pods, used for graphs.
    metrics_history: Arc<RwLock<MetricsHistory>>,
    /// CRD-declared printer columns, indexed by (group, kind), for generic rendering.
    columns: Arc<ColumnMap>,
}

impl InformerRegistry {
    pub fn new(cluster: Arc<ClusterAccess>, columns: Arc<ColumnMap>) -> Arc<Self> {
        let registry = Arc::new(Self {
            cluster,
            active: Mutex::new(HashMap::new()),
            pod_usage: Arc::new(RwLock::new(HashMap::new())),
            metrics_history: Arc::new(RwLock::new(HashMap::new())),
            columns,
        });
        spawn_reaper(registry.clone());
        spawn_metrics_refresh(registry.clone());
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

        let crd = printer_columns::cols_for(&self.columns, group, kind).to_vec();
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
                self.pod_usage.clone(),
                crd,
            )
        });

        // Subscribe first so no delta is missed between snapshot and stream.
        let rx = entry.tx.subscribe();
        *entry.idle_since.write().await = None;
        let snapshot = entry.rows.read().await.values().cloned().collect();
        WatchHandle { snapshot, rx, rows: entry.rows.clone() }
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
        let active = self.active.lock().await;
        for entry in active.values() {
            if entry.resource_key != resource_key {
                continue;
            }
            // A namespace-scoped informer can't hold an object from another namespace.
            if let (Some(en), Some(want)) = (entry.namespace.as_deref(), namespace) {
                if en != want {
                    continue;
                }
            }
            // O(1) name lookup using the secondary index.
            let lookup = (namespace.map(|s| s.to_string()), name.to_string());
            if let Some(uid) = entry.by_name.read().await.get(&lookup).cloned() {
                if let Some(obj) = entry.objects.read().await.get(&uid).cloned() {
                    return Some(obj);
                }
            }
        }
        None
    }

    /// Get the metrics history for a specific pod (namespace/name).
    pub async fn pod_metrics_history(&self, namespace: &str, name: &str) -> Option<Vec<MetricsPoint>> {
        let key = format!("{}/{}", namespace, name);
        let history = self.metrics_history.read().await;
        history.get(&key).map(|h| h.iter().copied().collect())
    }
}

fn start_informer(
    cluster: Arc<ClusterAccess>,
    ar: ApiResource,
    group: String,
    kind: String,
    namespaced: bool,
    namespace: Option<String>,
    selector: Option<String>,
    pod_usage: Arc<RwLock<UsageWithTrend>>,
    crd: Vec<PrinterCol>,
) -> Active {
    let is_pod = group.is_empty() && kind == "Pod";
    let resource_key = format!("{group}/{}/{kind}", ar.version);
    let active_ns = namespace.clone();
    let (tx, _rx) = broadcast::channel(CHANNEL_CAP);
    let rows: Arc<RwLock<HashMap<String, ResourceRow>>> = Arc::new(RwLock::new(HashMap::new()));
    let objects: Arc<RwLock<HashMap<String, DynamicObject>>> = Arc::new(RwLock::new(HashMap::new()));
    let by_name: NameIndex = Arc::new(RwLock::new(HashMap::new()));
    let idle_since: Arc<RwLock<Option<Instant>>> = Arc::new(RwLock::new(Some(Instant::now())));

    let task_tx = tx.clone();
    let task_rows = rows.clone();
    let task_objects = objects.clone();
    let task_by_name = by_name.clone();
    let handle = tokio::spawn(async move {
        // Outer loop rebuilds the watch (and picks up a refreshed client) after
        // an unrecoverable stream end (e.g. token rotated → 401).
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
                (obj.metadata.namespace.clone(), obj.metadata.name.clone().unwrap_or_default())
            };

            while let Some(event) = stream.next().await {
                match event {
                    Ok(Event::Init) => {
                        building.clear();
                        building_objs.clear();
                    }
                    Ok(Event::InitApply(obj)) => {
                        let usage = if is_pod { usage_for(&obj, &*pod_usage.read().await) } else { None };
                        let row = project_row(&group, &kind, &obj, usage, &crd);
                        building_objs.insert(row.uid.clone(), obj);
                        building.insert(row.uid.clone(), row);
                    }
                    Ok(Event::InitDone) => {
                        let rows_vec: Vec<ResourceRow> = building.values().cloned().collect();
                        // Rebuild the name index from the full init set.
                        let mut name_idx: HashMap<NameKey, String> = HashMap::new();
                        for (uid, obj) in building_objs.iter() {
                            name_idx.insert(name_key(obj), uid.clone());
                        }
                        *task_rows.write().await = std::mem::take(&mut building);
                        *task_objects.write().await = std::mem::take(&mut building_objs);
                        *task_by_name.write().await = name_idx;
                        let _ = task_tx.send(WatchEvent::Snapshot { rows: rows_vec });
                    }
                    Ok(Event::Apply(obj)) => {
                        let usage = if is_pod { usage_for(&obj, &*pod_usage.read().await) } else { None };
                        let row = project_row(&group, &kind, &obj, usage, &crd);
                        let uid = row.uid.clone();
                        task_objects.write().await.insert(uid.clone(), obj.clone());
                        task_rows.write().await.insert(uid.clone(), row.clone());
                        task_by_name.write().await.insert(name_key(&obj), uid);
                        let _ = task_tx.send(WatchEvent::Applied { row });
                    }
                    Ok(Event::Delete(obj)) => {
                        let row = project_row(&group, &kind, &obj, None, &crd);
                        task_rows.write().await.remove(&row.uid);
                        task_objects.write().await.remove(&row.uid);
                        task_by_name.write().await.remove(&name_key(&obj));
                        let _ = task_tx.send(WatchEvent::Deleted { uid: row.uid });
                    }
                    Err(e) => {
                        tracing::debug!("watch error for {group}/{kind}: {e}");
                    }
                }
            }
            // Stream ended; back off then rebuild with the latest client.
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    Active {
        tx,
        rows,
        handle,
        idle_since,
        is_pod,
        resource_key,
        namespace: active_ns,
        objects,
        by_name,
    }
}

/// Background task: stop informers that have had no subscribers for a while, so
/// we don't hold watches for views nobody is looking at.
fn spawn_reaper(registry: Arc<InformerRegistry>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tick.tick().await;
            let mut active = registry.active.lock().await;
            let mut to_remove = Vec::new();
            for (key, entry) in active.iter() {
                if entry.tx.receiver_count() == 0 {
                    let mut idle = entry.idle_since.write().await;
                    match *idle {
                        Some(since) if since.elapsed() >= IDLE_GRACE => {
                            to_remove.push(key.clone());
                        }
                        None => *idle = Some(Instant::now()),
                        _ => {}
                    }
                } else {
                    *entry.idle_since.write().await = None;
                }
            }
            for key in to_remove {
                if let Some(entry) = active.remove(&key) {
                    entry.handle.abort();
                    tracing::debug!("evicted idle informer {}", key.resource_key);
                }
            }
        }
    });
}

fn usage_for(obj: &DynamicObject, cache: &UsageWithTrend) -> Option<UsageEntry> {
    let ns = obj.metadata.namespace.as_deref().unwrap_or("");
    let name = obj.metadata.name.as_deref()?;
    cache.get(&format!("{ns}/{name}")).copied()
}

/// Background task: refresh pod usage from metrics-server and re-broadcast changed
/// pod rows so CPU/mem stay live between watch events. One fetch serves every pod
/// informer. No-op when metrics-server isn't installed.
fn spawn_metrics_refresh(registry: Arc<InformerRegistry>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tick.tick().await;
            let client = (*registry.cluster.client()).clone();
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
            {
                let mut usage = registry.pod_usage.write().await;
                let mut history = registry.metrics_history.write().await;
                
                // Set prev for all existing entries
                for entry in usage.values_mut() {
                    entry.prev_cpu = Some(entry.cpu);
                    entry.prev_mem = Some(entry.mem);
                }
                // Update current values for fresh entries and record history
                for (k, (cpu, mem)) in &fresh {
                    let entry = usage.entry(k.clone()).or_default();
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
                // Remove entries that disappeared from metrics-server
                usage.retain(|k, _| fresh.contains_key(k));
                history.retain(|k, _| fresh.contains_key(k));
            }

            let usage = registry.pod_usage.read().await.clone();
            let active = registry.active.lock().await;
            for entry in active.values() {
                // Only re-project pod informers that someone is actually watching.
                if !entry.is_pod || entry.tx.receiver_count() == 0 {
                    continue;
                }
                let objs = entry.objects.read().await;
                let mut rows = entry.rows.write().await;
                for (uid, obj) in objs.iter() {
                    let new_row = project_row("", "Pod", obj, usage_for(obj, &usage), &[]);
                    if rows.get(uid) != Some(&new_row) {
                        rows.insert(uid.clone(), new_row.clone());
                        let _ = entry.tx.send(WatchEvent::Applied { row: new_row });
                    }
                }
            }
        }
    });
}
