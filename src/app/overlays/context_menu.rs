//! Right-click context menu with resource-type-specific actions.

use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::events::{fire_action, fire_action_with};
use crate::app::overlays::confirm::{ask_confirm, Confirm};
use crate::app::state::{
    open_logs, Catalog, CtxMenu, DetailTarget, LogPods, LogTarget, TableRows, TableSelected,
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
    // Provided at App level; ResourceView fills in the Option on mount.
    let table_selected = expect_context::<TableSelected>().0;
    let table_rows = expect_context::<TableRows>().0;

    view! {
        {move || ctx.get().map(|m| {
            let (group, kind) = parse_key(&m.target.key);
            let kk = KindKind::new(&group, &kind);
            let is_pod = kk.is_pod();
            let is_workload = kk.is_workload();
            let is_scalable = kk.is_scalable();
            let is_flux = kk.is_flux();
            let is_eso = kk.is_eso();
            let is_cronjob = kk.is_cronjob();
            let style = format!("left:{}px;top:{}px", m.x, m.y);

            // When the right-clicked row is part of a multi-selection, all
            // bulk-capable actions fire on every selected row.
            let targets: Vec<DetailTarget> = match (table_selected.get_value(), table_rows.get_value()) {
                (Some(sel), Some(rows)) => {
                    let uids = sel.get_untracked();
                    if uids.len() > 1 && uids.contains(&m.uid) {
                        rows.with_untracked(|rm| {
                            uids.iter()
                                .filter_map(|uid| rm.get(uid).map(|r| DetailTarget {
                                    key: m.target.key.clone(),
                                    namespace: r.namespace.clone(),
                                    name: r.name.clone(),
                                }))
                                .collect()
                        })
                    } else {
                        vec![m.target.clone()]
                    }
                }
                _ => vec![m.target.clone()],
            };
            let is_bulk = targets.len() > 1;

            let open = { let t = m.target.clone(); move |_| { detail.set(Some(t.clone())); ctx.set(None); } };
            let has_logs = is_pod || is_workload || kk.is_job();
            let logs = {
                let ts = targets.clone();
                let agg = !is_pod;
                move |_| {
                    for t in &ts {
                        open_logs(log_pods, LogTarget {
                            key: t.key.clone(),
                            namespace: t.namespace.clone().unwrap_or_default(),
                            name: t.name.clone(),
                            aggregate: agg,
                        });
                    }
                    ctx.set(None);
                }
            };
            let goto_ns = { let ns = m.target.namespace.clone(); move |_| { selected_ns.set(ns.clone()); ctx.set(None); } };
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
                    ctx.set(None);
                }
            };
            let copy = {
                let names: Vec<String> = targets.iter().map(|t| t.name.clone()).collect();
                move |_| { copy_to_clipboard(&names.join("\n")); ctx.set(None); }
            };
            // Bulk-aware single-action closures — each captures its own clone of targets.
            macro_rules! bulk_act {
                ($action:literal) => {{
                    let ts = targets.clone();
                    move |_| { for t in &ts { fire_action($action, t); } ctx.set(None); }
                }};
            }
            let restart   = bulk_act!("restart");
            let reconcile = bulk_act!("flux-reconcile");
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
                        for t in &ts { fire_action("delete", t); }
                    });
                    ctx.set(None);
                }
            };

            let scale_n = RwSignal::new(1i32);
            let ns_item = (!is_bulk).then(|| m.target.namespace.clone()).flatten();
            let node_item = (!is_bulk && is_pod).then(|| m.node.clone()).flatten();

            view! {
                <div class="ctx-scrim"
                    on:click=move |_| ctx.set(None)
                    on:contextmenu=move |e: leptos::ev::MouseEvent| { e.prevent_default(); ctx.set(None); }></div>
                <div class="ctx-menu" style=style>
                    {is_bulk.then(|| view! {
                        <div class="ctx-item ctx-bulk-header">{targets.len()}" resources"</div>
                    })}
                    <button class="ctx-item" on:click=open>"Open details"</button>
                    {has_logs.then(|| view! { <button class="ctx-item" on:click=logs>"Logs"</button> })}
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
                                    fire_action_with("scale", &t, serde_json::json!({ "replicas": scale_n.get_untracked() }));
                                    ctx.set(None);
                                }>"→"</button>
                            </div>
                        }
                    })}
                    {is_cronjob.then(|| view! { <button class="ctx-item" on:click=trigger>"Trigger"</button> })}
                    {is_flux.then(|| view! {
                        <button class="ctx-item" on:click=reconcile>"Reconcile"</button>
                        <button class="ctx-item" on:click=suspend>"Suspend"</button>
                        <button class="ctx-item" on:click=resume>"Resume"</button>
                    })}
                    {is_eso.then(|| view! { <button class="ctx-item" on:click=refresh>"Refresh"</button> })}
                    <button class="ctx-item danger" on:click=delete>"Delete"</button>
                </div>
            }
        })}
    }
}
