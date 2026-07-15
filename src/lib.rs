#![recursion_limit = "512"]

pub mod app;
pub mod data;
pub mod version;

#[cfg(feature = "ssr")]
pub mod server;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::App;
    console_error_panic_hook::set_once();
    version::init_baseline();
    leptos::mount::hydrate_body(App);
}
