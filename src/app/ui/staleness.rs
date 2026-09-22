//! The countdown ring drawn around a refresh button's border.

use leptos::prelude::*;

const RESET_SECS: f64 = 0.45;

/// Browser clock in milliseconds; 0 on the server, where the ring never renders.
pub(crate) fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0.0
    }
}

pub(crate) fn duration_until(deadline_ms: f64) -> std::time::Duration {
    std::time::Duration::from_secs_f64(((deadline_ms - now_ms()) / 1000.0).max(0.0))
}

/// Time until the next fetch, traced around the parent's border.
///
/// The parent needs `position: relative`; the ring covers its border box.
#[component]
pub(crate) fn StalenessRing(
    /// When the next fetch starts, in browser milliseconds.
    next_refresh: RwSignal<Option<f64>>,
    /// The caller's poll period in seconds — one full turn of the ring.
    period_secs: u64,
) -> impl IntoView {
    view! {
        // Keyed on the deadline so each interval replaces the node: a CSS
        // animation only restarts when its element does, not when its delay is
        // patched.
        <For
            each=move || std::iter::once(next_refresh.get().map(|ms| ms as u64))
            key=|stamp| *stamp
            let:_stamp
        >
            <svg class="staleness-ring" aria-hidden="true">
                <rect
                    class="staleness-ring-progress"
                    pathLength="100"
                    style=staleness_style(next_refresh.get_untracked(), now_ms(), period_secs)
                ></rect>
                <rect
                    class="staleness-ring-reset"
                    pathLength="100"
                    style=reset_style(next_refresh.get_untracked(), now_ms(), period_secs)
                ></rect>
            </svg>
        </For>
    }
}

/// Uses the progress interval start so rendering delays advance both strokes equally.
fn reset_style(deadline_ms: Option<f64>, now_ms: f64, period_secs: u64) -> String {
    let Some(deadline_ms) = deadline_ms else {
        return "display:none".to_string();
    };
    let elapsed = ((now_ms - (deadline_ms - period_secs as f64 * 1000.0)) / 1000.0).max(0.0);
    if elapsed >= RESET_SECS {
        return "display:none".to_string();
    }

    format!(
        "animation-duration:{RESET_SECS}s;\
         animation-delay:-{elapsed:.2}s",
    )
}

/// The negative delay places a newly mounted ring at the shared deadline's
/// current progress. No deadline means a request is due or already in flight.
fn staleness_style(deadline_ms: Option<f64>, now_ms: f64, period_secs: u64) -> String {
    let period = period_secs as f64;
    let remaining = deadline_ms
        .map(|deadline| ((deadline - now_ms) / 1000.0).clamp(0.0, period))
        .unwrap_or(0.0);
    let elapsed = period - remaining;

    format!(
        "animation-duration:{period}s;\
         animation-delay:-{elapsed:.2}s",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_interval_starts_empty() {
        let now = 1_000_000.0;
        let style = staleness_style(Some(now + 30_000.0), now, 30);
        assert!(style.contains("animation-duration:30s"), "{style}");
        assert!(style.contains("animation-delay:-0.00s"), "{style}");
    }

    #[test]
    fn mounting_mid_interval_uses_the_deadline_offset() {
        let now = 1_000_000.0;
        let style = staleness_style(Some(now + 18_000.0), now, 30);
        assert!(style.contains("animation-delay:-12.00s"), "{style}");
    }

    #[test]
    fn reset_and_progress_start_together() {
        let now = 1_000_100.0;
        let style = reset_style(Some(1_030_000.0), now, 30);
        assert!(style.contains("animation-duration:0.45s"), "{style}");
        assert!(style.contains("animation-delay:-0.10s"), "{style}");
    }

    #[test]
    fn reset_is_gone_after_its_sweep() {
        let style = reset_style(Some(1_030_000.0), 1_000_500.0, 30);
        assert_eq!(style, "display:none");
    }

    #[test]
    fn a_due_request_has_no_reset_overlay() {
        assert_eq!(reset_style(None, 1_000_000.0, 30), "display:none");
    }

    #[test]
    fn no_deadline_reads_as_due() {
        let style = staleness_style(None, 1_000_000.0, 30);
        assert!(style.contains("animation-delay:-30.00s"), "{style}");
    }

    #[test]
    fn the_period_comes_from_the_caller() {
        let now = 1_000_000.0;
        let style = staleness_style(Some(now + 10_000.0), now, 10);
        assert!(style.contains("animation-duration:10s"), "{style}");
    }
}
