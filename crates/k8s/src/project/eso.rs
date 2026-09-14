//! ExternalSecrets row projectors: ExternalSecret and the generic ESO kinds.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::str_at;
use super::status::{cond_to_status, condition_status, ready_label, ready_reason};

pub(crate) fn external_secret_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let store_type = str_at(data, &["spec", "secretStoreRef", "kind"]).unwrap_or_default();
    let store = str_at(data, &["spec", "secretStoreRef", "name"]).unwrap_or_default();
    let refresh = str_at(data, &["spec", "refreshInterval"]).unwrap_or_default();
    let ready = condition_status(data, "Ready");
    // Raw RFC3339 flows to the client, which live-humanizes on its tick —
    // same path as the built-in Age column — so "Last Sync" stays relative.
    let last_sync = str_at(data, &["status", "refreshTime"]).unwrap_or_default();
    (
        vec![
            store_type,
            store,
            refresh,
            ready_reason(data),
            ready_label(&ready),
            last_sync,
        ],
        cond_to_status(ready.as_deref()),
    )
}

pub(crate) fn eso_generic_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let ready = condition_status(data, "Ready");
    (
        vec![ready_label(&ready), ready_reason(data)],
        cond_to_status(ready.as_deref()),
    )
}

pub(crate) fn cluster_external_secret_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let (mut cells, status) = eso_generic_cells(data);
    cells.push(
        str_at(
            data,
            &["spec", "externalSecretSpec", "secretStoreRef", "name"],
        )
        .unwrap_or_default(),
    );
    (cells, status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn external_secret_projection_includes_store_and_sync_state() {
        let data = json!({
            "spec": {
                "secretStoreRef": {"kind": "ClusterSecretStore", "name": "vault"},
                "refreshInterval": "1h"
            },
            "status": {
                "refreshTime": "2026-09-14T10:00:00Z",
                "conditions": [{
                    "type": "Ready", "status": "True", "reason": "SecretSynced"
                }]
            }
        });

        assert_eq!(
            external_secret_cells(&data),
            (
                vec![
                    "ClusterSecretStore".to_string(),
                    "vault".to_string(),
                    "1h".to_string(),
                    "SecretSynced".to_string(),
                    "True".to_string(),
                    "2026-09-14T10:00:00Z".to_string()
                ],
                RowStatus::Ok
            )
        );
    }

    #[test]
    fn eso_conditions_and_nested_store_drive_generic_projections() {
        for (condition, expected) in [
            ("True", RowStatus::Ok),
            ("False", RowStatus::Error),
            ("Unknown", RowStatus::Pending),
        ] {
            let data = json!({"status": {"conditions": [{
                "type": "Ready", "status": condition
            }]}});
            assert_eq!(eso_generic_cells(&data).1, expected, "{condition}");
        }

        let data = json!({
            "spec": {"externalSecretSpec": {
                "secretStoreRef": {"name": "production-vault"}
            }},
            "status": {"conditions": [{
                "type": "Ready", "status": "True", "reason": "Ready"
            }]}
        });
        assert_eq!(
            cluster_external_secret_cells(&data),
            (
                vec![
                    "True".to_string(),
                    "Ready".to_string(),
                    "production-vault".to_string()
                ],
                RowStatus::Ok
            )
        );
    }
}
