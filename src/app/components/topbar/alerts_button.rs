//! Topbar alerts button — hidden entirely when AlertManager isn't configured.

use leptos::prelude::*;

use crate::app::state::{AlertsData, AlertsOpen};

/// The loudest severity currently firing, which tints the button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Loudest {
    Quiet,
    Info,
    Warning,
    Critical,
}

/// Rank the firing alerts by severity.
///
/// Deliberately derived from the same counts the tooltip renders rather than
/// re-inspecting the alerts, so the colour can never disagree with the
/// breakdown that explains it — including the tooltip's rule that any severity
/// which isn't `critical` or `warning` is bucketed as info.
fn loudest(total: usize, critical: usize, warning: usize) -> Loudest {
    if total == 0 {
        Loudest::Quiet
    } else if critical > 0 {
        Loudest::Critical
    } else if warning > 0 {
        Loudest::Warning
    } else {
        Loudest::Info
    }
}

/// Alerts button — hidden entirely when AlertManager is not configured (data is None).
/// Shows total count and a hover tooltip with critical/warning/info breakdown.
#[component]
pub(crate) fn AlertsButton() -> impl IntoView {
    let data = expect_context::<AlertsData>().0;
    let open = expect_context::<AlertsOpen>().0;

    let counts = Memo::new(move |_| {
        data.get().map(|alerts| {
            let active: Vec<_> = alerts.iter().filter(|a| !a.silenced).collect();
            let critical = active.iter().filter(|a| a.severity == "critical").count();
            let warning = active.iter().filter(|a| a.severity == "warning").count();
            let info = active
                .iter()
                .filter(|a| a.severity != "critical" && a.severity != "warning")
                .count();
            (active.len(), critical, warning, info)
        })
    });

    let bouncing = RwSignal::new(false);
    Effect::new(move |prev: Option<usize>| {
        let total = counts.get().map(|(t, _, _, _)| t).unwrap_or(0);
        if prev.is_some_and(|p| p != total) {
            bouncing.set(true);
            set_timeout(
                move || {
                    let _ = bouncing.try_set(false);
                },
                std::time::Duration::from_millis(350),
            );
        }
        total
    });

    view! {
        {move || {
            counts.get().map(|(total, critical, warning, info)| {
                let loud = loudest(total, critical, warning);
                view! {
                    <button
                        class="alerts-btn tip-anchor-end"
                        class:alerts-firing=loud != Loudest::Quiet
                        class:alerts-critical=loud == Loudest::Critical
                        class:alerts-warning=loud == Loudest::Warning
                        class:alerts-info=loud == Loudest::Info
                        class:bouncing=move || bouncing.get()
                        aria-label=format!("{total} active alerts")
                        on:click=move |_| open.set(true)
                    >
                        <span class="alerts-count">{total}</span>
                        <span class="tooltip alerts-tip">
                            <span class="tip-row"><span class="sev-dot sev-critical"></span>"Critical: " {critical}</span>
                            <span class="tip-row"><span class="sev-dot sev-warning"></span>"Warning: " {warning}</span>
                            <span class="tip-row"><span class="sev-dot sev-info"></span>"Info: " {info}</span>
                        </span>
                    </button>
                }
            })
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::{loudest, Loudest};

    #[test]
    fn critical_outranks_a_larger_pile_of_warnings() {
        assert_eq!(loudest(9, 1, 8), Loudest::Critical);
    }

    #[test]
    fn warning_wins_only_when_nothing_is_critical() {
        assert_eq!(loudest(3, 0, 3), Loudest::Warning);
        assert_eq!(loudest(3, 1, 2), Loudest::Critical);
    }

    /// Severities outside critical/warning ("none", "page", …) are bucketed as
    /// info by the tooltip, so the colour has to follow.
    #[test]
    fn anything_else_firing_reads_as_info() {
        assert_eq!(loudest(2, 0, 0), Loudest::Info);
    }

    #[test]
    fn no_active_alerts_is_quiet() {
        assert_eq!(loudest(0, 0, 0), Loudest::Quiet);
    }
}
