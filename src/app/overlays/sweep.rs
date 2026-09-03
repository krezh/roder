use leptos::prelude::*;
use roder_core::SweepOptions;

use crate::app::ui::{use_sweep_preview, SweepOption, SweepPreview, SweepRequest};

#[component]
pub(crate) fn SweepDialog() -> impl IntoView {
    let sweep = expect_context::<RwSignal<Option<SweepRequest>>>();
    let (snapshot, closing, close) = super::use_option_overlay(sweep);

    view! {
        {move || snapshot.get().map(|request| view! {
            <SweepDialogView request closing close />
        })}
    }
}

#[component]
fn SweepDialogView(
    request: SweepRequest,
    closing: RwSignal<bool>,
    close: impl Fn() + Copy + Send + 'static,
) -> impl IntoView {
    let options = RwSignal::new(SweepOptions::default());
    let preview = use_sweep_preview(request.namespace.clone(), options);
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    crate::app::ui::use_dialog_focus(dialog_ref);

    view! {
        <div class="modal-scrim" class:closing=move || closing.get() on:click=move |_| close()></div>
        <div class="modal sweep-modal" class:closing=move || closing.get() node_ref=dialog_ref
            role="alertdialog" aria-modal="true" tabindex="-1">
            <div class="modal-msg">"Choose which resources to delete."</div>
            <div class="sweep-options">
                <SweepOption options field=|value| &mut value.terminal_pods label="Terminal pods"
                    hint="Succeeded, failed, and evicted pods." />
                <SweepOption options field=|value| &mut value.stuck_pods label="Stuck pods"
                    hint="Crash loops, image pull failures, unknown containers, and OOM kills." />
                <SweepOption options field=|value| &mut value.restarted_pods label="Pods with restarts"
                    hint="Pods whose regular or init containers have restarted." />
                <SweepOption options field=|value| &mut value.completed_jobs label="Completed jobs"
                    hint="Jobs with a successful Complete condition." />
                <SweepOption options field=|value| &mut value.failed_jobs label="Failed jobs"
                    hint="Jobs with a Failed condition." />
            </div>
            <SweepPreview preview />
            <div class="modal-actions">
                <button class="act" on:click=move |_| close()>"Cancel"</button>
                <button class="act danger"
                    disabled=move || closing.get() || !preview.with(|result| matches!(result, Some(Ok(summary)) if summary.pods + summary.jobs > 0))
                    on:click=move |_| {
                        if closing.get_untracked() { return; }
                        let action = request.on_confirm.clone();
                        let selected = options.get_untracked();
                        close();
                        action(selected);
                    }>"Sweep"</button>
            </div>
        </div>
    }
}
