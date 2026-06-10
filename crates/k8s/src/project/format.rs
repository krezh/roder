//! Value-shaping helpers: relative-age formatting, access-mode abbreviations,
//! endpoint flattening, and HPA target rendering.

use serde_json::Value;

use super::accessors::int_at;

/// Relative age of an RFC3339 timestamp ("5m", "2h3m", "1d4h"). Empty if unparsable.
pub(crate) fn humanize_since(ts: &str) -> String {
    use time::format_description::well_known::Rfc3339;
    let Ok(t) = time::OffsetDateTime::parse(ts, &Rfc3339) else {
        return String::new();
    };
    let secs = (time::OffsetDateTime::now_utc() - t).whole_seconds().max(0) as u64;
    roder_core::format_age_secs(secs)
}

/// Render a byte count using the binary (Ki/Mi/Gi/Ti) suffixes that match how
/// Kubernetes itself quantises storage. `0.0` or sub-1-KiB returns the raw
/// integer bytes (kubectl uses 0/512/1024 in that region).
pub(crate) fn human_bytes(b: f64) -> String {
    if !b.is_finite() || b < 1024.0 {
        return format!("{}B", b as u64);
    }
    const UNITS: &[(&str, f64)] = &[
        ("Ki", 1024.0),
        ("Mi", 1024.0 * 1024.0),
        ("Gi", 1024.0 * 1024.0 * 1024.0),
        ("Ti", 1024.0 * 1024.0 * 1024.0 * 1024.0),
    ];
    for (suffix, mult) in UNITS {
        if b < mult * 1024.0 {
            return format!("{:.1}{}", b / mult, suffix);
        }
    }
    let pb = 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0;
    format!("{:.1}Pi", b / pb)
}

/// kubectl-style access-mode abbreviations.
pub(crate) fn short_access_mode(m: &str) -> String {
    match m {
        "ReadWriteOnce" => "RWO",
        "ReadOnlyMany" => "ROX",
        "ReadWriteMany" => "RWX",
        "ReadWriteOncePod" => "RWOP",
        other => other,
    }
    .to_string()
}

/// Endpoints `subsets[]` flattened to `ip:port` entries (newline-joined for the tooltip).
pub(crate) fn endpoints_summary(data: &Value) -> String {
    let mut out: Vec<String> = Vec::new();
    if let Some(subsets) = data.get("subsets").and_then(|s| s.as_array()) {
        for ss in subsets {
            let ports: Vec<i64> = ss
                .get("ports")
                .and_then(|p| p.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.get("port").and_then(|v| v.as_i64()))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(addrs) = ss.get("addresses").and_then(|a| a.as_array()) {
                for a in addrs {
                    if let Some(ip) = a.get("ip").and_then(|v| v.as_str()) {
                        if ports.is_empty() {
                            out.push(ip.to_string());
                        } else {
                            out.extend(ports.iter().map(|p| format!("{ip}:{p}")));
                        }
                    }
                }
            }
        }
    }
    out.join("\n")
}

/// kubectl-style `current/target` per metric (e.g. `cpu: 41%/80%`). Handles the
/// v1 CPU-utilization fields and v2 Resource metrics; other metric types show their kind.
pub(crate) fn hpa_targets(data: &Value) -> String {
    let specs = data
        .get("spec")
        .and_then(|s| s.get("metrics"))
        .and_then(|m| m.as_array());
    let Some(specs) = specs else {
        // autoscaling/v1 shape.
        let Some(target) = int_at(data, &["spec", "targetCPUUtilizationPercentage"]) else {
            return String::new();
        };
        let cur = int_at(data, &["status", "currentCPUUtilizationPercentage"])
            .map(|c| format!("{c}%"))
            .unwrap_or_else(|| "<unknown>".into());
        return format!("cpu: {cur}/{target}%");
    };
    let curs = data
        .get("status")
        .and_then(|s| s.get("currentMetrics"))
        .and_then(|m| m.as_array());
    specs
        .iter()
        .enumerate()
        .map(|(i, m)| hpa_metric_str(m, curs.and_then(|c| c.get(i))))
        .collect::<Vec<_>>()
        .join(", ")
}

fn hpa_metric_str(spec: &Value, cur: Option<&Value>) -> String {
    let ty = spec.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if ty != "Resource" {
        return ty.to_string();
    }
    let res = spec.get("resource");
    let name = res
        .and_then(|r| r.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let target = res.and_then(|r| r.get("target"));
    let fmt = |v: &Value| {
        v.get("averageUtilization")
            .and_then(|x| x.as_i64())
            .map(|x| format!("{x}%"))
            .or_else(|| {
                v.get("averageValue")
                    .and_then(|x| x.as_str())
                    .map(String::from)
            })
    };
    let tstr = target.and_then(fmt).unwrap_or_default();
    let cstr = cur
        .and_then(|c| c.get("resource"))
        .and_then(|r| r.get("current"))
        .and_then(fmt)
        .unwrap_or_else(|| "<unknown>".into());
    format!("{name}: {cstr}/{tstr}")
}
