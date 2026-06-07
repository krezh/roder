//! Browser history helpers. Gated to `wasm32`; on the (native) SSR build these
//! are no-ops, since the UI is only loaded after hydration in the browser.

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
