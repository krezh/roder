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
