//! Mobile-native full-screen resource relationship explorer.

use leptos::prelude::*;
use roder_core::ResourceTreeNode;

use crate::app::state::{DetailTarget, TreeOpen};
use crate::data;

#[component]
pub(crate) fn MobileRelationshipTree() -> impl IntoView {
    let tree_open = expect_context::<TreeOpen>().0;

    view! {
        {move || tree_open.get().map(|target| {
            let title = target.name.clone();
            view! {
                <div class="mobile-relationship-view">
                    <header>
                        <div><span>"Relationships"</span><strong>{title}</strong></div>
                        <button aria-label="Close relationships" on:click=move |_| tree_open.set(None)>"×"</button>
                    </header>
                    <MobileRelationshipContent target=target />
                </div>
            }
        })}
    }
}

#[component]
fn MobileRelationshipContent(target: DetailTarget) -> impl IntoView {
    let request = target.clone();
    let root = LocalResource::new(move || {
        let target = request.clone();
        async move {
            let url = format!(
                "/api/resource-tree?key={}&namespace={}&name={}",
                target.key,
                target
                    .namespace
                    .as_deref()
                    .map(data::percent_encode)
                    .unwrap_or_default(),
                data::percent_encode(&target.name),
            );
            data::fetch_json::<ResourceTreeNode>(&url).await
        }
    });

    view! {
        <div class="mobile-relationship-body">
            {move || match root.get() {
                None => view! { <div class="mobile-relationship-status">"Resolving relationships..."</div> }.into_any(),
                Some(Err(error)) => view! { <div class="mobile-relationship-status error">{error}</div> }.into_any(),
                Some(Ok(node)) => view! { <MobileRelationshipNode node=node root=true /> }.into_any(),
            }}
        </div>
    }
}

#[component]
fn MobileRelationshipNode(node: ResourceTreeNode, root: bool) -> AnyView {
    let tree_open = expect_context::<TreeOpen>().0;
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let expanded = RwSignal::new(root);
    let has_children = !node.children.is_empty() || node.error.is_some();
    let key = node.key.clone();
    let namespace = node.namespace.clone();
    let name = node.name.clone();
    let relation = node.relation.map(|relation| relation.label());
    let subtitle = match (relation, node.namespace.as_deref()) {
        (Some(relation), Some(namespace)) => format!("{relation} · {} · {namespace}", node.kind),
        (Some(relation), None) => format!("{relation} · {}", node.kind),
        (None, Some(namespace)) => format!("{} · {namespace}", node.kind),
        (None, None) => node.kind.clone(),
    };
    let error = node.error;
    let children = node.children;

    view! {
        <div class="mobile-relationship-node">
            <div class="mobile-relationship-row">
                <button class="mobile-relationship-main" disabled=key.is_none() on:click=move |_| {
                    if let Some(key) = key.clone() {
                        detail.set(Some(DetailTarget { key, namespace: namespace.clone(), name: name.clone() }));
                        tree_open.set(None);
                    }
                }>
                    <span class="mobile-relationship-kind">{node.kind.chars().next().unwrap_or('?')}</span>
                    <span><strong>{node.name}</strong><small>{subtitle}</small></span>
                </button>
                {has_children.then(|| view! {
                    <button class="mobile-relationship-toggle" aria-label="Toggle relationships"
                        on:click=move |_| expanded.update(|value| *value = !*value)>
                        {move || if expanded.get() { "−" } else { "+" }}
                    </button>
                })}
            </div>
            {move || (has_children && expanded.get()).then(|| view! {
                <div class="mobile-relationship-children">
                    {error.clone().map(|error| view! { <div class="mobile-relationship-error">{error}</div> })}
                    {children.iter().cloned().map(|child| view! {
                        <MobileRelationshipNode node=child root=false />
                    }).collect_view()}
                </div>
            })}
        </div>
    }.into_any()
}
