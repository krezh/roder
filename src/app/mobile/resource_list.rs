//! Mobile replacement for `KindTable`: a card list instead of a dense grid,
//! with an explicit "Select" mode for multi-select (long-press instead opens
//! the action sheet — see `row_card.rs`).

use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::events::fire_action;
use crate::app::hooks::{use_sse_subscription, use_table_state};
use crate::app::mobile::bulk_bar::MobileBulkBar;
use crate::app::mobile::row_card::{use_select_mode, CardFields, MobileRowCard};
use crate::app::overlays::toast::Toast;
use crate::app::state::{
    open_logs, CtxMenu, DetailTarget, LogPods, LogTarget, OnlyProblems, SortKey, TableRows,
    TableSelected, Tick,
};
use crate::app::table_logic;
use crate::app::util::predicate::KindKind;
use crate::app::views::dashboard::Dashboard;
use crate::data;

#[component]
pub(crate) fn MobileResourceView() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();

    view! {
        {move || match selected_kind.get() {
            None => view! { <Dashboard /> }.into_any(),
            Some(kind) => {
                let storage_key = format!("roder.filter.{}", kind.key);
                let initial = data::storage_get(&storage_key).unwrap_or_default();
                let text_filter = RwSignal::new(initial);
                Effect::new(move |_| {
                    let val = text_filter.get();
                    if val.is_empty() {
                        data::storage_remove(&storage_key);
                    } else {
                        data::storage_set(&storage_key, &val);
                    }
                });
                let k = kind.clone();
                view! {
                    <MobileKindList
                        kind=kind
                        url_fn=move || {
                            let ns = if k.namespaced { selected_ns.get() } else { None };
                            Some(data::watch_url(&k.key, ns.as_deref(), None))
                        }
                        text_filter=text_filter />
                }.into_any()
            }
        }}
    }
}

#[component]
fn MobileKindList(
    kind: ResourceKind,
    url_fn: impl Fn() -> Option<String> + 'static,
    text_filter: RwSignal<String>,
) -> impl IntoView {
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let ctx_menu = expect_context::<RwSignal<Option<CtxMenu>>>();
    let only_problems = expect_context::<OnlyProblems>().0;
    let log_pods = expect_context::<LogPods>().0;
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let tick = expect_context::<Tick>().0;

    let t = use_table_state();

    // This is the primary (only) list mounted for the mobile resource route,
    // so it registers for the action sheet the same way `KindTable` does.
    let sv_sel = expect_context::<TableSelected>().0;
    let sv_rows = expect_context::<TableRows>().0;
    sv_sel.set_value(Some(t.selected));
    sv_rows.set_value(Some(t.rows));
    on_cleanup(move || {
        sv_sel.set_value(None);
        sv_rows.set_value(None);
    });

    let is_events = kind.group.is_empty() && kind.kind == "Event";
    t.sort.set(if is_events {
        (SortKey::Age, false)
    } else {
        (SortKey::Namespace, true)
    });

    let columns: RwSignal<Vec<String>> = RwSignal::new(kind.columns.clone());
    use_sse_subscription(t.rows, t.entering, t.removing, Some(columns), move || {
        t.rows.set(Default::default());
        t.selected.set(Default::default());
        t.last_clicked.set(None);
        t.entering.set(Default::default());
        t.removing.set(Default::default());
        t.scroll_top.set(0.0);
        url_fn()
    });

    let shown_uids = Memo::new(move |_| {
        let problems = only_problems.get();
        let filter_text = text_filter.get().to_lowercase();
        let (sort_key, asc) = t.sort.get();
        t.rows
            .with(|m| table_logic::shown_uids(m.values(), sort_key, asc, problems, &filter_text))
    });

    let is_pod_kind = kind.group.is_empty() && kind.kind == "Pod";
    let node_col = Memo::new(move |_| columns.get().iter().position(|c| c == "Node"));
    let kk = KindKind::new(&kind.group, &kind.kind);
    let bulk_workload = kk.is_workload();
    let bulk_flux = kk.is_flux();
    let bulk_helmrelease = kk.is_helmrelease();
    let bulk_has_source_ref = kk.has_source_ref();
    let key_sv = StoredValue::new(kind.key.clone());
    let title_sv = StoredValue::new(kind.kind.clone());

    let rows = t.rows;
    let selected = t.selected;
    let select_mode = use_select_mode(selected);

    let do_bulk = move |action: &'static str| {
        let key = key_sv.get_value();
        let uids = selected.get_untracked();
        let targets = rows.with_untracked(|v| table_logic::bulk_targets(&key, v, &uids));
        fire_action(toast, action, &targets);
        select_mode.set(false);
    };
    let on_logs = Callback::new(move |_| {
        let uids = selected.get_untracked();
        let key = key_sv.get_value();
        let agg = !is_pod_kind;
        rows.with_untracked(|v| {
            for r in v.values().filter(|r| uids.contains(&r.uid)) {
                open_logs(log_pods, LogTarget::from_row(&key, r, agg));
            }
        });
        select_mode.set(false);
    });

    view! {
        <div class="mobile-list">
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
                        let node_for_ctx = move || {
                            if is_pod_kind {
                                node_col.get_untracked().and_then(|i| {
                                    row.get_untracked().and_then(|r| r.cells.get(i).cloned())
                                })
                            } else {
                                None
                            }
                        };
                        let fields = Memo::new(move |_| {
                            let r = row.get()?;
                            let cols = columns.get();
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
                                node_for_ctx=node_for_ctx
                                fields=fields />
                        }
                    }
                </For>
                {move || shown_uids.with(|v| v.is_empty()).then(|| {
                    let msg = if only_problems.get() {
                        format!("No {} with problems", title_sv.get_value())
                    } else {
                        format!("No {} found", title_sv.get_value())
                    };
                    view! { <div class="empty pad">{msg}</div> }
                })}
            </div>
            <MobileBulkBar
                selected=selected
                select_mode=select_mode
                all_uids=move || shown_uids.get()
                do_bulk=do_bulk
                on_logs=on_logs
                bulk_workload=bulk_workload
                bulk_flux=bulk_flux
                bulk_helmrelease=bulk_helmrelease
                bulk_has_source_ref=bulk_has_source_ref />
        </div>
    }
}
