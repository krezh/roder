//! Everything Talos-specific: reading a node's machine status, diffing its
//! config against its peers, streaming dmesg, and the power/service actions
//! dispatched from `POST /api/action`.

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use roder_core::{
    ObjectDetail, TalosCapabilities, TalosConfigDiff, TalosConfigDifference, TalosConfigPeerDiff,
};
use roder_k8s::Backend;
use serde::Deserialize;
use tokio_stream::StreamExt;

use super::action::ActionRequest;
use super::{backend, backend_or_return, bad_gateway};
use crate::server::AppState;

fn talos_error(e: roder_talos::TalosError) -> Response {
    if e.is_timeout() {
        return (StatusCode::GATEWAY_TIMEOUT, e.to_string()).into_response();
    }
    match e {
        error if error.is_unavailable() => {
            (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response()
        }
        error => bad_gateway(error),
    }
}

#[derive(Deserialize)]
pub struct TalosNodeQuery {
    node: String,
}

fn request_groups(state: &AppState, headers: &HeaderMap) -> Option<Vec<String>> {
    if state.config.dev_mode {
        return Some(Vec::new());
    }
    let key = state.config.session_key?;
    let cookie =
        crate::server::session::cookie_value(headers, crate::server::session::SESSION_COOKIE)?;
    crate::server::session::open_session(&cookie, &key).map(|tokens| tokens.identity.groups)
}

pub(crate) fn talos_capabilities(state: &AppState, headers: &HeaderMap) -> TalosCapabilities {
    if state.talos.is_none() {
        return TalosCapabilities::default();
    }
    let Some(groups) = request_groups(state, headers) else {
        return TalosCapabilities::default();
    };
    let actions = state.config.can_operate_talos(&groups);
    let config = state.config.can_read_talos_config(&groups);
    TalosCapabilities {
        read: actions || config || state.config.can_read_talos(&groups),
        actions,
        config,
    }
}

async fn visible_node(backend: &Backend, node: &str) -> Result<(String, ObjectDetail), Response> {
    let key = backend
        .kinds()
        .into_iter()
        .find(|kind| kind.group.is_empty() && kind.kind == "Node")
        .map(|kind| kind.key)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Node resource unavailable").into_response())?;
    let detail = backend.detail(&key, None, node).await.map_err(|error| {
        tracing::warn!(node, "Talos node validation failed: {error}");
        (StatusCode::NOT_FOUND, "node is unavailable").into_response()
    })?;
    Ok((key, detail))
}

fn is_control_plane(detail: &ObjectDetail) -> bool {
    detail
        .object
        .pointer("/metadata/labels")
        .and_then(|labels| labels.as_object())
        .is_some_and(|labels| {
            labels.contains_key("node-role.kubernetes.io/control-plane")
                || labels.contains_key("node-role.kubernetes.io/master")
        })
}

/// Read-only machine status through Talos's native in-cluster API service.
pub async fn talos_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<TalosNodeQuery>,
) -> Response {
    if q.node.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing node").into_response();
    }
    let Some(talos) = state.talos.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let capabilities = talos_capabilities(&state, &headers);
    if !capabilities.read {
        return StatusCode::FORBIDDEN.into_response();
    }
    let backend = backend_or_return!(state);
    let (_, detail) = match visible_node(&backend, &q.node).await {
        Ok(node) => node,
        Err(response) => return response,
    };
    match talos.node(&q.node).await {
        Ok(mut status) => {
            status.control_plane = is_control_plane(&detail);
            if capabilities.config {
                match talos.config_snapshot(&q.node).await {
                    Ok(snapshot) => status.config_fingerprint = Some(snapshot.fingerprint),
                    Err(error) => {
                        status
                            .errors
                            .insert("machine config".into(), error.to_string());
                    }
                }
            }
            Json(status).into_response()
        }
        Err(e) => talos_error(e),
    }
}

