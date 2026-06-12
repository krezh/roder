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
            groups_claim: std::env::var("OIDC_GROUPS_CLAIM")
                .unwrap_or_else(|_| "groups".to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // The `from_env` function reads process-global state, so all env-var tests
    // are gated with `#[serial]` to avoid races between cargo's parallel test
    // threads. Each test cleans up after itself via a `Drop` guard.

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            Self {
                saved: keys.iter().map(|k| (*k, std::env::var(k).ok())).collect(),
            }
        }
        fn set(&self, key: &'static str, value: &str) {
            std::env::set_var(key, value);
        }
        fn unset(&self, key: &'static str) {
            std::env::remove_var(key);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in self.saved.drain(..) {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    fn prod_env() -> EnvGuard {
        let g = EnvGuard::new(&[
            "RODER_DEV_MODE",
            "BASE_URL",
            "OIDC_ISSUER_URL",
            "OIDC_CLIENT_ID",
            "OIDC_CLIENT_SECRET",
            "OIDC_ALLOWED_GROUPS",
            "OIDC_GROUPS_CLAIM",
        ]);
        g.unset("RODER_DEV_MODE");
        g.set("BASE_URL", "https://roder.example.com");
        g.set("OIDC_ISSUER_URL", "https://accounts.google.com");
        g.set("OIDC_CLIENT_ID", "client-123");
        g.set("OIDC_CLIENT_SECRET", "secret-abc");
        g
    }

    // -------- dev-mode bypass --------

    #[test]
    #[serial]
    fn from_env_dev_mode_bypasses_oidc() {
        let g = EnvGuard::new(&[
            "RODER_DEV_MODE",
            "OIDC_ISSUER_URL",
            "OIDC_CLIENT_ID",
            "OIDC_CLIENT_SECRET",
        ]);
        g.set("RODER_DEV_MODE", "1");
        g.unset("OIDC_ISSUER_URL");
        g.unset("OIDC_CLIENT_ID");
        g.unset("OIDC_CLIENT_SECRET");

        let cfg = ServerConfig::from_env().expect("dev mode should not require OIDC vars");
        assert!(cfg.dev_mode);
        assert!(cfg.oidc.is_none());
        assert!(!cfg.secure_cookies());
    }

    #[test]
    #[serial]
    fn from_env_dev_mode_accepts_truthy_variants() {
        for val in ["1", "true", "yes"] {
            let g = EnvGuard::new(&["RODER_DEV_MODE"]);
            g.set("RODER_DEV_MODE", val);
            let cfg = ServerConfig::from_env().expect("any truthy value should enable dev mode");
            assert!(cfg.dev_mode, "RODER_DEV_MODE={val} should enable dev mode");
        }
    }

    #[test]
    #[serial]
    fn from_env_empty_dev_mode_is_production() {
        let g = prod_env();
        g.unset("RODER_DEV_MODE");
        let cfg = ServerConfig::from_env().expect("all OIDC vars set");
        assert!(!cfg.dev_mode);
    }

    // -------- required vars in production --------

    #[test]
    #[serial]
    fn from_env_requires_issuer_url() {
        let g = prod_env();
        g.unset("OIDC_ISSUER_URL");
        let err = ServerConfig::from_env().unwrap_err();
        assert!(err.contains("OIDC_ISSUER_URL"), "got: {err}");
    }

    #[test]
    #[serial]
    fn from_env_requires_client_id() {
        let g = prod_env();
        g.unset("OIDC_CLIENT_ID");
        let err = ServerConfig::from_env().unwrap_err();
        assert!(err.contains("OIDC_CLIENT_ID"), "got: {err}");
    }

    #[test]
    #[serial]
    fn from_env_requires_client_secret() {
        let g = prod_env();
        g.unset("OIDC_CLIENT_SECRET");
        let err = ServerConfig::from_env().unwrap_err();
        assert!(err.contains("OIDC_CLIENT_SECRET"), "got: {err}");
    }

    // -------- BASE_URL handling --------

    #[test]
    #[serial]
    fn from_env_strips_trailing_slash_from_base_url() {
        let g = prod_env();
        g.set("BASE_URL", "https://roder.example.com/");
        let cfg = ServerConfig::from_env().unwrap();
        assert_eq!(cfg.base_url, "https://roder.example.com");
        assert_eq!(
            cfg.redirect_url(),
            "https://roder.example.com/auth/callback"
        );
    }

    #[test]
    #[serial]
    fn from_env_strips_multiple_trailing_slashes() {
        let g = prod_env();
        g.set("BASE_URL", "https://roder.example.com///");
        let cfg = ServerConfig::from_env().unwrap();
        // trim_end_matches('/') strips all trailing slashes — bare host remains.
        assert_eq!(cfg.base_url, "https://roder.example.com");
    }

    #[test]
    #[serial]
    fn from_env_defaults_base_url() {
        let g = prod_env();
        g.unset("BASE_URL");
        let cfg = ServerConfig::from_env().unwrap();
        assert_eq!(cfg.base_url, "http://0.0.0.0:8080");
        assert_eq!(cfg.redirect_url(), "http://0.0.0.0:8080/auth/callback");
    }

    #[test]
    #[serial]
    fn from_env_base_url_with_path_preserved() {
        let g = prod_env();
        g.set("BASE_URL", "https://example.com/roder");
        let cfg = ServerConfig::from_env().unwrap();
        assert_eq!(cfg.base_url, "https://example.com/roder");
        // NOTE: trim_end_matches('/') only strips trailing slashes, not the
        // entire path. The callback then appends /auth/callback, which gives
        // /roder/auth/callback — correct for path-based deployments.
        assert_eq!(
            cfg.redirect_url(),
            "https://example.com/roder/auth/callback"
        );
    }

    // -------- OIDC option parsing --------

    #[test]
    #[serial]
    fn from_env_parses_allowed_groups_as_csv() {
        let g = prod_env();
        g.set("OIDC_ALLOWED_GROUPS", "admins, devs , ,operators");
        let cfg = ServerConfig::from_env().unwrap();
        let o = cfg.oidc.unwrap();
        assert_eq!(o.allowed_groups, vec!["admins", "devs", "operators"]);
    }

    #[test]
    #[serial]
    fn from_env_empty_allowed_groups_means_open() {
        let g = prod_env();
        g.unset("OIDC_ALLOWED_GROUPS");
        let cfg = ServerConfig::from_env().unwrap();
        let o = cfg.oidc.unwrap();
        assert!(o.allowed_groups.is_empty());
    }

    #[test]
    #[serial]
    fn from_env_defaults_groups_claim() {
        let g = prod_env();
        g.unset("OIDC_GROUPS_CLAIM");
        let cfg = ServerConfig::from_env().unwrap();
        let o = cfg.oidc.unwrap();
        assert_eq!(o.groups_claim, "groups");
    }

    #[test]
    #[serial]
    fn from_env_uses_custom_groups_claim() {
        let g = prod_env();
        g.set("OIDC_GROUPS_CLAIM", "roles");
        let cfg = ServerConfig::from_env().unwrap();
        let o = cfg.oidc.unwrap();
        assert_eq!(o.groups_claim, "roles");
    }

    #[test]
    #[serial]
    fn from_env_propagates_oidc_settings() {
        let g = prod_env();
        g.set("OIDC_ISSUER_URL", "https://login.example.com");
        g.set("OIDC_CLIENT_ID", "my-client");
        g.set("OIDC_CLIENT_SECRET", "my-secret");
        let cfg = ServerConfig::from_env().unwrap();
        let o = cfg.oidc.unwrap();
        assert_eq!(o.issuer_url, "https://login.example.com");
        assert_eq!(o.client_id, "my-client");
        assert_eq!(o.client_secret, "my-secret");
    }

    // -------- secure_cookies / oidc_config --------

    #[test]
    #[serial]
    fn secure_cookies_true_in_production() {
        let g = prod_env();
        g.unset("RODER_DEV_MODE");
        let cfg = ServerConfig::from_env().unwrap();
        assert!(cfg.secure_cookies());
    }

    #[test]
    #[serial]
    fn oidc_config_uses_redirect_url() {
        let g = prod_env();
        g.set("BASE_URL", "https://r.example.com");
        let cfg = ServerConfig::from_env().unwrap();
        let oc = cfg.oidc_config();
        assert_eq!(oc.redirect_url, "https://r.example.com/auth/callback");
        assert_eq!(oc.issuer_url, cfg.oidc.as_ref().unwrap().issuer_url);
        assert_eq!(oc.client_id, cfg.oidc.as_ref().unwrap().client_id);
        // Default scopes empty — the auth crate fills them in.
        assert!(oc.scopes.is_empty());
    }
}
