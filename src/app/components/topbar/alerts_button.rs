//! Topbar alerts button — hidden entirely when AlertManager isn't configured.

use leptos::prelude::*;

use crate::app::state::{AlertsData, AlertsOpen};

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
                move || bouncing.set(false),
                std::time::Duration::from_millis(350),
            );
        }
        total
    });

    view! {
        {move || {
            counts.get().map(|(total, critical, warning, info)| {
                view! {
                    <button
                        class="alerts-btn tip-anchor-end"
                        class:alerts-firing={move || total > 0}
                        class:bouncing=move || bouncing.get()
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
