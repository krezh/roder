//! Drain overlay: options form (kubectl drain flags), then streamed progress.

use leptos::prelude::*;
use roder_core::DrainOptions;

use crate::app::overlays::toast::{show_toast_detail, Toast, ToastKind};
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
            // Task 7 replaces this with live progress (SSE-streamed events,
            // cancel). For now it just proves the Options -> Running
            // transition lands correctly and blocks re-submission.
            Phase::Running { job: _job } => view! {
                <div class="modal-msg">{format!("Draining {running_name}…")}</div>
                <div class="modal-actions">
                    <button class="act" disabled=true>"Working…"</button>
                </div>
            }.into_any(),
        }}
    }
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
