//! The main live resource table: a windowed (virtualized) list with filtering,
//! sorting, multi-select, bulk actions, and live enter/exit animations. Only the
//! rows in (or near) the viewport are mounted, so list size doesn't affect cost.

use std::collections::HashMap;

use leptos::prelude::*;
use roder_core::{ResourceKind, ResourceRow, RowStatus, Trend};

use crate::app::components::table::{cmp_cell, sortable_th, FlashTd};
use crate::app::events::{fire_action, range_select};
use crate::app::hooks::use_sse_subscription;
use crate::app::overlays::confirm::{ask_confirm, Confirm};
use crate::app::state::{
    open_logs, CtxMenu, DetailTarget, LogPods, LogTarget, OnlyProblems, ResourceFilter, SortKey, Tick,
};
use crate::app::util::color::{dot_class, name_color};
use crate::app::views::dashboard::Dashboard;
use crate::data;

/// Rows rendered beyond the viewport on each side, so scrolling doesn't flash blanks.
const OVERSCAN: usize = 12;
/// Column width bounds (in `ch`), derived from the data's longest value.
const MIN_CH: usize = 5;
const CAP_CH: usize = 44;
/// Added to the longest value to leave room for the cell's horizontal padding.
const PAD_CH: usize = 3;

/// Displayed character width of a cell (newline list values render as `", "`).
fn disp_len(s: &str) -> usize {
    s.chars().count() + s.matches('\n').count()
}

fn col_width(max_chars: usize) -> usize {
    (max_chars + PAD_CH).clamp(MIN_CH, CAP_CH)
}

/// Known minimum visual widths for columns whose content we project ourselves.
/// Ensures columns like Status never shrink below what the values can actually be,
/// even if the initial snapshot only has short values like "Running".
fn min_width(header: &str) -> usize {
    match header {
        "Ready" | "Available" | "Completions" => 5,
        "Status" => 24,
        "Restarts" => 3,
        "CPU" => 8,
        "MEM" => 7,
        "%CPU/R" | "%CPU/L" | "%MEM/R" | "%MEM/L" => 5,
        "IP" => 15,
        "Node" => 10,
        "Phase" => 8,
        "Version" => 12,
        "Type" | "Store" => 8,
        _ => 0,
    }
}

