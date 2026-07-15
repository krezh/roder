//! Topbar brand mark — doubles as a cluster-connectivity indicator and a
//! "clear selection" button.

use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::state::{ConnectionState, Connectivity};

#[component]
pub(crate) fn Brand() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let connection = expect_context::<ConnectionState>().0;
    view! {
        <span class="brand tip-anchor"
              class:brand-connected=move || matches!(connection.get(), Connectivity::Connected)
              class:brand-checking=move || matches!(connection.get(), Connectivity::Checking)
              class:brand-disconnected=move || matches!(connection.get(), Connectivity::Offline | Connectivity::Error(_))
              aria-label=move || connection.get().message().to_string()
              on:click=move |_| selected_kind.set(None)>
            <span class="brand-letter" style="--i:0">"R"</span>
            <span class="brand-letter" style="--i:1">"o"</span>
            <span class="brand-letter" style="--i:2">"d"</span>
            <span class="brand-letter" style="--i:3">"e"</span>
            <span class="brand-letter" style="--i:4">"r"</span>
            <span class="brand-tip tooltip">
                {move || connection.get().message().to_string()}
            </span>
        </span>
    }
}
