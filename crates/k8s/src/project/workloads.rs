//! Workload row projection: Deployment/StatefulSet/DaemonSet/ReplicaSet/Job/CronJob.

use std::collections::BTreeSet;
use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use croner::Cron;
use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::{int_at, str_at};
use super::status::{condition_is, condition_reason};
use super::{job_lifecycle, JobLifecycle};

pub(crate) fn replicaset_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let desired = desired_replicas(data);
    let ready = int_at(data, &["status", "readyReplicas"]).unwrap_or(0);
    let status = if !workload_generation_is_current(data) {
        RowStatus::Pending
    } else if condition_is(data, "ReplicaFailure", "True") {
        RowStatus::Error
    } else if [
        int_at(data, &["status", "replicas"]).unwrap_or(0),
        int_at(data, &["status", "fullyLabeledReplicas"]).unwrap_or(0),
        ready,
        int_at(data, &["status", "availableReplicas"]).unwrap_or(0),
    ]
    .into_iter()
    .all(|count| count == desired)
    {
        RowStatus::Ok
    } else {
        RowStatus::Pending
    };
    (vec![format!("{ready}/{desired}")], status)
}

pub(crate) fn deployment_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let desired = desired_replicas(data);
    let replicas = int_at(data, &["status", "replicas"]).unwrap_or(0);
    let updated = int_at(data, &["status", "updatedReplicas"]).unwrap_or(0);
    let ready = int_at(data, &["status", "readyReplicas"]).unwrap_or(0);
    let available = int_at(data, &["status", "availableReplicas"]).unwrap_or(0);
    let deadline_exceeded = condition_is(data, "Progressing", "False")
        && condition_reason(data, "Progressing").as_deref() == Some("ProgressDeadlineExceeded");
    let status = if !workload_generation_is_current(data) {
        RowStatus::Pending
    } else if condition_is(data, "ReplicaFailure", "True") || deadline_exceeded {
        RowStatus::Error
    } else if data
        .pointer("/spec/paused")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        RowStatus::Warn
    } else if replicas == desired && updated == desired && available >= desired {
        RowStatus::Ok
    } else {
        RowStatus::Pending
    };
    (
        vec![format!("{ready}/{desired}"), available.to_string()],
        status,
    )
}

pub(crate) fn statefulset_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let desired = desired_replicas(data);
    let replicas = int_at(data, &["status", "replicas"]).unwrap_or(0);
    let ready = int_at(data, &["status", "readyReplicas"]).unwrap_or(0);
    let available = int_at(data, &["status", "availableReplicas"]).unwrap_or(0);
    let updated = int_at(data, &["status", "updatedReplicas"]).unwrap_or(0);
    let base_ready = replicas == desired && ready == desired && available == desired;
    let strategy =
        str_at(data, &["spec", "updateStrategy", "type"]).unwrap_or_else(|| "RollingUpdate".into());
    let status = if !workload_generation_is_current(data) {
        RowStatus::Pending
    } else if condition_is(data, "ReplicaFailure", "True") {
        RowStatus::Error
    } else if !base_ready {
        RowStatus::Pending
    } else if strategy == "OnDelete" {
        let current = str_at(data, &["status", "currentRevision"]).unwrap_or_default();
        let update = str_at(data, &["status", "updateRevision"]).unwrap_or_default();
        if !update.is_empty() && current != update {
            RowStatus::Warn
        } else {
            RowStatus::Ok
        }
    } else {
        let partition = int_at(
            data,
            &["spec", "updateStrategy", "rollingUpdate", "partition"],
        )
        .unwrap_or(0)
        .clamp(0, desired);
        let expected_updated = desired - partition;
        let revisions_converged = partition > 0 || desired == 0 || {
            let current = str_at(data, &["status", "currentRevision"]).unwrap_or_default();
            let update = str_at(data, &["status", "updateRevision"]).unwrap_or_default();
            !update.is_empty() && current == update
        };
        if updated == expected_updated && revisions_converged {
            RowStatus::Ok
        } else {
            RowStatus::Pending
        }
    };
    (
        vec![format!("{ready}/{desired}"), available.to_string()],
        status,
    )
}

