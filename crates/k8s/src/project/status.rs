//! Condition / status helpers: turn k8s `status.conditions` and phases into the
//! `RowStatus` colour plus the label/reason strings the projectors display.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::{int_at, str_at};

pub(crate) fn generic_status(data: &Value) -> RowStatus {
    if status_generation_is_stale(data) {
        return RowStatus::Pending;
    }

    for type_ in ["Stalled", "Failed", "Failure", "Error"] {
        if condition_is(data, type_, "True") {
            return RowStatus::Error;
        }
    }
    for type_ in ["Complete", "Completed", "Succeeded"] {
        if condition_is(data, type_, "True") {
            return RowStatus::Done;
        }
    }
    if condition_is(data, "Degraded", "True") {
        return RowStatus::Warn;
    }
    for type_ in ["Paused", "Suspended"] {
        if condition_is(data, type_, "True") {
            return RowStatus::Warn;
        }
    }
    for type_ in ["Progressing", "Reconciling"] {
        if condition_is(data, type_, "True") {
            return RowStatus::Pending;
        }
    }
    for type_ in ["Ready", "Healthy", "Available"] {
        if let Some(status) = condition_status(data, type_) {
            return cond_to_status(Some(&status));
        }
    }

    let state = str_at(data, &["status", "phase"])
        .or_else(|| str_at(data, &["status", "state"]))
        .unwrap_or_default()
        .to_ascii_lowercase();
    match state.as_str() {
        "running" | "active" | "bound" | "ready" | "healthy" | "available" | "established"
        | "applied" | "synced" | "valid" => RowStatus::Ok,
        "pending" | "progressing" | "reconciling" | "creating" | "initializing" | "updating"
        | "upgrading" | "restoring" | "provisioning" | "terminating" => RowStatus::Pending,
        "degraded" | "warning" | "paused" | "suspended" => RowStatus::Warn,
        "succeeded" | "complete" | "completed" | "finished" => RowStatus::Done,
        "failed" | "failure" | "error" | "errored" | "lost" | "invalid" => RowStatus::Error,
        _ => RowStatus::Unknown,
    }
}

pub(crate) fn condition_status(data: &Value, type_: &str) -> Option<String> {
    condition(data, type_)?
        .get("status")?
        .as_str()
        .map(str::to_string)
}

pub(crate) fn condition_is(data: &Value, type_: &str, status: &str) -> bool {
    condition(data, type_)
        .and_then(|condition| condition.get("status"))
        .and_then(Value::as_str)
        == Some(status)
}

fn condition<'a>(data: &'a Value, type_: &str) -> Option<&'a Value> {
    if status_generation_is_stale(data) {
        return None;
    }
    let generation = int_at(data, &["metadata", "generation"]);
    data.pointer("/status/conditions")?
        .as_array()?
        .iter()
        .rev()
        .find(|condition| {
            condition.get("type").and_then(Value::as_str) == Some(type_)
                && generation
                    .zip(condition.get("observedGeneration").and_then(Value::as_i64))
                    .is_none_or(|(generation, observed)| generation == observed)
        })
}

pub(crate) fn condition_reason(data: &Value, type_: &str) -> Option<String> {
    condition(data, type_)?
        .get("reason")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

pub(crate) fn condition_message(data: &Value, type_: &str) -> Option<String> {
    condition(data, type_)?
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
}

pub(crate) fn status_generation_is_stale(data: &Value) -> bool {
    int_at(data, &["metadata", "generation"])
        .zip(int_at(data, &["status", "observedGeneration"]))
        .is_some_and(|(generation, observed)| observed < generation)
}

/// The Ready condition's reason (e.g. "ReconciliationSucceeded"), falling back to
/// its True/False status when there's no reason.
pub(crate) fn ready_reason(data: &Value) -> String {
    condition_reason(data, "Ready").unwrap_or_else(|| ready_label(&condition_status(data, "Ready")))
}

/// Map a Kubernetes condition status string to row health.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn condition_helpers_ignore_stale_condition_generations() {
        let data = json!({
            "metadata": {"generation": 4},
            "status": {"conditions": [
                {"type": "Ready", "status": "True", "reason": "Old", "observedGeneration": 3},
                {"type": "Ready", "status": "False", "reason": "Current", "observedGeneration": 4}
            ]}
        });

        assert_eq!(condition_status(&data, "Ready").as_deref(), Some("False"));
        assert_eq!(condition_reason(&data, "Ready").as_deref(), Some("Current"));
    }

    #[test]
    fn generic_status_precedence_and_phases_are_table_driven() {
        let cases = [
            (json!({"status": {"phase": "Completed"}}), RowStatus::Done),
            (json!({"status": {"phase": "Degraded"}}), RowStatus::Warn),
            (
                json!({"status": {"phase": "Restoring"}}),
                RowStatus::Pending,
            ),
            (json!({"status": {"phase": "Invalid"}}), RowStatus::Error),
            (json!({"status": {"phase": "Healthy"}}), RowStatus::Ok),
            (json!({"status": {"phase": "Mystery"}}), RowStatus::Unknown),
            (
                json!({"status": {"conditions": [
                    {"type": "Ready", "status": "False"},
                    {"type": "Reconciling", "status": "True"}
                ]}}),
                RowStatus::Pending,
            ),
        ];

        for (data, expected) in cases {
            assert_eq!(generic_status(&data), expected, "{data}");
        }
    }

    #[test]
    fn stale_top_level_observed_generation_is_pending() {
        let data = json!({
            "metadata": {"generation": 2},
            "status": {"observedGeneration": 1, "phase": "Healthy"}
        });

        assert_eq!(generic_status(&data), RowStatus::Pending);
    }
}
