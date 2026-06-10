//! Right-click context menu with resource-type-specific actions.

use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::events::{fire_action, fire_action_with};
use crate::app::overlays::confirm::{ask_confirm, Confirm};
use crate::app::state::{open_logs, Catalog, CtxMenu, DetailTarget, LogPods, LogTarget};
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

            let open = { let t = m.target.clone(); move |_| { detail.set(Some(t.clone())); ctx.set(None); } };
            let has_logs = is_pod || is_workload || kk.is_job();
            let logs = {
                let t = m.target.clone();
                let agg = !is_pod; // workloads/jobs aggregate their pods into one panel
                move |_| {
                    open_logs(log_pods, LogTarget {
                        key: t.key.clone(),
                        namespace: t.namespace.clone().unwrap_or_default(),
                        name: t.name.clone(),
                        aggregate: agg,
                    });
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
            let copy = { let name = m.target.name.clone(); move |_| { copy_to_clipboard(&name); ctx.set(None); } };
            let restart = { let t = m.target.clone(); move |_| { fire_action("restart", &t); ctx.set(None); } };
            let reconcile = { let t = m.target.clone(); move |_| { fire_action("flux-reconcile", &t); ctx.set(None); } };
            let suspend = { let t = m.target.clone(); move |_| { fire_action("flux-suspend", &t); ctx.set(None); } };
            let resume = { let t = m.target.clone(); move |_| { fire_action("flux-resume", &t); ctx.set(None); } };
            let refresh = { let t = m.target.clone(); move |_| { fire_action("eso-refresh", &t); ctx.set(None); } };
            let trigger = { let t = m.target.clone(); move |_| { fire_action("cronjob-trigger", &t); ctx.set(None); } };
            let delete = { let t = m.target.clone(); move |_| {
                let t = t.clone();
                ask_confirm(confirm, "Delete this resource?", move || fire_action("delete", &t));
                ctx.set(None);
            } };

            let scale_n = RwSignal::new(1i32);

            let ns_item = m.target.namespace.clone();
            let node_item = if is_pod { m.node.clone() } else { None };

            view! {
                <div class="ctx-scrim"
                    on:click=move |_| ctx.set(None)
                    on:contextmenu=move |e: leptos::ev::MouseEvent| { e.prevent_default(); ctx.set(None); }></div>
                <div class="ctx-menu" style=style>
                    <button class="ctx-item" on:click=open>"Open details"</button>
                    {has_logs.then(|| view! { <button class="ctx-item" on:click=logs>"Logs"</button> })}
                    {ns_item.map(|ns| view! { <button class="ctx-item" on:click=goto_ns>"Go to namespace " <span class="ctx-sub">{ns}</span></button> })}
                    {node_item.map(|node| view! { <button class="ctx-item" on:click=goto_node>"Go to node " <span class="ctx-sub">{node}</span></button> })}
                    <button class="ctx-item" on:click=copy>"Copy name"</button>
                    {is_workload.then(|| view! { <button class="ctx-item" on:click=restart>"Restart"</button> })}
                    {is_scalable.then(|| {
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
