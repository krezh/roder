//! Right-click context menu with resource-type-specific actions.

use leptos::prelude::*;
use roder_core::{ResourceKind, RowStatus};

use crate::app::events::{fire_action, fire_action_with};
use crate::app::overlays::confirm::{ask_confirm, Confirm};
use crate::app::overlays::toast::{show_toast, show_toast_detail, Toast, ToastKind};
use crate::app::state::{
    open_logs, Catalog, CtxMenu, DetailTarget, DrainOpen, DrainTarget, ExecOpen, ExecTarget,
    LogPods, LogTarget, TableRows, TableSelected, TreeOpen,
};
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
    let catalog = expect_context::<Catalog>().0;
    let log_pods = expect_context::<LogPods>().0;
    let exec_open = expect_context::<ExecOpen>().0;
    let tree_open = expect_context::<TreeOpen>().0;
    let drain_open = expect_context::<DrainOpen>().0;
    // Provided at App level; ResourceView fills in the Option on mount.
    let table_selected = expect_context::<TableSelected>().0;
    let table_rows = expect_context::<TableRows>().0;
    let toast = expect_context::<RwSignal<Option<Toast>>>();

    let (snapshot, closing, do_close) = super::use_option_overlay(ctx);
    let pos = RwSignal::new((0i32, 0i32));
    let menu_ref = NodeRef::<leptos::html::Div>::new();

    Effect::new(move |_| {
        if let Some(menu) = snapshot.get() {
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
            let (group, kind) = parse_key(&m.target.key);
            let kk = KindKind::new(&group, &kind);
            let is_pod = kk.is_pod();
            let is_workload = kk.is_workload();
            let is_scalable = kk.is_scalable();
            let is_flux = kk.is_flux();
            let is_helmrelease = kk.is_helmrelease();
            let is_kustomization = kk.is_kustomization();
            let has_source_ref = kk.has_source_ref();
            let is_eso = kk.is_eso();
            let is_cronjob = kk.is_cronjob();
            let is_node = kk.is_node();
            // When the right-clicked row is part of a multi-selection, all
            // bulk-capable actions fire on every selected row.
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
            // `RowStatus::Warn` is reserved for the suspended case in the Flux
            // projector (see `ready_message_cells`), so it doubles as the signal
            // for which of Suspend/Resume to show. `None` (mixed selection, or
            // no row data) falls back to showing both.
            let suspend_state: Option<bool> = rows_opt.and_then(|rows| {
                rows.with_untracked(|rm| {
                    let mut states = target_uids.iter().filter_map(|uid| rm.get(uid)).map(|r| r.status == RowStatus::Warn);
                    let first = states.next()?;
                    states.all(|s| s == first).then_some(first)
                })
            });
            let show_suspend = suspend_state != Some(true);
            let show_resume = suspend_state != Some(false);
            // Same `RowStatus::Warn` convention, scoped to Node rows instead of
            // Flux rows — see `node_cells` for where it's set from `spec.unschedulable`.
            let cordon_state: Option<bool> = rows_opt.and_then(|rows| {
                rows.with_untracked(|rm| {
                    let mut states = target_uids.iter().filter_map(|uid| rm.get(uid)).map(|r| r.status == RowStatus::Warn);
                    let first = states.next()?;
                    states.all(|s| s == first).then_some(first)
                })
            });
            let show_cordon = cordon_state != Some(true);
            let show_uncordon = cordon_state != Some(false);

            let open = { let t = m.target.clone(); move |_| { detail.set(Some(t.clone())); do_close(); } };
            let open_tree = { let t = m.target.clone(); move |_| { tree_open.set(Some(t.clone())); do_close(); } };
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
                    }));
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
                    ask_confirm(confirm, label, "Delete", move || {
                        fire_action(toast, "delete", &ts);
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
                <div class="ctx-scrim"
                    on:click=move |_| do_close()
                    on:contextmenu=move |e: leptos::ev::MouseEvent| { e.prevent_default(); do_close(); }></div>
                <div class="ctx-menu" node_ref=menu_ref class:closing=move || closing.get()
                    style=move || { let (x, y) = pos.get(); format!("left:{x}px;top:{y}px") }>
                    {is_bulk.then(|| view! {
                        <div class="ctx-item ctx-bulk-header">{targets.len()}" resources"</div>
                    })}
                    {(!is_bulk).then(|| view! { <button class="ctx-item" on:click=open>"Open details"</button> })}
                    {(!is_bulk && (is_kustomization || is_helmrelease)).then(|| view! {
                        <button class="ctx-item" on:click=open_tree>"Resource Tree"</button>
                    })}
                    {has_logs.then(|| view! { <button class="ctx-item" on:click=logs>"Logs"</button> })}
                    {shell.map(|s| view! { <button class="ctx-item" on:click=s>"Shell"</button> })}
                    {(!is_bulk && is_pod).then(|| {
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
                            }));
                            leptos::task::spawn_local(async move {
                                let url = format!(
                                    "/api/debug-shell?namespace={}&pod={}",
                                    crate::data::percent_encode(&ns),
                                    crate::data::percent_encode(&pod),
                                );
                                // Guard: don't touch exec_open if the user dismissed the overlay
                                // while the request was in flight (pending cleared or signal gone).
                                let still_pending = || {
                                    exec_open.get_untracked().map(|t| t.pending).unwrap_or(false)
                                };
                                match crate::data::fetch_json::<serde_json::Value>(&url).await {
                                    Ok(resp) => {
                                        if still_pending() {
                                            if let Some(ctr) = resp.get("container").and_then(|c| c.as_str()) {
                                                exec_open.set(Some(ExecTarget {
                                                    namespace: ns.clone(),
                                                    pod: pod.clone(),
                                                    container: Some(ctr.to_string()),
                                                    pending: false,
                                                    node_shell: false,
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
                    }).map(|h| view! { <button class="ctx-item" on:click=h>"Debug shell"</button> })}
                    {(!is_bulk && is_node).then(|| {
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
                            }));
                            leptos::task::spawn_local(async move {
                                let url = format!(
                                    "/api/node-shell?node={}",
                                    crate::data::percent_encode(&node),
                                );
                                // Guard: don't touch exec_open if the user dismissed the
                                // overlay while the request was in flight.
                                let still_pending = || {
                                    exec_open.get_untracked().map(|t| t.pending).unwrap_or(false)
                                };
                                match crate::data::fetch_json::<serde_json::Value>(&url).await {
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
                    }).map(|h| view! { <button class="ctx-item" on:click=h>"Node shell"</button> })}
                    {ns_item.map(|ns| view! { <button class="ctx-item" on:click=goto_ns>"Go to namespace " <span class="ctx-sub">{ns}</span></button> })}
                    {node_item.map(|node| view! { <button class="ctx-item" on:click=goto_node>"Go to node " <span class="ctx-sub">{node}</span></button> })}
                    <button class="ctx-item" on:click=copy>"Copy name"</button>
                    {is_workload.then(|| view! { <button class="ctx-item" on:click=restart>"Restart"</button> })}
                    {(!is_bulk && is_scalable).then(|| {
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
                    {is_cronjob.then(|| view! { <button class="ctx-item" on:click=trigger>"Trigger"</button> })}
                    {is_flux.then(|| view! {
                        <div class="ctx-item ctx-reconcile">
                            <button class="ctx-reconcile-btn" on:click=reconcile>"Reconcile"</button>
                            <span class="ctx-chips">
                                {has_source_ref.then(|| view! {
                                    <span class="ctx-chip" class:active=move || with_source_checked.get()
                                        on:click=move |e: leptos::ev::MouseEvent| { e.stop_propagation(); with_source_checked.update(|v| *v = !*v); }>"src"</span>
                                })}
                                {is_helmrelease.then(|| view! {
                                    <span class="ctx-chip" class:active=move || force_checked.get()
                                        on:click=move |e: leptos::ev::MouseEvent| { e.stop_propagation(); force_checked.update(|v| *v = !*v); }>"force"</span>
                                    <span class="ctx-chip" class:active=move || reset_checked.get()
                                        on:click=move |e: leptos::ev::MouseEvent| { e.stop_propagation(); reset_checked.update(|v| *v = !*v); }>"reset"</span>
                                })}
                            </span>
                        </div>
                        {show_suspend.then(|| view! { <button class="ctx-item" on:click=suspend>"Suspend"</button> })}
                        {show_resume.then(|| view! { <button class="ctx-item" on:click=resume>"Resume"</button> })}
                    })}
                    {is_eso.then(|| view! { <button class="ctx-item" on:click=refresh>"Refresh"</button> })}
                    {(is_node && show_cordon).then(|| view! { <button class="ctx-item" on:click=cordon>"Cordon"</button> })}
                    {(is_node && show_uncordon).then(|| view! { <button class="ctx-item" on:click=uncordon>"Uncordon"</button> })}
                    {(!is_bulk && is_node).then(|| view! { <button class="ctx-item danger" on:click=drain>"Drain"</button> })}
                    {is_pod.then(|| view! { <button class="ctx-item danger" on:click=evict>"Evict"</button> })}
                    <button class="ctx-item danger" on:click=delete>"Delete"</button>
                </div>
            }
        })}
    }
}
