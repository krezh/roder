//! HTTP API handlers, grouped by concern. Each submodule owns one route
//! group; this module only holds what's genuinely shared across all of them.

use std::sync::Arc;

use axum::http::StatusCode;
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
pub use drain::drain_progress;
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

/// A 502 response carrying the upstream error text.
fn bad_gateway(e: impl std::fmt::Display) -> Response {
    (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
}

/// Normalize an optional namespace query param, treating `""` as `None`.
fn ns_filter(ns: &Option<String>) -> Option<&str> {
    ns.as_deref().filter(|s| !s.is_empty())
}
