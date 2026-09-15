//! Client-side data access: REST fetch + SSE subscription. All browser APIs are
//! gated to `wasm32`; on the (native) SSR build these are no-ops, since the UI
//! loads its data after hydration in the browser.
//!
//! Split by transport concern — [`urls`] builds the routes, [`http`] fetches
//! and posts, [`sse`] holds the live subscriptions, [`storage`] wraps browser
//! storage and the few DOM queries the key dispatcher needs, and [`age`]
//! formats timestamp cells. Everything is re-exported here, so callers keep
//! using `crate::data::*` unchanged.

mod age;
mod http;
mod sse;
mod storage;
mod urls;

pub use age::{cell_needs_tick, humanize_age, humanize_cell, looks_like_rfc3339};
pub use http::{fetch_json, post_action, post_json, probe_error};
pub use sse::{reconnect_delay, subscribe_lines, subscribe_multi, subscribe_with_error, SseHandle};
pub use storage::{
    has_text_selection, is_text_input_focused, storage_get, storage_remove, storage_set,
};
// Session storage is only ever read from wasm-gated call sites, so unlike the
// other browser wrappers it has no native no-op counterpart to re-export.
#[cfg(target_arch = "wasm32")]
pub use storage::{session_storage_get, session_storage_remove, session_storage_set};
pub use urls::{container_file_url, detail_url, watch_multi_url, watch_url};

/// Minimal percent-encoding for a query value (label selectors contain `=`, `,`).
pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_unreserved_passthrough() {
        assert_eq!(percent_encode("nginx-web"), "nginx-web");
        assert_eq!(percent_encode("v1.2.3~rc_1"), "v1.2.3~rc_1");
    }

    #[test]
    fn percent_encode_special_chars() {
        assert_eq!(
            percent_encode("app=nginx,tier=web"),
            "app%3Dnginx%2Ctier%3Dweb"
        );
        assert_eq!(percent_encode("ns/name"), "ns%2Fname");
        assert_eq!(percent_encode("hello world"), "hello%20world");
    }

    #[test]
    fn timestamp_hints_require_the_live_ui_tick() {
        assert!(cell_needs_tick("2\x1f2026-08-27T12:00:00Z"));
        assert!(cell_needs_tick("2026-08-27T12:00:00Z"));
        assert!(!cell_needs_tick("2\x1f5m ago"));
    }

    #[test]
    fn percent_encode_empty() {
        assert_eq!(percent_encode(""), "");
    }

    #[test]
    fn detail_url_with_namespace() {
        let u = detail_url("apps/v1/Deployment", Some("default"), "my-deploy");
        assert_eq!(
            u,
            "/api/detail?key=apps/v1/Deployment&namespace=default&name=my-deploy"
        );
    }

    #[test]
    fn detail_url_cluster_scoped() {
        let u = detail_url("rbac.authorization.k8s.io/v1/ClusterRole", None, "admin");
        assert_eq!(
            u,
            "/api/detail?key=rbac.authorization.k8s.io/v1/ClusterRole&namespace=&name=admin"
        );
    }

    #[test]
    fn detail_url_encodes_name_with_special_chars() {
        let u = detail_url("v1/Pod", Some("kube system"), "my=pod");
        assert_eq!(
            u,
            "/api/detail?key=v1/Pod&namespace=kube%20system&name=my%3Dpod"
        );
    }

    #[test]
    fn container_file_url_encodes_every_query_value() {
        let url = container_file_url("files", "kube system", "web=1", "side/car", "/var/log/a b");
        assert_eq!(
            url,
            "/api/files?namespace=kube%20system&pod=web%3D1&container=side%2Fcar&path=%2Fvar%2Flog%2Fa%20b"
        );
    }

    #[test]
    fn rfc3339_detector_accepts_kubernetes_timestamps() {
        // The forms k8s actually emits: with `Z` or a numeric offset, with
        // and without sub-second precision.
        assert!(looks_like_rfc3339("2024-03-01T12:34:56Z"));
        assert!(looks_like_rfc3339("2024-03-01T12:34:56.000000Z"));
        assert!(looks_like_rfc3339("2024-03-01T12:34:56+00:00"));
        assert!(looks_like_rfc3339("2024-03-01T12:34:56.123456789+02:00"));
    }

    #[test]
    fn rfc3339_detector_rejects_non_dates() {
        // Phase, status, numeric metrics, plain text — none of these have the
        // RFC3339 shape, so a non-date cell is left untouched.
        assert!(!looks_like_rfc3339("Running"));
        assert!(!looks_like_rfc3339("True"));
        assert!(!looks_like_rfc3339("42"));
        assert!(!looks_like_rfc3339("3.14Gi"));
        assert!(!looks_like_rfc3339("2024-03-01"));
        assert!(!looks_like_rfc3339(""));
        // An ISO date without the `T` separator isn't RFC3339 — the form k8s
        // stamps onto objects always carries the `T`, so this guard keeps the
        // heuristic narrow.
        assert!(!looks_like_rfc3339("2024-03-01 12:34:56Z"));
    }
}
