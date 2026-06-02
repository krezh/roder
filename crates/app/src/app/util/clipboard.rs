//! Clipboard write, gated to the browser build.

#[cfg(target_arch = "wasm32")]
pub(crate) fn copy_to_clipboard(text: &str) {
    use leptos::prelude::window;
    let _ = window().navigator().clipboard().write_text(text);
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn copy_to_clipboard(_text: &str) {}
