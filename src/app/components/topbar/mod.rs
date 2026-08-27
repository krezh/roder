//! Top bar: hamburger, brand, connection dot, palette button, error filter,
//! namespace selector, failing-pod badge, cluster usage (with node health),
//! and identity. Each piece lives in its own file; this module only lays
//! them out.

mod alerts_button;
mod badge;
mod brand;
mod failing_badge;
mod failure_watch;
mod flux_failing_badge;
mod identity;
mod sanitize_button;
mod sync_button;
mod top_usage;

pub(crate) use alerts_button::AlertsButton;
pub(crate) use failing_badge::FailingBadge;
pub(crate) use failure_watch::use_failure_watch;
pub(crate) use flux_failing_badge::FluxFailingBadge;
pub(crate) use sanitize_button::SanitizeButton;
pub(crate) use sync_button::SyncButton;
pub(crate) use top_usage::TopUsage;

use brand::Brand;
use identity::Identity;
use leptos::prelude::*;

use crate::app::components::icons::ShiftIcon;
use crate::app::state::{NavOpen, NsPaletteOpen, OnlyProblems, PaletteOpen};

#[component]
pub(crate) fn Topbar() -> impl IntoView {
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let palette_open = expect_context::<PaletteOpen>().0;
    let ns_palette_open = expect_context::<NsPaletteOpen>().0;
    let only_problems = expect_context::<OnlyProblems>().0;

    view! {
        <header class="topbar">
            <button class="hamburger" on:click=move |_| nav_open.update(|o| *o = !*o)>"☰"</button>
            <Brand />
            <TopUsage />
            <div class="topbar-group topbar-nav">
                <button class="palette-btn" on:click=move |_| palette_open.set(true)>
                    "Search " <kbd><ShiftIcon />"K"</kbd>
                </button>
                <button class="errfilter" class:active=move || only_problems.get()
                    on:click=move |_| only_problems.update(|o| *o = !*o)>
                    "Errors " <kbd><ShiftIcon />"E"</kbd>
                </button>
                <button class="ns-palette-btn" class:scoped=move || selected_ns.get().is_some()
                    on:click=move |_| ns_palette_open.set(true)>
                    {move || selected_ns.get().unwrap_or_else(|| "All namespaces".to_string())}
                    " " <kbd><ShiftIcon />"N"</kbd>
                </button>
            </div>
            <div class="topbar-group topbar-actions">
                <SanitizeButton />
                <SyncButton />
            </div>
            <div class="topbar-group topbar-health">
                <AlertsButton />
                // Conditional badges stay last so appearing failures never move
                // the permanent health controls out from under the pointer.
                <FailingBadge />
                <FluxFailingBadge />
            </div>
            <div class="topbar-account"><Identity /></div>
        </header>
    }
}
