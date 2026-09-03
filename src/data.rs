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
pub(crate) fn percent_encode(s: &str) -> String {
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

/// Build the SSE URL for a multiplexed multi-pane watch stream.
pub fn watch_multi_url(panes: &[(&str, Option<&str>)]) -> String {
    let parts: Vec<String> = panes
        .iter()
        .map(|(key, ns)| format!("{}:{}", percent_encode(key), ns.unwrap_or("")))
        .collect();
    format!("/api/watch-multi?panes={}", parts.join(","))
}

/// Build the detail URL for a single object.
pub fn detail_url(key: &str, namespace: Option<&str>, name: &str) -> String {
    format!(
        "/api/detail?key={}&namespace={}&name={}",
        key,
        namespace.map(percent_encode).unwrap_or_default(),
        percent_encode(name)
    )
}

pub fn container_file_url(
    endpoint: &str,
    namespace: &str,
    pod: &str,
    container: &str,
    path: &str,
) -> String {
    format!(
        "/api/{endpoint}?namespace={}&pod={}&container={}&path={}",
        percent_encode(namespace),
        percent_encode(pod),
        percent_encode(container),
        percent_encode(path),
    )
}

// ---- REST fetch -----------------------------------------------------------

/// A 401 means the session is gone (cookie expired, or its refresh token can no
/// longer mint a new id token). Send the browser to the login route so it
/// re-authenticates — silent if the IdP still holds an SSO session, otherwise a
/// normal sign-in — instead of leaving the page stuck on failing requests.
/// Returns `true` if it redirected. The 45s heartbeat (`/api/me`) makes this the
/// chokepoint that catches a dead session even on SSE-only views.
#[cfg(target_arch = "wasm32")]
fn redirect_to_login_if_unauthorized(status: u16) -> bool {
    if status != 401 {
        return false;
    }
    if let Some(win) = web_sys::window() {
        let _ = win.location().set_href("/auth/login");
    }
    true
}

#[cfg(target_arch = "wasm32")]
pub async fn fetch_json<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    use gloo_net::http::Request;
    let resp = Request::get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.ok() {
        redirect_to_login_if_unauthorized(resp.status());
        let fallback = format!("{} {}", resp.status(), resp.status_text());
        return Err(match resp.text().await {
            Ok(message) if !message.trim().is_empty() => message,
            _ => fallback,
        });
    }
    resp.json::<T>().await.map_err(|e| e.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_json<T: DeserializeOwned>(_url: &str) -> Result<T, String> {
    Err("fetch is only available in the browser".to_string())
}

#[cfg(target_arch = "wasm32")]
pub async fn post_json<T: DeserializeOwned>(
    url: &str,
    body: &serde_json::Value,
) -> Result<T, String> {
    use gloo_net::http::Request;
    let resp = Request::post(url)
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        redirect_to_login_if_unauthorized(resp.status());
        let fallback = format!("{} {}", resp.status(), resp.status_text());
        return Err(match resp.text().await {
            Ok(message) if !message.trim().is_empty() => message,
            _ => fallback,
        });
    }
    resp.json::<T>().await.map_err(|e| e.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn post_json<T: DeserializeOwned>(
    _url: &str,
    _body: &serde_json::Value,
) -> Result<T, String> {
    Err("fetch is only available in the browser".to_string())
}

/// Probe `url` with a GET to extract a human-readable error (e.g. "401 Unauthorized").
/// Used when an SSE stream closes unexpectedly — the `onerror` event carries no message.
#[cfg(target_arch = "wasm32")]
pub async fn probe_error(url: String) -> String {
    use gloo_net::http::Request;
    match Request::get(&url).send().await {
        Err(e) => e.to_string(),
        Ok(resp) if resp.ok() => "Connection lost".to_string(),
        Ok(resp) => {
            redirect_to_login_if_unauthorized(resp.status());
            format!("{} {}", resp.status(), resp.status_text())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn probe_error(_url: String) -> String {
    "Connection lost".to_string()
}

// ---- POST (mutations) -----------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub async fn post_action(body: &serde_json::Value) -> Result<String, String> {
    use gloo_net::http::Request;
    let resp = Request::post("/api/action")
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.ok() {
        Ok(resp.text().await.unwrap_or_default())
    } else {
        redirect_to_login_if_unauthorized(resp.status());
        Err(resp.text().await.unwrap_or_else(|_| resp.status_text()))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn post_action(_body: &serde_json::Value) -> Result<String, String> {
    Err("not available on the server".to_string())
}

// ---- SSE ------------------------------------------------------------------

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
    let secs = ((js_sys::Date::now() - parsed) / 1000.0).max(0.0) as u64;
    roder_core::format_age_secs(secs)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn humanize_age(_created: &Option<String>) -> String {
    String::new()
}

/// Cheap structural check for an RFC3339 timestamp: `YYYY-MM-DDTHH:MM:SS…`.
/// Used to decide whether a cell value should be live-humanized on the tick
/// (mirroring the dedicated `Age` column) rather than rendered as a static
/// string. Kubernetes timestamp fields are reliably RFC3339, so the risk of a
/// false positive on a non-date column is negligible.
pub fn looks_like_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 20
        && b.len() <= 35
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        // year is digits
        && b[0..4].iter().all(|c| c.is_ascii_digit())
}

pub fn cell_needs_tick(s: &str) -> bool {
    looks_like_rfc3339(s)
        || s.split_once('\x1f')
            .is_some_and(|(_, hint)| looks_like_rfc3339(hint))
}

/// Humanize an RFC3339 cell or timestamp hint. The caller must read the global
/// `Tick` so the returned value re-renders each second.
#[cfg(target_arch = "wasm32")]
pub fn humanize_cell(s: &str) -> String {
    if let Some((value, hint)) = s.split_once('\x1f') {
        if looks_like_rfc3339(hint) {
            return format!("{value}\x1f{} ago", humanize_age(&Some(hint.to_string())));
        }
    }
    if looks_like_rfc3339(s) {
        humanize_age(&Some(s.to_string()))
    } else {
        s.to_string()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn humanize_cell(s: &str) -> String {
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_unreserved_passthrough() {
        assert_eq!(percent_encode("nginx-web"), "nginx-web");
        assert_eq!(percent_encode("v1.2.3~rc_1"), "v1.2.3~rc_1");
    }

    #[test]
    fn percent_encode_special_chars() {
        assert_eq!(
            percent_encode("app=nginx,tier=web"),
            "app%3Dnginx%2Ctier%3Dweb"
        );
        assert_eq!(percent_encode("ns/name"), "ns%2Fname");
        assert_eq!(percent_encode("hello world"), "hello%20world");
    }

    #[test]
    fn timestamp_hints_require_the_live_ui_tick() {
        assert!(cell_needs_tick("2\x1f2026-08-27T12:00:00Z"));
        assert!(cell_needs_tick("2026-08-27T12:00:00Z"));
        assert!(!cell_needs_tick("2\x1f5m ago"));
    }

    #[test]
    fn percent_encode_empty() {
        assert_eq!(percent_encode(""), "");
    }

    #[test]
    fn detail_url_with_namespace() {
        let u = detail_url("apps/v1/Deployment", Some("default"), "my-deploy");
        assert_eq!(
            u,
            "/api/detail?key=apps/v1/Deployment&namespace=default&name=my-deploy"
        );
    }

    #[test]
    fn detail_url_cluster_scoped() {
        let u = detail_url("rbac.authorization.k8s.io/v1/ClusterRole", None, "admin");
        assert_eq!(
            u,
            "/api/detail?key=rbac.authorization.k8s.io/v1/ClusterRole&namespace=&name=admin"
        );
    }

    #[test]
    fn detail_url_encodes_name_with_special_chars() {
        let u = detail_url("v1/Pod", Some("kube system"), "my=pod");
        assert_eq!(
            u,
            "/api/detail?key=v1/Pod&namespace=kube%20system&name=my%3Dpod"
        );
    }

    #[test]
    fn container_file_url_encodes_every_query_value() {
        let url = container_file_url("files", "kube system", "web=1", "side/car", "/var/log/a b");
        assert_eq!(
            url,
            "/api/files?namespace=kube%20system&pod=web%3D1&container=side%2Fcar&path=%2Fvar%2Flog%2Fa%20b"
        );
    }

    #[test]
    fn rfc3339_detector_accepts_kubernetes_timestamps() {
        // The forms k8s actually emits: with `Z` or a numeric offset, with
        // and without sub-second precision.
        assert!(looks_like_rfc3339("2024-03-01T12:34:56Z"));
        assert!(looks_like_rfc3339("2024-03-01T12:34:56.000000Z"));
        assert!(looks_like_rfc3339("2024-03-01T12:34:56+00:00"));
        assert!(looks_like_rfc3339("2024-03-01T12:34:56.123456789+02:00"));
    }

    #[test]
    fn rfc3339_detector_rejects_non_dates() {
        // Phase, status, numeric metrics, plain text — none of these have the
        // RFC3339 shape, so a non-date cell is left untouched.
        assert!(!looks_like_rfc3339("Running"));
        assert!(!looks_like_rfc3339("True"));
        assert!(!looks_like_rfc3339("42"));
        assert!(!looks_like_rfc3339("3.14Gi"));
        assert!(!looks_like_rfc3339("2024-03-01"));
        assert!(!looks_like_rfc3339(""));
        // An ISO date without the `T` separator isn't RFC3339 — the form k8s
        // stamps onto objects always carries the `T`, so this guard keeps the
        // heuristic narrow.
        assert!(!looks_like_rfc3339("2024-03-01 12:34:56Z"));
    }
}
