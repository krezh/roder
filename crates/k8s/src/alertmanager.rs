//! Alertmanager alert cache.
//!
//! `RODER_ALERTMANAGER_URL` is a direct HTTP(S) URL, such as
//! `http://alertmanager-operated.monitoring.svc.cluster.local:9093`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Returns the direct Alertmanager URL from `RODER_ALERTMANAGER_URL`.
pub fn alertmanager_url() -> Result<Option<String>, String> {
    match std::env::var("RODER_ALERTMANAGER_URL") {
        Ok(url) => {
            let url = direct_alertmanager_url(&url)?;
            info!("alertmanager: using {url}");
            Ok(Some(url))
        }
        Err(std::env::VarError::NotPresent) => {
            info!("alertmanager: RODER_ALERTMANAGER_URL not set, alerts disabled");
            Ok(None)
        }
        Err(error) => Err(format!("invalid RODER_ALERTMANAGER_URL: {error}")),
    }
}

fn direct_alertmanager_url(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|error| format!("invalid RODER_ALERTMANAGER_URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("RODER_ALERTMANAGER_URL must use http or https".to_string());
    }
    Ok(url.trim_end_matches('/').to_string())
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
    #[serde(default, rename = "silencedBy")]
    silenced_by: Vec<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SilenceRequest<'a> {
    matchers: Vec<SilenceMatcher<'a>>,
    starts_at: String,
    ends_at: String,
    created_by: &'a str,
    comment: &'a str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SilenceMatcher<'a> {
    name: &'a str,
    value: &'a str,
    is_regex: bool,
    is_equal: bool,
}

#[derive(serde::Deserialize)]
struct SilenceResponse {
    #[serde(rename = "silenceID")]
    silence_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SilenceError {
    #[error("alert not found")]
    NotFound,
    #[error("alert is already silenced")]
    AlreadySilenced,
    #[error("{0}")]
    Upstream(String),
}

fn silence_request<'a>(
    labels: &'a HashMap<String, String>,
    duration: Option<Duration>,
    created_by: &'a str,
    now: time::OffsetDateTime,
) -> Result<SilenceRequest<'a>, String> {
    if labels.is_empty() {
        return Err("alert has no labels".to_string());
    }
    let format = &time::format_description::well_known::Rfc3339;
    let ends_at = match duration {
        Some(duration) => {
            let duration_seconds = i64::try_from(duration.as_secs())
                .map_err(|_| "silence duration is too large".to_string())?;
            (now + time::Duration::seconds(duration_seconds))
                .format(format)
                .map_err(|error| format!("format silence end: {error}"))?
        }
        None => "9999-12-31T23:59:59Z".to_string(),
    };
    Ok(SilenceRequest {
        matchers: labels
            .iter()
            .map(|(name, value)| SilenceMatcher {
                name,
                value,
                is_regex: false,
                is_equal: true,
            })
            .collect(),
        starts_at: now
            .format(format)
            .map_err(|error| format!("format silence start: {error}"))?,
        ends_at,
        created_by,
        comment: "Silenced from Roder",
    })
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
            silenced: !self.status.silenced_by.is_empty(),
            labels: self.labels,
        }
    }
}

const CACHE_TTL: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Short-lived cache for the Alertmanager alert list.
pub struct AlertsCache {
    base_url: String,
    http: reqwest::Client,
    cache: tokio::sync::RwLock<Option<(Vec<roder_core::FiringAlert>, Instant)>>,
    refresh_lock: tokio::sync::Mutex<()>,
    silence_lock: tokio::sync::Mutex<()>,
}

