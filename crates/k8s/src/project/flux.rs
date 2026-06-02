//! Flux row projector (every `*.fluxcd.io` kind): Ready + reason/Suspended + message.

use roder_core::RowStatus;
use serde_json::Value;

use super::status::{cond_to_status, condition_status, ready_label, ready_reason};

pub(crate) fn ready_message_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let ready = condition_status(data, "Ready");
    let message = data
        .get("status")
        .and_then(|s| s.get("conditions"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.iter().find(|c| c["type"] == "Ready"))
        .and_then(|c| c["message"].as_str())
        .unwrap_or_default()
        .to_string();
    let suspended = data
        .get("spec")
        .and_then(|s| s.get("suspend"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let status = if suspended {
        RowStatus::Warn
    } else {
        cond_to_status(ready.as_deref())
    };
    let reason = if suspended {
        "Suspended".to_string()
    } else {
        ready_reason(data)
    };
    (vec![ready_label(&ready), reason, message], status)
}
