//! Server-side (SSR) wiring for roder: configuration, OIDC auth + sessions, the
//! token-passthrough cluster client, and the background token refresh.

pub mod api;
pub mod backends;
pub mod config;
pub mod drain_jobs;
pub mod ha;
pub mod handlers;
pub mod session;

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use axum::extract::FromRef;
use kube::api::{Api, Patch, PatchParams};
use leptos::prelude::LeptosOptions;
use roder_auth::OidcProvider;
use tokio::sync::{Mutex, RwLock};

pub use config::ServerConfig;

/// Publish the mTLS fingerprint as an annotation on this pod so peers can pin
/// to it. Idempotent: an OTP-on-startup patch (merge of the annotation map).
async fn publish_fingerprint(
    client: &kube::Client,
    pod_name: &str,
    fingerprint: &str,
) -> Result<(), String> {
    let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::default_namespaced(client.clone());
    let patch = serde_json::json!({
        "metadata": {
            "annotations": {
                roder_k8s::TLS_FINGERPRINT_ANNOTATION: fingerprint
            }
        }
    });
    pods.patch(pod_name, &PatchParams::default(), &Patch::Merge(patch))
        .await
        .map_err(|e| format!("patch pod annotation: {e}"))?;
    Ok(())
}

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
    /// Shared, ServiceAccount-owned cluster metadata and enrichment caches.
    /// Built once at startup and read by every per-user `Backend`.
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
    /// Multi-replica operation routing and Kubernetes Lease coordination.
    pub ha: Option<Arc<ha::HaState>>,
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

    // Connect at startup so the catalog is discovered once and shared instead
    // of being loaded for every client connection. Dev uses the inferred
    // kubeconfig; prod uses roder's pod ServiceAccount. Actual object reads go through
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
    // Build the shared SA-owned catalog, enrichment caches, and CRD watch.
    // Startup REQUIRES the cluster to be
    // reachable (roder runs in-cluster; the apiserver is up before roder schedules).
    let shared = roder_k8s::SharedCluster::connect_default()
        .await
        .map_err(|e| format!("failed to connect to cluster at startup: {e}"))?;
    tracing::info!(
        "connected to cluster at startup, {} kinds discovered",
        shared.kinds().len()
    );
    let ha = if matches!(
        std::env::var("RODER_HA_ENABLED").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    ) {
        let pod_name = std::env::var("RODER_POD_NAME")
            .map_err(|_| "RODER_HA_ENABLED requires RODER_POD_NAME".to_string())?;
        let pod_uid = std::env::var("RODER_POD_UID")
            .map_err(|_| "RODER_HA_ENABLED requires RODER_POD_UID".to_string())?;
        let selector = std::env::var("RODER_POD_LABEL_SELECTOR")
            .map_err(|_| "RODER_HA_ENABLED requires RODER_POD_LABEL_SELECTOR".to_string())?;
        let scope = std::env::var("RODER_HA_SCOPE").unwrap_or_else(|_| selector.clone());
        let holder = format!("{pod_name}/{pod_uid}");
        let peer_port = match std::env::var("RODER_PEER_PORT") {
            Ok(value) => value
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| "RODER_PEER_PORT must be an integer from 1 to 65535".to_string())?,
            Err(std::env::VarError::NotPresent) => ha::DEFAULT_PEER_PORT,
            Err(error) => return Err(format!("failed to read RODER_PEER_PORT: {error}")),
        };

        let coordinator =
            roder_k8s::NodeCoordinator::new(shared.sa_client(), selector, holder, scope);

        let cert = roder_mtls::PeerCert::mint(&pod_name)
            .map_err(|error| format!("failed to mint peer mTLS certificate: {error}"))?;
        let verifier = Arc::new(roder_mtls::PinnedVerifier::new());
        let client_cfg = roder_mtls::client_config(&cert, verifier.clone())
            .map_err(|error| format!("failed to build peer mTLS client config: {error}"))?;
        let server_cfg = roder_mtls::server_config(&cert, verifier.clone())
            .map_err(|error| format!("failed to build peer mTLS server config: {error}"))?;
        publish_fingerprint(&shared.sa_client(), &pod_name, &cert.fingerprint)
            .await
            .map_err(|error| format!("failed to publish peer mTLS fingerprint: {error}"))?;

        let ha = Arc::new(ha::HaState::new(
            coordinator,
            pod_name,
            peer_port,
            verifier,
            Arc::new(client_cfg),
            Arc::new(server_cfg),
        ));
        let peer_count = ha
            .refresh_fingerprints()
            .await
            .map_err(|error| format!("failed to load peer mTLS fingerprints: {error}"))?;
        tracing::info!(peer_count, "initialized peer mTLS trust set");
        ha.spawn_fingerprint_refresh();
        Some(ha)
    } else {
        None
    };

    let url = roder_k8s::alertmanager_url()?;
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

    // cargo-leptos writes the built bundle to `{site_root}/{site_pkg_dir}/{output_name}.wasm`.
    // When `hash-files = true` (set in Cargo.toml), the filename includes a content hash:
    // `{output_name}.{hash}.wasm`. Resolve the hash the same way `leptos_meta::HashedStylesheet`
    // resolves the CSS hash: from `hash.txt` next to the running executable, which is the
    // manifest cargo-leptos actually writes for this lookup (not a directory scan, which is
    // ambiguous whenever a stale hashed build is left behind in `pkg/` from an earlier run).
    let mut wasm_file_name = leptos_options.output_name.to_string();
    if leptos_options.hash_files {
        let hash_path = std::env::current_exe()
            .map(|path| path.parent().map(|p| p.to_path_buf()).unwrap_or_default())
            .unwrap_or_default()
            .join(leptos_options.hash_file.as_ref());
        if let Ok(hashes) = std::fs::read_to_string(&hash_path) {
            for line in hashes.lines() {
                if let Some((file, hash)) = line.trim().split_once(':') {
                    if file == "wasm" {
                        wasm_file_name.push('.');
                        wasm_file_name.push_str(hash.trim());
                    }
                }
            }
        }
    }
    wasm_file_name.push_str(".wasm");
    let wasm_path = std::path::PathBuf::from(format!(
        "{}/{}/{}",
        leptos_options.site_root, leptos_options.site_pkg_dir, wasm_file_name
    ));
    let asset_version: Arc<str> = match std::fs::read(&wasm_path) {
        Ok(bytes) => compute_asset_version(&bytes).into(),
        Err(e) => {
            // Best-effort: an unusual local layout (e.g. a non-standard dev setup)
            // must not crash startup. Fall back to a value that's still unique
            // per process, so equality-based skew detection stays sound even
            // here — it just won't reflect real build content.
            tracing::warn!(
                "could not read {} to compute the asset version ({e}); \
                 version-skew auto-reload will use a per-process fallback",
                wasm_path.display()
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
        drain_jobs: Arc::new(drain_jobs::DrainJobs::new(
            ha.as_ref().map(|ha| ha.pod_name.clone()),
        )),
        backends,
        ha,
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
