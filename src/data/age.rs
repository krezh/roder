//! Relative-age formatting for timestamp cells.

/// Humanize an RFC3339 timestamp into a compact relative age (e.g. "3d", "5m").
#[cfg(target_arch = "wasm32")]
pub fn humanize_age(created: &Option<String>) -> String {
    let Some(ts) = created else {
        return String::new();
    };
    let parsed = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(ts)).get_time();
    if parsed.is_nan() {
        return String::new();
    }
    let secs = ((js_sys::Date::now() - parsed) / 1000.0).max(0.0) as u64;
    roder_core::format_age_secs(secs)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn humanize_age(_created: &Option<String>) -> String {
    String::new()
}

/// Cheap structural check for an RFC3339 timestamp: `YYYY-MM-DDTHH:MM:SS…`.
/// Used to decide whether a cell value should be live-humanized on the tick
/// (mirroring the dedicated `Age` column) rather than rendered as a static
/// string. Kubernetes timestamp fields are reliably RFC3339, so the risk of a
/// false positive on a non-date column is negligible.
pub fn looks_like_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 20
        && b.len() <= 35
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        // year is digits
        && b[0..4].iter().all(|c| c.is_ascii_digit())
}

pub fn cell_needs_tick(s: &str) -> bool {
    looks_like_rfc3339(s)
        || s.split_once('\x1f')
            .is_some_and(|(_, hint)| looks_like_rfc3339(hint))
}

/// Humanize an RFC3339 cell or timestamp hint. The caller must read the global
/// `Tick` so the returned value re-renders each second.
#[cfg(target_arch = "wasm32")]
pub fn humanize_cell(s: &str) -> String {
    if let Some((value, hint)) = s.split_once('\x1f') {
        if looks_like_rfc3339(hint) {
            return format!("{value}\x1f{} ago", humanize_age(&Some(hint.to_string())));
        }
    }
    if looks_like_rfc3339(s) {
        humanize_age(&Some(s.to_string()))
    } else {
        s.to_string()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn humanize_cell(s: &str) -> String {
    s.to_string()
}
