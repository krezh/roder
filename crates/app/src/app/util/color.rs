//! Status-driven colours and stable per-string hues.

use roder_core::RowStatus;

/// Tint a resource's *name* only for failing (red) and completed/neutral (gray)
/// states — every other status leaves the name its default colour.
pub(crate) fn name_color(status: RowStatus) -> &'static str {
    match status {
        RowStatus::Error => "color:var(--error)",
        RowStatus::Done => "color:var(--unknown)",
        _ => "",
    }
}

/// Stable hue (0–359) from a string via djb2 — for log pills.
pub(crate) fn hue_of(s: &str) -> u32 {
    let mut h: u32 = 5381;
    for b in s.bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u32);
    }
    h % 360
}

pub(crate) fn dot_class(status: RowStatus) -> &'static str {
    match status {
        RowStatus::Ok => "ok",
        RowStatus::Pending => "pending",
        RowStatus::Warn => "warn",
        RowStatus::Error => "error",
        // Completed and genuinely-unknown both read as neutral gray.
        RowStatus::Done | RowStatus::Unknown => "unknown",
    }
}
