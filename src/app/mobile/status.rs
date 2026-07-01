//! Compact status row for the mobile header: reuses the desktop's badge
//! components as-is (`topbar.rs`, made `pub(crate)`) so the SSE-subscription
//! logic behind "what counts as failing" lives in exactly one place, each
//! wrapped in `TapReveal` so its hover-only breakdown tooltip becomes
//! tap-to-reveal on touch.

use leptos::prelude::*;

use crate::app::components::topbar::{AlertsButton, FailingBadge, FluxFailingBadge, TopUsage};
use crate::app::mobile::disclosure::TapReveal;

#[component]
pub(crate) fn MobileStatusRow() -> impl IntoView {
    view! {
        <div class="mobile-status-row">
            <FailingBadge />
            <TapReveal><FluxFailingBadge /></TapReveal>
            <TapReveal><TopUsage /></TapReveal>
            <TapReveal><AlertsButton /></TapReveal>
        </div>
    }
}
