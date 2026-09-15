//! Kopiur backup row projectors.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::{int_at, str_at};
use super::status::{
    condition_is, condition_message, condition_reason, condition_status, generic_status,
    status_generation_is_stale,
};

struct Health {
    status: RowStatus,
    state: String,
    message: String,
}

pub(crate) fn snapshot_policy_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let repository = str_at(data, &["spec", "repository", "name"]).unwrap_or_default();
    let repositories =
        str_at(data, &["status", "repositorySummary"]).unwrap_or_else(|| repository_names(data));
    let last_snapshot = str_at(data, &["status", "lastSuccessfulSnapshot"]).unwrap_or_default();
    let last_verified = str_at(data, &["status", "lastVerified"]).unwrap_or_default();
    let suspended = bool_at(data, "/spec/suspend");
    let health = policy_health(data, suspended, last_verified.is_empty());

    (
        vec![
            repository,
            repositories,
            last_snapshot,
            last_verified,
            suspended.to_string(),
            health.state,
            health.message,
        ],
        health.status,
    )
}

pub(crate) fn snapshot_schedule_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let policy =
        str_at(data, &["spec", "policyRef", "name"]).unwrap_or_else(|| "selector".to_string());
    let schedule = str_at(data, &["spec", "schedule", "cron"]).unwrap_or_default();
    let suspended = bool_at(data, "/spec/schedule/suspend");
    let last = str_at(data, &["status", "lastSchedule", "at"]).unwrap_or_default();
    let last_success =
        str_at(data, &["status", "lastSuccessfulSchedule", "at"]).unwrap_or_default();
    let next = str_at(data, &["status", "nextSchedule", "at"]).unwrap_or_default();
    let health = schedule_health(data, suspended);

    (
        vec![
            policy,
            schedule,
            suspended.to_string(),
            last,
            last_success,
            next,
            health.state,
            health.message,
        ],
        health.status,
    )
}

pub(crate) fn snapshot_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let phase = str_at(data, &["status", "phase"]).unwrap_or_default();
    let origin = str_at(data, &["status", "origin"]).unwrap_or_default();
    let snapshot = str_at(data, &["status", "snapshot", "kopiaSnapshotID"]).unwrap_or_default();
    let source = str_at(data, &["spec", "source", "target", "pvc", "name"]).unwrap_or_default();
    let completed = str_at(data, &["status", "timing", "endTime"]).unwrap_or_default();
    let health = snapshot_health(data, &phase);

    (
        vec![phase, origin, snapshot, source, completed, health.message],
        health.status,
    )
}

pub(crate) fn verification_enabled(data: &Value) -> bool {
    [
        "/spec/verification/quick/schedule",
        "/spec/verification/deep",
    ]
    .iter()
    .any(|path| data.pointer(path).is_some_and(|value| !value.is_null()))
}

pub(crate) fn recorded_verification_failed(data: &Value) -> bool {
    raw_condition(data, "Verified")
        .and_then(|condition| condition.get("status"))
        .and_then(Value::as_str)
        == Some("False")
}

fn policy_health(data: &Value, suspended: bool, never_verified: bool) -> Health {
    if status_generation_is_stale(data) {
        return health(RowStatus::Pending, "Reconciling", "Status is stale");
    }
    if condition_is(data, "Stalled", "True") {
        return condition_health(data, "Stalled", RowStatus::Error);
    }
    if condition_is(data, "RepositoriesReady", "False") {
        return condition_health(data, "RepositoriesReady", RowStatus::Warn);
    }
    if condition_is(data, "ScratchStorageClassIgnored", "True") {
        return condition_health(data, "ScratchStorageClassIgnored", RowStatus::Warn);
    }
    if verification_enabled(data) && recorded_verification_failed(data) {
        return raw_condition_health(data, "Verified", RowStatus::Warn);
    }
    if suspended {
        return health(RowStatus::Warn, "Suspended", "Backup policy is suspended");
    }
    if condition_is(data, "Reconciling", "True") {
        return condition_health(data, "Reconciling", RowStatus::Pending);
    }
    if verification_enabled(data)
        && never_verified
        && condition_status(data, "Ready").as_deref() == Some("True")
    {
        return health(
            RowStatus::Pending,
            "AwaitingVerification",
            "No successful verification recorded",
        );
    }
    ready_health(data)
}