#[component]
pub(crate) fn ResourceView() -> impl IntoView {
    let log_pods = expect_context::<LogPods>().0;
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let ctx_menu = expect_context::<RwSignal<Option<CtxMenu>>>();
    let tick = expect_context::<Tick>().0;
    let only_problems = expect_context::<OnlyProblems>().0;
    let resource_filter = expect_context::<ResourceFilter>().0;
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let rows = RwSignal::new(HashMap::<String, ResourceRow>::new());
    let selected = RwSignal::new(std::collections::BTreeSet::<String>::new());
    // Anchor for shift-click range selection (uid of the last individually-toggled row).
    let last_clicked = RwSignal::new(None::<String>);
    let sort = RwSignal::new((SortKey::Namespace, true));
    // uids briefly tagged for the enter/exit animations.
    let entering = RwSignal::new(std::collections::BTreeSet::<String>::new());
    let removing = RwSignal::new(std::collections::BTreeSet::<String>::new());
    // Virtual window: scroll offset + viewport height + measured row height drive
    // which slice of the sorted list is actually mounted.
    let scroll_top = RwSignal::new(0.0f64);
    // A generous default so the first paint isn't too short before the rAF measure.
    let viewport_h = RwSignal::new(1000.0f64);
    let row_h = RwSignal::new(35.0f64);
    let table_ref = NodeRef::<leptos::html::Div>::new();
    // Long-press ("hold") to enter multi-select on touch/mouse.
    let press_handle = StoredValue::new(None::<TimeoutHandle>);
    let press_xy = StoredValue::new((0i32, 0i32));
    let press_fired = StoredValue::new(false);
    let cancel_press = move || {
        press_handle.update_value(|h| {
            if let Some(h) = h.take() {
                h.clear();
            }
        });
    };

    // (Re)subscribe to the live list whenever the selected kind/namespace changes.
    use_sse_subscription(rows, entering, removing, move || {
        rows.set(HashMap::new());
        selected.set(std::collections::BTreeSet::new());
        last_clicked.set(None);
        entering.set(std::collections::BTreeSet::new());
        removing.set(std::collections::BTreeSet::new());
        scroll_top.set(0.0);
        let kind = selected_kind.get()?;
        let ns = if kind.namespaced { selected_ns.get() } else { None };
        Some(data::watch_url(&kind.key, ns.as_deref(), None))
    });

    // Sorted/filtered uids. O(1) per-row lookup happens in each row's own memo below.
    let shown_uids = Memo::new(move |_| {
        let problems = only_problems.get();
        let filter_text = resource_filter.get().to_lowercase();
        let (key, asc) = sort.get();
        rows.with(|m| {
            let mut v: Vec<&ResourceRow> = m
                .values()
                .filter(|r| !problems || matches!(r.status, RowStatus::Error | RowStatus::Warn))
                .filter(|r| {
                    if filter_text.is_empty() {
                        true
                    } else {
                        // Simple substring match for now
                        r.name.to_lowercase().contains(&filter_text)
                    }
                })
                .collect();
            // Each key ends in a name + uid tiebreak so the order is fully stable
            // despite the map's unordered iteration.
            v.sort_by(|a, b| {
                let ord = match key {
                    SortKey::Namespace => a.namespace.cmp(&b.namespace).then_with(|| a.name.cmp(&b.name)),
                    SortKey::Name => a.name.cmp(&b.name),
                    SortKey::Age => a.created.cmp(&b.created).then_with(|| a.name.cmp(&b.name)),
                    SortKey::Cell(i) => cmp_cell(a.cells.get(i), b.cells.get(i)).then_with(|| a.name.cmp(&b.name)),
                }
                .then_with(|| a.uid.cmp(&b.uid));
                if asc { ord } else { ord.reverse() }
            });
            v.into_iter().map(|r| r.uid.clone()).collect::<Vec<String>>()
        })
    });

    // Per-kind default sort: Events newest-first by age, everything else by namespace.
    Effect::new(move |_| {
        let Some(kind) = selected_kind.get() else { return };
        let is_events = kind.group.is_empty() && kind.kind == "Event";
        sort.set(if is_events {
            (SortKey::Age, false)
        } else {
            (SortKey::Namespace, true)
        });
    });

    // Measure the real viewport + row height from the DOM. Done in a rAF so the read
    // happens *after* layout (a synchronous read can be 0/stale, which collapsed the
    // window before), guarded so a bad read never shrinks it. Re-measures as the list
    // (re)populates and on window resize.
    #[cfg(target_arch = "wasm32")]
    let measure = move || {
        request_animation_frame(move || {
            let Some(wrap) = table_ref.get_untracked() else { return };
            let ch = wrap.client_height() as f64;
            if ch > 50.0 && (ch - viewport_h.get_untracked()).abs() > 0.5 {
                viewport_h.set(ch);
            }
            if let Ok(Some(row)) = wrap.query_selector(".grid-row.row") {
                let h = row.get_bounding_client_rect().height();
                if h > 1.0 && (h - row_h.get_untracked()).abs() > 0.5 {
                    row_h.set(h);
                }
            }
        });
    };
    Effect::new(move |_| {
        shown_uids.with(|v| v.len());
        #[cfg(target_arch = "wasm32")]
        measure();
    });
    Effect::new(move |_| {
        let h = window_event_listener(leptos::ev::resize, move |_| {
            #[cfg(target_arch = "wasm32")]
            measure();
        });
        on_cleanup(move || h.remove());
    });

    // The slice of the sorted list to actually mount, sized to the viewport.
    let window = Memo::new(move |_| {
        let total = shown_uids.with(|v| v.len());
        let rh = row_h.get().max(1.0);
        let first = ((scroll_top.get() / rh).floor() as usize)
            .saturating_sub(OVERSCAN)
            .min(total);
        let count = (viewport_h.get() / rh).ceil() as usize + 2 * OVERSCAN;
        let last = (first + count).min(total);
        (first, last)
    });

    // Attach the scroll listener directly to the container. Scroll doesn't bubble, so
    // event delegation can miss it — a direct listener is reliable.
    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::closure::Closure;
            use wasm_bindgen::JsCast;
            let Some(wrap) = table_ref.get() else { return };
            let el = wrap.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                scroll_top.set(el.scroll_top() as f64);
                let ch = el.client_height() as f64;
                if ch > 50.0 {
                    viewport_h.set(ch);
                }
            });
            let _ = wrap.add_event_listener_with_callback("scroll", cb.as_ref().unchecked_ref());
            cb.forget(); // listener lives for the element's lifetime
        }
    });

    // Esc clears an active multi-select (only when one exists, so it doesn't
    // pre-empt the app-level Esc that closes the detail drawer).
    // ⌘C copies the names of selected resources to the clipboard.
    Effect::new(move |_| {
        let h = window_event_listener(leptos::ev::keydown, move |e| {
            if e.key() == "Escape" && !selected.with_untracked(|s| s.is_empty()) {
                selected.set(std::collections::BTreeSet::new());
            } else if (e.meta_key() || e.ctrl_key()) && e.key().eq_ignore_ascii_case("c") {
                // Copy selected resource names to clipboard
                let uids = selected.with_untracked(|s| s.clone());
                if !uids.is_empty() {
                    let names: Vec<String> = rows.with_untracked(|m| {
                        uids.iter()
                            .filter_map(|uid| m.get(uid).map(|r| r.name.clone()))
                            .collect()
                    });
                    if !names.is_empty() {
                        crate::app::util::clipboard::copy_to_clipboard(&names.join("\n"));
                    }
                } else if let Some(target) = detail.with_untracked(|d| d.clone()) {
                    // If nothing selected but detail is open, copy that resource name
                    crate::app::util::clipboard::copy_to_clipboard(&target.name);
                }
            }
        });
        on_cleanup(move || h.remove());
    });

    view! {
        <div class="resource-view">
            {move || match selected_kind.get() {
                None => view! { <Dashboard /> }.into_any(),
                Some(kind) => {
                    let cols = kind.columns.clone();
                    let namespaced = kind.namespaced;
                    let key = kind.key.clone();
                    let title = kind.kind.clone();
                    let is_pod_kind = kind.group.is_empty() && kind.kind == "Pod";
                    let node_col = cols.iter().position(|c| c == "Node");
                    // Columns whose text we tint by row status (e.g. Pod "Ready" + "Phase").
                    let colored_cols = StoredValue::new(
                        cols.iter()
                            .enumerate()
                            .filter(|(_, c)| matches!(c.as_str(), "Phase" | "Status" | "Ready"))
                            .map(|(i, _)| i)
                            .collect::<Vec<usize>>(),
                    );
                    // Metrics refresh every ~15s; don't flash those cells or the table
                    // would constantly blink.
                    let metric_cols = StoredValue::new(
                        cols.iter()
                            .enumerate()
                            .filter(|(_, c)| c.starts_with("CPU") || c.starts_with("MEM") || c.starts_with("%"))
                            .map(|(i, _)| i)
                            .collect::<Vec<usize>>(),
                    );
                    let bulk_workload = kind.group == "apps"
                        && matches!(kind.kind.as_str(), "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet");
                    let key_sv = StoredValue::new(kind.key.clone());
                    // Run an action on every selected row, then clear the selection.
                    let do_bulk = move |action: &'static str| {
                        let key = key_sv.get_value();
                        let uids = selected.get_untracked();
                        rows.with_untracked(|v| {
                            for r in v.values().filter(|r| uids.contains(&r.uid)) {
                                let t = DetailTarget { key: key.clone(), namespace: r.namespace.clone(), name: r.name.clone() };
                                fire_action(action, &t);
                            }
                        });
                        selected.set(std::collections::BTreeSet::new());
                    };

                    // Fixed column widths, computed once from the data's longest value per
                    // column (virtualization mounts only a slice, so `max-content` would
                    // jump as you scroll). The trailing 1fr absorbs slack; `.cwi` truncates.
                    let ncols = cols.len();
                    let cols_for_w = cols.clone();
                    let grid_template = RwSignal::new(String::new());
                    Effect::new(move |_| {
                        let n = shown_uids.with(|v| v.len());
                        if n == 0 {
                            return;
                        }
                        // Seed each slot with its header label width.
                        let mut ns_w = if namespaced { "Namespace".len() } else { 0 };
                        let mut name_w = "Name".len();
                        let mut cell_w: Vec<usize> = cols_for_w.iter().map(|c| c.len().max(min_width(c))).collect();
                        let age_w = "Age".len().max(6);
                        rows.with(|m| {
                            for r in m.values() {
                                if namespaced {
                                    ns_w = ns_w.max(disp_len(r.namespace.as_deref().unwrap_or("")));
                                }
                                name_w = name_w.max(r.name.chars().count());
                                for i in 0..ncols {
                                    if let Some(c) = r.cells.get(i) {
                                        cell_w[i] = cell_w[i].max(disp_len(c)).max(cols_for_w.get(i).map_or(0, |h| min_width(h)));
                                    }
                                }
                            }
                        });
                        let mut tracks: Vec<String> = Vec::new();
                        if namespaced {
                            tracks.push(format!("{}ch", col_width(ns_w)));
                        }
                        // Name gets a little extra for the (revealed) checkbox.
                        tracks.push(format!("{}ch", col_width(name_w + 2)));
                        for w in &cell_w {
                            tracks.push(format!("{}ch", col_width(*w)));
                        }
                        tracks.push(format!("{}ch", col_width(age_w)));
                        grid_template.set(format!(
                            "grid-template-columns: {} minmax(0,1fr);",
                            tracks.join(" ")
                        ));
                    });

                    let header = {
                        let cols = cols.clone();
                        view! {
                            <div class="grid-row head">
                                {namespaced.then(|| sortable_th("Namespace".to_string(), SortKey::Namespace, sort))}
                                {sortable_th("Name".to_string(), SortKey::Name, sort)}
                                {cols.iter().enumerate().map(|(i, c)| sortable_th(c.clone(), SortKey::Cell(i), sort)).collect_view()}
                                {sortable_th("Age".to_string(), SortKey::Age, sort)}
                            </div>
                        }
                    };
                    view! {
                        <div class="view-head">
                            <h2 class="view-title">{title}</h2>
                            <span class="count">{move || format!("{} items", shown_uids.get().len())}</span>
                        </div>
                        <div class="bulkbar-wrap" class:open=move || !selected.get().is_empty()>
                            <div class="bulkbar">
                                <span class="bulk-count">{move || format!("{} selected", selected.get().len())}</span>
                                <button class="act" on:click=move |_| selected.set(shown_uids.get().into_iter().collect())>"Select all"</button>
                                <button class="act" on:click=move |_| selected.set(std::collections::BTreeSet::new())>"Clear"</button>
                                {(is_pod_kind || bulk_workload).then(|| view! { <button class="act" on:click=move |_| {
                                    let uids = selected.get_untracked();
                                    let key = key_sv.get_value();
                                    let agg = !is_pod_kind; // workloads aggregate their pods into one panel
                                    rows.with_untracked(|v| {
                                        for r in v.values().filter(|r| uids.contains(&r.uid)) {
                                            open_logs(log_pods, LogTarget { key: key.clone(), namespace: r.namespace.clone().unwrap_or_default(), name: r.name.clone(), aggregate: agg });
                                        }
                                    });
                                    selected.set(std::collections::BTreeSet::new());
                                }>"Logs"</button> })}
                                {bulk_workload.then(|| view! { <button class="act" on:click=move |_| do_bulk("restart")>"Restart"</button> })}
                                <button class="act danger" on:click=move |_| {
                                    let n = selected.get_untracked().len();
                                    ask_confirm(confirm, format!("Delete {n} resources?"), move || do_bulk("delete"));
                                }>"Delete"</button>
                            </div>
                        </div>
                        <div class="table-wrap" node_ref=table_ref>
                            <div class="grid-table"
                                style=move || grid_template.get()
                                class:selecting=move || !selected.get().is_empty()>
                                {header}
                                // Top spacer: reserves the height of the rows above the window.
                                <div class="vpad" style=move || {
                                    format!("grid-column:1/-1;height:{}px", window.get().0 as f64 * row_h.get())
                                }></div>
                                    <For each=move || {
                                        let (first, last) = window.get();
                                        shown_uids.with(|v| v.get(first..last).map(<[String]>::to_vec).unwrap_or_default())
                                    } key=|uid| uid.clone() let:uid>
                                        {
                                            let key = key.clone();
                                            let ncols = cols.len();
                                            let uid_row = uid.clone();
                                            let uid_chk = uid.clone();
                                            let uid_clk = uid.clone();
                                            let uid_en = uid.clone();
                                            let uid_rm = uid.clone();
                                            let uid_te = uid.clone();
                                            let uid_pd = uid.clone();
                                            let uid_ctrl = uid;
                                            // This row's live data, reactive to the shared rows signal.
                                            let row = Memo::new(move |_| rows.with(|m| m.get(&uid_row).cloned()));
                                            // Stable identity (name/namespace don't change for a uid).
                                            let init = row.get_untracked();
                                            let target = DetailTarget {
                                                key,
                                                namespace: init.as_ref().and_then(|r| r.namespace.clone()),
                                                name: init.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
                                            };
                                            let t_click = target.clone();
                                            let t_cls = target.clone();
                                            let t_ctx = target;
                                            let is_active = move || detail.get().as_ref() == Some(&t_cls);

                                            view! {
                                                <div class="grid-row row"
                                                    class:active=is_active
                                                    class:selected=move || selected.get().contains(&uid_chk)
                                                    class:entering=move || entering.get().contains(&uid_en)
                                                    class:removing=move || removing.get().contains(&uid_rm)
                                                    on:click=move |e: leptos::ev::MouseEvent| {
                                                        // A long-press already toggled selection — swallow its trailing click.
                                                        if press_fired.get_value() { press_fired.set_value(false); return; }
                                                        if e.shift_key() {
                                                            range_select(selected, last_clicked, shown_uids, &uid_ctrl);
                                                        } else if e.ctrl_key() || e.meta_key() {
                                                            let u = uid_ctrl.clone();
                                                            selected.update(|s| { if !s.remove(&u) { s.insert(u.clone()); } });
                                                            last_clicked.set(Some(u));
                                                        } else {
                                                            let t = t_click.clone();
                                                            detail.update(|d| if d.as_ref() == Some(&t) { *d = None } else { *d = Some(t.clone()) });
                                                        }
                                                    }
                                                    on:pointerdown=move |e: leptos::ev::PointerEvent| {
                                                        press_fired.set_value(false);
                                                        if e.button() != 0 || e.ctrl_key() || e.shift_key() || e.meta_key() { return; }
                                                        press_xy.set_value((e.client_x(), e.client_y()));
                                                        let u = uid_pd.clone();
                                                        let h = set_timeout_with_handle(move || {
                                                            selected.update(|s| { s.insert(u.clone()); });
                                                            last_clicked.set(Some(u.clone()));
                                                            press_fired.set_value(true);
                                                            press_handle.set_value(None);
                                                        }, std::time::Duration::from_millis(450)).ok();
                                                        press_handle.set_value(h);
                                                    }
                                                    on:pointermove=move |e: leptos::ev::PointerEvent| {
                                                        if press_handle.with_value(|h| h.is_some()) {
                                                            let (sx, sy) = press_xy.get_value();
                                                            if (e.client_x() - sx).abs() > 10 || (e.client_y() - sy).abs() > 10 {
                                                                cancel_press();
                                                            }
                                                        }
                                                    }
                                                    on:pointerup=move |_| cancel_press()
                                                    on:pointercancel=move |_| cancel_press()
                                                    on:mousedown=move |e: leptos::ev::MouseEvent| {
                                                        // Shift-click extends the browser's text selection by default —
                                                        // suppress it so range-select doesn't highlight cell text.
                                                        if e.shift_key() { e.prevent_default(); }
                                                    }
                                                    on:contextmenu=move |e: leptos::ev::MouseEvent| {
                                                        e.prevent_default();
                                                        let node = if is_pod_kind { node_col.and_then(|i| row.get_untracked().and_then(|r| r.cells.get(i).cloned())) } else { None };
                                                        ctx_menu.set(Some(CtxMenu { x: e.client_x(), y: e.client_y(), target: t_ctx.clone(), node }));
                                                    }
                                                    on:transitionend=move |e: leptos::ev::TransitionEvent| {
                                                        // The collapse has visually finished — now unmount the row.
                                                        if e.property_name() == "grid-template-rows"
                                                            && removing.get_untracked().contains(&uid_te)
                                                        {
                                                            let u = uid_te.clone();
                                                            rows.update(|m| { m.remove(&u); });
                                                            removing.update(|s| { s.remove(&uid_te); });
                                                        }
                                                    }>
                                                    {namespaced.then(|| view!{ <FlashTd value=move || row.get().and_then(|r| r.namespace).unwrap_or_default() class="cell-ns" /> })}
                                                    <div class="cell cell-name">
                                                        <div class="cw"><div class="cwi">
                                                            <span class="check" on:click=move |e: leptos::ev::MouseEvent| {
                                                                e.stop_propagation();
                                                                if e.shift_key() {
                                                                    range_select(selected, last_clicked, shown_uids, &uid_clk);
                                                                } else {
                                                                    let u = uid_clk.clone();
                                                                    selected.update(|s| { if !s.remove(&u) { s.insert(u.clone()); } });
                                                                    last_clicked.set(Some(u));
                                                                }
                                                            }></span>
                                                            <span class="nm" style=move || name_color(row.get().map(|r| r.status).unwrap_or(RowStatus::Unknown))>
                                                                {move || row.get().map(|r| r.name).unwrap_or_default()}
                                                            </span>
                                                        </div></div>
                                                    </div>
                                                    {(0..ncols).map(|i| {
                                                        let val = move || row.get().and_then(|r| r.cells.get(i).cloned()).unwrap_or_default();
                                                        let trend_sig = Signal::derive(move || row.get().and_then(|r| r.trends.get(i).copied()).unwrap_or(Trend::None));
                                                        if colored_cols.with_value(|v| v.contains(&i)) {
                                                            view! { <FlashTd value=val color=Signal::derive(move || dot_class(row.get().map(|r| r.status).unwrap_or(RowStatus::Unknown))) /> }.into_any()
                                                        } else {
                                                            let no_flash = metric_cols.with_value(|v| v.contains(&i));
                                                            view! { <FlashTd value=val no_flash=no_flash trend=trend_sig /> }.into_any()
                                                        }
                                                    }).collect_view()}
                                                    <div class="cell cell-age"><div class="cw"><div class="cwi">{move || { tick.get(); data::humanize_age(&row.get().and_then(|r| r.created)) }}</div></div></div>
                                                </div>
                                            }
                                        }
                                    </For>
                                // Bottom spacer: reserves the height of the rows below the window.
                                <div class="vpad" style=move || {
                                    let (_, last) = window.get();
                                    let total = shown_uids.get().len();
                                    format!("grid-column:1/-1;height:{}px", total.saturating_sub(last) as f64 * row_h.get())
                                }></div>
                            </div>
                            {move || {
                                let is_empty = shown_uids.get().is_empty();
                                if !is_empty {
                                    return None;
                                }
                                let problems = only_problems.get();
                                let ns = selected_ns.get();
                                let kind_name = kind.kind.clone();
                                
                                let msg = if problems {
                                    format!("No {} with problems", kind_name)
                                } else if let Some(ns) = ns {
                                    format!("No {} in namespace \"{}\"", kind_name, ns)
                                } else {
                                    format!("No {} found", kind_name)
                                };
                                
                                Some(view! { <div class="empty pad">{msg}</div> })
                            }}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}
