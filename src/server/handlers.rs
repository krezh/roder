use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
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
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
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
                        [
                            (
                                header::SET_COOKIE,
                                set_cookie(SESSION_COOKIE, &sid, secure, 604800),
                            ),
                            (header::SET_COOKIE, set_cookie(LOGIN_COOKIE, "", secure, 0)),
                        ],
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
    // If a backend already exists (re-login / same user), just swap the token in.
    if let Some(backend) = state.backend.read().await.clone() {
        if backend.set_token(id_token).is_ok() {
            return Ok(());
        }
    }
    match Backend::connect_with_token(id_token).await {
        Ok(backend) => {
            *state.backend.write().await = Some(Arc::new(backend));
            Ok(())
        }
        Err(e) => Err((
            StatusCode::BAD_GATEWAY,
            format!("connected to OIDC but failed to reach the cluster: {e}"),
        )
            .into_response()),
    }
}
