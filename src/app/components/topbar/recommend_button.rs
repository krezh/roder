//! Topbar button for the resource recommendation panel — hidden entirely when
//! no Prometheus is configured.

use leptos::prelude::*;

use crate::app::state::{RecommendEnabled, RecommendOpen, RecommendScanning};

#[component]
pub(crate) fn RecommendButton() -> impl IntoView {
    let enabled = expect_context::<RecommendEnabled>().0;
    let open = expect_context::<RecommendOpen>().0;
    let scanning = expect_context::<RecommendScanning>().0;

    view! {
        <Show when=move || enabled.get()>
            <button
                class="sweep-btn"
                class:active=move || scanning.get()
                data-tip="Compare requests against usage history"
                on:click=move |_| open.set(true)
            >
                // Label stays fixed so the button keeps its width and the
                // controls beside it hold position; `active` carries the state.
                "Resources"
            </button>
        </Show>
    }
}
