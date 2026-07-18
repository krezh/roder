//! HTTP API handlers, grouped by concern. Each submodule owns one route
//! group; this module only holds what's genuinely shared across all of them.

use std::sync::Arc;

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use roder_k8s::Backend;

use super::AppState;

mod action;
mod drain;
mod exec;
mod logs;
mod misc;
mod resource_reads;
mod talos;
mod watch;

pub use action::action;
pub use drain::{active_drain, drain_progress};
pub use exec::{debug_shell, exec_ws, node_shell_create, terminal_page};
pub use logs::{logs, metrics_history};
pub use misc::{alerts, features, health, namespaces, overview, resources};
pub use resource_reads::{access_review, detail, permissions, resource_tree};
pub use talos::{talos_config_diff, talos_dmesg, talos_node};
pub use watch::{watch, watch_multi};

/// Resolve the connected backend or a 503 if login/connection hasn't happened.
pub(crate) async fn backend(state: &AppState) -> Result<Arc<Backend>, Response> {
    state.backend.read().await.clone().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "not connected to a cluster yet",
        )
            .into_response()
    })
}

/// Bind the connected backend or early-return its 503 response.
macro_rules! backend_or_return {
    ($state:expr) => {
        match backend(&$state).await {
            Ok(b) => b,
            Err(r) => return r,
        }
    };
}
pub(crate) use backend_or_return;

/// Stable ownership and human-readable audit identities for a request.
pub(crate) struct RequestCaller {
    pub owner: String,
    pub audit: String,
}

pub(crate) fn request_caller(state: &AppState, headers: &HeaderMap) -> Option<RequestCaller> {
    if state.config.dev_mode {
        return Some(RequestCaller {
            owner: "dev".into(),
            audit: "dev".into(),
        });
    }
    let key = state.config.session_key?;
    let cookie =
        crate::server::session::cookie_value(headers, crate::server::session::SESSION_COOKIE)?;
    let identity = crate::server::session::open_session(&cookie, &key)?.identity;
    if identity.subject.is_empty() {
        return None;
    }
    Some(RequestCaller {
        owner: identity.subject.clone(),
        audit: identity.email.unwrap_or(identity.subject),
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
    use crate::server::handlers::fixtures::{
        dev_state, fake_tokens, prod_state_without_provider, sealed_cookie_header,
    };

    #[test]
    fn dev_caller_uses_dev_for_owner_and_audit() {
        let caller = request_caller(&dev_state(), &HeaderMap::new()).unwrap();
        assert_eq!(caller.owner, "dev");
        assert_eq!(caller.audit, "dev");
    }

    #[test]
    fn production_caller_uses_subject_for_owner_and_email_for_audit() {
        let state = prod_state_without_provider();
        let mut tokens = fake_tokens();
        tokens.identity.subject = "stable-subject".into();
        tokens.identity.email = Some("person@example.com".into());

        let caller = request_caller(&state, &sealed_cookie_header(&tokens)).unwrap();
        assert_eq!(caller.owner, "stable-subject");
        assert_eq!(caller.audit, "person@example.com");
    }

    #[test]
    fn production_caller_falls_back_to_subject_for_audit() {
        let state = prod_state_without_provider();
        let mut tokens = fake_tokens();
        tokens.identity.subject = "stable-subject".into();
        tokens.identity.email = None;

        let caller = request_caller(&state, &sealed_cookie_header(&tokens)).unwrap();
        assert_eq!(caller.owner, "stable-subject");
        assert_eq!(caller.audit, "stable-subject");
        assert!(request_caller(&state, &HeaderMap::new()).is_none());
    }
}
