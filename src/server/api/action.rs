//! `POST /api/action`: the single mutation endpoint, dispatching by
//! `action`. Talos-specific actions are handled by `talos::talos_mutation`;
//! everything else is a generic per-resource-kind mutation.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::talos::talos_mutation;
use super::{backend, backend_or_return, bad_gateway, ns_filter};
use crate::server::AppState;

#[derive(Deserialize)]
pub struct ActionRequest {
    pub(crate) action: String,
    key: Option<String>,
    namespace: Option<String>,
    pub(crate) name: Option<String>,
    replicas: Option<i32>,
    yaml: Option<String>,
    pub(crate) force: Option<bool>,
    reset: Option<bool>,
    pub(crate) service: Option<String>,
    pub(crate) drain: Option<bool>,
    pub(crate) options: Option<roder_core::DrainOptions>,
    pub(crate) job: Option<u64>,
}

/// Resolve the acting identity for the audit log.
async fn actor(state: &AppState, headers: &HeaderMap) -> String {
    if state.config.dev_mode {
        return "dev".to_string();
    }
    if let (Some(key), Some(cookie)) = (
        state.config.session_key,
        crate::server::session::cookie_value(headers, crate::server::session::SESSION_COOKIE),
    ) {
        if let Some(tokens) = crate::server::session::open_session(&cookie, &key) {
            return tokens.identity.email.unwrap_or(tokens.identity.subject);
        }
    }
    "unknown".to_string()
}

/// Single mutation endpoint dispatching by `action`.
pub async fn action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ActionRequest>,
) -> Response {
    // Audit every mutation with the acting identity.
    let who = actor(&state, &headers).await;
    let ns = ns_filter(&req.namespace);
    tracing::info!(
        actor = %who,
        action = %req.action,
        key = req.key.as_deref().unwrap_or("-"),
        namespace = ns.unwrap_or("-"),
        name = req.name.as_deref().unwrap_or("-"),
        "mutation requested"
    );

    if let Some(response) = talos_mutation(&state, &headers, &req).await {
        return response;
    }

    let b = backend_or_return!(state);

    // `apply` and `sanitize` don't operate on a named resource.
    let res = if req.action == "apply" {
        match req.yaml.as_deref() {
            Some(y) => b.apply_yaml(y).await,
            None => return (StatusCode::BAD_REQUEST, "missing yaml").into_response(),
        }
    } else if req.action == "sanitize" {
        return match b.sanitize(req.namespace.clone()).await {
            Ok(summary) => {
                (StatusCode::OK, serde_json::to_string(&summary).unwrap()).into_response()
            }
            Err(e) => bad_gateway(e),
        };
    } else if req.action == "flux-reconcile-all" {
        return match b.flux_reconcile_all(ns).await {
            Ok(n) => (StatusCode::OK, n.to_string()).into_response(),
            Err(e) => bad_gateway(e),
        };
    } else if req.action == "drain" {
        let (Some(key), Some(name)) = (req.key.as_deref(), req.name.as_deref()) else {
            return (StatusCode::BAD_REQUEST, "missing key or name").into_response();
        };
        let options = req.options.clone().unwrap_or_default();
        let id = super::drain::spawn_drain_job(
            &state,
            b,
            key.to_string(),
            name.to_string(),
            options,
            None,
        );
        return (StatusCode::OK, serde_json::json!({ "job": id }).to_string()).into_response();
    } else if req.action == "drain-cancel" {
        let Some(id) = req.job else {
            return (StatusCode::BAD_REQUEST, "missing job").into_response();
        };
        return if state.drain_jobs.cancel(id) {
            (StatusCode::OK, "ok").into_response()
        } else {
            (StatusCode::NOT_FOUND, "unknown job").into_response()
        };
    } else {
        let (Some(key), Some(name)) = (req.key.as_deref(), req.name.as_deref()) else {
            return (StatusCode::BAD_REQUEST, "missing key or name").into_response();
        };
        match req.action.as_str() {
            "delete" => b.delete(key, ns, name).await,
            "evict" => {
                let Some(ns) = ns else {
                    return (StatusCode::BAD_REQUEST, "missing namespace").into_response();
                };
                b.evict_pod(ns, name).await
            }
            "scale" => b.scale(key, ns, name, req.replicas.unwrap_or(0)).await,
            "restart" => b.rollout_restart(key, ns, name).await,
            "flux-suspend" => b.flux_suspend(key, ns, name, true).await,
            "flux-resume" => b.flux_suspend(key, ns, name, false).await,
            "cordon" => b.cordon(key, name, true).await,
            "uncordon" => b.cordon(key, name, false).await,
            "flux-reconcile" => {
                b.flux_reconcile(
                    key,
                    ns,
                    name,
                    req.force.unwrap_or(false),
                    req.reset.unwrap_or(false),
                )
                .await
            }
            "flux-reconcile-with-source" => {
                b.flux_reconcile_with_source(
                    key,
                    ns,
                    name,
                    req.force.unwrap_or(false),
                    req.reset.unwrap_or(false),
                )
                .await
            }
            "flux-force" => b.flux_force(key, ns, name).await,
            "flux-reset" => b.flux_reset(key, ns, name).await,
            "eso-refresh" => b.eso_refresh(key, ns, name).await,
            "cronjob-trigger" => b.cronjob_trigger(key, ns, name).await,
            other => {
                return (StatusCode::BAD_REQUEST, format!("unknown action: {other}"))
                    .into_response()
            }
        }
    };

    match res {
        Ok(()) => (StatusCode::OK, "ok").into_response(),
        Err(e) => bad_gateway(e),
    }
}
