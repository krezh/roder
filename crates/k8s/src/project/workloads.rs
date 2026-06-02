//! Workload row projection: Deployment/StatefulSet/DaemonSet/ReplicaSet/Job/CronJob.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::{int_at, str_at};
use super::status::{ready_replicas, replicas_status};

pub(crate) fn replicaset_cells(data: &Value) -> (Vec<String>, RowStatus) {
    (vec![ready_replicas(data)], replicas_status(data))
}

pub(crate) fn workload_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let desired = int_at(data, &["status", "replicas"]).unwrap_or(0);
    let ready = int_at(data, &["status", "readyReplicas"]).unwrap_or(0);
    let available = int_at(data, &["status", "availableReplicas"])
        .or_else(|| int_at(data, &["status", "numberAvailable"]))
        .unwrap_or(0);
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

pub(crate) fn daemonset_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let desired = int_at(data, &["status", "desiredNumberScheduled"]).unwrap_or(0);
    let ready = int_at(data, &["status", "numberReady"]).unwrap_or(0);
    let available = int_at(data, &["status", "numberAvailable"]).unwrap_or(0);
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
    let desired = int_at(data, &["spec", "completions"]).unwrap_or(1);
    let failed = int_at(data, &["status", "failed"]).unwrap_or(0);
    let status = if succeeded >= desired {
        RowStatus::Ok
    } else if failed > 0 {
        RowStatus::Error
    } else {
        RowStatus::Pending
    };
    (
        vec![
            format!("{succeeded}/{desired}"),
            if succeeded >= desired {
                "Complete".into()
            } else {
                "Running".into()
            },
        ],
        status,
    )
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
