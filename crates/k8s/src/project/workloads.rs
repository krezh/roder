//! Workload row projection: Deployment/StatefulSet/DaemonSet/ReplicaSet/Job/CronJob.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::{int_at, str_at};
use super::status::{ready_replicas, replicas_status};

pub(crate) fn replicaset_cells(data: &Value) -> (Vec<String>, RowStatus) {
    (vec![ready_replicas(data)], replicas_status(data))
}

pub(crate) fn workload_cells(data: &Value) -> (Vec<String>, RowStatus) {
    replica_cells(
        int_at(data, &["status", "replicas"]).unwrap_or(0),
        int_at(data, &["status", "readyReplicas"]).unwrap_or(0),
        int_at(data, &["status", "availableReplicas"])
            .or_else(|| int_at(data, &["status", "numberAvailable"]))
            .unwrap_or(0),
    )
}

pub(crate) fn daemonset_cells(data: &Value) -> (Vec<String>, RowStatus) {
    replica_cells(
        int_at(data, &["status", "desiredNumberScheduled"]).unwrap_or(0),
        int_at(data, &["status", "numberReady"]).unwrap_or(0),
        int_at(data, &["status", "numberAvailable"]).unwrap_or(0),
    )
}

fn replica_cells(desired: i64, ready: i64, available: i64) -> (Vec<String>, RowStatus) {
    let status = if desired == 0 {
        RowStatus::Warn
    } else if ready >= desired {
        RowStatus::Ok
    } else {
        RowStatus::Pending
    };
    (
        vec![format!("{ready}/{desired}"), available.to_string()],
        status,
    )
}

pub(crate) fn job_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let succeeded = int_at(data, &["status", "succeeded"]).unwrap_or(0);
    let desired = int_at(data, &["spec", "completions"]);
    let condition_true = |kind: &str| {
        data.get("status")
            .and_then(|status| status.get("conditions"))
            .and_then(Value::as_array)
            .is_some_and(|conditions| {
                conditions.iter().any(|condition| {
                    condition.get("type").and_then(Value::as_str) == Some(kind)
                        && condition.get("status").and_then(Value::as_str) == Some("True")
                })
            })
    };
    let complete = condition_true("Complete");
    let failed = condition_true("Failed");
    let (status, phase) = if complete {
        (RowStatus::Ok, "Complete")
    } else if failed {
        (RowStatus::Error, "Failed")
    } else {
        (RowStatus::Pending, "Running")
    };
    let completions_str = desired.map_or_else(
        || succeeded.to_string(),
        |desired| format!("{succeeded}/{desired}"),
    );
    (vec![completions_str, phase.into()], status)
}

pub(crate) fn cronjob_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let schedule = str_at(data, &["spec", "schedule"]).unwrap_or_default();
    let suspended = data
        .get("spec")
        .and_then(|s| s.get("suspend"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let status = if suspended {
        RowStatus::Warn
    } else {
        RowStatus::Ok
    };
    (vec![schedule, suspended.to_string()], status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn job_fixed_completions_pending() {
        let data = json!({"spec": {"completions": 3}, "status": {"succeeded": 1}});
        let (cells, status) = job_cells(&data);
        assert_eq!(cells[0], "1/3");
        assert_eq!(cells[1], "Running");
        assert_eq!(status, RowStatus::Pending);
    }

    #[test]
    fn job_fixed_completions_done() {
        let data = json!({
            "spec": {"completions": 3},
            "status": {
                "succeeded": 3,
                "conditions": [{"type": "Complete", "status": "True"}]
            }
        });
        let (cells, status) = job_cells(&data);
        assert_eq!(cells[0], "3/3");
        assert_eq!(cells[1], "Complete");
        assert_eq!(status, RowStatus::Ok);
    }

    #[test]
    fn job_fixed_completions_failed() {
        let data = json!({
            "spec": {"completions": 3},
            "status": {
                "succeeded": 1,
                "failed": 2,
                "conditions": [{"type": "Failed", "status": "True"}]
            }
        });
        let (cells, status) = job_cells(&data);
        assert_eq!(cells[0], "1/3");
        assert_eq!(cells[1], "Failed");
        assert_eq!(status, RowStatus::Error);
    }

    #[test]
    fn job_failed_pod_count_remains_retrying() {
        let data = json!({"spec": {"completions": 3}, "status": {"failed": 2}});
        let (cells, status) = job_cells(&data);
        assert_eq!(cells[1], "Running");
        assert_eq!(status, RowStatus::Pending);
    }

    #[test]
    fn job_work_queue_pending() {
        // spec.completions absent → work-queue mode; show just succeeded count
        let data = json!({"spec": {}, "status": {"succeeded": 0}});
        let (cells, status) = job_cells(&data);
        assert_eq!(cells[0], "0");
        assert_eq!(cells[1], "Running");
        assert_eq!(status, RowStatus::Pending);
    }

    #[test]
    fn job_work_queue_done() {
        let data = json!({
            "spec": {},
            "status": {
                "succeeded": 1,
                "conditions": [{"type": "Complete", "status": "True"}]
            }
        });
        let (cells, status) = job_cells(&data);
        assert_eq!(cells[0], "1");
        assert_eq!(cells[1], "Complete");
        assert_eq!(status, RowStatus::Ok);
    }

    #[test]
    fn job_work_queue_failed() {
        let data = json!({
            "spec": {},
            "status": {
                "succeeded": 0,
                "failed": 1,
                "conditions": [{"type": "Failed", "status": "True"}]
            }
        });
        let (cells, status) = job_cells(&data);
        assert_eq!(cells[0], "0");
        assert_eq!(cells[1], "Failed");
        assert_eq!(status, RowStatus::Error);
    }

    #[test]
    fn cronjob_active() {
        let data = json!({"spec": {"schedule": "0 * * * *", "suspend": false}});
        let (cells, status) = cronjob_cells(&data);
        assert_eq!(cells[0], "0 * * * *");
        assert_eq!(cells[1], "false");
        assert_eq!(status, RowStatus::Ok);
    }

    #[test]
    fn cronjob_suspended() {
        let data = json!({"spec": {"schedule": "*/5 * * * *", "suspend": true}});
        let (cells, status) = cronjob_cells(&data);
        assert_eq!(cells[1], "true");
        assert_eq!(status, RowStatus::Warn);
    }
}
