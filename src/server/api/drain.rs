//! Drain jobs: spawning the background drain task and streaming its events
//! over SSE with lossless replay (see `server::drain_jobs`).

use std::collections::VecDeque;
use std::convert::Infallible;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::{FutureExt, Stream};
use roder_core::{DrainEvent, DrainEventKind, DrainOptions};
use roder_k8s::Backend;
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::StreamExt;

use crate::server::drain_jobs::{CreateError, DrainJobs, JobHandle};
use crate::server::AppState;

/// The chained power/wait phase run after a successful, unblocked drain when
/// the caller requested drain-then-reboot/shutdown (`talos_mutation`'s
/// drain-first path). Holds the Talos action-serialization guard for the
/// whole phase — including the reboot wait — so it releases exactly when the
/// job ends, whether that's success, failure, or a panic.
pub(crate) struct PowerPhase {
    pub action: String,
    pub talos: Arc<roder_talos::Backend>,
    pub was_cordoned: bool,
    pub previous_boot_id: Option<String>,
    /// Never read — held only for its `Drop`, which releases the guard when
    /// this `PowerPhase` (and thus the job) ends.
    #[allow(dead_code)]
    pub lock: tokio::sync::OwnedMutexGuard<()>,
}

/// Spawn a drain (optionally chained into a power action) as a background
/// job: registers it with `state.drain_jobs`, runs `Backend::drain` on a
/// detached task, then — if `power` is `Some` and the drain succeeded
/// without blockers — powers the node off/reboots it and waits for the
/// reboot, before finishing the job with exactly one terminal event.
pub(crate) fn spawn_drain_job(
    state: &AppState,
    backend: Arc<Backend>,
    owner: String,
    key: String,
    name: String,
    options: DrainOptions,
    power: Option<PowerPhase>,
) -> Result<u64, CreateError> {
    let power_action = power.as_ref().map(|phase| phase.action.clone());
    let handle = state
        .drain_jobs
        .create_for(owner, key.clone(), name.clone(), power_action)?;
    let id = handle.id;
    let cancel = handle.cancel_flag();
    // Capture the request's Kubernetes identity before detaching the task.
    let session = backend.drain_session();
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
            session.drain(&key, &name, &options, &tx, &cancel).await
        };
        finish_with(&handle, async {
            // Join drain+pump to completion first, so every progress event is
            // flushed (in order) before any power-phase event is emitted —
            // otherwise `PowerRequested` (emitted synchronously, before its
            // first await) could get a lower `seq` than trailing drain ticks
            // still sitting in the unbounded channel.
            let result = tokio::join!(drain, pump).0;
            match result {
                Ok(_summary) if !handle.begin_non_cancellable() => DrainEventKind::Cancelled,
                Ok(summary) if summary.failed.is_empty() => match power.as_ref() {
                    Some(power) => {
                        handle.emit(DrainEventKind::PowerRequested {
                            action: power.action.clone(),
                        });
                        if let Err(error) = power.talos.power_action(&name, &power.action).await {
                            if !power.was_cordoned {
                                let _ = session.cordon(&name, false).await;
                            }
                            DrainEventKind::Error {
                                message: error.to_string(),
                            }
                        } else if power.action == "reboot" {
                            match session
                                .wait_for_node_reboot(
                                    &name,
                                    power.previous_boot_id.as_deref(),
                                    std::time::Duration::from_secs(300),
                                )
                                .await
                            {
                                Ok(()) => {
                                    handle.emit(DrainEventKind::NodeReady);
                                    if !power.was_cordoned {
                                        if let Err(error) = session.cordon(&name, false).await {
                                            return DrainEventKind::Error {
                                                message: error.to_string(),
                                            };
                                        }
                                    }
                                    DrainEventKind::Done { summary }
                                }
                                Err(error) => {
                                    if !power.was_cordoned {
                                        let _ = session.cordon(&name, false).await;
                                    }
                                    DrainEventKind::Error {
                                        message: error.to_string(),
                                    }
                                }
                            }
                        } else {
                            DrainEventKind::Done { summary }
                        }
                    }
                    None => DrainEventKind::Done { summary },
                },
                Ok(summary) => {
                    // Blocked or failed drain: mirror the old early-return —
                    // restore the pre-drain cordon state, do NOT power off.
                    if let Some(p) = power.as_ref() {
                        if !p.was_cordoned {
                            let _ = session.cordon(&name, false).await;
                        }
                    }
                    DrainEventKind::Done { summary }
                }
                Err(e) => {
                    // A cancellation accepted before this transition wins and
                    // deliberately leaves the node cordoned. Otherwise close
                    // cancellation before restoring state for the error.
                    if !handle.begin_non_cancellable() {
                        return DrainEventKind::Cancelled;
                    }
                    if let Some(p) = power.as_ref() {
                        if !p.was_cordoned {
                            let _ = session.cordon(&name, false).await;
                        }
                    }
                    DrainEventKind::Error {
                        message: e.to_string(),
                    }
                }
            }
        })
        .await;
    });
    Ok(id)
}

