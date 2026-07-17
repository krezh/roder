//! Drain jobs: spawning the background drain task and streaming its events
//! over SSE with lossless replay (see `server::drain_jobs`).

use std::convert::Infallible;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::{FutureExt, Stream};
use roder_core::{DrainEvent, DrainEventKind, DrainOptions, DrainSummary};
use roder_k8s::Backend;
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::StreamExt;

use crate::server::drain_jobs::JobHandle;
use crate::server::AppState;

/// Spawn a drain as a background job: registers it with `state.drain_jobs`,
/// runs `Backend::drain` on a detached task, and finishes the job with
/// exactly one terminal event once the drain returns.
pub(crate) fn spawn_drain_job(
    state: &AppState,
    backend: Arc<Backend>,
    key: String,
    name: String,
    options: DrainOptions,
) -> u64 {
    let handle = state.drain_jobs.create();
    let id = handle.id;
    let cancel = state.drain_jobs.cancel_flag(id).expect("job just created");
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // Pump backend events into the buffered handle concurrently with the
        // drain so subscribers see progress live, not after the fact. The pump
        // ends when `tx` drops at the end of the drain block.
        let pump = {
            let handle = &handle;
            async move {
                while let Some(kind) = rx.recv().await {
                    handle.emit(kind);
                }
            }
        };
        let drain = async {
            let tx = tx; // move so it drops when the drain finishes
            backend.drain(&key, &name, &options, &tx, &cancel).await
        };
        finish_with(&handle, &cancel, async { tokio::join!(drain, pump).0 }).await;
    });
    id
}

/// Run `fut` to completion, then emit exactly one terminal event on `handle`
/// and finish the job — guaranteed even if `fut` panics.
///
/// Without this, a panic anywhere in the drain (or its concurrent event
/// pump, both of which `fut` wraps) would unwind straight out of the spawned
/// task: tokio silently drops the task, `handle.finish()` never runs, the
/// registry entry's broadcast sender never drops, and every subscriber's
/// `live_events` sits in `rx.recv().await` forever — no terminal event, no
/// `eof`, no expiry. `catch_unwind` restores the "exactly one terminal event,
/// always" invariant by converting a panic into an `Error` event instead.
async fn finish_with<E: std::fmt::Display>(
    handle: &JobHandle,
    cancel: &AtomicBool,
    fut: impl Future<Output = Result<DrainSummary, E>>,
) {
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(Ok(_summary)) if cancel.load(Ordering::Relaxed) => {
            handle.emit(DrainEventKind::Cancelled)
        }
        Ok(Ok(summary)) => handle.emit(DrainEventKind::Done { summary }),
        Ok(Err(e)) => handle.emit(DrainEventKind::Error {
            message: e.to_string(),
        }),
        Err(panic) => handle.emit(DrainEventKind::Error {
            message: panic_message(&panic),
        }),
    }
    handle.finish();
}

/// Best-effort text for a panic payload (`&str`/`String` cover `panic!` and
/// `.expect()`/`.unwrap()`; anything else falls back to a fixed message
/// rather than failing to produce a terminal event at all).
fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    let detail = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned());
    match detail {
        Some(msg) => format!("drain job panicked: {msg}"),
        None => "drain job panicked".to_string(),
    }
}

#[derive(Deserialize)]
pub struct DrainProgressQuery {
    id: u64,
}

/// Does this event end a job's stream? A job always emits exactly one of
/// these last, so the live SSE stream stops right after forwarding it
/// instead of waiting for the broadcast sender itself to drop.
fn is_terminal(kind: &DrainEventKind) -> bool {
    matches!(
        kind,
        DrainEventKind::Done { .. } | DrainEventKind::Error { .. } | DrainEventKind::Cancelled
    )
}

fn sse_event(ev: &DrainEvent) -> Result<SseEvent, Infallible> {
    Ok(SseEvent::default()
        .json_data(ev)
        .unwrap_or_else(|_| SseEvent::default().data("{}")))
}

/// Live events from a job's broadcast channel, stopping right after the
/// terminal one. `take_while`-style combinators can't express this: their
/// predicate only runs on an item the inner stream actually produced, so
/// "stop after the terminal event" would need one *more* item to decide —
/// which never arrives, since the sender lingers in the registry for the
/// `EXPIRY` window after `finish()`. `unfold` carries the stop decision as
/// state instead, so it can end the stream immediately after yielding the
/// terminal item, without waiting on a next poll.
///
/// A lagged receiver (slow subscriber) is not fatal: the client de-dupes by
/// `seq` and a fresh subscribe replays the buffer, so skipping ahead just
/// drops a few progress ticks instead of ending the connection.
fn live_events(
    rx: broadcast::Receiver<DrainEvent>,
) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    futures::stream::unfold((rx, false), |(mut rx, stop)| async move {
        if stop {
            return None;
        }
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let terminal = is_terminal(&ev.kind);
                    return Some((sse_event(&ev), (rx, terminal)));
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    })
}

