//! Mobile replacement for `KindTable`: a card list instead of a dense grid,
//! with hold-to-select multi-select and an explicit per-row action button.

use leptos::prelude::*;
use roder_core::{ResourceKind, RowStatus};

use crate::app::components::topbar::{SanitizeButton, SyncButton};
use crate::app::events::{make_bulk_open_logs, make_do_bulk, make_do_delete};
use crate::app::hooks::{use_sse_subscription, use_table_state};
use crate::app::mobile::bulk_bar::MobileBulkBar;
use crate::app::mobile::row_card::{use_select_mode, CardFields, MobileRowCard};
use crate::app::overlays::toast::Toast;
use crate::app::state::{
    CtxMenu, DetailTarget, LogPods, NavigationRestored, OnlyProblems, SortKey, TableRows,
    TableSelected, Tick,
};
use crate::app::table_logic;
use crate::app::util::predicate::KindKind;
use crate::app::views::dashboard::Dashboard;
use crate::data;

#[component]
pub(crate) fn MobileListActions() -> impl IntoView {
    let only_problems = expect_context::<OnlyProblems>().0;

    view! {
        <div class="mobile-list-actions" aria-label="Resource actions">
            <button type="button" class="mobile-problems-toggle"
                class:active=move || only_problems.get()
                aria-pressed=move || only_problems.get().to_string()
                on:click=move |_| only_problems.update(|active| *active = !*active)>
                "Problems"
            </button>
            <SanitizeButton />
            <SyncButton />
        </div>
    }
}

#[component]
pub(crate) fn MobileResourceView() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let restored = expect_context::<NavigationRestored>().0;

    view! {
        {move || if !restored.get() {
            view! { <div aria-label="Restoring view"></div> }.into_any()
        } else { match selected_kind.get() {
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
        }}}
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

    let columns: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
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
        t.rows.with(|m| {
            table_logic::shown_uids(m.values(), sort_key, asc, problems, None, &filter_text)
        })
    });

    let is_pod_kind = kind.group.is_empty() && kind.kind == "Pod";
    let node_col = Memo::new(move |_| columns.get().iter().position(|c| c == "Node"));
    let kk = KindKind::new(&kind.group, &kind.kind);
    let bulk_workload = kk.is_workload();
    let bulk_job = kk.is_job();
    let bulk_flux = kk.is_flux();
    let bulk_helmrelease = kk.is_helmrelease();
    let bulk_has_source_ref = kk.has_source_ref();
    let bulk_certificate = kk.is_certificate();
    let key_sv = StoredValue::new(kind.key.clone());
    let title_sv = StoredValue::new(kind.kind.clone());

    let rows = t.rows;
    let selected = t.selected;
    let can_rerun_jobs = Signal::derive(move || {
        let selected = selected.get();
        !selected.is_empty()
            && rows.with(|rows| {
                selected.iter().all(|uid| {
                    rows.get(uid)
                        .is_some_and(|row| matches!(row.status, RowStatus::Ok | RowStatus::Error))
                })
            })
    });
    let select_mode = use_select_mode(selected);

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
        <div class="mobile-list">
            <div class="mobile-list-head">
                <label class="mobile-filter-wrap">
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                        <circle cx="11" cy="11" r="6.5" />
                        <path d="m16 16 4 4" />
                    </svg>
                    <input class="mobile-filter" placeholder="Filter resources"
                        aria-label="Filter resources"
                        prop:value=move || text_filter.get()
                        on:input=move |e| text_filter.set(event_target_value(&e)) />
                    {move || (!text_filter.get().is_empty()).then(|| view! {
                        <button type="button" aria-label="Clear filter"
                            on:click=move |_| text_filter.set(String::new())>"×"</button>
                    })}
                </label>
                <MobileListActions />
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
                            Some(CardFields::from_row(&r, &cols, None))
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
                do_delete=do_delete
                on_logs=on_logs
                bulk_workload=bulk_workload
                bulk_job=bulk_job
                can_rerun_jobs=can_rerun_jobs
                bulk_flux=bulk_flux
                bulk_helmrelease=bulk_helmrelease
                bulk_has_source_ref=bulk_has_source_ref
                bulk_certificate=bulk_certificate />
        </div>
    }
}
