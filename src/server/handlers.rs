use std::sync::Arc;

use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use axum::Json;
use roder_auth::{AuthError, Identity, Tokens};
use roder_k8s::Backend;
use serde::Deserialize;

use super::session::{
    cookie_value, open_pending, open_session, seal_pending, seal_session, set_cookie, PendingLogin,
    LOGIN_COOKIE, SESSION_COOKIE,
};
use super::AppState;

/// Liveness/readiness — always public.
pub async fn health() -> Json<roder_core::Health> {
    Json(roder_core::Health::ok())
}

/// Add security headers to every response. The CSP
/// allows the wasm runtime + same-origin SSE/fetch. In dev mode, the
/// `connect-src` is widened to permit cargo-leptos's `live_reload` WebSocket
/// on the loopback hot-reload port (default 8081, configurable).
pub async fn security_headers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();

    // Cache-Control: static assets need explicit directives so browsers don't
    // serve a stale WASM/JS after a new container is deployed.
    //
    // - .wasm / .js: must revalidate every load (ETag round-trip is cheap;
    //   a stale WASM paired with new SSR HTML causes hydration failures).
    // - .css: cargo-leptos writes a content-hash into the filename via
    //   HashedStylesheet, so the URL changes on every build — safe to cache
    //   forever.
    // - fonts / favicon: content rarely changes, short public TTL is fine.
    let cache = if path.ends_with(".wasm") || path.ends_with(".js") {
        Some("no-cache")
    } else if path.ends_with(".css") {
        Some("public, max-age=31536000, immutable")
    } else if path.starts_with("/fonts/") || path.ends_with(".svg") || path.ends_with(".ico") {
        Some("public, max-age=604800")
    } else {
        None
    };
    if let Some(v) = cache {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static(v));
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    h.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
    );

    // /terminal is served inside our own iframe — it must allow framing by
    // 'self' and load xterm.js + its CSS from the CDN.
    // All other pages stay locked down with frame-ancestors 'none'.
    let is_terminal = path == "/terminal";

    if !is_terminal {
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }

    let connect_src = if state.config.dev_mode {
        "connect-src 'self' ws://127.0.0.1:* ws://localhost:* wss://127.0.0.1:* wss://localhost:*"
    } else {
        "connect-src 'self'"
    };
    let csp = if is_terminal {
        format!(
            "default-src 'self'; \
             script-src 'self' https://cdn.jsdelivr.net 'unsafe-inline'; \
             style-src 'self' https://cdn.jsdelivr.net 'unsafe-inline'; \
             img-src 'self' data:; font-src 'self' https://cdn.jsdelivr.net; \
             {connect_src}; frame-ancestors 'self'; base-uri 'self'"
        )
    } else {
        format!(
            "default-src 'self'; \
             script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; \
             style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; font-src 'self'; \
             {connect_src}; frame-ancestors 'none'; base-uri 'self'"
        )
    };
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&csp).expect("static CSP is valid"),
    );
    resp
}

/// Middleware: require a valid session for app pages and `/api/*`. The session
/// lives entirely in the sealed `roder_session` cookie (no server-side store),
/// so it survives roder being restarted/rescheduled: we decrypt the cookie,
/// (re)establish the cluster backend, refresh the token if it's near expiry, and
/// re-seal the — possibly rotated — token set back into the cookie. Unauthorized
/// browser requests redirect to `/auth/login`; API requests get 401.
pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if state.config.dev_mode {
        return next.run(req).await;
    }

    let is_api = req.uri().path().starts_with("/api/");
    let reject = || -> Response {
        if is_api {
            StatusCode::UNAUTHORIZED.into_response()
        } else {
            Redirect::to("/auth/login").into_response()
        }
    };

    let key = match state.config.session_key {
        Some(k) => k,
        None => return reject(),
    };
    let Some(cookie) = cookie_value(req.headers(), SESSION_COOKIE) else {
        return reject();
    };
    let Some(cookie_tokens) = open_session(&cookie, &key) else {
        return reject();
    };

    // Ensure the backend is live and the token is fresh (refreshing on the way
    // through), yielding the token set to re-seal.
    let fresh = match ensure_session(&state, cookie_tokens).await {
        Ok(t) => t,
        Err(()) => return reject(),
    };

    let mut resp = next.run(req).await;
    let sealed = seal_session(&fresh, &key);
    if let Ok(v) = HeaderValue::from_str(&set_cookie(
        SESSION_COOKIE,
        &sealed,
        state.config.secure_cookies(),
        604_800,
    )) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    resp
}

