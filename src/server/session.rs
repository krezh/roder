use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Instant;

use axum::http::HeaderMap;
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
        let mut map = self.inner.write().await;
        // Opportunistically evict sessions whose token has been expired for
        // >24 hours and cannot be refreshed (no refresh token). Sessions with
        // a refresh token are kept because the background refresh loop handles
        // them and removes them on failure.
        map.retain(|_, s| s.tokens.refresh_token.is_some() || !s.tokens.is_abandoned());
        map.insert(id, session);
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
    let bytes: [u8; 32] = rand::random();
    let mut s = String::with_capacity(64);
    for b in &bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use roder_auth::{Identity, Tokens};
    use std::time::Duration;
    use time::OffsetDateTime;

    // -------- set_cookie --------

    #[test]
    fn set_cookie_secure_emits_all_attrs() {
        let c = set_cookie("k", "v", true, 600);
        assert_eq!(c, "k=v; HttpOnly; SameSite=Lax; Path=/; Max-Age=600; Secure");
    }

    #[test]
    fn set_cookie_insecure_omits_secure_flag() {
        let c = set_cookie("k", "v", false, 600);
        assert_eq!(c, "k=v; HttpOnly; SameSite=Lax; Path=/; Max-Age=600");
        assert!(!c.contains("Secure;"));
    }

    #[test]
    fn set_cookie_max_age_zero_uses_zero() {
        // Value 0 means "expire immediately" — keep the flag, set Max-Age=0.
        let c = set_cookie("k", "", true, 0);
        assert!(c.contains("Max-Age=0"));
        assert!(c.starts_with("k=;"));
    }

    #[test]
    fn set_cookie_empty_value_keeps_equals_sign() {
        let c = set_cookie("roder_login", "", true, 0);
        assert!(c.starts_with("roder_login=;"));
    }

    #[test]
    fn set_cookie_large_max_age() {
        let c = set_cookie("k", "v", true, 604_800);
        assert!(c.contains("Max-Age=604800"));
    }

    #[test]
    fn set_cookie_attribute_order_is_deterministic() {
        // Browsers don't care, but we want a stable format for tests / docs.
        let c = set_cookie("a", "b", true, 60);
        let expected = "a=b; HttpOnly; SameSite=Lax; Path=/; Max-Age=60; Secure";
        assert_eq!(c, expected);
    }

    // -------- cookie_value --------

    fn make_cookie_header(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::COOKIE,
            axum::http::HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[test]
    fn cookie_value_single_cookie() {
        let h = make_cookie_header("roder_login=abc123");
        assert_eq!(cookie_value(&h, "roder_login").as_deref(), Some("abc123"));
    }

    #[test]
    fn cookie_value_multiple_cookies_picks_named() {
        let h = make_cookie_header("foo=1; roder_login=abc; bar=2");
        assert_eq!(cookie_value(&h, "roder_login").as_deref(), Some("abc"));
    }

    #[test]
    fn cookie_value_trims_whitespace_around_name_and_value() {
        let h = make_cookie_header("  roder_login  =  abc  ;  other=x");
        assert_eq!(cookie_value(&h, "roder_login").as_deref(), Some("abc"));
    }

    #[test]
    fn cookie_value_missing_returns_none() {
        let h = make_cookie_header("foo=1; bar=2");
        assert_eq!(cookie_value(&h, "roder_login"), None);
    }

    #[test]
    fn cookie_value_no_cookie_header_returns_none() {
        let h = HeaderMap::new();
        assert_eq!(cookie_value(&h, "roder_login"), None);
    }

    #[test]
    fn cookie_value_partial_match_does_not_count() {
        // `roder_login` and `roder_loginx` are different cookies.
        let h = make_cookie_header("roder_loginx=wrong; foo=1");
        assert_eq!(cookie_value(&h, "roder_login"), None);
    }

    #[test]
    fn cookie_value_preserves_internal_value_chars() {
        // Values may contain `=` (e.g. base64 padding). split_once stops at first `=`.
        let h = make_cookie_header("roder_login=YWJjMTIzZA==");
        assert_eq!(
            cookie_value(&h, "roder_login").as_deref(),
            Some("YWJjMTIzZA==")
        );
    }

    #[test]
    fn cookie_value_empty_value_is_some_empty_string() {
        // Important: server-side clearing sets `Max-Age=0`, value is "".
        let h = make_cookie_header("roder_login=");
        assert_eq!(cookie_value(&h, "roder_login").as_deref(), Some(""));
    }

    // -------- random_id --------

    #[test]
    fn random_id_is_64_hex_chars() {
        let id = random_id();
        assert_eq!(id.len(), 64, "32 bytes -> 64 hex chars, got {id}");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn random_id_unique_across_calls() {
        let a = random_id();
        let b = random_id();
        let c = random_id();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    // -------- SessionStore --------

    fn fake_tokens(expires_in_secs: i64) -> Tokens {
        Tokens {
            id_token: "id".into(),
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            expires_at: OffsetDateTime::now_utc() + time::Duration::seconds(expires_in_secs),
            identity: Identity {
                subject: "sub".into(),
                email: None,
                name: None,
                groups: vec![],
            },
        }
    }

    fn fake_session(expires_in_secs: i64) -> Session {
        Session {
            identity: Identity {
                subject: "sub".into(),
                email: Some("a@b".into()),
                name: None,
                groups: vec!["g".into()],
            },
            tokens: fake_tokens(expires_in_secs),
        }
    }

    #[tokio::test]
    async fn session_store_insert_get_contains_remove() {
        let store = SessionStore::default();
        let s = fake_session(3600);
        store.insert("sid".into(), s.clone()).await;

        assert!(store.contains("sid").await);
        assert!(!store.contains("other").await);
        let got = store.get("sid").await;
        assert!(got.is_some());
        assert_eq!(got.unwrap().identity.subject, "sub");

        store.remove("sid").await;
        assert!(!store.contains("sid").await);
        assert!(store.get("sid").await.is_none());
    }

    #[tokio::test]
    async fn session_store_needing_refresh_filters_by_expiry() {
        let store = SessionStore::default();
        // Within 1-min safety buffer (needs_refresh) + already-expired.
        store.insert("stale".into(), fake_session(-10)).await;
        store.insert("soon".into(), fake_session(30)).await;
        // Well outside the buffer.
        store.insert("fresh".into(), fake_session(3600)).await;
        // No refresh token — must not be picked up even if expired.
        let mut no_refresh = fake_session(-10);
        no_refresh.tokens.refresh_token = None;
        store.insert("no_rt".into(), no_refresh).await;

        let need = store.needing_refresh().await;
        let ids: Vec<&str> = need.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"stale"), "stale should need refresh: {ids:?}");
        assert!(ids.contains(&"soon"), "soon should need refresh: {ids:?}");
        assert!(!ids.contains(&"fresh"), "fresh must not need refresh: {ids:?}");
        assert!(
            !ids.contains(&"no_rt"),
            "no refresh_token must be skipped: {ids:?}"
        );
    }

    #[tokio::test]
    async fn session_store_update_tokens_replaces_in_place() {
        let store = SessionStore::default();
        store.insert("sid".into(), fake_session(3600)).await;

        let new_tokens = fake_tokens(7200);
        store.update_tokens("sid", new_tokens.clone()).await;

        let got = store.get("sid").await.unwrap();
        assert_eq!(got.tokens.expires_at, new_tokens.expires_at);
    }

    #[tokio::test]
    async fn session_store_update_tokens_missing_sid_is_noop() {
        let store = SessionStore::default();
        // No panic, no insert.
        store.update_tokens("nope", fake_tokens(60)).await;
        assert!(!store.contains("nope").await);
    }

    // -------- PendingStore --------

    #[tokio::test]
    async fn pending_store_insert_and_take_round_trip() {
        let store = PendingStore::default();
        let p = PendingLogin {
            csrf_state: "csrf".into(),
            nonce: "nonce".into(),
            pkce_verifier: "verifier".into(),
            created: Instant::now(),
        };
        store.insert("pid".into(), p.clone()).await;

        let taken = store.take("pid").await;
        assert!(taken.is_some());
        assert_eq!(taken.unwrap().csrf_state, "csrf");

        // Single-use: take again returns None.
        assert!(store.take("pid").await.is_none());
    }

    #[tokio::test]
    async fn pending_store_insert_drops_stale_entries() {
        // Seed with an old (manually-aged) pending login; the next insert should
        // garbage-collect anything older than 10 minutes.
        let store = PendingStore::default();
        let old = PendingLogin {
            csrf_state: "old-csrf".into(),
            nonce: "old-nonce".into(),
            pkce_verifier: "old-verifier".into(),
            // 11 minutes ago — over the 600s retention.
            created: Instant::now() - Duration::from_secs(11 * 60),
        };
        // Bypass insert's GC by writing directly through a fresh `inner` map.
        // We use a *new* entry, then a second one, to trigger GC.
        store.insert("new1".into(), old).await;
        let fresh = PendingLogin {
            csrf_state: "fresh".into(),
            nonce: "n".into(),
            pkce_verifier: "v".into(),
            created: Instant::now(),
        };
        store.insert("new2".into(), fresh).await;

        // After GC, "new1" must be gone.
        assert!(store.take("new1").await.is_none());
        // "new2" survives.
        assert!(store.take("new2").await.is_some());
    }
}
