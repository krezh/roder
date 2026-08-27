//! `POST /api/action`: the single mutation endpoint, dispatching by
//! `action`. Talos-specific actions are handled by `talos::talos_mutation`;
//! everything else is a generic per-resource-kind mutation.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use roder_k8s::Backend;
use serde::{Deserialize, Serialize};

use super::talos::talos_mutation;
use super::{bad_gateway, ns_filter, request_caller};
use crate::server::drain_jobs::CancelResult;
use crate::server::AppState;

#[derive(Deserialize, Serialize)]
pub struct ActionRequest {
    pub(crate) action: String,
    key: Option<String>,
    namespace: Option<String>,
    pub(crate) name: Option<String>,
    replicas: Option<i32>,
    yaml: Option<String>,
    pub(crate) force: Option<bool>,
    pub(crate) propagation: Option<roder_core::DeletePropagation>,
    reset: Option<bool>,
    pub(crate) service: Option<String>,
    pub(crate) drain: Option<bool>,
    pub(crate) options: Option<roder_core::DrainOptions>,
    pub(crate) job: Option<u64>,
    pub(crate) executor: Option<String>,
}

/// Single mutation endpoint dispatching by `action`.
pub async fn action(
    State(state): State<AppState>,
    Extension(b): Extension<Arc<Backend>>,
    Extension(identity): Extension<roder_auth::Identity>,
    headers: HeaderMap,
    Json(req): Json<ActionRequest>,
) -> Response {
    let Some(caller) = request_caller(&identity) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // Audit every mutation with the acting identity.
    let ns = ns_filter(&req.namespace);
    tracing::info!(
        actor = %caller.audit,
        action = %req.action,
        key = req.key.as_deref().unwrap_or("-"),
        namespace = ns.unwrap_or("-"),
        name = req.name.as_deref().unwrap_or("-"),
        "mutation requested"
    );

    if req.action == "drain-cancel" {
        if let Some(response) = crate::server::ha::proxy_to_executor(
            &state,
            &identity,
            req.executor.as_deref(),
            reqwest::Method::POST,
            "/api/action",
            Some(&req),
        )
        .await
        {
            return response;
        }
        let Some(id) = req.job else {
            return (StatusCode::BAD_REQUEST, "missing job").into_response();
        };
        return match state.drain_jobs.cancel(&caller.owner, id) {
            CancelResult::Accepted => (StatusCode::OK, "ok").into_response(),
            CancelResult::NotFound => (StatusCode::NOT_FOUND, "unknown job").into_response(),
            CancelResult::NotCancellable => {
                (StatusCode::CONFLICT, "job is no longer cancellable").into_response()
            }
        };
    }

    let drain_options = if req.action == "drain" {
        let options = req.options.clone().unwrap_or_default();
        if let Err(error) = options.validate() {
            return (StatusCode::BAD_REQUEST, error.to_string()).into_response();
        }
        Some(options)
    } else {
        None
    };

    if let Some(response) =
        talos_mutation(&state, &headers, &identity, &caller.owner, &req, b.clone()).await
    {
        return response;
    }

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
        let options = drain_options.expect("drain options validated above");
        if let Some(response) =
            crate::server::ha::forward_action_from_target(&state, &headers, &identity, name, &req)
                .await
        {
            return response;
        }
        let lease = match state.ha.as_ref() {
            Some(ha) => match ha.coordinator.acquire(name).await {
                Ok(lease) => Some(lease),
                Err(error) => return (StatusCode::CONFLICT, error.to_string()).into_response(),
            },
            None => None,
        };
        let id = match super::drain::spawn_drain_job(
            &state,
            b,
            super::drain::DrainJobRequest {
                owner: caller.owner,
                key: key.to_string(),
                name: name.to_string(),
                options,
                power: None,
                lease,
            },
        ) {
            Ok(id) => id,
            Err(_) => {
                return (
                    StatusCode::CONFLICT,
                    "a drain is already active for this node",
                )
                    .into_response()
            }
        };
        return (
            StatusCode::OK,
            serde_json::json!({
                "job": id,
                "executor": state.ha.as_ref().map(|ha| ha.pod_name.as_str())
            })
            .to_string(),
        )
            .into_response();
    } else {
        let (Some(key), Some(name)) = (req.key.as_deref(), req.name.as_deref()) else {
            return (StatusCode::BAD_REQUEST, "missing key or name").into_response();
        };
        match req.action.as_str() {
            "delete" => {
                b.delete(key, ns, name, req.force.unwrap_or(false), req.propagation)
                    .await
            }
            "evict" => {
                let Some(ns) = ns else {
                    return (StatusCode::BAD_REQUEST, "missing namespace").into_response();
                };
                b.evict_pod(ns, name).await
            }
            "scale" => {
                let Some(replicas) = req.replicas else {
                    return (StatusCode::BAD_REQUEST, "missing replicas").into_response();
                };
                b.scale(key, ns, name, replicas).await
            }
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
            "certificate-renew" => b.certificate_renew(key, ns, name).await,
            "cronjob-trigger" => b.cronjob_trigger(key, ns, name).await,
            "job-rerun" => b.job_rerun(key, ns, name).await,
            "kopiur-snapshot-now" => b.kopiur_snapshot_now(key, ns, name).await,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::handlers::fixtures::{
        fake_tokens, prod_state_without_provider, sealed_cookie_header, test_backend,
    };

    fn request(action: &str) -> ActionRequest {
        ActionRequest {
            action: action.into(),
            key: None,
            namespace: None,
            name: None,
            replicas: None,
            yaml: None,
            force: None,
            propagation: None,
            reset: None,
            service: None,
            drain: None,
            options: None,
            job: None,
            executor: None,
        }
    }

    #[tokio::test]
    async fn drain_cancel_is_owner_scoped_without_requiring_a_backend() {
        let state = prod_state_without_provider();
        let mut owner = fake_tokens();
        owner.identity.subject = "owner-a".into();
        let handle = state
            .drain_jobs
            .create("owner-a".into(), "node-a".into())
            .unwrap();
        let flag = handle.cancel_flag();
        let mut cancel = request("drain-cancel");
        cancel.job = Some(handle.id);

        let response = action(
            State(state.clone()),
            Extension(test_backend()),
            Extension(owner.identity.clone()),
            sealed_cookie_header(&owner),
            Json(cancel),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed));

        let second = state
            .drain_jobs
            .create("owner-a".into(), "node-b".into())
            .unwrap();
        let second_flag = second.cancel_flag();
        let mut foreign = fake_tokens();
        foreign.identity.subject = "owner-b".into();
        let mut cancel = request("drain-cancel");
        cancel.job = Some(second.id);
        let response = action(
            State(state),
            Extension(test_backend()),
            Extension(foreign.identity.clone()),
            sealed_cookie_header(&foreign),
            Json(cancel),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!second_flag.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn drain_cancel_reports_non_cancellable_power_phase() {
        let state = prod_state_without_provider();
        let mut owner = fake_tokens();
        owner.identity.subject = "owner-a".into();
        let handle = state
            .drain_jobs
            .create("owner-a".into(), "node-a".into())
            .unwrap();
        assert!(handle.begin_non_cancellable());
        let mut cancel = request("drain-cancel");
        cancel.job = Some(handle.id);

        let response = action(
            State(state),
            Extension(test_backend()),
            Extension(owner.identity.clone()),
            sealed_cookie_header(&owner),
            Json(cancel),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn drain_options_are_rejected_before_backend_resolution() {
        let state = prod_state_without_provider();
        let tokens = fake_tokens();
        let mut drain = request("drain");
        drain.key = Some("nodes.v1.".into());
        drain.name = Some("node-a".into());
        drain.options = Some(roder_core::DrainOptions {
            timeout_secs: roder_core::DRAIN_TIMEOUT_MAX_SECS + 1,
            ..Default::default()
        });

        let response = action(
            State(state),
            Extension(test_backend()),
            Extension(tokens.identity.clone()),
            sealed_cookie_header(&tokens),
            Json(drain),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn scale_requires_replicas() {
        let state = prod_state_without_provider();
        let tokens = fake_tokens();
        let mut scale = request("scale");
        scale.key = Some("deployments.v1.apps".into());
        scale.namespace = Some("default".into());
        scale.name = Some("api".into());

        let response = action(
            State(state),
            Extension(test_backend()),
            Extension(tokens.identity.clone()),
            sealed_cookie_header(&tokens),
            Json(scale),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
