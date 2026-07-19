//! Shared data-transfer types used by both the Leptos UI (wasm + ssr) and the
//! server/k8s layer. Keep this crate dependency-light and wasm-safe: no tokio,
//! no kube-rs, no anything that can't compile to `wasm32-unknown-unknown`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Liveness/readiness payload served at `/health`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Health {
    pub status: HealthStatus,
    pub version: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
    Degraded,
}

impl Health {
    pub fn ok() -> Self {
        Self {
            status: HealthStatus::Ok,
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// Navigation grouping for a resource kind in the sidebar.
///
/// Fixed variants map to well-known k8s API groups. `Custom(String)` carries
/// the base domain of the CRD's API group (e.g. `"coreos.com"`) so that each
/// third-party operator gets its own collapsible section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Workloads,
    Config,
    Network,
    Storage,
    Rbac,
    Flux,
    ExternalSecrets,
    CertManager,
    Rook,
    Cluster,
    Custom(String),
}

impl Category {
    pub fn label(&self) -> String {
        match self {
            Category::Workloads => "Workloads".into(),
            Category::Config => "Config".into(),
            Category::Network => "Network".into(),
            Category::Storage => "Storage".into(),
            Category::Rbac => "RBAC".into(),
            Category::Flux => "Flux".into(),
            Category::ExternalSecrets => "External Secrets".into(),
            Category::CertManager => "cert-manager".into(),
            Category::Rook => "Rook Ceph".into(),
            Category::Cluster => "Cluster".into(),
            Category::Custom(name) => name.clone(),
        }
    }

    /// Stable display ordering of categories in the sidebar.
    pub fn order(&self) -> u8 {
        match self {
            Category::Cluster => 0,
            Category::Workloads => 1,
            Category::Config => 2,
            Category::Network => 3,
            Category::Storage => 4,
            Category::Rbac => 5,
            Category::Flux => 6,
            Category::ExternalSecrets => 7,
            Category::CertManager => 8,
            Category::Rook => 9,
            Category::Custom(_) => 10,
        }
    }

    /// True for dynamically-derived categories (third-party CRD groups).
    pub fn is_dynamic(&self) -> bool {
        matches!(self, Category::Custom(_))
    }
}

/// A resource type (GroupVersionKind) the user may browse, as surfaced to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceKind {
    /// Stable id used in URLs / SSE: `group/version/kind` (group empty for core).
    pub key: String,
    pub group: String,
    pub version: String,
    pub kind: String,
    pub plural: String,
    pub namespaced: bool,
    pub category: Category,
    /// Extra column headers (beyond the standard Name / Namespace / Age).
    pub columns: Vec<String>,
}

impl ResourceKind {
    pub fn make_key(group: &str, version: &str, kind: &str) -> String {
        format!("{group}/{version}/{kind}")
    }
}

/// Health/severity of a row, used for coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RowStatus {
    Ok,
    Pending,
    Warn,
    Error,
    /// Finished successfully (e.g. completed Job pods) — rendered neutral/gray.
    Done,
    /// No determinable status (e.g. ClusterRole) — rendered with the default colour.
    Unknown,
}

/// Trend direction for a metric cell (CPU / memory usage over time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Trend {
    #[default]
    None,
    Up,
    Down,
}

impl Trend {
    pub fn arrow(&self) -> Option<&'static str> {
        match self {
            Trend::Up => Some("↑"),
            Trend::Down => Some("↓"),
            Trend::None => None,
        }
    }
}

/// A single row in a resource list, projected from the watched object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRow {
    pub uid: String,
    pub namespace: Option<String>,
    pub name: String,
    /// RFC3339 creation timestamp (UI renders relative age).
    pub created: Option<String>,
    /// Values aligned with the kind's `columns`.
    pub cells: Vec<String>,
    /// Per-cell trend arrows, aligned with `cells`. Most are `Trend::None`;
    /// pod CPU/MEM cells carry `Up`/`Down` when usage changed vs the prior sample.
    pub trends: Vec<Trend>,
    pub status: RowStatus,
    /// `metadata.labels` from the Kubernetes object.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// SSE event for a live resource list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WatchEvent {
    /// Full current contents on (re)connect, or a re-projection after the kind's
    /// columns changed. Carries the current column headers so the table renders
    /// headers and cells from the same message — they can never disagree, and an
    /// open table reflows live when a CRD's `additionalPrinterColumns` change.
    Snapshot {
        columns: Vec<String>,
        rows: Vec<ResourceRow>,
    },
    /// A row was added or changed.
    Applied { row: ResourceRow },
    /// A row was removed (by uid).
    Deleted { uid: String },
    /// The watch hit an HTTP 403 (RBAC forbids this subject from watching this
    /// kind/namespace under token passthrough). Sent once, after which the
    /// informer stops retrying — the client should treat the stream as ended
    /// rather than expect further events.
    Forbidden { message: String },
}