fn schedule_health(data: &Value, suspended: bool) -> Health {
    if status_generation_is_stale(data) {
        return health(RowStatus::Pending, "Reconciling", "Status is stale");
    }
    if condition_is(data, "Stalled", "True") {
        return condition_health(data, "Stalled", RowStatus::Error);
    }
    if condition_is(data, "ScheduleRunnable", "False") {
        return condition_health(data, "ScheduleRunnable", RowStatus::Error);
    }
    if condition_is(data, "FanoutCapped", "True") {
        return condition_health(data, "FanoutCapped", RowStatus::Error);
    }
    if suspended {
        return health(RowStatus::Warn, "Suspended", "Backup schedule is suspended");
    }
    if condition_is(data, "TimezoneDefaultAmbiguous", "True") {
        return condition_health(data, "TimezoneDefaultAmbiguous", RowStatus::Warn);
    }
    if condition_is(data, "ReplacementHeld", "True") {
        return condition_health(data, "ReplacementHeld", RowStatus::Pending);
    }
    if condition_is(data, "Reconciling", "True") {
        return condition_health(data, "Reconciling", RowStatus::Pending);
    }
    ready_health(data)
}

fn snapshot_health(data: &Value, phase: &str) -> Health {
    if status_generation_is_stale(data) {
        return health(RowStatus::Pending, phase, "Status is stale");
    }
    for type_ in ["Stalled", "Failed", "Failure", "Error"] {
        if condition_is(data, type_, "True") {
            return condition_health(data, type_, RowStatus::Error);
        }
    }
    if phase.eq_ignore_ascii_case("failed")
        || data
            .pointer("/status/failure")
            .is_some_and(|failure| !failure.is_null())
    {
        let message = str_at(data, &["status", "failure", "message"]).unwrap_or_default();
        return health(RowStatus::Error, phase, &message);
    }
    if int_at(data, &["status", "stats", "filesFailed"]).unwrap_or(0) > 0 {
        return health(
            RowStatus::Warn,
            phase,
            "Snapshot completed with unreadable files",
        );
    }
    if condition_is(data, "SecurityContextCompatible", "False") {
        return condition_health(data, "SecurityContextCompatible", RowStatus::Warn);
    }
    if condition_is(data, "Degraded", "True") {
        return condition_health(data, "Degraded", RowStatus::Warn);
    }

    let status = match phase.to_ascii_lowercase().as_str() {
        "succeeded" | "discovered" | "unchanged" => RowStatus::Done,
        "pending" | "running" | "deleting" => RowStatus::Pending,
        "failed" => RowStatus::Error,
        "" => generic_status(data),
        _ => RowStatus::Pending,
    };
    let message = condition_message(data, "Reconciling").unwrap_or_default();
    health(status, phase, &message)
}

fn ready_health(data: &Value) -> Health {
    let status = match condition_status(data, "Ready").as_deref() {
        Some("True") => RowStatus::Ok,
        Some("False") => RowStatus::Error,
        Some(_) => RowStatus::Pending,
        None => generic_status(data),
    };
    let state = condition_reason(data, "Ready").unwrap_or_else(|| {
        condition_status(data, "Ready").unwrap_or_else(|| {
            str_at(data, &["status", "phase"])
                .or_else(|| str_at(data, &["status", "state"]))
                .unwrap_or_default()
        })
    });
    let message = condition_message(data, "Ready").unwrap_or_default();
    Health {
        status,
        state,
        message,
    }
}

fn condition_health(data: &Value, type_: &str, status: RowStatus) -> Health {
    Health {
        status,
        state: condition_reason(data, type_).unwrap_or_else(|| type_.to_string()),
        message: condition_message(data, type_).unwrap_or_default(),
    }
}

