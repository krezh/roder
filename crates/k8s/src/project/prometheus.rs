//! Prometheus Operator readiness projections.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::{int_at, str_at};
use super::status::{condition, status_generation_is_stale};

pub(crate) fn prometheus_cells(data: &Value) -> (Vec<String>, RowStatus) {
    readiness_cells(data, true)
}

pub(crate) fn prometheus_agent_cells(data: &Value) -> (Vec<String>, RowStatus) {
    readiness_cells(data, true)
}

pub(crate) fn alertmanager_cells(data: &Value) -> (Vec<String>, RowStatus) {
    readiness_cells(data, false)
}

pub(crate) fn thanos_ruler_cells(data: &Value) -> (Vec<String>, RowStatus) {
    readiness_cells(data, false)
}

fn readiness_cells(data: &Value, sharded: bool) -> (Vec<String>, RowStatus) {
    let version = str_at(data, &["spec", "version"]).unwrap_or_default();
    let desired = desired_replicas(data, sharded);
    let ready = int_at(data, &["status", "availableReplicas"]).unwrap_or(0);
    let replicas = int_at(data, &["status", "replicas"]);
    let updated = int_at(data, &["status", "updatedReplicas"]);
    let reconciled = condition_value(data, "Reconciled");
    let available = condition_value(data, "Available");
    let paused = data
        .pointer("/status/paused")
        .and_then(Value::as_bool)
        .or_else(|| data.pointer("/spec/paused").and_then(Value::as_bool))
        .unwrap_or(false);
    let status = readiness_status(data, desired, ready, replicas, updated, paused);

    (
        vec![
            version,
            desired.to_string(),
            format!("{ready}/{desired}"),
            reconciled,
            available,
            paused.to_string(),
        ],
        status,
    )
}

fn desired_replicas(data: &Value, sharded: bool) -> i64 {
    if sharded && str_at(data, &["spec", "mode"]).is_some_and(|mode| mode == "DaemonSet") {
        return int_at(data, &["status", "replicas"]).unwrap_or(0).max(0);
    }
    let replicas = int_at(data, &["spec", "replicas"]).unwrap_or(1).max(0);
    let shards = if sharded {
        int_at(data, &["spec", "shards"]).unwrap_or(1).max(0)
    } else {
        1
    };
    replicas.saturating_mul(shards)
}

fn condition_value(data: &Value, type_: &str) -> String {
    condition(data, type_)
        .and_then(|condition| condition.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string()
}

fn has_stale_condition(data: &Value, type_: &str) -> bool {
    condition(data, type_).is_none()
        && data
            .pointer("/status/conditions")
            .and_then(Value::as_array)
            .is_some_and(|conditions| {
                conditions
                    .iter()
                    .any(|condition| condition.get("type").and_then(Value::as_str) == Some(type_))
            })
}

fn readiness_status(
    data: &Value,
    desired: i64,
    ready: i64,
    replicas: Option<i64>,
    updated: Option<i64>,
    paused: bool,
) -> RowStatus {
    if paused {
        return RowStatus::Warn;
    }
    if status_generation_is_stale(data)
        || has_stale_condition(data, "Reconciled")
        || has_stale_condition(data, "Available")
    {
        return RowStatus::Pending;
    }

    match condition_value(data, "Reconciled").as_str() {
        "False" => return RowStatus::Error,
        "Unknown" => return RowStatus::Pending,
        _ => {}
    }
    match condition_value(data, "Available").as_str() {
        "False" => return RowStatus::Error,
        "Degraded" => return RowStatus::Warn,
        "Unknown" => return RowStatus::Pending,
        _ => {}
    }

    let status_reported = data
        .get("status")
        .and_then(Value::as_object)
        .is_some_and(|status| !status.is_empty());
    if !status_reported {
        RowStatus::Unknown
    } else if ready == desired
        && replicas.is_none_or(|replicas| replicas == desired)
        && updated.is_none_or(|updated| updated == desired)
    {
        RowStatus::Ok
    } else {
        RowStatus::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sharded_prometheus_projects_total_readiness() {
        let data = json!({
            "metadata": {"generation": 2},
            "spec": {"version": "v3.5.0", "replicas": 2, "shards": 3},
            "status": {
                "availableReplicas": 6,
                "conditions": [
                    {"type": "Reconciled", "status": "True", "observedGeneration": 2},
                    {"type": "Available", "status": "True", "observedGeneration": 2}
                ]
            }
        });

        let (cells, status) = prometheus_cells(&data);
        assert_eq!(cells, ["v3.5.0", "6", "6/6", "True", "True", "false"]);
        assert_eq!(status, RowStatus::Ok);
    }

    #[test]
    fn readiness_failures_and_degradation_are_semantic() {
        let failed = json!({"status": {"conditions": [
            {"type": "Reconciled", "status": "False"},
            {"type": "Available", "status": "True"}
        ]}});
        let degraded = json!({"status": {"availableReplicas": 1, "conditions": [
            {"type": "Reconciled", "status": "True"},
            {"type": "Available", "status": "Degraded"}
        ]}});

        assert_eq!(alertmanager_cells(&failed).1, RowStatus::Error);
        assert_eq!(thanos_ruler_cells(&degraded).1, RowStatus::Warn);
    }

    #[test]
    fn paused_and_stale_resources_do_not_report_ready() {
        let paused = json!({
            "spec": {"paused": true},
            "status": {"availableReplicas": 1, "conditions": [{"type": "Available", "status": "True"}]}
        });
        let stale = json!({
            "metadata": {"generation": 3},
            "status": {"availableReplicas": 1, "conditions": [
                {"type": "Reconciled", "status": "True", "observedGeneration": 2},
                {"type": "Available", "status": "True", "observedGeneration": 2}
            ]}
        });

        assert_eq!(prometheus_agent_cells(&paused).1, RowStatus::Warn);
        assert_eq!(prometheus_cells(&stale).1, RowStatus::Pending);
    }

    #[test]
    fn daemonset_agent_uses_controller_reported_replica_count() {
        let data = json!({
            "spec": {"mode": "DaemonSet", "replicas": 99},
            "status": {"replicas": 4, "availableReplicas": 3}
        });

        let (cells, status) = prometheus_agent_cells(&data);
        assert_eq!(&cells[1..3], ["4", "3/4"]);
        assert_eq!(status, RowStatus::Pending);
    }
}