impl AlertsCache {
    pub fn new(url: String) -> Self {
        Self {
            base_url: url,
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("static Alertmanager HTTP client configuration is valid"),
            cache: tokio::sync::RwLock::new(None),
            refresh_lock: tokio::sync::Mutex::new(()),
            silence_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Return the current alert list, refreshing if the cache is stale.
    pub async fn get(&self) -> Result<Vec<roder_core::FiringAlert>, String> {
        self.load(None).await
    }

    /// Fetch a fresh alert list, coalescing concurrent refresh requests.
    pub async fn refresh(&self) -> Result<Vec<roder_core::FiringAlert>, String> {
        self.load(Some(Instant::now())).await
    }

    pub async fn silence_alert(
        &self,
        fingerprint: &str,
        duration: Option<Duration>,
        created_by: &str,
    ) -> Result<String, SilenceError> {
        let _silence = self.silence_lock.lock().await;
        let alerts = self.get().await.map_err(SilenceError::Upstream)?;
        let alert = alerts
            .into_iter()
            .find(|alert| alert.fingerprint == fingerprint)
            .ok_or(SilenceError::NotFound)?;
        if alert.silenced {
            return Err(SilenceError::AlreadySilenced);
        }
        self.create_silence(&alert.labels, duration, created_by)
            .await
            .map_err(SilenceError::Upstream)
    }

    async fn create_silence(
        &self,
        labels: &HashMap<String, String>,
        duration: Option<Duration>,
        created_by: &str,
    ) -> Result<String, String> {
        let payload = silence_request(
            labels,
            duration,
            created_by,
            time::OffsetDateTime::now_utc(),
        )?;
        let body = serde_json::to_vec(&payload)
            .map_err(|error| format!("serialize Alertmanager silence: {error}"))?;
        let url = format!("{}/api/v2/silences", self.base_url);

        let response_body = {
            let response = self
                .http
                .post(&url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|error| format!("alertmanager silence request: {error}"))?;
            let mut response = response
                .error_for_status()
                .map_err(|error| format!("alertmanager silence response: {error}"))?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| format!("alertmanager silence response: {error}"))?
            {
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Err(format!(
                        "alertmanager silence response exceeds {MAX_RESPONSE_BYTES} byte limit"
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            String::from_utf8(bytes)
                .map_err(|error| format!("alertmanager silence response is not UTF-8: {error}"))?
        };
        let response: SilenceResponse = serde_json::from_str(&response_body)
            .map_err(|error| format!("alertmanager silence response: {error}"))?;

        if let Some((alerts, timestamp)) = self.cache.write().await.as_mut() {
            for alert in alerts {
                if &alert.labels == labels {
                    alert.silenced = true;
                }
            }
            *timestamp = Instant::now();
        }
        Ok(response.silence_id)
    }

    async fn load(
        &self,
        refresh_requested_at: Option<Instant>,
    ) -> Result<Vec<roder_core::FiringAlert>, String> {
        if let Some(data) = self.cached(refresh_requested_at).await {
            return Ok(data);
        }

        // Serialize refreshes without retaining the cache lock across network I/O.
        let _refresh = self.refresh_lock.lock().await;
        if let Some(data) = self.cached(refresh_requested_at).await {
            return Ok(data);
        }

        let url = format!("{}/api/v2/alerts", self.base_url);
        info!("alertmanager: fetching {url}");

        let body = {
            let response = self.http.get(&url).send().await.map_err(|e| {
                warn!("alertmanager: request failed: {e}");
                format!("alertmanager request: {e}")
            })?;
            let mut response = response
                .error_for_status()
                .map_err(|e| format!("alertmanager response: {e}"))?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| format!("alertmanager response: {e}"))?
            {
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Err(format!(
                        "alertmanager response exceeds {MAX_RESPONSE_BYTES} byte limit"
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            String::from_utf8(bytes)
                .map_err(|e| format!("alertmanager response is not UTF-8: {e}"))?
        };

        let raw: Vec<AmAlert> = serde_json::from_str(&body).map_err(|e| {
            warn!(
                "alertmanager: json parse failed: {e}\nbody: {}",
                utf8_prefix(&body, 500)
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

        *self.cache.write().await = Some((alerts.clone(), Instant::now()));
        Ok(alerts)
    }

    async fn cached(
        &self,
        refresh_requested_at: Option<Instant>,
    ) -> Option<Vec<roder_core::FiringAlert>> {
        let guard = self.cache.read().await;
        let (data, timestamp) = guard.as_ref()?;
        let reusable = refresh_requested_at
            .map(|requested_at| *timestamp >= requested_at)
            .unwrap_or_else(|| timestamp.elapsed() < CACHE_TTL);
        reusable.then(|| data.clone())
    }
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::{direct_alertmanager_url, silence_request, utf8_prefix, AmAlert, SilenceResponse};

    #[test]
    fn response_preview_truncates_at_utf8_boundary() {
        let value = format!("{}é", "a".repeat(499));
        assert_eq!(utf8_prefix(&value, 500), "a".repeat(499));
        assert_eq!(utf8_prefix(&value, 501), value);
    }

    #[test]
    fn silence_uses_every_alert_label_as_an_exact_matcher() {
        let labels = HashMap::from([
            ("alertname".to_string(), "PodDown".to_string()),
            ("namespace".to_string(), "production".to_string()),
        ]);
        let now = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let request = silence_request(&labels, Some(Duration::from_secs(3_600)), "operator", now)
            .expect("valid silence request");
        let value = serde_json::to_value(request).unwrap();
        let matchers = value["matchers"].as_array().unwrap();

        assert_eq!(matchers.len(), labels.len());
        assert!(matchers.iter().all(|matcher| {
            matcher["isRegex"] == false
                && matcher["isEqual"] == true
                && labels
                    .get(matcher["name"].as_str().unwrap())
                    .map(String::as_str)
                    == matcher["value"].as_str()
        }));
        assert_eq!(value["createdBy"], "operator");
        assert_eq!(value["comment"], "Silenced from Roder");
    }

    #[test]
    fn forever_silence_uses_alertmanager_maximum_timestamp() {
        let labels = HashMap::from([("alertname".to_string(), "PodDown".to_string())]);
        let now = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let request = silence_request(&labels, None, "operator", now).unwrap();

        assert_eq!(request.ends_at, "9999-12-31T23:59:59Z");
    }

    #[test]
    fn alertmanager_silence_response_uses_capital_id() {
        let response: SilenceResponse =
            serde_json::from_str(r#"{"silenceID":"silence-123"}"#).unwrap();
        assert_eq!(response.silence_id, "silence-123");
    }

    #[test]
    fn inhibited_alert_is_not_reported_as_silenced() {
        let alert: AmAlert = serde_json::from_value(serde_json::json!({
            "fingerprint": "abc",
            "labels": { "alertname": "PodDown", "severity": "warning" },
            "annotations": {},
            "startsAt": "2026-01-01T00:00:00Z",
            "status": { "state": "suppressed", "silencedBy": [], "inhibitedBy": ["def"] }
        }))
        .unwrap();
        assert!(!alert.into_firing().silenced);
    }

    #[test]
    fn alertmanager_url_requires_direct_http() {
        assert_eq!(
            direct_alertmanager_url("http://alertmanager:9093/").unwrap(),
            "http://alertmanager:9093"
        );
        assert!(direct_alertmanager_url(
            "/api/v1/namespaces/monitoring/services/alertmanager/proxy"
        )
        .is_err());
    }
}
