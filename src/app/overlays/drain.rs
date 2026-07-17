//! Drain overlay: options form (kubectl drain flags), then streamed progress.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use roder_core::{DrainBlocker, DrainEvent, DrainEventKind, DrainOptions};

use crate::app::overlays::toast::{show_toast, show_toast_detail, Toast, ToastKind};
use crate::app::state::{DrainOpen, DrainTarget};

#[derive(Clone, PartialEq)]
enum Phase {
    Options,
    Running { job: u64 },
}

/// Always mounted (alongside `ConfirmDialog`/`ExecWindow`); opening it is just
/// setting `DrainOpen`'s signal. The per-open form state lives in `DrainForm`
/// below, not here — `DrainForm` is created fresh every time `snapshot` picks
/// up a new target, so reopening always starts at `Phase::Options` with
/// default options (mirrors `ResourceTreeWindow`/`TreeContent`).
#[component]
pub(crate) fn DrainOverlay() -> impl IntoView {
    let open = expect_context::<DrainOpen>().0;
    let (snapshot, closing, do_close) = super::use_option_overlay(open);

    view! {
        {move || snapshot.get().map(|target| view! {
            <div class="modal-scrim" class:closing=move || closing.get()
                on:click=move |_| do_close()></div>
            <div class="modal drain-modal" class:closing=move || closing.get()>
                <DrainForm target=target do_close=do_close />
            </div>
        })}
    }
}

#[component]
fn DrainForm(target: DrainTarget, do_close: impl Fn() + Copy + Send + 'static) -> impl IntoView {
    // Captured here (not inside `spawn_local`, after the `.await`): every
    // other async handler in this codebase resolves its context signals at
    // component-body level and only carries the `Copy` handle across the
    // await, since `expect_context` isn't guaranteed to resolve once a task
    // has been polled back in after suspending.
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let phase = RwSignal::new(Phase::Options);
    let force = RwSignal::new(false);
    let delete_emptydir = RwSignal::new(false);
    let ignore_daemonsets = RwSignal::new(true);
    let disable_eviction = RwSignal::new(false);
    let grace = RwSignal::new(String::new()); // blank = pod default
    let timeout = RwSignal::new("60".to_string());
    let submitting = RwSignal::new(false);

    let name = target.name.clone();
    let running_name = name.clone();
    let (title, action_label, show_warning) = match target.power.as_deref() {
        Some("reboot") => (
            format!("Drain & reboot {name}"),
            "Drain & Reboot",
            target.control_plane,
        ),
        Some("shutdown") => (
            format!("Drain & shut down {name}"),
            "Drain & Shutdown",
            target.control_plane,
        ),
        _ => (format!("Drain {name}"), "Drain", false),
    };

    // `StoredValue` (unconditionally `Copy`) rather than holding `target`
    // itself in the `submit` closure below — `DrainTarget` isn't `Copy`, and
    // the closure is reused across every re-render of the reactive `Phase::Options`
    // arm further down.
    let target_sv = StoredValue::new(target);

    let submit = move |_: leptos::ev::MouseEvent| {
        if submitting.get_untracked() {
            return;
        }
        submitting.set(true);
        let options = options_from_signals(
            force,
            delete_emptydir,
            ignore_daemonsets,
            disable_eviction,
            grace,
            timeout,
        );
        let target = target_sv.get_value();
        leptos::task::spawn_local(async move {
            match start_drain(&target, &options).await {
                Ok(job) => phase.set(Phase::Running { job }),
                Err(e) => {
                    submitting.set(false);
                    show_toast_detail(
                        toast,
                        format!("Drain of {} failed", target.name),
                        Some(e),
                        ToastKind::Err,
                    );
                }
            }
        });
    };

    view! {
        {move || match phase.get() {
            Phase::Options => view! {
                <div class="modal-msg">{title.clone()}</div>
                {show_warning.then(|| view! {
                    <div class="drain-warning">
                        "This is a control-plane node; verify etcd quorum before continuing."
                    </div>
                })}
                <label class="drain-opt">
                    <input type="checkbox" prop:checked=move || force.get()
                        on:change=move |e| force.set(event_target_checked(&e)) />
                    <span>"Force"</span>
                    <span class="hint">"Evict pods not managed by a controller."</span>
                </label>
                <label class="drain-opt">
                    <input type="checkbox" prop:checked=move || delete_emptydir.get()
                        on:change=move |e| delete_emptydir.set(event_target_checked(&e)) />
                    <span>"Delete emptyDir data"</span>
                    <span class="hint">"Evict pods using emptyDir volumes."</span>
                </label>
                <label class="drain-opt">
                    <input type="checkbox" prop:checked=move || ignore_daemonsets.get()
                        on:change=move |e| ignore_daemonsets.set(event_target_checked(&e)) />
                    <span>"Ignore DaemonSets"</span>
                    <span class="hint">"Proceed while leaving DaemonSet pods in place."</span>
                </label>
                <label class="drain-opt">
                    <input type="checkbox" prop:checked=move || disable_eviction.get()
                        on:change=move |e| disable_eviction.set(event_target_checked(&e)) />
                    <span>"Disable eviction"</span>
                    <span class="hint">"Delete pods directly, bypassing PodDisruptionBudgets."</span>
                </label>
                <label class="drain-num">
                    <span>"Grace period (s)"</span>
                    <input type="number" min="0" placeholder="pod default"
                        prop:value=move || grace.get()
                        on:input=move |e| grace.set(event_target_value(&e)) />
                </label>
                <label class="drain-num">
                    <span>"Timeout (s)"</span>
                    <input type="number" min="1"
                        prop:value=move || timeout.get()
                        on:input=move |e| timeout.set(event_target_value(&e)) />
                </label>
                <div class="modal-actions">
                    <button class="act" on:click=move |_| do_close()>"Cancel"</button>
                    <button class="act danger" disabled=move || submitting.get() on:click=submit>
                        {action_label}
                    </button>
                </div>
            }.into_any(),
            // A fresh `DrainProgress` mounts per `job` — both the initial
            // entry into `Running` and every retry (which sets `phase` to a
            // new `Phase::Running { job }`) replace this whole subtree, so
            // there's no run state to reset by hand: it's simply never
            // reused across jobs.
            Phase::Running { job } => view! {
                <DrainProgress
                    job=job name=running_name.clone() target=target_sv phase=phase
                    force=force delete_emptydir=delete_emptydir ignore_daemonsets=ignore_daemonsets
                    disable_eviction=disable_eviction grace=grace timeout=timeout
                    toast=toast do_close=do_close
                />
            }.into_any(),
        }}
    }
}

