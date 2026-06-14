use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::stream::select_all;
use roder_core::{MultiWatchEvent, WatchEvent};
use roder_k8s::Backend;
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::AppState;

/// Resolve the connected backend or a 503 if login/connection hasn't happened.
async fn backend(state: &AppState) -> Result<Arc<Backend>, Response> {
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

/// A 502 response carrying the upstream error text.
fn bad_gateway(e: impl std::fmt::Display) -> Response {
    (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
}

/// Normalize an optional namespace query param, treating `""` as `None`.
fn ns_filter(ns: &Option<String>) -> Option<&str> {
    ns.as_deref().filter(|s| !s.is_empty())
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

#[derive(Deserialize)]
pub struct DetailQuery {
    key: String,
    namespace: Option<String>,
    name: String,
}

pub async fn detail(State(state): State<AppState>, Query(q): Query<DetailQuery>) -> Response {
    let b = backend_or_return!(state);
    let ns = ns_filter(&q.namespace);
    match b.detail(&q.key, ns, &q.name).await {
        Ok(d) => Json(d).into_response(),
        Err(e) => bad_gateway(e),
    }
}

#[derive(Deserialize)]
pub struct WatchQuery {
    key: String,
    namespace: Option<String>,
    /// Optional label selector — used to list a workload's own pods.
    selector: Option<String>,
}

/// Live resource list as Server-Sent Events: a `snapshot` followed by `applied` /
/// `deleted` deltas, all fed from a single shared informer (etcd-kind).
pub async fn watch(State(state): State<AppState>, Query(q): Query<WatchQuery>) -> Response {
    let b = backend_or_return!(state);
    let ns = q.namespace.filter(|s| !s.is_empty());
    let selector = q.selector.filter(|s| !s.is_empty());
    let handle = match b.subscribe(&q.key, ns, selector).await {
        Ok(h) => h,
        Err(e) => return bad_gateway(e),
    };

    let rows = handle.rows.clone();
    let columns = handle.columns.clone();
    let snapshot = WatchEvent::Snapshot {
        columns: (**columns.load()).clone(),
        rows: handle.snapshot,
    };
    let init = tokio_stream::once(to_event(&snapshot));
    // On broadcast lag (slow consumer), re-read the current state and send a
    // fresh Snapshot so the client gets a consistent view instead of silently
    // dropping deltas.
    let live = BroadcastStream::new(handle.rx).then(move |res| {
        let rows = rows.clone();
        let columns = columns.clone();
        async move {
            match res {
                Ok(ev) => to_event(&ev),
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => {
                    // Consumer fell behind — send a fresh snapshot so the client
                    // gets a consistent view instead of silently dropping deltas.
                    let current = rows.read().await;
                    let resync = WatchEvent::Snapshot {
                        columns: (**columns.load()).clone(),
                        rows: current.values().cloned().collect(),
                    };
                    to_event(&resync)
                }
            }
        }
    });
    let stream = init.chain(live);

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[derive(Deserialize)]
pub struct MultiWatchQuery {
    panes: String,
}

/// Multiplexed workspace watch: one SSE stream merging N per-kind informer
/// streams, so all panes share a single browser connection.
pub async fn watch_multi(
    State(state): State<AppState>,
    Query(q): Query<MultiWatchQuery>,
) -> Response {
    let b = backend_or_return!(state);

    let pane_specs: Vec<(String, Option<String>)> = q
        .panes
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let mut parts = s.splitn(2, ':');
            let key = parts.next()?.to_string();
            if key.is_empty() {
                return None;
            }
            let ns = parts.next().filter(|n| !n.is_empty()).map(String::from);
            Some((key, ns))
        })
        .collect();

    if pane_specs.is_empty() {
        return (StatusCode::BAD_REQUEST, "no panes specified").into_response();
    }

    type PaneStream =
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<SseEvent, Infallible>> + Send>>;
    let mut streams: Vec<PaneStream> = Vec::new();

    for (key, ns) in pane_specs {
        let handle = match b.subscribe(&key, ns, None).await {
            Ok(h) => h,
            Err(e) => return bad_gateway(e),
        };
        let rows = handle.rows.clone();
        let columns = handle.columns.clone();
        let snapshot = WatchEvent::Snapshot {
            columns: (**columns.load()).clone(),
            rows: handle.snapshot,
        };
        let init = tokio_stream::once(to_multi_event(&MultiWatchEvent {
            key: key.clone(),
            event: snapshot,
        }));
        let live = BroadcastStream::new(handle.rx).then(move |res| {
            let key = key.clone();
            let rows = rows.clone();
            let columns = columns.clone();
            async move {
                let ev = match res {
                    Ok(ev) => ev,
                    Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => {
                        let current = rows.read().await;
                        WatchEvent::Snapshot {
                            columns: (**columns.load()).clone(),
                            rows: current.values().cloned().collect(),
                        }
                    }
                };
                to_multi_event(&MultiWatchEvent { key, event: ev })
            }
        });
        streams.push(Box::pin(init.chain(live)));
    }

    Sse::new(select_all(streams))
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn to_multi_event(ev: &MultiWatchEvent) -> Result<SseEvent, Infallible> {
    Ok(SseEvent::default()
        .json_data(ev)
        .unwrap_or_else(|_| SseEvent::default().data("{}")))
}

// ---- mutations + logs (M6) -----------------------------------------------

#[derive(Deserialize)]
pub struct ActionRequest {
    action: String,
    key: Option<String>,
    namespace: Option<String>,
    name: Option<String>,
    replicas: Option<i32>,
    yaml: Option<String>,
}

/// Resolve the acting identity for the audit log.
async fn actor(state: &AppState, headers: &HeaderMap) -> String {
    if state.config.dev_mode {
        return "dev".to_string();
    }
    // Prefer the live identity (set by `require_auth`); fall back to the cookie.
    if let Some(tokens) = state.current.read().await.as_ref() {
        return tokens
            .identity
            .email
            .clone()
            .unwrap_or_else(|| tokens.identity.subject.clone());
    }
    if let (Some(key), Some(cookie)) = (
        state.config.session_key,
        super::session::cookie_value(headers, super::session::SESSION_COOKIE),
    ) {
        if let Some(tokens) = super::session::open_session(&cookie, &key) {
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
    let b = backend_or_return!(state);
    let ns = ns_filter(&req.namespace);

    // Audit every mutation with the acting identity.
    let who = actor(&state, &headers).await;
    tracing::info!(
        actor = %who,
        action = %req.action,
        key = req.key.as_deref().unwrap_or("-"),
        namespace = ns.unwrap_or("-"),
        name = req.name.as_deref().unwrap_or("-"),
        "mutation requested"
    );

    // `apply` only needs YAML; the rest need key + name.
    let res = if req.action == "apply" {
        match req.yaml.as_deref() {
            Some(y) => b.apply_yaml(y).await,
            None => return (StatusCode::BAD_REQUEST, "missing yaml").into_response(),
        }
    } else {
        let (Some(key), Some(name)) = (req.key.as_deref(), req.name.as_deref()) else {
            return (StatusCode::BAD_REQUEST, "missing key or name").into_response();
        };
        match req.action.as_str() {
            "delete" => b.delete(key, ns, name).await,
            "scale" => b.scale(key, ns, name, req.replicas.unwrap_or(0)).await,
            "restart" => b.rollout_restart(key, ns, name).await,
            "flux-suspend" => b.flux_suspend(key, ns, name, true).await,
            "flux-resume" => b.flux_suspend(key, ns, name, false).await,
            "flux-reconcile" => b.flux_reconcile(key, ns, name).await,
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

#[derive(Deserialize)]
pub struct PermQuery {
    key: String,
    namespace: Option<String>,
}

/// Which mutations the current identity may perform (drives button visibility).
pub async fn permissions(State(state): State<AppState>, Query(q): Query<PermQuery>) -> Response {
    let b = backend_or_return!(state);
    let ns = ns_filter(&q.namespace);
    let patch = b.can("patch", &q.key, ns).await;
    let delete = b.can("delete", &q.key, ns).await;
    Json(serde_json::json!({ "patch": patch, "delete": delete })).into_response()
}

#[derive(Deserialize)]
pub struct LogQuery {
    namespace: String,
    /// Single-pod logs.
    pod: Option<String>,
    container: Option<String>,
    /// Workload logs: aggregate all pods of `key`/`name` (e.g. a Deployment).
    key: Option<String>,
    name: Option<String>,
}

/// Live pod logs as SSE (follows the log stream).
pub async fn logs(State(state): State<AppState>, Query(q): Query<LogQuery>) -> Response {
    let b = backend_or_return!(state);
    let res = if let Some(pod) = q.pod.as_deref() {
        // Only follow a running pod; a finished pod's stream ends at once.
        let follow = b.pod_running(&q.namespace, pod).await;
        b.logs(&q.namespace, pod, q.container, follow).await
    } else if let (Some(key), Some(name)) = (q.key.as_deref(), q.name.as_deref()) {
        b.logs_workload(key, &q.namespace, name, true).await
    } else {
        return (StatusCode::BAD_REQUEST, "logs: need pod, or key+name").into_response();
    };
    // Surface kube API errors as a visible log line rather than a silent 502, so
    // the user sees why logs aren't showing (RBAC denied, container not found, etc.).
    let stream: std::pin::Pin<Box<dyn futures::Stream<Item = String> + Send>> = match res {
        Ok(s) => s,
        Err(e) => Box::pin(futures::stream::once(async move {
            format!("[roder] failed to get logs: {e}")
        })),
    };
    // After the log stream ends (e.g. a completed pod), emit an `eof` event so the
    // browser closes the EventSource instead of auto-reconnecting and replaying.
    let lines = stream.map(|line| Ok::<_, Infallible>(SseEvent::default().data(line)));
    // Non-empty data: an SSE event with an empty data buffer is never dispatched.
    let eof = tokio_stream::once(Ok::<_, Infallible>(
        SseEvent::default().event("eof").data("1"),
    ));
    Sse::new(lines.chain(eof))
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[derive(Deserialize)]
pub struct MetricsQuery {
    namespace: String,
    name: String,
}

/// Get historical metrics data for a pod (CPU and memory over time).
pub async fn metrics_history(
    State(state): State<AppState>,
    Query(q): Query<MetricsQuery>,
) -> Response {
    let b = backend_or_return!(state);
    match b.pod_metrics_history(&q.namespace, &q.name).await {
        Ok(history) => Json(history).into_response(),
        Err(e) => bad_gateway(e),
    }
}

fn to_event(ev: &WatchEvent) -> Result<SseEvent, Infallible> {
    Ok(SseEvent::default()
        .json_data(ev)
        .unwrap_or_else(|_| SseEvent::default().data("{}")))
}
