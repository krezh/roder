//! Rook Ceph and object-bucket row projectors.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::str_at;

pub(crate) fn ceph_cluster_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let phase = str_at(data, &["status", "phase"])
        .or_else(|| str_at(data, &["status", "state"]))
        .unwrap_or_default();
    let health = str_at(data, &["status", "ceph", "health"]).unwrap_or_default();
    let version = str_at(data, &["status", "version", "version"])
        .or_else(|| str_at(data, &["status", "version", "image"]))
        .unwrap_or_default();
    let message = str_at(data, &["status", "message"]).unwrap_or_default();
    let status = match health.as_str() {
        "HEALTH_OK" => RowStatus::Ok,
        "HEALTH_WARN" => RowStatus::Warn,
        "HEALTH_ERR" => RowStatus::Error,
        _ => phase_status(&phase),
    };
    (vec![phase, health, version, message], status)
}

pub(crate) fn ceph_resource_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let phase = str_at(data, &["status", "phase"])
        .or_else(|| str_at(data, &["status", "state"]))
        .unwrap_or_default();
    let message = str_at(data, &["status", "message"]).unwrap_or_default();
    let status = phase_status(&phase);
    (vec![phase, message], status)
}

pub(crate) fn object_bucket_claim_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let phase = str_at(data, &["status", "phase"]).unwrap_or_default();
    let storage_class = str_at(data, &["spec", "storageClassName"]).unwrap_or_default();
    let bucket = str_at(data, &["spec", "bucketName"])
        .or_else(|| str_at(data, &["status", "bucketName"]))
        .unwrap_or_default();
    let status = phase_status(&phase);
    (vec![phase, storage_class, bucket], status)
}

fn phase_status(phase: &str) -> RowStatus {
    match phase.to_ascii_lowercase().as_str() {
        "ready" | "created" | "connected" | "bound" => RowStatus::Ok,
        "creating" | "connecting" | "progressing" | "reconciling" => RowStatus::Pending,
        "degraded" => RowStatus::Warn,
        "error" | "failed" | "failure" => RowStatus::Error,
        _ => RowStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ceph_cluster_health_controls_row_severity() {
        let data = json!({
            "status": {
                "phase": "Ready",
                "message": "Cluster created successfully",
                "ceph": {"health": "HEALTH_WARN"},
                "version": {"version": "19.2.3"}
            }
        });
        let (cells, status) = ceph_cluster_cells(&data);

        assert_eq!(
            cells,
            [
                "Ready",
                "HEALTH_WARN",
                "19.2.3",
                "Cluster created successfully"
            ]
        );
        assert_eq!(status, RowStatus::Warn);
    }

    #[test]
    fn failed_ceph_resource_is_an_error() {
        let data = json!({"status": {"phase": "Failed", "message": "pool create failed"}});
        let (cells, status) = ceph_resource_cells(&data);

        assert_eq!(cells, ["Failed", "pool create failed"]);
        assert_eq!(status, RowStatus::Error);
    }

    #[test]
    fn object_bucket_claim_surfaces_storage_binding() {
        let data = json!({
            "spec": {"storageClassName": "rook-ceph-bucket", "bucketName": "reports"},
            "status": {"phase": "Bound"}
        });
        let (cells, status) = object_bucket_claim_cells(&data);

        assert_eq!(cells, ["Bound", "rook-ceph-bucket", "reports"]);
        assert_eq!(status, RowStatus::Ok);
    }
}