/// The live progress view for one drain job: SSE log, determinate progress
/// bar, blocked-retry footer, and cancel/hide/close actions. Mounted fresh
/// per `job` (see the `Phase::Running` arm above), so every signal here is
/// this job's alone — a retry starting a new job gets a brand new instance
/// rather than a reset of this one.
#[component]
fn DrainProgress(
    job: u64,
    name: String,
    /// Same target the options form submitted against — retried with
    /// adjusted options via the same `start_drain` helper.
    target: StoredValue<DrainTarget>,
    phase: RwSignal<Phase>,
    force: RwSignal<bool>,
    delete_emptydir: RwSignal<bool>,
    ignore_daemonsets: RwSignal<bool>,
    disable_eviction: RwSignal<bool>,
    grace: RwSignal<String>,
    timeout: RwSignal<String>,
    toast: RwSignal<Option<Toast>>,
    do_close: impl Fn() + Copy + Send + 'static,
) -> impl IntoView {
    let heading = match target.get_value().power {
        Some(p) => format!("Draining {name} → {p}"),
        None => format!("Draining {name}"),
    };
    // `Copy` (via `StoredValue`) rather than a plain `String` from here on:
    // the retry/cancel/hide handlers below are referenced from inside
    // reactive `move || …` blocks that run repeatedly, so they themselves
    // need to be `Copy` closures — which requires every capture to be `Copy`.
    let name = StoredValue::new(name);

    let log: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let done_count = RwSignal::new(0usize);
    let total = RwSignal::new(0usize);
    let finished = RwSignal::new(false);
    // One entry per distinct `DrainBlocker::clearable_by` seen in the last
    // `Blocked` event, pre-checked; empty whenever the job isn't blocked.
    let retry_toggles: RwSignal<Vec<(String, RwSignal<bool>)>> = RwSignal::new(Vec::new());
    let last_seq = StoredValue::new(None::<u64>);
    let log_ref = NodeRef::<leptos::html::Div>::new();

    // Runs once on mount (nothing it reads is reactive — `job` is captured by
    // value). Disposed, closing the `EventSource`, exactly when this
    // component's subtree is torn down: on `Phase::Options` (dialog reopened
    // later — a fresh `DrainForm` — or the modal snapshot clearing after
    // close) or on a retry replacing this arm with a new `job`.
    Effect::new(move |_prev: Option<Option<crate::data::SseHandle>>| {
        crate::data::subscribe_lines(&format!("/api/drain-progress?id={job}"), move |line| {
            let Ok(ev) = serde_json::from_str::<DrainEvent>(&line) else {
                return;
            };
            // Replay (on reconnect) repeats every buffered event from seq 0;
            // `seq` increases monotonically, so anything not strictly past
            // the last one seen is a repeat.
            if last_seq.get_value().is_some_and(|s| ev.seq <= s) {
                return;
            }
            last_seq.set_value(Some(ev.seq));
            match ev.kind {
                DrainEventKind::Cordoned => log.update(|l| l.push("node cordoned".into())),
                DrainEventKind::Started { total: t } => {
                    total.set(t);
                    log.update(|l| l.push(format!("evicting {t} pods")));
                }
                DrainEventKind::Evicted { pod, done, .. } => {
                    done_count.set(done);
                    log.update(|l| l.push(format!("evicted {pod}")));
                }
                DrainEventKind::EvictFailed { pod, reason } => {
                    log.update(|l| l.push(format!("FAILED {pod}: {reason}")));
                }
                DrainEventKind::Blocked { blockers: b } => {
                    for bl in &b {
                        log.update(|l| l.push(format!("blocked by {}: {}", bl.pod, bl.reason)));
                    }
                    retry_toggles.set(
                        distinct_clearable_by(&b)
                            .into_iter()
                            .map(|k| (k, RwSignal::new(true)))
                            .collect(),
                    );
                    finished.set(true);
                }
                DrainEventKind::WaitingTermination { remaining } => {
                    log.update(|l| l.push(format!("waiting for {remaining} pod(s) to terminate")));
                }
                DrainEventKind::PowerRequested { action } => {
                    log.update(|l| l.push(format!("{action} requested")));
                }
                DrainEventKind::NodeReady => log.update(|l| l.push("node returned Ready".into())),
                DrainEventKind::Done { summary } => {
                    log.update(|l| {
                        l.push(format!(
                            "done: evicted {}, skipped {}, {} failed",
                            summary.evicted,
                            summary.skipped,
                            summary.failed.len()
                        ));
                    });
                    // Skipped pods (already terminal, already gone) never
                    // get their own `Evicted` tick, so `done_count` can sit
                    // below `total` forever even on a clean finish — snap the
                    // bar to full then. Not when a `Blocked` preceded this
                    // `Done` (the retry footer's already showing the run
                    // stopped short; a full bar next to it would mislead).
                    if retry_toggles.get_untracked().is_empty() {
                        done_count.set(total.get_untracked());
                    }
                    finished.set(true);
                }
                DrainEventKind::Error { message } => {
                    log.update(|l| l.push(format!("ERROR: {message}")));
                    finished.set(true);
                }
                DrainEventKind::Cancelled => {
                    log.update(|l| l.push("cancelled — node remains cordoned".into()));
                    finished.set(true);
                }
            }
        })
    });

    // Auto-follow the log to its newest line, deferred to
    // `request_animation_frame` for the same reason as `LogsView`
    // (src/app/logs/mod.rs): the `<For>`'s own DOM-insert effect runs after
    // this one, so a synchronous `scroll_top = scroll_height` would read the
    // pre-insert height and leave the newest line just outside the viewport.
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        log.track();
        if let Some(el) = log_ref.get_untracked() {
            let el2 = el.clone();
            request_animation_frame(move || {
                el2.set_scroll_top(el2.scroll_height());
            });
        }
    });

    let retry = move |_: leptos::ev::MouseEvent| {
        for (key, checked) in retry_toggles.get_untracked() {
            let val = checked.get_untracked();
            match key.as_str() {
                "force" => force.set(val),
                "delete_emptydir_data" => delete_emptydir.set(val),
                "ignore_daemonsets" => ignore_daemonsets.set(val),
                _ => {}
            }
        }
        let options = options_from_signals(
            force,
            delete_emptydir,
            ignore_daemonsets,
            disable_eviction,
            grace,
            timeout,
        );
        let t = target.get_value();
        // Resolved before the `await` below, not after — matches `DrainForm`'s
        // `submit` handler above (see its comment on why context/store reads
        // are captured at handler-invocation time, not after suspending).
        let retry_name = name.get_value();
        leptos::task::spawn_local(async move {
            match start_drain(&t, &options).await {
                Ok(new_job) => phase.set(Phase::Running { job: new_job }),
                Err(e) => show_toast_detail(
                    toast,
                    format!("Retry of {retry_name} failed"),
                    Some(e),
                    ToastKind::Err,
                ),
            }
        });
    };

    let cancel = move |_: leptos::ev::MouseEvent| {
        let cancel_name = name.get_value();
        leptos::task::spawn_local(async move {
            let payload = serde_json::json!({"action": "drain-cancel", "job": job});
            if let Err(e) = crate::data::post_action(&payload).await {
                show_toast_detail(
                    toast,
                    format!("Cancel of {cancel_name} failed"),
                    Some(e),
                    ToastKind::Err,
                );
            }
        });
        // Deliberately no local state change here: the window stays open and
        // `Cancelled` (or whatever the in-flight step ends with) arrives over
        // the same SSE stream like any other terminal event.
    };

    let hide =
        move |_: leptos::ev::MouseEvent| detach_and_close(job, name.get_value(), toast, do_close);

    view! {
        <div class="modal-msg">{heading}</div>
        <div class="drain-bar"><div class="drain-bar-fill"
            style:width=move || {
                let t = total.get().max(1);
                format!("{}%", done_count.get() * 100 / t)
            }></div></div>
        <div class="drain-log" node_ref=log_ref>
            <For each={move || log.get().into_iter().enumerate().collect::<Vec<_>>()} key=|(i, _)| *i let:item>
                <div class="drain-log-line">{item.1}</div>
            </For>
        </div>
        {move || (!retry_toggles.get().is_empty()).then(|| view! {
            <div class="drain-blocked">
                <div class="drain-warning">"Drain blocked — choose how to proceed, then retry."</div>
                <For each=move || retry_toggles.get() key=|(k, _)| k.clone() let:item>
                    {
                        let (key, checked) = item;
                        let label = clearable_by_label(&key);
                        view! {
                            <label class="drain-opt">
                                <input type="checkbox" prop:checked=move || checked.get()
                                    on:change=move |e| checked.set(event_target_checked(&e)) />
                                <span>{label}</span>
                            </label>
                        }
                    }
                </For>
                <div class="modal-actions">
                    <button class="act danger" on:click=retry>"Retry"</button>
                </div>
            </div>
        })}
        <div class="modal-actions">
            {move || if finished.get() {
                view! { <button class="act" on:click=move |_| do_close()>"Close"</button> }.into_any()
            } else {
                view! {
                    <button class="act danger" on:click=cancel>"Cancel drain"</button>
                    <button class="act" on:click=hide>"Hide"</button>
                }.into_any()
            }}
        </div>
    }
}

