//! ExternalSecrets row projectors: ExternalSecret and the generic ESO kinds.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::str_at;
use super::format::humanize_since;
use super::status::{cond_to_status, condition_status, ready_label, ready_reason};

pub(crate) fn external_secret_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let store_type = str_at(data, &["spec", "secretStoreRef", "kind"]).unwrap_or_default();
    let store = str_at(data, &["spec", "secretStoreRef", "name"]).unwrap_or_default();
    let refresh = str_at(data, &["spec", "refreshInterval"]).unwrap_or_default();
    let ready = condition_status(data, "Ready");
    let last_sync = str_at(data, &["status", "refreshTime"])
        .map(|t| humanize_since(&t))
        .unwrap_or_default();
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
    let store = str_at(data, &["spec", "secretStoreRef", "name"]).unwrap_or_default();
    (
        vec![ready_label(&ready), ready_reason(data), store],
        cond_to_status(ready.as_deref()),
    )
}
