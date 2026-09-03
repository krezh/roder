use leptos::prelude::*;
use roder_core::DeletePropagation;

use crate::app::ui::{
    use_option_overlay, use_sweep_preview, Confirm, DeleteRequest, SweepOption, SweepPreview,
    SweepRequest,
};

#[component]
pub(crate) fn MobileConfirmDialog() -> impl IntoView {
    let signal = expect_context::<RwSignal<Option<Confirm>>>();
    let (snapshot, closing, close) = use_option_overlay(signal);
    view! { {move || snapshot.get().map(|request| view! {
        <div class="mobile-modal-scrim" class:closing=move || closing.get() on:click=move |_| close()></div>
        <section class="mobile-dialog" class:closing=move || closing.get() role="alertdialog" aria-modal="true">
            <p class="mobile-dialog-message">{request.message}</p>
            <div class="mobile-dialog-actions">
                <button type="button" on:click=move |_| close()>"Cancel"</button>
                {request.buttons.into_iter().map(|button| view! {
                    <button type="button" class="danger" disabled=move || closing.get() on:click=move |_| {
                        if !closing.get_untracked() { close(); (button.on_click)(); }
                    }>{button.label}</button>
                }).collect_view()}
            </div>
        </section>
    })} }
}

#[component]
pub(crate) fn MobileSweepDialog() -> impl IntoView {
    let signal = expect_context::<RwSignal<Option<SweepRequest>>>();
    let (snapshot, closing, close) = use_option_overlay(signal);
    view! { {move || snapshot.get().map(|request| {
        let options = RwSignal::new(roder_core::SweepOptions::default());
        let preview = use_sweep_preview(request.namespace.clone(), options);
        view! {
            <div class="mobile-modal-scrim" class:closing=move || closing.get() on:click=move |_| close()></div>
            <section class="mobile-dialog mobile-sweep-dialog" class:closing=move || closing.get() role="alertdialog" aria-modal="true">
                <p class="mobile-dialog-message">"Choose which resources to delete."</p>
                <SweepOption options field=|value| &mut value.terminal_pods label="Terminal pods" hint="Succeeded, failed, and evicted pods." />
                <SweepOption options field=|value| &mut value.stuck_pods label="Stuck pods" hint="Crash loops, image pull failures, unknown containers, and OOM kills." />
                <SweepOption options field=|value| &mut value.restarted_pods label="Pods with restarts" hint="Pods whose regular or init containers have restarted." />
                <SweepOption options field=|value| &mut value.completed_jobs label="Completed jobs" hint="Jobs with a successful Complete condition." />
                <SweepOption options field=|value| &mut value.failed_jobs label="Failed jobs" hint="Jobs with a Failed condition." />
                <SweepPreview preview />
                <div class="mobile-dialog-actions">
                    <button type="button" on:click=move |_| close()>"Cancel"</button>
                    <button type="button" class="danger"
                        disabled=move || closing.get() || !preview.with(|result| matches!(result, Some(Ok(summary)) if summary.pods + summary.jobs > 0))
                        on:click=move |_| {
                            if closing.get_untracked() { return; }
                            let action = request.on_confirm.clone();
                            let selected = options.get_untracked();
                            close();
                            action(selected);
                        }>"Sweep"</button>
                </div>
            </section>
        }
    })} }
}

#[component]
pub(crate) fn MobileDeleteDialog() -> impl IntoView {
    let signal = expect_context::<RwSignal<Option<DeleteRequest>>>();
    let (snapshot, closing, close) = use_option_overlay(signal);
    view! { {move || snapshot.get().map(|request| view! {
        <MobileDeleteRequest request closing close />
    })} }
}

#[component]
fn MobileDeleteRequest(
    request: DeleteRequest,
    closing: RwSignal<bool>,
    close: impl Fn() + Copy + Send + 'static,
) -> impl IntoView {
    let force = RwSignal::new(false);
    let propagation = RwSignal::new(String::new());
    view! {
        <div class="mobile-modal-scrim" class:closing=move || closing.get() on:click=move |_| close()></div>
        <section class="mobile-dialog mobile-delete-dialog" class:closing=move || closing.get() role="alertdialog" aria-modal="true">
            <p class="mobile-dialog-message">{request.message}</p>
            <label class="mobile-dialog-field">
                <span>"Propagation"</span>
                <select prop:value=move || propagation.get() on:change=move |event| propagation.set(event_target_value(&event))>
                    <option value="">"Default"</option><option value="Foreground">"Foreground"</option>
                    <option value="Background">"Background"</option><option value="Orphan">"Orphan"</option>
                </select>
            </label>
            <label class="mobile-dialog-check">
                <input type="checkbox" prop:checked=move || force.get() on:change=move |event| force.set(event_target_checked(&event)) />
                <span><strong>"Force"</strong><small>"Delete immediately, bypassing graceful pod termination."</small></span>
            </label>
            <div class="mobile-dialog-actions">
                <button type="button" on:click=move |_| close()>"Cancel"</button>
                <button type="button" class="danger" disabled=move || closing.get() on:click=move |_| {
                    if closing.get_untracked() { return; }
                    let propagation = match propagation.get_untracked().as_str() {
                        "Foreground" => Some(DeletePropagation::Foreground),
                        "Background" => Some(DeletePropagation::Background),
                        "Orphan" => Some(DeletePropagation::Orphan),
                        _ => None,
                    };
                    let action = request.on_confirm.clone();
                    close();
                    action(force.get_untracked(), propagation);
                }>"Delete"</button>
            </div>
        </section>
    }
}
