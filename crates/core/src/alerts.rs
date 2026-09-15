//! Alertmanager payloads and silence requests.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
/// A discovered Kubernetes resource that can be opened from an alert.
pub struct AlertResourceTarget {
    pub key: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
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
    #[serde(default)]
    pub targets: Vec<AlertResourceTarget>,
    #[serde(default)]
    pub defining_rules: Vec<AlertResourceTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SilenceAlertRequest {
    pub fingerprint: String,
    pub duration: AlertSilenceDuration,
    pub matcher_labels: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AlertSilenceDuration {
    Finite { seconds: u64 },
    Forever,
}

pub const MIN_ALERT_SILENCE_SECS: u64 = 60;
pub const MAX_ALERT_SILENCE_SECS: u64 = 365 * 24 * 60 * 60;