/// Run `fut` to completion, then emit exactly one terminal event on `handle`
/// and finish the job — guaranteed even if `fut` panics.
///
/// Without this, a panic anywhere in the drain (or the chained power phase,
/// or the concurrent event pump — all of which `fut` wraps) would unwind
/// straight out of the spawned task: tokio silently drops the task,
/// `handle.finish(kind)` never runs, the registry entry's broadcast sender never
/// drops, and every subscriber's `live_events` sits in `rx.recv().await`
/// forever — no terminal event, no `eof`, no expiry. `catch_unwind` restores
/// the "exactly one terminal event, always" invariant by converting a panic
/// into an `Error` event instead.
async fn finish_with(handle: &JobHandle, fut: impl Future<Output = DrainEventKind>) {
    let kind = match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(kind) => kind,
        Err(panic) => DrainEventKind::Error {
            message: panic_message(&panic),
        },
    };
    handle.finish(kind);
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
/// A lagged receiver (slow subscriber) recovers every event after its last
/// sequence from the registry buffer. Broadcast events already covered by
/// that recovery are then ignored by sequence number.
fn live_events(
    jobs: Arc<DrainJobs>,
    owner: String,
    id: u64,
    rx: broadcast::Receiver<DrainEvent>,
    last_seq: Option<u64>,
) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    futures::stream::unfold(
        (rx, false, last_seq, VecDeque::<DrainEvent>::new()),
        move |(mut rx, stop, mut last_seq, mut pending)| {
            let jobs = Arc::clone(&jobs);
            let owner = owner.clone();
            async move {
                if stop {
                    return None;
                }
                loop {
                    if let Some(ev) = pending.pop_front() {
                        last_seq = Some(ev.seq);
                        let terminal = is_terminal(&ev.kind);
                        return Some((sse_event(&ev), (rx, terminal, last_seq, pending)));
                    }
                    match rx.recv().await {
                        Ok(ev) if last_seq.is_some_and(|seq| ev.seq <= seq) => continue,
                        Ok(ev) => {
                            last_seq = Some(ev.seq);
                            let terminal = is_terminal(&ev.kind);
                            return Some((sse_event(&ev), (rx, terminal, last_seq, pending)));
                        }
                        Err(RecvError::Lagged(_)) => {
                            let recovered = jobs.replay_after(&owner, id, last_seq)?;
                            pending.extend(recovered);
                        }
                        Err(RecvError::Closed) => return None,
                    }
                }
            }
        },
    )
}