/// Tagged SSE event for a multiplexed workspace watch stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiWatchEvent {
    pub key: String,
    pub event: WatchEvent,
}

/// Object detail payload (the expanded row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectDetail {
    pub name: String,
    pub namespace: Option<String>,
    /// The full object as JSON, for the describe-style info view.
    pub object: serde_json::Value,
    pub yaml: String,
    pub events: Vec<ObjectEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectEvent {
    pub type_: String,
    pub reason: String,
    pub message: String,
    pub age: Option<String>,
    pub count: i32,
}

/// One node in the fully-resolved recursive ownership tree of everything a
/// Kustomization/HelmRelease transitively creates (Flux "app-of-apps"),
/// resolved entirely server-side in one shot — see `Backend::resource_tree`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceTreeNode {
    /// The resource's Kind, e.g. "Kustomization", "HelmRelease", "Deployment".
    pub kind: String,
    /// The resource's API group (empty for core/v1 kinds).
    pub group: String,
    pub name: String,
    pub namespace: Option<String>,
    /// Resolved "group/version/Kind" catalog key (matches every other `key`
    /// used across the app, e.g. `ResourceKind::key`/`DetailTarget::key`), for
    /// opening this node in the detail drawer. `None` when the kind isn't
    /// currently discoverable in the cluster's catalog — the row renders
    /// greyed/non-clickable with a tooltip instead of erroring.
    pub key: Option<String>,
    /// The kind's sidebar category (Workloads/Flux/Rbac/…), for icon/color
    /// selection. `None` only when `key` is also `None` (kind not in the
    /// current catalog).
    pub category: Option<Category>,
    /// Ready/Suspended/Error status dot. Only populated for Kustomization/
    /// HelmRelease "owner" nodes — leaves intentionally carry `None` (no live
    /// status fetched per-leaf, by design, to keep the tree cheap).
    pub status: Option<RowStatus>,
    pub children: Vec<ResourceTreeNode>,
    /// Best-effort note when this node's *children* couldn't be (fully)
    /// resolved — RBAC denied reading the inventory/Helm secret, HelmRelease
    /// has no deployed revision yet, the recursion depth cap was hit, etc.
    /// `None` means either a leaf (no children expected) or children resolved
    /// cleanly (which may still mean zero children, e.g. an empty inventory).
    pub error: Option<String>,
}

/// Dashboard overview (M4).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ClusterOverview {
    pub kubernetes_version: String,
    pub nodes: Vec<NodeSummary>,
    pub namespace_count: u32,
    pub pod_total: u32,
    pub pod_running: u32,
    pub pod_pending: u32,
    pub pod_failed: u32,
    pub warnings: Vec<String>,
    pub flux: HealthRollup,
    pub external_secrets: HealthRollup,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NodeSummary {
    pub name: String,
    pub ready: bool,
    pub cpu_cores: Option<f64>,
    pub cpu_used: Option<f64>,
    pub mem_bytes: Option<f64>,
    pub mem_used: Option<f64>,
    /// `status.nodeInfo.kubeletVersion`, e.g. "v1.30.1".
    pub kubelet_version: Option<String>,
    /// `status.nodeInfo.osImage`, e.g. "Talos (v1.7.6)".
    pub os_image: Option<String>,
}

