//! CloudNativePG row projectors.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::{int_at, str_at};

pub(crate) fn cluster_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let instances = int_at(data, &["status", "instances"])
        .or_else(|| int_at(data, &["spec", "instances"]))
        .unwrap_or(1);
    let ready = int_at(data, &["status", "readyInstances"]).unwrap_or(0);
    let primary = str_at(data, &["status", "currentPrimary"]).unwrap_or_default();
    let phase = str_at(data, &["status", "phase"]).unwrap_or_default();
    let image = str_at(data, &["status", "image"])
        .or_else(|| str_at(data, &["spec", "imageName"]))
        .unwrap_or_default();
    let status = cluster_phase_status(&phase, ready, instances);
    (
        vec![
            instances.to_string(),
            format!("{ready}/{instances}"),
            primary,
            phase,
            image,
        ],
        status,
    )
}

pub(crate) fn backup_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let cluster = str_at(data, &["spec", "cluster", "name"]).unwrap_or_default();
    let method = str_at(data, &["status", "method"])
        .or_else(|| str_at(data, &["spec", "method"]))
        .unwrap_or_else(|| "barmanObjectStore".to_string());
    let phase = str_at(data, &["status", "phase"]).unwrap_or_default();
    let started = str_at(data, &["status", "startedAt"]).unwrap_or_default();
    let completed = str_at(data, &["status", "stoppedAt"]).unwrap_or_default();
    let status = backup_phase_status(&phase);
    (vec![cluster, method, phase, started, completed], status)
}

pub(crate) fn scheduled_backup_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let cluster = str_at(data, &["spec", "cluster", "name"]).unwrap_or_default();
    let schedule = str_at(data, &["spec", "schedule"]).unwrap_or_default();
    let suspended = data
        .pointer("/spec/suspend")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let last_schedule = str_at(data, &["status", "lastScheduleTime"]).unwrap_or_default();
    let next_schedule = str_at(data, &["status", "nextScheduleTime"]).unwrap_or_default();
    let error = str_at(data, &["status", "error"]).unwrap_or_default();
    let status = if !error.is_empty() {
        RowStatus::Error
    } else if suspended {
        RowStatus::Warn
    } else if last_schedule.is_empty() && next_schedule.is_empty() {
        RowStatus::Pending
    } else {
        RowStatus::Ok
    };
    (
        vec![
            cluster,
            schedule,
            suspended.to_string(),
            last_schedule,
            next_schedule,
            error,
        ],
        status,
    )
}

pub(crate) fn pooler_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let cluster = str_at(data, &["spec", "cluster", "name"]).unwrap_or_default();
    let type_ = str_at(data, &["spec", "type"]).unwrap_or_default();
    let instances = int_at(data, &["status", "instances"])
        .or_else(|| int_at(data, &["spec", "instances"]))
        .unwrap_or(1);
    let phase = str_at(data, &["status", "phase"]).unwrap_or_default();
    let reason = str_at(data, &["status", "phaseReason"])
        .or_else(|| str_at(data, &["status", "error"]))
        .unwrap_or_default();
    let status = match phase.to_ascii_lowercase().as_str() {
        "active" => RowStatus::Ok,
        "paused" => RowStatus::Warn,
        "inactive" => RowStatus::Pending,
        "failed" => RowStatus::Error,
        _ => RowStatus::Unknown,
    };
    (
        vec![cluster, type_, instances.to_string(), phase, reason],
        status,
    )
}

fn cluster_phase_status(phase: &str, ready: i64, instances: i64) -> RowStatus {
    let phase = phase.to_ascii_lowercase();
    if phase.contains("unrecoverable")
        || phase.contains("invalid")
        || phase.contains("unable to create")
        || phase.contains("plugin rollout failed")
        || phase.contains("unknown state")
    {
        RowStatus::Error
    } else if phase.contains("degraded")
        || phase.contains("upgrade delayed")
        || phase.contains("waiting for user action")
    {
        RowStatus::Warn
    } else if phase.contains("healthy") {
        replica_status(ready, instances)
    } else if phase.is_empty() {
        RowStatus::Unknown
    } else {
        RowStatus::Pending
    }
}

fn backup_phase_status(phase: &str) -> RowStatus {
    match phase.to_ascii_lowercase().as_str() {
        "completed" | "succeeded" => RowStatus::Ok,
        "pending" | "running" | "started" | "finalizing" => RowStatus::Pending,
        "failed" | "error" | "walarchivingfailing" | "invalid backup definition" => {
            RowStatus::Error
        }
        _ => RowStatus::Unknown,
    }
}

fn replica_status(ready: i64, instances: i64) -> RowStatus {
    if instances > 0 && ready >= instances {
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
    fn healthy_cluster_surfaces_instance_readiness() {
        let data = json!({
            "spec": {"instances": 3, "imageName": "ghcr.io/cloudnative-pg/postgresql:18"},
            "status": {
                "readyInstances": 3,
                "currentPrimary": "app-1",
                "phase": "Cluster in healthy state"
            }
        });
        let (cells, status) = cluster_cells(&data);

        assert_eq!(
            cells,
            [
                "3",
                "3/3",
                "app-1",
                "Cluster in healthy state",
                "ghcr.io/cloudnative-pg/postgresql:18"
            ]
        );
        assert_eq!(status, RowStatus::Ok);
    }

    #[test]
    fn healthy_phase_with_missing_replica_is_pending() {
        let data = json!({
            "spec": {"instances": 3},
            "status": {"readyInstances": 2, "phase": "Cluster in healthy state"}
        });

        assert_eq!(cluster_cells(&data).1, RowStatus::Pending);
    }

    #[test]
    fn failed_backup_is_an_error() {
        let data = json!({
            "spec": {"cluster": {"name": "app"}},
            "status": {"phase": "failed", "startedAt": "2026-09-04T01:00:00Z"}
        });
        let (cells, status) = backup_cells(&data);

        assert_eq!(cells[0], "app");
        assert_eq!(cells[2], "failed");
        assert_eq!(status, RowStatus::Error);
    }

    #[test]
    fn suspended_schedule_is_a_warning() {
        let data = json!({
            "spec": {"cluster": {"name": "app"}, "schedule": "0 0 2 * * *", "suspend": true},
            "status": {"lastScheduleTime": "2026-09-04T02:00:00Z"}
        });
        let (cells, status) = scheduled_backup_cells(&data);

        assert_eq!(
            cells,
            ["app", "0 0 2 * * *", "true", "2026-09-04T02:00:00Z", "", ""]
        );
        assert_eq!(status, RowStatus::Warn);
    }

    #[test]
    fn failing_over_is_transitional_not_failed() {
        let data = json!({
            "spec": {"instances": 3},
            "status": {"instances": 3, "readyInstances": 1, "phase": "Failing over"}
        });

        assert_eq!(cluster_cells(&data).1, RowStatus::Pending);
    }

    #[test]
    fn active_pooler_uses_reported_instances() {
        let data = json!({
            "spec": {"cluster": {"name": "app"}, "type": "rw", "instances": 3},
            "status": {"instances": 2, "phase": "active"}
        });
        let (cells, status) = pooler_cells(&data);

        assert_eq!(cells, ["app", "rw", "2", "active", ""]);
        assert_eq!(status, RowStatus::Ok);
    }
}
