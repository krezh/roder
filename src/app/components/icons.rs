use leptos::prelude::*;

/// Shift key arrow icon (inline SVG, no external deps).
#[component]
pub(crate) fn ShiftIcon() -> impl IntoView {
    view! {
        <svg class="key-shift" viewBox="0 0 10 10" fill="currentColor" aria-hidden="true">
            <path d="M5 0L10 5H6.5V10H3.5V5H0Z" />
        </svg>
    }
}