/// Read-only status returned directly by a Talos node through the machine API.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosNode {
    pub node: String,
    pub version: String,
    pub control_plane: bool,
    pub services: Vec<TalosService>,
    pub mounts: Vec<TalosMount>,
    pub interfaces: Vec<TalosNetworkInterface>,
    pub disk_inventory: Vec<TalosDisk>,
    pub volumes: Vec<TalosVolume>,
    pub disks: Vec<TalosDiskStat>,
    pub config_fingerprint: Option<String>,
    /// Per-section upstream failures; successful sections remain populated.
    pub errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosDmesg {
    pub log: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosCapabilities {
    pub read: bool,
    pub actions: bool,
    pub config: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosService {
    pub id: String,
    pub state: String,
    pub healthy: bool,
    pub message: String,
    pub health_unknown: bool,
    pub last_change: Option<String>,
    pub events: Vec<TalosServiceEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosServiceEvent {
    pub state: String,
    pub message: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosMount {
    pub filesystem: String,
    pub mounted_on: String,
    pub size: u64,
    pub available: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosNetworkInterface {
    pub name: String,
    pub addresses: Vec<String>,
    pub link_up: Option<bool>,
    pub operational_state: Option<String>,
    pub hardware_address: Option<String>,
    pub mtu: Option<u32>,
    pub speed_mbps: Option<u32>,
    pub duplex: Option<String>,
    pub kind: Option<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosDisk {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub transport: Option<String>,
    pub wwid: Option<String>,
    pub rotational: bool,
    pub readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosVolume {
    pub name: String,
    pub path: String,
    pub parent_path: Option<String>,
    pub partition_index: Option<u32>,
    pub size: u64,
    pub filesystem: Option<String>,
    pub phase: String,
    pub encryption: Option<String>,
    pub used_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosDiskStat {
    pub name: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub reads: u64,
    pub writes: u64,
    pub io_in_progress: u64,
    pub io_time_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosConfigDiff {
    pub node: String,
    pub fingerprint: String,
    pub peers: Vec<TalosConfigPeerDiff>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosConfigPeerDiff {
    pub node: String,
    pub fingerprint: Option<String>,
    pub matches: Option<bool>,
    pub differences: Vec<TalosConfigDifference>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosConfigDifference {
    pub path: String,
    pub node_value: Option<String>,
    pub peer_value: Option<String>,
    pub sensitive: bool,
}

/// A single pod metrics data point, shared between the server (serializes) and
/// the frontend (deserializes) via the `/api/metrics` endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MetricsPoint {
    pub timestamp: u64,
    pub cpu: f64,
    pub mem: f64,
}

/// Compact human-readable duration from a number of seconds ("5m", "2h3m", "1d4h").
pub fn format_age_secs(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}

/// Result of a sweep/sanitize operation.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CleanupSummary {
    pub pods_deleted: usize,
    pub jobs_deleted: usize,
}

/// Result of a node drain operation. `skipped` counts pods that did not require
/// eviction: DaemonSet-owned, mirror (static), terminal, and already-deleting pods.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DrainSummary {
    pub evicted: usize,
    pub skipped: usize,
    pub failed: Vec<String>,
}

/// Minimum overall drain timeout accepted by [`DrainOptions::validate`].
pub const DRAIN_TIMEOUT_MIN_SECS: u64 = 1;
/// Maximum overall drain timeout accepted by [`DrainOptions::validate`].
pub const DRAIN_TIMEOUT_MAX_SECS: u64 = 3_600;
/// Maximum pod termination grace period accepted by [`DrainOptions::validate`].
pub const DRAIN_GRACE_PERIOD_MAX_SECS: u32 = 86_400;

/// Options for a node drain, mirroring `kubectl drain`'s flags.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DrainOptions {
    /// Evict pods not managed by a controller (`--force`).
    pub force: bool,
    /// Evict pods using emptyDir volumes (`--delete-emptydir-data`).
    pub delete_emptydir_data: bool,
    /// Proceed while leaving DaemonSet pods in place (`--ignore-daemonsets`).
    /// When false, DaemonSet pods block the drain, matching kubectl's default.
    pub ignore_daemonsets: bool,
    /// Delete pods directly instead of the eviction API, bypassing
    /// PodDisruptionBudgets (`--disable-eviction`).
    pub disable_eviction: bool,
    /// Per-pod termination grace period override in seconds (`--grace-period`).
    pub grace_period: Option<u32>,
    /// Overall wall-clock budget for eviction + termination (`--timeout`).
    pub timeout_secs: u64,
}

impl Default for DrainOptions {
    fn default() -> Self {
        Self {
            force: false,
            delete_emptydir_data: false,
            ignore_daemonsets: true,
            disable_eviction: false,
            grace_period: None,
            timeout_secs: 60,
        }
    }
}

impl DrainOptions {
    /// Validate user-controlled duration fields against the shared drain bounds.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(DRAIN_TIMEOUT_MIN_SECS..=DRAIN_TIMEOUT_MAX_SECS).contains(&self.timeout_secs) {
            return Err("timeout must be between 1 and 3600 seconds");
        }
        if self
            .grace_period
            .is_some_and(|seconds| seconds > DRAIN_GRACE_PERIOD_MAX_SECS)
        {
            return Err("grace period must not exceed 86400 seconds");
        }
        Ok(())
    }
}

/// One pod blocking a drain, and which [`DrainOptions`] field would clear it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DrainBlocker {
    pub pod: String,
    pub reason: String,
    /// `"force"`, `"delete_emptydir_data"`, or `"ignore_daemonsets"`.
    pub clearable_by: String,
}

/// One progress event from a drain job. `seq` increases monotonically from 0
/// so a resubscribing client can de-duplicate replayed events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrainEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub kind: DrainEventKind,
}

/// Metadata needed by a refreshed client to reopen an unfinished drain job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveDrainJob {
    pub job: u64,
    pub key: String,
    pub name: String,
    pub power: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DrainEventKind {
    Started {
        total: usize,
    },
    Cordoned,
    Evicted {
        pod: String,
        done: usize,
        total: usize,
    },
    EvictFailed {
        pod: String,
        reason: String,
    },
    Blocked {
        blockers: Vec<DrainBlocker>,
    },
    WaitingTermination {
        pods: Vec<String>,
    },
    PowerRequested {
        action: String,
    },
    NodeReady,
    Done {
        summary: DrainSummary,
    },
    Error {
        message: String,
    },
    Cancelled,
}

/// One resource kind's RBAC access review row: which verbs the current
/// identity may perform on it, in the requested namespace scope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessRow {
    pub kind: String,
    pub group: String,
    pub namespaced: bool,
    /// `(verb, allowed)` pairs, in a fixed order (see `ACCESS_REVIEW_VERBS`).
    pub verbs: Vec<(String, bool)>,
}

/// Verbs checked by the access review, in display order.
pub const ACCESS_REVIEW_VERBS: &[&str] = &["get", "list", "create", "patch", "delete"];

/// Counts of resources by reconciled/suspended/failing for a CRD family.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthRollup {
    pub total: u32,
    pub ready: u32,
    pub suspended: u32,
    pub failing: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FiringAlert {
    pub fingerprint: String,
    pub name: String,
    pub severity: String,
    pub summary: String,
    pub description: String,
    pub starts_at: String,
    pub labels: std::collections::HashMap<String, String>,
    pub silenced: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_age_seconds() {
        assert_eq!(format_age_secs(0), "0s");
        assert_eq!(format_age_secs(45), "45s");
        assert_eq!(format_age_secs(59), "59s");
    }

    #[test]
    fn format_age_minutes() {
        assert_eq!(format_age_secs(60), "1m");
        assert_eq!(format_age_secs(90), "1m");
        assert_eq!(format_age_secs(3599), "59m");
    }

    #[test]
    fn format_age_hours() {
        assert_eq!(format_age_secs(3600), "1h0m");
        assert_eq!(format_age_secs(3660), "1h1m");
        assert_eq!(format_age_secs(86399), "23h59m");
    }

    #[test]
    fn format_age_days() {
        assert_eq!(format_age_secs(86400), "1d0h");
        assert_eq!(format_age_secs(86400 + 3600 * 5), "1d5h");
        assert_eq!(format_age_secs(86400 * 7 + 3600 * 12), "7d12h");
    }

    #[test]
    fn drain_options_defaults() {
        let o = DrainOptions::default();
        assert!(!o.force && !o.delete_emptydir_data && !o.disable_eviction);
        assert!(o.ignore_daemonsets);
        assert_eq!(o.timeout_secs, 60);
        assert_eq!(o.grace_period, None);
        // An empty JSON object deserializes to the same defaults.
        let from_empty: DrainOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(from_empty, o);
    }

    #[test]
    fn drain_options_validates_timeout_boundaries() {
        let mut options = DrainOptions {
            timeout_secs: DRAIN_TIMEOUT_MIN_SECS,
            ..Default::default()
        };
        assert_eq!(options.validate(), Ok(()));

        options.timeout_secs = DRAIN_TIMEOUT_MAX_SECS;
        assert_eq!(options.validate(), Ok(()));

        options.timeout_secs = DRAIN_TIMEOUT_MIN_SECS - 1;
        assert_eq!(
            options.validate(),
            Err("timeout must be between 1 and 3600 seconds")
        );

        options.timeout_secs = DRAIN_TIMEOUT_MAX_SECS + 1;
        assert_eq!(
            options.validate(),
            Err("timeout must be between 1 and 3600 seconds")
        );
    }

    #[test]
    fn drain_options_validates_grace_period_boundary() {
        let mut options = DrainOptions {
            grace_period: Some(DRAIN_GRACE_PERIOD_MAX_SECS),
            ..Default::default()
        };
        assert_eq!(options.validate(), Ok(()));

        options.grace_period = Some(DRAIN_GRACE_PERIOD_MAX_SECS + 1);
        assert_eq!(
            options.validate(),
            Err("grace period must not exceed 86400 seconds")
        );

        options.grace_period = None;
        assert_eq!(options.validate(), Ok(()));
    }

    #[test]
    fn drain_event_round_trips() {
        let ev = DrainEvent {
            seq: 3,
            kind: DrainEventKind::Blocked {
                blockers: vec![DrainBlocker {
                    pod: "standalone".into(),
                    reason: "unmanaged pod".into(),
                    clearable_by: "force".into(),
                }],
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"kind\":\"blocked\""));
        assert_eq!(serde_json::from_str::<DrainEvent>(&json).unwrap(), ev);
    }
}
