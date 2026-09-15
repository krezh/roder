//! Time-series samples and age formatting.

use serde::{Deserialize, Serialize};

/// A single pod metrics data point, shared between the server (serializes) and
/// the frontend (deserializes) via the `/api/metrics` endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MetricsPoint {
    pub timestamp: u64,
    pub cpu: f64,
    pub mem: f64,
}

/// Compact human-readable duration from a number of seconds ("5m", "2h3m", "1d4h").
pub fn format_age_secs(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}