pub(crate) fn daemonset_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let desired = int_at(data, &["status", "desiredNumberScheduled"]).unwrap_or(0);
    let current = int_at(data, &["status", "currentNumberScheduled"]).unwrap_or(0);
    let updated = int_at(data, &["status", "updatedNumberScheduled"]).unwrap_or(0);
    let ready = int_at(data, &["status", "numberReady"]).unwrap_or(0);
    let available = int_at(data, &["status", "numberAvailable"]).unwrap_or(0);
    let misscheduled = int_at(data, &["status", "numberMisscheduled"]).unwrap_or(0);
    let base_ready =
        current == desired && ready == desired && available == desired && misscheduled == 0;
    let strategy =
        str_at(data, &["spec", "updateStrategy", "type"]).unwrap_or_else(|| "RollingUpdate".into());
    let status = if !workload_generation_is_current(data) {
        RowStatus::Pending
    } else if condition_is(data, "ReplicaFailure", "True") {
        RowStatus::Error
    } else if !base_ready {
        RowStatus::Pending
    } else if updated == desired {
        RowStatus::Ok
    } else if strategy == "OnDelete" {
        RowStatus::Warn
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
    let (status, phase) = match job_lifecycle(data) {
        JobLifecycle::Failed | JobLifecycle::Failing => (RowStatus::Error, "Failed"),
        JobLifecycle::Complete => (RowStatus::Done, "Complete"),
        JobLifecycle::Suspended => (RowStatus::Warn, "Suspended"),
        JobLifecycle::Completing => (RowStatus::Pending, "Completing"),
        JobLifecycle::Running => (RowStatus::Pending, "Running"),
    };
    let completions_str = desired.map_or_else(
        || succeeded.to_string(),
        |desired| format!("{succeeded}/{desired}"),
    );
    (vec![completions_str, phase.into()], status)
}

pub(crate) fn cronjob_cells(data: &Value) -> (Vec<String>, RowStatus) {
    cronjob_cells_at(data, time::OffsetDateTime::now_utc())
}

