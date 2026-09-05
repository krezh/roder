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
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    crate::app::ui::use_dialog_focus(dialog_ref);

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
            <div class="alerts-panel" class:closing=move || closing.get() node_ref=dialog_ref
                role="dialog" aria-modal="true" tabindex="-1">
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
    let duration = RwSignal::new("3600".to_string());
    let mut available_matchers: Vec<_> = alert
        .labels
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    available_matchers.sort_by(|left, right| left.0.cmp(&right.0));
    let all_matchers = StoredValue::new(
        available_matchers
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
    );
    let mut initial_matchers: std::collections::HashSet<_> = available_matchers
        .iter()
        .filter(|(name, _)| matches!(name.as_str(), "alertname" | "namespace"))
        .map(|(name, _)| name.clone())
        .collect();
    if initial_matchers.is_empty() {
        initial_matchers.extend(available_matchers.iter().map(|(name, _)| name.clone()));
    }
    let selected_matchers = RwSignal::new(initial_matchers);
    let matchers_valid = Memo::new(move |_| {
        !crate::app::alert_silence_matchers(&all_matchers.read_value(), &selected_matchers.read())
            .is_empty()
    });
    let silencing = RwSignal::new(false);
    let silence_error = RwSignal::new(None::<String>);
    let silence_dialog_open = RwSignal::new(false);

    let starts_at = alert.starts_at.clone();
    let alert_name = alert.name.clone();
    let dialog_matchers = StoredValue::new(available_matchers.clone());
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
                    duration: duration
                        .get_untracked()
                        .parse()
                        .map_or(roder_core::AlertSilenceDuration::Forever, |seconds| {
                            roder_core::AlertSilenceDuration::Finite { seconds }
                        }),
                    matcher_labels: crate::app::alert_silence_matchers(
                        &all_matchers.read_value(),
                        &selected_matchers.read(),
                    ),
                };
                let body = serde_json::to_value(request).unwrap_or_default();
                match crate::data::post_json::<serde_json::Value>("/api/alerts/silences", &body)
                    .await
                {
                    Ok(_) => {
                        silence_dialog_open.set(false);
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
                {available_matchers.clone().into_iter()
                    .filter(|(name, _)| name != "alertname" && name != "severity")
                    .map(|(name, value)| view! { <span class="label-chip"><span class="label-key">{name}</span>"="{value}</span> })
                    .collect_view()}
            </div>
            <Show when=move || silences_enabled.get() && !silenced>
                <div class="alert-silence-actions">
                    <button
                        class="alert-silence-submit"
                        on:click=move |_| silence_dialog_open.set(true)
                    >
                        "Silence"
                    </button>
                </div>
            </Show>
        </div>
        </div>
        </div>
        <Show when=move || silence_dialog_open.get()>
            <div class="modal-scrim alert-silence-scrim" on:click=move |_| {
                if !silencing.get_untracked() { silence_dialog_open.set(false); }
            }></div>
            <section class="modal delete-modal alert-silence-dialog" role="dialog" aria-modal="true">
                <div class="modal-msg">{format!("Silence {alert_name}")}</div>
                <div class="delete-options">
                    <div class="delete-opt alert-match-opt">
                        <div class="alert-match-copy">
                            <span>"Match labels"</span>
                            <div class="alert-selected-matchers">
                                {move || {
                                    let selected = selected_matchers.read();
                                    dialog_matchers.get_value().into_iter()
                                        .filter(|(name, _)| selected.contains(name))
                                        .map(|(name, value)| view! { <span class="label-chip">{name}"="{value}</span> })
                                        .collect_view()
                                }}
                            </div>
                        </div>
                        <Dropdown label=move || {
                            let count = selected_matchers.read().len();
                            format!("{count} label{}", if count == 1 { "" } else { "s" })
                        }>
                            {dialog_matchers.get_value().into_iter().map(|(name, value)| view! {
                                <SilenceMatcherItem selected=selected_matchers name value />
                            }).collect_view()}
                        </Dropdown>
                    </div>
                    <div class="delete-opt">
                        <span>"Duration"</span>
                        <Dropdown label=move || match duration.get().as_str() {
                            "21600" => "6 hours", "86400" => "1 day", "604800" => "1 week",
                            "forever" => "Forever", _ => "1 hour",
                        }.to_string()>
                            <SilenceMenuItem selection=duration value="3600" label="1 hour" />
                            <SilenceMenuItem selection=duration value="21600" label="6 hours" />
                            <SilenceMenuItem selection=duration value="86400" label="1 day" />
                            <SilenceMenuItem selection=duration value="604800" label="1 week" />
                            <SilenceMenuItem selection=duration value="forever" label="Forever" />
                        </Dropdown>
                    </div>
                </div>
                <div class="alert-dialog-error">{move || if !matchers_valid.get() { Some("Select at least one label".to_string()) } else { silence_error.get() }}</div>
                <div class="modal-actions">
                    <button class="act" disabled=move || silencing.get() on:click=move |_| silence_dialog_open.set(false)>"Cancel"</button>
                    <button class="act danger" disabled=move || silencing.get() || !matchers_valid.get() on:click=move |_| silence.run(())>
                        {move || if silencing.get() { "Silencing..." } else { "Create silence" }}
                    </button>
                </div>
            </section>
        </Show>
    }
}

#[component]
fn SilenceMatcherItem(
    selected: RwSignal<std::collections::HashSet<String>>,
    name: String,
    value: String,
) -> impl IntoView {
    let selected_name = name.clone();
    let changed_name = name.clone();
    let marked_name = name.clone();
    view! {
        <button type="button" class="dropdown-item alert-matcher-item"
            role="menuitemcheckbox"
            aria-checked=move || selected.read().contains(&selected_name).to_string()
            on:click=move |_| selected.update(|labels| {
                if !labels.remove(&changed_name) {
                    labels.insert(changed_name.clone());
                }
            })>
            <span class="alert-matcher-mark" class:selected=move || selected.read().contains(&marked_name)></span>
            <span><strong>{name}</strong><small>{value}</small></span>
        </button>
    }
}

#[component]
fn SilenceMenuItem(
    selection: RwSignal<String>,
    value: &'static str,
    label: &'static str,
) -> impl IntoView {
    let close = expect_context::<DropdownClose>().0;
    view! {
        <button type="button" class="dropdown-item" role="menuitem" on:click=move |_| {
            selection.set(value.to_string());
            close.run(());
        }>{label}</button>
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
