//! REST fetch and mutation POSTs. Every browser API is `wasm32`-gated; on
//! the SSR build these are no-ops, since the UI loads its data after
//! hydration in the browser.

use serde::de::DeserializeOwned;

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
