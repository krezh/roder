//! Dashboard rollups: cluster health, controller signals, node summaries.

use serde::{Deserialize, Serialize};

use crate::row::RowStatus;

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
    pub warnings: Vec<OverviewWarning>,
    #[serde(default)]
    pub controller_groups: Vec<ControllerHealthGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControllerHealthGroup {
    pub name: String,
    pub resources: Vec<ResourceHealthRollup>,
    #[serde(default)]
    pub signals: Vec<ControllerHealthSignal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControllerHealthSignal {
    pub label: String,
    pub value: String,
    pub status: RowStatus,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OverviewWarning {
    pub event_name: String,
    pub namespace: Option<String>,
    pub involved_kind: String,
    pub involved_name: String,
    pub reason: String,
    pub message: String,
    pub source: String,
    pub timestamp: Option<String>,
    pub count: u32,
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

/// Counts of resources by projected health state for one resource type.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthRollup {
    pub total: u32,
    pub ready: u32,
    #[serde(default)]
    pub reconciling: u32,
    pub suspended: u32,
    #[serde(default)]
    pub warning: u32,
    pub failing: u32,
    #[serde(default)]
    pub unknown: u32,
    #[serde(default)]
    pub unreported: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceHealthRollup {
    /// Stable `group/version/kind` catalog key used to open the exact resource type.
    #[serde(default)]
    pub key: String,
    pub kind: String,
    pub health: HealthRollup,
    /// List/RBAC failure for this resource type. Counts remain zero when unreadable.
    #[serde(default)]
    pub error: Option<String>,
}
