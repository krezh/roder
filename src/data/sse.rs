//! Server-sent-event subscriptions: the live streams every table and detail
//! view reads from.

#[cfg(target_arch = "wasm32")]
pub struct SseHandle {
    es: web_sys::EventSource,
    _on_message: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_error: Option<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>>,
    _on_eof: Option<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MessageEvent)>>,
    _on_version: Option<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MessageEvent)>>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for SseHandle {
    fn drop(&mut self) {
        self.es.close();
    }
}

/// Reconnect delay for a dropped SSE stream: a short *fixed* interval so the UI
/// recovers within ~a second of the server coming back. Deliberately flat — no
/// exponential backoff — because the connection blips repeatedly during a
/// rolling cluster upgrade (nodes rebooting in turn), and a dashboard you have
/// to hard-reload is far worse than a few extra reconnect attempts against a
/// single-user server. A little jitter keeps the handful of table streams from
/// all reconnecting on the exact same tick.
pub fn reconnect_delay() -> std::time::Duration {
    const BASE_MS: u64 = 1000;
    #[cfg(target_arch = "wasm32")]
    let ms = BASE_MS + (js_sys::Math::random() * 400.0) as u64; // +0..400ms
    #[cfg(not(target_arch = "wasm32"))]
    let ms = BASE_MS;
    std::time::Duration::from_millis(ms)
}

/// Attach a listener for the `version` named SSE event (see `version_event`
/// in `src/server/api.rs`) that forwards its payload to
/// `crate::version::on_server_version`, which reloads the tab on a mismatch.
#[cfg(target_arch = "wasm32")]
fn attach_version_listener(
    es: &web_sys::EventSource,
) -> wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MessageEvent)> {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let cb = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
        if let Some(txt) = e.data().as_string() {
            crate::version::on_server_version(&txt);
        }
    }) as Box<dyn FnMut(web_sys::MessageEvent)>);
    let _ = es.add_event_listener_with_callback("version", cb.as_ref().unchecked_ref());
    cb
}

/// Like [`subscribe_with_error`] but calls `on_error` when the EventSource fires an error
/// (e.g. the server returned a non-200 status). Callers use this to schedule a
/// reconnect rather than leaving the stream dead.
#[cfg(target_arch = "wasm32")]
pub fn subscribe_with_error<F, E>(url: &str, on_event: F, on_error: E) -> Option<SseHandle>
where
    F: Fn(roder_core::WatchEvent) + 'static,
    E: Fn() + 'static,
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
    let err_cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
        // Only trigger reconnect when the connection is fully closed (readyState=2).
        // While CONNECTING (0) the browser is already auto-retrying; firing our own
        // reconnect would open a second connection unnecessarily.
        let closed = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::EventSource>().ok())
            .is_none_or(|es| es.ready_state() == web_sys::EventSource::CLOSED);
        if closed {
            on_error();
        }
    }) as Box<dyn FnMut(web_sys::Event)>);
    es.set_onerror(Some(err_cb.as_ref().unchecked_ref()));
    let version_cb = attach_version_listener(&es);
    Some(SseHandle {
        es,
        _on_message: cb,
        _on_error: Some(err_cb),
        _on_eof: None,
        _on_version: Some(version_cb),
    })
}

/// Subscribe to a multiplexed multi-pane watch stream. Events are routed by
/// their `key` field to the appropriate pane signal.
#[cfg(target_arch = "wasm32")]
pub fn subscribe_multi<F, E>(url: &str, on_event: F, on_error: E) -> Option<SseHandle>
where
    F: Fn(String, roder_core::WatchEvent) + 'static,
    E: Fn() + 'static,
{
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let es = web_sys::EventSource::new(url).ok()?;
    let cb = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
        if let Some(txt) = e.data().as_string() {
            if let Ok(ev) = serde_json::from_str::<roder_core::MultiWatchEvent>(&txt) {
                on_event(ev.key, ev.event);
            }
        }
    }) as Box<dyn FnMut(web_sys::MessageEvent)>);
    es.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    let err_cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
        let closed = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::EventSource>().ok())
            .is_none_or(|es| es.ready_state() == web_sys::EventSource::CLOSED);
        if closed {
            on_error();
        }
    }) as Box<dyn FnMut(web_sys::Event)>);
    es.set_onerror(Some(err_cb.as_ref().unchecked_ref()));
    let version_cb = attach_version_listener(&es);
    Some(SseHandle {
        es,
        _on_message: cb,
        _on_error: Some(err_cb),
        _on_eof: None,
        _on_version: Some(version_cb),
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub fn subscribe_multi<F, E>(_url: &str, _on_event: F, _on_error: E) -> Option<SseHandle>
where
    F: Fn(String, roder_core::WatchEvent) + 'static,
    E: Fn() + 'static,
{
    None
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
        _on_error: None,
        _on_eof: Some(eof),
        _on_version: None,
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub struct SseHandle;

#[cfg(not(target_arch = "wasm32"))]
pub fn subscribe_with_error<F, E>(_url: &str, _on_event: F, _on_error: E) -> Option<SseHandle>
where
    F: Fn(roder_core::WatchEvent) + 'static,
    E: Fn() + 'static,
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
