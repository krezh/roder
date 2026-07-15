//! Security response headers, and the auth-guarding middleware: session
//! validation, token refresh, and (re)establishing the shared cluster backend.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use roder_auth::Tokens;
use roder_k8s::Backend;

use super::super::session::{cookie_value, open_session, seal_session, set_cookie, SESSION_COOKIE};
use crate::server::AppState;

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
pub(crate) async fn ensure_backend(state: &AppState, id_token: &str) -> bool {
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

#[cfg(test)]
mod tests {
    //! `tokens_for_subject` and the `require_auth` middleware across dev/prod
    //! and with/without a valid session cookie.

    use super::super::fixtures::*;
    use super::*;
    use axum::body::Body;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

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