/// Make the request's session usable: ensure the shared cluster backend exists
/// and carries a non-expired token, refreshing via the IdP when needed. Returns
/// the live token set (post-refresh) to re-seal into the cookie. Refreshes are
/// single-flighted so concurrent requests don't each spend (and rotate away)
/// the refresh token.
async fn ensure_session(state: &AppState, cookie_tokens: Tokens) -> Result<Tokens, ()> {
    // Reuse the in-memory token only for the same subject. A different user's
    // cookie must never inherit the previous user's identity, groups, or token.
    let working = {
        let cur = state.current.read().await;
        tokens_for_subject(cur.as_ref(), cookie_tokens)
    };

    // The cookie carries no id token (kept small), so a cold start yields an
    // empty one and must refresh to obtain a usable token + rebuild the backend.
    if working.id_token.is_empty() || working.needs_refresh() {
        let _guard = state.refresh_lock.lock().await;
        // Re-check under the lock: another request may have just refreshed.
        if let Some(c) = state
            .current
            .read()
            .await
            .as_ref()
            .filter(|tokens| tokens.identity.subject == working.identity.subject)
            .cloned()
        {
            if !c.needs_refresh() {
                ensure_backend(state, &c.id_token).await;
                return Ok(c);
            }
        }
        // The token is unusable without a successful refresh — these failures
        // do reject (→ re-login).
        let provider = state.provider.clone().ok_or(())?;
        let rt = working.refresh_token.clone().ok_or(())?;
        let refreshed = provider.refresh(rt).await.map_err(|_| ())?;
        ensure_backend(state, &refreshed.id_token).await;
        *state.current.write().await = Some(refreshed.clone());
        return Ok(refreshed);
    }

    // `provider.is_some()` is always true for a real (non-dev) server — it just
    // keeps us from attempting cluster I/O when there's no provider wired up.
    if state.provider.is_some() {
        ensure_backend(state, &working.id_token).await;
    }
    // Adopt the cookie's tokens as the live set if we didn't have one.
    {
        let mut cur = state.current.write().await;
        if cur.is_none() {
            *cur = Some(working.clone());
        }
    }
    Ok(working)
}

fn tokens_for_subject(current: Option<&Tokens>, cookie: Tokens) -> Tokens {
    current
        .filter(|tokens| tokens.identity.subject == cookie.identity.subject)
        .cloned()
        .unwrap_or(cookie)
}

/// Ensure the shared backend exists and uses `id_token`, building it on first
/// use (e.g. the first authenticated request after a restart). Best-effort: a
/// cluster that's briefly unreachable must not log the user out — the session
/// is still valid, and the API handlers already surface "not connected" (503)
/// until the backend comes up. Returns whether the backend is now ready, which
/// the login callback uses to report a hard connect failure immediately.
async fn ensure_backend(state: &AppState, id_token: &str) -> bool {
    // Fast path: the backend already exists (normally built once at startup) —
    // just swap in this request's token.
    if let Some(backend) = state.backend.read().await.as_ref() {
        let _ = backend.set_token(id_token);
        return true;
    }
    // Cold path: startup connect failed and the backend isn't up. Single-flight
    // the (expensive) discovery + CRD load so concurrent/repeated requests don't
    // each run it and hammer the apiserver.
    let _guard = state.backend_build_lock.lock().await;
    if let Some(backend) = state.backend.read().await.as_ref() {
        let _ = backend.set_token(id_token);
        return true;
    }
    let new_backend = match Backend::connect_with_token(id_token).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("failed to (re)connect cluster backend: {e}");
            return false;
        }
    };
    let client = new_backend.client();
    *state.backend.write().await = Some(Arc::new(new_backend));
    let url = roder_k8s::discover_alertmanager(&client).await;
    *state.alerts.write().await = url.map(|u| Arc::new(roder_k8s::AlertsCache::new(u)));
    true
}

