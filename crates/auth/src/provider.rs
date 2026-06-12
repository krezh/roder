use std::future::Future;
use std::pin::Pin;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreIdTokenClaims, CoreProviderMetadata,
};
use openidconnect::{
    AsyncHttpClient, AuthorizationCode, ClientId, ClientSecret, CsrfToken, HttpClientError,
    HttpRequest, HttpResponse, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenResponse,
};
use time::OffsetDateTime;

use crate::error::{AuthError, Result};
use crate::{Identity, Tokens};

struct ReqwestClient(reqwest::Client);

impl<'c> AsyncHttpClient<'c> for ReqwestClient {
    type Error = HttpClientError<reqwest::Error>;

    #[cfg(not(target_arch = "wasm32"))]
    type Future = Pin<
        Box<
            dyn Future<Output = std::result::Result<HttpResponse, Self::Error>>
                + Send
                + Sync
                + 'c,
        >,
    >;
    #[cfg(target_arch = "wasm32")]
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<HttpResponse, Self::Error>> + 'c>>;

    fn call(&'c self, request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            let req: reqwest::Request = request.try_into().map_err(Box::new)?;
            let response = self.0.execute(req).await.map_err(Box::new)?;

            let mut builder =
                openidconnect::http::Response::builder().status(response.status());

            #[cfg(not(target_arch = "wasm32"))]
            {
                builder = builder.version(response.version());
            }

            for (name, value) in response.headers() {
                builder = builder.header(name, value);
            }

            builder
                .body(response.bytes().await.map_err(Box::new)?.to_vec())
                .map_err(HttpClientError::Http)
        })
    }
}

/// Static OIDC configuration (from env / Helm values).
#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    /// Empty = the default set (openid, email, profile, groups, offline_access).
    pub scopes: Vec<String>,
    /// Empty = allow any authenticated user.
    pub allowed_groups: Vec<String>,
    /// Empty = "groups".
    pub groups_claim: String,
}

impl OidcConfig {
    fn effective_scopes(&self) -> Vec<String> {
        if self.scopes.is_empty() {
            ["openid", "email", "profile", "groups", "offline_access"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            self.scopes.clone()
        }
    }
}

/// Values produced when a login begins; the caller stashes them in the session
/// and replays them on the OAuth callback.
#[derive(Debug, Clone)]
pub struct LoginStart {
    pub authorize_url: String,
    pub csrf_state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

/// Wraps discovered provider metadata + client credentials. The `openidconnect`
/// client carries endpoint state in its type, so we rebuild it (cheaply, no
/// network) inside each method rather than storing the unnameable type.
pub struct OidcProvider {
    metadata: CoreProviderMetadata,
    client_id: ClientId,
    client_secret: ClientSecret,
    redirect_uri: RedirectUrl,
    http: reqwest::Client,
    scopes: Vec<String>,
    allowed_groups: Vec<String>,
    groups_claim: String,
}

impl OidcProvider {
    /// Discover the provider's `.well-known/openid-configuration`.
    pub async fn discover(cfg: OidcConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            // SSRF hardening: never follow redirects on OIDC endpoints.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| AuthError::Config(e.to_string()))?;

        let issuer = IssuerUrl::new(cfg.issuer_url.clone())
            .map_err(|e| AuthError::Config(format!("issuer_url: {e}")))?;
        let metadata = CoreProviderMetadata::discover_async(issuer, &ReqwestClient(http.clone()))
            .await
            .map_err(|e| AuthError::Discovery(cfg.issuer_url.clone(), e.to_string()))?;
        let redirect_uri = RedirectUrl::new(cfg.redirect_url.clone())
            .map_err(|e| AuthError::Config(format!("redirect_url: {e}")))?;

        Ok(Self {
            metadata,
            client_id: ClientId::new(cfg.client_id.clone()),
            client_secret: ClientSecret::new(cfg.client_secret.clone()),
            redirect_uri,
            http,
            scopes: cfg.effective_scopes(),
            allowed_groups: cfg.allowed_groups,
            groups_claim: if cfg.groups_claim.is_empty() {
                "groups".to_string()
            } else {
                cfg.groups_claim
            },
        })
    }

    fn core_client(
        &self,
    ) -> CoreClient<
        openidconnect::EndpointSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointMaybeSet,
        openidconnect::EndpointMaybeSet,
    > {
        CoreClient::from_provider_metadata(
            self.metadata.clone(),
            self.client_id.clone(),
            Some(self.client_secret.clone()),
        )
        .set_redirect_uri(self.redirect_uri.clone())
    }

