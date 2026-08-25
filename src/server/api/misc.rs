//! Small standalone read endpoints: connectivity check, the browsable
//! resource catalog, the dashboard overview, namespaces, alerts, and which
//! optional integrations this deployment has configured.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use roder_auth::Identity;
use roder_k8s::Backend;

use super::talos::talos_capabilities;
use super::{bad_gateway, request_caller};
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

fn silence_duration(seconds: u64) -> Option<std::time::Duration> {
    (roder_core::MIN_ALERT_SILENCE_SECS..=roder_core::MAX_ALERT_SILENCE_SECS)
        .contains(&seconds)
        .then(|| std::time::Duration::from_secs(seconds))
}

pub async fn alerts(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<AlertsQuery>,
) -> Response {
    if !state.config.can_read_alerts(&identity.groups) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let cache = state.alerts.read().await.as_ref().map(Arc::clone);
    let Some(cache) = cache else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let result = if query.refresh {
        cache.refresh().await
    } else {
        cache.get().await
    };
    match result {
        Ok(alerts) => Json(alerts).into_response(),
        Err(e) => {
            tracing::warn!("alerts: {e}");
            bad_gateway(e)
        }
    }
}

pub async fn silence_alert(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<roder_core::SilenceAlertRequest>,
) -> Response {
    if !state.config.can_read_alerts(&identity.groups)
        || !state.config.can_silence_alerts(&identity.groups)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(caller) = request_caller(&identity) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(duration) = silence_duration(request.duration_secs) else {
        return (StatusCode::BAD_REQUEST, "unsupported silence duration").into_response();
    };
    let cache = state.alerts.read().await.as_ref().map(Arc::clone);
    let Some(cache) = cache else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    tracing::info!(
        actor = %caller.audit,
        fingerprint = %request.fingerprint,
        duration_secs = request.duration_secs,
        "Alertmanager silence requested"
    );
    match cache
        .silence_alert(&request.fingerprint, duration, &caller.audit)
        .await
    {
        Ok(silence_id) => Json(serde_json::json!({ "silence_id": silence_id })).into_response(),
        Err(roder_k8s::SilenceError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(roder_k8s::SilenceError::AlreadySilenced) => {
            (StatusCode::CONFLICT, "alert is already silenced").into_response()
        }
        Err(roder_k8s::SilenceError::Upstream(error)) => {
            tracing::warn!("Alertmanager silence: {error}");
            bad_gateway(error)
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
pub async fn features(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Extension(b): Extension<Arc<Backend>>,
) -> Response {
    let alerts = state.alerts.read().await;
    let alertmanager = alerts.is_some() && state.config.can_read_alerts(&identity.groups);
    let alertmanager_silences = alertmanager && state.config.can_silence_alerts(&identity.groups);
    let talos = talos_capabilities(&state, &identity);
    Json(serde_json::json!({
        "talos": talos,
        "alertmanager": alertmanager,
        "alertmanager_silences": alertmanager_silences,
        "debug_image": b.debug_image(),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::silence_duration;

    #[test]
    fn silence_duration_accepts_bounded_custom_values() {
        for seconds in [60, 900, 3_600, 43_200, 604_800, 31_536_000] {
            assert_eq!(
                silence_duration(seconds).map(|value| value.as_secs()),
                Some(seconds)
            );
        }
        for seconds in [0, 59, 31_536_001, u64::MAX] {
            assert!(silence_duration(seconds).is_none());
        }
    }
}
