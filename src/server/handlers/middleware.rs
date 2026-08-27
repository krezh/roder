//! Security response headers, and the auth-guarding middleware: session
//! validation, per-subject backend resolution, token refresh, and re-sealing
//! the (possibly rotated) session cookie.

use axum::extract::{Request, State};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use roder_auth::Identity;

use super::super::session::{
    cookie_value, open_forwarded_tokens, open_session, seal_session, set_cookie, SESSION_COOKIE,
};
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
    } else if path.starts_with("/fonts/")
        || path.starts_with("/icons/")
        || path.ends_with(".svg")
        || path.ends_with(".ico")
    {
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
             script-src 'self' 'unsafe-inline'; \
             style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; font-src 'self'; \
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
/// resolve the subject's own per-user backend via the `BackendRegistry`
/// (building/reusing it and refreshing the token if it's near expiry), insert
/// it into the request's extensions for handlers to read, and re-seal the —
/// possibly rotated — token set back into the cookie. Unauthorized browser
/// requests redirect to `/auth/login`; API requests get 401.
pub async fn require_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    if state.config.dev_mode {
        // Dev mode bypasses OIDC entirely: there's no per-user token, just the
        // single implicit dev backend (built once, from inferred kubeconfig
        // creds) shared by every request.
        let backend = match state.backends.resolve_dev().await {
            Ok(b) => b,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        req.extensions_mut().insert(backend);
        req.extensions_mut().insert(Identity {
            subject: "dev".into(),
            email: None,
            name: Some("dev mode".into()),
            groups: Vec::new(),
        });
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
    let Some(mut cookie_tokens) = open_session(&cookie, &key) else {
        return reject();
    };

    if req
        .headers()
        .contains_key(crate::server::ha::INTERNAL_HEADER)
    {
        if let Some(forwarded) = req
            .headers()
            .get(crate::server::ha::FORWARDED_AUTH_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| open_forwarded_tokens(value, &key))
            .filter(|tokens| tokens.identity.subject == cookie_tokens.identity.subject)
        {
            cookie_tokens = forwarded;
        }
    }

    // Resolve (build-or-reuse, single-flighted per subject) this caller's own
    // backend, refreshing the token via the IdP on the way through if it's
    // near expiry.
    let resolved = match state.backends.resolve(&cookie_tokens).await {
        Ok(resolved) => resolved,
        Err(_) => return reject(),
    };
    req.extensions_mut().insert(resolved.backend.clone());
    req.extensions_mut()
        .insert(resolved.tokens.identity.clone());

    let mut resp = next.run(req).await;

    let has_forwarded_session = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.starts_with(&format!("{SESSION_COOKIE}=")));
    if !has_forwarded_session {
        let sealed = seal_session(&resolved.tokens, &key);
        if let Ok(v) = HeaderValue::from_str(&set_cookie(
            SESSION_COOKIE,
            &sealed,
            state.config.secure_cookies(),
            604_800,
        )) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    resp
}

/// Peer routing headers are meaningful only after the dedicated listener has
/// authenticated the caller's client certificate.
pub async fn reject_peer_headers(req: Request, next: Next) -> Response {
    if req
        .headers()
        .contains_key(crate::server::ha::INTERNAL_HEADER)
        || req
            .headers()
            .contains_key(crate::server::ha::FORWARDED_AUTH_HEADER)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    next.run(req).await
}

pub async fn require_peer_request(req: Request, next: Next) -> Response {
    let authenticated_peer = req
        .headers()
        .get(crate::server::ha::INTERNAL_HEADER)
        .is_some_and(|value| value == "1");
    if !authenticated_peer {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    //! The `require_auth` middleware across dev/prod and with/without a
    //! valid session cookie, plus that it inserts an `Extension<Arc<Backend>>`
    //! for downstream handlers.

    use super::super::fixtures::*;
    use super::*;
    use axum::body::Body;
    use axum::middleware::{from_fn, from_fn_with_state};
    use axum::routing::get;
    use axum::Extension;
    use axum::Json;
    use axum::Router;
    use roder_k8s::Backend;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn make_protected_app(state: AppState) -> Router {
        Router::new()
            .route("/protected", get(|| async { "secret" }))
            .route("/api/data", get(|| async { "json" }))
            .route_layer(from_fn_with_state(state, require_auth))
    }

    fn make_backend_asserting_app(state: AppState) -> Router {
        Router::new()
            .route(
                "/protected",
                get(|Extension(b): Extension<Arc<Backend>>| async move {
                    let _ = b.kinds();
                    "ok"
                }),
            )
            .route_layer(from_fn_with_state(state, require_auth))
    }

    fn make_identity_app(state: AppState) -> Router {
        Router::new()
            .route(
                "/identity",
                get(|Extension(identity): Extension<Identity>| async move { Json(identity) }),
            )
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
    async fn require_auth_dev_mode_inserts_dev_backend_extension() {
        // Dev mode resolves the single implicit dev backend into the request
        // extensions so downstream handlers can extract `Extension<Arc<Backend>>`.
        let app = make_backend_asserting_app(dev_state());
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
    async fn require_auth_prod_valid_session_inserts_user_backend_extension() {
        // A validly-sealed cookie resolves the subject's own backend (built via
        // the registry's #[cfg(test)] seam) into the request extensions.
        let state = prod_state_without_provider();
        let sealed = seal_session(&fake_tokens(), &TEST_KEY);
        let app = make_backend_asserting_app(state);
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
        // A validly-sealed cookie with a present, non-expired id token resolves
        // the subject's backend (via the registry's #[cfg(test)] build seam) and
        // passes through — no refresh needed.
        let state = prod_state_without_provider();
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
    async fn stale_cookie_groups_do_not_replace_resolved_identity() {
        let state = prod_state_without_provider();
        let mut current = fake_tokens();
        current.identity.subject = "alice".into();
        current.identity.groups = vec!["viewers".into()];
        state.backends.resolve_login(&current).await.unwrap();

        let mut stale = current.clone();
        stale.identity.groups = vec!["admins".into()];
        stale.refresh_token = Some("stale-refresh-token".into());
        let sealed = seal_session(&stale, &TEST_KEY);

        let response = make_identity_app(state)
            .oneshot(
                Request::builder()
                    .uri("/identity")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}={sealed}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let identity: Identity = serde_json::from_slice(&body).unwrap();
        assert_eq!(identity.groups, vec!["viewers"]);
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

    #[tokio::test]
    async fn public_listener_rejects_peer_headers() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn(reject_peer_headers));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(crate::server::ha::INTERNAL_HEADER, "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn peer_listener_requires_internal_marker() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn(require_peer_request));
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn peer_listener_accepts_internal_marker_after_transport_auth() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn(require_peer_request));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(crate::server::ha::INTERNAL_HEADER, "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