/// SSE stream of a drain job's events: replays the buffer, then (if the job
/// isn't finished yet) live events up to and including the terminal one,
/// then the `eof` named event (same convention as `logs`/`watch`).
///
/// Auth/backend gating matches the other SSE endpoints: this route lives in
/// `main.rs`'s session-gated `protected` router alongside `logs`/`watch`.
pub async fn drain_progress(
    State(state): State<AppState>,
    Query(q): Query<DrainProgressQuery>,
) -> Response {
    let Some((replay, rx, done)) = state.drain_jobs.subscribe(q.id) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    let replay_stream = tokio_stream::iter(replay).map(|e| sse_event(&e));
    let eof = tokio_stream::once(Ok::<_, Infallible>(
        SseEvent::default().event("eof").data("1"),
    ));
    if done {
        return Sse::new(replay_stream.chain(eof))
            .keep_alive(KeepAlive::default())
            .into_response();
    }
    Sse::new(replay_stream.chain(live_events(rx)).chain(eof))
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_terminal_matches_exactly_the_three_terminal_kinds() {
        assert!(is_terminal(&DrainEventKind::Done {
            summary: Default::default()
        }));
        assert!(is_terminal(&DrainEventKind::Error {
            message: "boom".into()
        }));
        assert!(is_terminal(&DrainEventKind::Cancelled));

        assert!(!is_terminal(&DrainEventKind::Cordoned));
        assert!(!is_terminal(&DrainEventKind::Started { total: 1 }));
        assert!(!is_terminal(&DrainEventKind::NodeReady));
    }

    /// Regression test: `live_events` must end right after the terminal
    /// event, not wait for the broadcast sender to drop (the registry keeps
    /// it alive for `EXPIRY` — 5 minutes — after the job finishes).
    #[tokio::test]
    async fn live_events_ends_at_the_terminal_event_without_the_sender_closing() {
        let (tx, rx) = broadcast::channel(8);
        tx.send(DrainEvent {
            seq: 0,
            kind: DrainEventKind::Cordoned,
        })
        .unwrap();
        tx.send(DrainEvent {
            seq: 1,
            kind: DrainEventKind::Done {
                summary: Default::default(),
            },
        })
        .unwrap();
        // `tx` stays alive (not dropped) for the whole test — if `live_events`
        // waited for the sender to close instead of stopping at the terminal
        // event, the timeout below would trip.
        let body = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            axum::body::to_bytes(Sse::new(live_events(rx)).into_response().into_body(), 8192),
        )
        .await
        .expect("live_events should end at the terminal event, not hang")
        .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(text.matches("data:").count(), 2, "got: {text}");
        assert!(text.contains("\"cordoned\""), "got: {text}");
        assert!(text.contains("\"done\""), "got: {text}");
    }

    /// Regression test: a panic inside the wrapped future must not escape
    /// `finish_with` — it has to land as a single `Error` terminal event,
    /// with the job marked done, exactly like a normal `Err` outcome would.
    /// Without `catch_unwind`, this future's panic would instead unwind the
    /// whole (spawned, in production) task and `handle.finish()` would never
    /// run, permanently hanging subscribers (see `live_events`).
    #[tokio::test]
    async fn finish_with_turns_a_panic_into_a_terminal_error_event() {
        let jobs = Arc::new(crate::server::drain_jobs::DrainJobs::default());
        let handle = jobs.create();
        let id = handle.id;
        let cancel = AtomicBool::new(false);

        finish_with::<String>(&handle, &cancel, async { panic!("boom") }).await;

        let (replay, _rx, done) = jobs.subscribe(id).unwrap();
        assert!(done, "job must be finished even after a panic");
        assert_eq!(
            replay.len(),
            1,
            "exactly one terminal event, got: {replay:?}"
        );
        match &replay[0].kind {
            DrainEventKind::Error { message } => {
                assert!(message.contains("boom"), "got: {message}")
            }
            other => panic!("expected a terminal Error event, got: {other:?}"),
        }
    }

    /// A normal (non-panicking) success still produces exactly one `Done`.
    #[tokio::test]
    async fn finish_with_reports_done_on_a_normal_success() {
        let jobs = Arc::new(crate::server::drain_jobs::DrainJobs::default());
        let handle = jobs.create();
        let id = handle.id;
        let cancel = AtomicBool::new(false);

        finish_with::<String>(&handle, &cancel, async { Ok(DrainSummary::default()) }).await;

        let (replay, _rx, done) = jobs.subscribe(id).unwrap();
        assert!(done);
        assert!(matches!(
            replay.last().unwrap().kind,
            DrainEventKind::Done { .. }
        ));
    }
}
