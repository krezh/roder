//! The interactive pod shell: injecting/creating the target container, then
//! a WebSocket endpoint that proxies its stdin/stdout, plus the terminal
//! page the exec overlay iframe loads.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use roder_k8s::Backend;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct ExecQuery {
    namespace: String,
    pod: String,
    container: Option<String>,
    #[serde(default)]
    node_shell: bool,
}

const EXEC_JOIN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
pub struct DebugShellRequest {
    namespace: String,
    pod: String,
}

/// Injects the configured debug container into a pod and waits for it to reach
/// Running, then returns `{"container": "<name>"}`.
pub async fn debug_shell(
    Extension(b): Extension<Arc<Backend>>,
    Json(request): Json<DebugShellRequest>,
) -> Response {
    match b
        .inject_debug_container(&request.namespace, &request.pod)
        .await
    {
        Ok(container) => Json(serde_json::json!({ "container": container })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct NodeShellRequest {
    node: String,
}

/// Creates a privileged debug pod on `node` and waits for it to reach
/// Running, then returns `{"namespace": .., "pod": ..}` for use with `/api/exec`.
pub async fn node_shell_create(
    Extension(b): Extension<Arc<Backend>>,
    Json(request): Json<NodeShellRequest>,
) -> Response {
    match b.create_node_shell(&request.node).await {
        Ok((namespace, pod)) => {
            Json(serde_json::json!({ "namespace": namespace, "pod": pod })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// WebSocket endpoint that proxies stdin/stdout for an interactive pod shell.
pub async fn exec_ws(
    State(state): State<crate::server::AppState>,
    Extension(b): Extension<Arc<Backend>>,
    headers: HeaderMap,
    Query(q): Query<ExecQuery>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&headers, &state.config.base_url) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |socket| exec_session(socket, b, q))
}

fn origin_allowed(headers: &HeaderMap, base_url: &str) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let (Ok(origin), Ok(base)) = (reqwest::Url::parse(origin), reqwest::Url::parse(base_url))
    else {
        return false;
    };
    origin.origin() == base.origin()
}

async fn cleanup_node_shell(b: &Backend, q: &ExecQuery) {
    if q.node_shell {
        let _ = b.delete_node_shell_pod(&q.namespace, &q.pod).await;
    }
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
            cleanup_node_shell(&b, &q).await;
            return;
        }
    };

    let mut resize_tx = attached.terminal_size();
    let Some(mut stdin) = attached.stdin() else {
        cleanup_node_shell(&b, &q).await;
        return;
    };
    let Some(mut stdout) = attached.stdout() else {
        cleanup_node_shell(&b, &q).await;
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

    let _ = tokio::time::timeout(EXEC_JOIN_TIMEOUT, attached.join()).await;
    cleanup_node_shell(&b, &q).await;
}

/// Serves the xterm.js terminal page loaded in the exec overlay iframe.
pub async fn terminal_page() -> impl IntoResponse {
    axum::response::Html(include_str!("../terminal.html"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_origin_must_match_configured_base_url() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "https://roder.example.com".parse().unwrap());
        assert!(origin_allowed(&headers, "https://roder.example.com/app"));

        headers.insert(header::ORIGIN, "https://evil.example.com".parse().unwrap());
        assert!(!origin_allowed(&headers, "https://roder.example.com"));
    }

    #[test]
    fn websocket_origin_may_be_absent_for_non_browser_clients() {
        assert!(origin_allowed(
            &HeaderMap::new(),
            "https://roder.example.com"
        ));
    }
}