    /// Build the authorization URL (PKCE + state + nonce).
    pub fn begin_login(&self) -> LoginStart {
        let client = self.core_client();
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();

        let mut req = client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        // `openidconnect` always includes `openid`; add the rest.
        for s in self.scopes.iter().filter(|s| s.as_str() != "openid") {
            req = req.add_scope(Scope::new(s.clone()));
        }
        let (url, csrf, nonce) = req.set_pkce_challenge(challenge).url();

        LoginStart {
            authorize_url: url.to_string(),
            csrf_state: csrf.secret().clone(),
            nonce: nonce.secret().clone(),
            pkce_verifier: verifier.secret().clone(),
        }
    }

    /// Exchange the authorization code for tokens and verify the ID token against
    /// the expected nonce.
    pub async fn exchange_code(
        &self,
        code: String,
        pkce_verifier: String,
        nonce: String,
    ) -> Result<Tokens> {
        let client = self.core_client();
        let resp = client
            .exchange_code(AuthorizationCode::new(code))
            .map_err(|e| AuthError::Exchange(e.to_string()))?
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
            .request_async(&ReqwestClient(self.http.clone()))
            .await
            .map_err(|e| AuthError::Exchange(e.to_string()))?;

        let expected = Nonce::new(nonce);
        self.tokens_from_response(&client, resp, &expected)
    }

    /// Exchange a refresh token for a fresh ID token set.
    pub async fn refresh(&self, refresh_token: String) -> Result<Tokens> {
        let client = self.core_client();
        let resp = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token))
            .map_err(|e| AuthError::Refresh(e.to_string()))?
            .request_async(&ReqwestClient(self.http.clone()))
            .await
            .map_err(|e| AuthError::Refresh(e.to_string()))?;

        // Refreshed ID tokens carry no nonce; skip the nonce check.
        self.tokens_from_response_no_nonce(&client, resp)
    }

    fn tokens_from_response<C>(
        &self,
        client: &C,
        resp: openidconnect::core::CoreTokenResponse,
        expected_nonce: &Nonce,
    ) -> Result<Tokens>
    where
        C: HasVerifier,
    {
        let id_token = resp.id_token().ok_or(AuthError::MissingIdToken)?;
        let verifier = client.verifier();
        let claims = id_token
            .claims(&verifier, expected_nonce)
            .map_err(|e| AuthError::Verification(e.to_string()))?;
        self.assemble(resp.clone(), id_token.to_string(), claims)
    }

    fn tokens_from_response_no_nonce<C>(
        &self,
        client: &C,
        resp: openidconnect::core::CoreTokenResponse,
    ) -> Result<Tokens>
    where
        C: HasVerifier,
    {
        let id_token = resp.id_token().ok_or(AuthError::MissingIdToken)?;
        let verifier = client.verifier();
        let claims = id_token
            .claims(&verifier, |_: Option<&Nonce>| Ok::<(), String>(()))
            .map_err(|e| AuthError::Verification(e.to_string()))?;
        self.assemble(resp.clone(), id_token.to_string(), claims)
    }

    fn assemble(
        &self,
        resp: openidconnect::core::CoreTokenResponse,
        raw_id_token: String,
        claims: &CoreIdTokenClaims,
    ) -> Result<Tokens> {
        let groups = extract_groups(&raw_id_token, &self.groups_claim);
        let identity = Identity {
            subject: claims.subject().to_string(),
            email: claims.email().map(|e| e.to_string()),
            name: claims
                .name()
                .and_then(|n| n.get(None))
                .map(|n| n.as_str().to_string()),
            groups,
        };

        if !self.allowed_groups.is_empty()
            && !identity
                .groups
                .iter()
                .any(|g| self.allowed_groups.contains(g))
        {
            return Err(AuthError::Forbidden);
        }

        let expires_at = OffsetDateTime::from_unix_timestamp(claims.expiration().timestamp())
            .unwrap_or_else(|_| OffsetDateTime::now_utc());

        Ok(Tokens {
            id_token: raw_id_token,
            access_token: resp.access_token().secret().clone(),
            refresh_token: resp.refresh_token().map(|t| t.secret().clone()),
            expires_at,
            identity,
        })
    }
}

