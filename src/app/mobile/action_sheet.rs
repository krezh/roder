//! Mobile's bottom action sheet: the touch equivalent of the desktop's
//! cursor-anchored right-click context menu (`overlays/context_menu.rs`).
//! Triggered by the overflow button on a `MobileRowCard`.

use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::controllers::detail::fetch_selection_permissions;
use crate::app::events::{fire_action, fire_action_with};
use crate::app::resource_actions::{ActionSurface, ResourceActionModel, ResourceMenuAction};
use crate::app::state::{
    open_logs, Catalog, CtxMenu, DetailTarget, ExecOpen, ExecTarget, FileBrowserOpen, LogPods,
    LogTarget, TableRows, TableSelected, TableTargets, TreeOpen,
};
use crate::app::table_logic::resolve_current_action_targets;
use crate::app::ui::confirm::{ask_confirm, Confirm};
use crate::app::ui::delete::{ask_delete, delete_extra, DeleteRequest};
use crate::app::ui::toast::{show_toast, Toast, ToastKind};
use crate::app::ui::use_option_overlay;
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
    let file_browser_open = expect_context::<FileBrowserOpen>().0;
    let table_selected = expect_context::<TableSelected>().0;
    let table_rows = expect_context::<TableRows>().0;
    let table_targets = expect_context::<TableTargets>().0;
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let tree_open = expect_context::<TreeOpen>().0;

    let (snapshot, closing, do_close) = use_option_overlay(ctx);
    let action_permissions = LocalResource::new(move || {
        let targets = snapshot
            .get()
            .map(|menu| {
                resolve_current_action_targets(&menu, table_selected, table_rows, table_targets)
                    .targets
            })
            .unwrap_or_default();
        async move { fetch_selection_permissions(targets).await }
    });

    view! {
        {move || snapshot.get().map(|m| {
            let rows_opt = table_rows.get_value();
            let resolved = resolve_current_action_targets(&m, table_selected, table_rows, table_targets);
            let target_uids = resolved.uids;
            let targets = resolved.targets;
            let is_bulk = targets.len() > 1;
            let permitted = move |action| {
                action_permissions
                    .get()
                    .is_some_and(|permissions| permissions.allows_all(action))
            };
            let rows_snapshot = rows_opt.map(|rows| rows.get_untracked());
            let actions = ResourceActionModel::for_selection(
                ActionSurface::Mobile,
                &targets,
                &target_uids,
                rows_snapshot.as_ref(),
                m.node.as_deref(),
                false,
                permitted,
            );

            let open = { let t = m.target.clone(); move |_| { detail.set(Some(t.clone())); do_close(); } };
            let open_tree = { let t = m.target.clone(); move |_| { tree_open.set(Some(t.clone())); do_close(); } };
            let has_logs = actions.supports(ResourceMenuAction::Logs);
            let logs = {
                let ts = targets.clone();
                move |_| {
                    for t in &ts {
                        let (group, version, kind) = parse_key(&t.key);
                        let aggregate = !KindKind::new(&group, &version, &kind).is_pod();
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
            let cnpg_suspend = bulk_act!("cnpg-suspend");
            let cnpg_resume = bulk_act!("cnpg-resume");
            let create_cnpg_backup = {
                let ts = targets.clone();
                move |_| {
                    let ts = ts.clone();
                    let n = ts.len();
                    let label = if n == 1 {
                        "Create an immediate Backup for this Cluster? Completion is reported separately.".to_string()
                    } else {
                        format!("Create immediate Backups for {n} Clusters? Completion is reported separately.")
                    };
                    ask_confirm(confirm, label, "Create backup", move || {
                        fire_action(toast, "cnpg-backup", &ts);
                        if let Some(sel) = table_selected.get_value() {
                            sel.set(Default::default());
                        }
                    });
                    do_close();
                }
            };
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
            let shell = actions.supports(ResourceMenuAction::Shell).then(|| {
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
            let files = actions.supports(ResourceMenuAction::BrowseFiles).then(|| {
                let target = m.target.clone();
                move |_| {
                    file_browser_open.set(Some(target.clone()));
                    do_close();
                }
            });

            let ns_item = actions.supports(ResourceMenuAction::GoToNamespace).then(|| m.target.namespace.clone()).flatten();
            let node_item = actions.supports(ResourceMenuAction::GoToNode).then(|| m.node.clone()).flatten();
            let has_flux = actions.supports_any(&[
                ResourceMenuAction::FluxReconcile,
                ResourceMenuAction::FluxReconcileWithSource,
                ResourceMenuAction::FluxForce,
                ResourceMenuAction::FluxReset,
                ResourceMenuAction::FluxSuspend,
                ResourceMenuAction::FluxResume,
            ]);

            view! {
                <div class="sheet-scrim" class:closing=move || closing.get() on:click=move |_| do_close()></div>
                <div class="action-sheet" class:closing=move || closing.get()>
                    <div class="sheet-handle"></div>
                    {is_bulk.then(|| view! {
                        <div class="sheet-item sheet-header">{targets.len()}" resources"</div>
                    })}
                    {actions.supports(ResourceMenuAction::OpenDetails).then(|| view! { <button class="sheet-item" on:click=open>"Open details"</button> })}
                    {actions.supports(ResourceMenuAction::Relationships).then(|| view! { <button class="sheet-item" on:click=open_tree>"Relationships"</button> })}
                    {has_logs.then(|| view! { <button class="sheet-item" on:click=logs>"Logs"</button> })}
                    {shell.map(|s| view! { <button class="sheet-item" on:click=s>"Shell"</button> })}
                    {files.map(|open| view! { <button class="sheet-item" on:click=open>"Files"</button> })}
                    {ns_item.map(|ns| view! { <button class="sheet-item" on:click=goto_ns>"Go to namespace "<span class="sheet-sub">{ns}</span></button> })}
                    {node_item.map(|node| view! { <button class="sheet-item" on:click=goto_node>"Go to node "<span class="sheet-sub">{node}</span></button> })}
                    {actions.supports(ResourceMenuAction::CopyNames).then(|| view! { <button class="sheet-item" on:click=copy>"Copy name"</button> })}
                    {actions.supports(ResourceMenuAction::Restart).then(|| view! { <button class="sheet-item" on:click=restart>"Restart"</button> })}
                    {actions.supports(ResourceMenuAction::Scale).then(|| {
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
                    {actions.supports(ResourceMenuAction::CronJobTrigger).then(|| view! { <button class="sheet-item" on:click=trigger>"Trigger"</button> })}
                    {actions.supports(ResourceMenuAction::JobRerun).then(|| view! { <button class="sheet-item" on:click=rerun>"Re-run"</button> })}
                    {actions.supports(ResourceMenuAction::KopiurSnapshotNow).then(|| view! { <button class="sheet-item" on:click=snapshot_now>"Snapshot Now"</button> })}
                    {actions.supports(ResourceMenuAction::CnpgBackup).then(|| view! { <button class="sheet-item" on:click=create_cnpg_backup>"Create backup..."</button> })}
                    {actions.supports(ResourceMenuAction::CnpgSuspend).then(|| view! { <button class="sheet-item" on:click=cnpg_suspend>"Suspend schedule"</button> })}
                    {actions.supports(ResourceMenuAction::CnpgResume).then(|| view! { <button class="sheet-item" on:click=cnpg_resume>"Resume schedule"</button> })}
                    {has_flux.then(|| view! {
                        {actions.supports(ResourceMenuAction::FluxReconcile).then(|| view! {
                            <button class="sheet-item" on:click=reconcile>"Reconcile"</button>
                            {actions.supports(ResourceMenuAction::FluxReconcileWithSource).then(|| view! {
                                <button class="sheet-item" on:click=reconcile_with_source>"Reconcile w/ source"</button>
                            })}
                            {actions.supports(ResourceMenuAction::FluxForce).then(|| view! {
                                <button class="sheet-item" on:click=force>"Force"</button>
                            })}
                            {actions.supports(ResourceMenuAction::FluxReset).then(|| view! {
                                <button class="sheet-item" on:click=reset>"Reset"</button>
                            })}
                        })}
                        {actions.supports(ResourceMenuAction::FluxSuspend).then(|| view! { <button class="sheet-item" on:click=suspend>"Suspend"</button> })}
                        {actions.supports(ResourceMenuAction::FluxResume).then(|| view! { <button class="sheet-item" on:click=resume>"Resume"</button> })}
                    })}
                    {actions.supports(ResourceMenuAction::ExternalSecretsRefresh).then(|| view! { <button class="sheet-item" on:click=refresh>"Refresh"</button> })}
                    {actions.supports(ResourceMenuAction::CertificateRenew).then(|| view! { <button class="sheet-item" on:click=renew_certificate>"Force renew"</button> })}
                    {actions.supports(ResourceMenuAction::Delete).then(|| view! { <button class="sheet-item danger" on:click=delete>"Delete"</button> })}
                </div>
            }
        })}
    }
}
