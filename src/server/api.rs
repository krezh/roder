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

/// Current firing alerts from Alertmanager, with 30-second in-process cache.
pub async fn alerts(State(state): State<AppState>) -> Response {
    let cache = state.alerts.read().await.as_ref().map(Arc::clone);
    let Some(cache) = cache else {
        tracing::warn!("alerts: no alertmanager configured (discovery found nothing at startup)");
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
pub struct ResourceTreeQuery {
    key: String,
    namespace: Option<String>,
    name: String,
}

/// Full recursive ownership tree for a Kustomization/HelmRelease, resolved
/// server-side in one shot (see `Backend::resource_tree`).
pub async fn resource_tree(
    State(state): State<AppState>,
    Query(q): Query<ResourceTreeQuery>,
) -> Response {
    let b = backend_or_return!(state);
    let ns = ns_filter(&q.namespace);
    match b.resource_tree(&q.key, ns, &q.name).await {
        Ok(tree) => Json(tree).into_response(),
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
    let init = tokio_stream::once(sse_event(&snapshot));
    // On broadcast lag (slow consumer), re-read the current state and send a
    // fresh Snapshot so the client gets a consistent view instead of silently
    // dropping deltas.
    let live = BroadcastStream::new(handle.rx).then(move |res| {
        let rows = rows.clone();
        let columns = columns.clone();
        async move {
            match res {
                Ok(ev) => sse_event(&ev),
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => {
                    // Consumer fell behind — send a fresh snapshot so the client
                    // gets a consistent view instead of silently dropping deltas.
                    let current = rows.read().await;
                    let resync = WatchEvent::Snapshot {
                        columns: (**columns.load()).clone(),
                        rows: current.values().cloned().collect(),
                    };
                    sse_event(&resync)
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
        let init = tokio_stream::once(sse_event(&MultiWatchEvent {
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
                sse_event(&MultiWatchEvent { key, event: ev })
            }
        });
        streams.push(Box::pin(init.chain(live)));
    }

    Sse::new(select_all(streams))
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn sse_event<T: serde::Serialize>(ev: &T) -> Result<SseEvent, Infallible> {
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
    force: Option<bool>,
    reset: Option<bool>,
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
    let (res, follow) = if let Some(pod) = q.pod.as_deref() {
        // Follow anything not yet in a terminal phase; a pod that's still starting
        // (Pending/ContainerCreating) may produce its first log line, or crash, at
        // any moment, so it needs to be watched just like a Running one.
        let follow = b.pod_active(&q.namespace, pod).await;
        (b.logs(&q.namespace, pod, q.container, follow).await, follow)
    } else if let (Some(key), Some(name)) = (q.key.as_deref(), q.name.as_deref()) {
        (b.logs_workload(key, &q.namespace, name, true).await, true)
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
    let lines = stream.map(|line| Ok::<_, Infallible>(SseEvent::default().data(line)));
    if follow {
        // Still following: the stream ending here (container not started yet, a
        // dropped connection, ...) doesn't mean there will never be more output, so
        // skip `eof` and let the browser's EventSource auto-reconnect and retry
        // instead of going dark until the user manually refreshes.
        return Sse::new(lines)
            .keep_alive(KeepAlive::default())
            .into_response();
    }
    // A one-shot fetch of a pod that has already finished for good: emit `eof` so
    // the browser closes the EventSource instead of reconnecting and replaying.
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

// ---- pod exec (WebSocket terminal) ---------------------------------------

#[derive(Deserialize)]
pub struct ExecQuery {
    namespace: String,
    pod: String,
    container: Option<String>,
    #[serde(default)]
    node_shell: bool,
}

/// Injects a `nicolaka/netshoot` ephemeral container into a pod and waits for
/// it to reach Running, then returns `{"container": "<name>"}`.
pub async fn debug_shell(State(state): State<AppState>, Query(q): Query<ExecQuery>) -> Response {
    let b = backend_or_return!(state);
    match b.inject_debug_container(&q.namespace, &q.pod).await {
        Ok(container) => Json(serde_json::json!({ "container": container })).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct NodeShellQuery {
    node: String,
}

/// Creates a privileged debug pod on `node` and waits for it to reach
/// Running, then returns `{"namespace": .., "pod": ..}` for use with `/api/exec`.
pub async fn node_shell_create(
    State(state): State<AppState>,
    Query(q): Query<NodeShellQuery>,
) -> Response {
    let b = backend_or_return!(state);
    match b.create_node_shell(&q.node).await {
        Ok((namespace, pod)) => {
            Json(serde_json::json!({ "namespace": namespace, "pod": pod })).into_response()
        }
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// WebSocket endpoint that proxies stdin/stdout for an interactive pod shell.
pub async fn exec_ws(
    State(state): State<AppState>,
    Query(q): Query<ExecQuery>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    let b = backend_or_return!(state);
    ws.on_upgrade(move |socket| exec_session(socket, b, q))
}

async fn exec_session(socket: axum::extract::ws::WebSocket, b: Arc<Backend>, q: ExecQuery) {
    use axum::extract::ws::Message;
    use futures::{SinkExt, StreamExt};
    use roder_k8s::TerminalSize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut ws_sink, mut ws_stream) = socket.split();

    let exec_result = if q.node_shell {
        b.exec_node_shell(&q.namespace, &q.pod).await
    } else {
        b.exec(
            &q.namespace,
            &q.pod,
            q.container.as_deref().filter(|s| !s.is_empty()),
        )
        .await
    };
    let mut attached = match exec_result {
        Ok(a) => a,
        Err(e) => {
            let msg = format!("\r\n\x1b[31m[exec: {}]\x1b[0m\r\n", e);
            let _ = ws_sink.send(Message::Binary(msg.into_bytes().into())).await;
            if q.node_shell {
                let _ = b.delete_node_shell_pod(&q.namespace, &q.pod).await;
            }
            return;
        }
    };

    let mut resize_tx = attached.terminal_size();
    let Some(mut stdin) = attached.stdin() else {
        return;
    };
    let Some(mut stdout) = attached.stdout() else {
        return;
    };

    let to_client = async move {
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if ws_sink
                        .send(Message::Binary(buf[..n].to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    };

    let from_client = async move {
        while let Some(Ok(msg)) = futures::StreamExt::next(&mut ws_stream).await {
            match msg {
                Message::Binary(data) => {
                    if stdin.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Message::Text(txt) => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(txt.as_str()) {
                        if v.get("type").and_then(|t| t.as_str()) == Some("resize") {
                            if let (Some(r), Some(c)) = (
                                v.get("rows").and_then(|v| v.as_u64()),
                                v.get("cols").and_then(|v| v.as_u64()),
                            ) {
                                if let Some(ref mut tx) = resize_tx {
                                    let _ = tx
                                        .send(TerminalSize {
                                            height: r as u16,
                                            width: c as u16,
                                        })
                                        .await;
                                }
                            }
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };

    tokio::select! {
        _ = to_client => {}
        _ = from_client => {}
    }

    let _ = attached.join().await;
    if q.node_shell {
        let _ = b.delete_node_shell_pod(&q.namespace, &q.pod).await;
    }
}

/// Serves the xterm.js terminal page loaded in the exec overlay iframe.
pub async fn terminal_page() -> impl IntoResponse {
    axum::response::Html(include_str!("terminal.html"))
}
