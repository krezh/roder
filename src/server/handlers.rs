use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use axum::Json;
use roder_auth::{AuthError, Identity};
use roder_k8s::Backend;
use serde::Deserialize;

use super::session::{
    cookie_value, random_id, set_cookie, PendingLogin, Session, LOGIN_COOKIE, SESSION_COOKIE,
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
        h.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(v),
        );
    }
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
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
    // CSP is strict by default; the only relaxation is dev-mode loopback ws
    // for the live-reload socket, which is otherwise harmless to allow since
    // an attacker would already need code execution on the developer box.
    let connect_src = if state.config.dev_mode {
        "connect-src 'self' ws://127.0.0.1:* ws://localhost:*"
    } else {
        "connect-src 'self'"
    };
    let csp = format!(
        "default-src 'self'; \
         script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; font-src 'self'; \
         {connect_src}; frame-ancestors 'none'; base-uri 'self'"
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&csp).expect("static CSP is valid"),
    );
    resp
}

/// Middleware: require a valid session for app pages and `/api/*`. Unauthenticated
/// browser requests are redirected to `/auth/login`; API requests get 401.
pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if state.config.dev_mode {
        return next.run(req).await;
    }

    if let Some(sid) = cookie_value(req.headers(), SESSION_COOKIE) {
        if state.sessions.contains(&sid).await {
            return next.run(req).await;
        }
    }

    if req.uri().path().starts_with("/api/") {
        StatusCode::UNAUTHORIZED.into_response()
    } else {
        Redirect::to("/auth/login").into_response()
    }
}

