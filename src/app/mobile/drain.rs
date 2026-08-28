use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use roder_core::ActiveDrainJob;
use roder_core::{DrainJobRef, DrainOptions};

use crate::app::controllers::drain::{
    cancel, option_label, parse_options, progress_percent, start, subscribe, DrainPhase,
    DrainProgressState,
};
use crate::app::state::{DrainOpen, DrainTarget};
use crate::app::ui::{show_toast_detail, Toast, ToastKind};

#[component]
pub(crate) fn MobileDrainOverlay() -> impl IntoView {
    let open = expect_context::<DrainOpen>().0;
    let (snapshot, closing, close) = crate::app::ui::use_option_overlay(open);
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            if let Ok(Some(active)) =
                crate::data::fetch_json::<Option<ActiveDrainJob>>("/api/drain-active").await
            {
                if open.get_untracked().is_none() {
                    open.set(Some(DrainTarget {
                        key: active.key,
                        name: active.name,
                        power: active.power,
                        control_plane: false,
                        job: Some(DrainJobRef {
                            job: active.job,
                            executor: active.executor,
                        }),
                    }));
                }
            }
        });
    });
    view! { {move || snapshot.get().map(|target| view! { <MobileDrainDialog target closing close /> })} }
}

#[component]
fn MobileDrainDialog(
    target: DrainTarget,
    closing: RwSignal<bool>,
    close: impl Fn() + Copy + Send + 'static,
) -> impl IntoView {
    let phase = RwSignal::new(
        target
            .job
            .clone()
            .map_or(DrainPhase::Options, DrainPhase::Running),
    );
    let force = RwSignal::new(false);
    let delete_emptydir = RwSignal::new(false);
    let ignore_daemonsets = RwSignal::new(true);
    let disable_eviction = RwSignal::new(false);
    let grace = RwSignal::new(String::new());
    let timeout = RwSignal::new(DrainOptions::default().timeout_secs.to_string());
    let pending = RwSignal::new(false);
    let target_value = StoredValue::new(target.clone());
    let title = match target.power.as_deref() {
        Some("reboot") => format!("Drain & reboot {}", target.name),
        Some("shutdown") => format!("Drain & shut down {}", target.name),
        _ => format!("Drain {}", target.name),
    };
    let action_label = match target.power.as_deref() {
        Some("reboot") => "Drain & Reboot",
        Some("shutdown") => "Drain & Shutdown",
        _ => "Drain",
    };
    let submit = move |_| {
        if pending.get_untracked() {
            return;
        }
        pending.set(true);
        let options = current_options(
            force,
            delete_emptydir,
            ignore_daemonsets,
            disable_eviction,
            grace,
            timeout,
        );
        leptos::task::spawn_local(async move {
            match start(&target_value.get_value(), &options).await {
                Ok(job) => phase.set(DrainPhase::Running(job)),
                Err(error) => show_toast_detail(
                    expect_context::<RwSignal<Option<Toast>>>(),
                    "Drain failed",
                    Some(error),
                    ToastKind::Err,
                ),
            }
            pending.set(false);
        });
    };
    view! {
        <div class="mobile-modal-scrim" class:closing=move || closing.get() on:click=move |_| if !pending.get_untracked() { close() }></div>
        <section class="mobile-dialog mobile-drain-dialog" class:closing=move || closing.get() role="dialog" aria-modal="true">
            {move || match phase.get() {
                DrainPhase::Options => view! {
                    <header class="mobile-dialog-head"><strong>{title.clone()}</strong><button disabled=move || pending.get() on:click=move |_| close()>"x"</button></header>
                    {target.control_plane.then(|| view! { <div class="drain-warning">"This is a control-plane node; verify etcd quorum before continuing."</div> })}
                    <div class="mobile-drain-options">
                        <DrainToggle signal=force label="Force" hint="Evict pods not managed by a controller." />
                        <DrainToggle signal=delete_emptydir label="Delete emptyDir data" hint="Evict pods using emptyDir volumes." />
                        <DrainToggle signal=ignore_daemonsets label="Ignore DaemonSets" hint="Leave DaemonSet pods in place." />
                        <DrainToggle signal=disable_eviction label="Disable eviction" hint="Bypass PodDisruptionBudgets." />
                        <label class="mobile-dialog-field"><span>"Grace period"</span><input type="number" min="0" max="86400" placeholder="pod default" prop:value=move || grace.get() on:input=move |event| grace.set(event_target_value(&event)) /></label>
                        <label class="mobile-dialog-field"><span>"Timeout (0 = none)"</span><input type="number" min="0" max="3600" prop:value=move || timeout.get() on:input=move |event| timeout.set(event_target_value(&event)) /></label>
                    </div>
                    <footer class="mobile-dialog-actions"><button disabled=move || pending.get() on:click=move |_| close()>"Cancel"</button><button class="danger" disabled=move || pending.get() on:click=submit>{action_label}</button></footer>
                }.into_any(),
                DrainPhase::Running(job) => view! { <MobileDrainProgress job target=target_value phase force delete_emptydir ignore_daemonsets disable_eviction grace timeout pending close /> }.into_any(),
            }}
        </section>
    }
}

