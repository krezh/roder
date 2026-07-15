//! Small standalone read endpoints: connectivity check, the browsable
//! resource catalog, the dashboard overview, namespaces, alerts, and which
//! optional integrations this deployment has configured.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::talos::talos_capabilities;
use super::{backend, backend_or_return, bad_gateway};
use crate::server::AppState;

/// Authenticated end-to-end connectivity check against the Kubernetes API.
pub async fn health(State(state): State<AppState>) -> Response {
    let b = backend_or_return!(state);
    match b.probe().await {
        Ok(version) => Json(serde_json::json!({ "version": version })).into_response(),
        Err(e) => bad_gateway(e),
    }
}

/// Current firing alerts from Alertmanager, with 30-second in-process cache.
pub async fn alerts(State(state): State<AppState>) -> Response {
    let cache = state.alerts.read().await.as_ref().map(Arc::clone);
    let Some(cache) = cache else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let b = backend_or_return!(state);
    match cache.get(&b.client()).await {
        Ok(alerts) => Json(alerts).into_response(),
        Err(e) => {
            tracing::warn!("alerts: {e}");
            bad_gateway(e)
        }
    }
}

/// The full browsable resource catalog (every GVR, RBAC-permitting).
pub async fn resources(State(state): State<AppState>) -> Response {
    let b = backend_or_return!(state);
    Json(b.kinds()).into_response()
}

/// Dashboard overview (nodes, pod counts, Flux/ESO health, warnings).
pub async fn overview(State(state): State<AppState>) -> Response {
    let b = backend_or_return!(state);
    match b.overview().await {
        Ok(o) => Json(o).into_response(),
        Err(e) => bad_gateway(e),
    }
}

pub async fn namespaces(State(state): State<AppState>) -> Response {
    let b = backend_or_return!(state);
    match b.namespaces().await {
        Ok(ns) => Json(ns).into_response(),
        Err(e) => bad_gateway(e),
    }
}

/// Optional server integrations available to the current deployment.
pub async fn features(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let alertmanager = state.alerts.read().await.is_some();
    let talos = talos_capabilities(&state, &headers);
    Json(serde_json::json!({
        "talos": talos,
        "alertmanager": alertmanager,
    }))
    .into_response()
}
