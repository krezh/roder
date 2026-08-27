//! Small table building blocks: status dot, flashing cell, sortable header, and
//! the workload scale control.

use leptos::prelude::*;
use roder_core::{RowStatus, Trend};

use crate::app::state::{SortKey, Tick};
use crate::app::util::color::dot_class;
use crate::data;

fn primary_value(value: &str) -> &str {
    value.split_once('\x1f').map_or(value, |parts| parts.0)
}

pub(crate) fn cmp_str(a: &str, b: &str) -> std::cmp::Ordering {
    let a_val = primary_value(a);
    let b_val = primary_value(b);
    match (a_val.parse::<f64>(), b_val.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a_val.cmp(b_val),
    }
}

pub(crate) fn cell_value_changed(current: &str, previous: &str) -> bool {
    primary_value(current) != primary_value(previous)
}

/// Compare two optional cell values, numerically when both parse as numbers.
pub(crate) fn cmp_cell(a: Option<&String>, b: Option<&String>) -> std::cmp::Ordering {
    cmp_str(
        a.map(|s| s.as_str()).unwrap_or(""),
        b.map(|s| s.as_str()).unwrap_or(""),
    )
}

/// A sortable column header. Clicking sets/toggles the table sort.
pub(crate) fn sortable_th(
    label: String,
    key: SortKey,
    sort: RwSignal<(SortKey, bool)>,
) -> impl IntoView {
    let identity_edge = key == SortKey::Name;
    let arrow = move || {
        let (k, asc) = sort.get();
        if k == key {
            if asc {
                " ▲"
            } else {
                " ▼"
            }
        } else {
            ""
        }
    };
    view! {
        <div class="cell sortable" class:identity-edge=identity_edge
            class:active=move || sort.get().0 == key
            on:click=move |_| sort.update(|s| {
                if s.0 == key { s.1 = !s.1; } else { s.0 = key; s.1 = true; }
            })>
            {label}<span class="sort-arrow">{arrow}</span>
        </div>
    }
}

#[component]
pub(crate) fn StatusDot(status: RowStatus) -> impl IntoView {
    view! { <span class=format!("dot dot-{}", dot_class(status))></span> }
}