/// Redacted machine-configuration differences against the other Talos nodes.
pub async fn talos_config_diff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<TalosNodeQuery>,
) -> Response {
    if q.node.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing node").into_response();
    }
    if !talos_capabilities(&state, &headers).config {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(talos) = state.talos.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let backend = backend_or_return!(state);
    if let Err(response) = visible_node(&backend, &q.node).await {
        return response;
    }
    let selected = match talos.config_snapshot(&q.node).await {
        Ok(snapshot) => snapshot,
        Err(error) => return talos_error(error),
    };
    let overview = match backend.overview().await {
        Ok(overview) => overview,
        Err(error) => return bad_gateway(error),
    };
    let mut peers = Vec::new();
    for peer in overview.nodes.into_iter().filter(|node| {
        node.name != q.node
            && node
                .os_image
                .as_deref()
                .is_some_and(|image| image.starts_with("Talos"))
    }) {
        if visible_node(&backend, &peer.name).await.is_err() {
            continue;
        }
        match talos.config_snapshot(&peer.name).await {
            Ok(snapshot) => peers.push(TalosConfigPeerDiff {
                node: peer.name,
                fingerprint: Some(snapshot.fingerprint.clone()),
                matches: Some(snapshot.fingerprint == selected.fingerprint),
                differences: config_differences(&selected.fields, &snapshot.fields),
                error: None,
            }),
            Err(error) => peers.push(TalosConfigPeerDiff {
                node: peer.name,
                fingerprint: None,
                matches: None,
                differences: Vec::new(),
                error: Some(error.to_string()),
            }),
        }
    }
    Json(TalosConfigDiff {
        node: q.node,
        fingerprint: selected.fingerprint,
        peers,
    })
    .into_response()
}

fn config_differences(
    selected: &std::collections::BTreeMap<String, roder_talos::ConfigField>,
    peer: &std::collections::BTreeMap<String, roder_talos::ConfigField>,
) -> Vec<TalosConfigDifference> {
    selected
        .keys()
        .chain(peer.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|path| {
            let left = selected.get(&path);
            let right = peer.get(&path);
            if left == right {
                return None;
            }
            let sensitive = matches!(left, Some(roder_talos::ConfigField::Sensitive(_)))
                || matches!(right, Some(roder_talos::ConfigField::Sensitive(_)));
            Some(TalosConfigDifference {
                path,
                node_value: config_display_value(left),
                peer_value: config_display_value(right),
                sensitive,
            })
        })
        .collect()
}

fn config_display_value(value: Option<&roder_talos::ConfigField>) -> Option<String> {
    value.map(|value| match value {
        roder_talos::ConfigField::Plain(value) => value.clone(),
        roder_talos::ConfigField::Sensitive(_) => "<redacted>".into(),
    })
}

/// Live kernel ring buffer from a Talos node as SSE.
pub async fn talos_dmesg(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<TalosNodeQuery>,
) -> Response {
    if q.node.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing node").into_response();
    }
    let Some(talos) = state.talos.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !talos_capabilities(&state, &headers).read {
        return StatusCode::FORBIDDEN.into_response();
    }
    let backend = backend_or_return!(state);
    if let Err(response) = visible_node(&backend, &q.node).await {
        return response;
    }
    let stream: std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<String, roder_talos::TalosError>> + Send>,
    > = match talos.dmesg(&q.node).await {
        Ok(stream) => stream,
        Err(error) => Box::pin(futures::stream::once(async move { Err(error) })),
    };
    let lines = stream.map(|result| {
        let line =
            result.unwrap_or_else(|error| format!("[roder] Talos log stream failed: {error}"));
        Ok::<_, Infallible>(SseEvent::default().data(line))
    });
    let eof = tokio_stream::once(Ok::<_, Infallible>(
        SseEvent::default().event("eof").data("1"),
    ));
    Sse::new(lines.chain(eof))
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn talos_lock_conflict() -> Response {
    (
        StatusCode::CONFLICT,
        "another Talos operation is already in progress",
    )
        .into_response()
}

