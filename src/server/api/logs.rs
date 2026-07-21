//! Live pod logs (SSE) and historical CPU/memory metrics for a pod.

use std::convert::Infallible;

use std::sync::Arc;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use roder_k8s::Backend;
use serde::Deserialize;
use tokio_stream::StreamExt;

use super::bad_gateway;

/// The catalog key for pods, used to RBAC-gate the metrics-history lookup
/// against the caller's own `get pods` access (see `metrics_history`).
const POD_KEY: &str = "/v1/Pod";

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
pub async fn logs(Extension(b): Extension<Arc<Backend>>, Query(q): Query<LogQuery>) -> Response {
    let (res, follow) = if let Some(pod) = q.pod.as_deref() {
        // Follow anything not yet in a terminal phase; a pod that's still starting
        // (Pending/ContainerCreating) may produce its first log line, or crash, at
        // any moment, so it needs to be watched just like a Running one.
        let follow = b.pod_active(&q.namespace, pod).await;
        (b.logs(&q.namespace, pod, q.container, follow).await, follow)
    } else if let (Some(key), Some(name)) = (q.key.as_deref(), q.name.as_deref()) {
        (b.logs_workload(key, &q.namespace, name, true).await, true)
    } else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "logs: need pod, or key+name",
        )
            .into_response();
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
///
/// Unlike every other enrichment path, this serves from the shared
/// SA-scraped metrics cache keyed by arbitrary namespace/name, rather than
/// decorating an object the caller's own watch already returned — so it
/// needs its own RBAC gate: only a caller who can `get` the pod itself may
/// read its metrics history.
pub async fn metrics_history(
    Extension(b): Extension<Arc<Backend>>,
    Query(q): Query<MetricsQuery>,
) -> Response {
    if !b.can("get", POD_KEY, Some(&q.namespace)).await {
        return StatusCode::FORBIDDEN.into_response();
    }
    match b.pod_metrics_history(&q.namespace, &q.name).await {
        Ok(history) => Json(history).into_response(),
        Err(e) => bad_gateway(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::handlers::fixtures::test_backend;

    // The test backend has no catalog entry for pods, so its access check
    // returns false without contacting an API server.
    #[tokio::test]
    async fn metrics_history_is_forbidden_without_pod_get_access() {
        let q = MetricsQuery {
            namespace: "other-team".into(),
            name: "some-pod".into(),
        };
        let resp = metrics_history(Extension(test_backend()), Query(q)).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
