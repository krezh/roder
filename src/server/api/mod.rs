//! HTTP API handlers, grouped by concern. Each submodule owns one route
//! group; this module only holds what's genuinely shared across all of them.
//!
//! Per-user backend access: `require_auth` resolves the caller's own
//! `Arc<Backend>` and inserts it into the request extensions, so handlers
//! take it via an `Extension<Arc<Backend>>` extractor rather than reading a
//! single shared backend off `AppState`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use roder_auth::Identity;

pub(crate) mod action;
mod drain;
mod exec;
mod files;
mod logs;
mod misc;
mod resource_reads;
mod talos;
mod watch;

pub use action::action;
pub use drain::{active_drain, drain_progress};
pub use exec::{
    debug_shell, exec_ws, node_shell_create, terminal_page, terminal_xterm_addon_fit_js,
    terminal_xterm_css, terminal_xterm_js,
};
pub use files::{
    create_file, delete_file, download_file, list_files, read_file, upload_file, write_file,
};
pub use logs::{logs, metrics_history};
pub use misc::{alerts, features, health, namespaces, overview, resources, silence_alert};
pub use resource_reads::{access_review, detail, permissions, resource_tree};
pub use talos::{talos_config_diff, talos_dmesg, talos_node};
pub use watch::{watch, watch_multi};

/// Stable ownership and human-readable audit identities for a request.
pub(crate) struct RequestCaller {
    pub owner: String,
    pub audit: String,
}

pub(crate) fn request_caller(identity: &Identity) -> Option<RequestCaller> {
    if identity.subject.is_empty() {
        return None;
    }
    Some(RequestCaller {
        owner: identity.subject.clone(),
        audit: identity
            .email
            .clone()
            .unwrap_or_else(|| identity.subject.clone()),
    })
}

/// A 502 response carrying the upstream error text.
fn bad_gateway(e: impl std::fmt::Display) -> Response {
    (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
}

/// Normalize an optional namespace query param, treating `""` as `None`.
fn ns_filter(ns: &Option<String>) -> Option<&str> {
    ns.as_deref().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::handlers::fixtures::fake_tokens;

    #[test]
    fn dev_caller_uses_dev_for_owner_and_audit() {
        let identity = Identity {
            subject: "dev".into(),
            email: None,
            name: Some("dev mode".into()),
            groups: Vec::new(),
        };
        let caller = request_caller(&identity).unwrap();
        assert_eq!(caller.owner, "dev");
        assert_eq!(caller.audit, "dev");
    }

    #[test]
    fn production_caller_uses_subject_for_owner_and_email_for_audit() {
        let mut tokens = fake_tokens();
        tokens.identity.subject = "stable-subject".into();
        tokens.identity.email = Some("person@example.com".into());

        let caller = request_caller(&tokens.identity).unwrap();
        assert_eq!(caller.owner, "stable-subject");
        assert_eq!(caller.audit, "person@example.com");
    }

    #[test]
    fn production_caller_falls_back_to_subject_for_audit() {
        let mut tokens = fake_tokens();
        tokens.identity.subject = "stable-subject".into();
        tokens.identity.email = None;

        let caller = request_caller(&tokens.identity).unwrap();
        assert_eq!(caller.owner, "stable-subject");
        assert_eq!(caller.audit, "stable-subject");
        tokens.identity.subject.clear();
        assert!(request_caller(&tokens.identity).is_none());
    }
}