/// Begin the OIDC auth-code flow: stash CSRF/nonce/PKCE and redirect to the IdP.
pub async fn login(State(state): State<AppState>) -> Response {
    let Some(provider) = state.provider.clone() else {
        // Dev mode: nothing to log into.
        return Redirect::to("/").into_response();
    };

    let start = provider.begin_login();
    let pending_id = random_id();
    state
        .pending
        .insert(
            pending_id.clone(),
            PendingLogin {
                csrf_state: start.csrf_state,
                nonce: start.nonce,
                pkce_verifier: start.pkce_verifier,
                created: Instant::now(),
            },
        )
        .await;

    let cookie = set_cookie(
        LOGIN_COOKIE,
        &pending_id,
        state.config.secure_cookies(),
        600,
    );
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

    let Some(pending_id) = cookie_value(&headers, LOGIN_COOKIE) else {
        return (StatusCode::BAD_REQUEST, "missing login cookie").into_response();
    };
    let Some(pending) = state.pending.take(&pending_id).await else {
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
                    if let Err(resp) = establish_cluster(&state, &tokens.id_token).await {
                        return resp;
                    }
                    let sid = random_id();
                    state
                        .sessions
                        .insert(
                            sid.clone(),
                            Session {
                                identity: tokens.identity.clone(),
                                tokens,
                            },
                        )
                        .await;

                    let secure = state.config.secure_cookies();
                    (
                        AppendHeaders([
                            (
                                header::SET_COOKIE,
                                set_cookie(SESSION_COOKIE, &sid, secure, 604800),
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

/// Clear the session and return to login.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(sid) = cookie_value(&headers, SESSION_COOKIE) {
        state.sessions.remove(&sid).await;
    }
    let cookie = set_cookie(SESSION_COOKIE, "", state.config.secure_cookies(), 0);
    ([(header::SET_COOKIE, cookie)], Redirect::to("/auth/login")).into_response()
}

/// Who am I — used by the UI to show the signed-in identity.
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

    match cookie_value(&headers, SESSION_COOKIE) {
        Some(sid) => match state.sessions.get(&sid).await {
            Some(session) => Json(session.identity).into_response(),
            None => StatusCode::UNAUTHORIZED.into_response(),
        },
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// Connect (or reconnect) the cluster backend using the passthrough ID token.
async fn establish_cluster(state: &AppState, id_token: &str) -> Result<(), Response> {
    // Fast path: an existing backend only needs its token swapped.
    if let Some(backend) = state.backend.read().await.as_ref() {
        if backend.set_token(id_token).is_ok() {
            return Ok(());
        }
    }
    // Slow path: no backend yet. Build it outside the write lock so that
    // other readers are not blocked during the (potentially slow) cluster probe.
    let new_backend = match Backend::connect_with_token(id_token).await {
        Ok(b) => Arc::new(b),
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("connected to OIDC but failed to reach the cluster: {e}"),
            )
                .into_response());
        }
    };
    // Re-check under the write lock to close the TOCTOU window: if another
    // concurrent login raced us and already wrote a backend, update its token
    // instead of overwriting it with a potentially different identity.
    let mut lock = state.backend.write().await;
    if let Some(existing) = lock.as_ref() {
        let _ = existing.set_token(id_token);
    } else {
        *lock = Some(new_backend);
    }
    Ok(())
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
    use crate::server::session::{PendingStore, SessionStore};
    use axum::body::Body;
    use axum::http::header::SET_COOKIE;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use axum::Router;
    use leptos::prelude::LeptosOptions;
    use roder_auth::Identity;
    use std::sync::Arc;
    use time::OffsetDateTime;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    // -------- helpers --------

    fn dev_config() -> Arc<ServerConfig> {
        Arc::new(ServerConfig {
            dev_mode: true,
            base_url: "http://localhost:8080".into(),
            oidc: None,
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
        })
    }

    /// Build a dev-mode `AppState` with empty session/pending stores.
    fn dev_state() -> AppState {
        AppState {
            leptos_options: empty_leptos_options(),
            config: dev_config(),
            provider: None,
            sessions: Arc::new(SessionStore::default()),
            pending: Arc::new(PendingStore::default()),
            backend: Arc::new(RwLock::new(None)),
        }
    }

    /// Build a production `AppState` (no real `provider` — tests that don't
    /// need the OIDC exchange just pass through with `provider: None`).
    fn prod_state_without_provider() -> AppState {
        AppState {
            leptos_options: empty_leptos_options(),
            config: prod_config(),
            provider: None,
            sessions: Arc::new(SessionStore::default()),
            pending: Arc::new(PendingStore::default()),
            backend: Arc::new(RwLock::new(None)),
        }
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
                (
                    SET_COOKIE,
                    set_cookie(SESSION_COOKIE, sid, secure, 604_800),
                ),
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
        let state = dev_state();
        // Plant a session so we can verify it's removed.
        let sid = "sid-to-remove".to_string();
        state
            .sessions
            .insert(
                sid.clone(),
                Session {
                    identity: Identity {
                        subject: "sub".into(),
                        email: None,
                        name: None,
                        groups: vec![],
                    },
                    tokens: fake_tokens(),
                },
            )
            .await;

        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}={sid}")).unwrap(),
        );

        let res = logout(State(state.clone()), headers).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            res.headers().get(header::LOCATION).unwrap(),
            "/auth/login"
        );

        let cookies = collect_set_cookies(&res);
        assert_eq!(cookies.len(), 1, "logout should clear one cookie: {cookies:?}");
        assert!(
            cookies[0].contains(&format!("{SESSION_COOKIE}=;")),
            "must clear {SESSION_COOKIE}: {}",
            cookies[0]
        );
        assert!(cookies[0].contains("Max-Age=0"));

        // Server-side store should no longer have the session.
        assert!(!state.sessions.contains(&sid).await);
    }

    #[tokio::test]
    async fn logout_without_session_cookie_still_clears_and_redirects() {
        let state = dev_state();
        let res = logout(State(state), HeaderMap::new()).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers().get(header::LOCATION).unwrap(), "/auth/login");
        assert_eq!(collect_set_cookies(&res).len(), 1);
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
        let sid = "valid-sid".to_string();
        state
            .sessions
            .insert(
                sid.clone(),
                Session {
                    identity: Identity {
                        subject: "user-42".into(),
                        email: Some("user@x".into()),
                        name: Some("User Forty-Two".into()),
                        groups: vec!["g".into()],
                    },
                    tokens: fake_tokens(),
                },
            )
            .await;

        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}={sid}")).unwrap(),
        );

        let res = me(State(state), headers).await;
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
        let state = prod_state_without_provider();
        let sid = "good-sid".to_string();
        state
            .sessions
            .insert(
                sid.clone(),
                Session {
                    identity: Identity {
                        subject: "u".into(),
                        email: None,
                        name: None,
                        groups: vec![],
                    },
                    tokens: fake_tokens(),
                },
            )
            .await;

        let app = make_protected_app(state);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn require_auth_production_with_unknown_session_redirects() {
        // Cookie present but the session store has never seen this id.
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
