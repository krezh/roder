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

    view! {
        <div class="mobile-detail" class:open=move || detail.get().is_some()>
            <div class="mobile-detail-head">
                <button class="mobile-detail-back" on:click=move |_| detail.set(None)>"‹ Back"</button>
                <span class="mobile-detail-title">{move || detail.get().map(|t| t.name).unwrap_or_default()}</span>
            </div>
            <div class="mobile-detail-body">
                {move || detail.get().map(|t| view! { <RowDetail target=t on_delete=move || detail.set(None) /> })}
            </div>
        </div>
    }
}