/// Distinct `DrainBlocker::clearable_by` values, in first-seen order (stable
/// regardless of how many pods share a reason) — one retry toggle per value,
/// not per blocked pod.
fn distinct_clearable_by(blockers: &[DrainBlocker]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    blockers
        .iter()
        .map(|b| b.clearable_by.clone())
        .filter(|k| seen.insert(k.clone()))
        .collect()
}

/// Human label for a `DrainBlocker::clearable_by` value / `DrainOptions` flag name.
fn clearable_by_label(key: &str) -> &'static str {
    match key {
        "force" => "Force",
        "delete_emptydir_data" => "Delete emptyDir data",
        "ignore_daemonsets" => "Ignore DaemonSets",
        _ => "Unknown option",
    }
}

/// Closing mid-run: drop the window's subscription but keep watching the job
/// in the background just long enough to toast the final result.
fn detach_and_close(job: u64, name: String, toast: RwSignal<Option<Toast>>, do_close: impl Fn()) {
    do_close();
    let sse_bg: Rc<RefCell<Option<crate::data::SseHandle>>> = Rc::new(RefCell::new(None));
    let sse_bg2 = Rc::clone(&sse_bg);
    let handle =
        crate::data::subscribe_lines(&format!("/api/drain-progress?id={job}"), move |line| {
            let Ok(ev) = serde_json::from_str::<DrainEvent>(&line) else {
                return;
            };
            let msg = match ev.kind {
                DrainEventKind::Done { summary } if summary.failed.is_empty() => Some((
                    format!(
                        "Drained {name}: evicted {}, skipped {}",
                        summary.evicted, summary.skipped
                    ),
                    ToastKind::Ok,
                )),
                DrainEventKind::Done { summary } => Some((
                    format!(
                        "Drain of {name} left {} pod(s) failed",
                        summary.failed.len()
                    ),
                    ToastKind::Err,
                )),
                DrainEventKind::Blocked { .. } => {
                    Some((format!("Drain of {name} blocked"), ToastKind::Err))
                }
                DrainEventKind::Error { message } => {
                    Some((format!("Drain of {name} failed: {message}"), ToastKind::Err))
                }
                DrainEventKind::Cancelled => {
                    Some((format!("Drain of {name} cancelled"), ToastKind::Err))
                }
                _ => None,
            };
            if let Some((text, kind)) = msg {
                show_toast(toast, text, kind);
                sse_bg2.borrow_mut().take(); // drop handle → closes the stream
            }
        });
    *sse_bg.borrow_mut() = handle;
}