/// Decode the (already signature-verified) ID token payload and read a groups
/// claim, which may be an array or a single string.
fn extract_groups(raw_jwt: &str, claim: &str) -> Vec<String> {
    let Some(payload_b64) = raw_jwt.split('.').nth(1) else {
        return vec![];
    };
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(payload_b64) else {
        return vec![];
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return vec![];
    };
    match json.get(claim) {
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        _ => vec![],
    }
}

/// Small helper trait so the two response paths can share the verifier call
/// without naming the full client typestate.
trait HasVerifier {
    fn verifier(&self) -> openidconnect::core::CoreIdTokenVerifier<'_>;
}

impl<A, B, C, D, E, F> HasVerifier for CoreClient<A, B, C, D, E, F>
where
    A: openidconnect::EndpointState,
    B: openidconnect::EndpointState,
    C: openidconnect::EndpointState,
    D: openidconnect::EndpointState,
    E: openidconnect::EndpointState,
    F: openidconnect::EndpointState,
{
    fn verifier(&self) -> openidconnect::core::CoreIdTokenVerifier<'_> {
        self.id_token_verifier()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    #[test]
    fn effective_scopes_default_when_empty() {
        let cfg = OidcConfig {
            issuer_url: "https://x".into(),
            client_id: "c".into(),
            client_secret: "s".into(),
            redirect_url: "https://x/cb".into(),
            scopes: vec![],
            allowed_groups: vec![],
            groups_claim: "groups".into(),
        };
        assert_eq!(
            cfg.effective_scopes(),
            vec!["openid", "email", "profile", "groups", "offline_access"]
        );
    }

    #[test]
    fn effective_scopes_uses_custom_when_provided() {
        let cfg = OidcConfig {
            issuer_url: "https://x".into(),
            client_id: "c".into(),
            client_secret: "s".into(),
            redirect_url: "https://x/cb".into(),
            scopes: vec!["openid".into(), "email".into(), "custom_scope".into()],
            allowed_groups: vec![],
            groups_claim: "groups".into(),
        };
        assert_eq!(
            cfg.effective_scopes(),
            vec!["openid", "email", "custom_scope"]
        );
    }

    fn jwt_with_payload(claims: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{}");
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn extract_groups_array_claim() {
        let jwt = jwt_with_payload(serde_json::json!({
            "sub": "user-1",
            "groups": ["admins", "developers", "ops"],
        }));
        assert_eq!(
            extract_groups(&jwt, "groups"),
            vec!["admins", "developers", "ops"]
        );
    }

    #[test]
    fn extract_groups_string_claim() {
        let jwt = jwt_with_payload(serde_json::json!({"groups": "admins"}));
        assert_eq!(extract_groups(&jwt, "groups"), vec!["admins"]);
    }

    #[test]
    fn extract_groups_missing_claim_returns_empty() {
        let jwt = jwt_with_payload(serde_json::json!({"sub": "user-1"}));
        assert!(extract_groups(&jwt, "groups").is_empty());
    }

    #[test]
    fn extract_groups_wrong_claim_name_returns_empty() {
        let jwt = jwt_with_payload(serde_json::json!({"roles": ["admins"]}));
        assert!(extract_groups(&jwt, "groups").is_empty());
    }

    #[test]
    fn extract_groups_array_with_non_string_entries_skipped() {
        let jwt = jwt_with_payload(serde_json::json!({
            "groups": ["admins", 42, null, "developers"],
        }));
        assert_eq!(extract_groups(&jwt, "groups"), vec!["admins", "developers"]);
    }

    #[test]
    fn extract_groups_claim_wrong_type_returns_empty() {
        let jwt = jwt_with_payload(serde_json::json!({"groups": 42}));
        assert!(extract_groups(&jwt, "groups").is_empty());
    }

    #[test]
    fn extract_groups_malformed_jwt_returns_empty() {
        assert!(extract_groups("not-a-jwt", "groups").is_empty());
    }

    #[test]
    fn extract_groups_invalid_base64_returns_empty() {
        assert!(extract_groups("aaa.!!!.sig", "groups").is_empty());
    }

    #[test]
    fn extract_groups_invalid_json_returns_empty() {
        let bad = URL_SAFE_NO_PAD.encode(b"not json at all");
        let jwt = format!("aaa.{bad}.sig");
        assert!(extract_groups(&jwt, "groups").is_empty());
    }

    #[test]
    fn extract_groups_custom_claim_name() {
        let jwt = jwt_with_payload(serde_json::json!({
            "roles": ["a", "b"],
            "groups": ["ignored"],
        }));
        assert_eq!(extract_groups(&jwt, "roles"), vec!["a", "b"]);
    }
}
