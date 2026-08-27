//! cert-manager row projectors: Certificate / Issuer / CertificateRequest / ACME.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::{int_at, str_at};
use super::status::{
    cond_to_status, condition_reason, condition_status, ready_label, ready_reason,
};

pub(crate) fn certificate_cells(data: &Value) -> (Vec<String>, RowStatus) {
    certificate_cells_at(data, time::OffsetDateTime::now_utc())
}

fn certificate_cells_at(data: &Value, now: time::OffsetDateTime) -> (Vec<String>, RowStatus) {
    let ready = condition_status(data, "Ready");
    let issuing = condition_status(data, "Issuing").as_deref() == Some("True");
    let not_after = str_at(data, &["status", "notAfter"]);
    let renewal_time = str_at(data, &["status", "renewalTime"]);
    let expired = not_after
        .as_deref()
        .and_then(parse_timestamp)
        .is_some_and(|expiry| expiry <= now);
    let status = if issuing {
        RowStatus::Pending
    } else if expired {
        RowStatus::Error
    } else {
        cond_to_status(ready.as_deref())
    };
    let state = if issuing {
        condition_reason(data, "Issuing").unwrap_or_else(|| "Renewing".to_string())
    } else if expired {
        "Expired".to_string()
    } else {
        ready_reason(data)
    };
    let revision = int_at(data, &["status", "revision"])
        .map(|value| value.to_string())
        .unwrap_or_default();
    let secret = str_at(data, &["spec", "secretName"]).unwrap_or_default();
    (
        vec![
            ready_label(&ready),
            state,
            compact_date(not_after.as_deref()),
            compact_date(renewal_time.as_deref()),
            revision,
            secret,
        ],
        status,
    )
}

fn parse_timestamp(value: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}

fn compact_date(value: Option<&str>) -> String {
    value
        .and_then(|value| value.split_once('T').map(|(date, _)| date))
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn issuer_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let ready = condition_status(data, "Ready");
    (
        vec![ready_label(&ready), ready_reason(data)],
        cond_to_status(ready.as_deref()),
    )
}

pub(crate) fn certrequest_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let approved = condition_status(data, "Approved");
    let denied = condition_status(data, "Denied");
    let ready = condition_status(data, "Ready");
    let issuer = str_at(data, &["spec", "issuerRef", "name"]).unwrap_or_default();
    let status = if denied.as_deref() == Some("True") {
        RowStatus::Error
    } else {
        cond_to_status(ready.as_deref())
    };
    (
        vec![ready_label(&approved), ready_label(&ready), issuer],
        status,
    )
}

/// ACME Order/Challenge `status.state` (valid/pending/invalid/…) + its reason.
pub(crate) fn acme_state_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let state = str_at(data, &["status", "state"]).unwrap_or_default();
    let reason = str_at(data, &["status", "reason"]).unwrap_or_default();
    let st = match state.as_str() {
        "valid" => RowStatus::Ok,
        "ready" | "pending" | "processing" => RowStatus::Pending,
        "invalid" | "expired" | "errored" => RowStatus::Error,
        _ => RowStatus::Unknown,
    };
    (vec![state, reason], st)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> time::OffsetDateTime {
        parse_timestamp("2026-08-27T00:00:00Z").unwrap()
    }

    #[test]
    fn certificate_surfaces_validity_and_renewal_schedule() {
        let data = json!({
            "spec": {"secretName": "api-tls"},
            "status": {
                "notAfter": "2026-10-01T12:00:00Z",
                "renewalTime": "2026-09-01T12:00:00Z",
                "revision": 4,
                "conditions": [{"type": "Ready", "status": "True", "reason": "Ready"}]
            }
        });
        let (cells, status) = certificate_cells_at(&data, now());
        assert_eq!(
            cells,
            ["True", "Ready", "2026-10-01", "2026-09-01", "4", "api-tls"]
        );
        assert_eq!(status, RowStatus::Ok);
    }

    #[test]
    fn issuing_certificate_is_pending_even_while_ready() {
        let data = json!({
            "status": {"conditions": [
                {"type": "Ready", "status": "True"},
                {"type": "Issuing", "status": "True", "reason": "ManuallyTriggered"}
            ]}
        });
        let (cells, status) = certificate_cells_at(&data, now());
        assert_eq!(cells[1], "ManuallyTriggered");
        assert_eq!(status, RowStatus::Pending);
    }

    #[test]
    fn expired_certificate_is_an_error() {
        let data = json!({
            "status": {
                "notAfter": "2026-08-26T23:59:59Z",
                "conditions": [{"type": "Ready", "status": "True"}]
            }
        });
        let (cells, status) = certificate_cells_at(&data, now());
        assert_eq!(cells[1], "Expired");
        assert_eq!(status, RowStatus::Error);
    }
}
