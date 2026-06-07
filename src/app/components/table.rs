//! Small table building blocks: status dot, flashing cell, sortable header, and
//! the workload scale control.

use leptos::prelude::*;
use roder_core::{RowStatus, Trend};

use crate::app::state::SortKey;
use crate::app::util::color::dot_class;

/// Compare two cell values, numerically when both parse as numbers.
pub(crate) fn cmp_cell(a: Option<&String>, b: Option<&String>) -> std::cmp::Ordering {
    let a = a.map(|s| s.as_str()).unwrap_or("");
    let b = b.map(|s| s.as_str()).unwrap_or("");
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.cmp(b),
    }
}

/// A sortable column header. Clicking sets/toggles the table sort.
pub(crate) fn sortable_th(
    label: String,
    key: SortKey,
    sort: RwSignal<(SortKey, bool)>,
) -> impl IntoView {
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
        <div class="cell sortable" class:active=move || sort.get().0 == key
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

/// A table cell that flashes briefly whenever its own value changes (skips the
/// first render), so live updates highlight just the cell that changed.
/// When a `trend` signal is provided, an inline arrow (↑ red / ↓ green) is shown
/// next to the value; the arrow fades out after 1 s.
#[component]
pub(crate) fn FlashTd<F>(
    value: F,
    #[prop(optional)] class: &'static str,
    #[prop(optional, into)] color: Option<Signal<&'static str>>,
    #[prop(optional)] no_flash: bool,
    #[prop(optional)] trend: Option<Signal<Trend>>,
) -> impl IntoView
where
    F: Fn() -> String + Copy + Send + Sync + 'static,
{
    let flash = RwSignal::new(false);
    if !no_flash {
        Effect::new(move |prev: Option<String>| {
            let v = value();
            if let Some(p) = prev {
                if p != v {
                    flash.set(true);
                    set_timeout(
                        move || flash.set(false),
                        std::time::Duration::from_millis(1500),
                    );
                }
            }
            v
        });
    }
    let trend_arrow = move || trend.and_then(|t| t.get().arrow());
    let trend_class = move || {
        trend
            .map(|t| match t.get() {
                Trend::Up => "trend-arrow trend-up",
                Trend::Down => "trend-arrow trend-down",
                Trend::None => "trend-arrow",
            })
            .unwrap_or("trend-arrow")
    };
    view! {
        <div class=format!("cell {class}") class:flash=move || flash.get()
            data-tip=value
            style=move || color.map(|c| {
                let v = c.get();
                format!("color:var(--{v});--flash-bg:color-mix(in srgb, var(--{v}) 45%, transparent)")
            }).unwrap_or_default()>
            // Newlines mark a list value; collapse to a compact inline form for the
            // cell (the tooltip renders the real list from data-tip).
            <div class="cw"><div class="cwi">
                {move || value().replace('\n', ", ")}
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
    let replicas = RwSignal::new(0i32);
    // Pre-fill with the live spec.replicas once the object loads.
    Effect::new(move |_| {
        if let Some(n) = current.get() {
            replicas.set(n);
        }
    });
    view! {
        <span class="scale">
            <input type="number" min="0" class="scale-input"
                prop:value=move || replicas.get().to_string()
                on:input=move |e| { if let Ok(n) = event_target_value(&e).parse::<i32>() { replicas.set(n); } } />
            <button class="act" on:click=move |_| run("scale", serde_json::json!({ "replicas": replicas.get() }))>"Scale"</button>
        </span>
    }
}
