use leptos::prelude::*;
use roder_core::FiringAlert;

use super::use_bool_overlay;
use crate::app::components::dropdown::{Dropdown, DropdownClose};
use crate::app::state::{AlertSilencesEnabled, AlertsData, AlertsLastRefresh, AlertsOpen, Tick};

#[component]
pub(crate) fn AlertsPanel() -> impl IntoView {
    let open = expect_context::<AlertsOpen>().0;
    let data = expect_context::<AlertsData>().0;
    let last_refresh = expect_context::<AlertsLastRefresh>().0;
    let silences_enabled = expect_context::<AlertSilencesEnabled>().0;
    let tick = expect_context::<Tick>().0;
    let (visible, closing, do_close) = use_bool_overlay(open);
    let show_silenced = RwSignal::new(false);
    let refreshing = RwSignal::new(false);
    let refresh_error = RwSignal::new(None::<String>);

    let refresh = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            if refreshing.get_untracked() {
                return;
            }
            refreshing.set(true);
            refresh_error.set(None);
            leptos::task::spawn_local(async move {
                match crate::app::fetch_alerts(true).await {
                    Ok(alerts) => crate::app::update_alerts(data, last_refresh, alerts),
                    Err(error) => {
                        refresh_error.set(Some(error));
                        set_timeout(
                            move || refresh_error.set(None),
                            std::time::Duration::from_secs(4),
                        );
                    }
                }
                refreshing.set(false);
            });
        }
    };

    let sorted_alerts = Memo::new(move |_| {
        let all = data.get().unwrap_or_default();
        let show_sil = show_silenced.get();
        let mut alerts: Vec<_> = all
            .into_iter()
            .filter(|a| show_sil || !a.silenced)
            .collect();
        alerts.sort_by(|a, b| {
            sev_order(&a.severity)
                .cmp(&sev_order(&b.severity))
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
                    <div class="alerts-heading">
                        <span class="alerts-title">"Firing Alerts"</span>
                        <span
                            class="alerts-refreshed"
                            class:error=move || refresh_error.get().is_some()
                            data-tip=move || refresh_error.get().unwrap_or_default()
                        >
                            {move || {
                                tick.track();
                                refresh_status(last_refresh.get(), refresh_error.get().is_some())
                            }}
                        </span>
                    </div>
                    <button
                        class="alerts-silence-toggle"
                        class:active=move || show_silenced.get()
                        on:click=move |_| show_silenced.update(|s| *s = !*s)
                    >
                        "Silenced"
                    </button>
                    <button
                        class="alerts-refresh"
                        disabled=move || refreshing.get()
                        on:click=refresh
                    >
                        {move || if refreshing.get() { "Refreshing..." } else { "Refresh" }}
                    </button>
                    <button class="alerts-close" on:click=move |_| do_close()>"✕"</button>
                </div>
                <div class="alerts-list">
                    <Show when=move || sorted_alerts.get().is_empty()>
                        <p class="alerts-empty">"No firing alerts"</p>
                    </Show>
                    <For
                        each=move || sorted_alerts.get()
                        key=|a| (a.fingerprint.clone(), a.silenced)
                        let:alert
                    >
                        <AlertRow alert data last_refresh silences_enabled />
                    </For>
                </div>
            </div>
        </Show>
    }
}

fn refresh_status(last_refresh_ms: Option<f64>, failed: bool) -> String {
    if failed {
        return "Refresh failed".to_string();
    }
    let Some(ms) = last_refresh_ms else {
        return "Not refreshed yet".to_string();
    };

    #[cfg(target_arch = "wasm32")]
    {
        let elapsed_secs = ((js_sys::Date::now() - ms) / 1000.0).max(0.0) as u64;
        return format!(
            "Last refreshed {} ago",
            roder_core::format_age_secs(elapsed_secs)
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = ms;
        "Last refreshed".to_string()
    }
}

#[component]
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
fn AlertRow(
    alert: FiringAlert,
    data: RwSignal<Option<Vec<FiringAlert>>>,
    last_refresh: RwSignal<Option<f64>>,
    silences_enabled: RwSignal<bool>,
) -> impl IntoView {
    let tick = use_context::<crate::app::state::Tick>().map(|t| t.0);
    let duration_amount = RwSignal::new(1u64);
    let duration_unit = RwSignal::new(3_600u64);
    let duration_secs =
        Memo::new(move |_| duration_amount.get().saturating_mul(duration_unit.get()));
    let duration_valid = Memo::new(move |_| {
        (roder_core::MIN_ALERT_SILENCE_SECS..=roder_core::MAX_ALERT_SILENCE_SECS)
            .contains(&duration_secs.get())
    });
    let duration_unit_label = move || {
        match duration_unit.get() {
            60 => "minutes",
            86_400 => "days",
            604_800 => "weeks",
            _ => "hours",
        }
        .to_string()
    };
    let silencing = RwSignal::new(false);
    let silence_error = RwSignal::new(None::<String>);

    let starts_at = alert.starts_at.clone();
    let duration_str = move || {
        tick.map(|t| t.get());
        format_duration(&starts_at)
    };

    let sev_class = format!("alert-sev sev-{}", alert.severity);
    let fingerprint = alert.fingerprint.clone();
    let silenced = alert.silenced;
    let silence = Callback::new(move |()| {
        #[cfg(target_arch = "wasm32")]
        {
            if silencing.get_untracked() {
                return;
            }
            silencing.set(true);
            silence_error.set(None);
            let fingerprint = fingerprint.clone();
            leptos::task::spawn_local(async move {
                let request = roder_core::SilenceAlertRequest {
                    fingerprint: fingerprint.clone(),
                    duration_secs: duration_secs.get_untracked(),
                };
                let body = serde_json::to_value(request).unwrap_or_default();
                match crate::data::post_json::<serde_json::Value>("/api/alerts/silences", &body)
                    .await
                {
                    Ok(_) => {
                        data.update(|alerts| {
                            if let Some(alert) = alerts.as_mut().and_then(|alerts| {
                                alerts.iter_mut().find(|a| a.fingerprint == fingerprint)
                            }) {
                                alert.silenced = true;
                            }
                        });
                        if let Ok(alerts) = crate::app::fetch_alerts(true).await {
                            crate::app::update_alerts(data, last_refresh, alerts);
                        }
                    }
                    Err(error) => {
                        silence_error.set(Some(error));
                        set_timeout(
                            move || silence_error.set(None),
                            std::time::Duration::from_secs(4),
                        );
                    }
                }
                silencing.set(false);
            });
        }
    });

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
            <Show when=move || silences_enabled.get() && !silenced>
                <div class="alert-silence-actions">
                    <input
                        class="alert-duration-input"
                        type="number"
                        min="1"
                        step="1"
                        aria-label="Silence duration"
                        prop:value=move || duration_amount.get().to_string()
                        disabled=move || silencing.get()
                        on:input=move |event| {
                            duration_amount.set(event_target_value(&event).parse().unwrap_or(0));
                        }
                    />
                    <Dropdown label=duration_unit_label>
                        <DurationUnitItem duration_unit value=60 label="minutes" />
                        <DurationUnitItem duration_unit value=3_600 label="hours" />
                        <DurationUnitItem duration_unit value=86_400 label="days" />
                        <DurationUnitItem duration_unit value=604_800 label="weeks" />
                    </Dropdown>
                    <button
                        class="alert-silence-submit"
                        disabled=move || silencing.get() || !duration_valid.get()
                        on:click=move |_| silence.run(())
                    >
                        {move || if silencing.get() { "Silencing..." } else { "Silence" }}
                    </button>
                    <span class="alert-duration-error">
                        {move || (!duration_valid.get()).then_some("Choose 1 minute to 1 year")}
                    </span>
                    <span class="alert-silence-error">{move || silence_error.get()}</span>
                </div>
            </Show>
        </div>
        </div>
        </div>
    }
}

#[component]
fn DurationUnitItem(
    duration_unit: RwSignal<u64>,
    value: u64,
    label: &'static str,
) -> impl IntoView {
    let close = expect_context::<DropdownClose>().0;
    view! {
        <button type="button" class="dropdown-item" on:click=move |_| {
            duration_unit.set(value);
            close.run(());
        }>
            {label}
        </button>
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
        let parsed = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(iso)).get_time();
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
