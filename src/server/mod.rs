//! Server-side (SSR) wiring for roder: configuration, OIDC auth + sessions, the
//! token-passthrough cluster client, and the background token refresh.

pub mod api;
pub mod backends;
pub mod config;
pub mod drain_jobs;
pub mod handlers;
pub mod session;

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use axum::extract::FromRef;
use leptos::prelude::LeptosOptions;
use roder_auth::OidcProvider;
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

/// Shared application state for the axum router. Per-user cluster clients live in
/// `backends` (the `BackendRegistry`), keyed by OIDC subject and resolved by
/// `require_auth` on each request; this struct otherwise holds the process-wide
/// singletons (SA-owned shared cluster metadata, config, OIDC provider, etc.).
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
    /// Alertmanager HTTP cache. `None` if no Alertmanager was discovered.
    pub alerts: Arc<RwLock<Option<Arc<roder_k8s::AlertsCache>>>>,
    /// Shared, ServiceAccount-owned cluster metadata + enrichment (catalog,
    /// CRD printer columns, metrics/PVC scrape caches). Built once at startup;
    /// every per-user Backend reads discovery/columns/enrichment from here.
    pub shared: std::sync::Arc<roder_k8s::SharedCluster>,
    /// Native Talos in-cluster API client. None when no generated config is mounted.
    pub talos: Option<Arc<roder_talos::Backend>>,
    /// Serialize Talos mutations so duplicate service/power operations cannot race.
    pub talos_action_lock: Arc<Mutex<()>>,
    /// In-flight drain jobs: buffered events for lossless SSE replay.
    pub drain_jobs: Arc<drain_jobs::DrainJobs>,
    /// Per-OIDC-subject backends (token passthrough): the live request-path
    /// source of each caller's `Backend`, resolved by `require_auth` and
    /// passed to handlers via request extensions. Built lazily on each user's
    /// first authenticated request; idle-evicted; soft-capped.
    pub backends: std::sync::Arc<crate::server::backends::BackendRegistry>,
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
    // each user's own passthrough token, via their per-subject `Backend` in
    // `backends` (built lazily on first use by `BackendRegistry::resolve`).
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
    // Build the shared SA-owned layer once: catalog + CRD printer columns +
    // metrics/PVC scrape caches + CRD watch. Startup REQUIRES the cluster to be
    // reachable (roder runs in-cluster; the apiserver is up before roder schedules).
    let shared = roder_k8s::SharedCluster::connect_default()
        .await
        .map_err(|e| format!("failed to connect to cluster at startup: {e}"))?;
    tracing::info!(
        "connected to cluster at startup, {} kinds discovered",
        shared.kinds().len()
    );

    // Alertmanager discovery uses the SA client (unchanged behavior, now via shared).
    let am_client = shared.sa_client();
    let url = roder_k8s::discover_alertmanager(&am_client).await;
    *alerts.write().await = url.map(|u| Arc::new(roder_k8s::AlertsCache::new(u)));

    // Per-subject backend registry (token passthrough): this is the live
    // request-path source of each caller's `Backend`, resolved by
    // `require_auth` and passed downstream via request extensions.
    // `spawn_reaper` must be called after `Arc::new`-wrapping (it takes
    // `&Arc<Self>`, can't self-start from `new`), or idle eviction silently
    // never runs.
    let backends = Arc::new(backends::BackendRegistry::new(
        shared.clone(),
        provider.clone(),
        config.max_user_backends,
        std::time::Duration::from_secs(config.backend_idle_secs),
    ));
    backends.spawn_reaper();

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
        alerts,
        shared,
        talos,
        talos_action_lock: Arc::new(Mutex::new(())),
        drain_jobs: Arc::new(drain_jobs::DrainJobs::default()),
        backends,
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
