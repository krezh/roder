//! Mobile's bottom action sheet: the touch equivalent of the desktop's
//! cursor-anchored right-click context menu (`overlays/context_menu.rs`).
//! Triggered by the overflow button on a `MobileRowCard`.

use leptos::prelude::*;
use roder_core::{ResourceKind, RowStatus};

use crate::app::events::{fire_action, fire_action_with};
use crate::app::state::{
    open_logs, Catalog, CtxMenu, DetailTarget, ExecOpen, ExecTarget, LogPods, LogTarget, TableRows,
    TableSelected, TableTargets,
};
use crate::app::table_logic::{resolve_action_targets, targets_all};
use crate::app::ui::{
    ask_confirm, ask_delete, delete_extra, show_toast, use_option_overlay, Confirm, DeleteRequest,
    Toast, ToastKind,
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
    let delete_confirm = expect_context::<RwSignal<Option<DeleteRequest>>>();
    let catalog = expect_context::<Catalog>().0;
    let log_pods = expect_context::<LogPods>().0;
    let exec_open = expect_context::<ExecOpen>().0;
    let table_selected = expect_context::<TableSelected>().0;
    let table_rows = expect_context::<TableRows>().0;
    let table_targets = expect_context::<TableTargets>().0;
    let toast = expect_context::<RwSignal<Option<Toast>>>();

    let (snapshot, closing, do_close) = use_option_overlay(ctx);

    view! {
        {move || snapshot.get().map(|m| {
            let rows_opt = table_rows.get_value();
            let selected = table_selected.get_value().map(|signal| signal.get_untracked());
            let empty_rows = Default::default();
            let empty_targets = Default::default();
            let resolved = match (rows_opt, table_targets.get_value()) {
                (Some(rows), Some(row_targets)) => rows.with_untracked(|rows| {
                    row_targets.with_untracked(|row_targets| {
                        resolve_action_targets(
                            &m.uid,
                            &m.target,
                            selected.as_ref(),
                            rows,
                            row_targets,
                        )
                    })
                }),
                (Some(rows), None) => rows.with_untracked(|rows| {
                    resolve_action_targets(
                        &m.uid,
                        &m.target,
                        selected.as_ref(),
                        rows,
                        &empty_targets,
                    )
                }),
                (None, Some(row_targets)) => row_targets.with_untracked(|row_targets| {
                    resolve_action_targets(
                        &m.uid,
                        &m.target,
                        selected.as_ref(),
                        &empty_rows,
                        row_targets,
                    )
                }),
                (None, None) => resolve_action_targets(
                    &m.uid,
                    &m.target,
                    selected.as_ref(),
                    &empty_rows,
                    &empty_targets,
                ),
            };
            let target_uids = resolved.uids;
            let targets = resolved.targets;
            let is_bulk = targets.len() > 1;
            let is_pod = targets_all(&targets, |kind| kind.is_pod());
            let is_workload = targets_all(&targets, |kind| kind.is_workload());
            let is_scalable = targets_all(&targets, |kind| kind.is_scalable());
            let is_flux = targets_all(&targets, |kind| kind.is_flux());
            let is_helmrelease = targets_all(&targets, |kind| kind.is_helmrelease());
            let has_source_ref = targets_all(&targets, |kind| kind.has_source_ref());
            let is_eso = targets_all(&targets, |kind| kind.is_eso());
            let is_certificate = targets_all(&targets, |kind| kind.is_certificate());
            let is_cronjob = targets_all(&targets, |kind| kind.is_cronjob());
            let is_job = targets_all(&targets, |kind| kind.is_job());
            let is_kopiur_snapshot_policy = targets_all(&targets, |kind| kind.is_kopiur_snapshot_policy());
            let suspend_state: Option<bool> = rows_opt.and_then(|rows| {
                rows.with_untracked(|rm| {
                    let mut states = target_uids.iter().filter_map(|uid| rm.get(uid)).map(|r| r.suspended);
                    let first = states.next()?;
                    states.all(|s| s == first).then_some(first)
                })
            });
            let show_suspend = suspend_state != Some(true);
            let show_resume = suspend_state != Some(false);
            let jobs_terminal = is_job && rows_opt.is_some_and(|rows| {
                rows.with_untracked(|rows| {
                    target_uids.iter().all(|uid| {
                        rows.get(uid).is_some_and(|row| {
                            matches!(row.status, RowStatus::Ok | RowStatus::Error)
                        })
                    })
                })
            });

            let open = { let t = m.target.clone(); move |_| { detail.set(Some(t.clone())); do_close(); } };
            let has_logs = targets_all(&targets, |kind| {
                kind.is_pod() || kind.is_workload() || kind.is_job()
            });
            let logs = {
                let ts = targets.clone();
                move |_| {
                    for t in &ts {
                        let (group, kind) = parse_key(&t.key);
                        let aggregate = !KindKind::new(&group, &kind).is_pod();
                        open_logs(log_pods, LogTarget::from_detail(t, aggregate));
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
            let rerun     = bulk_act!("job-rerun");
            let snapshot_now = bulk_act!("kopiur-snapshot-now");
            let renew_certificate = {
                let ts = targets.clone();
                move |_| {
                    let ts = ts.clone();
                    let n = ts.len();
                    let label = if n == 1 {
                        "Force renewal of this Certificate?".to_string()
                    } else {
                        format!("Force renewal of {n} Certificates?")
                    };
                    ask_confirm(confirm, label, "Renew", move || {
                        fire_action(toast, "certificate-renew", &ts);
                        if let Some(sel) = table_selected.get_value() {
                            sel.set(Default::default());
                        }
                    });
                    do_close();
                }
            };
            let delete = {
                let ts = targets.clone();
                move |_| {
                    let ts = ts.clone();
                    let n = ts.len();
                    let label = if n == 1 { "Delete this resource?".to_string() }
                                else { format!("Delete {n} resources?") };
                    ask_delete(delete_confirm, label, move |force, propagation| {
                        fire_action_with(toast, "delete", &ts, delete_extra(force, propagation));
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
                        image: String::new(),
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
                    {jobs_terminal.then(|| view! { <button class="sheet-item" on:click=rerun>"Re-run"</button> })}
                    {is_kopiur_snapshot_policy.then(|| view! { <button class="sheet-item" on:click=snapshot_now>"Snapshot Now"</button> })}
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
                    {is_certificate.then(|| view! { <button class="sheet-item" on:click=renew_certificate>"Force renew"</button> })}
                    <button class="sheet-item danger" on:click=delete>"Delete"</button>
                </div>
            }
        })}
    }
}
