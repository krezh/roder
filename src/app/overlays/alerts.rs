use leptos::prelude::*;
use roder_core::FiringAlert;

use crate::app::state::{AlertsData, AlertsOpen};
use super::use_bool_overlay;

#[component]
pub(crate) fn AlertsPanel() -> impl IntoView {
    let open = expect_context::<AlertsOpen>().0;
    let data = expect_context::<AlertsData>().0;
    let (visible, closing, do_close) = use_bool_overlay(open);
    let show_silenced = RwSignal::new(false);

    let sorted_alerts = Memo::new(move |_| {
        let all = data.get().unwrap_or_default();
        let show_sil = show_silenced.get();
        let mut alerts: Vec<_> = all.into_iter()
            .filter(|a| show_sil || !a.silenced)
            .collect();
        alerts.sort_by(|a, b| {
            sev_order(&a.severity).cmp(&sev_order(&b.severity))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.fingerprint.cmp(&b.fingerprint))
        });
        alerts
    });

    view! {
        <Show when=move || visible.get()>
            <div class="alerts-scrim" on:click=move |_| do_close()></div>
            <div class="alerts-panel" class:closing=move || closing.get()>
                <div class="alerts-header">
                    <span class="alerts-title">"Firing Alerts"</span>
                    <button
                        class="alerts-silence-toggle"
                        class:active=move || show_silenced.get()
                        on:click=move |_| show_silenced.update(|s| *s = !*s)
                    >
                        "Silenced"
                    </button>
                    <button class="alerts-close" on:click=move |_| do_close()>"✕"</button>
                </div>
                <div class="alerts-list">
                    <Show when=move || sorted_alerts.get().is_empty()>
                        <p class="alerts-empty">"No firing alerts"</p>
                    </Show>
                    <For
                        each=move || sorted_alerts.get()
                        key=|a| a.fingerprint.clone()
                        let:alert
                    >
                        <AlertRow alert />
                    </For>
                </div>
            </div>
        </Show>
    }
}

#[component]
fn AlertRow(alert: FiringAlert) -> impl IntoView {
    let tick = use_context::<crate::app::state::Tick>().map(|t| t.0);

    let starts_at = alert.starts_at.clone();
    let duration_str = move || {
        tick.map(|t| t.get());
        format_duration(&starts_at)
    };

    let sev_class = format!("alert-sev sev-{}", alert.severity);

    view! {
        <div class="alert-row">
        <div class="alert-cw">
        <div class="alert-cwi">
            <div class="alert-row-header">
                <span class=sev_class>{alert.severity.clone()}</span>
                <span class="alert-name">{alert.name.clone()}</span>
                {alert.silenced.then(|| view! {
                    <span class="alert-sev sev-silenced">"Silenced"</span>
                })}
                <span class="alert-age">{duration_str}</span>
            </div>
            {(!alert.summary.is_empty()).then(|| view! {
                <p class="alert-summary">{alert.summary.clone()}</p>
            })}
            {(!alert.description.is_empty()).then(|| view! {
                <p class="alert-desc">{alert.description.clone()}</p>
            })}
            <div class="alert-labels">
                {alert.labels
                    .into_iter()
                    .filter(|(k, _)| k != "alertname" && k != "severity")
                    .map(|(k, v)| view! {
                        <span class="label-chip">
                            <span class="label-key">{k}</span>"="{v}
                        </span>
                    })
                    .collect_view()}
            </div>
        </div>
        </div>
        </div>
    }
}

fn sev_order(sev: &str) -> u8 {
    match sev {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

/// Compute a human-readable elapsed duration from an ISO 8601 `starts_at` string.
///
/// On wasm32 we use `js_sys::Date` to parse the timestamp and compute elapsed
/// seconds, then delegate to `roder_core::format_age_secs` for formatting
/// (e.g. "3d1h", "1h30m", "5m", "45s"). On SSR we have no wall clock, so we
/// return the raw `starts_at` string as a fallback.
fn format_duration(iso: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let parsed =
            js_sys::Date::new(&wasm_bindgen::JsValue::from_str(iso)).get_time();
        if parsed.is_nan() {
            return iso.to_string();
        }
        let secs = ((js_sys::Date::now() - parsed) / 1000.0).max(0.0) as u64;
        roder_core::format_age_secs(secs)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        iso.to_string()
    }
}
