use std::sync::Arc;

use arc_swap::ArcSwap;
use kube::api::{Api, DynamicObject};
use kube::core::ApiResource;
use kube::{Client, Config};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum K8sError {
    #[error("failed to load Kubernetes config: {0}")]
    Config(String),

    #[error("failed to build Kubernetes client: {0}")]
    Build(String),

    #[error("Kubernetes API error: {0}")]
    Api(String),
}

pub type Result<T> = std::result::Result<T, K8sError>;

/// Holds the cluster connection used for all API access. roder passes the logged-in
/// user's OIDC **ID token** through as the bearer, so RBAC is the user's identity.
///
/// ID tokens are short-lived, so instead of a dynamic per-request auth layer we
/// rebuild the `kube::Client` with the current token and hot-swap it ([`set_token`]).
/// Informers read [`client`] on each (re)connect; when a token expires mid-watch the
/// old client 401s, the informer reconnects, and picks up the swapped-in client.
///
/// Single user ⇒ one identity ⇒ one shared client/cache ⇒ one watch per type on etcd.
pub struct ClusterAccess {
    /// Inferred once: API server URL, CA, default namespace. Auth is overridden per token.
    base: Config,
    current: ArcSwap<Client>,
}

impl ClusterAccess {
    /// Connect using the user's OIDC ID token as the bearer (production path).
    pub async fn connect_with_token(id_token: &str) -> Result<Self> {
        let base = infer_base().await?;
        let client = build_client(&base, id_token)?;
        let access = Self {
            base,
            current: ArcSwap::from_pointee(client),
        };
        access.probe().await?;
        Ok(access)
    }

    /// Connect using whatever credentials the inferred config already carries
    /// (kubeconfig / in-cluster SA). Used for local dev against a kind cluster
    /// whose API server doesn't validate OIDC tokens.
    pub async fn connect_with_default() -> Result<Self> {
        let base = infer_base().await?;
        let client = Client::try_from(base.clone()).map_err(|e| K8sError::Build(e.to_string()))?;
        let access = Self {
            base,
            current: ArcSwap::from_pointee(client),
        };
        access.probe().await?;
        Ok(access)
    }

    /// Swap in a client built from a freshly refreshed ID token.
    pub fn set_token(&self, id_token: &str) -> Result<()> {
        let client = build_client(&self.base, id_token)?;
        self.current.store(Arc::new(client));
        Ok(())
    }

    /// The current client. Cheap to clone (Arc inside).
    pub fn client(&self) -> Arc<Client> {
        self.current.load_full()
    }

    /// Validate connectivity + credentials by asking the API server its version.
    /// Returns the full git version (e.g. "v1.30.1") when available.
    pub async fn probe(&self) -> Result<String> {
        let v = self
            .client()
            .apiserver_version()
            .await
            .map_err(|e| K8sError::Api(e.to_string()))?;
        if !v.git_version.is_empty() {
            Ok(v.git_version)
        } else {
            Ok(format!("{}.{}", v.major, v.minor))
        }
    }
}

impl ClusterAccess {
    /// A ClusterAccess pointing at an unreachable loopback apiserver, for unit
    /// tests that exercise registry/backend bookkeeping without real I/O.
    /// `pub` (not `#[cfg(test)]`) so `SharedCluster::for_test` — used by the
    /// `roder` server crate's own test fixtures — can build one across the
    /// crate boundary (`cfg(test)` is per-crate and wouldn't be visible there).
    #[doc(hidden)]
    pub fn for_test() -> Arc<Self> {
        // Tests don't run main()'s provider install, and the server crate's test
        // binary links both ring and aws-lc-rs — pick ring explicitly so building a
        // test client doesn't panic on provider ambiguity. Idempotent across calls.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut base = Config::new("https://127.0.0.1:1".parse().unwrap());
        base.default_namespace = "default".into();
        let client = Client::try_from(base.clone()).expect("build test client");
        Arc::new(Self {
            base,
            current: ArcSwap::from_pointee(client),
        })
    }
}

/// Build a dynamic `Api` scoped correctly for a kind: all-namespaces for a
/// cluster-scoped kind or when no namespace is given, otherwise namespaced.
pub(crate) fn make_api(
    client: Client,
    ar: &ApiResource,
    namespaced: bool,
    ns: Option<&str>,
) -> Api<DynamicObject> {
    if namespaced {
        match ns {
            Some(n) => Api::namespaced_with(client, n, ar),
            None => Api::all_with(client, ar),
        }
    } else {
        Api::all_with(client, ar)
    }
}

async fn infer_base() -> Result<Config> {
    Config::infer()
        .await
        .map_err(|e| K8sError::Config(e.to_string()))
}

fn build_client(base: &Config, id_token: &str) -> Result<Client> {
    let mut config = base.clone();
    // Override auth with the bearer token. Deserializing AuthInfo from JSON avoids
    // depending on secrecy's exact SecretString constructor across versions.
    config.auth_info = serde_json::from_value(serde_json::json!({ "token": id_token }))
        .map_err(|e| K8sError::Build(e.to_string()))?;
    Client::try_from(config).map_err(|e| K8sError::Build(e.to_string()))
}
