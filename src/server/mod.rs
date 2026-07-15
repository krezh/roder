//! Server-side (SSR) wiring for roder: configuration, OIDC auth + sessions, the
//! token-passthrough cluster client, and the background token refresh.

pub mod api;
pub mod config;
pub mod handlers;
pub mod session;

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use axum::extract::FromRef;
use leptos::prelude::LeptosOptions;
use roder_auth::{OidcProvider, Tokens};
use roder_k8s::Backend;
use tokio::sync::{Mutex, RwLock};

pub use config::ServerConfig;

/// Deterministically hash the built WASM bundle's bytes so a redeploy (which
/// always changes these bytes) yields a new value, while an unchanged image
/// restarting yields the same one. `DefaultHasher` (unlike `HashMap`'s
/// `RandomState`) is not randomized per-process — same input always hashes
/// the same, which is the property this depends on.
fn compute_asset_version(wasm_bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    wasm_bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Shared application state for the axum router. Single user ⇒ one shared cluster
/// client behind an `RwLock` (rebuilt on token refresh).
#[derive(Clone)]
pub struct AppState {
    pub leptos_options: LeptosOptions,
    /// Hash of the built WASM bundle, computed once at startup. Embedded into
    /// the SSR shell and pushed over SSE so an already-open tab can detect a
    /// redeploy and reload itself (see `src/version.rs` on the client side).
    pub asset_version: Arc<str>,
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

    // cargo-leptos writes the built bundle to `{site_root}/{site_pkg_dir}/{output_name}.wasm`
    // (the same layout `HydrationScripts`/`HashedStylesheet` already assume). Hashed once at
    // startup — a redeploy always restarts this process, so "once at startup" is "once per build".
    let wasm_path = format!(
        "{}/{}/{}.wasm",
        leptos_options.site_root, leptos_options.site_pkg_dir, leptos_options.output_name
    );
    let asset_version: Arc<str> = match std::fs::read(&wasm_path) {
        Ok(bytes) => compute_asset_version(&bytes).into(),
        Err(e) => {
            // Best-effort: an unusual local layout (e.g. a non-standard dev setup)
            // must not crash startup. Fall back to a value that's still unique
            // per process, so equality-based skew detection stays sound even
            // here — it just won't reflect real build content.
            tracing::warn!(
                "could not read {wasm_path} to compute the asset version ({e}); \
                 version-skew auto-reload will use a per-process fallback"
            );
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            compute_asset_version(&nanos.to_le_bytes()).into()
        }
    };

    Ok(AppState {
        leptos_options,
        asset_version,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_asset_version_is_deterministic() {
        let bytes = b"fake wasm bytes";
        assert_eq!(compute_asset_version(bytes), compute_asset_version(bytes));
    }

    #[test]
    fn compute_asset_version_differs_for_different_input() {
        assert_ne!(
            compute_asset_version(b"build one"),
            compute_asset_version(b"build two")
        );
    }
}
