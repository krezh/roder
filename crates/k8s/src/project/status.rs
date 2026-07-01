//! Condition / status helpers: turn k8s `status.conditions` and phases into the
//! `RowStatus` colour plus the label/reason strings the projectors display.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::{int_at, str_at};

pub(crate) fn generic_status(data: &Value) -> RowStatus {
    if let Some(s) = condition_status(data, "Ready") {
        return cond_to_status(Some(&s));
    }
    if let Some(s) = condition_status(data, "Available") {
        return cond_to_status(Some(&s));
    }
    match str_at(data, &["status", "phase"]).as_deref() {
        Some("Running" | "Active" | "Bound" | "Succeeded" | "Ready") => RowStatus::Ok,
        Some("Pending") => RowStatus::Pending,
        Some("Failed" | "Lost") => RowStatus::Error,
        _ => RowStatus::Unknown,
    }
}

pub(crate) fn condition_status(data: &Value, type_: &str) -> Option<String> {
    data.get("status")?
        .get("conditions")?
        .as_array()?
        .iter()
        .find(|c| c["type"] == type_)
        .and_then(|c| c["status"].as_str().map(|s| s.to_string()))
}

pub(crate) fn condition_reason(data: &Value, type_: &str) -> Option<String> {
    data.get("status")?
        .get("conditions")?
        .as_array()?
        .iter()
        .find(|c| c["type"] == type_)
        .and_then(|c| c["reason"].as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// The Ready condition's reason (e.g. "ReconciliationSucceeded"), falling back to
/// its True/False status when there's no reason.
pub(crate) fn ready_reason(data: &Value) -> String {
    condition_reason(data, "Ready").unwrap_or_else(|| ready_label(&condition_status(data, "Ready")))
}

/// A status reason to show for an otherwise-uncolumned resource: the Ready/Available
/// condition's reason, else `status.reason`, else `status.phase`.
pub(crate) fn cond_to_status(s: Option<&str>) -> RowStatus {
    match s {
        Some("True") => RowStatus::Ok,
        Some("False") => RowStatus::Error,
        Some(_) => RowStatus::Pending,
        None => RowStatus::Unknown,
    }
}

pub(crate) fn ready_label(s: &Option<String>) -> String {
    s.as_deref().unwrap_or("-").to_string()
}

pub(crate) fn ready_replicas(data: &Value) -> String {
    let desired = int_at(data, &["status", "replicas"]).unwrap_or(0);
    let ready = int_at(data, &["status", "readyReplicas"]).unwrap_or(0);
    format!("{ready}/{desired}")
}

pub(crate) fn replicas_status(data: &Value) -> RowStatus {
    let desired = int_at(data, &["status", "replicas"]).unwrap_or(0);
    let ready = int_at(data, &["status", "readyReplicas"]).unwrap_or(0);
    if desired == 0 {
        RowStatus::Warn
    } else if ready >= desired {
        RowStatus::Ok
    } else {
        RowStatus::Pending
    }
}