/// A table cell that flashes briefly whenever its value changes (skips first
/// render). Pass `flash` to use a row-level signal instead of a per-cell
/// Effect — one Effect at the row level then fans out to each cell via bits.
/// Pass `no_flash=true` to disable flashing entirely (e.g. metric columns).
#[component]
pub(crate) fn FlashTd<F>(
    value: F,
    #[prop(optional)] class: &'static str,
    #[prop(optional, into)] color: Option<Signal<&'static str>>,
    #[prop(optional)] no_flash: bool,
    #[prop(optional, into)] flash: Option<Signal<bool>>,
    #[prop(optional)] trend: Option<Signal<Trend>>,
    /// Render `color` as a solid-fill status pill around the value instead of
    /// coloring the cell text directly (used for Phase/Status/Ready columns).
    #[prop(optional)]
    pill: bool,
    /// Render a small proportional bar under the value, filled by `color`
    /// (falling back to `ok` when `color` is empty/unset) — used for the
    /// %CPU/%MEM saturation columns.
    #[prop(optional)]
    pct_bar: bool,
) -> impl IntoView
where
    F: Fn() -> String + Copy + Send + Sync + 'static,
{
    let tick = expect_context::<Tick>().0;
    let value_changed = RwSignal::new(false);
    Effect::new(move |prev: Option<String>| {
        let v = value();
        if let Some(p) = prev {
            if p != v {
                value_changed.set(true);
                set_timeout(
                    move || value_changed.set(false),
                    std::time::Duration::from_millis(1500),
                );
            }
        }
        v
    });

    let flash_state: Signal<bool> = if let Some(sig) = flash {
        sig
    } else if no_flash {
        Signal::derive(|| false)
    } else {
        value_changed.into()
    };

    let trend_arrow = move || {
        if value_changed.get() {
            trend.and_then(|t| t.get().arrow())
        } else {
            None
        }
    };
    let trend_class = move || {
        trend
            .map(|t| match t.get() {
                Trend::Up => "trend-arrow trend-up",
                Trend::Down => "trend-arrow trend-down",
                Trend::None => "trend-arrow",
            })
            .unwrap_or("trend-arrow")
    };
    let display_value = move || {
        let value = value();
        if data::cell_needs_tick(&value) {
            tick.get();
        }
        data::humanize_cell(&value)
    };
    view! {
        <div class=format!("cell {class}") class:flash=move || flash_state.get()
            data-tip=move || display_value().replace('\x1f', " ")
            style=move || color.map(|c| {
                let v = c.get();
                if v.is_empty() { return String::new(); }
                let flash_bg = format!("--flash-bg:color-mix(in srgb, var(--{v}) 45%, transparent)");
                if pill {
                    flash_bg
                } else {
                    format!("color:var(--{v});{flash_bg}")
                }
            }).unwrap_or_default()>
            // Newlines mark a list value; collapse to a compact inline form for the
            // cell (the tooltip renders the real list from data-tip).
            <div class="cw"><div class="cwi">
                {move || {
                    let v = display_value().replace('\n', ", ");
                    if pill {
                        let style = color.map(|c| c.get()).filter(|s| !s.is_empty())
                            .map(|s| format!("background:var(--{s});color:var(--pill-fg)"))
                            .unwrap_or_default();
                        view! { <span class="pill" style=style>{v}</span> }.into_any()
                    } else if pct_bar {
                        let bar_color = color.map(|c| c.get()).filter(|s| !s.is_empty()).unwrap_or("ok");
                        let pct = v.parse::<f64>().ok().map(|p| p.clamp(0.0, 100.0));
                        view! {
                            <div class="pctcell">
                                <span>{v.clone()}</span>
                                {pct.map(|p| view! {
                                    <div class="pct-bar"><div class="pct-fill"
                                        style=format!("width:{p}%;background:var(--{bar_color})")></div></div>
                                })}
                            </div>
                        }.into_any()
                    } else if let Some((main, hint)) = v.split_once('\x1f') {
                        view! {
                            <span>{main.to_string()} <span class="hint" style="font-size: 0.85em; opacity: 0.8; margin-left: 0.5ch;">{hint.to_string()}</span></span>
                        }.into_any()
                    } else {
                        v.into_any()
                    }
                }}
                {move || trend_arrow().map(|a| view! { <span class=trend_class>{a}</span> })}
            </div></div>
        </div>
    }
}

#[component]
pub(crate) fn ScaleControl<F>(run: F, #[prop(into)] current: Signal<Option<i32>>) -> impl IntoView
where
    F: Fn(&'static str, serde_json::Value) + Clone + 'static,
{
    let replicas = RwSignal::new(None::<i32>);
    Effect::new(move |_| {
        replicas.set(current.get());
    });
    view! {
        <span class="scale">
            <input type="number" min="0" class="scale-input"
                disabled=move || current.get().is_none()
                prop:value=move || replicas.get().map(|n| n.to_string()).unwrap_or_default()
                on:input=move |e| replicas.set(event_target_value(&e).parse::<i32>().ok()) />
            <button class="act"
                disabled=move || current.get().is_none() || replicas.get().is_none()
                on:click=move |_| {
                    if current.get_untracked().is_some() {
                        if let Some(replicas) = replicas.get_untracked() {
                            run("scale", serde_json::json!({ "replicas": replicas }));
                        }
                    }
                }>"Scale"</button>
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::cell_value_changed;

    #[test]
    fn hint_changes_do_not_count_as_cell_value_changes() {
        assert!(!cell_value_changed("2\x1f5m ago", "2\x1f4m ago"));
        assert!(cell_value_changed("3\x1f1s ago", "2\x1f5m ago"));
    }
}
