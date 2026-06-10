//! The main live resource table: a windowed (virtualized) list with filtering,
//! sorting, multi-select, bulk actions, and live enter/exit animations. Only the
//! rows in (or near) the viewport are mounted, so list size doesn't affect cost.

use leptos::prelude::*;
use roder_core::{ResourceKind, ResourceRow, RowStatus};

use crate::app::components::table::{cmp_cell, sortable_th, FlashTd};
use crate::app::components::table_row::{NameCell, ResourceRow as ResourceRowView};
use crate::app::util::predicate::KindKind;
use crate::app::events::fire_action;
use crate::app::hooks::{
    col_width, disp_len, min_width, table_window, use_resource_table, use_sse_subscription,
};
use crate::app::overlays::confirm::{ask_confirm, Confirm};
use crate::app::state::{
    open_logs, CtxMenu, DetailTarget, LogPods, LogTarget, OnlyProblems, ResourceFilter, SortKey,
    TableRows, TableSelected, Tick,
};
use crate::app::util::color::dot_class;
use crate::app::views::dashboard::Dashboard;
use crate::data;

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

    let t = use_resource_table(detail);
    // Write into the app-level StoredValue slots so ContextMenu (a sibling,
    // not a child) can see the live selection and row map.
    let sv_sel = expect_context::<TableSelected>().0;
    let sv_rows = expect_context::<TableRows>().0;
    sv_sel.set_value(Some(t.selected));
    sv_rows.set_value(Some(t.rows));
    on_cleanup(move || {
        sv_sel.set_value(None);
        sv_rows.set_value(None);
    });

    // (Re)subscribe to the live list whenever the selected kind/namespace changes.
    use_sse_subscription(t.rows, t.entering, t.removing, move || {
        t.rows.set(Default::default());
        t.selected.set(Default::default());
        t.last_clicked.set(None);
        t.entering.set(Default::default());
        t.removing.set(Default::default());
        t.scroll_top.set(0.0);
        let kind = selected_kind.get()?;
        let ns = if kind.namespaced {
            selected_ns.get()
        } else {
            None
        };
        Some(data::watch_url(&kind.key, ns.as_deref(), None))
    });

    // Sorted/filtered uids. O(1) per-row lookup happens in each row's own memo below.
    let shown_uids = Memo::new(move |_| {
        let problems = only_problems.get();
        let filter_text = resource_filter.get().to_lowercase();
        let (key, asc) = t.sort.get();
        t.rows.with(|m| {
            let mut v: Vec<&ResourceRow> = m
                .values()
                .filter(|r| !problems || matches!(r.status, RowStatus::Error | RowStatus::Warn))
                .filter(|r| {
                    if filter_text.is_empty() {
                        true
                    } else {
                        r.name.to_lowercase().contains(&filter_text)
                    }
                })
                .collect();
            v.sort_by(|a, b| {
                let ord = match key {
                    SortKey::Namespace => a
                        .namespace
                        .cmp(&b.namespace)
                        .then_with(|| a.name.cmp(&b.name)),
                    SortKey::Name => a.name.cmp(&b.name),
                    SortKey::Age => a.created.cmp(&b.created).then_with(|| a.name.cmp(&b.name)),
                    SortKey::Cell(i) => {
                        cmp_cell(a.cells.get(i), b.cells.get(i)).then_with(|| a.name.cmp(&b.name))
                    }
                }
                .then_with(|| a.uid.cmp(&b.uid));
                if asc {
                    ord
                } else {
                    ord.reverse()
                }
            });
            v.into_iter()
                .map(|r| r.uid.clone())
                .collect::<Vec<String>>()
        })
    });

    // Per-kind default sort: Events newest-first by age, everything else by namespace.
    Effect::new(move |_| {
        let Some(kind) = selected_kind.get() else {
            return;
        };
        let is_events = kind.group.is_empty() && kind.kind == "Event";
        t.sort.set(if is_events {
            (SortKey::Age, false)
        } else {
            (SortKey::Namespace, true)
        });
    });

    let window = table_window(t, shown_uids);

    let table_ref = t.table_ref;
    let rows = t.rows;
    let selected = t.selected;
    let last_clicked = t.last_clicked;
    let sort = t.sort;
    let entering = t.entering;
    let removing = t.removing;
    let row_h = t.row_h;
    let press = t.press;

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
                    // Boolean columns (true/false) — colour by literal value
                    // rather than by row status, so PVC Mount reads green for
                    // "true" and amber for "false".
                    let bool_cols = StoredValue::new(
                        cols.iter()
                            .enumerate()
                            .filter(|(_, c)| matches!(c.as_str(), "Mount"))
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
                    let kk = KindKind::new(&kind.group, &kind.kind);
                    let bulk_workload = kk.is_workload();
                    let bulk_flux = kk.is_flux();
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
                    let cols_for_w = cols.clone();
                    let grid_template = RwSignal::new(String::new());
                    Effect::new(move |_| {
                        let n = shown_uids.with(|v| v.len());
                        if n == 0 {
                            return;
                        }
                        let mut ns_w = if namespaced { "Namespace".len() } else { 0 };
                        let mut name_w = "Name".len();
                        let mut cell_w: Vec<usize> = cols_for_w.iter().map(|c| c.len().max(min_width(c))).collect();
                        let age_w = "Age".len().max(6);
                        rows.with_untracked(|m| {
                            for r in m.values() {
                                if namespaced {
                                    ns_w = ns_w.max(disp_len(r.namespace.as_deref().unwrap_or("")));
                                }
                                name_w = name_w.max(r.name.chars().count());
                                for (i, c) in r.cells.iter().enumerate() {
                                    cell_w[i] = cell_w[i].max(disp_len(c)).max(min_width(cols_for_w.get(i).map_or("", String::as_str)));
                                }
                            }
                        });
                        let mut tracks: Vec<String> = Vec::new();
                        if namespaced {
                            tracks.push(format!("{}ch", col_width(ns_w)));
                        }
                        tracks.push(format!("{}ch", col_width(name_w + 2)));
                        for w in &cell_w {
                            tracks.push(format!("{}ch", col_width(*w)));
                        }
                        tracks.push(format!("{}ch", col_width(age_w)));
                        let new_tmpl = format!(
                            "grid-template-columns: {} minmax(0,1fr);",
                            tracks.join(" ")
                        );
                        if grid_template.get_untracked() != new_tmpl {
                            grid_template.set(new_tmpl);
                        }
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
                                    let agg = !is_pod_kind;
                                    rows.with_untracked(|v| {
                                        for r in v.values().filter(|r| uids.contains(&r.uid)) {
                                            open_logs(log_pods, LogTarget { key: key.clone(), namespace: r.namespace.clone().unwrap_or_default(), name: r.name.clone(), aggregate: agg });
                                        }
                                    });
                                    selected.set(std::collections::BTreeSet::new());
                                }>"Logs"</button> })}
                                {bulk_workload.then(|| view! { <button class="act" on:click=move |_| do_bulk("restart")>"Restart"</button> })}
                                {bulk_flux.then(|| view! {
                                    <button class="act" on:click=move |_| do_bulk("flux-reconcile")>"Reconcile"</button>
                                    <button class="act" on:click=move |_| do_bulk("flux-suspend")>"Suspend"</button>
                                    <button class="act" on:click=move |_| do_bulk("flux-resume")>"Resume"</button>
                                })}
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
                                        let row = Memo::new(move |_| rows.with(|m| m.get(&uid_row).cloned()));

                                        // One Effect per row diffs all cells at once and writes a
                                        // bitmask: bit 0 = namespace, bits 1..=ncols = cells[i].
                                        // Each FlashTd reads its own bit — no per-cell Effects.
                                        let flash_bits = RwSignal::new(0u64);
                                        Effect::new(move |prev: Option<(Option<String>, Vec<String>)>| {
                                            let r = row.get();
                                            let ns = r.as_ref().and_then(|r| r.namespace.clone());
                                            let cells = r.map(|r| r.cells.clone()).unwrap_or_default();
                                            if let Some((prev_ns, prev_cells)) = prev {
                                                let mut bits = 0u64;
                                                if ns != prev_ns { bits |= 1; }
                                                for (i, (cur, old)) in cells.iter().zip(prev_cells.iter()).enumerate() {
                                                    if cur != old { bits |= 1u64 << (i + 1); }
                                                }
                                                if bits != 0 {
                                                    flash_bits.update(|b| *b |= bits);
                                                    set_timeout(
                                                        move || flash_bits.update(|b| *b &= !bits),
                                                        std::time::Duration::from_millis(1500),
                                                    );
                                                }
                                            }
                                            (ns, cells)
                                        });

                                        let init = row.get_untracked();
                                        let target = DetailTarget {
                                            key,
                                            namespace: init.as_ref().and_then(|r| r.namespace.clone()),
                                            name: init.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
                                        };
                                        let node_for_ctx = {
                                            let r = row;
                                            move || {
                                                if is_pod_kind {
                                                    node_col.and_then(|i| r.get().and_then(|rr| rr.cells.get(i).cloned()))
                                                } else {
                                                    None
                                                }
                                            }
                                        };
                                        let on_unmount = {
                                            let uid = uid.clone();
                                            let rows = rows;
                                            Callback::new(move |u: String| {
                                                debug_assert_eq!(u, uid, "transitionend uid mismatch");
                                                rows.update(|m| { m.remove(&uid); });
                                            })
                                        };

                                        view! {
                                            <ResourceRowView
                                                uid=uid.clone()
                                                target=target
                                                detail=detail
                                                ctx_menu=ctx_menu
                                                selected=selected
                                                last_clicked=last_clicked
                                                entering=entering
                                                removing=removing
                                                on_unmount=on_unmount
                                                shown_uids=shown_uids
                                                press=press
                                                node_for_ctx=node_for_ctx>
                                                {namespaced.then(|| view! {
                                                    <FlashTd
                                                        value=move || row.get().and_then(|r| r.namespace).unwrap_or_default()
                                                        class="cell-ns"
                                                        flash=Signal::derive(move || flash_bits.get() & 1 != 0) />
                                                })}
                                                <NameCell
                                                    uid=uid.clone()
                                                    name=move || row.get().map(|r| r.name)
                                                    status=move || row.get().map(|r| r.status)
                                                    selected=selected
                                                    last_clicked=last_clicked
                                                    shown_uids=shown_uids />
                                                {(0..ncols).map(|i| {
                                                    let val = move || row.get().and_then(|r| r.cells.get(i).cloned()).unwrap_or_default();
                                                    let trend_sig = Signal::derive(move || row.get().and_then(|r| r.trends.get(i).copied()).unwrap_or(roder_core::Trend::None));
                                                    let flash = Signal::derive(move || flash_bits.get() & (1u64 << (i + 1)) != 0);
                                                    if bool_cols.with_value(|v| v.contains(&i)) {
                                                        view! { <FlashTd value=val no_flash=true
                                                            color=Signal::derive(move || match val().as_str() {
                                                                "true" => "ok",
                                                                "false" => "warn",
                                                                _ => "unknown",
                                                            }) /> }.into_any()
                                                    } else if metric_cols.with_value(|v| v.contains(&i)) {
                                                        view! { <FlashTd value=val no_flash=true trend=trend_sig /> }.into_any()
                                                    } else if colored_cols.with_value(|v| v.contains(&i)) {
                                                        view! { <FlashTd value=val flash=flash color=Signal::derive(move || dot_class(row.get().map(|r| r.status).unwrap_or(RowStatus::Unknown))) /> }.into_any()
                                                    } else {
                                                        view! { <FlashTd value=val flash=flash trend=trend_sig /> }.into_any()
                                                    }
                                                }).collect_view()}
                                                <div class="cell cell-age"><div class="cw"><div class="cwi">{move || { tick.get(); data::humanize_age(&row.get().and_then(|r| r.created)) }}</div></div></div>
                                            </ResourceRowView>
                                        }
                                    }
                                </For>
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