#[component]
fn DrainToggle(signal: RwSignal<bool>, label: &'static str, hint: &'static str) -> impl IntoView {
    view! { <label class="mobile-dialog-check"><input type="checkbox" prop:checked=move || signal.get() on:change=move |event| signal.set(event_target_checked(&event)) /><span><strong>{label}</strong><small>{hint}</small></span></label> }
}

#[component]
#[allow(clippy::too_many_arguments)]
fn MobileDrainProgress(
    job: DrainJobRef,
    target: StoredValue<DrainTarget>,
    phase: RwSignal<DrainPhase>,
    force: RwSignal<bool>,
    delete_emptydir: RwSignal<bool>,
    ignore_daemonsets: RwSignal<bool>,
    disable_eviction: RwSignal<bool>,
    grace: RwSignal<String>,
    timeout: RwSignal<String>,
    pending: RwSignal<bool>,
    close: impl Fn() + Copy + Send + 'static,
) -> impl IntoView {
    let job_value = StoredValue::new(job);
    let progress = RwSignal::new(DrainProgressState::default());
    let cancel_pending = RwSignal::new(false);
    let retry_choices = RwSignal::new(Vec::<(String, RwSignal<bool>)>::new());
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    Effect::new(move |_previous: Option<Option<crate::data::SseHandle>>| {
        subscribe(&job_value.get_value(), move |event| {
            progress.update(|state| state.apply(event.kind));
            let options = progress.get_untracked().retry_options;
            if !options.is_empty() {
                retry_choices.set(
                    options
                        .into_iter()
                        .map(|option| (option, RwSignal::new(true)))
                        .collect(),
                );
            }
        })
    });
    let retry = move |_| {
        if pending.get_untracked() {
            return;
        }
        for (option, enabled) in retry_choices.get_untracked() {
            match option.as_str() {
                "force" => force.set(enabled.get_untracked()),
                "delete_emptydir_data" => delete_emptydir.set(enabled.get_untracked()),
                "ignore_daemonsets" => ignore_daemonsets.set(enabled.get_untracked()),
                _ => {}
            }
        }
        pending.set(true);
        let options = current_options(
            force,
            delete_emptydir,
            ignore_daemonsets,
            disable_eviction,
            grace,
            timeout,
        );
        leptos::task::spawn_local(async move {
            match start(&target.get_value(), &options).await {
                Ok(job) => phase.set(DrainPhase::Running(job)),
                Err(error) => {
                    show_toast_detail(toast, "Drain retry failed", Some(error), ToastKind::Err)
                }
            }
            pending.set(false);
        });
    };
    let cancel_drain = move |_| {
        if cancel_pending.get_untracked() || progress.get_untracked().power_requested {
            return;
        }
        cancel_pending.set(true);
        leptos::task::spawn_local(async move {
            if let Err(error) = cancel(&job_value.get_value()).await {
                show_toast_detail(toast, "Cancel failed", Some(error), ToastKind::Err);
                cancel_pending.set(false);
            }
        });
    };
    view! {
        <header class="mobile-dialog-head"><strong>{format!("Draining {}", target.get_value().name)}</strong><button on:click=move |_| close()>"x"</button></header>
        <div class="mobile-drain-progress"><div class="drain-bar"><div class="drain-bar-fill" style:width=move || format!("{}%", progress_percent(&progress.get(), target.get_value().power.as_deref()))></div></div>
            <div class="drain-log">
                <For each={move || progress.get().log.into_iter().enumerate().collect::<Vec<_>>()} key=|(index, _)| *index let:item>
                    <div class="drain-log-line">{item.1}</div>
                </For>
            </div>
            {move || (!retry_choices.get().is_empty()).then(|| view! {
                <div class="drain-blocked">
                    <div class="drain-warning">"Drain blocked. Choose how to proceed, then retry."</div>
                    <For each=move || retry_choices.get() key=|(option, _)| option.clone() let:item>
                        {{ let (option, enabled) = item; view! { <DrainToggle signal=enabled label=option_label(&option) hint="Enable for the retry." /> } }}
                    </For>
                    <button class="danger" disabled=move || pending.get() on:click=retry>"Retry"</button>
                </div>
            })}
        </div>
        <footer class="mobile-dialog-actions">{move || if progress.get().finished { view! { <button on:click=move |_| close()>"Close"</button> }.into_any() } else { view! { {move || (!progress.get().power_requested && retry_choices.get().is_empty()).then(|| view! { <button class="danger" disabled=move || cancel_pending.get() on:click=cancel_drain>"Cancel drain"</button> })}<button on:click=move |_| close()>"Hide"</button> }.into_any() }}</footer>
    }
}

fn current_options(
    force: RwSignal<bool>,
    delete_emptydir: RwSignal<bool>,
    ignore_daemonsets: RwSignal<bool>,
    disable_eviction: RwSignal<bool>,
    grace: RwSignal<String>,
    timeout: RwSignal<String>,
) -> DrainOptions {
    parse_options(
        force.get_untracked(),
        delete_emptydir.get_untracked(),
        ignore_daemonsets.get_untracked(),
        disable_eviction.get_untracked(),
        &grace.get_untracked(),
        &timeout.get_untracked(),
    )
}
