//! Server-side (SSR) wiring for roder: configuration, OIDC auth + sessions, the
//! token-passthrough cluster client, and the background token refresh.

pub mod api;
pub mod config;
pub mod handlers;
pub mod session;

use std::sync::Arc;

use axum::extract::FromRef;
use leptos::prelude::LeptosOptions;
use roder_auth::{OidcProvider, Tokens};
use roder_k8s::Backend;
use tokio::sync::{Mutex, RwLock};

pub use config::ServerConfig;

/// Shared application state for the axum router. Single user ⇒ one shared cluster
/// client behind an `RwLock` (rebuilt on token refresh).
#[derive(Clone)]
pub struct AppState {
    pub leptos_options: LeptosOptions,
    pub config: Arc<ServerConfig>,
    /// None in dev mode.
    pub provider: Option<Arc<OidcProvider>>,
    /// Established after the first successful login (or at startup in dev mode):
    /// the connected cluster + discovery catalog + informer registry.
    pub backend: Arc<RwLock<Option<Arc<Backend>>>>,
    /// The live token set for the shared cluster identity. Reconstructed from
    /// the sealed `roder_session` cookie after a restart, and refreshed on use.
    /// `None` until the first authenticated request (or in dev mode).
    pub current: Arc<RwLock<Option<Tokens>>>,
    /// Single-flight guard around token refresh so concurrent requests don't
    /// each call the IdP and invalidate one another's rotated refresh token.
    pub refresh_lock: Arc<Mutex<()>>,
    /// Single-flight guard around (cold) backend construction, so concurrent
    /// requests can't each run a full discovery + CRD load and hammer the
    /// apiserver. Normally the backend is already built at startup, so this is
    /// only contended if the startup connect failed.
    pub backend_build_lock: Arc<Mutex<()>>,
    /// Alertmanager HTTP cache. `None` if no Alertmanager was discovered.
    pub alerts: Arc<RwLock<Option<Arc<roder_k8s::AlertsCache>>>>,
    /// Native Talos in-cluster API client. None when no generated config is mounted.
    pub talos: Option<Arc<roder_talos::Backend>>,
    /// Serialize Talos mutations so duplicate service/power operations cannot race.
    pub talos_action_lock: Arc<Mutex<()>>,
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

    // Connect at startup so the catalog + CRD printer columns are discovered
    // ONCE, up front, and cached — not on the first client login, and never
    // re-loaded per connection (which is what let a client hammer the apiserver).
    // Dev uses the inferred kubeconfig; prod uses roder's pod ServiceAccount —
    // the catalog/columns are cluster metadata, so the SA only needs discovery +
    // read on `customresourcedefinitions`. Actual object reads still go through
    // each user's passthrough token, swapped into this same client on login.
    // Best-effort: if the cluster isn't reachable yet, the first request builds it.
    let backend = Arc::new(RwLock::new(None));
    let alerts = Arc::new(RwLock::new(None));
    let talos = match roder_talos::Backend::connect_in_cluster().await {
        Ok(client) => {
            if client.is_some() {
                tracing::info!("connected to Talos through the in-cluster API service");
            }
            client.map(Arc::new)
        }
        Err(e) => {
            tracing::warn!("Talos in-cluster configuration is present but unusable: {e}");
            None
        }
    };
    match Backend::connect_with_default().await {
        Ok(b) => {
            tracing::info!(
                "connected to cluster at startup, {} kinds discovered",
                b.kinds().len()
            );
            let am_client = b.client();
            *backend.write().await = Some(Arc::new(b));
            let url = roder_k8s::discover_alertmanager(&am_client).await;
            *alerts.write().await = url.map(|u| Arc::new(roder_k8s::AlertsCache::new(u)));
        }
        Err(e) => {
            tracing::warn!("no cluster connection at startup (will connect on first request): {e}")
        }
    }

    Ok(AppState {
        leptos_options,
        config: Arc::new(config),
        provider,
        backend,
        current: Arc::new(RwLock::new(None)),
        refresh_lock: Arc::new(Mutex::new(())),
        backend_build_lock: Arc::new(Mutex::new(())),
        alerts,
        talos,
        talos_action_lock: Arc::new(Mutex::new(())),
    })
}