/// The Talos-specific branch of `POST /api/action` (service start/stop/
/// restart, reboot, shutdown). Returns `None` when `req.action` isn't a Talos
/// action, so the generic dispatcher in `action.rs` can fall through to it.
pub(crate) async fn talos_mutation(
    state: &AppState,
    headers: &HeaderMap,
    req: &ActionRequest,
) -> Option<Response> {
    if !matches!(
        req.action.as_str(),
        "talos-service-start"
            | "talos-service-stop"
            | "talos-service-restart"
            | "talos-reboot"
            | "talos-shutdown"
    ) {
        return None;
    }
    if !talos_capabilities(state, headers).actions {
        return Some(StatusCode::FORBIDDEN.into_response());
    }
    let Some(talos) = state.talos.as_ref() else {
        return Some(StatusCode::NOT_FOUND.into_response());
    };
    let Some(node) = req.name.as_deref() else {
        return Some((StatusCode::BAD_REQUEST, "missing node name").into_response());
    };
    let backend = match backend(state).await {
        Ok(b) => b,
        Err(r) => return Some(r),
    };
    let (node_key, detail) = match visible_node(&backend, node).await {
        Ok(node) => node,
        Err(response) => return Some(response),
    };
    let power_action = matches!(req.action.as_str(), "talos-reboot" | "talos-shutdown");
    if power_action && state.config.pod_node_name.as_deref() == Some(node) {
        return Some(
            (
                StatusCode::CONFLICT,
                "refusing to power off the node hosting this Roder instance",
            )
                .into_response(),
        );
    }
    let was_cordoned = detail
        .object
        .pointer("/spec/unschedulable")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let previous_boot_id = detail
        .object
        .pointer("/status/nodeInfo/bootID")
        .and_then(|value| value.as_str())
        .map(String::from);

    if power_action && req.drain.unwrap_or(false) {
        // Drain-first power actions run as a chained background job (drain,
        // then power off/reboot + wait), streamed over SSE — see
        // `drain::spawn_drain_job`/`drain::PowerPhase`. The guard has to be
        // an *owned* lock so it can move into the job and release when the
        // job (including the reboot wait) ends, rather than when this
        // request handler returns.
        let lock = match Arc::clone(&state.talos_action_lock).try_lock_owned() {
            Ok(guard) => guard,
            Err(_) => return Some(talos_lock_conflict()),
        };
        let phase = super::drain::PowerPhase {
            action: req.action.trim_start_matches("talos-").to_string(),
            talos: Arc::clone(talos),
            node_key: node_key.clone(),
            was_cordoned,
            previous_boot_id,
            lock,
        };
        let id = super::drain::spawn_drain_job(
            state,
            backend,
            node_key,
            node.to_string(),
            req.options.clone().unwrap_or_default(),
            Some(phase),
        );
        return Some(
            (StatusCode::OK, serde_json::json!({ "job": id }).to_string()).into_response(),
        );
    }

    let _action_guard = match state.talos_action_lock.try_lock() {
        Ok(guard) => guard,
        Err(_) => return Some(talos_lock_conflict()),
    };
    let result = match req.action.as_str() {
        "talos-service-start" | "talos-service-stop" | "talos-service-restart" => {
            let Some(service) = req.service.as_deref() else {
                return Some((StatusCode::BAD_REQUEST, "missing service").into_response());
            };
            talos
                .service_action(
                    node,
                    service,
                    req.action.trim_start_matches("talos-service-"),
                )
                .await
        }
        "talos-reboot" => talos.power_action(node, "reboot").await,
        "talos-shutdown" => talos.power_action(node, "shutdown").await,
        _ => unreachable!(),
    };
    if let Err(error) = result {
        return Some(talos_error(error));
    }
    if req.action == "talos-reboot" {
        if let Err(error) = backend
            .wait_for_node_reboot(
                node,
                previous_boot_id.as_deref(),
                std::time::Duration::from_secs(300),
            )
            .await
        {
            return Some((StatusCode::GATEWAY_TIMEOUT, error.to_string()).into_response());
        }
    }
    Some(
        Json(serde_json::json!({
            "status": if req.action == "talos-reboot" { "ready" } else { "requested" },
        }))
        .into_response(),
    )
}
