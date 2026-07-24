//! Mobile replacement for `views/workspace.rs`: the desktop lays every pane
//! out side-by-side in a CSS grid, which doesn't fit a phone's width — mobile
//! shows one pane at a time with a chip switcher instead. The multiplexed SSE
//! subscription (one connection for every pane) mirrors the desktop version.

use std::collections::HashMap;

use leptos::prelude::*;
use roder_core::{ResourceKind, WatchEvent};

use crate::app::events::{make_bulk_open_logs, make_do_bulk, make_do_delete, RowMap};
use crate::app::hooks::{use_table_state, Coalescer};
use crate::app::mobile::bulk_bar::MobileBulkBar;
use crate::app::mobile::row_card::{use_select_mode, CardFields, MobileRowCard};
use crate::app::overlays::toast::Toast;
use crate::app::state::{
    Catalog, ConnectionState, Connectivity, CtxMenu, DetailTarget, LogPods, OnlyProblems, SortKey,
    Tick, WorkspaceConf,
};
use crate::app::table_logic;
use crate::app::util::predicate::KindKind;
use crate::data;

#[component]
pub(crate) fn MobileWorkspaceView() -> impl IntoView {
    let ws = expect_context::<WorkspaceConf>().0;
    let catalog = expect_context::<Catalog>().0;
    let connection = expect_context::<ConnectionState>().0;

    let pane_rows: StoredValue<HashMap<String, RowMap>> = StoredValue::new(HashMap::new());
    let pane_loaded: StoredValue<HashMap<String, RwSignal<bool>>> =
        StoredValue::new(HashMap::new());

    let reconnect = RwSignal::new(0u32);
    Effect::new(move |_prev: Option<Option<data::SseHandle>>| {
        reconnect.track();
        let panes = ws.with(|w| w.panes.clone());
        if panes.is_empty() {
            return None;
        }

        pane_rows.update_value(|map| {
            map.retain(|k, _| panes.iter().any(|p| &p.kind_key == k));
        });
        pane_loaded.update_value(|map| {
            map.retain(|k, _| panes.iter().any(|p| &p.kind_key == k));
        });

        for p in &panes {
            pane_rows.update_value(|map| {
                map.entry(p.kind_key.clone())
                    .or_insert_with(|| RwSignal::new(HashMap::new()));
            });
            pane_loaded.update_value(|map| {
                map.entry(p.kind_key.clone())
                    .or_insert_with(|| RwSignal::new(false));
            });
        }

        let url = data::watch_multi_url(
            &panes
                .iter()
                .map(|p| (p.kind_key.as_str(), p.namespace.as_deref()))
                .collect::<Vec<_>>(),
        );
        let probe_url = url.clone();

        let coalescer = Coalescer::new(move |batch: Vec<(String, WatchEvent)>| {
            for (key, event) in batch {
                let is_snapshot = matches!(event, WatchEvent::Snapshot { .. });
                pane_rows.with_value(|map| {
                    if let Some(&rows) = map.get(&key) {
                        match event {
                            WatchEvent::Snapshot { rows: r, .. } => {
                                rows.set(r.into_iter().map(|row| (row.uid.clone(), row)).collect());
                            }
                            WatchEvent::Applied { row } => {
                                rows.update(|m| {
                                    m.insert(row.uid.clone(), row);
                                });
                            }
                            WatchEvent::Deleted { uid } => {
                                rows.update(|m| {
                                    m.remove(&uid);
                                });
                            }
                            WatchEvent::Forbidden { message } => {
                                leptos::logging::warn!("watch forbidden for pane {key}: {message}");
                            }
                        }
                    }
                });
                if is_snapshot {
                    pane_loaded.with_value(|map| {
                        if let Some(&loaded) = map.get(&key) {
                            loaded.set(true);
                        }
                    });
                }
            }
        });

        data::subscribe_multi(
            &url,
            move |key, event| coalescer.push((key, event)),
            move || {
                let url = probe_url.clone();
                leptos::task::spawn_local(async move {
                    connection.set(Connectivity::Error(data::probe_error(url).await));
                });
                set_timeout(
                    move || reconnect.update(|n| *n += 1),
                    data::reconnect_delay(),
                );
            },
        )
    });

    let active_key = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        let panes = ws.with(|w| w.panes.clone());
        if active_key
            .get_untracked()
            .as_ref()
            .is_none_or(|k| !panes.iter().any(|p| &p.kind_key == k))
        {
            active_key.set(panes.first().map(|p| p.kind_key.clone()));
        }
    });

    view! {
        <div class="mobile-list">
            <div class="mobile-pane-switcher">
                <For each=move || ws.with(|w| w.panes.clone()) key=|cfg| cfg.kind_key.clone() let:cfg>
                    {
                        let key_click = cfg.kind_key.clone();
                        let key_active = cfg.kind_key.clone();
                        let key_close = cfg.kind_key.clone();
                        let title = catalog.get_untracked().into_iter().find(|k| k.key == cfg.kind_key).map(|k| k.kind).unwrap_or(cfg.kind_key.clone());
                        view! {
                            <span class="mobile-pane-chip" class:active=move || active_key.get().as_deref() == Some(&key_active)
                                on:click=move |_| active_key.set(Some(key_click.clone()))>
                                {title}
                                <span class="mobile-pane-close" on:click=move |e: leptos::ev::MouseEvent| {
                                    e.stop_propagation();
                                    ws.update(|w| w.panes.retain(|p| p.kind_key != key_close));
                                }>"×"</span>
                            </span>
                        }
                    }
                </For>
            </div>
            {move || ws.with(|w| w.panes.is_empty()).then(|| view! {
                <div class="workspace-empty">
                    <p>"No panes — add one from the sidebar's kind context menu."</p>
                </div>
            })}
            {move || {
                let key = active_key.get()?;
                let kind = catalog.get().into_iter().find(|k| k.key == key)?;
                let rows = pane_rows.with_value(|m| m.get(&key).copied())?;
                let loaded = pane_loaded.with_value(|m| m.get(&key).copied())?;
                if !loaded.get() {
                    return None;
                }
                Some(view! { <MobilePane kind=kind rows=rows /> })
            }}
        </div>
    }
}

