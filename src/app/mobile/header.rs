//! Compact mobile top bar: hamburger nav toggle, brand, and the handful of
//! actions that need a visible tap target (search, namespace, alerts) instead
//! of the desktop's dense row of hover-driven controls.

use leptos::prelude::*;
use leptos_router::hooks::use_location;
use roder_core::ResourceKind;

use crate::app::mobile::status::{MobileAlertActions, MobileStatusRow};
use crate::app::state::{ConnectionState, Connectivity};

#[component]
pub(crate) fn MobileHeader() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let connection = expect_context::<ConnectionState>().0;
    let pathname = use_location().pathname;

    view! {
        <header class="mobile-topbar">
            <div class="mobile-title">
                <span class="mobile-brand"
                    class:mobile-brand-connected=move || matches!(connection.get(), Connectivity::Connected)
                    class:mobile-brand-checking=move || matches!(connection.get(), Connectivity::Checking)
                    class:mobile-brand-disconnected=move || matches!(connection.get(), Connectivity::Offline | Connectivity::Error(_))
                    aria-label=move || connection.get().message().to_string()>
                    "Roder"
                </span>
                <strong>{move || match pathname.get().as_str() {
                    "/workspace" => "Workspace".to_string(),
                    "/search" => "Search results".to_string(),
                    _ => selected_kind.get().map(|kind| kind.kind).unwrap_or_else(|| "Overview".to_string()),
                }}</strong>
            </div>
            <MobileAlertActions />
        </header>
        <MobileStatusRow />
    }
}
