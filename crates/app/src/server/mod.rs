//! Server-side (SSR) wiring for roder: configuration, OIDC auth + sessions, the
//! token-passthrough cluster client, and the background token refresh.

pub mod api;
pub mod config;
pub mod handlers;
pub mod refresh;
pub mod session;

use std::sync::Arc;

use axum::extract::FromRef;
use leptos::prelude::LeptosOptions;
use roder_auth::OidcProvider;
use roder_k8s::Backend;
use tokio::sync::RwLock;

pub use config::ServerConfig;
use session::{PendingStore, SessionStore};

/// Shared application state for the axum router. Single user ⇒ one shared cluster
/// client behind an `RwLock` (rebuilt on token refresh).
#[derive(Clone)]
pub struct AppState {
    pub leptos_options: LeptosOptions,
    pub config: Arc<ServerConfig>,
    /// None in dev mode.
    pub provider: Option<Arc<OidcProvider>>,
    pub sessions: Arc<SessionStore>,
    pub pending: Arc<PendingStore>,
    /// Established after the first successful login (or at startup in dev mode):
    /// the connected cluster + discovery catalog + informer registry.
    pub backend: Arc<RwLock<Option<Arc<Backend>>>>,
}

// Lets Leptos's axum handlers pull `LeptosOptions` out of our custom state.
impl FromRef<AppState> for LeptosOptions {
    fn from_ref(state: &AppState) -> Self {
        state.leptos_options.clone()
    }
}

/// Build application state: load config, discover the OIDC provider (unless dev),
/// and in dev mode connect to the cluster with the inferred kubeconfig creds.
pub async fn build_state(leptos_options: LeptosOptions) -> Result<AppState, String> {
    let config = ServerConfig::from_env()?;

    let provider = if config.dev_mode {
        tracing::warn!("RODER_DEV_MODE is set — OIDC is disabled and auth is bypassed");
        None
    } else {
        Some(Arc::new(
            OidcProvider::discover(config.oidc_config())
                .await
                .map_err(|e| e.to_string())?,
        ))
    };

    let backend = Arc::new(RwLock::new(None));
    if config.dev_mode {
        match Backend::connect_with_default().await {
            Ok(b) => {
                tracing::info!(
                    "dev mode: connected to cluster, {} kinds discovered",
                    b.kinds().len()
                );
                *backend.write().await = Some(Arc::new(b));
            }
            Err(e) => tracing::warn!("dev mode: no cluster connection yet: {e}"),
        }
    }

    Ok(AppState {
        leptos_options,
        config: Arc::new(config),
        provider,
        sessions: Arc::new(SessionStore::default()),
        pending: Arc::new(PendingStore::default()),
        backend,
    })
}
