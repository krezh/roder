//! Mobile replacement for the desktop's multi-kind search grid
//! (`views/search.rs`): a card list, each card labeled with its resource kind.

use std::collections::HashMap;
use std::sync::Arc;

use leptos::prelude::*;
use roder_core::{ResourceKind, ResourceRow, WatchEvent};

use crate::app::events::{apply_event, fire_action};
use crate::app::hooks::{use_table_state, Coalescer};
use crate::app::mobile::bulk_bar::MobileBulkBar;
use crate::app::mobile::row_card::{use_select_mode, CardFields, MobileRowCard};
use crate::app::overlays::toast::Toast;
use crate::app::state::{
    open_logs, Catalog, ConnectionState, CtxMenu, DetailTarget, LogPods, LogTarget,
    MultiKindSearch, OnlyProblems, ResourceFilter, TableRows, TableSelected, Tick,
};
use crate::app::table_logic;
#[cfg(target_arch = "wasm32")]
use crate::app::util::history::history_back;
use crate::data;

/// A row paired with its resource kind, same shape as the desktop's `MergedRow`.
#[derive(Clone, PartialEq)]
struct MergedRow {
    kind: Arc<ResourceKind>,
    row: ResourceRow,
}

#[component]
pub(crate) fn MobileSearchList() -> impl IntoView {
    let log_pods = expect_context::<LogPods>().0;
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let ctx_menu = expect_context::<RwSignal<Option<CtxMenu>>>();
    let tick = expect_context::<Tick>().0;
    let only_problems = expect_context::<OnlyProblems>().0;
    let resource_filter = expect_context::<ResourceFilter>().0;
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let catalog = expect_context::<Catalog>().0;

    let t = use_table_state();

    let sv_sel = expect_context::<TableSelected>().0;
    let sv_rows = expect_context::<TableRows>().0;
    sv_sel.set_value(Some(t.selected));
    sv_rows.set_value(Some(t.rows));
    on_cleanup(move || {
        sv_sel.set_value(None);
        sv_rows.set_value(None);
    });

    let merged_rows: RwSignal<HashMap<String, MergedRow>> = RwSignal::new(Default::default());
    let rows_for_ctx = t.rows;
    Effect::new(move |_| {
        merged_rows.with(|m| {
            rows_for_ctx.set(
                m.iter()
                    .map(|(k, mr)| (k.clone(), mr.row.clone()))
                    .collect(),
            );
        });
    });

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

    let resolved_kinds = Memo::new(move |_| {
        let Some(query) = search_query.get() else {
            return Vec::new();
        };
        let kinds = catalog.get();
        kinds
            .iter()
            .filter(|k| {
                query
                    .kinds
                    .iter()
                    .any(|qn| k.plural.eq_ignore_ascii_case(qn) || k.kind.eq_ignore_ascii_case(qn))
            })
            .map(|k| Arc::new(k.clone()))
            .collect::<Vec<Arc<ResourceKind>>>()
    });

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
                let probe_url = url.clone();
                let coalescer = Coalescer::new(move |batch: Vec<WatchEvent>| {
                    for ev in batch {
                        match ev {
                            WatchEvent::Snapshot { columns, rows: r } => {
                                if let Some(c) = conn {
                                    c.set(None);
                                }
                                apply_event(
                                    kr,
                                    ent,
                                    rm,
                                    None,
                                    WatchEvent::Snapshot {
                                        columns,
                                        rows: r.clone(),
                                    },
                                );
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
                            WatchEvent::Applied { row } => {
                                apply_event(
                                    kr,
                                    ent,
                                    rm,
                                    None,
                                    WatchEvent::Applied { row: row.clone() },
                                );
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
                            WatchEvent::Deleted { uid } => {
                                apply_event(
                                    kr,
                                    ent,
                                    rm,
                                    None,
                                    WatchEvent::Deleted { uid: uid.clone() },
                                );
                                let merged_key = format!("{}{}", pfx, uid);
                                rm.update(|s| {
                                    s.insert(merged_key);
                                });
                            }
                        }
                    }
                });
                data::subscribe_with_error(
                    &url,
                    move |ev| coalescer.push(ev),
                    move || {
                        if let Some(c) = conn {
                            let probe = probe_url.clone();
                            leptos::task::spawn_local(async move {
                                let msg = data::probe_error(probe).await;
                                c.set(Some(msg));
                            });
                        }
                        set_timeout(
                            move || reconnect.update(|n| *n += 1),
                            data::reconnect_delay(),
                        );
                    },
                )
            });
        }
    });

    let shown_uids = Memo::new(move |_| {
        let problems = only_problems.get();
        let filter_text = resource_filter.get().to_lowercase();
        let (sort_key, asc) = t.sort.get();
        merged_rows.with(|m| {
            table_logic::shown_uids(
                m.values().map(|mr| &mr.row),
                sort_key,
                asc,
                problems,
                &filter_text,
            )
        })
    });

    let selected = t.selected;
    let select_mode = use_select_mode(selected);

    let do_bulk = move |action: &'static str| {
        let uids = selected.get_untracked();
        let targets: Vec<DetailTarget> = merged_rows.with_untracked(|m| {
            uids.iter()
                .filter_map(|uid| {
                    m.get(uid).map(|mr| DetailTarget {
                        key: mr.kind.key.clone(),
                        namespace: mr.row.namespace.clone(),
                        name: mr.row.name.clone(),
                    })
                })
                .collect()
        });
        fire_action(toast, action, &targets);
        selected.set(Default::default());
    };
    let on_logs = Callback::new(move |_| {
        let uids = selected.get_untracked();
        merged_rows.with_untracked(|m| {
            for uid in &uids {
                if let Some(mr) = m.get(uid) {
                    let is_pod = mr.kind.group.is_empty() && mr.kind.kind == "Pod";
                    open_logs(
                        log_pods,
                        LogTarget {
                            key: mr.kind.key.clone(),
                            namespace: mr.row.namespace.clone().unwrap_or_default(),
                            name: mr.row.name.clone(),
                            aggregate: !is_pod,
                        },
                    );
                }
            }
        });
        selected.set(Default::default());
    });

    let clear_search = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            data::session_storage_remove("roder_search_query");
            history_back();
        }
    };

    view! {
        <div class="mobile-list">
            <div class="mobile-list-head">
                <span class="view-title">"Search"</span>
                <button class="act" on:click=clear_search>"Clear"</button>
                <button class="mobile-select-toggle" class:active=move || select_mode.get()
                    on:click=move |_| select_mode.update(|s| *s = !*s)>
                    {move || if select_mode.get() { "Cancel" } else { "Select" }}
                </button>
            </div>
            <div class="mobile-cards">
                <For each=move || shown_uids.get() key=|uid| uid.clone() let:uid>
                    {
                        let uid_row = uid.clone();
                        let merged = Memo::new(move |_| merged_rows.with(|m| m.get(&uid_row).cloned()));
                        let init = merged.get_untracked();
                        let target = DetailTarget {
                            key: init.as_ref().map(|mr| mr.kind.key.clone()).unwrap_or_default(),
                            namespace: init.as_ref().and_then(|mr| mr.row.namespace.clone()),
                            name: init.as_ref().map(|mr| mr.row.name.clone()).unwrap_or_default(),
                        };
                        let node_for_ctx = {
                            let kinds = resolved_kinds.get_untracked();
                            let node_col = kinds.first().and_then(|k| k.columns.iter().position(|c| c == "Node"));
                            move || {
                                let node_col = node_col?;
                                merged.get_untracked().and_then(|mr| mr.row.cells.get(node_col).cloned())
                            }
                        };
                        let fields = Memo::new(move |_| {
                            let mr = merged.get()?;
                            tick.get();
                            Some(CardFields::from_row(
                                &mr.row,
                                &mr.kind.columns,
                                Some(mr.kind.kind.clone()),
                                data::humanize_age(&mr.row.created),
                            ))
                        });
                        view! {
                            <MobileRowCard
                                uid=uid.clone()
                                target=target
                                detail=detail
                                ctx_menu=ctx_menu
                                selected=selected
                                select_mode=select_mode
                                press=t.press
                                node_for_ctx=node_for_ctx
                                fields=fields />
                        }
                    }
                </For>
                {move || shown_uids.with(|v| v.is_empty()).then(|| view! { <div class="empty pad">"No results"</div> })}
            </div>
            <MobileBulkBar
                selected=selected
                all_uids=move || shown_uids.get()
                do_bulk=do_bulk
                on_logs=on_logs />
        </div>
    }
}
