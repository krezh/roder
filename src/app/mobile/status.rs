//! Compact status row for the mobile header: reuses the desktop's badge
//! components as-is (`topbar.rs`, made `pub(crate)`) so the SSE-subscription
//! logic behind "what counts as failing" lives in exactly one place.
//! `FluxFailingBadge` stays wrapped in `TapReveal` so its hover-only tooltip
//! becomes tap-to-reveal on touch. `TopUsage` and `AlertsButton` are NOT
//! wrapped: both already have their own tap-to-navigate `on:click` (view
//! nodes / open the alerts panel), and `TapReveal`'s own click handler was
//! firing alongside it on every tap — the click bubbles from the inner
//! button to the wrapping div — so a single tap did two things at once
//! (navigated *and* force-opened the breakdown tooltip).

use leptos::prelude::*;

use crate::app::components::topbar::{AlertsButton, FailingBadge, FluxFailingBadge, TopUsage};
use crate::app::mobile::disclosure::TapReveal;

#[component]
pub(crate) fn MobileStatusRow() -> impl IntoView {
    view! {
        <div class="mobile-status-row">
            <FailingBadge />
            <TapReveal><FluxFailingBadge /></TapReveal>
            <TopUsage />
            <AlertsButton />
        </div>
    }
}
