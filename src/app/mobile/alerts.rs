use leptos::prelude::*;
use roder_core::FiringAlert;

use crate::app::state::{AlertSilencesEnabled, AlertsData, AlertsLastRefresh, AlertsOpen, Tick};
use crate::app::ui::use_bool_overlay;

fn severity_order(severity: &str) -> u8 {
    match severity {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

fn elapsed(timestamp: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let parsed = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(timestamp)).get_time();
        if parsed.is_nan() {
            return timestamp.to_string();
        }
        return roder_core::format_age_secs(
            ((js_sys::Date::now() - parsed) / 1000.0).max(0.0) as u64
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    timestamp.to_string()
}

fn refresh_label(last_refresh: Option<f64>, failed: bool) -> String {
    if failed {
        return "Refresh failed".into();
    }
    let Some(timestamp) = last_refresh else {
        return "Not refreshed yet".into();
    };
    #[cfg(target_arch = "wasm32")]
    return format!(
        "Updated {} ago",
        roder_core::format_age_secs(((js_sys::Date::now() - timestamp) / 1000.0).max(0.0) as u64)
    );
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = timestamp;
        "Updated".into()
    }
}

#[component]
pub(crate) fn MobileAlertsPanel() -> impl IntoView {
    let open = expect_context::<AlertsOpen>().0;
    let data = expect_context::<AlertsData>().0;
    let last_refresh = expect_context::<AlertsLastRefresh>().0;
    let silences_enabled = expect_context::<AlertSilencesEnabled>().0;
    let tick = expect_context::<Tick>().0;
    let (visible, closing, close) = use_bool_overlay(open);
    let show_silenced = RwSignal::new(false);
    let refreshing = RwSignal::new(false);
    let refresh_error = RwSignal::new(None::<String>);
    let alerts = Memo::new(move |_| {
        let mut alerts: Vec<_> = data
            .get()
            .unwrap_or_default()
            .into_iter()
            .filter(|alert| show_silenced.get() || !alert.silenced)
            .collect();
        alerts.sort_by(|left, right| {
            severity_order(&left.severity)
                .cmp(&severity_order(&right.severity))
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.fingerprint.cmp(&right.fingerprint))
        });
        alerts
    });
    let refresh = move |_| {
        #[cfg(target_arch = "wasm32")]
        if !refreshing.get_untracked() {
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
    view! { <Show when=move || visible.get()>
        <section class="mobile-alerts-panel" class:closing=move || closing.get()>
            <header class="mobile-alerts-head"><div><small>"Monitoring"</small><strong>"Firing alerts"</strong><span class:error=move || refresh_error.get().is_some()>{move || { tick.track(); refresh_label(last_refresh.get(), refresh_error.get().is_some()) }}</span></div>
                <button class:active=move || show_silenced.get() on:click=move |_| show_silenced.update(|value| *value = !*value)>"Silenced"</button>
                <button disabled=move || refreshing.get() on:click=refresh>{move || if refreshing.get() { "…" } else { "↻" }}</button>
                <button aria-label="Close alerts" on:click=move |_| close()>"×"</button>
            </header>
            <div class="mobile-alerts-list">
                {move || alerts.with(|items| items.is_empty()).then(|| view! { <p class="mobile-alerts-empty">"No firing alerts"</p> })}
                <For each=move || alerts.get() key=|alert| (alert.fingerprint.clone(), alert.silenced) let:alert>
                    <MobileAlertRow alert data last_refresh silences_enabled />
                </For>
            </div>
        </section>
    </Show> }
}

#[component]
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
fn MobileAlertRow(
    alert: FiringAlert,
    data: RwSignal<Option<Vec<FiringAlert>>>,
    last_refresh: RwSignal<Option<f64>>,
    silences_enabled: RwSignal<bool>,
) -> impl IntoView {
    let tick = expect_context::<Tick>().0;
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
    let fingerprint = StoredValue::new(alert.fingerprint.clone());
    let starts_at = alert.starts_at.clone();
    let alert_name = alert.name.clone();
    let dialog_matchers = StoredValue::new(available_matchers.clone());
    let silenced = alert.silenced;
    let silence = move |_| {
        #[cfg(target_arch = "wasm32")]
        if !silencing.get_untracked() {
            silencing.set(true);
            silence_error.set(None);
            let fingerprint = fingerprint.get_value();
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
                match crate::data::post_json::<serde_json::Value>(
                    "/api/alerts/silences",
                    &serde_json::to_value(request).unwrap_or_default(),
                )
                .await
                {
                    Ok(_) => {
                        silence_dialog_open.set(false);
                        data.update(|alerts| {
                            if let Some(alert) = alerts.as_mut().and_then(|alerts| {
                                alerts
                                    .iter_mut()
                                    .find(|alert| alert.fingerprint == fingerprint)
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
    };
    view! { <article class="mobile-alert-row">
        <header><span class=format!("severity {}", alert.severity)>{alert.severity.clone()}</span><strong>{alert.name}</strong>
            {alert.silenced.then(|| view! { <span class="silenced">"Silenced"</span> })}<time>{move || { tick.get(); elapsed(&starts_at) }}</time>
        </header>
        {(!alert.summary.is_empty()).then(|| view! { <p class="summary">{alert.summary}</p> })}
        {(!alert.description.is_empty()).then(|| view! { <p>{alert.description}</p> })}
        <div class="mobile-alert-labels">{available_matchers.clone().into_iter().filter(|(key, _)| key != "alertname" && key != "severity").map(|(key, value)| view! { <span><b>{key}</b>"="{value}</span> }).collect_view()}</div>
        <Show when=move || silences_enabled.get() && !silenced><div class="mobile-silence-actions">
            <button on:click=move |_| silence_dialog_open.set(true)>"Silence"</button>
        </div></Show>
    </article>
    <Show when=move || silence_dialog_open.get()>
        <div class="mobile-modal-scrim mobile-silence-scrim" on:click=move |_| { if !silencing.get_untracked() { silence_dialog_open.set(false); } }></div>
        <section class="mobile-dialog mobile-silence-dialog" role="dialog" aria-modal="true">
            <p class="mobile-dialog-message">"Silence "<strong>{alert_name.clone()}</strong></p>
            <div class="mobile-silence-matchers">
                <span>"Match labels"</span>
                {dialog_matchers.get_value().into_iter().map(|(name, value)| {
                    let checked_name = name.clone(); let changed_name = name.clone();
                    view! { <label class="mobile-dialog-check"><input type="checkbox" prop:checked=move || selected_matchers.read().contains(&checked_name)
                        on:change=move |event| selected_matchers.update(|selected| { if event_target_checked(&event) { selected.insert(changed_name.clone()); } else { selected.remove(&changed_name); } }) />
                        <span><strong>{name}</strong><small>{value}</small></span></label> }
                }).collect_view()}
            </div>
            <label class="mobile-dialog-field"><span>"Duration"</span><select prop:value=move || duration.get() on:change=move |event| duration.set(event_target_value(&event))>
                <option value="3600">"1 hour"</option><option value="21600">"6 hours"</option><option value="86400">"1 day"</option><option value="604800">"1 week"</option><option value="forever">"Forever"</option>
            </select></label>
            <small class="mobile-dialog-error">{move || if !matchers_valid.get() { Some("Select at least one label".to_string()) } else { silence_error.get() }}</small>
            <div class="mobile-dialog-actions"><button disabled=move || silencing.get() on:click=move |_| silence_dialog_open.set(false)>"Cancel"</button>
                <button disabled=move || silencing.get() || !matchers_valid.get() on:click=silence>{move || if silencing.get() { "Silencing…" } else { "Create silence" }}</button></div>
        </section>
    </Show> }
}
