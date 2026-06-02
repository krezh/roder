use std::collections::HashMap;
use std::time::Instant;

use axum::http::HeaderMap;
use rand::RngCore;
use roder_auth::{Identity, Tokens};
use tokio::sync::RwLock;

pub const SESSION_COOKIE: &str = "roder_session";
pub const LOGIN_COOKIE: &str = "roder_login";

/// A logged-in session. Tokens (incl. the passthrough ID token + refresh token)
/// live server-side; the browser only holds an opaque session id.
#[derive(Debug, Clone)]
pub struct Session {
    pub identity: Identity,
    pub tokens: Tokens,
}

/// In-flight login: the CSRF state / nonce / PKCE verifier stashed between the
/// `/auth/login` redirect and the `/auth/callback` return.
#[derive(Debug, Clone)]
pub struct PendingLogin {
    pub csrf_state: String,
    pub nonce: String,
    pub pkce_verifier: String,
    pub created: Instant,
}

#[derive(Default)]
pub struct SessionStore {
    inner: RwLock<HashMap<String, Session>>,
}

impl SessionStore {
    pub async fn insert(&self, id: String, session: Session) {
        self.inner.write().await.insert(id, session);
    }

    pub async fn get(&self, id: &str) -> Option<Session> {
        self.inner.read().await.get(id).cloned()
    }

    pub async fn contains(&self, id: &str) -> bool {
        self.inner.read().await.contains_key(id)
    }

    pub async fn remove(&self, id: &str) {
        self.inner.write().await.remove(id);
    }

    /// (session_id, refresh_token) for sessions whose ID token is near expiry.
    pub async fn needing_refresh(&self) -> Vec<(String, String)> {
        self.inner
            .read()
            .await
            .iter()
            .filter(|(_, s)| s.tokens.needs_refresh())
            .filter_map(|(id, s)| s.tokens.refresh_token.clone().map(|rt| (id.clone(), rt)))
            .collect()
    }

    pub async fn update_tokens(&self, id: &str, tokens: Tokens) {
        if let Some(s) = self.inner.write().await.get_mut(id) {
            s.identity = tokens.identity.clone();
            s.tokens = tokens;
        }
    }
}

#[derive(Default)]
pub struct PendingStore {
    inner: RwLock<HashMap<String, PendingLogin>>,
}

impl PendingStore {
    pub async fn insert(&self, id: String, pending: PendingLogin) {
        let mut map = self.inner.write().await;
        // Opportunistically drop stale pending logins (>10 min).
        map.retain(|_, p| p.created.elapsed().as_secs() < 600);
        map.insert(id, pending);
    }

    /// Take (remove + return) a pending login — single use.
    pub async fn take(&self, id: &str) -> Option<PendingLogin> {
        self.inner.write().await.remove(id)
    }
}

/// 256 bits of randomness, hex-encoded — used for opaque session/login ids.
pub fn random_id() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read a single cookie value from the request headers.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookies = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k.trim() == name).then(|| v.trim().to_string())
    })
}

/// Build a `Set-Cookie` value. `max_age` of 0 expires the cookie immediately.
pub fn set_cookie(name: &str, value: &str, secure: bool, max_age: i64) -> String {
    let mut c = format!("{name}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}");
    if secure {
        c.push_str("; Secure");
    }
    c
}
