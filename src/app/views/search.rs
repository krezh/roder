//! Multi-kind search results view: live SSE-fed table for N resource kinds
//! simultaneously, with merged column schema, sorting, multi-select, and bulk actions.

use std::collections::HashMap;
use std::sync::Arc;

use leptos::prelude::*;
use roder_core::{ResourceKind, ResourceRow, RowStatus, Trend};

use crate::app::components::table::{cmp_str, sortable_th, FlashTd};
use crate::app::components::table_row::{NameCell, ResourceRow as ResourceRowView};
use crate::app::events::{apply_event, fire_action};
use crate::app::hooks::{table_window, use_table_state};
use crate::app::overlays::confirm::{ask_confirm, Confirm};
use crate::app::state::{
    open_logs, ConnectionState, CtxMenu, DetailTarget, LogPods, LogTarget,
    MultiKindSearch, OnlyProblems, ResourceFilter, SortKey, TableRows, TableSelected, Tick,
};
#[cfg(target_arch = "wasm32")]
use crate::app::util::history::history_back;
use crate::data;

/// A row paired with its resource kind for multi-kind results. The `kind` is
/// wrapped in `Arc` so the SSE handler doesn't need to clone the kind on every
/// event for every row.
#[derive(Clone, PartialEq)]
struct MergedRow {
    kind: Arc<ResourceKind>,
    row: ResourceRow,
}

/// Unified column schema for multi-kind results.
#[derive(Clone, Debug, PartialEq)]
struct UnifiedColumn {
    name: String,
    /// Index in the original kind's columns, if applicable
    kind_column_idx: Option<usize>,
    /// Whether this column should be colored by status
    colored: bool,
    /// Whether this is a metric column (no flash)
    is_metric: bool,
    /// Whether this column holds a boolean value (true/false) and should be
    /// coloured by the value itself — green for "true", amber for "false" —
    /// rather than by the row's status. Used for PVC Mount, etc.
    bool_colored: bool,
}

/// Build unified column schema from multiple resource kinds.
///
/// Columns from every kind are merged in encounter order (deduped by name).
/// Cell values are resolved per-row by name, so the index stored here is only
/// used as a tiebreaker for ordering — not for cell access.
fn build_unified_columns(kinds: &[Arc<ResourceKind>]) -> Vec<UnifiedColumn> {
    let mut unified = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if kinds.iter().any(|k| k.namespaced) {
        unified.push(UnifiedColumn {
            name: "Namespace".to_string(),
            kind_column_idx: None,
            colored: false,
            is_metric: false,
            bool_colored: false,
        });
        seen.insert("Namespace".to_string());
    }

    unified.push(UnifiedColumn {
        name: "Name".to_string(),
        kind_column_idx: None,
        colored: false,
        is_metric: false,
        bool_colored: false,
    });
    seen.insert("Name".to_string());

    // Merge columns from ALL kinds so every kind's specific columns appear.
    for kind in kinds {
        for (idx, col) in kind.columns.iter().enumerate() {
            if seen.insert(col.clone()) {
                let colored = matches!(col.as_str(), "Phase" | "Status" | "Ready");
                let is_metric =
                    col.starts_with("CPU") || col.starts_with("MEM") || col.starts_with("%");
                let bool_colored = matches!(col.as_str(), "Mount");
                unified.push(UnifiedColumn {
                    name: col.clone(),
                    kind_column_idx: Some(idx),
                    colored,
                    is_metric,
                    bool_colored,
                });
            }
        }
    }

    unified.push(UnifiedColumn {
        name: "Age".to_string(),
        kind_column_idx: None,
        colored: false,
        is_metric: false,
        bool_colored: false,
    });

    unified
}

