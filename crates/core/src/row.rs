//! A projected table row and the watch events that keep it live.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
    /// Whether an operator-managed resource has reconciliation or scheduling suspended.
    #[serde(default)]
    pub suspended: bool,
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
    /// A non-recoverable Table/API error for this resource stream.
    Error { message: String },
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
