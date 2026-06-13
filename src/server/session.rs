use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use axum::http::HeaderMap;
use base64::Engine;
use roder_auth::{Identity, Tokens};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub const SESSION_COOKIE: &str = "roder_session";
pub const LOGIN_COOKIE: &str = "roder_login";

/// In-flight login state stashed between the `/auth/login` redirect and the
/// `/auth/callback` return. Sealed into the `roder_login` cookie (not a
/// server-side store), so the callback can recover it on any replica and across
/// a restart — that store was the last thing making the login flow pod-sticky.
#[derive(Debug, Clone)]
pub struct PendingLogin {
    pub csrf_state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

/// What we persist in the sealed `roder_session` cookie. Deliberately *not* the
/// id token (a JWT — the largest field, and groups-heavy ones can push the
/// cookie past the ~4 KB browser limit) nor the access token (roder only ever
/// passes the *id* token to the apiserver). The refresh token is enough: on a
/// cold start the id token is re-minted from it, so the cookie stays small.
#[derive(Serialize, Deserialize)]
struct SealedSession {
    #[serde(default)]
    refresh_token: Option<String>,
    identity: Identity,
    /// id-token expiry as a unix timestamp (avoids serializing OffsetDateTime).
    expires_at_unix: i64,
}

/// The `roder_login` cookie payload: the login state plus an absolute expiry, so
/// a stale or replayed login cookie is rejected even within its browser Max-Age.
#[derive(Serialize, Deserialize)]
struct SealedPending {
    csrf_state: String,
    nonce: String,
    pkce_verifier: String,
    expires_at_unix: i64,
}

fn cipher(key: &[u8; 32]) -> Aes256Gcm {
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
}

/// AES-256-GCM seal of arbitrary bytes over a 12-byte random nonce, URL-safe
/// base64 of `nonce || ciphertext`. The shared primitive behind both sealed
/// cookies; any instance holding the same `RODER_SESSION_KEY` can open them, so
/// there is no server-side store to lose on restart or to make pods sticky.
fn seal_bytes(plain: &[u8], key: &[u8; 32]) -> String {
    let nonce_bytes: [u8; 12] = rand::random();
    let Ok(ct) = cipher(key).encrypt(Nonce::from_slice(&nonce_bytes), plain) else {
        return String::new();
    };
    let mut buf = Vec::with_capacity(nonce_bytes.len() + ct.len());
    buf.extend_from_slice(&nonce_bytes);
    buf.extend_from_slice(&ct);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Reverse of [`seal_bytes`]: `None` if malformed, sealed with a different key,
/// or GCM authentication fails (tampered).
fn open_bytes(cookie: &str, key: &[u8; 32]) -> Option<Vec<u8>> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cookie.as_bytes())
        .ok()?;
    // 12-byte nonce + 16-byte GCM tag minimum.
    if raw.len() < 28 {
        return None;
    }
    let (nonce, ct) = raw.split_at(12);
    cipher(key).decrypt(Nonce::from_slice(nonce), ct).ok()
}

/// Seal a token set into the `roder_session` cookie value.
pub fn seal_session(tokens: &Tokens, key: &[u8; 32]) -> String {
    let payload = SealedSession {
        refresh_token: tokens.refresh_token.clone(),
        identity: tokens.identity.clone(),
        expires_at_unix: tokens.expires_at.unix_timestamp(),
    };
    seal_bytes(&serde_json::to_vec(&payload).unwrap_or_default(), key)
}

/// Reverse of [`seal_session`]. The returned `id_token` is empty — it isn't
/// stored — so a cold-start path must refresh to obtain one (see `ensure_session`).
pub fn open_session(cookie: &str, key: &[u8; 32]) -> Option<Tokens> {
    let s: SealedSession = serde_json::from_slice(&open_bytes(cookie, key)?).ok()?;
    Some(Tokens {
        id_token: String::new(),
        access_token: String::new(),
        refresh_token: s.refresh_token,
        expires_at: OffsetDateTime::from_unix_timestamp(s.expires_at_unix).ok()?,
        identity: s.identity,
    })
}

