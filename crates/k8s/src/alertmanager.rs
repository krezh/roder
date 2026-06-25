//! Alertmanager discovery and alert cache via the Kubernetes API proxy.
//!
//! Requests to Alertmanager go through `/api/v1/namespaces/{ns}/services/http:{svc}:{port}/proxy`,
//! so the feature works both in-cluster (where .svc DNS resolves) and from a local kubeconfig.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use k8s_openapi::api::core::v1::Service;
use kube::api::ListParams;
use kube::Api;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Returns the kube API proxy path for Alertmanager, or `None` if none is found.
///
/// Discovery order:
/// 1. `RODER_ALERTMANAGER_URL` env var — must be a kube proxy path, e.g.
///    `/api/v1/namespaces/monitoring/services/http:alertmanager-main:9093/proxy`
/// 2. Scan all Services cluster-wide; pick the first whose name contains "alertmanager"
///    and that exposes port 9093 (or a port named "web").
///
/// All requests flow through the kube API server proxy so the feature works both
/// locally (kubeconfig) and in-cluster.
pub async fn discover_alertmanager(client: &kube::Client) -> Option<String> {
    if let Ok(path) = std::env::var("RODER_ALERTMANAGER_URL") {
        info!("alertmanager: using env override → {path}");
        return Some(path);
    }

    let svc_api: Api<Service> = Api::all(client.clone());
    match svc_api.list(&ListParams::default()).await {
        Ok(list) => {
            let found = list.items.into_iter().find(|svc| {
                let name = svc.metadata.name.as_deref().unwrap_or("");
                let has_name = name.contains("alertmanager");
                let has_port = svc
                    .spec
                    .as_ref()
                    .and_then(|s| s.ports.as_ref())
                    .map(|ports| {
                        ports
                            .iter()
                            .any(|p| p.port == 9093 || p.name.as_deref() == Some("web"))
                    })
                    .unwrap_or(false);
                has_name && has_port
            });
            if let Some(svc) = found {
                let name = svc.metadata.name.unwrap_or_else(|| "alertmanager".into());
                let namespace = svc.metadata.namespace.unwrap_or_else(|| "monitoring".into());
                let port = svc
                    .spec
                    .as_ref()
                    .and_then(|s| s.ports.as_ref())
                    .and_then(|ports| {
                        ports
                            .iter()
                            .find(|p| p.port == 9093 || p.name.as_deref() == Some("web"))
                    })
                    .map(|p| p.port)
                    .unwrap_or(9093);
                let path = kube_proxy_path(&namespace, &name, port);
                info!("alertmanager: discovered {name} in {namespace} → proxy {path}");
                return Some(path);
            }
            info!("alertmanager: no Service containing 'alertmanager' with port 9093 found; set RODER_ALERTMANAGER_URL to configure manually");
        }
        Err(e) => {
            warn!("alertmanager: Service list failed ({e}); set RODER_ALERTMANAGER_URL to configure manually");
        }
    }

    None
}

fn kube_proxy_path(namespace: &str, service: &str, port: i32) -> String {
    format!("/api/v1/namespaces/{namespace}/services/http:{service}:{port}/proxy")
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Raw Alertmanager `/api/v2/alerts` payload — private, only used for deserialization.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AmAlert {
    fingerprint: String,
    labels: HashMap<String, String>,
    annotations: HashMap<String, String>,
    starts_at: String,
    status: AmAlertStatus,
}

#[derive(serde::Deserialize)]
struct AmAlertStatus {
    state: String,
}

impl AmAlert {
    fn into_firing(self) -> roder_core::FiringAlert {
        roder_core::FiringAlert {
            fingerprint: self.fingerprint,
            name: self.labels.get("alertname").cloned().unwrap_or_default(),
            severity: self.labels.get("severity").cloned().unwrap_or_default(),
            summary: self.annotations.get("summary").cloned().unwrap_or_default(),
            description: self
                .annotations
                .get("description")
                .cloned()
                .unwrap_or_default(),
            starts_at: self.starts_at,
            silenced: self.status.state != "active",
            labels: self.labels,
        }
    }
}

const CACHE_TTL: Duration = Duration::from_secs(30);

/// Short-lived cache for the Alertmanager alert list.
///
/// Requests go through the kube API server proxy so they work both in-cluster
/// and from a local kubeconfig without port-forwarding.
pub struct AlertsCache {
    /// Kube API proxy path, e.g. `/api/v1/namespaces/monitoring/services/http:alertmanager-main:9093/proxy`
    proxy_path: String,
    cache: tokio::sync::Mutex<Option<(Vec<roder_core::FiringAlert>, Instant)>>,
}

impl AlertsCache {
    pub fn new(proxy_path: String) -> Self {
        Self {
            proxy_path,
            cache: tokio::sync::Mutex::new(None),
        }
    }

    /// Return the current alert list, fetching via the kube API proxy if the
    /// cached value is absent or older than 30 seconds.
    pub async fn get(&self, client: &kube::Client) -> Result<Vec<roder_core::FiringAlert>, String> {
        let mut guard = self.cache.lock().await;

        if let Some((ref data, ref ts)) = *guard {
            if ts.elapsed() < CACHE_TTL {
                return Ok(data.clone());
            }
        }

        let path = format!("{}/api/v2/alerts", self.proxy_path);
        info!("alertmanager: fetching {path}");
        let req = http::Request::get(&path)
            .body(Vec::new())
            .map_err(|e| format!("alertmanager request build: {e}"))?;
        let body = client
            .request_text(req)
            .await
            .map_err(|e| {
                warn!("alertmanager: proxy request failed: {e}");
                format!("alertmanager proxy: {e}")
            })?;
        let raw: Vec<AmAlert> = serde_json::from_str(&body).map_err(|e| {
            warn!("alertmanager: json parse failed: {e}\nbody: {}", &body[..body.len().min(500)]);
            format!("alertmanager json: {e}")
        })?;
        info!("alertmanager: {} alert(s) fetched", raw.len());

        let alerts: Vec<roder_core::FiringAlert> = raw
            .into_iter()
            .filter(|a| {
                let sev = a.labels.get("severity").map(String::as_str).unwrap_or("");
                !sev.is_empty() && sev != "none"
            })
            .map(AmAlert::into_firing)
            .collect();

        *guard = Some((alerts.clone(), Instant::now()));
        Ok(alerts)
    }
}
