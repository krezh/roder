//! Right-click context menu with resource-type-specific actions.

use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::controllers::detail::fetch_selection_permissions;
use crate::app::events::{fire_action, fire_action_with};
use crate::app::resource_actions::{ActionSurface, ResourceActionModel, ResourceMenuAction};
use crate::app::state::{
    open_logs, Catalog, CtxMenu, DebugImage, DetailTarget, DrainOpen, DrainTarget, ExecOpen,
    ExecTarget, FileBrowserOpen, LogPods, LogTarget, TableRows, TableSelected, TableTargets,
    TalosFeatures, TreeOpen,
};
use crate::app::table_logic::resolve_current_action_targets;
use crate::app::ui::confirm::{ask_confirm, Confirm};
use crate::app::ui::delete::{ask_delete, delete_extra, DeleteRequest};
use crate::app::ui::toast::{show_toast, show_toast_detail, ToastKind, Toasts};
use crate::app::util::clipboard::copy_to_clipboard;
use crate::app::util::format::parse_key;
use crate::app::util::predicate::KindKind;

#[component]
pub(crate) fn ContextMenu() -> impl IntoView {
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
    let debug_image = expect_context::<DebugImage>().0;
    let tree_open = expect_context::<TreeOpen>().0;
    let drain_open = expect_context::<DrainOpen>().0;
    let talos_features = expect_context::<TalosFeatures>().0;
    // Provided at App level; ResourceView fills in the Option on mount.
    let table_selected = expect_context::<TableSelected>().0;
    let table_rows = expect_context::<TableRows>().0;
    let table_targets = expect_context::<TableTargets>().0;
    let toast = expect_context::<Toasts>();

    let (snapshot, closing, do_close) = crate::app::ui::use_option_overlay(ctx);
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
    let pos = RwSignal::new((0i32, 0i32));
    let menu_ref = NodeRef::<leptos::html::Div>::new();

    // Keyboard nav for the menu. The item list is built from ~40 kind-specific
    // conditionals below, so rather than mirror that into an index, this walks
    // the rendered buttons and uses native focus — `Enter` then
    // activates the focused button with no extra wiring, and the same code
    // works whichever items a given resource kind ends up showing.
    #[cfg(target_arch = "wasm32")]
    {
        let step_focus = move |delta: i32| {
            use wasm_bindgen::JsCast;
            let Some(menu) = menu_ref.get_untracked() else {
                return;
            };
            let Ok(items) = menu.query_selector_all("button") else {
                return;
            };
            let n = items.length() as i32;
            if n == 0 {
                return;
            }
            let active = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.active_element());
            let current = (0..n).find(|i| match (items.item(*i as u32), active.as_ref()) {
                (Some(item), Some(a)) => &item == a.as_ref(),
                _ => false,
            });
            // Nothing focused yet: `j` enters at the top, `k` at the bottom.
            let next = match current {
                Some(i) => (i + delta).rem_euclid(n),
                None if delta >= 0 => 0,
                None => n - 1,
            };
            if let Some(el) = items
                .item(next as u32)
                .and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
            {
                let _ = el.focus();
            }
        };
        Effect::new(move |_| {
            let handle = window_event_listener(leptos::ev::keydown, move |e| {
                if ctx.with_untracked(Option::is_none) {
                    return;
                }
                match e.key().as_str() {
                    "j" | "ArrowDown" => {
                        e.prevent_default();
                        step_focus(1);
                    }
                    "k" | "ArrowUp" => {
                        e.prevent_default();
                        step_focus(-1);
                    }
                    _ => {}
                }
            });
            on_cleanup(move || handle.remove());
        });
    }

    Effect::new(move |_| {
        if let Some(menu) = snapshot.get() {
            let _ = action_permissions.get();
            pos.set((menu.x, menu.y));
        }
    });

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        let Some(menu) = snapshot.get() else {
            return;
        };
        let Some(el) = menu_ref.get() else {
            return;
        };

        // A keyboard-opened menu responds to Enter immediately. Pointer-opened
        // menus stay unfocused until the user starts keyboard navigation.
        if menu.focus_first {
            use wasm_bindgen::JsCast;
            if let Ok(Some(first)) = el.query_selector("button") {
                if let Ok(first) = first.dyn_into::<web_sys::HtmlElement>() {
                    let _ = first.focus();
                }
            }
        }

        // Measure the untransformed layout box; getBoundingClientRect is scaled
        // by the opening animation and can underestimate the final menu size.
        let width = f64::from(el.client_width()) + 2.0;
        let height = f64::from(el.client_height()) + 2.0;
        let window = web_sys::window().unwrap();
        let viewport_width = window
            .inner_width()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let viewport_height = window
            .inner_height()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let margin = 8.0;
        let anchor_x = f64::from(menu.x);
        let anchor_y = f64::from(menu.y);

        let mut left = anchor_x;
        if left + width + margin > viewport_width {
            left = anchor_x - width;
        }
        left = left
            .max(margin)
            .min((viewport_width - width - margin).max(margin));

        let mut top = anchor_y;
        if top + height + margin > viewport_height {
            top = anchor_y - height;
        }
        top = top
            .max(margin)
            .min((viewport_height - height - margin).max(margin));

        pos.set((left.round() as i32, top.round() as i32));
    });

    view! {
        {move || snapshot.get().map(|m| {
            // When the right-clicked row is part of a multi-selection, all
            // bulk-capable actions fire on every selected row.
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
            let talos_actions = talos_features.get().actions;
            let rows_snapshot = rows_opt.map(|rows| rows.get_untracked());
            let actions = ResourceActionModel::for_selection(
                ActionSurface::Desktop,
                &targets,
                &target_uids,
                rows_snapshot.as_ref(),
                m.node.as_deref(),
                talos_actions,
                permitted,
            );
            let control_plane = actions.control_plane;

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
            // Bulk-aware single-action closures — each captures its own clone of targets.
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
            let with_source_checked = RwSignal::new(false);
            let force_checked = RwSignal::new(false);
            let reset_checked = RwSignal::new(false);
            let reconcile = {
                let ts = targets.clone();
                move |_| {
                    let action = if with_source_checked.get_untracked() {
                        "flux-reconcile-with-source"
                    } else {
                        "flux-reconcile"
                    };
                    let extra = serde_json::json!({
                        "force": force_checked.get_untracked(),
                        "reset": reset_checked.get_untracked(),
                    });
                    fire_action_with(toast, action, &ts, extra);
                    if let Some(sel) = table_selected.get_value() { sel.set(Default::default()); }
                    do_close();
                }
            };
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
            let cordon    = bulk_act!("cordon");
            let uncordon  = bulk_act!("uncordon");
            // Opens the drain options dialog (`overlays::drain`) rather than
            // running immediately — see `DrainOpen`.
            let drain = {
                let key = m.target.key.clone();
                let name = m.target.name.clone();
                move |_| {
                    drain_open.set(Some(DrainTarget {
                        key: key.clone(),
                        name: name.clone(),
                        power: None,
                        control_plane: false,
                        job: None,
                    }));
                    do_close();
                }
            };
            let talos_reboot = {
                let key = m.target.key.clone();
                let name = m.target.name.clone();
                move |_| {
                    drain_open.set(Some(DrainTarget {
                        key: key.clone(),
                        name: name.clone(),
                        power: Some("reboot".to_string()),
                        control_plane,
                        job: None,
                    }));
                    do_close();
                }
            };
            let talos_shutdown = {
                let key = m.target.key.clone();
                let name = m.target.name.clone();
                move |_| {
                    drain_open.set(Some(DrainTarget {
                        key: key.clone(),
                        name: name.clone(),
                        power: Some("shutdown".to_string()),
                        control_plane,
                        job: None,
                    }));
                    do_close();
                }
            };
            let talos_etcd_defrag = {
                let target = m.target.clone();
                move |_| {
                    let target = target.clone();
                    let node = target.name.clone();
                    ask_confirm(
                        confirm,
                        format!("Defragment etcd on {node}? This is a resource-intensive operation."),
                        "Defrag",
                        move || fire_action(toast, "talos-etcd-defrag", std::slice::from_ref(&target)),
                    );
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
            // PDB-respecting eviction, distinct from `delete`: the server
            // enforces disruption budgets and may reject the request.
            let evict = {
                let ts = targets.clone();
                move |_| {
                    let ts = ts.clone();
                    let n = ts.len();
                    let label = if n == 1 { "Evict this pod?".to_string() }
                                else { format!("Evict {n} pods?") };
                    ask_confirm(confirm, label, "Evict", move || {
                        fire_action(toast, "evict", &ts);
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
            let operate_actions = [
                ResourceMenuAction::Restart, ResourceMenuAction::Scale,
                ResourceMenuAction::CronJobTrigger, ResourceMenuAction::JobRerun,
                ResourceMenuAction::KopiurSnapshotNow, ResourceMenuAction::FluxReconcile,
                ResourceMenuAction::CnpgBackup, ResourceMenuAction::CnpgSuspend,
                ResourceMenuAction::CnpgResume,
                ResourceMenuAction::FluxSuspend, ResourceMenuAction::FluxResume,
                ResourceMenuAction::ExternalSecretsRefresh, ResourceMenuAction::CertificateRenew,
                ResourceMenuAction::Cordon, ResourceMenuAction::Uncordon, ResourceMenuAction::Drain,
            ];
            let has_operate = actions.supports_any(&operate_actions);
            let has_flux = actions.supports_any(&[
                ResourceMenuAction::FluxReconcile,
                ResourceMenuAction::FluxReconcileWithSource,
                ResourceMenuAction::FluxForce,
                ResourceMenuAction::FluxReset,
                ResourceMenuAction::FluxSuspend,
                ResourceMenuAction::FluxResume,
            ]);
            let header_label = if is_bulk {
                format!("{} selected", targets.len())
            } else {
                m.target.name.clone()
            };
            let header_title = header_label.clone();

            view! {
                <div class="ctx-scrim"
                    on:click=move |_| do_close()
                    on:contextmenu=move |e: leptos::ev::MouseEvent| { e.prevent_default(); do_close(); }></div>
                <div class="ctx-menu" role="menu" node_ref=menu_ref class:closing=move || closing.get()
                    style=move || { let (x, y) = pos.get(); format!("left:{x}px;top:{y}px") }>
                    <div class="ctx-header" data-tip=header_title>
                        {header_label}
                    </div>

                    {(!is_bulk || has_logs).then(|| view! { <div class="ctx-section-label">"Inspect"</div> })}
                    {actions.supports(ResourceMenuAction::OpenDetails).then(|| view! { <button class="ctx-item" role="menuitem" on:click=open>"Open details"</button> })}
                    {actions.supports(ResourceMenuAction::Relationships).then(|| view! {
                        <button class="ctx-item" role="menuitem" on:click=open_tree>"View relationships"</button>
                    })}
                    {has_logs.then(|| view! { <button class="ctx-item" role="menuitem" on:click=logs>"View logs"</button> })}
                    {shell.map(|s| view! { <button class="ctx-item" role="menuitem" on:click=s>"Open shell"</button> })}
                    {files.map(|open| view! { <button class="ctx-item" role="menuitem" on:click=open>"Browse files"</button> })}
                    {actions.supports(ResourceMenuAction::DebugShell).then(|| {
                        let ns  = m.target.namespace.clone().unwrap_or_default();
                        let pod = m.target.name.clone();
                        move |_: leptos::ev::MouseEvent| {
                            let ns  = ns.clone();
                            let pod = pod.clone();
                            do_close();
                            exec_open.set(Some(ExecTarget {
                                namespace: ns.clone(),
                                pod: pod.clone(),
                                container: None,
                                pending: true,
                                node_shell: false,
                                image: debug_image.get_untracked(),
                            }));
                            leptos::task::spawn_local(async move {
                                let still_pending = || {
                                    exec_open.get_untracked().is_some_and(|target| {
                                        target.pending
                                            && !target.node_shell
                                            && target.namespace == ns
                                            && target.pod == pod
                                    })
                                };
                                let body = serde_json::json!({
                                    "namespace": ns,
                                    "pod": pod,
                                });
                                match crate::data::post_json::<serde_json::Value>(
                                    "/api/debug-shell",
                                    &body,
                                )
                                .await
                                {
                                    Ok(resp) => {
                                        if still_pending() {
                                            if let Some(ctr) = resp.get("container").and_then(|c| c.as_str()) {
                                                exec_open.set(Some(ExecTarget {
                                                    namespace: ns.clone(),
                                                    pod: pod.clone(),
                                                    container: Some(ctr.to_string()),
                                                    pending: false,
                                                    node_shell: false,
                                                    image: resp
                                                        .get("image")
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("")
                                                        .to_string(),
                                                }));
                                            } else {
                                                exec_open.set(None);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        if still_pending() {
                                            exec_open.set(None);
                                        }
                                        show_toast_detail(toast, "Debug shell failed", Some(e), ToastKind::Err);
                                    }
                                }
                            });
                        }
                    }).map(|h| view! { <button class="ctx-item" role="menuitem" on:click=h>"Open debug shell"</button> })}
                    {actions.supports(ResourceMenuAction::NodeShell).then(|| {
                        let node = m.target.name.clone();
                        move |_: leptos::ev::MouseEvent| {
                            let node = node.clone();
                            do_close();
                            exec_open.set(Some(ExecTarget {
                                namespace: String::new(),
                                pod: node.clone(),
                                container: None,
                                pending: true,
                                node_shell: true,
                                image: debug_image.get_untracked(),
                            }));
                            leptos::task::spawn_local(async move {
                                let still_pending = || {
                                    exec_open.get_untracked().is_some_and(|target| {
                                        target.pending
                                            && target.node_shell
                                            && target.pod == node
                                    })
                                };
                                let body = serde_json::json!({ "node": node });
                                match crate::data::post_json::<serde_json::Value>(
                                    "/api/node-shell",
                                    &body,
                                )
                                .await
                                {
                                    Ok(resp) => {
                                        if still_pending() {
                                            let ns = resp.get("namespace").and_then(|v| v.as_str());
                                            let pod = resp.get("pod").and_then(|v| v.as_str());
                                            if let (Some(ns), Some(pod)) = (ns, pod) {
                                                exec_open.set(Some(ExecTarget {
                                                    namespace: ns.to_string(),
                                                    pod: pod.to_string(),
                                                    container: Some("shell".to_string()),
                                                    pending: false,
                                                    node_shell: true,
                                                    image: resp
                                                        .get("image")
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("")
                                                        .to_string(),
                                                }));
                                            } else {
                                                exec_open.set(None);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        if still_pending() {
                                            exec_open.set(None);
                                        }
                                        show_toast_detail(toast, "Node shell failed", Some(e), ToastKind::Err);
                                    }
                                }
                            });
                        }
                    }).map(|h| view! { <button class="ctx-item" role="menuitem" on:click=h>"Open node shell"</button> })}

                    <div class="ctx-section-label">"Navigate"</div>
                    {ns_item.map(|ns| view! { <button class="ctx-item" role="menuitem" on:click=goto_ns>"Go to namespace" <span class="ctx-sub">{ns}</span></button> })}
                    {node_item.map(|node| view! { <button class="ctx-item" role="menuitem" on:click=goto_node>"Go to node" <span class="ctx-sub">{node}</span></button> })}
                    <button class="ctx-item" role="menuitem" on:click=copy>{if is_bulk { "Copy resource names" } else { "Copy resource name" }}</button>

                    {has_operate.then(|| view! { <div class="ctx-section-label">"Operate"</div> })}
                    {actions.supports(ResourceMenuAction::Restart).then(|| view! { <button class="ctx-item" role="menuitem" on:click=restart>"Restart workload"</button> })}
                    {actions.supports(ResourceMenuAction::Scale).then(|| {
                        let t = m.target.clone();
                        view! {
                            <div class="ctx-item ctx-scale">
                                <span>"Scale"</span>
                                <input type="number" min="0" class="ctx-scale-input"
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
                    {actions.supports(ResourceMenuAction::CronJobTrigger).then(|| view! { <button class="ctx-item" role="menuitem" on:click=trigger>"Trigger job"</button> })}
                    {actions.supports(ResourceMenuAction::JobRerun).then(|| view! { <button class="ctx-item" role="menuitem" on:click=rerun>"Re-run job"</button> })}
                    {actions.supports(ResourceMenuAction::KopiurSnapshotNow).then(|| view! { <button class="ctx-item" role="menuitem" on:click=snapshot_now>"Snapshot now"</button> })}
                    {actions.supports(ResourceMenuAction::CnpgBackup).then(|| view! { <button class="ctx-item" role="menuitem" on:click=create_cnpg_backup>"Create backup..."</button> })}
                    {actions.supports(ResourceMenuAction::CnpgSuspend).then(|| view! { <button class="ctx-item" role="menuitem" on:click=cnpg_suspend>"Suspend schedule"</button> })}
                    {actions.supports(ResourceMenuAction::CnpgResume).then(|| view! { <button class="ctx-item" role="menuitem" on:click=cnpg_resume>"Resume schedule"</button> })}
                    {has_flux.then(|| view! {
                        {actions.supports(ResourceMenuAction::FluxReconcile).then(|| view! {
                        <div class="ctx-item ctx-reconcile">
                            <button class="ctx-reconcile-btn" on:click=reconcile>"Reconcile"</button>
                            <span class="ctx-chips">
                                {actions.supports(ResourceMenuAction::FluxReconcileWithSource).then(|| view! {
                                    <button type="button" class="ctx-chip" class:active=move || with_source_checked.get()
                                        on:click=move |e: leptos::ev::MouseEvent| { e.stop_propagation(); with_source_checked.update(|v| *v = !*v); }>"src"</button>
                                })}
                                {actions.supports(ResourceMenuAction::FluxForce).then(|| view! {
                                    <button type="button" class="ctx-chip" class:active=move || force_checked.get()
                                        on:click=move |e: leptos::ev::MouseEvent| { e.stop_propagation(); force_checked.update(|v| *v = !*v); }>"force"</button>
                                })}
                                {actions.supports(ResourceMenuAction::FluxReset).then(|| view! {
                                    <button type="button" class="ctx-chip" class:active=move || reset_checked.get()
                                        on:click=move |e: leptos::ev::MouseEvent| { e.stop_propagation(); reset_checked.update(|v| *v = !*v); }>"reset"</button>
                                })}
                            </span>
                        </div>
                        })}
                        {actions.supports(ResourceMenuAction::FluxSuspend).then(|| view! { <button class="ctx-item" role="menuitem" on:click=suspend>"Suspend"</button> })}
                        {actions.supports(ResourceMenuAction::FluxResume).then(|| view! { <button class="ctx-item" role="menuitem" on:click=resume>"Resume"</button> })}
                    })}
                    {actions.supports(ResourceMenuAction::ExternalSecretsRefresh).then(|| view! { <button class="ctx-item" role="menuitem" on:click=refresh>"Refresh secret"</button> })}
                    {actions.supports(ResourceMenuAction::CertificateRenew).then(|| view! { <button class="ctx-item" role="menuitem" on:click=renew_certificate>"Force renewal"</button> })}
                    {actions.supports(ResourceMenuAction::Cordon).then(|| view! { <button class="ctx-item" role="menuitem" on:click=cordon>"Cordon node"</button> })}
                    {actions.supports(ResourceMenuAction::Uncordon).then(|| view! { <button class="ctx-item" role="menuitem" on:click=uncordon>"Uncordon node"</button> })}
                    {actions.supports(ResourceMenuAction::Drain).then(|| view! { <button class="ctx-item caution" role="menuitem" on:click=drain>"Drain node…"</button> })}

                    {actions.supports_any(&[ResourceMenuAction::TalosEtcdDefrag, ResourceMenuAction::TalosReboot, ResourceMenuAction::TalosShutdown]).then(|| view! {
                        <div class="ctx-section-label">"Talos"</div>
                        {actions.supports(ResourceMenuAction::TalosEtcdDefrag).then(|| view! {
                            <button class="ctx-item caution" role="menuitem" on:click=talos_etcd_defrag>"Defragment etcd…"</button>
                        })}
                        {actions.supports(ResourceMenuAction::TalosReboot).then(|| view! { <button class="ctx-item caution" role="menuitem" on:click=talos_reboot>"Reboot node…"</button> })}
                        {actions.supports(ResourceMenuAction::TalosShutdown).then(|| view! { <button class="ctx-item danger" role="menuitem" on:click=talos_shutdown>"Shut down node…"</button> })}
                    })}

                    <div class="ctx-section-label danger">"Danger"</div>
                    {actions.supports(ResourceMenuAction::Evict).then(|| view! { <button class="ctx-item caution" role="menuitem" on:click=evict>"Evict pod…"</button> })}
                    {actions.supports(ResourceMenuAction::Delete).then(|| view! { <button class="ctx-item danger" role="menuitem" on:click=delete>"Delete resource…"</button> })}
                </div>
            }
        })}
    }
}
