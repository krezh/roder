//! Compact mobile top bar: hamburger nav toggle, brand, and the handful of
//! actions that need a visible tap target (search, namespace, alerts) instead
//! of the desktop's dense row of hover-driven controls.

use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::mobile::status::MobileStatusRow;
use crate::app::state::{ConnectionState, Connectivity, NavOpen, NsPaletteOpen, PaletteOpen};

#[component]
pub(crate) fn MobileHeader() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let palette_open = expect_context::<PaletteOpen>().0;
    let ns_palette_open = expect_context::<NsPaletteOpen>().0;
    let connection = expect_context::<ConnectionState>().0;

    view! {
        <header class="mobile-topbar">
            <button class="mobile-hamburger" on:click=move |_| nav_open.update(|o| *o = !*o)>"☰"</button>
            <span class="mobile-brand"
                class:mobile-brand-connected=move || matches!(connection.get(), Connectivity::Connected)
                class:mobile-brand-checking=move || matches!(connection.get(), Connectivity::Checking)
                class:mobile-brand-disconnected=move || matches!(connection.get(), Connectivity::Offline | Connectivity::Error(_))
                aria-label=move || connection.get().message().to_string()
                on:click=move |_| selected_kind.set(None)>
                "Roder"
            </span>
            <button class="mobile-icon-btn" aria-label="Search" on:click=move |_| palette_open.set(true)>"🔍"</button>
            <button class="mobile-ns-chip" on:click=move |_| ns_palette_open.set(true)>
                {move || selected_ns.get().unwrap_or_else(|| "All namespaces".to_string())}
            </button>
        </header>
        <MobileStatusRow />
    }
}