/// Seal in-flight login state into the `roder_login` cookie, valid for `ttl_secs`.
pub fn seal_pending(pending: &PendingLogin, key: &[u8; 32], ttl_secs: i64) -> String {
    let payload = SealedPending {
        csrf_state: pending.csrf_state.clone(),
        nonce: pending.nonce.clone(),
        pkce_verifier: pending.pkce_verifier.clone(),
        expires_at_unix: OffsetDateTime::now_utc().unix_timestamp() + ttl_secs,
    };
    seal_bytes(&serde_json::to_vec(&payload).unwrap_or_default(), key)
}

/// Reverse of [`seal_pending`]; also rejects a login whose absolute expiry passed.
pub fn open_pending(cookie: &str, key: &[u8; 32]) -> Option<PendingLogin> {
    let s: SealedPending = serde_json::from_slice(&open_bytes(cookie, key)?).ok()?;
    if OffsetDateTime::now_utc().unix_timestamp() > s.expires_at_unix {
        return None;
    }
    Some(PendingLogin {
        csrf_state: s.csrf_state,
        nonce: s.nonce,
        pkce_verifier: s.pkce_verifier,
    })
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
    use base64::Engine;
    use roder_auth::{Identity, Tokens};
    use time::OffsetDateTime;

    // -------- set_cookie --------

    #[test]
    fn set_cookie_secure_emits_all_attrs() {
        let c = set_cookie("k", "v", true, 600);
        assert_eq!(
            c,
            "k=v; HttpOnly; SameSite=Lax; Path=/; Max-Age=600; Secure"
        );
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

    // -------- token / cookie helpers --------

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

    // -------- seal_session / open_session --------

    #[test]
    fn seal_open_round_trips_the_token_set() {
        let key = [42u8; 32];
        let mut tokens = fake_tokens(3600);
        tokens.identity.email = Some("a@b".into());
        tokens.identity.groups = vec!["admins".into()];

        let sealed = seal_session(&tokens, &key);
        assert!(!sealed.is_empty());

        let opened = open_session(&sealed, &key).expect("round trip");
        // id_token is intentionally not stored (re-minted on cold start).
        assert!(opened.id_token.is_empty());
        assert_eq!(opened.refresh_token, tokens.refresh_token);
        assert_eq!(opened.identity, tokens.identity);
        assert_eq!(
            opened.expires_at.unix_timestamp(),
            tokens.expires_at.unix_timestamp()
        );
    }

    #[test]
    fn open_with_wrong_key_fails() {
        let sealed = seal_session(&fake_tokens(3600), &[1u8; 32]);
        assert!(open_session(&sealed, &[2u8; 32]).is_none());
    }

    #[test]
    fn open_rejects_tampered_or_garbage_cookies() {
        assert!(open_session("not base64 $$$", &[0u8; 32]).is_none());
        assert!(open_session("", &[0u8; 32]).is_none());
        // Valid base64 but too short to be nonce+tag.
        assert!(open_session("AAAA", &[0u8; 32]).is_none());

        // Flip a byte of a real sealed cookie → GCM auth must reject it.
        let key = [9u8; 32];
        let sealed = seal_session(&fake_tokens(3600), &key);
        let mut bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&sealed)
            .unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        let tampered = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        assert!(open_session(&tampered, &key).is_none());
    }

    // -------- seal_pending / open_pending --------

    fn fake_pending() -> PendingLogin {
        PendingLogin {
            csrf_state: "csrf".into(),
            nonce: "nonce".into(),
            pkce_verifier: "verifier".into(),
        }
    }

    #[test]
    fn pending_seal_open_round_trips() {
        let key = [3u8; 32];
        let p = fake_pending();
        let sealed = seal_pending(&p, &key, 600);
        let opened = open_pending(&sealed, &key).expect("round trip");
        assert_eq!(opened.csrf_state, p.csrf_state);
        assert_eq!(opened.nonce, p.nonce);
        assert_eq!(opened.pkce_verifier, p.pkce_verifier);
    }

    #[test]
    fn pending_rejects_expired_login() {
        let key = [3u8; 32];
        // Negative TTL → already expired.
        let sealed = seal_pending(&fake_pending(), &key, -1);
        assert!(open_pending(&sealed, &key).is_none());
    }

    #[test]
    fn pending_rejects_wrong_key_and_garbage() {
        let sealed = seal_pending(&fake_pending(), &[1u8; 32], 600);
        assert!(open_pending(&sealed, &[2u8; 32]).is_none());
        assert!(open_pending("garbage", &[1u8; 32]).is_none());
    }
}
