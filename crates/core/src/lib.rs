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
            Category::Custom(_) => 9,
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
    /// Full current contents on (re)connect.
    Snapshot { rows: Vec<ResourceRow> },
    /// A row was added or changed.
    Applied { row: ResourceRow },
    /// A row was removed (by uid).
    Deleted { uid: String },
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

/// A single pod metrics data point, shared between the server (serializes) and
/// the frontend (deserializes) via the `/api/metrics` endpoint.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

/// Counts of resources by reconciled/suspended/failing for a CRD family.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthRollup {
    pub total: u32,
    pub ready: u32,
    pub suspended: u32,
    pub failing: u32,
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
}