fn cronjob_cells_at(data: &Value, now: time::OffsetDateTime) -> (Vec<String>, RowStatus) {
    let schedule = str_at(data, &["spec", "schedule"]).unwrap_or_default();
    let suspended = data
        .get("spec")
        .and_then(|s| s.get("suspend"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let active = data
        .pointer("/status/active")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|reference| reference.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("\n");
    let last_schedule = str_at(data, &["status", "lastScheduleTime"]).unwrap_or_default();
    let last_success = str_at(data, &["status", "lastSuccessfulTime"]).unwrap_or_default();
    let schedule_state = cronjob_schedule_state(data, &schedule, now);
    let (label, status) = if schedule_state == ScheduleState::Invalid {
        ("Invalid schedule", RowStatus::Error)
    } else if suspended {
        ("Suspended", RowStatus::Warn)
    } else if schedule_state == ScheduleState::Missed {
        ("Missed schedule", RowStatus::Warn)
    } else if !active.is_empty() {
        ("Active", RowStatus::Ok)
    } else {
        ("Scheduled", RowStatus::Ok)
    };
    (
        vec![
            schedule,
            suspended.to_string(),
            active,
            last_schedule,
            last_success,
            label.to_string(),
        ],
        status,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScheduleState {
    Current,
    Missed,
    Invalid,
}

fn cronjob_schedule_state(
    data: &Value,
    schedule: &str,
    now: time::OffsetDateTime,
) -> ScheduleState {
    let Ok(cron) = Cron::from_str(schedule) else {
        return ScheduleState::Invalid;
    };
    let Some(timezone) = str_at(data, &["spec", "timeZone"]) else {
        return ScheduleState::Current;
    };
    let Ok(timezone) = timezone.parse::<Tz>() else {
        return ScheduleState::Invalid;
    };
    let baseline = str_at(data, &["status", "lastScheduleTime"])
        .or_else(|| str_at(data, &["metadata", "creationTimestamp"]));
    let Some(baseline) = baseline
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
    else {
        return ScheduleState::Current;
    };
    let Some(now) = DateTime::<Utc>::from_timestamp(now.unix_timestamp(), now.nanosecond()) else {
        return ScheduleState::Current;
    };
    let baseline = baseline.with_timezone(&timezone);
    let now = now.with_timezone(&timezone);
    let Ok(next) = cron.find_next_occurrence(&baseline, false) else {
        return ScheduleState::Invalid;
    };
    let grace = int_at(data, &["spec", "startingDeadlineSeconds"])
        .unwrap_or(60)
        .max(0);
    let Some(deadline) =
        Duration::try_seconds(grace).and_then(|grace| next.checked_add_signed(grace))
    else {
        return ScheduleState::Current;
    };
    if now > deadline {
        ScheduleState::Missed
    } else {
        ScheduleState::Current
    }
}

fn desired_replicas(data: &Value) -> i64 {
    int_at(data, &["spec", "replicas"]).unwrap_or(1).max(0)
}

fn workload_generation_is_current(data: &Value) -> bool {
    int_at(data, &["metadata", "generation"]).is_none_or(|generation| {
        int_at(data, &["status", "observedGeneration"])
            .is_some_and(|observed| observed >= generation)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deployment_rollout_status_is_table_driven() {
        let cases = [
            (
                "converged",
                json!({
                    "metadata": {"generation": 2},
                    "spec": {"replicas": 3},
                    "status": {"observedGeneration": 2, "replicas": 3, "updatedReplicas": 3, "readyReplicas": 3, "availableReplicas": 3}
                }),
                RowStatus::Ok,
            ),
            (
                "scaled to zero",
                json!({
                    "metadata": {"generation": 2},
                    "spec": {"replicas": 0},
                    "status": {"observedGeneration": 2}
                }),
                RowStatus::Ok,
            ),
            (
                "stale generation",
                json!({
                    "metadata": {"generation": 3},
                    "spec": {"replicas": 3},
                    "status": {"observedGeneration": 2, "replicas": 3, "updatedReplicas": 3, "readyReplicas": 3, "availableReplicas": 3, "conditions": [{"type": "ReplicaFailure", "status": "True"}]}
                }),
                RowStatus::Pending,
            ),
            (
                "old replicas remain",
                json!({
                    "metadata": {"generation": 2},
                    "spec": {"replicas": 3},
                    "status": {"observedGeneration": 2, "replicas": 4, "updatedReplicas": 3, "readyReplicas": 3, "availableReplicas": 3, "conditions": [{"type": "Available", "status": "True"}]}
                }),
                RowStatus::Pending,
            ),
            (
                "deadline exceeded",
                json!({
                    "metadata": {"generation": 2},
                    "spec": {"replicas": 3},
                    "status": {"observedGeneration": 2, "conditions": [{"type": "Progressing", "status": "False", "reason": "ProgressDeadlineExceeded"}]}
                }),
                RowStatus::Error,
            ),
            (
                "paused",
                json!({
                    "metadata": {"generation": 2},
                    "spec": {"replicas": 3, "paused": true},
                    "status": {"observedGeneration": 2, "replicas": 3, "updatedReplicas": 3, "readyReplicas": 3, "availableReplicas": 3}
                }),
                RowStatus::Warn,
            ),
        ];

        for (name, data, expected) in cases {
            assert_eq!(deployment_cells(&data).1, expected, "{name}");
        }
    }

    #[test]
    fn statefulset_rollout_strategy_controls_convergence() {
        let rolling = json!({
            "metadata": {"generation": 2},
            "spec": {"replicas": 3},
            "status": {"observedGeneration": 2, "replicas": 3, "readyReplicas": 3, "availableReplicas": 3, "updatedReplicas": 3, "currentRevision": "new", "updateRevision": "new"}
        });
        let partitioned = json!({
            "metadata": {"generation": 2},
            "spec": {"replicas": 3, "updateStrategy": {"type": "RollingUpdate", "rollingUpdate": {"partition": 1}}},
            "status": {"observedGeneration": 2, "replicas": 3, "readyReplicas": 3, "availableReplicas": 3, "updatedReplicas": 2, "currentRevision": "old", "updateRevision": "new"}
        });
        let on_delete = json!({
            "metadata": {"generation": 2},
            "spec": {"replicas": 3, "updateStrategy": {"type": "OnDelete"}},
            "status": {"observedGeneration": 2, "replicas": 3, "readyReplicas": 3, "availableReplicas": 3, "updatedReplicas": 2, "currentRevision": "old", "updateRevision": "new"}
        });
        let zero = json!({
            "metadata": {"generation": 2},
            "spec": {"replicas": 0},
            "status": {"observedGeneration": 2}
        });

        assert_eq!(statefulset_cells(&rolling).1, RowStatus::Ok);
        assert_eq!(statefulset_cells(&partitioned).1, RowStatus::Ok);
        assert_eq!(statefulset_cells(&on_delete).1, RowStatus::Warn);
        assert_eq!(statefulset_cells(&zero).1, RowStatus::Ok);
    }

    #[test]
    fn daemonset_and_replicaset_handle_zero_and_failure_states() {
        let daemon_zero = json!({
            "metadata": {"generation": 2},
            "status": {"observedGeneration": 2}
        });
        let daemon_misscheduled = json!({
            "metadata": {"generation": 2},
            "status": {"observedGeneration": 2, "desiredNumberScheduled": 2, "currentNumberScheduled": 2, "updatedNumberScheduled": 2, "numberReady": 2, "numberAvailable": 2, "numberMisscheduled": 1}
        });
        let replica_zero = json!({
            "metadata": {"generation": 2},
            "spec": {"replicas": 0},
            "status": {"observedGeneration": 2}
        });
        let replica_failure = json!({
            "metadata": {"generation": 2},
            "status": {"observedGeneration": 2, "conditions": [{"type": "ReplicaFailure", "status": "True"}]}
        });

        assert_eq!(daemonset_cells(&daemon_zero).1, RowStatus::Ok);
        assert_eq!(daemonset_cells(&daemon_misscheduled).1, RowStatus::Pending);
        assert_eq!(replicaset_cells(&replica_zero).1, RowStatus::Ok);
        assert_eq!(replicaset_cells(&replica_failure).1, RowStatus::Error);
    }

    #[test]
    fn job_conditions_cover_new_terminal_and_suspended_states() {
        let cases = [
            (
                json!({"status": {"conditions": [{"type": "FailureTarget", "status": "True"}]}}),
                (RowStatus::Error, "Failed"),
            ),
            (
                json!({"status": {"conditions": [{"type": "SuccessCriteriaMet", "status": "True"}]}}),
                (RowStatus::Pending, "Completing"),
            ),
            (
                json!({"spec": {"suspend": true}}),
                (RowStatus::Warn, "Suspended"),
            ),
        ];

        for (data, (status, phase)) in cases {
            let (cells, actual) = job_cells(&data);
            assert_eq!(actual, status);
            assert_eq!(cells[1], phase);
        }
    }

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
        assert_eq!(status, RowStatus::Done);
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
        assert_eq!(status, RowStatus::Done);
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
        let data = json!({
            "spec": {"schedule": "0 * * * *", "suspend": false},
            "status": {
                "active": [{"name": "hourly-b"}, {"name": "hourly-a"}, {"name": "hourly-a"}],
                "lastScheduleTime": "2026-09-14T12:00:00Z",
                "lastSuccessfulTime": "2026-09-14T11:00:12Z"
            }
        });
        let now = super::super::parse_timestamp("2026-09-14T12:00:30Z").unwrap();
        let (cells, status) = cronjob_cells_at(&data, now);
        assert_eq!(cells[0], "0 * * * *");
        assert_eq!(cells[1], "false");
        assert_eq!(cells[2], "hourly-a\nhourly-b");
        assert_eq!(cells[3], "2026-09-14T12:00:00Z");
        assert_eq!(cells[4], "2026-09-14T11:00:12Z");
        assert_eq!(cells[5], "Active");
        assert_eq!(status, RowStatus::Ok);
    }

    #[test]
    fn cronjob_suspended() {
        let data = json!({"spec": {"schedule": "*/5 * * * *", "suspend": true}});
        let (cells, status) = cronjob_cells(&data);
        assert_eq!(cells[1], "true");
        assert_eq!(cells[5], "Suspended");
        assert_eq!(status, RowStatus::Warn);
    }

    #[test]
    fn cronjob_reports_missed_and_invalid_schedules() {
        let now = super::super::parse_timestamp("2026-09-14T12:02:00Z").unwrap();
        let missed = json!({
            "metadata": {"creationTimestamp": "2026-09-14T11:00:00Z"},
            "spec": {"schedule": "0 * * * *", "timeZone": "UTC"},
            "status": {"lastScheduleTime": "2026-09-14T11:00:00Z"}
        });
        let invalid = json!({
            "metadata": {"creationTimestamp": "2026-09-14T11:00:00Z"},
            "spec": {"schedule": "not a schedule"}
        });

        let (missed_cells, missed_status) = cronjob_cells_at(&missed, now);
        assert_eq!(missed_cells[5], "Missed schedule");
        assert_eq!(missed_status, RowStatus::Warn);
        let (invalid_cells, invalid_status) = cronjob_cells_at(&invalid, now);
        assert_eq!(invalid_cells[5], "Invalid schedule");
        assert_eq!(invalid_status, RowStatus::Error);
    }

    #[test]
    fn cronjob_honors_starting_deadline_and_timezone() {
        let data = json!({
            "metadata": {"creationTimestamp": "2026-09-14T10:00:00Z"},
            "spec": {
                "schedule": "0 8 * * *",
                "timeZone": "America/New_York",
                "startingDeadlineSeconds": 300
            }
        });
        let inside_deadline = super::super::parse_timestamp("2026-09-14T12:04:00Z").unwrap();
        let past_deadline = super::super::parse_timestamp("2026-09-14T12:06:00Z").unwrap();

        assert_eq!(cronjob_cells_at(&data, inside_deadline).1, RowStatus::Ok);
        assert_eq!(cronjob_cells_at(&data, past_deadline).1, RowStatus::Warn);
    }
}
