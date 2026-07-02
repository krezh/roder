//! Pure formatting / parsing helpers used across the UI. Log-line parsing lives
//! in [`log_line`] and ANSI-to-HTML conversion in [`ansi`]; everything else
//! (resource keys, camelCase labels, node/pod metric strings) stays here.

mod ansi;
mod log_line;

pub(crate) use ansi::ansi_to_html;
pub(crate) use log_line::{log_level, parse_log_line};

/// Parse a resource key ("group/version/kind", group may be empty) into (group, kind).
pub(crate) fn parse_key(key: &str) -> (String, String) {
    let mut parts = key.splitn(3, '/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(g), Some(_v), Some(k)) => (g.to_string(), k.to_string()),
        _ => (String::new(), key.to_string()),
    }
}

/// "readyReplicas" -> "Ready replicas", "hostIP" -> "Host IP" (acronyms kept intact).
pub(crate) fn camel_label(s: &str) -> String {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if prev.is_none() {
            out.extend(ch.to_uppercase());
            prev = Some(ch);
            continue;
        }
        if ch.is_uppercase() {
            let p = prev.unwrap();
            let next_lower = chars.peek().is_some_and(|n| n.is_lowercase());
            // Word boundary: camelCase (prev lower/digit) or end of an acronym (HTTPServer -> HTTP Server).
            if p.is_lowercase() || p.is_ascii_digit() || (p.is_uppercase() && next_lower) {
                out.push(' ');
            }
        }
        out.push(ch);
        prev = Some(ch);
    }
    out
}

/// Extract a Talos version from a node's `osImage` ("Talos (v1.7.6)" → "v1.7.6").
pub(crate) fn talos_version(os_image: &str) -> Option<String> {
    if let (Some(a), Some(b)) = (os_image.find('('), os_image.find(')')) {
        if b > a + 1 {
            return Some(os_image[a + 1..b].to_string());
        }
    }
    os_image
        .strip_prefix("Talos")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub(crate) fn pct(used: Option<f64>, total: Option<f64>) -> f64 {
    match (used, total) {
        (Some(u), Some(t)) if t > 0.0 => (u / t * 100.0).clamp(0.0, 100.0),
        _ => 0.0,
    }
}

/// Cluster-wide CPU/mem usage % = sum(used) / sum(capacity) across nodes.
pub(crate) fn cluster_usage_pct(nodes: &[roder_core::NodeSummary]) -> (f64, f64) {
    let cpu = pct(
        Some(nodes.iter().filter_map(|n| n.cpu_used).sum()),
        Some(nodes.iter().filter_map(|n| n.cpu_cores).sum()),
    );
    let mem = pct(
        Some(nodes.iter().filter_map(|n| n.mem_used).sum()),
        Some(nodes.iter().filter_map(|n| n.mem_bytes).sum()),
    );
    (cpu, mem)
}

pub(crate) fn fmt_cores(used: Option<f64>, total: Option<f64>) -> String {
    match (used, total) {
        (Some(u), Some(t)) => format!("{u:.1} / {t:.0}"),
        (None, Some(t)) => format!("{t:.0} cores"),
        _ => "—".to_string(),
    }
}

pub(crate) fn fmt_mem(used: Option<f64>, total: Option<f64>) -> String {
    let g = |b: f64| b / (1024.0 * 1024.0 * 1024.0);
    match (used, total) {
        (Some(u), Some(t)) => format!("{:.1} / {:.1} GiB", g(u), g(t)),
        (None, Some(t)) => format!("{:.1} GiB", g(t)),
        _ => "—".to_string(),
    }
}
