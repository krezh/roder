//! The countdown ring drawn around a refresh button's border.
//!
//! Bare just after a fetch, a closed ring as the next one is due. Callers pass
//! the period their own poll runs at, so the ring closes exactly as the next
//! fetch lands.

use leptos::prelude::*;

/// How long the tail takes to run down the head when a fetch lands.
const CATCHUP_SECS: f64 = 0.45;
/// Only play the catch-up for a fetch that just landed. Mounting long after one
/// resumes the fill, not the whip.
const CATCHUP_WINDOW_SECS: f64 = 1.5;

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

/// Time until the next fetch, traced around the parent's border.
///
/// The parent needs `position: relative`; the ring covers its border box.
#[component]
pub(crate) fn StalenessRing(
    /// When the last fetch landed, in browser milliseconds.
    last_refresh: RwSignal<Option<f64>>,
    /// The caller's poll period in seconds — one full turn of the ring.
    period_secs: u64,
) -> impl IntoView {
    // `(this fetch, the one before it)`. The tail starts from the arc on
    // screen, which needs the previous fetch time. One memo keeps both values
    // from the same update; separate signals can be read mid-swap.
    let ring = Memo::new(move |prior: Option<&(Option<f64>, Option<f64>)>| {
        let current = last_refresh.get();
        (current, prior.and_then(|(previous, _)| *previous))
    });

    view! {
        // Keyed on the fetch time so each fetch replaces the node: a CSS
        // animation only restarts when its element does, not when its delay is
        // patched.
        <For
            each=move || ring.get().0.map(|ms| ms as u64)
            key=|stamp| *stamp
            let:_stamp
        >
            <svg class="staleness-ring" aria-hidden="true">
                <rect
                    pathLength="100"
                    style=staleness_style(ring.get_untracked(), now_ms(), period_secs)
                ></rect>
            </svg>
        </For>
    }
}

/// Inline timing for the ring, from `(this fetch, the one before it)`.
///
/// Two animations share the element: the tail catching up, then the fill. The
/// dash array is the arc the previous cycle drew, which the catch-up keyframes
/// pick up as their implicit start.
///
/// The fill's negative delay places a ring mounted mid-cycle at the point the
/// countdown has actually reached. Never refreshed reads as fully stale.
fn staleness_style(ring: (Option<f64>, Option<f64>), now_ms: f64, period_secs: u64) -> String {
    let (last_refresh_ms, previous_ms) = ring;
    let period = period_secs as f64;
    let elapsed = last_refresh_ms
        .map(|ms| ((now_ms - ms) / 1000.0).clamp(0.0, period))
        .unwrap_or(period);

    // The arc the last cycle reached — the distance the tail travels.
    let drawn = match (last_refresh_ms, previous_ms) {
        (Some(current), Some(previous)) => {
            (((current - previous) / 1000.0) / period).clamp(0.0, 1.0) * 100.0
        }
        _ => 0.0,
    };
    let catchup = if elapsed < CATCHUP_WINDOW_SECS && drawn > 0.0 {
        CATCHUP_SECS
    } else {
        0.0
    };

    format!(
        "stroke-dasharray:{drawn:.2} {:.2};\
         animation-duration:{catchup}s,{period}s;\
         animation-delay:0s,{:.2}s",
        100.0 - drawn,
        catchup - elapsed,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fetch that just landed replays the tail from the arc the last cycle
    /// drew, then starts the new fill behind it.
    #[test]
    fn a_fresh_fetch_whips_the_tail_down_from_the_drawn_arc() {
        let now = 1_000_000.0;
        // The previous cycle ran a full period, so the ring was closed.
        let style = staleness_style((Some(now), Some(now - 30_000.0)), now, 30);
        assert!(style.contains("stroke-dasharray:100.00 0.00"), "{style}");
        assert!(style.contains("animation-duration:0.45s,30s"), "{style}");
        assert!(style.contains("animation-delay:0s,0.45"), "{style}");
    }

    /// A manual refresh part-way through shortens exactly what is on screen.
    #[test]
    fn a_mid_cycle_fetch_starts_from_the_partial_arc() {
        let now = 1_000_000.0;
        let style = staleness_style((Some(now), Some(now - 12_000.0)), now, 30);
        assert!(style.contains("stroke-dasharray:40.00 60.00"), "{style}");
    }

    /// Mounting long after a fetch resumes the fill rather than replaying it.
    #[test]
    fn mounting_mid_cycle_skips_the_catchup_and_offsets_the_fill() {
        let now = 1_000_000.0;
        let style = staleness_style((Some(now - 12_000.0), Some(now - 42_000.0)), now, 30);
        assert!(style.contains("animation-duration:0s,30s"), "{style}");
        assert!(style.contains("animation-delay:0s,-12.00"), "{style}");
    }

    #[test]
    fn never_refreshed_reads_as_fully_stale() {
        let style = staleness_style((None, None), 1_000_000.0, 30);
        assert!(style.contains("animation-delay:0s,-30.00"), "{style}");
        assert!(style.contains("stroke-dasharray:0.00 100.00"), "{style}");
    }

    /// The period is the caller's, so the dashboard's 10s ring and the alert
    /// panel's 30s one share the same code.
    #[test]
    fn the_period_comes_from_the_caller() {
        let now = 1_000_000.0;
        let style = staleness_style((Some(now), Some(now - 10_000.0)), now, 10);
        assert!(style.contains("animation-duration:0.45s,10s"), "{style}");
        assert!(style.contains("stroke-dasharray:100.00 0.00"), "{style}");
    }
}