fn raw_condition_health(data: &Value, type_: &str, status: RowStatus) -> Health {
    let condition = raw_condition(data, type_);
    Health {
        status,
        state: condition
            .and_then(|condition| condition.get("reason"))
            .and_then(Value::as_str)
            .unwrap_or(type_)
            .to_string(),
        message: condition
            .and_then(|condition| condition.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

fn raw_condition<'a>(data: &'a Value, type_: &str) -> Option<&'a Value> {
    // Kopiur verification movers deliberately write observedGeneration 0.
    data.pointer("/status/conditions")?
        .as_array()?
        .iter()
        .rev()
        .find(|condition| condition.get("type").and_then(Value::as_str) == Some(type_))
}

fn health(status: RowStatus, state: &str, message: &str) -> Health {
    Health {
        status,
        state: state.to_string(),
        message: message.to_string(),
    }
}

fn bool_at(data: &Value, path: &str) -> bool {
    data.pointer(path).and_then(Value::as_bool).unwrap_or(false)
}

fn repository_names(data: &Value) -> String {
    data.pointer("/spec/repositories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|repository| repository.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn policy_surfaces_backup_and_verification_age() {
        let data = json!({
            "spec": {
                "repositories": [{"name": "local"}, {"name": "offsite"}],
                "verification": {"quick": {"schedule": {"cron": "H 3 * * *"}}}
            },
            "status": {
                "repositorySummary": "local, offsite",
                "lastSuccessfulSnapshot": "2026-09-14T02:00:00Z",
                "lastVerified": "2026-09-14T03:00:00Z",
                "conditions": [{"type": "Ready", "status": "True", "reason": "Ready"}]
            }
        });

        let (cells, status) = snapshot_policy_cells(&data);

        assert_eq!(cells[1], "local, offsite");
        assert_eq!(cells[2], "2026-09-14T02:00:00Z");
        assert_eq!(cells[3], "2026-09-14T03:00:00Z");
        assert_eq!(status, RowStatus::Ok);
    }

    #[test]
    fn failed_verification_is_a_warning() {
        let data = json!({
            "spec": {
                "repository": {"name": "local"},
                "verification": {"deep": {"schedule": {"cron": "H 3 * * 0"}}}
            },
            "status": {"conditions": [
                {"type": "Ready", "status": "True", "observedGeneration": 2},
                {
                    "type": "Verified",
                    "status": "False",
                    "reason": "VerificationFailed",
                    "message": "restore check failed",
                    "observedGeneration": 0
                }
            ]},
            "metadata": {"generation": 2}
        });

        let (cells, status) = snapshot_policy_cells(&data);

        assert_eq!(cells[5], "VerificationFailed");
        assert_eq!(cells[6], "restore check failed");
        assert_eq!(status, RowStatus::Warn);
    }

    #[test]
    fn enabled_verification_without_success_is_pending() {
        let data = json!({
            "spec": {
                "repository": {"name": "local"},
                "verification": {"quick": {"schedule": {"cron": "H 3 * * *"}}}
            },
            "status": {"conditions": [{"type": "Ready", "status": "True"}]}
        });

        assert_eq!(snapshot_policy_cells(&data).1, RowStatus::Pending);
    }

    #[test]
    fn suspended_schedule_is_a_warning() {
        let data = json!({
            "spec": {
                "policyRef": {"name": "database"},
                "schedule": {"cron": "H 2 * * *", "suspend": true}
            },
            "status": {"conditions": [{"type": "Ready", "status": "True"}]}
        });

        let (cells, status) = snapshot_schedule_cells(&data);

        assert_eq!(cells[2], "true");
        assert_eq!(cells[6], "Suspended");
        assert_eq!(status, RowStatus::Warn);
    }

    #[test]
    fn unrunnable_schedule_is_an_error_even_when_ready() {
        let data = json!({
            "spec": {"schedule": {"cron": "bad"}},
            "status": {"conditions": [
                {"type": "Ready", "status": "True"},
                {
                    "type": "ScheduleRunnable",
                    "status": "False",
                    "reason": "InvalidSchedule",
                    "message": "cron is invalid"
                }
            ]}
        });

        let (cells, status) = snapshot_schedule_cells(&data);

        assert_eq!(cells[6], "InvalidSchedule");
        assert_eq!(status, RowStatus::Error);
    }

    #[test]
    fn snapshot_lifecycle_and_partial_failures_are_classified() {
        let cases = [
            (json!({"status": {"phase": "Running"}}), RowStatus::Pending),
            (json!({"status": {"phase": "Succeeded"}}), RowStatus::Done),
            (json!({"status": {"phase": "Unchanged"}}), RowStatus::Done),
            (json!({"status": {"phase": "Discovered"}}), RowStatus::Done),
            (
                json!({"status": {"phase": "Failed", "failure": {"message": "upload failed"}}}),
                RowStatus::Error,
            ),
            (
                json!({"status": {"phase": "Succeeded", "stats": {"filesFailed": 2}}}),
                RowStatus::Warn,
            ),
            (
                json!({"status": {"phase": "Quiescing"}}),
                RowStatus::Pending,
            ),
        ];

        for (data, expected) in cases {
            assert_eq!(snapshot_cells(&data).1, expected, "{data}");
        }
    }
}