/// SSE stream of a drain job's events: replays the buffer, then (if the job
/// isn't finished yet) live events up to and including the terminal one,
/// then the `eof` named event (same convention as `logs`/`watch`).
///
/// Auth/backend gating matches the other SSE endpoints: this route lives in
/// `main.rs`'s session-gated `protected` router alongside `logs`/`watch`.
pub async fn drain_progress(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<DrainProgressQuery>,
) -> Response {
    let Some(caller) = super::request_caller(&state, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some((replay, rx, done)) = state.drain_jobs.subscribe(&caller.owner, q.id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let last_seq = replay.last().map(|event| event.seq);
    let replay_stream = tokio_stream::iter(replay).map(|e| sse_event(&e));
    let eof = tokio_stream::once(Ok::<_, Infallible>(
        SseEvent::default().event("eof").data("1"),
    ));
    if done {
        return Sse::new(replay_stream.chain(eof))
            .keep_alive(KeepAlive::default())
            .into_response();
    }
    Sse::new(
        replay_stream
            .chain(live_events(
                Arc::clone(&state.drain_jobs),
                caller.owner,
                q.id,
                rx,
                last_seq,
            ))
            .chain(eof),
    )
    .keep_alive(KeepAlive::default())
    .into_response()
}

/// Return the newest unfinished drain owned by this session so a refreshed
/// browser can reopen its progress window.
pub async fn active_drain(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(caller) = super::request_caller(&state, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    Json(state.drain_jobs.active(&caller.owner)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::handlers::fixtures::{
        fake_tokens, prod_state_without_provider, sealed_cookie_header,
    };

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
        let jobs = Arc::new(crate::server::drain_jobs::DrainJobs::default());
        let handle = jobs.create("alice".into(), "node-a".into()).unwrap();
        let (_, rx, _) = jobs.subscribe("alice", handle.id).unwrap();
        handle.emit(DrainEventKind::Cordoned);
        handle.finish(DrainEventKind::Done {
            summary: Default::default(),
        });
        // The registry sender stays alive for the whole test — if `live_events`
        // waited for the sender to close instead of stopping at the terminal
        // event, the timeout below would trip.
        let body = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            axum::body::to_bytes(
                Sse::new(live_events(
                    Arc::clone(&jobs),
                    "alice".into(),
                    handle.id,
                    rx,
                    None,
                ))
                .into_response()
                .into_body(),
                8192,
            ),
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
    /// whole (spawned, in production) task and `handle.finish(kind)` would never
    /// run, permanently hanging subscribers (see `live_events`).
    #[tokio::test]
    async fn finish_with_turns_a_panic_into_a_terminal_error_event() {
        let jobs = Arc::new(crate::server::drain_jobs::DrainJobs::default());
        let handle = jobs.create("alice".into(), "node-a".into()).unwrap();
        let id = handle.id;

        finish_with(&handle, async { panic!("boom") }).await;

        let (replay, _rx, done) = jobs.subscribe("alice", id).unwrap();
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
        let handle = jobs.create("alice".into(), "node-a".into()).unwrap();
        let id = handle.id;

        finish_with(&handle, async {
            DrainEventKind::Done {
                summary: Default::default(),
            }
        })
        .await;

        let (replay, _rx, done) = jobs.subscribe("alice", id).unwrap();
        assert!(done);
        assert!(matches!(
            replay.last().unwrap().kind,
            DrainEventKind::Done { .. }
        ));
    }

    #[tokio::test]
    async fn live_events_recovers_every_event_after_broadcast_lag() {
        let jobs = Arc::new(crate::server::drain_jobs::DrainJobs::default());
        let handle = jobs.create("alice".into(), "node-a".into()).unwrap();
        let (_, rx, _) = jobs.subscribe("alice", handle.id).unwrap();
        for _ in 0..300 {
            handle.emit(DrainEventKind::Cordoned);
        }
        handle.finish(DrainEventKind::Done {
            summary: Default::default(),
        });

        let body = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            axum::body::to_bytes(
                Sse::new(live_events(
                    Arc::clone(&jobs),
                    "alice".into(),
                    handle.id,
                    rx,
                    None,
                ))
                .into_response()
                .into_body(),
                1_000_000,
            ),
        )
        .await
        .expect("lag recovery should reach the terminal event")
        .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(text.matches("data:").count(), 301, "got: {text}");
        assert!(text.contains("\"seq\":0"), "got: {text}");
        assert!(text.contains("\"seq\":300"), "got: {text}");
    }

    #[tokio::test]
    async fn drain_progress_hides_foreign_jobs_and_allows_the_owner() {
        let state = prod_state_without_provider();
        let mut owner = fake_tokens();
        owner.identity.subject = "owner-a".into();
        let handle = state
            .drain_jobs
            .create("owner-a".into(), "node-a".into())
            .unwrap();
        let id = handle.id;
        handle.finish(DrainEventKind::Done {
            summary: Default::default(),
        });

        let mut foreign = fake_tokens();
        foreign.identity.subject = "owner-b".into();
        let response = drain_progress(
            State(state.clone()),
            sealed_cookie_header(&foreign),
            Query(DrainProgressQuery { id }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = drain_progress(
            State(state),
            sealed_cookie_header(&owner),
            Query(DrainProgressQuery { id }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("\"kind\":\"done\""), "got: {text}");
        assert!(text.contains("event: eof"), "got: {text}");
    }
}
