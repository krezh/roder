//! Browser history helpers. Gated to `wasm32`; on the (native) SSR build these
//! are no-ops, since the UI is only loaded after hydration in the browser.

use serde::{Deserialize, Serialize};

use crate::app::DetailTarget;

#[cfg(target_arch = "wasm32")]
const NAVIGATION_STATE_PREFIX: &str = "roder:";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NavigationState {
    pub(crate) kind: Option<String>,
    pub(crate) namespace: Option<String>,
    pub(crate) detail: Option<DetailTarget>,
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn navigation_state(value: &wasm_bindgen::JsValue) -> Option<NavigationState> {
    value
        .as_string()
        .and_then(|value| {
            value
                .strip_prefix(NAVIGATION_STATE_PREFIX)
                .map(str::to_owned)
        })
        .and_then(|value| serde_json::from_str(&value).ok())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn current_navigation_state() -> Option<NavigationState> {
    web_sys::window()
        .and_then(|window| window.history().ok())
        .and_then(|history| history.state().ok())
        .and_then(|state| navigation_state(&state))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn current_navigation_state() -> Option<NavigationState> {
    None
}

#[cfg(target_arch = "wasm32")]
fn encoded_navigation_state(state: &NavigationState) -> wasm_bindgen::JsValue {
    let json = serde_json::to_string(state).unwrap_or_default();
    wasm_bindgen::JsValue::from_str(&format!("{NAVIGATION_STATE_PREFIX}{json}"))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn replace_navigation_state(state: &NavigationState) {
    if let Some(history) = web_sys::window().and_then(|window| window.history().ok()) {
        let _ = history.replace_state(&encoded_navigation_state(state), "");
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn replace_navigation_state(_state: &NavigationState) {}

#[cfg(target_arch = "wasm32")]
pub(crate) fn push_navigation_state(state: &NavigationState) {
    if let Some(history) = web_sys::window().and_then(|window| window.history().ok()) {
        let _ = history.push_state(&encoded_navigation_state(state), "");
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn push_navigation_state(_state: &NavigationState) {}

/// Step the browser back one entry. Falls back to navigating to `/` if the
/// history is empty (e.g. the user landed on this page directly).
#[cfg(target_arch = "wasm32")]
pub(crate) fn history_back() {
    if let Some(window) = web_sys::window() {
        if let Ok(history) = window.history() {
            // If there's no previous entry (length == 1), back() does nothing.
            // Fall back to an explicit navigation so the user actually leaves.
            if history.length().unwrap_or(0) <= 1 {
                let _ = window.location().set_href("/");
            } else {
                let _ = history.back();
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub(crate) fn history_back() {
    // no-op on the server
}
