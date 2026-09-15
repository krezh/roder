//! URL construction for the API routes the browser calls.

use super::percent_encode;

/// Build the SSE URL for a live resource list. An optional label `selector`
/// (e.g. a workload's `spec.selector`) narrows the watch to matching objects.
pub fn watch_url(key: &str, namespace: Option<&str>, selector: Option<&str>) -> String {
    let mut url = format!(
        "/api/watch?key={}&namespace={}",
        key,
        namespace.unwrap_or("")
    );
    if let Some(sel) = selector.filter(|s| !s.is_empty()) {
        url.push_str("&selector=");
        url.push_str(&percent_encode(sel));
    }
    url
}

/// Build the SSE URL for a multiplexed multi-pane watch stream.
pub fn watch_multi_url(panes: &[(&str, Option<&str>)]) -> String {
    let parts: Vec<String> = panes
        .iter()
        .map(|(key, ns)| format!("{}:{}", percent_encode(key), ns.unwrap_or("")))
        .collect();
    format!("/api/watch-multi?panes={}", parts.join(","))
}

/// Build the detail URL for a single object.
pub fn detail_url(key: &str, namespace: Option<&str>, name: &str) -> String {
    format!(
        "/api/detail?key={}&namespace={}&name={}",
        key,
        namespace.map(percent_encode).unwrap_or_default(),
        percent_encode(name)
    )
}

pub fn container_file_url(
    endpoint: &str,
    namespace: &str,
    pod: &str,
    container: &str,
    path: &str,
) -> String {
    format!(
        "/api/{endpoint}?namespace={}&pod={}&container={}&path={}",
        percent_encode(namespace),
        percent_encode(pod),
        percent_encode(container),
        percent_encode(path),
    )
}