/// Begin the OIDC auth-code flow. The CSRF/nonce/PKCE state is sealed into the
/// `roder_login` cookie (not a server-side store), so the callback can recover
/// it on any replica and after a restart.
pub async fn login(State(state): State<AppState>) -> Response {
    let Some(provider) = state.provider.clone() else {
        // Dev mode: nothing to log into.
        return Redirect::to("/").into_response();
    };
    let Some(key) = state.config.session_key else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "missing session key").into_response();
    };

    let start = provider.begin_login();
    let pending = PendingLogin {
        csrf_state: start.csrf_state,
        nonce: start.nonce,
        pkce_verifier: start.pkce_verifier,
    };
    let sealed = seal_pending(&pending, &key, 600);
    let cookie = set_cookie(LOGIN_COOKIE, &sealed, state.config.secure_cookies(), 600);
    (
        [(header::SET_COOKIE, cookie)],
        Redirect::to(&start.authorize_url),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// OAuth callback: verify state, exchange the code, enforce groups, create a
/// session, and establish the token-passthrough cluster client.
pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Response {
    let Some(provider) = state.provider.clone() else {
        return Redirect::to("/").into_response();
    };

    if let Some(err) = params.error {
        let desc = params.error_description.unwrap_or_default();
        return (StatusCode::BAD_REQUEST, format!("OIDC error: {err} {desc}")).into_response();
    }

    let Some(key) = state.config.session_key else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "missing session key").into_response();
    };
    let Some(login_cookie) = cookie_value(&headers, LOGIN_COOKIE) else {
        return (StatusCode::BAD_REQUEST, "missing login cookie").into_response();
    };
    let Some(pending) = open_pending(&login_cookie, &key) else {
        return (StatusCode::BAD_REQUEST, "unknown or expired login").into_response();
    };

    match (params.state.as_deref(), params.code) {
        (Some(s), _) if s != pending.csrf_state => {
            (StatusCode::BAD_REQUEST, "state mismatch (possible CSRF)").into_response()
        }
        (Some(_), Some(code)) => {
            match provider
                .exchange_code(code, pending.pkce_verifier, pending.nonce)
                .await
            {
                Ok(tokens) => {
                    if !ensure_backend(&state, &tokens.id_token).await {
                        return (
                            StatusCode::BAD_GATEWAY,
                            "connected to OIDC but failed to reach the cluster",
                        )
                            .into_response();
                    }
                    let sealed = seal_session(&tokens, &key);
                    *state.current.write().await = Some(tokens);

                    let secure = state.config.secure_cookies();
                    (
                        AppendHeaders([
                            (
                                header::SET_COOKIE,
                                set_cookie(SESSION_COOKIE, &sealed, secure, 604_800),
                            ),
                            (header::SET_COOKIE, set_cookie(LOGIN_COOKIE, "", secure, 0)),
                        ]),
                        Redirect::to("/"),
                    )
                        .into_response()
                }
                Err(AuthError::Forbidden) => (
                    StatusCode::FORBIDDEN,
                    "you are not a member of an allowed group",
                )
                    .into_response(),
                Err(e) => (StatusCode::BAD_GATEWAY, format!("login failed: {e}")).into_response(),
            }
        }
        _ => (StatusCode::BAD_REQUEST, "missing code or state").into_response(),
    }
}

/// Clear the live session and the cookie, then redirect. If `signout_redirect_url`
/// is configured the browser is sent there (so the OIDC provider can end the SSO
/// session too); otherwise falls back to `/auth/login`.
pub async fn logout(State(state): State<AppState>, _headers: HeaderMap) -> Response {
    *state.current.write().await = None;
    let cookie = set_cookie(SESSION_COOKIE, "", state.config.secure_cookies(), 0);
    let dest = state
        .config
        .signout_redirect_url
        .as_deref()
        .unwrap_or("/auth/login");
    ([(header::SET_COOKIE, cookie)], Redirect::to(dest)).into_response()
}

