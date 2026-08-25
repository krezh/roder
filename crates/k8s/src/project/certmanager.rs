//! cert-manager row projectors: Certificate / Issuer / CertificateRequest / ACME.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::str_at;
use super::status::{cond_to_status, condition_status, ready_label, ready_reason};

pub(crate) fn certificate_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let ready = condition_status(data, "Ready");
    let secret = str_at(data, &["spec", "secretName"]).unwrap_or_default();
    (
        vec![ready_label(&ready), ready_reason(data), secret],
        cond_to_status(ready.as_deref()),
    )
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