/// Parse the grace-period/timeout text inputs into `DrainOptions`, falling
/// back to safe defaults on anything unparsable rather than panicking:
/// a blank or invalid grace period means "pod default" (`None`), and an
/// invalid or zero timeout falls back to the server's own default (60s).
fn options_from_signals(
    force: RwSignal<bool>,
    delete_emptydir: RwSignal<bool>,
    ignore_daemonsets: RwSignal<bool>,
    disable_eviction: RwSignal<bool>,
    grace: RwSignal<String>,
    timeout: RwSignal<String>,
) -> DrainOptions {
    let grace_period = grace.get_untracked().trim().parse::<u32>().ok();
    let timeout_secs = timeout
        .get_untracked()
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|&secs| secs > 0)
        .unwrap_or(60);
    DrainOptions {
        force: force.get_untracked(),
        delete_emptydir_data: delete_emptydir.get_untracked(),
        ignore_daemonsets: ignore_daemonsets.get_untracked(),
        disable_eviction: disable_eviction.get_untracked(),
        grace_period,
        timeout_secs,
    }
}

/// POST the drain (or drain-first power) action; resolve to the job id.
async fn start_drain(target: &DrainTarget, options: &DrainOptions) -> Result<u64, String> {
    let payload = match &target.power {
        None => serde_json::json!({
            "action": "drain", "key": target.key, "name": target.name, "options": options,
        }),
        Some(p) => serde_json::json!({
            "action": format!("talos-{p}"), "name": target.name, "drain": true, "options": options,
        }),
    };
    let body = crate::data::post_action(&payload).await?;
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("job")?.as_u64())
        .ok_or_else(|| format!("unexpected response: {body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocker(pod: &str, clearable_by: &str) -> DrainBlocker {
        DrainBlocker {
            pod: pod.to_string(),
            reason: "reason".to_string(),
            clearable_by: clearable_by.to_string(),
        }
    }

    #[test]
    fn distinct_clearable_by_dedupes_preserving_first_seen_order() {
        let blockers = vec![
            blocker("a", "force"),
            blocker("b", "ignore_daemonsets"),
            blocker("c", "force"),
            blocker("d", "delete_emptydir_data"),
        ];
        assert_eq!(
            distinct_clearable_by(&blockers),
            vec!["force", "ignore_daemonsets", "delete_emptydir_data"]
        );
    }

    #[test]
    fn distinct_clearable_by_empty_for_no_blockers() {
        assert!(distinct_clearable_by(&[]).is_empty());
    }

    #[test]
    fn clearable_by_label_covers_known_options() {
        assert_eq!(clearable_by_label("force"), "Force");
        assert_eq!(
            clearable_by_label("delete_emptydir_data"),
            "Delete emptyDir data"
        );
        assert_eq!(clearable_by_label("ignore_daemonsets"), "Ignore DaemonSets");
        assert_eq!(clearable_by_label("something_else"), "Unknown option");
    }
}
