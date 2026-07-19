//! The OIDC auth-code flow: login, callback, logout, and "who am I", plus
//! the plain liveness/readiness check.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use axum::Json;
use roder_auth::{AuthError, Identity};
use serde::Deserialize;

use super::super::session::{
    cookie_value, open_pending, open_session, seal_pending, seal_session, set_cookie, PendingLogin,
    LOGIN_COOKIE, SESSION_COOKIE,
};
use crate::server::AppState;

/// Liveness/readiness — always public.
pub async fn health() -> Json<roder_core::Health> {
    Json(roder_core::Health::ok())
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
                    // Establish (and validate cluster reachability for) this
                    // subject's own backend up front, so a hard connect failure
                    // is reported immediately rather than on the first API call.
                    if state.backends.resolve(&tokens).await.is_err() {
                        return (
                            StatusCode::BAD_GATEWAY,
                            "connected to OIDC but failed to reach the cluster",
                        )
                            .into_response();
                    }
                    let sealed = seal_session(&tokens, &key);

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
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Drop the caller's own cached backend (shedding their informers/watches).
    // Recover the subject from the session cookie; if we can't, we still clear
    // the cookie and redirect below.
    if let Some(key) = state.config.session_key {
        if let Some(cookie) = cookie_value(&headers, SESSION_COOKIE) {
            if let Some(tokens) = open_session(&cookie, &key) {
                state.backends.evict(&tokens.identity.subject).await;
            }
        }
    }
    let cookie = set_cookie(SESSION_COOKIE, "", state.config.secure_cookies(), 0);
    let dest = state
        .config
        .signout_redirect_url
        .as_deref()
        .unwrap_or("/auth/login");
    ([(header::SET_COOKIE, cookie)], Redirect::to(dest)).into_response()
}

/// Who am I — used by the UI to show the signed-in identity. `require_auth` has
/// already validated/refreshed the session, so the identity comes straight from
/// the sealed session cookie.
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
    //! Tests for `health`, `login`, `callback`, `logout`, and `me`.
    //!
    //! What we can exercise without a real OIDC provider:
    //! 1. The cookie-format contract (the regression we just fixed).
    //! 2. Dev-mode paths (`provider: None`) for `login`, `callback`, `me`.
    //! 3. `logout` (no provider needed).
    //!
    //! Out of scope: the actual OIDC code exchange, which needs a real IdP.

    use super::super::fixtures::*;
    use super::*;
    use axum::http::header::SET_COOKIE;
    use axum::http::HeaderValue;
    use std::sync::Arc;

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
    async fn logout_clears_session_cookie_and_evicts_the_backend() {
        let state = prod_state_without_provider();
        // Establish this subject's cached backend so we can verify logout evicts it.
        let mut tokens = fake_tokens();
        tokens.identity.subject = "logout-subject".into();
        state.backends.resolve(&tokens).await.unwrap();
        assert_eq!(state.backends.len().await, 1);

        // Logout recovers the subject from the (sealed) session cookie.
        let res = logout(State(state.clone()), sealed_cookie_header(&tokens)).await;
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

        // The cached backend for this subject must be gone.
        assert_eq!(state.backends.len().await, 0);
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
}
