//! Mobile cluster health controls, reusing the desktop badge components so
//! their watches and failure classification stay shared.

use leptos::prelude::*;

use crate::app::components::topbar::{AlertsButton, FailingBadge, FluxFailingBadge, TopUsage};
use crate::app::mobile::disclosure::TapReveal;

#[component]
pub(crate) fn MobileAlertActions() -> impl IntoView {
    view! {
        <div class="mobile-alert-actions">
            <FailingBadge />
            <TapReveal><FluxFailingBadge /></TapReveal>
            <AlertsButton />
        </div>
    }
}

#[component]
pub(crate) fn MobileStatusRow() -> impl IntoView {
    view! { <div class="mobile-status-row"><TopUsage /></div> }
}
