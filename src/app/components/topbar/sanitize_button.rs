//! Configurable pod and job sweep.

use leptos::prelude::*;

use crate::app::ui::sweep::{ask_sweep, run_sweep, SweepRequest};
use crate::app::ui::toast::Toast;

#[component]
pub(crate) fn SanitizeButton() -> impl IntoView {
    let sweep = expect_context::<RwSignal<Option<SweepRequest>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let toast = expect_context::<RwSignal<Option<Toast>>>();

    let do_sanitize = move |options: roder_core::SweepOptions| {
        run_sweep(toast, selected_ns.get_untracked(), options);
    };

    view! {
        <button class="sweep-btn" data-tip="Choose pods and jobs to delete"
            on:click=move |_| {
                ask_sweep(sweep, selected_ns.get_untracked(), do_sanitize);
            }>
            "Sweep"
        </button>
    }
}
