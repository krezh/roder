//! Small standalone read endpoints: connectivity check, the browsable
//! resource catalog, the dashboard overview, namespaces, alerts, and which
//! optional integrations this deployment has configured.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use roder_k8s::Backend;

use super::bad_gateway;
use super::talos::{request_groups, talos_capabilities};
use crate::server::AppState;

/// Authenticated end-to-end connectivity check against the Kubernetes API.
pub async fn health(Extension(b): Extension<Arc<Backend>>) -> Response {
    match b.probe().await {
        Ok(version) => Json(serde_json::json!({ "version": version })).into_response(),
        Err(e) => bad_gateway(e),
    }
}

/// Current firing alerts from Alertmanager, with a 30-second in-process cache
/// unless an explicit refresh is requested.
///
/// Fetched via the ServiceAccount client (not the caller's token): Alertmanager
/// returns cluster-wide alerts unfiltered by k8s identity, and the cache is
/// shared across every user on a 30s TTL, so per-user token passthrough was
/// incoherent. Gated by `RODER_ALERTS_GROUPS` instead — see
/// `ServerConfig::can_read_alerts`.
#[derive(Default, serde::Deserialize)]
pub struct AlertsQuery {
    #[serde(default)]
    refresh: bool,
}

pub async fn alerts(
    State(state): State<AppState>,
    Query(query): Query<AlertsQuery>,
    headers: HeaderMap,
) -> Response {
    let groups = request_groups(&state, &headers).unwrap_or_default();
    if !state.config.can_read_alerts(&groups) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let cache = state.alerts.read().await.as_ref().map(Arc::clone);
    let Some(cache) = cache else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let result = if query.refresh {
        cache.refresh(&state.shared.sa_client()).await
    } else {
        cache.get(&state.shared.sa_client()).await
    };
    match result {
        Ok(alerts) => Json(alerts).into_response(),
        Err(e) => {
            tracing::warn!("alerts: {e}");
            bad_gateway(e)
        }
    }
}

/// The full browsable resource catalog (every GVR, RBAC-permitting).
pub async fn resources(Extension(b): Extension<Arc<Backend>>) -> Response {
    Json(b.kinds()).into_response()
}

/// Dashboard overview (nodes, pod counts, Flux/ESO health, warnings).
pub async fn overview(Extension(b): Extension<Arc<Backend>>) -> Response {
    match b.overview().await {
        Ok(o) => Json(o).into_response(),
        Err(e) => bad_gateway(e),
    }
}

pub async fn namespaces(Extension(b): Extension<Arc<Backend>>) -> Response {
    match b.namespaces().await {
        Ok(ns) => Json(ns).into_response(),
        Err(e) => bad_gateway(e),
    }
}

/// Optional server integrations available to the current deployment.
pub async fn features(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let alertmanager = state.alerts.read().await.is_some()
        && state
            .config
            .can_read_alerts(&request_groups(&state, &headers).unwrap_or_default());
    let talos = talos_capabilities(&state, &headers);
    Json(serde_json::json!({
        "talos": talos,
        "alertmanager": alertmanager,
    }))
    .into_response()
}
