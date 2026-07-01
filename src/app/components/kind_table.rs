use leptos::prelude::*;
use roder_core::{ResourceKind, RowStatus, Trend};

use crate::app::components::table::{sortable_th, FlashTd};
use crate::app::components::table_row::{NameCell, ResourceRow as ResourceRowView};
use crate::app::events::fire_action;
use crate::app::events::RowMap;
use crate::app::hooks::{table_window, use_sse_subscription, use_table_state};
use crate::app::overlays::confirm::{ask_confirm, Confirm};
use crate::app::overlays::toast::{show_toast, Toast, ToastKind};
use crate::app::state::{
    open_logs, CtxMenu, DetailTarget, FilterFocus, LogPods, LogTarget, OnlyProblems,
    ResourceFilter, SortKey, TableRows, TableSelected, Tick,
};
use crate::app::table_logic;
use crate::app::util::color::{dot_class, pct_thresh_color};
use crate::app::util::format::parse_key;
use crate::app::util::predicate::KindKind;
use crate::data;

/// Full-featured live resource table. Handles its own SSE subscription, virtual
/// scrolling, column flash, sorting, multi-select, and bulk actions.
///
/// `url_fn` is tracked reactively — any signals it reads trigger a clean
/// re-subscription and state reset. Return `None` to stay disconnected.
///
/// Set `keyboard = false` for workspace panes to avoid competing global listeners.
/// Set `register_global_selection = true` for the primary view so the context-menu
/// sibling component can see the live selection.
#[component]
pub(crate) fn KindTable(
    kind: ResourceKind,
    url_fn: impl Fn() -> Option<String> + 'static,
    #[prop(optional)] on_close: Option<Callback<()>>,
    namespace: Option<String>,
    selector: Option<String>,
    #[prop(optional)] text_filter: Option<RwSignal<String>>,
    /// Per-pane namespace signal. When present the view-head shows a live `<select>`
    /// instead of the static badge, and the signal is updated on change.
    #[prop(optional)]
    ns_filter: Option<RwSignal<Option<String>>>,
    #[prop(default = true)] keyboard: bool,
    #[prop(default = false)] register_global_selection: bool,
    /// When provided, rows are driven from this external signal instead of
    /// opening a per-table SSE connection (used by the workspace multi-watch).
    #[prop(optional)]
    rows_override: Option<RowMap>,
) -> impl IntoView {
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let ctx_menu = expect_context::<RwSignal<Option<CtxMenu>>>();
    let tick = expect_context::<Tick>().0;
    let only_problems = expect_context::<OnlyProblems>().0;
    let resource_filter = expect_context::<ResourceFilter>().0;
    let log_pods = expect_context::<LogPods>().0;
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let selected_ns_ctx = use_context::<RwSignal<Option<String>>>();

    let t = use_table_state();

    // Expose row-map and selection to ContextMenu (a sibling) when this is the primary table.
    if register_global_selection {
        let sv_sel = expect_context::<TableSelected>().0;
        let sv_rows = expect_context::<TableRows>().0;
        sv_sel.set_value(Some(t.selected));
        sv_rows.set_value(Some(t.rows));
        on_cleanup(move || {
            sv_sel.set_value(None);
            sv_rows.set_value(None);
        });
    }

    // Default sort: Events newest-first, everything else alphabetical by namespace.
    let is_events = kind.group.is_empty() && kind.kind == "Event";
    t.sort.set(if is_events {
        (SortKey::Age, false)
    } else {
        (SortKey::Namespace, true)
    });

    // Global keyboard shortcuts — primary table only; workspace panes skip this.
    if keyboard {
        let kind_sv = StoredValue::new(kind.clone());
        let rows_kb = t.rows;
        let sel_kb = t.selected;
        let detail_kb = detail;
        let log_pods_kb = log_pods;
        let toast_kb = toast;
        Effect::new(move |_| {
            let h = window_event_listener(leptos::ev::keydown, move |e| {
                let key = e.key();
                if key == "Escape" && !sel_kb.with_untracked(|s| s.is_empty()) {
                    sel_kb.set(std::collections::BTreeSet::new());
                } else if (e.meta_key() || e.ctrl_key()) && key.eq_ignore_ascii_case("c") {
                    let uids = sel_kb.with_untracked(|s| s.clone());
                    if !uids.is_empty() {
                        let names: Vec<String> = rows_kb.with_untracked(|m| {
                            uids.iter()
                                .filter_map(|uid| m.get(uid).map(|r| r.name.clone()))
                                .collect()
                        });
                        if !names.is_empty() {
                            crate::app::util::clipboard::copy_to_clipboard(&names.join("\n"));
                            show_toast(toast_kb, "Copied to clipboard", ToastKind::Ok);
                        }
                    } else if let Some(t) = detail_kb.with_untracked(|d| d.clone()) {
                        crate::app::util::clipboard::copy_to_clipboard(&t.name);
                        show_toast(toast_kb, "Copied to clipboard", ToastKind::Ok);
                    }
                } else if key == "Enter" && !data::is_text_input_focused() {
                    let uids = sel_kb.with_untracked(|s| s.clone());
                    if uids.len() == 1 {
                        let uid = uids.into_iter().next().unwrap();
                        let k = kind_sv.get_value();
                        if let Some(row) = rows_kb.with_untracked(|m| m.get(&uid).cloned()) {
                            detail_kb.set(Some(DetailTarget {
                                key: k.key,
                                namespace: row.namespace,
                                name: row.name,
                            }));
                        }
                    }
                } else if key.eq_ignore_ascii_case("l") && !data::is_text_input_focused() {
                    let k = kind_sv.get_value();
                    let (group, knd) = parse_key(&k.key);
                    let kk = KindKind::new(&group, &knd);
                    if kk.is_pod() || kk.is_workload() || kk.is_job() {
                        let agg = !kk.is_pod();
                        let uids = sel_kb.with_untracked(|s| s.clone());
                        rows_kb.with_untracked(|m| {
                            for uid in &uids {
                                if let Some(row) = m.get(uid) {
                                    open_logs(log_pods_kb, LogTarget::from_row(&k.key, row, agg));
                                }
                            }
                        });
                    }
                }
            });
            on_cleanup(move || h.remove());
        });
    }

    // Namespace list for the per-pane selector — shared context resource (fetched once at
    // App level) so individual panes don't each open a separate HTTP connection.
    let ns_list =
        ns_filter.and_then(|_| use_context::<LocalResource<Result<Vec<String>, String>>>());

    // Live column headers: seeded from the catalog, then updated by each snapshot
    // so the table reflows in place when a CRD's printer columns change. (In
    // `rows_override`/workspace mode there's no own subscription, so it stays at
    // the catalog value.)
    let columns: RwSignal<Vec<String>> = RwSignal::new(kind.columns.clone());

    if let Some(ext) = rows_override {
        Effect::new(move |_| t.rows.set(ext.get()));
    } else {
        use_sse_subscription(t.rows, t.entering, t.removing, Some(columns), move || {
            t.rows.set(Default::default());
            t.selected.set(Default::default());
            t.last_clicked.set(None);
            t.entering.set(Default::default());
            t.removing.set(Default::default());
            t.scroll_top.set(0.0);
            url_fn()
        });
    }

    // Filtered + sorted UIDs. Uses per-pane text_filter if provided, global otherwise.
    let shown_uids = Memo::new(move |_| {
        let problems = only_problems.get();
        let filter_text = if let Some(tf) = text_filter {
            tf.get()
        } else {
            resource_filter.get()
        }
        .to_lowercase();
        let (sort_key, asc) = t.sort.get();
        t.rows
            .with(|m| table_logic::shown_uids(m.values(), sort_key, asc, problems, &filter_text))
    });

    let window = table_window(t, shown_uids);

    // Column metadata derived *reactively* from the live `columns` signal, so the
    // table reflows in place when a CRD's printer columns change.
    let namespaced = kind.namespaced;
    let key = kind.key.clone();
    let title = kind.kind.clone();
    let is_pod_kind = kind.group.is_empty() && kind.kind == "Pod";
    let node_col = Memo::new(move |_| columns.get().iter().position(|c| c == "Node"));
    let colored_cols = Memo::new(move |_| {
        columns
            .get()
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.as_str(), "Phase" | "Status" | "Ready"))
            .map(|(i, _)| i)
            .collect::<Vec<usize>>()
    });
    let bool_cols = Memo::new(move |_| {
        columns
            .get()
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.as_str(), "Mount"))
            .map(|(i, _)| i)
            .collect::<Vec<usize>>()
    });
    let metric_cols = Memo::new(move |_| {
        columns
            .get()
            .iter()
            .enumerate()
            .filter(|(_, c)| c.starts_with("CPU") || c.starts_with("MEM") || c.starts_with("%"))
            .map(|(i, _)| i)
            .collect::<Vec<usize>>()
    });
    // Saturation-style columns whose value is a percentage of the pod's
    // request/limit: red at 90+, yellow at 70+ (see `pct_thresh_color`).
    let pct_thresh_cols = Memo::new(move |_| {
        columns
            .get()
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.as_str(), "%CPU/R" | "%CPU/L" | "%MEM/R" | "%MEM/L"))
            .map(|(i, _)| i)
            .collect::<std::collections::HashSet<usize>>()
    });
    let kk = KindKind::new(&kind.group, &kind.kind);
    let bulk_workload = kk.is_workload();
    let bulk_flux = kk.is_flux();
    let bulk_helmrelease = kk.is_helmrelease();
    let bulk_has_source_ref = kk.has_source_ref();
    let key_sv = StoredValue::new(kind.key.clone());

    let rows = t.rows;
    let selected = t.selected;
    // `RowStatus::Warn` is reserved for the suspended case in the Flux projector
    // (see `ready_message_cells`), so it doubles as the signal for which of
    // Suspend/Resume to show. `None` (mixed selection) falls back to both.
    let bulk_suspend_state = move || -> Option<bool> {
        let uids = selected.get();
        rows.with(|rm| {
            let mut states = uids
                .iter()
                .filter_map(|u| rm.get(u))
                .map(|r| r.status == RowStatus::Warn);
            let first = states.next()?;
            states.all(|s| s == first).then_some(first)
        })
    };
    let bulk_show_suspend = move || bulk_suspend_state() != Some(true);
    let bulk_show_resume = move || bulk_suspend_state() != Some(false);
    let last_clicked = t.last_clicked;
    let sort = t.sort;
    let entering = t.entering;
    let removing = t.removing;
    let row_h = t.row_h;
    let press = t.press;
    let table_ref = t.table_ref;

    let do_bulk = move |action: &'static str| {
        let key = key_sv.get_value();
        let uids = selected.get_untracked();
        let targets = rows.with_untracked(|v| table_logic::bulk_targets(&key, v, &uids));
        fire_action(toast, action, &targets);
        selected.set(std::collections::BTreeSet::new());
    };

    // Grid track count follows the live column count, so the grid reflows when
    // columns change.
    let tmpl = move || {
        let n = columns.get().len() + 2 + usize::from(namespaced);
        format!(
            "grid-template-columns: {} 1fr;",
            vec!["max-content"; n].join(" ")
        )
    };
    let sizer: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    {
        Effect::new(move |_| {
            let _ = shown_uids.with(|v| v.len());
            // Also track raw row data so cell value changes (live metrics) trigger
            // a sizer update — shown_uids only fires when UIDs change, not values.
            rows.track();
            let cols_now = columns.get();
            let n_tracks = cols_now.len() + 2 + usize::from(namespaced);
            let ncells = cols_now.len();
            let mut ns_max = if namespaced {
                "Namespace".to_string()
            } else {
                String::new()
            };
            let mut name_max = "Name".to_string();
            let mut cell_maxes: Vec<String> = cols_now.clone();
            let mut col_has_trend = vec![false; ncells];
            rows.with_untracked(|m| {
                for r in m.values() {
                    if namespaced {
                        let ns = r.namespace.as_deref().unwrap_or("");
                        if ns.len() > ns_max.len() {
                            ns_max = ns.to_string();
                        }
                    }
                    if r.name.len() > name_max.len() {
                        name_max = r.name.clone();
                    }
                    for (i, c) in r.cells.iter().take(ncells).enumerate() {
                        if c.len() > cell_maxes[i].len() {
                            cell_maxes[i] = c.clone();
                        }
                        if matches!(r.trends.get(i), Some(Trend::Up | Trend::Down)) {
                            col_has_trend[i] = true;
                        }
                    }
                }
            });
            // Pre-widen sizer for columns that carry trend arrows so the grid
            // track doesn't shift when the arrow appears or disappears.
            let cell_maxes: Vec<String> = cell_maxes
                .into_iter()
                .enumerate()
                .map(|(i, s)| {
                    if col_has_trend[i] {
                        format!("{s} ↑")
                    } else {
                        s
                    }
                })
                .collect();
            let mut new_vals: Vec<String> = Vec::with_capacity(n_tracks);
            if namespaced {
                new_vals.push(ns_max);
            }
            new_vals.push(name_max);
            new_vals.extend(cell_maxes);
            new_vals.push("000d00h".to_string());
            // Sticky: columns only ever widen within a kind; reset when the column
            // set changes (kind switch). This prevents metric values bouncing
            // between wide ("100%") and narrow ("5%") from jittering the layout.
            sizer.update(|old| {
                if old.len() != n_tracks {
                    *old = new_vals;
                } else {
                    for (o, n) in old.iter_mut().zip(new_vals.iter()) {
                        if n.len() > o.len() {
                            *o = n.clone();
                        }
                    }
                }
            });
        });
    }

    // Reactive header: the per-column `<th>`s follow the live `columns` signal.
    let header = move || {
        view! {
            <div class="grid-row head">
                {namespaced.then(|| sortable_th("Namespace".to_string(), SortKey::Namespace, sort))}
                {sortable_th("Name".to_string(), SortKey::Name, sort)}
                {columns.get().into_iter().enumerate().map(|(i, c)| sortable_th(c, SortKey::Cell(i), sort)).collect_view()}
                {sortable_th("Age".to_string(), SortKey::Age, sort)}
            </div>
        }
    };

    let title_sv = StoredValue::new(title.clone());
    let ns_sv = StoredValue::new(namespace);
    let sel_sv = StoredValue::new(selector);

    view! {
        <div class="resource-view">
            <div class="view-head">
                <h2 class="view-title">{title.clone()}</h2>
                {if let Some(nf) = ns_filter.filter(|_| namespaced) {
                    let nl = ns_list.unwrap();
                    view! {
                        <select class="pane-ns-select"
                            on:change=move |e| {
                                let v = event_target_value(&e);
                                nf.set(if v.is_empty() { None } else { Some(v) });
                            }
                        >
                            <option value="" prop:selected=move || nf.get().is_none()>"All"</option>
                            <Suspense>
                                {move || nl.get().map(|res| {
                                    if let Ok(list) = res {
                                        list.into_iter().map(|ns| {
                                            let ns2 = ns.clone();
                                            view! {
                                                <option
                                                    value=ns
                                                    prop:selected=move || nf.get().as_deref() == Some(&ns2)
                                                >{ns2.clone()}</option>
                                            }
                                        }).collect_view().into_any()
                                    } else {
                                        ().into_any()
                                    }
                                })}
                            </Suspense>
                        </select>
                    }.into_any()
                } else {
                    ns_sv.get_value().map(|ns| view! { <span class="pane-badge pane-badge-ns">{ns}</span> }).into_any()
                }}
                {sel_sv.get_value().map(|sel| view! { <span class="pane-badge pane-badge-sel">{sel}</span> })}
                {text_filter.map(|tf| {
                    let filter_ref = NodeRef::<leptos::html::Input>::new();
                    let expanded = RwSignal::new(false);

                    let do_expand = move || {
                        expanded.set(true);
                        if let Some(el) = filter_ref.get_untracked() {
                            let _ = el.focus();
                        }
                    };
                    let do_collapse = move || {
                        expanded.set(false);
                        if let Some(el) = filter_ref.get_untracked() {
                            let _ = el.blur();
                        }
                    };

                    // Focus + expand when `/` is pressed globally.
                    if let Some(ff) = use_context::<FilterFocus>().map(|f| f.0) {
                        Effect::new(move |prev: Option<u32>| {
                            let v = ff.get();
                            if prev.is_some() {
                                do_expand();
                            }
                            v
                        });
                    }

                    view! {
                        <div class="view-filter-wrap"
                             class:vfw-expanded=move || expanded.get()
                             class:vfw-active=move || !tf.get().is_empty()
                             on:click=move |_| { if !expanded.get() { do_expand(); } }>
                            <input class="view-filter" node_ref=filter_ref placeholder="filter…"
                                prop:value=move || tf.get()
                                on:input=move |e| tf.set(event_target_value(&e))
                                on:blur=move |_| expanded.set(false)
                                on:keydown=move |e| {
                                    match e.key().as_str() {
                                        "Escape" => {
                                            tf.set(String::new());
                                            do_collapse();
                                        }
                                        "Enter" => do_collapse(),
                                        _ => {}
                                    }
                                } />
                        </div>
                    }
                })}
                <span class="count">{move || format!("{} items", shown_uids.with(|v| v.len()))}</span>
                {on_close.map(|cb| view! {
                    <button class="view-close" on:click=move |_| cb.run(())>"×"</button>
                })}
            </div>
            <div class="bulkbar-wrap" class:open=move || !selected.get().is_empty()>
                <div class="bulkbar">
                    <span class="bulk-count">{move || format!("{} selected", selected.get().len())}</span>
                    <button class="act" on:click=move |_| selected.set(shown_uids.get().into_iter().collect())>"Select all"</button>
                    <button class="act" on:click=move |_| selected.set(std::collections::BTreeSet::new())>"Clear"</button>
                    {(is_pod_kind || bulk_workload).then(|| view! {
                        <button class="act" on:click=move |_| {
                            let uids = selected.get_untracked();
                            let key = key_sv.get_value();
                            let agg = !is_pod_kind;
                            rows.with_untracked(|v| {
                                for r in v.values().filter(|r| uids.contains(&r.uid)) {
                                    open_logs(log_pods, LogTarget::from_row(&key, r, agg));
                                }
                            });
                            selected.set(std::collections::BTreeSet::new());
                        }>"Logs"</button>
                    })}
                    {bulk_workload.then(|| view! {
                        <button class="act" on:click=move |_| do_bulk("restart")>"Restart"</button>
                    })}
                    {bulk_flux.then(|| view! {
                        <button class="act" on:click=move |_| do_bulk("flux-reconcile")>"Reconcile"</button>
                        {bulk_has_source_ref.then(|| view! {
                            <button class="act" on:click=move |_| do_bulk("flux-reconcile-with-source")>"Reconcile w/ source"</button>
                        })}
                        {bulk_helmrelease.then(|| view! {
                            <button class="act" on:click=move |_| do_bulk("flux-force")>"Force"</button>
                            <button class="act" on:click=move |_| do_bulk("flux-reset")>"Reset"</button>
                        })}
                        {move || bulk_show_suspend().then(|| view! {
                            <button class="act" on:click=move |_| do_bulk("flux-suspend")>"Suspend"</button>
                        })}
                        {move || bulk_show_resume().then(|| view! {
                            <button class="act" on:click=move |_| do_bulk("flux-resume")>"Resume"</button>
                        })}
                    })}
                    <button class="act danger" on:click=move |_| {
                        let n = selected.get_untracked().len();
                        ask_confirm(confirm, format!("Delete {n} resources?"), move || do_bulk("delete"));
                    }>"Delete"</button>
                </div>
            </div>
            <div class="table-wrap" node_ref=table_ref>
                <div class="grid-table" style=tmpl class:selecting=move || !selected.get().is_empty()>
                    {header}
                    <div class="grid-row sizer" aria-hidden="true">
                        {move || sizer.get().into_iter().map(|s| view! { <div class="cell">{s}</div> }).collect_view()}
                    </div>
                    <div class="vpad" style=move || {
                        format!("grid-column:1/-1;height:{}px", window.get().0 as f64 * row_h.get())
                    }></div>
                    <For
                        // Key includes the live column count so a CRD column
                        // change remounts the visible rows — their cell count
                        // then matches the reflowed header (never misaligned).
                        each=move || {
                            let ncols = columns.with(|c| c.len());
                            let (first, last) = window.get();
                            shown_uids
                                .with(|v| v.get(first..last).map(<[String]>::to_vec).unwrap_or_default())
                                .into_iter()
                                .map(move |uid| (ncols, uid))
                                .collect::<Vec<_>>()
                        }
                        key=|item| item.clone()
                        let:item
                    >
                        {
                            let (ncols, uid) = item;
                            let key = key.clone();
                            let uid_row = uid.clone();
                            let row = Memo::new(move |_| rows.with(|m| m.get(&uid_row).cloned()));

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
                                        node_col.get_untracked().and_then(|i| r.get().and_then(|rr| rr.cells.get(i).cloned()))
                                    } else {
                                        None
                                    }
                                }
                            };
                            let on_unmount = {
                                let uid = uid.clone();
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
                                        if bool_cols.with_untracked(|v| v.contains(&i)) {
                                            view! { <FlashTd value=val no_flash=true
                                                color=Signal::derive(move || match val().as_str() {
                                                    "true" => "ok",
                                                    "false" => "warn",
                                                    _ => "unknown",
                                                }) /> }.into_any()
                                        } else if pct_thresh_cols.with_untracked(|s| s.contains(&i)) {
                                            view! { <FlashTd value=val no_flash=true trend=trend_sig
                                                color=Signal::derive(move || pct_thresh_color(&val())) /> }.into_any()
                                        } else if metric_cols.with_untracked(|v| v.contains(&i)) {
                                            view! { <FlashTd value=val no_flash=true trend=trend_sig /> }.into_any()
                                        } else if colored_cols.with_untracked(|v| v.contains(&i)) {
                                            view! { <FlashTd value=val flash=flash
                                                color=Signal::derive(move || dot_class(row.get().map(|r| r.status).unwrap_or(RowStatus::Unknown))) /> }.into_any()
                                        } else {
                                            view! { <FlashTd value=val flash=flash trend=trend_sig /> }.into_any()
                                        }
                                    }).collect_view()}
                                    <div class="cell cell-age">
                                        <div class="cw"><div class="cwi">
                                            {move || { tick.get(); data::humanize_age(&row.get().and_then(|r| r.created)) }}
                                        </div></div>
                                    </div>
                                </ResourceRowView>
                            }
                        }
                    </For>
                    <div class="vpad" style=move || {
                        let (_, last) = window.get();
                        let total = shown_uids.with(|v| v.len());
                        format!("grid-column:1/-1;height:{}px", total.saturating_sub(last) as f64 * row_h.get())
                    }></div>
                </div>
                {move || {
                    shown_uids.with(|v| v.is_empty()).then(|| {
                        let problems = only_problems.get();
                        let ns = if let Some(nf) = ns_filter {
                            nf.get()
                        } else {
                            ns_sv.get_value()
                                .or_else(|| selected_ns_ctx.and_then(|s| s.get_untracked()))
                        };
                        let msg = if problems {
                            format!("No {} with problems", title_sv.get_value())
                        } else if let Some(ns) = ns {
                            format!("No {} in namespace \"{}\"", title_sv.get_value(), ns)
                        } else {
                            format!("No {} found", title_sv.get_value())
                        };
                        view! { <div class="empty pad">{msg}</div> }
                    })
                }}
            </div>
        </div>
    }
}