/// Look up the cell value for `col` in a `MergedRow` by column name.
///
/// The lookup is per-row so that kinds with different column orderings (or no
/// column of that name at all) each resolve correctly. A kind that doesn't have
/// the column returns `""` instead of leaking another kind's cell value.
fn cell_value_str<'a>(col: &UnifiedColumn, mr: &'a MergedRow) -> &'a str {
    match col.name.as_str() {
        "Namespace" => mr.row.namespace.as_deref().unwrap_or(""),
        "Name" => mr.row.name.as_str(),
        "Age" => mr.row.created.as_deref().unwrap_or(""),
        col_name => {
            let Some(idx) = mr.kind.columns.iter().position(|c| c.as_str() == col_name) else {
                return "";
            };
            mr.row.cells.get(idx).map(String::as_str).unwrap_or("")
        }
    }
}

#[component]
pub(crate) fn SearchResultsView() -> impl IntoView {
    let log_pods = expect_context::<LogPods>().0;
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let ctx_menu = expect_context::<RwSignal<Option<CtxMenu>>>();
    let tick = expect_context::<Tick>().0;
    let only_problems = expect_context::<OnlyProblems>().0;
    let resource_filter = expect_context::<ResourceFilter>().0;
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();

    let t = use_table_state();

    // Register selection + row map so the context menu can fire bulk actions in the search view.
    let sv_sel = expect_context::<TableSelected>().0;
    let sv_rows = expect_context::<TableRows>().0;
    sv_sel.set_value(Some(t.selected));
    sv_rows.set_value(Some(t.rows));
    on_cleanup(move || {
        sv_sel.set_value(None);
        sv_rows.set_value(None);
    });

    // Merged rows: the parent map for the search view. Keyed by `{kind_key}/{uid}`
    // so the same uid from different kinds can't collide. Holds a `MergedRow`
    // (kind + row) so the row component can deref both at render time.
    let merged_rows: RwSignal<HashMap<String, MergedRow>> = RwSignal::new(Default::default());
    // Mirror merged_rows into t.rows (ResourceRow projection) so the context menu's
    // TableRows lookup finds namespace/name by uid for bulk right-click actions.
    let rows_for_ctx = t.rows;
    Effect::new(move |_| {
        merged_rows.with(|m| {
            rows_for_ctx.set(m.iter().map(|(k, mr)| (k.clone(), mr.row.clone())).collect());
        });
    });

    // Load search query from session storage.
    let search_query = RwSignal::new(None::<MultiKindSearch>);
    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        if let Some(json) = data::session_storage_get("roder_search_query") {
            if let Ok(query) = serde_json::from_str::<MultiKindSearch>(&json) {
                resource_filter.set(query.text.clone());
                search_query.set(Some(query));
            }
        }
    });

    // Resolve the query's `kinds: Vec<String>` to actual `Arc<ResourceKind>`s.
    // Memoized so the subscription Effect and the column schema share one source.
    let resolved_kinds: RwSignal<Vec<Arc<ResourceKind>>> = RwSignal::new(Default::default());
    Effect::new(move |_| {
        let Some(query) = search_query.get() else {
            resolved_kinds.set(Default::default());
            return;
        };
        let catalog = expect_context::<crate::app::state::Catalog>().0;
        let kinds = catalog.get();
        let resolved: Vec<Arc<ResourceKind>> = kinds
            .iter()
            .filter(|k| {
                query
                    .kinds
                    .iter()
                    .any(|qn| k.plural.eq_ignore_ascii_case(qn) || k.kind.eq_ignore_ascii_case(qn))
            })
            .map(|k| Arc::new(k.clone()))
            .collect();
        resolved_kinds.set(resolved);
    });

    // Unified column schema from the resolved kinds.
    let unified_columns = Memo::new(move |_| build_unified_columns(&resolved_kinds.get()));

    // Subscribe to each resolved kind via SSE. Label selectors are passed to the
    // watch URL so the API server filters server-side — no client-side fan-out.
    // Re-subscribes when the query changes; fully resets table state each time.
    Effect::new(move |_| {
        let Some(query) = search_query.get() else {
            return;
        };

        merged_rows.set(Default::default());
        t.selected.set(Default::default());
        t.last_clicked.set(None);
        t.entering.set(Default::default());
        t.removing.set(Default::default());
        t.scroll_top.set(0.0);

        let kinds = resolved_kinds.get();
        if kinds.is_empty() {
            return;
        }

        let conn = use_context::<ConnectionState>().map(|c| c.0);
        for kind in &kinds {
            let kind_key = kind.key.clone();
            let url = data::watch_url(
                &kind_key,
                query.namespaces.first().map(String::as_str),
                query.selector.as_deref(),
            );
            let entering = t.entering;
            let removing = t.removing;
            let kind_arc = kind.clone();
            // Per-kind row buffer; events are applied here, then mirrored to
            // the parent `merged_rows` with the `{kind_key}/` prefix.
            let kind_rows = RwSignal::new(HashMap::<String, ResourceRow>::new());
            let prefix = format!("{}/", kind_key);
            let reconnect: RwSignal<u32> = RwSignal::new(0);
            Effect::new(move |_prev: Option<Option<data::SseHandle>>| {
                reconnect.track();
                let ka = kind_arc.clone();
                let url = url.clone();
                let kr = kind_rows;
                let mr = merged_rows;
                let ent = entering;
                let rm = removing;
                let pfx = prefix.clone();
                data::subscribe_with_error(&url, move |ev| {
                    use roder_core::WatchEvent::*;
                    match ev {
                        // Snapshot replaces the whole kind: mirror the full rebuild
                        // (otherwise the per-kind buffer's safety-net timeouts for
                        // deletes would orphan entries in the parent).
                        Snapshot { rows: r } => {
                            if let Some(c) = conn { c.set(true); }
                            apply_event(kr, ent, rm, Snapshot { rows: r.clone() });
                            mr.update(|m| {
                                m.retain(|k, _| !k.starts_with(&pfx));
                                for row in r {
                                    let merged_key = format!("{}{}", pfx, row.uid);
                                    m.insert(
                                        merged_key,
                                        MergedRow {
                                            kind: ka.clone(),
                                            row,
                                        },
                                    );
                                }
                            });
                        }
                        // Applied upserts a single row — no full rebuild needed.
                        Applied { row } => {
                            apply_event(kr, ent, rm, Applied { row: row.clone() });
                            let merged_key = format!("{}{}", pfx, row.uid);
                            mr.update(|m| {
                                m.insert(
                                    merged_key,
                                    MergedRow {
                                        kind: ka.clone(),
                                        row,
                                    },
                                );
                            });
                        }
                        // Deleted removes a single row — the per-kind buffer's
                        // `apply_event` already tags it in `removing`; mirror that
                        // tagging for the parent's row.
                        Deleted { uid } => {
                            apply_event(kr, ent, rm, Deleted { uid: uid.clone() });
                            let merged_key = format!("{}{}", pfx, uid);
                            rm.update(|s| {
                                s.insert(merged_key);
                            });
                        }
                    }
                }, move || {
                    if let Some(c) = conn { c.set(false); }
                    set_timeout(
                        move || reconnect.update(|n| *n += 1),
                        std::time::Duration::from_secs(3),
                    );
                })
            });
        }
    });

    // Sorted/filtered uids. Label selectors are applied server-side via the SSE
    // watch URL, so only text + problems filtering is needed here.
    let shown_uids = Memo::new(move |_| {
        let problems = only_problems.get();
        let filter_text = resource_filter.get().to_lowercase();
        let (key, asc) = t.sort.get();
        let cols = unified_columns.get();
        merged_rows.with(|m| {
            let mut v: Vec<(&String, &MergedRow)> = m
                .iter()
                .filter(|(_, mr)| {
                    !problems || matches!(mr.row.status, RowStatus::Error | RowStatus::Warn)
                })
                .filter(|(_, mr)| {
                    if filter_text.is_empty() {
                        true
                    } else {
                        mr.row.name.to_lowercase().contains(&filter_text)
                    }
                })
                .collect();
            v.sort_by(|a, b| {
                let ord = match key {
                    SortKey::Namespace => {
                        a.1.row
                            .namespace
                            .cmp(&b.1.row.namespace)
                            .then_with(|| a.1.row.name.cmp(&b.1.row.name))
                    }
                    SortKey::Name => a.1.row.name.cmp(&b.1.row.name),
                    SortKey::Age => {
                        a.1.row
                            .created
                            .cmp(&b.1.row.created)
                            .then_with(|| a.1.row.name.cmp(&b.1.row.name))
                    }
                    SortKey::Cell(i) => {
                        if let Some(col) = cols.get(i) {
                            // Borrow both cell values directly, then numeric-or-lexical compare.
                            let av = cell_value_str(col, a.1);
                            let bv = cell_value_str(col, b.1);
                            cmp_str(av, bv)
                                .then_with(|| a.1.row.name.cmp(&b.1.row.name))
                        } else {
                            a.1.row.name.cmp(&b.1.row.name)
                        }
                    }
                }
                .then_with(|| a.0.cmp(b.0));
                if asc {
                    ord
                } else {
                    ord.reverse()
                }
            });
            v.into_iter()
                .map(|(uid, _)| uid.clone())
                .collect::<Vec<String>>()
        })
    });

    let window = table_window(t, shown_uids);

    let table_ref = t.table_ref;
    let selected = t.selected;
    let last_clicked = t.last_clicked;
    let sort = t.sort;
    let entering = t.entering;
    let removing = t.removing;
    let row_h = t.row_h;
    let press = t.press;

    let grid_template = RwSignal::new(String::new());
    Effect::new(move |_| {
        let cols = unified_columns.get();
        if cols.is_empty() { return; }
        let new_tmpl = format!(
            "grid-template-columns: {};",
            vec!["max-content"; cols.len()].join(" ")
        );
        if grid_template.get_untracked() != new_tmpl {
            grid_template.set(new_tmpl);
        }
    });
    let sizer: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    Effect::new(move |_| {
        let cols = unified_columns.get();
        if cols.is_empty() { return; }
        let _ = shown_uids.with(|v| v.len());
        let mut col_maxes: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
        merged_rows.with_untracked(|m| {
            for mr in m.values() {
                for (i, col) in cols.iter().enumerate() {
                    let val = cell_value_str(col, mr);
                    if val.len() > col_maxes[i].len() {
                        col_maxes[i] = val.to_string();
                    }
                }
            }
        });
        sizer.set(col_maxes);
    });

    // Bulk action helper
    let do_bulk = move |action: &'static str| {
        let uids = selected.get_untracked();
        merged_rows.with_untracked(|m| {
            for uid in &uids {
                if let Some(mr) = m.get(uid) {
                    let t = DetailTarget {
                        key: mr.kind.key.clone(),
                        namespace: mr.row.namespace.clone(),
                        name: mr.row.name.clone(),
                    };
                    fire_action(action, &t);
                }
            }
        });
        selected.set(std::collections::BTreeSet::new());
    };

    let clear_search = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            data::session_storage_remove("roder_search_query");
            history_back();
        }
    };

    let on_unmount = move |uid: String| {
        merged_rows.update(|m| {
            m.remove(&uid);
        });
    };
    let _ = on_unmount; // captured by the For row closures below

    view! {
        <div class="resource-view">
            <div class="view-head">
                <h2 class="view-title">"Search Results"</h2>
                <button class="act" on:click=clear_search>"Clear Search"</button>
                <span class="count">{move || format!("{} items", shown_uids.with(|v| v.len()))}</span>
            </div>
            <div class="bulkbar-wrap" class:open=move || !selected.get().is_empty()>
                <div class="bulkbar">
                    <span class="bulk-count">{move || format!("{} selected", selected.get().len())}</span>
                    <button class="act" on:click=move |_| selected.set(shown_uids.get().into_iter().collect())>"Select all"</button>
                    <button class="act" on:click=move |_| selected.set(std::collections::BTreeSet::new())>"Clear"</button>
                    <button class="act" on:click=move |_| {
                        let uids = selected.get_untracked();
                        merged_rows.with_untracked(|m| {
                            for uid in &uids {
                                if let Some(mr) = m.get(uid) {
                                    let is_pod = mr.kind.group.is_empty() && mr.kind.kind == "Pod";
                                    open_logs(log_pods, LogTarget {
                                        key: mr.kind.key.clone(),
                                        namespace: mr.row.namespace.clone().unwrap_or_default(),
                                        name: mr.row.name.clone(),
                                        aggregate: !is_pod,
                                    });
                                }
                            }
                        });
                        selected.set(std::collections::BTreeSet::new());
                    }>"Logs"</button>
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
                    <div class="grid-row head">
                        {move || {
                            let cols = unified_columns.get();
                            cols.iter().enumerate().map(|(i, col)| {
                                let sort_key = match col.name.as_str() {
                                    "Namespace" => SortKey::Namespace,
                                    "Name" => SortKey::Name,
                                    "Age" => SortKey::Age,
                                    _ => SortKey::Cell(i),
                                };
                                sortable_th(col.name.clone(), sort_key, sort)
                            }).collect_view()
                        }}
                    </div>
                    <div class="grid-row sizer" aria-hidden="true">
                        {move || sizer.get().into_iter().map(|s| view! { <div class="cell">{s}</div> }).collect_view()}
                    </div>
                    <div class="vpad" style=move || {
                        format!("grid-column:1/-1;height:{}px", window.get().0 as f64 * row_h.get())
                    }></div>
                    <For each=move || {
                        let (first, last) = window.get();
                        shown_uids.with(|v| v.get(first..last).map(<[String]>::to_vec).unwrap_or_default())
                    } key=|uid| uid.clone() let:uid>
                        {
                            let uid_row = uid.clone();
                            let merged = Memo::new(move |_| merged_rows.with(|m| m.get(&uid_row).cloned()));
                            let init = merged.get_untracked();
                            let target = DetailTarget {
                                key: init.as_ref().map(|mr| mr.kind.key.clone()).unwrap_or_default(),
                                namespace: init.as_ref().and_then(|mr| mr.row.namespace.clone()),
                                name: init.as_ref().map(|mr| mr.row.name.clone()).unwrap_or_default(),
                            };
                            // For the context menu, derive the "node" from the first kind's
                            // Node column (only meaningful for pod kinds).
                            let node_for_ctx = {
                                let kinds = resolved_kinds.get();
                                let node_col = kinds.first().and_then(|k| k.columns.iter().position(|c| c == "Node"));
                                move || {
                                    let node_col = node_col?;
                                    merged.get().and_then(|mr| mr.row.cells.get(node_col).cloned())
                                }
                            };
                            let on_unmount = {
                                let uid = uid.clone();
                                move |u: String| {
                                    debug_assert_eq!(u, uid, "transitionend uid mismatch");
                                    merged_rows.update(|m| { m.remove(&uid); });
                                }
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
                                    on_unmount=Callback::new(on_unmount)
                                    shown_uids=shown_uids
                                    press=press
                                    node_for_ctx=node_for_ctx>
                                    {move || {
                                        let cols = unified_columns.get();
                                        cols.iter().map(|col| {
                                            let col_colored = col.colored;
                                            let col_is_metric = col.is_metric;
                                            let col_bool_colored = col.bool_colored;
                                            match col.name.as_str() {
                                                "Name" => {
                                                    view! {
                                                        <NameCell
                                                            uid=uid.clone()
                                                            name=move || merged.get().map(|mr| mr.row.name)
                                                            status=move || merged.get().map(|mr| mr.row.status)
                                                            selected=selected
                                                            last_clicked=last_clicked
                                                            shown_uids=shown_uids />
                                                    }.into_any()
                                                }
                                                "Namespace" => {
                                                    view! {
                                                        <FlashTd
                                                            value=move || merged.get().and_then(|mr| mr.row.namespace.clone()).unwrap_or_default()
                                                            class="cell-ns" />
                                                    }.into_any()
                                                }
                                                "Age" => {
                                                    view! {
                                                        <div class="cell cell-age"><div class="cw"><div class="cwi">{move || { tick.get(); merged.get().as_ref().and_then(|mr| mr.row.created.as_ref().map(|c| data::humanize_age(&Some(c.clone())))).unwrap_or_default() }}</div></div></div>
                                                    }.into_any()
                                                }
                                                _ => {
                                                    // StoredValue<String> is Copy so it can be captured
                                                    // by all closures (value, color, trend) in both
                                                    // branches without cloning or partial-move errors.
                                                    // We look up the column index per row so mixed-kind
                                                    // results with different column orderings work.
                                                    let col_name_sv = StoredValue::new(col.name.clone());
                                                    if col_colored || col_bool_colored {
                                                        view! {
                                                            <FlashTd value=move || {
                                                                let cn = col_name_sv.get_value();
                                                                merged.get()
                                                                    .and_then(|mr| {
                                                                        let i = mr.kind.columns.iter().position(|c| c.as_str() == cn.as_str())?;
                                                                        mr.row.cells.get(i).cloned()
                                                                    })
                                                                    .unwrap_or_default()
                                                            } no_flash=col_is_metric
                                                                color=Signal::derive(move || {
                                                                    if col_bool_colored {
                                                                        let cn = col_name_sv.get_value();
                                                                        match merged.get()
                                                                            .and_then(|mr| {
                                                                                let i = mr.kind.columns.iter().position(|c| c.as_str() == cn.as_str())?;
                                                                                mr.row.cells.get(i).cloned()
                                                                            })
                                                                            .as_deref()
                                                                        {
                                                                            Some("true") => "ok",
                                                                            Some("false") => "warn",
                                                                            _ => "unknown",
                                                                        }
                                                                    } else {
                                                                        crate::app::util::color::dot_class(merged.get().map(|mr| mr.row.status).unwrap_or(RowStatus::Unknown))
                                                                    }
                                                                }) />
                                                        }.into_any()
                                                    } else {
                                                        let trend_sig = Signal::derive(move || {
                                                            let cn = col_name_sv.get_value();
                                                            merged.get().and_then(|mr| {
                                                                let i = mr.kind.columns.iter().position(|c| c.as_str() == cn.as_str())?;
                                                                mr.row.trends.get(i).copied()
                                                            }).unwrap_or(Trend::None)
                                                        });
                                                        view! {
                                                            <FlashTd value=move || {
                                                                let cn = col_name_sv.get_value();
                                                                merged.get()
                                                                    .and_then(|mr| {
                                                                        let i = mr.kind.columns.iter().position(|c| c.as_str() == cn.as_str())?;
                                                                        mr.row.cells.get(i).cloned()
                                                                    })
                                                                    .unwrap_or_default()
                                                            } no_flash=col_is_metric trend=trend_sig />
                                                        }.into_any()
                                                    }
                                                }
                                            }
                                        }).collect_view()
                                    }}
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
                    if !shown_uids.with(|v| v.is_empty()) {
                        return None;
                    }
                    let problems = only_problems.get();
                    let kinds = resolved_kinds.get();
                    let filter_text = resource_filter.get();
                    let kind_list = if kinds.is_empty() {
                        "resources".to_string()
                    } else {
                        kinds.iter()
                            .map(|k| k.kind.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    let msg = if !filter_text.is_empty() {
                        format!("No {} matching \"{}\"", kind_list, filter_text)
                    } else if problems {
                        format!("No {} with problems", kind_list)
                    } else {
                        format!("No {} found", kind_list)
                    };
                    Some(view! { <div class="empty pad">{msg}</div> })
                }}
            </div>
        </div>
    }
}