#[component]
fn MobilePane(kind: ResourceKind, rows: RowMap) -> impl IntoView {
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let ctx_menu = expect_context::<RwSignal<Option<CtxMenu>>>();
    let only_problems = expect_context::<OnlyProblems>().0;
    let log_pods = expect_context::<LogPods>().0;
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let tick = expect_context::<Tick>().0;

    // Rows are driven by the workspace's own multiplexed subscription (the
    // `rows` prop), so only the selection/sort/long-press pieces of
    // `use_table_state` are used here — `t.rows` etc. stay unused rather than
    // hand-reimplementing that plumbing (e.g. `RowPress`) a third time.
    let t = use_table_state();
    let sort = t.sort;
    sort.set((SortKey::Namespace, true));
    let text_filter = RwSignal::new(String::new());
    let selected = t.selected;
    let select_mode = use_select_mode(selected);

    let shown_uids = Memo::new(move |_| {
        let problems = only_problems.get();
        let filter_text = text_filter.get().to_lowercase();
        let (sort_key, asc) = sort.get();
        rows.with(|m| {
            table_logic::shown_uids(m.values(), sort_key, asc, problems, None, &filter_text)
        })
    });

    let is_pod_kind = kind.group.is_empty() && kind.kind == "Pod";
    let kk = KindKind::new(&kind.group, &kind.kind);
    let bulk_workload = kk.is_workload();
    let bulk_flux = kk.is_flux();
    let bulk_helmrelease = kk.is_helmrelease();
    let bulk_has_source_ref = kk.has_source_ref();
    let key_sv = StoredValue::new(kind.key.clone());
    let columns_sv = StoredValue::new(kind.columns.clone());

    let reset_selection = move || select_mode.set(false);
    let do_bulk = make_do_bulk(toast, key_sv, rows, selected, reset_selection);
    let do_delete = make_do_delete(toast, key_sv, rows, selected, reset_selection);
    let do_logs = make_bulk_open_logs(
        log_pods,
        key_sv,
        rows,
        selected,
        is_pod_kind,
        reset_selection,
    );
    let on_logs = Callback::new(move |()| do_logs());

    view! {
        <div class="mobile-list-head">
            <input class="mobile-filter" placeholder="Filter…"
                prop:value=move || text_filter.get()
                on:input=move |e| text_filter.set(event_target_value(&e)) />
        </div>
        <div class="mobile-cards">
            <For each=move || shown_uids.get() key=|uid| uid.clone() let:uid>
                {
                    let key = key_sv.get_value();
                    let uid_row = uid.clone();
                    let row = Memo::new(move |_| rows.with(|m| m.get(&uid_row).cloned()));
                    let init = row.get_untracked();
                    let target = DetailTarget {
                        key: key.clone(),
                        namespace: init.as_ref().and_then(|r| r.namespace.clone()),
                        name: init.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
                    };
                    let fields = Memo::new(move |_| {
                        let r = row.get()?;
                        let cols = columns_sv.get_value();
                        tick.get();
                        Some(CardFields::from_row(&r, &cols, None, data::humanize_age(&r.created)))
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
                            node_for_ctx=move || None
                            fields=fields />
                    }
                }
            </For>
            {move || shown_uids.with(|v| v.is_empty()).then(|| view! { <div class="empty pad">"No resources"</div> })}
        </div>
        <MobileBulkBar
            selected=selected
            select_mode=select_mode
            all_uids=move || shown_uids.get()
            do_bulk=do_bulk
            do_delete=do_delete
            on_logs=on_logs
            show_logs=is_pod_kind || bulk_workload
            bulk_workload=bulk_workload
            bulk_flux=bulk_flux
            bulk_helmrelease=bulk_helmrelease
            bulk_has_source_ref=bulk_has_source_ref />
    }
}
