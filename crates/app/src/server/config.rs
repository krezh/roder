use roder_auth::OidcConfig;

/// Server configuration assembled from environment variables.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// `RODER_DEV_MODE=1` — skip OIDC entirely and use the inferred kubeconfig
    /// credentials (for local dev against a kind cluster that doesn't validate
    /// OIDC tokens). Cookies are not marked `Secure` in dev.
    pub dev_mode: bool,
    /// Public base URL, e.g. `https://roder.example.com`. The OIDC redirect URL
    /// is `<base_url>/auth/callback`.
    pub base_url: String,
    /// OIDC settings (None in dev mode).
    pub oidc: Option<OidcSettings>,
}

#[derive(Debug, Clone)]
pub struct OidcSettings {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub allowed_groups: Vec<String>,
    pub groups_claim: String,
}

impl ServerConfig {
    /// Load from the environment. Returns an error string describing the first
    /// missing required variable.
    pub fn from_env() -> Result<Self, String> {
        let dev_mode = matches!(
            std::env::var("RODER_DEV_MODE").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        );

        let base_url = std::env::var("BASE_URL")
            .unwrap_or_else(|_| "http://0.0.0.0:8080".to_string())
            .trim_end_matches('/')
            .to_string();

        if dev_mode {
            return Ok(Self {
                dev_mode,
                base_url,
                oidc: None,
            });
        }

        let oidc = OidcSettings {
            issuer_url: require_env("OIDC_ISSUER_URL")?,
            client_id: require_env("OIDC_CLIENT_ID")?,
            client_secret: require_env("OIDC_CLIENT_SECRET")?,
            allowed_groups: std::env::var("OIDC_ALLOWED_GROUPS")
                .ok()
                .map(|s| split_csv(&s))
                .unwrap_or_default(),
            groups_claim: std::env::var("OIDC_GROUPS_CLAIM").unwrap_or_else(|_| "groups".to_string()),
        };

        Ok(Self {
            dev_mode,
            base_url,
            oidc: Some(oidc),
        })
    }

    /// Cookies are marked `Secure` unless we're in dev mode (plain http).
    pub fn secure_cookies(&self) -> bool {
        !self.dev_mode
    }

    pub fn redirect_url(&self) -> String {
        format!("{}/auth/callback", self.base_url)
    }

    /// Convert to the `roder_auth` config (panics if called in dev mode).
    pub fn oidc_config(&self) -> OidcConfig {
        let o = self.oidc.as_ref().expect("oidc settings in non-dev mode");
        OidcConfig {
            issuer_url: o.issuer_url.clone(),
            client_id: o.client_id.clone(),
            client_secret: o.client_secret.clone(),
            redirect_url: self.redirect_url(),
            scopes: Vec::new(),
            allowed_groups: o.allowed_groups.clone(),
            groups_claim: o.groups_claim.clone(),
        }
    }
}

fn require_env(key: &str) -> Result<String, String> {
    std::env::var(key).map_err(|_| format!("missing required env var {key}"))
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}
