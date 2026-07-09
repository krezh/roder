//! Mobile's bottom action sheet — the touch equivalent of the desktop's
//! cursor-anchored right-click context menu (`overlays/context_menu.rs`).
//! Triggered by long-press on a `MobileRowCard` instead of `oncontextmenu`.
//!
//! The bulk-target-resolution and action business logic mirrors
//! `context_menu.rs` line for line — it's duplicated rather than extracted
//! because the two are too view-entangled to share cleanly, and the desktop
//! file must stay untouched (see the mobile UI plan's Phase 3 notes).

use leptos::prelude::*;
use roder_core::{ResourceKind, RowStatus};

use crate::app::events::{fire_action, fire_action_with};
use crate::app::overlays::confirm::{ask_confirm, Confirm};
use crate::app::overlays::toast::{show_toast, Toast, ToastKind};
use crate::app::state::{
    open_logs, Catalog, CtxMenu, DetailTarget, ExecOpen, ExecTarget, LogPods, LogTarget, TableRows,
    TableSelected,
};
use crate::app::util::clipboard::copy_to_clipboard;
use crate::app::util::format::parse_key;
use crate::app::util::predicate::KindKind;

#[component]
pub(crate) fn MobileActionSheet() -> impl IntoView {
    let ctx = expect_context::<RwSignal<Option<CtxMenu>>>();
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let catalog = expect_context::<Catalog>().0;
    let log_pods = expect_context::<LogPods>().0;
    let exec_open = expect_context::<ExecOpen>().0;
    let table_selected = expect_context::<TableSelected>().0;
    let table_rows = expect_context::<TableRows>().0;
    let toast = expect_context::<RwSignal<Option<Toast>>>();

    let (snapshot, closing, do_close) = crate::app::overlays::use_option_overlay(ctx);

    view! {
        {move || snapshot.get().map(|m| {
            let (group, kind) = parse_key(&m.target.key);
            let kk = KindKind::new(&group, &kind);
            let is_pod = kk.is_pod();
            let is_workload = kk.is_workload();
            let is_scalable = kk.is_scalable();
            let is_flux = kk.is_flux();
            let is_helmrelease = kk.is_helmrelease();
            let has_source_ref = kk.has_source_ref();
            let is_eso = kk.is_eso();
            let is_cronjob = kk.is_cronjob();

            let rows_opt = table_rows.get_value();
            let target_uids: Vec<String> = match (table_selected.get_value(), rows_opt) {
                (Some(sel), Some(_)) => {
                    let uids = sel.get_untracked();
                    if uids.len() > 1 && uids.contains(&m.uid) {
                        uids.into_iter().collect()
                    } else {
                        vec![m.uid.clone()]
                    }
                }
                _ => vec![m.uid.clone()],
            };
            let targets: Vec<DetailTarget> = match rows_opt {
                Some(rows) => {
                    let ts: Vec<DetailTarget> = rows.with_untracked(|rm| {
                        target_uids.iter()
                            .filter_map(|uid| rm.get(uid).map(|r| DetailTarget {
                                key: m.target.key.clone(),
                                namespace: r.namespace.clone(),
                                name: r.name.clone(),
                            }))
                            .collect()
                    });
                    if ts.is_empty() { vec![m.target.clone()] } else { ts }
                }
                None => vec![m.target.clone()],
            };
            let is_bulk = targets.len() > 1;
            let suspend_state: Option<bool> = rows_opt.and_then(|rows| {
                rows.with_untracked(|rm| {
                    let mut states = target_uids.iter().filter_map(|uid| rm.get(uid)).map(|r| r.status == RowStatus::Warn);
                    let first = states.next()?;
                    states.all(|s| s == first).then_some(first)
                })
            });
            let show_suspend = suspend_state != Some(true);
            let show_resume = suspend_state != Some(false);

            let open = { let t = m.target.clone(); move |_| { detail.set(Some(t.clone())); do_close(); } };
            let has_logs = is_pod || is_workload || kk.is_job();
            let logs = {
                let ts = targets.clone();
                let agg = !is_pod;
                move |_| {
                    for t in &ts {
                        open_logs(log_pods, LogTarget::from_detail(t, agg));
                    }
                    if let Some(sel) = table_selected.get_value() { sel.set(Default::default()); }
                    do_close();
                }
            };
            let goto_ns = { let ns = m.target.namespace.clone(); move |_| { selected_ns.set(ns.clone()); do_close(); } };
            let goto_node = {
                let node = m.node.clone();
                move |_| {
                    if let Some(node) = node.clone() {
                        if let Some(nk) = catalog.get_untracked().into_iter().find(|k| k.kind == "Node" && k.group.is_empty()) {
                            let key = nk.key.clone();
                            selected_kind.set(Some(nk));
                            selected_ns.set(None);
                            detail.set(Some(DetailTarget { key, namespace: None, name: node }));
                        }
                    }
                    do_close();
                }
            };
            let copy = {
                let names: Vec<String> = targets.iter().map(|t| t.name.clone()).collect();
                move |_| {
                    copy_to_clipboard(&names.join("\n"));
                    show_toast(toast, "Copied to clipboard", ToastKind::Ok);
                    do_close();
                }
            };
            macro_rules! bulk_act {
                ($action:literal) => {{
                    let ts = targets.clone();
                    move |_| {
                        fire_action(toast, $action, &ts);
                        if let Some(sel) = table_selected.get_value() { sel.set(Default::default()); }
                        do_close();
                    }
                }};
            }
            let restart   = bulk_act!("restart");
            let reconcile = {
                let ts = targets.clone();
                move |_| {
                    fire_action_with(toast, "flux-reconcile", &ts, serde_json::json!({}));
                    if let Some(sel) = table_selected.get_value() { sel.set(Default::default()); }
                    do_close();
                }
            };
            let reconcile_with_source = {
                let ts = targets.clone();
                move |_| {
                    fire_action_with(toast, "flux-reconcile-with-source", &ts, serde_json::json!({}));
                    if let Some(sel) = table_selected.get_value() { sel.set(Default::default()); }
                    do_close();
                }
            };
            let force     = bulk_act!("flux-force");
            let reset     = bulk_act!("flux-reset");
            let suspend   = bulk_act!("flux-suspend");
            let resume    = bulk_act!("flux-resume");
            let refresh   = bulk_act!("eso-refresh");
            let trigger   = bulk_act!("cronjob-trigger");
            let delete = {
                let ts = targets.clone();
                move |_| {
                    let ts = ts.clone();
                    let n = ts.len();
                    let label = if n == 1 { "Delete this resource?".to_string() }
                                else { format!("Delete {n} resources?") };
                    ask_confirm(confirm, label, move || {
                        fire_action(toast, "delete", &ts);
                        if let Some(sel) = table_selected.get_value() { sel.set(Default::default()); }
                    });
                    do_close();
                }
            };

            let scale_n = RwSignal::new(1i32);
            let shell = (!is_bulk && is_pod).then(|| {
                let ns  = m.target.namespace.clone().unwrap_or_default();
                let pod = m.target.name.clone();
                move |_| {
                    exec_open.set(Some(ExecTarget {
                        namespace: ns.clone(),
                        pod: pod.clone(),
                        container: None,
                        pending: false,
                        node_shell: false,
                    }));
                    do_close();
                }
            });

            let ns_item = (!is_bulk).then(|| m.target.namespace.clone()).flatten();
            let node_item = (!is_bulk && is_pod).then(|| m.node.clone()).flatten();

            view! {
                <div class="sheet-scrim" class:closing=move || closing.get() on:click=move |_| do_close()></div>
                <div class="action-sheet" class:closing=move || closing.get()>
                    <div class="sheet-handle"></div>
                    {is_bulk.then(|| view! {
                        <div class="sheet-item sheet-header">{targets.len()}" resources"</div>
                    })}
                    {(!is_bulk).then(|| view! { <button class="sheet-item" on:click=open>"Open details"</button> })}
                    {has_logs.then(|| view! { <button class="sheet-item" on:click=logs>"Logs"</button> })}
                    {shell.map(|s| view! { <button class="sheet-item" on:click=s>"Shell"</button> })}
                    {ns_item.map(|ns| view! { <button class="sheet-item" on:click=goto_ns>"Go to namespace "<span class="sheet-sub">{ns}</span></button> })}
                    {node_item.map(|node| view! { <button class="sheet-item" on:click=goto_node>"Go to node "<span class="sheet-sub">{node}</span></button> })}
                    <button class="sheet-item" on:click=copy>"Copy name"</button>
                    {is_workload.then(|| view! { <button class="sheet-item" on:click=restart>"Restart"</button> })}
                    {(!is_bulk && is_scalable).then(|| {
                        let t = m.target.clone();
                        view! {
                            <div class="sheet-item sheet-scale">
                                <span>"Scale"</span>
                                <input type="number" min="0" class="sheet-scale-input"
                                    prop:value=move || scale_n.get().to_string()
                                    on:click=|e: leptos::ev::MouseEvent| e.stop_propagation()
                                    on:input=move |e| {
                                        if let Ok(n) = event_target_value(&e).parse::<i32>() {
                                            scale_n.set(n);
                                        }
                                    } />
                                <button on:click=move |_| {
                                    fire_action_with(toast, "scale", std::slice::from_ref(&t), serde_json::json!({ "replicas": scale_n.get_untracked() }));
                                    do_close();
                                }>"→"</button>
                            </div>
                        }
                    })}
                    {is_cronjob.then(|| view! { <button class="sheet-item" on:click=trigger>"Trigger"</button> })}
                    {is_flux.then(|| view! {
                        <button class="sheet-item" on:click=reconcile>"Reconcile"</button>
                        {has_source_ref.then(|| view! {
                            <button class="sheet-item" on:click=reconcile_with_source>"Reconcile w/ source"</button>
                        })}
                        {is_helmrelease.then(|| view! {
                            <button class="sheet-item" on:click=force>"Force"</button>
                            <button class="sheet-item" on:click=reset>"Reset"</button>
                        })}
                        {show_suspend.then(|| view! { <button class="sheet-item" on:click=suspend>"Suspend"</button> })}
                        {show_resume.then(|| view! { <button class="sheet-item" on:click=resume>"Resume"</button> })}
                    })}
                    {is_eso.then(|| view! { <button class="sheet-item" on:click=refresh>"Refresh"</button> })}
                    <button class="sheet-item danger" on:click=delete>"Delete"</button>
                </div>
            }
        })}
    }
}
