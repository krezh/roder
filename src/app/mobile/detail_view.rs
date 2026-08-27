//! Full-screen mobile replacement for the desktop's slide-in `DetailDrawer`.
//! Wraps the same `RowDetail` tab content (Info/YAML/Metrics/Logs, actions) —
//! only the chrome differs (full-screen with a back button vs. a resizable
//! side panel).

use leptos::prelude::*;

use crate::app::detail::RowDetail;
use crate::app::state::DetailTarget;

#[component]
pub(crate) fn MobileDetailView() -> impl IntoView {
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let (snapshot, closing, do_close) = crate::app::overlays::use_option_overlay(detail);

    view! {
        <div class="mobile-detail"
            class:open=move || snapshot.get().is_some()
            class:closing=move || closing.get()>
            <div class="mobile-detail-head">
                <button class="mobile-detail-back" aria-label="Back" on:click=move |_| do_close()>
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m15 18-6-6 6-6" /></svg>
                </button>
                <div class="mobile-detail-heading">
                    <span>"Resource"</span>
                    <strong class="mobile-detail-title">{move || snapshot.get().map(|t| t.name).unwrap_or_default()}</strong>
                </div>
            </div>
            <div class="mobile-detail-body">
                {move || snapshot.get().map(|t| view! { <RowDetail target=t on_delete=move || do_close() /> })}
            </div>
        </div>
    }
}
