//! Alertmanager alert cache.
//!
//! `RODER_ALERTMANAGER_URL` accepts two forms:
//!   - A full URL (`http://alertmanager-operated.monitoring.svc.cluster.local:9093`) — fetched
//!     directly via reqwest, works in-cluster.
//!   - A kube API proxy path (`/api/v1/namespaces/monitoring/services/http:alertmanager-main:9093/proxy`)
//!     — proxied through the kube API server, works from a local kubeconfig too.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Returns the Alertmanager URL from `RODER_ALERTMANAGER_URL`, or `None`.
pub async fn discover_alertmanager(_client: &kube::Client) -> Option<String> {
    match std::env::var("RODER_ALERTMANAGER_URL") {
        Ok(url) => {
            info!("alertmanager: using {url}");
            Some(url)
        }
        Err(_) => {
            info!("alertmanager: RODER_ALERTMANAGER_URL not set, alerts disabled");
            None
        }
    }
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

enum Transport {
    /// Direct HTTP — `RODER_ALERTMANAGER_URL` is a full `http(s)://` URL.
    Direct(reqwest::Client),
    /// Kube API proxy — `RODER_ALERTMANAGER_URL` is a proxy path starting with `/`.
    KubeProxy,
}

/// Short-lived cache for the Alertmanager alert list.
pub struct AlertsCache {
    base_url: String,
    transport: Transport,
    cache: tokio::sync::Mutex<Option<(Vec<roder_core::FiringAlert>, Instant)>>,
}

impl AlertsCache {
    pub fn new(url: String) -> Self {
        let transport = if url.starts_with("http://") || url.starts_with("https://") {
            Transport::Direct(reqwest::Client::new())
        } else {
            Transport::KubeProxy
        };
        Self {
            base_url: url,
            transport,
            cache: tokio::sync::Mutex::new(None),
        }
    }

    /// Return the current alert list, refreshing if the cache is stale.
    pub async fn get(&self, client: &kube::Client) -> Result<Vec<roder_core::FiringAlert>, String> {
        self.load(client, None).await
    }

    /// Fetch a fresh alert list, coalescing concurrent refresh requests.
    pub async fn refresh(
        &self,
        client: &kube::Client,
    ) -> Result<Vec<roder_core::FiringAlert>, String> {
        self.load(client, Some(Instant::now())).await
    }

    async fn load(
        &self,
        client: &kube::Client,
        refresh_requested_at: Option<Instant>,
    ) -> Result<Vec<roder_core::FiringAlert>, String> {
        let mut guard = self.cache.lock().await;

        if let Some((ref data, ref ts)) = *guard {
            let reusable = refresh_requested_at
                .map(|requested_at| *ts >= requested_at)
                .unwrap_or_else(|| ts.elapsed() < CACHE_TTL);
            if reusable {
                return Ok(data.clone());
            }
        }

        let url = format!("{}/api/v2/alerts", self.base_url);
        info!("alertmanager: fetching {url}");

        let body = match &self.transport {
            Transport::Direct(http) => http
                .get(&url)
                .send()
                .await
                .map_err(|e| {
                    warn!("alertmanager: request failed: {e}");
                    format!("alertmanager request: {e}")
                })?
                .text()
                .await
                .map_err(|e| format!("alertmanager response: {e}"))?,
            Transport::KubeProxy => {
                let req = http::Request::get(&url)
                    .body(Vec::new())
                    .map_err(|e| format!("alertmanager request build: {e}"))?;
                client.request_text(req).await.map_err(|e| {
                    warn!("alertmanager: proxy request failed: {e}");
                    format!("alertmanager proxy: {e}")
                })?
            }
        };

        let raw: Vec<AmAlert> = serde_json::from_str(&body).map_err(|e| {
            warn!(
                "alertmanager: json parse failed: {e}\nbody: {}",
                &body[..body.len().min(500)]
            );
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
