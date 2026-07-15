//! Live resource lists as Server-Sent Events, backed by the shared informer
//! layer: a single-kind stream (`watch`) and a multiplexed one merging
//! several kinds over one connection (`watch_multi`, for a workspace of panes).

use std::convert::Infallible;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream::select_all;
use roder_core::{MultiWatchEvent, WatchEvent};
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::{backend, backend_or_return, bad_gateway};
use crate::server::AppState;

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
    // Suppressed in dev mode: cargo-leptos watch's own hot-reload already
    // owns this concern there, and dev builds don't have a stable hash.
    let version =
        tokio_stream::iter((!state.config.dev_mode).then(|| version_event(&state.asset_version)));
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
    let stream = version.chain(init).chain(live);

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

    // Suppressed in dev mode — see the comment in `watch`.
    if !state.config.dev_mode {
        streams.push(Box::pin(tokio_stream::once(version_event(
            &state.asset_version,
        ))));
    }

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

/// The first event on every watch/watch-multi SSE connection: the server's
/// current build hash, so an already-open tab can detect a redeploy (see
/// `src/version.rs` on the client side). Callers suppress this in dev mode.
fn version_event(asset_version: &str) -> Result<SseEvent, Infallible> {
    Ok(SseEvent::default().event("version").data(asset_version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn version_event_serializes_as_named_sse_event() {
        let ev = version_event("abc123").unwrap();
        let stream = tokio_stream::once(Ok::<_, Infallible>(ev));
        let resp = Sse::new(stream).into_response();
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("event: version"), "got: {text}");
        assert!(text.contains("data: abc123"), "got: {text}");
    }
}
