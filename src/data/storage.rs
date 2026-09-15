//! Browser storage, and the small DOM queries the key dispatcher needs.

// ---- localStorage (persist UI state across reloads) -----------------------

#[cfg(target_arch = "wasm32")]
pub fn storage_get(key: &str) -> Option<String> {
    web_sys::window()?
        .local_storage()
        .ok()
        .flatten()?
        .get_item(key)
        .ok()
        .flatten()
}

#[cfg(target_arch = "wasm32")]
pub fn storage_set(key: &str, value: &str) {
    if let Some(Ok(Some(store))) = web_sys::window().map(|w| w.local_storage()) {
        let _ = store.set_item(key, value);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn storage_get(_key: &str) -> Option<String> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn storage_set(_key: &str, _value: &str) {}

#[cfg(target_arch = "wasm32")]
pub fn storage_remove(key: &str) {
    if let Some(Ok(Some(store))) = web_sys::window().map(|w| w.local_storage()) {
        let _ = store.remove_item(key);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn storage_remove(_key: &str) {}

// ---- sessionStorage (persist within a browser tab, cleared on tab close) ---

#[cfg(target_arch = "wasm32")]
pub fn session_storage_get(key: &str) -> Option<String> {
    web_sys::window()?
        .session_storage()
        .ok()
        .flatten()?
        .get_item(key)
        .ok()
        .flatten()
}

#[cfg(target_arch = "wasm32")]
pub fn session_storage_set(key: &str, value: &str) {
    if let Some(Ok(Some(store))) = web_sys::window().map(|w| w.session_storage()) {
        let _ = store.set_item(key, value);
    }
}

#[cfg(target_arch = "wasm32")]
pub fn session_storage_remove(key: &str) {
    if let Some(Ok(Some(store))) = web_sys::window().map(|w| w.session_storage()) {
        let _ = store.remove_item(key);
    }
}

/// Whether focus is currently in a text input (so shortcuts like ⌃Z don't hijack it).
#[cfg(target_arch = "wasm32")]
pub fn is_text_input_focused() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element())
        .map(|e| matches!(e.tag_name().to_uppercase().as_str(), "INPUT" | "TEXTAREA"))
        .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
pub fn has_text_selection() -> bool {
    web_sys::window()
        .and_then(|window| window.get_selection().ok().flatten())
        .is_some_and(|selection| !selection.is_collapsed())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn has_text_selection() -> bool {
    false
}

#[cfg(not(target_arch = "wasm32"))]
pub fn is_text_input_focused() -> bool {
    false
}
