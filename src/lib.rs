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
    if let Some(window) = web_sys::window() {
        let registration = window
            .navigator()
            .service_worker()
            .register("/service-worker.js");
        wasm_bindgen_futures::spawn_local(async move {
            let _ = wasm_bindgen_futures::JsFuture::from(registration).await;
        });
    }
}
