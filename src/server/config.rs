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
    /// 32-byte key (from `RODER_SESSION_KEY`) used to seal the stateless session
    /// cookie. `None` in dev mode (auth is bypassed). Required in production so
    /// sessions survive a restart/reschedule — without a *stable* key across
    /// instances, every sealed cookie would be undecryptable after a restart.
    pub session_key: Option<[u8; 32]>,
    /// Optional URL to redirect to after logout (`RODER_SIGNOUT_REDIRECT_URL`).
    /// When set, the logout handler redirects here instead of `/auth/login` so
    /// the OIDC provider can also end the SSO session (RP-initiated logout).
    /// Example: `https://sso.example.com/application/o/roder/end-session/`
    pub signout_redirect_url: Option<String>,
    /// Empty means any authenticated user may read Talos status.
    pub talos_reader_groups: Vec<String>,
    /// Mutations require both this group allow-list and `talos_actions_enabled`.
    pub talos_operator_groups: Vec<String>,
    pub talos_actions_enabled: bool,
    pub talos_config_groups: Vec<String>,
    pub talos_config_enabled: bool,
    /// Downward-API node name; coordinated power actions reject their own host.
    pub pod_node_name: Option<String>,
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

        let base_url = std::env::var("RODER_BASE_URL")
            .unwrap_or_else(|_| "http://0.0.0.0:8080".to_string())
            .trim_end_matches('/')
            .to_string();

        let signout_redirect_url = std::env::var("RODER_SIGNOUT_REDIRECT_URL").ok();
        let talos_reader_groups = env_groups("RODER_TALOS_READER_GROUPS");
        let talos_operator_groups = env_groups("RODER_TALOS_OPERATOR_GROUPS");
        let talos_actions_enabled = env_bool("RODER_TALOS_ACTIONS_ENABLED");
        let talos_config_groups = env_groups("RODER_TALOS_CONFIG_GROUPS");
        let talos_config_enabled = env_bool("RODER_TALOS_CONFIG_ENABLED");
        let pod_node_name = std::env::var("RODER_POD_NODE_NAME").ok();

        if dev_mode {
            return Ok(Self {
                dev_mode,
                base_url,
                oidc: None,
                session_key: None,
                signout_redirect_url,
                talos_reader_groups,
                talos_operator_groups,
                talos_actions_enabled,
                talos_config_groups,
                talos_config_enabled,
                pod_node_name,
            });
        }

        let session_key = Some(parse_session_key(&require_env("RODER_SESSION_KEY")?)?);

        let oidc = OidcSettings {
            issuer_url: require_env("RODER_OIDC_ISSUER_URL")?,
            client_id: require_env("RODER_OIDC_CLIENT_ID")?,
            client_secret: require_env("RODER_OIDC_CLIENT_SECRET")?,
            allowed_groups: std::env::var("RODER_OIDC_ALLOWED_GROUPS")
                .ok()
                .map(|s| split_csv(&s))
                .unwrap_or_default(),
            groups_claim: std::env::var("RODER_OIDC_GROUPS_CLAIM")
                .unwrap_or_else(|_| "groups".to_string()),
        };

        Ok(Self {
            dev_mode,
            base_url,
            oidc: Some(oidc),
            session_key,
            signout_redirect_url,
            talos_reader_groups,
            talos_operator_groups,
            talos_actions_enabled,
            talos_config_groups,
            talos_config_enabled,
            pod_node_name,
        })
    }

    /// Cookies are marked `Secure` unless we're in dev mode (plain http).
    pub fn secure_cookies(&self) -> bool {
        !self.dev_mode
    }

    pub fn redirect_url(&self) -> String {
        format!("{}/auth/callback", self.base_url)
    }

    pub fn can_read_talos(&self, groups: &[String]) -> bool {
        self.dev_mode
            || self.talos_reader_groups.is_empty()
            || has_any_group(groups, &self.talos_reader_groups)
    }

    pub fn can_operate_talos(&self, groups: &[String]) -> bool {
        self.talos_actions_enabled
            && (self.dev_mode || has_any_group(groups, &self.talos_operator_groups))
    }

    pub fn can_read_talos_config(&self, groups: &[String]) -> bool {
        self.talos_config_enabled
            && (self.dev_mode || has_any_group(groups, &self.talos_config_groups))
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

/// Parse `RODER_SESSION_KEY` into exactly 32 bytes. Accepts 64-char hex or
/// base64 (standard or URL-safe). Generate one with e.g.
/// `openssl rand -hex 32` or `head -c32 /dev/urandom | base64`.
fn parse_session_key(s: &str) -> Result<[u8; 32], String> {
    use base64::Engine;
    let s = s.trim();
    let bytes: Vec<u8> = if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        (0..32)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("invalid hex RODER_SESSION_KEY: {e}"))?
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(s)
            .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s))
            .map_err(|_| {
                "RODER_SESSION_KEY must be 32 bytes encoded as hex or base64".to_string()
            })?
    };
    if bytes.len() != 32 {
        return Err(format!(
            "RODER_SESSION_KEY must decode to 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

fn env_groups(key: &str) -> Vec<String> {
    let Ok(value) = std::env::var(key) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&value).unwrap_or_else(|_| split_csv(&value))
}

fn env_bool(key: &str) -> bool {
    matches!(
        std::env::var(key).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

fn has_any_group(actual: &[String], allowed: &[String]) -> bool {
    !allowed.is_empty() && actual.iter().any(|group| allowed.contains(group))
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
            "RODER_BASE_URL",
            "RODER_OIDC_ISSUER_URL",
            "RODER_OIDC_CLIENT_ID",
            "RODER_OIDC_CLIENT_SECRET",
            "RODER_OIDC_ALLOWED_GROUPS",
            "RODER_OIDC_GROUPS_CLAIM",
            "RODER_SESSION_KEY",
            "RODER_SIGNOUT_REDIRECT_URL",
            "RODER_TALOS_READER_GROUPS",
            "RODER_TALOS_OPERATOR_GROUPS",
            "RODER_TALOS_ACTIONS_ENABLED",
            "RODER_TALOS_CONFIG_GROUPS",
            "RODER_TALOS_CONFIG_ENABLED",
            "RODER_POD_NODE_NAME",
        ]);
        g.unset("RODER_DEV_MODE");
        g.set("RODER_BASE_URL", "https://roder.example.com");
        g.set("RODER_OIDC_ISSUER_URL", "https://accounts.google.com");
        g.set("RODER_OIDC_CLIENT_ID", "client-123");
        g.set("RODER_OIDC_CLIENT_SECRET", "secret-abc");
        // 64 hex chars = 32 bytes.
        g.set(
            "RODER_SESSION_KEY",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        );
        g.unset("RODER_TALOS_READER_GROUPS");
        g.unset("RODER_TALOS_OPERATOR_GROUPS");
        g.unset("RODER_TALOS_ACTIONS_ENABLED");
        g.unset("RODER_TALOS_CONFIG_GROUPS");
        g.unset("RODER_TALOS_CONFIG_ENABLED");
        g.unset("RODER_POD_NODE_NAME");
        g
    }

    // -------- dev-mode bypass --------

    #[test]
    #[serial]
    fn from_env_dev_mode_bypasses_oidc() {
        let g = EnvGuard::new(&[
            "RODER_DEV_MODE",
            "RODER_OIDC_ISSUER_URL",
            "RODER_OIDC_CLIENT_ID",
            "RODER_OIDC_CLIENT_SECRET",
        ]);
        g.set("RODER_DEV_MODE", "1");
        g.unset("RODER_OIDC_ISSUER_URL");
        g.unset("RODER_OIDC_CLIENT_ID");
        g.unset("RODER_OIDC_CLIENT_SECRET");

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
    fn from_env_requires_session_key() {
        let g = prod_env();
        g.unset("RODER_SESSION_KEY");
        let err = ServerConfig::from_env().unwrap_err();
        assert!(err.contains("RODER_SESSION_KEY"), "got: {err}");
    }

    #[test]
    #[serial]
    fn from_env_rejects_short_session_key() {
        let g = prod_env();
        g.set("RODER_SESSION_KEY", "tooshort");
        let err = ServerConfig::from_env().unwrap_err();
        assert!(err.contains("RODER_SESSION_KEY"), "got: {err}");
    }

    #[test]
    #[serial]
    fn from_env_accepts_base64_session_key() {
        use base64::Engine;
        let key = [7u8; 32];
        let encoded = base64::engine::general_purpose::STANDARD.encode(key);
        let g = prod_env();
        g.set("RODER_SESSION_KEY", &encoded);
        let cfg = ServerConfig::from_env().expect("valid base64 key");
        assert_eq!(cfg.session_key, Some(key));
    }

    #[test]
    #[serial]
    fn from_env_requires_issuer_url() {
        let g = prod_env();
        g.unset("RODER_OIDC_ISSUER_URL");
        let err = ServerConfig::from_env().unwrap_err();
        assert!(err.contains("RODER_OIDC_ISSUER_URL"), "got: {err}");
    }

    #[test]
    #[serial]
    fn from_env_requires_client_id() {
        let g = prod_env();
        g.unset("RODER_OIDC_CLIENT_ID");
        let err = ServerConfig::from_env().unwrap_err();
        assert!(err.contains("RODER_OIDC_CLIENT_ID"), "got: {err}");
    }

    #[test]
    #[serial]
    fn from_env_requires_client_secret() {
        let g = prod_env();
        g.unset("RODER_OIDC_CLIENT_SECRET");
        let err = ServerConfig::from_env().unwrap_err();
        assert!(err.contains("RODER_OIDC_CLIENT_SECRET"), "got: {err}");
    }

    // -------- RODER_BASE_URL handling --------

    #[test]
    #[serial]
    fn from_env_strips_trailing_slash_from_base_url() {
        let g = prod_env();
        g.set("RODER_BASE_URL", "https://roder.example.com/");
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
        g.set("RODER_BASE_URL", "https://roder.example.com///");
        let cfg = ServerConfig::from_env().unwrap();
        // trim_end_matches('/') strips all trailing slashes — bare host remains.
        assert_eq!(cfg.base_url, "https://roder.example.com");
    }

    #[test]
    #[serial]
    fn from_env_defaults_base_url() {
        let g = prod_env();
        g.unset("RODER_BASE_URL");
        let cfg = ServerConfig::from_env().unwrap();
        assert_eq!(cfg.base_url, "http://0.0.0.0:8080");
        assert_eq!(cfg.redirect_url(), "http://0.0.0.0:8080/auth/callback");
    }

    #[test]
    #[serial]
    fn from_env_base_url_with_path_preserved() {
        let g = prod_env();
        g.set("RODER_BASE_URL", "https://example.com/roder");
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
        g.set("RODER_OIDC_ALLOWED_GROUPS", "admins, devs , ,operators");
        let cfg = ServerConfig::from_env().unwrap();
        let o = cfg.oidc.unwrap();
        assert_eq!(o.allowed_groups, vec!["admins", "devs", "operators"]);
    }

    #[test]
    #[serial]
    fn from_env_empty_allowed_groups_means_open() {
        let g = prod_env();
        g.unset("RODER_OIDC_ALLOWED_GROUPS");
        let cfg = ServerConfig::from_env().unwrap();
        let o = cfg.oidc.unwrap();
        assert!(o.allowed_groups.is_empty());
    }

    #[test]
    #[serial]
    fn from_env_defaults_groups_claim() {
        let g = prod_env();
        g.unset("RODER_OIDC_GROUPS_CLAIM");
        let cfg = ServerConfig::from_env().unwrap();
        let o = cfg.oidc.unwrap();
        assert_eq!(o.groups_claim, "groups");
    }

    #[test]
    #[serial]
    fn from_env_uses_custom_groups_claim() {
        let g = prod_env();
        g.set("RODER_OIDC_GROUPS_CLAIM", "roles");
        let cfg = ServerConfig::from_env().unwrap();
        let o = cfg.oidc.unwrap();
        assert_eq!(o.groups_claim, "roles");
    }

    #[test]
    #[serial]
    fn talos_reader_groups_restrict_read_access() {
        let g = prod_env();
        g.set("RODER_TALOS_READER_GROUPS", "platform,talos-readers");
        let cfg = ServerConfig::from_env().unwrap();
        assert!(cfg.can_read_talos(&["platform".into()]));
        assert!(!cfg.can_read_talos(&["developers".into()]));
    }

    #[test]
    #[serial]
    fn talos_operator_requires_switch_and_group() {
        let g = prod_env();
        g.set("RODER_TALOS_OPERATOR_GROUPS", "platform-operators");
        let groups = vec!["platform-operators".into()];
        let disabled = ServerConfig::from_env().unwrap();
        assert!(!disabled.can_operate_talos(&groups));
        g.set("RODER_TALOS_ACTIONS_ENABLED", "true");
        let enabled = ServerConfig::from_env().unwrap();
        assert!(enabled.can_operate_talos(&groups));
        assert!(!enabled.can_operate_talos(&["developers".into()]));
    }

    #[test]
    #[serial]
    fn talos_groups_accept_json_without_splitting_group_names() {
        let g = prod_env();
        g.set("RODER_TALOS_READER_GROUPS", r#"["team,ops"]"#);
        let cfg = ServerConfig::from_env().unwrap();
        assert_eq!(cfg.talos_reader_groups, vec!["team,ops"]);
    }

    #[test]
    #[serial]
    fn talos_config_requires_explicit_switch_and_group() {
        let g = prod_env();
        g.set("RODER_TALOS_CONFIG_GROUPS", r#"["platform-admins"]"#);
        let groups = vec!["platform-admins".into()];
        assert!(!ServerConfig::from_env()
            .unwrap()
            .can_read_talos_config(&groups));
        g.set("RODER_TALOS_CONFIG_ENABLED", "true");
        let cfg = ServerConfig::from_env().unwrap();
        assert!(cfg.can_read_talos_config(&groups));
        assert!(!cfg.can_read_talos_config(&["developers".into()]));
    }

    #[test]
    #[serial]
    fn from_env_propagates_oidc_settings() {
        let g = prod_env();
        g.set("RODER_OIDC_ISSUER_URL", "https://login.example.com");
        g.set("RODER_OIDC_CLIENT_ID", "my-client");
        g.set("RODER_OIDC_CLIENT_SECRET", "my-secret");
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
        g.set("RODER_BASE_URL", "https://r.example.com");
        let cfg = ServerConfig::from_env().unwrap();
        let oc = cfg.oidc_config();
        assert_eq!(oc.redirect_url, "https://r.example.com/auth/callback");
        assert_eq!(oc.issuer_url, cfg.oidc.as_ref().unwrap().issuer_url);
        assert_eq!(oc.client_id, cfg.oidc.as_ref().unwrap().client_id);
        // Default scopes empty — the auth crate fills them in.
        assert!(oc.scopes.is_empty());
    }
}
