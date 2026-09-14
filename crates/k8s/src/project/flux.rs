//! Flux row projector (every `*.fluxcd.io` kind): Ready + reason/Suspended + message.

use roder_core::RowStatus;
use serde_json::Value;

use super::status::{
    cond_to_status, condition_message, condition_status, ready_label, ready_reason,
};

pub(crate) fn ready_message_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let ready = condition_status(data, "Ready");
    let message = condition_message(data, "Ready").unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ready_and_suspension_drive_flux_projection() {
        let cases = [
            (
                json!({"status": {"conditions": [{
                    "type": "Ready",
                    "status": "True",
                    "reason": "ReconciliationSucceeded",
                    "message": "Applied revision main@sha1:abc"
                }]}}),
                vec![
                    "True".to_string(),
                    "ReconciliationSucceeded".to_string(),
                    "Applied revision main@sha1:abc".to_string(),
                ],
                RowStatus::Ok,
            ),
            (
                json!({"status": {"conditions": [{
                    "type": "Ready", "status": "False"
                }]}}),
                vec!["False".to_string(), "False".to_string(), String::new()],
                RowStatus::Error,
            ),
            (
                json!({"status": {"conditions": [{
                    "type": "Ready", "status": "Unknown", "reason": "Progressing"
                }]}}),
                vec![
                    "Unknown".to_string(),
                    "Progressing".to_string(),
                    String::new(),
                ],
                RowStatus::Pending,
            ),
            (
                json!({"spec": {"suspend": true}}),
                vec!["-".to_string(), "Suspended".to_string(), String::new()],
                RowStatus::Warn,
            ),
            (
                json!({}),
                vec!["-".to_string(), "-".to_string(), String::new()],
                RowStatus::Unknown,
            ),
        ];

        for (data, expected_cells, expected_status) in cases {
            assert_eq!(
                ready_message_cells(&data),
                (expected_cells, expected_status),
                "{data}"
            );
        }
    }
}
