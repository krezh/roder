//! Prometheus HTTP API client (`/api/v1/query`, `/api/v1/query_range`).
//!
//! Domain-free on purpose: no queries and no caching, because the useful TTL
//! differs per feature — seconds for a live chart, hours for a resource scan.
//!
//! `RODER_PROMETHEUS_URL` is a direct HTTP(S) URL, such as
//! `http://prometheus-operated.monitoring.svc.cluster.local:9090`.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use time::OffsetDateTime;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Returns the direct Prometheus URL from `RODER_PROMETHEUS_URL`.
pub fn prometheus_url() -> Result<Option<String>, String> {
    match std::env::var("RODER_PROMETHEUS_URL") {
        Ok(url) => {
            let url = direct_prometheus_url(&url)?;
            info!("prometheus: using {url}");
            Ok(Some(url))
        }
        Err(std::env::VarError::NotPresent) => {
            info!("prometheus: RODER_PROMETHEUS_URL not set, PromQL features disabled");
            Ok(None)
        }
        Err(error) => Err(format!("invalid RODER_PROMETHEUS_URL: {error}")),
    }
}

fn direct_prometheus_url(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|error| format!("invalid RODER_PROMETHEUS_URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("RODER_PROMETHEUS_URL must use http or https".to_string());
    }
    Ok(url.trim_end_matches('/').to_string())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PromError {
    #[error("prometheus request: {0}")]
    Request(String),
    #[error("prometheus response: {0}")]
    Response(String),
    /// A rejected query, an exceeded `max_samples`, or a server-side timeout.
    /// `error_type` is upstream's own classification.
    #[error("prometheus {error_type}: {message}")]
    Api { error_type: String, message: String },
    #[error("prometheus response exceeds {0} byte limit")]
    TooLarge(usize),
    #[error("prometheus returned resultType {got:?}, expected {want:?}")]
    ResultType { want: &'static str, got: String },
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// One series of an instant query: labels plus a single value at one instant.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub labels: HashMap<String, String>,
    pub timestamp: f64,
    pub value: f64,
}

/// One series of a range query: labels plus `(timestamp, value)` over the window.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeSeries {
    pub labels: HashMap<String, String>,
    pub values: Vec<(f64, f64)>,
}

impl Sample {
    pub fn label(&self, name: &str) -> Option<&str> {
        self.labels.get(name).map(String::as_str)
    }
}

impl RangeSeries {
    pub fn label(&self, name: &str) -> Option<&str> {
        self.labels.get(name).map(String::as_str)
    }
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Prometheus wraps every answer in this envelope; `status` decides which half
/// of the body is populated.
#[derive(Deserialize)]
struct Envelope {
    status: String,
    #[serde(default)]
    data: Option<EnvelopeData>,
    #[serde(default, rename = "errorType")]
    error_type: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct EnvelopeData {
    #[serde(default, rename = "resultType")]
    result_type: String,
    #[serde(default)]
    result: Vec<RawSeries>,
}

/// A `vector` series carries `value`, a `matrix` series carries `values`.
/// Both optional so one struct covers either, without an untagged enum.
#[derive(Deserialize)]
struct RawSeries {
    #[serde(default)]
    metric: HashMap<String, String>,
    #[serde(default)]
    value: Option<(f64, String)>,
    #[serde(default)]
    values: Vec<(f64, String)>,
}

/// Values arrive as strings so Prometheus can express `NaN` and `±Inf`, which
/// Rust's parser accepts. Passed through unfiltered: only the caller knows
/// whether a `NaN` means zero or no-data.
fn parse_sample_value(raw: &str) -> Result<f64, PromError> {
    raw.parse::<f64>()
        .map_err(|error| PromError::Response(format!("sample value {raw:?}: {error}")))
}

// ---------------------------------------------------------------------------
// Query construction helpers
// ---------------------------------------------------------------------------

/// Escape a string for use inside a PromQL string literal (`pod="…"`).
pub fn escape_label_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

/// Escape a string for use as a literal inside an RE2 matcher (`pod=~"…-.*"`).
///
/// Apply before [`escape_label_value`]: this handles regex metacharacters, that
/// handles the surrounding string literal.
pub fn escape_regex(value: &str) -> String {
    const META: &[char] = &[
        '\\', '.', '+', '*', '?', '(', ')', '|', '[', ']', '{', '}', '^', '$',
    ];
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if META.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Render a duration as a PromQL duration literal (`336h`, `75s`).
///
/// Falls back to seconds so a fractional-minute step renders as `75s`, not a
/// fraction PromQL would reject.
pub fn promql_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs == 0 {
        "0s".to_string()
    } else if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A two-week subquery is slow by nature, and cutting the client off early
/// leaves Prometheus doing the work anyway.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Stateless query client: holds no results, so one `Arc` can be shared.
pub struct PromClient {
    base_url: String,
    http: reqwest::Client,
    max_response_bytes: usize,
    /// Mirrored into the `timeout` query param so Prometheus abandons an
    /// expensive evaluation itself rather than being cut off mid-flight.
    timeout: Duration,
}

impl PromClient {
    pub fn new(url: String) -> Self {
        Self::with_timeout(url, DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(url: String, timeout: Duration) -> Self {
        Self {
            base_url: url,
            http: reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .expect("static Prometheus HTTP client configuration is valid"),
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            timeout,
        }
    }

    /// Range queries over long windows can legitimately exceed the default.
    #[must_use]
    pub fn max_response_bytes(mut self, bytes: usize) -> Self {
        self.max_response_bytes = bytes;
        self
    }

    /// Instant query evaluated at `now`.
    pub async fn query(&self, query: &str) -> Result<Vec<Sample>, PromError> {
        self.query_at(query, OffsetDateTime::now_utc()).await
    }

    /// Instant query evaluated at a specific time.
    pub async fn query_at(
        &self,
        query: &str,
        at: OffsetDateTime,
    ) -> Result<Vec<Sample>, PromError> {
        let params = [
            ("query", query.to_string()),
            ("time", unix_seconds(at)),
            ("timeout", promql_duration(self.timeout)),
        ];
        let data = self.post("/api/v1/query", &params).await?;
        if data.result_type != "vector" {
            return Err(PromError::ResultType {
                want: "vector",
                got: data.result_type,
            });
        }

        data.result
            .into_iter()
            .map(|series| {
                let (timestamp, raw) = series.value.ok_or_else(|| {
                    PromError::Response("vector series is missing its value".to_string())
                })?;
                Ok(Sample {
                    labels: series.metric,
                    timestamp,
                    value: parse_sample_value(&raw)?,
                })
            })
            .collect()
    }

    /// Range query over `[start, end]` at `step` resolution.
    pub async fn query_range(
        &self,
        query: &str,
        start: OffsetDateTime,
        end: OffsetDateTime,
        step: Duration,
    ) -> Result<Vec<RangeSeries>, PromError> {
        let params = [
            ("query", query.to_string()),
            ("start", unix_seconds(start)),
            ("end", unix_seconds(end)),
            ("step", promql_duration(step)),
            ("timeout", promql_duration(self.timeout)),
        ];
        let data = self.post("/api/v1/query_range", &params).await?;
        if data.result_type != "matrix" {
            return Err(PromError::ResultType {
                want: "matrix",
                got: data.result_type,
            });
        }

        data.result
            .into_iter()
            .map(|series| {
                Ok(RangeSeries {
                    labels: series.metric,
                    values: series
                        .values
                        .into_iter()
                        .map(|(timestamp, raw)| Ok((timestamp, parse_sample_value(&raw)?)))
                        .collect::<Result<Vec<_>, PromError>>()?,
                })
            })
            .collect()
    }

    /// POST, not GET: a two-week subquery outgrows what proxies allow in a URL.
    async fn post(&self, path: &str, params: &[(&str, String)]) -> Result<EnvelopeData, PromError> {
        let url = format!("{}{path}", self.base_url);
        let response = self
            .http
            .post(&url)
            .form(params)
            .send()
            .await
            .map_err(|error| {
                warn!("prometheus: request failed: {error}");
                PromError::Request(error.to_string())
            })?;

        // A rejected query returns 400/422 with a JSON envelope explaining why,
        // so the body is read and classified before the status is judged.
        let status = response.status();
        let body = self.read_body(response).await?;
        let envelope: Envelope = sonic_rs::from_str(&body).map_err(|error| {
            warn!(
                "prometheus: json parse failed: {error}\nbody: {}",
                utf8_prefix(&body, 500)
            );
            PromError::Response(format!("json: {error}"))
        })?;

        if envelope.status != "success" {
            return Err(PromError::Api {
                error_type: envelope.error_type.unwrap_or_else(|| status.to_string()),
                message: envelope
                    .error
                    .unwrap_or_else(|| "no error detail".to_string()),
            });
        }
        envelope
            .data
            .ok_or_else(|| PromError::Response("successful response has no data".to_string()))
    }

    /// Stream the body, aborting past the cap rather than buffering a runaway
    /// matrix into memory.
    async fn read_body(&self, response: reqwest::Response) -> Result<String, PromError> {
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| PromError::Response(error.to_string()))?
        {
            if bytes.len().saturating_add(chunk.len()) > self.max_response_bytes {
                return Err(PromError::TooLarge(self.max_response_bytes));
            }
            bytes.extend_from_slice(&chunk);
        }
        String::from_utf8(bytes).map_err(|error| PromError::Response(format!("not UTF-8: {error}")))
    }
}

fn unix_seconds(at: OffsetDateTime) -> String {
    format!("{}.{:03}", at.unix_timestamp(), at.millisecond())
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
    use super::*;

    fn envelope(body: &str) -> Envelope {
        sonic_rs::from_str(body).expect("valid envelope")
    }

    #[test]
    fn rejects_non_http_urls() {
        assert!(direct_prometheus_url("file:///etc/passwd").is_err());
        assert_eq!(
            direct_prometheus_url("http://prom:9090/").unwrap(),
            "http://prom:9090"
        );
    }

    #[test]
    fn instant_vector_parses_labels_and_value() {
        let data = envelope(
            r#"{"status":"success","data":{"resultType":"vector","result":[
                {"metric":{"pod":"api-0","container":"api"},"value":[1700000000.5,"0.125"]}
            ]}}"#,
        )
        .data
        .expect("data");

        assert_eq!(data.result_type, "vector");
        let series = &data.result[0];
        let (timestamp, raw) = series.value.clone().expect("value");
        assert_eq!(timestamp, 1_700_000_000.5);
        assert_eq!(parse_sample_value(&raw).unwrap(), 0.125);
        assert_eq!(series.metric["container"], "api");
    }

    #[test]
    fn range_matrix_parses_every_point() {
        let data = envelope(
            r#"{"status":"success","data":{"resultType":"matrix","result":[
                {"metric":{"pod":"api-0"},"values":[[1700000000,"1"],[1700000075,"2.5"]]}
            ]}}"#,
        )
        .data
        .expect("data");

        assert_eq!(data.result_type, "matrix");
        assert_eq!(data.result[0].values.len(), 2);
        assert!(data.result[0].value.is_none());
    }

    #[test]
    fn non_finite_sample_values_survive_parsing() {
        // An absent series yields NaN and an unbounded ratio yields +Inf; both
        // are real answers the caller has to interpret, not parse failures.
        assert!(parse_sample_value("NaN").unwrap().is_nan());
        assert_eq!(parse_sample_value("+Inf").unwrap(), f64::INFINITY);
        assert_eq!(parse_sample_value("-Inf").unwrap(), f64::NEG_INFINITY);
        assert!(parse_sample_value("not-a-number").is_err());
    }

    #[test]
    fn api_errors_carry_the_upstream_classification() {
        let envelope = envelope(
            r#"{"status":"error","errorType":"execution","error":"query exceeded max samples"}"#,
        );
        assert_eq!(envelope.status, "error");
        assert_eq!(envelope.error_type.as_deref(), Some("execution"));
        assert!(envelope.error.unwrap().contains("max samples"));
    }

    #[test]
    fn label_values_and_regex_literals_are_escaped_separately() {
        assert_eq!(escape_label_value(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(escape_regex("a+b(c)"), r"a\+b\(c\)");
        // Hyphens are not RE2 metacharacters outside a character class, so a
        // workload name survives intact.
        assert_eq!(escape_regex("web-api"), "web-api");
        // A name is a regex literal first, then a string literal: the backslash
        // escape_regex adds is itself escaped for the surrounding quotes.
        assert_eq!(escape_label_value(&escape_regex("a.b")), r"a\\.b");
    }

    #[test]
    fn durations_render_as_promql_literals() {
        assert_eq!(
            promql_duration(Duration::from_secs(24 * 7 * 2 * 3600)),
            "336h"
        );
        // KRR's 1.25-minute step has no whole-minute form.
        assert_eq!(promql_duration(Duration::from_secs(75)), "75s");
        assert_eq!(promql_duration(Duration::from_secs(300)), "5m");
        assert_eq!(promql_duration(Duration::ZERO), "0s");
    }

    #[test]
    fn timestamps_render_with_millisecond_precision() {
        let at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        assert_eq!(unix_seconds(at), "1700000000.000");
    }

    #[test]
    fn response_preview_truncates_at_utf8_boundary() {
        let value = format!("{}é", "a".repeat(499));
        assert_eq!(utf8_prefix(&value, 500), "a".repeat(499));
    }
}
