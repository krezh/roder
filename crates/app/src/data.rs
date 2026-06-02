//! Client-side data access: REST fetch + SSE subscription. All browser APIs are
//! gated to `wasm32`; on the (native) SSR build these are no-ops, since the UI
//! loads its data after hydration in the browser.

use serde::de::DeserializeOwned;

/// Build the SSE URL for a live resource list. An optional label `selector`
/// (e.g. a workload's `spec.selector`) narrows the watch to matching objects.
pub fn watch_url(key: &str, namespace: Option<&str>, selector: Option<&str>) -> String {
    let mut url = format!(
        "/api/watch?key={}&namespace={}",
        key,
        namespace.unwrap_or("")
    );
    if let Some(sel) = selector.filter(|s| !s.is_empty()) {
        url.push_str("&selector=");
        url.push_str(&percent_encode(sel));
    }
    url
}

/// Minimal percent-encoding for a query value (label selectors contain `=`, `,`).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the detail URL for a single object.
pub fn detail_url(key: &str, namespace: Option<&str>, name: &str) -> String {
    format!(
        "/api/detail?key={}&namespace={}&name={}",
        key,
        namespace.unwrap_or(""),
        name
    )
}

// ---- REST fetch -----------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub async fn fetch_json<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    use gloo_net::http::Request;
    let resp = Request::get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("{} {}", resp.status(), resp.status_text()));
    }
    resp.json::<T>().await.map_err(|e| e.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_json<T: DeserializeOwned>(_url: &str) -> Result<T, String> {
    Err("fetch is only available in the browser".to_string())
}

// ---- POST (mutations) -----------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub async fn post_action(body: &serde_json::Value) -> Result<(), String> {
    use gloo_net::http::Request;
    let resp = Request::post("/api/action")
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.ok() {
        Ok(())
    } else {
        Err(resp.text().await.unwrap_or_else(|_| resp.status_text()))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn post_action(_body: &serde_json::Value) -> Result<(), String> {
    Err("not available on the server".to_string())
}

// ---- SSE ------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub struct SseHandle {
    es: web_sys::EventSource,
    _on_message: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_eof: Option<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MessageEvent)>>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for SseHandle {
    fn drop(&mut self) {
        self.es.close();
    }
}

/// Open an SSE connection, calling `on_event` for each decoded message. The
/// returned handle closes the connection when dropped.
#[cfg(target_arch = "wasm32")]
pub fn subscribe<F>(url: &str, on_event: F) -> Option<SseHandle>
where
    F: Fn(roder_core::WatchEvent) + 'static,
{
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let es = web_sys::EventSource::new(url).ok()?;
    let cb = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
        if let Some(txt) = e.data().as_string() {
            if let Ok(ev) = serde_json::from_str::<roder_core::WatchEvent>(&txt) {
                on_event(ev);
            }
        }
    }) as Box<dyn FnMut(web_sys::MessageEvent)>);
    es.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    Some(SseHandle {
        es,
        _on_message: cb,
        _on_eof: None,
    })
}

/// Subscribe to a raw-text SSE stream (used for log lines).
#[cfg(target_arch = "wasm32")]
pub fn subscribe_lines<F>(url: &str, on_line: F) -> Option<SseHandle>
where
    F: Fn(String) + 'static,
{
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let es = web_sys::EventSource::new(url).ok()?;
    let cb = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
        if let Some(txt) = e.data().as_string() {
            on_line(txt);
        }
    }) as Box<dyn FnMut(web_sys::MessageEvent)>);
    es.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    // The server sends an `eof` event when the log stream finishes (e.g. a completed
    // pod). Close on it so the browser doesn't auto-reconnect and replay the logs.
    let es_eof = es.clone();
    let eof = Closure::wrap(Box::new(move |_e: web_sys::MessageEvent| {
        es_eof.close();
    }) as Box<dyn FnMut(web_sys::MessageEvent)>);
    let _ = es.add_event_listener_with_callback("eof", eof.as_ref().unchecked_ref());
    Some(SseHandle {
        es,
        _on_message: cb,
        _on_eof: Some(eof),
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub struct SseHandle;

#[cfg(not(target_arch = "wasm32"))]
pub fn subscribe<F>(_url: &str, _on_event: F) -> Option<SseHandle>
where
    F: Fn(roder_core::WatchEvent) + 'static,
{
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn subscribe_lines<F>(_url: &str, _on_line: F) -> Option<SseHandle>
where
    F: Fn(String) + 'static,
{
    None
}

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

/// Whether focus is currently in a text input (so shortcuts like ⌃Z don't hijack it).
#[cfg(target_arch = "wasm32")]
pub fn is_text_input_focused() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element())
        .map(|e| matches!(e.tag_name().to_uppercase().as_str(), "INPUT" | "TEXTAREA"))
        .unwrap_or(false)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn is_text_input_focused() -> bool {
    false
}

// ---- age formatting -------------------------------------------------------

/// Humanize an RFC3339 timestamp into a compact relative age (e.g. "3d", "5m").
#[cfg(target_arch = "wasm32")]
pub fn humanize_age(created: &Option<String>) -> String {
    let Some(ts) = created else {
        return String::new();
    };
    let parsed = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(ts)).get_time();
    if parsed.is_nan() {
        return String::new();
    }
    let now = js_sys::Date::now();
    let secs = ((now - parsed) / 1000.0).max(0.0) as u64;
    format_age(secs)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn humanize_age(_created: &Option<String>) -> String {
    String::new()
}

#[allow(dead_code)]
fn format_age(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}