/// Who am I — used by the UI to show the signed-in identity. `require_auth` has
/// already validated/refreshed the session, so the identity is in `current`;
/// fall back to decrypting the cookie directly just in case.
pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if state.config.dev_mode {
        return Json(Identity {
            subject: "dev".to_string(),
            email: None,
            name: Some("dev mode".to_string()),
            groups: vec![],
        })
        .into_response();
    }

    if let Some(tokens) = state.current.read().await.as_ref() {
        return Json(tokens.identity.clone()).into_response();
    }
    match (
        state.config.session_key,
        cookie_value(&headers, SESSION_COOKIE),
    ) {
        (Some(key), Some(cookie)) => match open_session(&cookie, &key) {
            Some(tokens) => Json(tokens.identity).into_response(),
            None => StatusCode::UNAUTHORIZED.into_response(),
        },
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the auth handlers and the middleware that guards them.
    //!
    //! What we can exercise without a real OIDC provider:
    //! 1. The cookie-format contract (the regression we just fixed).
    //! 2. Dev-mode paths (`provider: None`) for `login`, `callback`, `me`.
    //! 3. `logout` (no provider needed).
    //! 4. `require_auth` middleware behavior across dev/prod and with/without
    //!    a valid session cookie.
    //!
    //! Out of scope: the actual OIDC code exchange, which needs a real IdP.

    use super::*;
    use crate::server::config::ServerConfig;
    use axum::body::Body;
    use axum::http::header::SET_COOKIE;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use axum::Router;
    use leptos::prelude::LeptosOptions;
    use roder_auth::Identity;
    use std::sync::Arc;
    use time::OffsetDateTime;
    use tokio::sync::{Mutex, RwLock};
    use tower::ServiceExt;

    // -------- helpers --------

    const TEST_KEY: [u8; 32] = [7u8; 32];

    fn dev_config() -> Arc<ServerConfig> {
        Arc::new(ServerConfig {
            dev_mode: true,
            base_url: "http://localhost:8080".into(),
            oidc: None,
            session_key: None,
            signout_redirect_url: None,
            talos_reader_groups: vec![],
            talos_operator_groups: vec![],
            talos_actions_enabled: false,
            talos_config_groups: vec![],
            talos_config_enabled: false,
            pod_node_name: None,
        })
    }

    fn prod_config() -> Arc<ServerConfig> {
        Arc::new(ServerConfig {
            dev_mode: false,
            base_url: "https://roder.example.com".into(),
            oidc: Some(crate::server::config::OidcSettings {
                issuer_url: "https://idp.example.com".into(),
                client_id: "cid".into(),
                client_secret: "csec".into(),
                allowed_groups: vec![],
                groups_claim: "groups".into(),
            }),
            session_key: Some(TEST_KEY),
            signout_redirect_url: None,
            talos_reader_groups: vec![],
            talos_operator_groups: vec![],
            talos_actions_enabled: false,
            talos_config_groups: vec![],
            talos_config_enabled: false,
            pod_node_name: None,
        })
    }

    fn empty_state(config: Arc<ServerConfig>) -> AppState {
        AppState {
            leptos_options: empty_leptos_options(),
            asset_version: Arc::from("test-version"),
            config,
            provider: None,
            backend: Arc::new(RwLock::new(None)),
            current: Arc::new(RwLock::new(None)),
            refresh_lock: Arc::new(Mutex::new(())),
            backend_build_lock: Arc::new(Mutex::new(())),
            alerts: Arc::new(RwLock::new(None)),
            talos: None,
            talos_action_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Build a dev-mode `AppState`.
    fn dev_state() -> AppState {
        empty_state(dev_config())
    }

    /// Build a production `AppState` (no real `provider` — tests that don't
    /// need the OIDC exchange just pass through with `provider: None`).
    fn prod_state_without_provider() -> AppState {
        empty_state(prod_config())
    }

    /// A `Cookie:` header carrying a validly-sealed session for `tokens`.
    fn sealed_cookie_header(tokens: &roder_auth::Tokens) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let sealed = seal_session(tokens, &TEST_KEY);
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}={sealed}")).unwrap(),
        );
        headers
    }

    /// `LeptosOptions` has no public `Default` and is `#[non_exhaustive]`, so
    /// the only reliable way to build one in a test is to deserialize an
    /// empty-ish JSON. Most fields carry `#[serde(default = "…")]`, so
    /// `output-name` is the only required key.
    fn empty_leptos_options() -> LeptosOptions {
        serde_json::from_str(r#"{"output-name": "roder"}"#)
            .expect("LeptosOptions should deserialize from a minimal JSON")
    }

    fn fake_tokens() -> roder_auth::Tokens {
        roder_auth::Tokens {
            id_token: "id".into(),
            access_token: "access".into(),
            refresh_token: Some("rt".into()),
            expires_at: OffsetDateTime::now_utc() + time::Duration::seconds(3600),
            identity: Identity {
                subject: "sub".into(),
                email: Some("a@b".into()),
                name: Some("Alice".into()),
                groups: vec!["admins".into()],
            },
        }
    }

    #[test]
    fn never_reuses_another_subjects_live_tokens() {
        let current = fake_tokens();
        let mut cookie = fake_tokens();
        cookie.identity.subject = "other-user".into();
        cookie.identity.groups = vec!["readers".into()];
        cookie.id_token.clear();

        let selected = tokens_for_subject(Some(&current), cookie);
        assert_eq!(selected.identity.subject, "other-user");
        assert_eq!(selected.identity.groups, vec!["readers"]);
        assert!(selected.id_token.is_empty());
    }

    fn collect_set_cookies(resp: &Response) -> Vec<String> {
        resp.headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(String::from))
            .collect()
    }

    // -------- THE cookie regression test --------
    //
    // The `Set-Cookie` header is the one HTTP header that MUST be appendable:
    // a response setting both a session cookie and a login-clearing cookie has
    // to carry two `Set-Cookie` lines. axum's `IntoResponseParts` impl for
    // `[(K, V); N]` calls `headers.insert()` (overwrites duplicates), so a
    // naive `[(SET_COOKIE, …), (SET_COOKIE, …)]` tuple silently drops the
    // first cookie. `AppendHeaders` uses `headers.append()` and is the only
    // correct way to set multiple Set-Cookie headers in axum. This test pins
    // the exact response-building pattern the callback uses so a future
    // refactor can't regress to the broken form.

    #[test]
    fn callback_success_response_emits_both_set_cookie_headers() {
        let sid = "abc123def456";
        let secure = true;

        // Mirror the production success path's tuple exactly.
        let response = (
            AppendHeaders([
                (SET_COOKIE, set_cookie(SESSION_COOKIE, sid, secure, 604_800)),
                (SET_COOKIE, set_cookie(LOGIN_COOKIE, "", secure, 0)),
            ]),
            Redirect::to("/"),
        )
            .into_response();

        let cookies = collect_set_cookies(&response);
        assert_eq!(
            cookies.len(),
            2,
            "expected 2 Set-Cookie headers, got {}: {:?}",
            cookies.len(),
            cookies
        );

        let joined = cookies.join("\n");
        assert!(
            joined.contains(&format!("roder_session={sid}")),
            "session cookie missing: {joined}"
        );
        assert!(
            joined.contains("roder_login=;"),
            "login-clearing cookie missing: {joined}"
        );
        assert!(
            joined.contains("Max-Age=0"),
            "login-clearing cookie must have Max-Age=0: {joined}"
        );
        assert!(
            joined.contains("Max-Age=604800"),
            "session cookie must have 1-week Max-Age: {joined}"
        );
    }

    #[test]
    fn array_form_of_two_set_cookies_only_keeps_last_documents_the_bug() {
        // Documents the axum behavior the regression test protects against.
        // If this test ever changes behavior, axum has changed and the
        // production code is safe to revert to the array form.
        let response: Response = (
            [
                (SET_COOKIE, "roder_session=abc; Max-Age=604800".to_string()),
                (SET_COOKIE, "roder_login=; Max-Age=0".to_string()),
            ],
            Redirect::to("/"),
        )
            .into_response();

        let cookies = collect_set_cookies(&response);
        assert_eq!(
            cookies.len(),
            1,
            "the array form drops the first Set-Cookie (this is the bug): {cookies:?}"
        );
        assert!(cookies[0].contains("roder_login=;"));
    }

    // -------- health --------

    #[tokio::test]
    async fn health_returns_ok() {
        let res = health().await;
        assert_eq!(res.0.status, roder_core::HealthStatus::Ok);
    }

    // -------- login --------

    #[tokio::test]
    async fn login_dev_mode_redirects_to_root() {
        let state = dev_state();
        let res = login(State(state)).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers().get(header::LOCATION).unwrap(), "/");
    }

    #[tokio::test]
    async fn login_dev_mode_does_not_set_login_cookie() {
        let state = dev_state();
        let res = login(State(state)).await;
        // No real provider ⇒ no PKCE flow ⇒ no login cookie.
        assert!(collect_set_cookies(&res).is_empty());
    }

    // -------- callback (dev mode / no provider) --------

    #[tokio::test]
    async fn callback_without_provider_redirects_to_root() {
        let state = dev_state();
        let res = callback(
            State(state),
            HeaderMap::new(),
            Query(CallbackParams {
                code: None,
                state: None,
                error: None,
                error_description: None,
            }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers().get(header::LOCATION).unwrap(), "/");
    }

    // -------- logout --------

    #[tokio::test]
    async fn logout_clears_session_cookie_and_redirects_to_login() {
        let state = prod_state_without_provider();
        // Plant a live session so we can verify it's cleared.
        *state.current.write().await = Some(fake_tokens());

        let res = logout(State(state.clone()), HeaderMap::new()).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers().get(header::LOCATION).unwrap(), "/auth/login");

        let cookies = collect_set_cookies(&res);
        assert_eq!(
            cookies.len(),
            1,
            "logout should clear one cookie: {cookies:?}"
        );
        assert!(
            cookies[0].contains(&format!("{SESSION_COOKIE}=;")),
            "must clear {SESSION_COOKIE}: {}",
            cookies[0]
        );
        assert!(cookies[0].contains("Max-Age=0"));

        // The live session must be gone.
        assert!(state.current.read().await.is_none());
    }

    #[tokio::test]
    async fn logout_without_session_cookie_still_clears_and_redirects() {
        let state = dev_state();
        let res = logout(State(state), HeaderMap::new()).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers().get(header::LOCATION).unwrap(), "/auth/login");
        assert_eq!(collect_set_cookies(&res).len(), 1);
    }

    #[tokio::test]
    async fn logout_with_signout_redirect_url_uses_configured_url() {
        let mut config = (*prod_config()).clone();
        config.signout_redirect_url =
            Some("https://sso.example.com/application/o/roder/end-session/".into());
        let state = empty_state(Arc::new(config));

        let res = logout(State(state), HeaderMap::new()).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            res.headers().get(header::LOCATION).unwrap(),
            "https://sso.example.com/application/o/roder/end-session/"
        );
        let cookies = collect_set_cookies(&res);
        assert_eq!(cookies.len(), 1);
        assert!(cookies[0].contains(&format!("{SESSION_COOKIE}=;")));
    }

    // -------- me --------

    #[tokio::test]
    async fn me_dev_mode_returns_dev_identity() {
        let state = dev_state();
        let res = me(State(state), HeaderMap::new()).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let id: Identity = serde_json::from_slice(&body).unwrap();
        assert_eq!(id.subject, "dev");
    }

    #[tokio::test]
    async fn me_production_without_session_returns_401() {
        let state = prod_state_without_provider();
        let res = me(State(state), HeaderMap::new()).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_production_with_unknown_session_returns_401() {
        let state = prod_state_without_provider();
        // A cookie that doesn't decrypt under our key (no live session either).
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}=nope")).unwrap(),
        );
        let res = me(State(state), headers).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_production_with_valid_session_returns_identity() {
        let state = prod_state_without_provider();
        let mut tokens = fake_tokens();
        tokens.identity.subject = "user-42".into();
        tokens.identity.name = Some("User Forty-Two".into());

        // No live session in memory: `me` must fall back to opening the cookie.
        let res = me(State(state), sealed_cookie_header(&tokens)).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let id: Identity = serde_json::from_slice(&body).unwrap();
        assert_eq!(id.subject, "user-42");
        assert_eq!(id.name.as_deref(), Some("User Forty-Two"));
    }

    // -------- require_auth middleware --------
    //
    // We use `tower::ServiceExt::oneshot` against a real `Router` so the
    // middleware's `Next` future is wired up correctly.

    fn make_protected_app(state: AppState) -> Router {
        Router::new()
            .route("/protected", get(|| async { "secret" }))
            .route("/api/data", get(|| async { "json" }))
            .route_layer(from_fn_with_state(state, require_auth))
    }

    #[tokio::test]
    async fn require_auth_dev_mode_always_passes() {
        let app = make_protected_app(dev_state());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn require_auth_production_without_session_redirects_to_login() {
        let app = make_protected_app(prod_state_without_provider());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers().get(header::LOCATION).unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn require_auth_production_api_without_session_returns_401() {
        let app = make_protected_app(prod_state_without_provider());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/data")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_auth_production_with_valid_session_passes() {
        // A warm in-memory session (as just after login) with a validly-sealed
        // cookie passes even with no cluster backend: the id token is present so
        // no refresh is needed, and backend establishment is best-effort, so an
        // unreachable cluster does not log the user out (handlers surface 503).
        let state = prod_state_without_provider();
        *state.current.write().await = Some(fake_tokens());
        let sealed = seal_session(&fake_tokens(), &TEST_KEY);

        let app = make_protected_app(state);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}={sealed}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn require_auth_production_with_unknown_session_redirects() {
        // Cookie present but it doesn't decrypt under our key (forged/garbage).
        let app = make_protected_app(prod_state_without_provider());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}=ghost"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
    }
}
