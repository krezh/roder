//! HTTP handlers for the app shell: security headers and the auth-guarding
//! middleware live in `middleware`; the OIDC login/callback/logout/identity
//! endpoints live in `oidc`.

mod middleware;
mod oidc;

pub use middleware::{reject_peer_headers, require_auth, require_peer_request, security_headers};
pub use oidc::{callback, health, login, logout, me, CallbackParams};

/// Shared test fixtures (fake `AppState`s, tokens, cookie helpers) used by
/// both `middleware::tests` and `oidc::tests`.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::sync::Arc;

    use axum::http::{header, HeaderMap, HeaderValue};
    use leptos::prelude::LeptosOptions;
    use roder_auth::Identity;
    use time::OffsetDateTime;
    use tokio::sync::{Mutex, RwLock};

    use crate::server::config::ServerConfig;
    use crate::server::session::{seal_session, SESSION_COOKIE};
    use crate::server::AppState;

    pub(crate) const TEST_KEY: [u8; 32] = [7u8; 32];

    pub(crate) fn dev_config() -> Arc<ServerConfig> {
        Arc::new(ServerConfig {
            dev_mode: true,
            base_url: "http://localhost:8080".into(),
            oidc: None,
            session_key: None,
            signout_redirect_url: None,
            talos_reader_groups: vec![],
            talos_operator_groups: vec![],
            talos_actions_enabled: false,
            talos_config_groups: vec![],
            talos_config_enabled: false,
            alerts_groups: vec![],
            alerts_actions_enabled: false,
            alerts_operator_groups: vec![],
            pod_node_name: None,
            max_user_backends: 200,
            backend_idle_secs: 1200,
        })
    }

    pub(crate) fn prod_config() -> Arc<ServerConfig> {
        Arc::new(ServerConfig {
            dev_mode: false,
            base_url: "https://roder.example.com".into(),
            oidc: Some(crate::server::config::OidcSettings {
                issuer_url: "https://idp.example.com".into(),
                client_id: "cid".into(),
                client_secret: "csec".into(),
                allowed_groups: vec![],
                groups_claim: "groups".into(),
            }),
            session_key: Some(TEST_KEY),
            signout_redirect_url: None,
            talos_reader_groups: vec![],
            talos_operator_groups: vec![],
            talos_actions_enabled: false,
            talos_config_groups: vec![],
            talos_config_enabled: false,
            alerts_groups: vec![],
            alerts_actions_enabled: false,
            alerts_operator_groups: vec![],
            pod_node_name: None,
            max_user_backends: 200,
            backend_idle_secs: 1200,
        })
    }

    pub(crate) fn empty_state(config: Arc<ServerConfig>) -> AppState {
        let shared = roder_k8s::SharedCluster::for_test();
        AppState {
            leptos_options: empty_leptos_options(),
            asset_version: Arc::from("test-version"),
            provider: None,
            alerts: Arc::new(RwLock::new(None)),
            backends: Arc::new(crate::server::backends::BackendRegistry::new(
                shared.clone(),
                None,
                config.max_user_backends,
                std::time::Duration::from_secs(config.backend_idle_secs),
            )),
            config,
            shared,
            talos: None,
            talos_action_lock: Arc::new(Mutex::new(())),
            drain_jobs: Arc::new(crate::server::drain_jobs::DrainJobs::default()),
            ha: None,
        }
    }

    /// Build a dev-mode `AppState`.
    pub(crate) fn dev_state() -> AppState {
        empty_state(dev_config())
    }

    /// Build a production `AppState` (no real `provider` — tests that don't
    /// need the OIDC exchange just pass through with `provider: None`).
    pub(crate) fn prod_state_without_provider() -> AppState {
        empty_state(prod_config())
    }

    /// A test `Arc<Backend>` (no live cluster I/O), for handler unit tests that
    /// now take the backend via an `Extension<Arc<Backend>>` extractor instead
    /// of resolving it from `AppState`.
    pub(crate) fn test_backend() -> Arc<roder_k8s::Backend> {
        Arc::new(roder_k8s::Backend::from_parts_for_test(
            roder_k8s::ClusterAccess::for_test(),
            roder_k8s::SharedCluster::for_test(),
        ))
    }

    /// A `Cookie:` header carrying a validly-sealed session for `tokens`.
    pub(crate) fn sealed_cookie_header(tokens: &roder_auth::Tokens) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let sealed = seal_session(tokens, &TEST_KEY);
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}={sealed}")).unwrap(),
        );
        headers
    }

    /// `LeptosOptions` has no public `Default` and is `#[non_exhaustive]`, so
    /// the only reliable way to build one in a test is to deserialize an
    /// empty-ish JSON. Most fields carry `#[serde(default = "…")]`, so
    /// `output-name` is the only required key.
    pub(crate) fn empty_leptos_options() -> LeptosOptions {
        serde_json::from_str(r#"{"output-name": "roder"}"#)
            .expect("LeptosOptions should deserialize from a minimal JSON")
    }

    pub(crate) fn fake_tokens() -> roder_auth::Tokens {
        roder_auth::Tokens {
            id_token: "id".into(),
            access_token: "access".into(),
            refresh_token: Some("rt".into()),
            expires_at: OffsetDateTime::now_utc() + time::Duration::seconds(3600),
            identity: Identity {
                subject: "sub".into(),
                email: Some("a@b".into()),
                name: Some("Alice".into()),
                groups: vec!["admins".into()],
            },
        }
    }

    pub(crate) fn collect_set_cookies(resp: &axum::response::Response) -> Vec<String> {
        resp.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(String::from))
            .collect()
    }
}
