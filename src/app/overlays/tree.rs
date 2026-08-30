//! Modal window showing a server-resolved resource relationship tree.

use leptos::prelude::*;
use roder_core::{ResourceTreeNode, RowStatus};

use crate::app::components::icons::TreeKindIcon;
use crate::app::detail::RowDetail;
use crate::app::overlays::use_option_overlay;
use crate::app::state::{DetailTarget, TreeOpen};
use crate::data;

pub(crate) enum ChildGroup {
    Leaves(Vec<ResourceTreeNode>),
    Branch(ResourceTreeNode),
}

pub(crate) fn group_children(children: Vec<ResourceTreeNode>) -> Vec<ChildGroup> {
    let mut groups = Vec::new();
    let mut pending_leaves = Vec::new();
    for child in children {
        if child.expandable {
            if !pending_leaves.is_empty() {
                groups.push(ChildGroup::Leaves(std::mem::take(&mut pending_leaves)));
            }
            groups.push(ChildGroup::Branch(child));
        } else {
            pending_leaves.push(child);
        }
    }
    if !pending_leaves.is_empty() {
        groups.push(ChildGroup::Leaves(pending_leaves));
    }
    groups
}

/// Total resources below a relationship branch.
pub(crate) fn descendant_count(node: &ResourceTreeNode) -> usize {
    node.children.len() + node.children.iter().map(descendant_count).sum::<usize>()
}

fn status_border_class(status: RowStatus) -> &'static str {
    match status {
        RowStatus::Ok => "status-ok",
        RowStatus::Pending => "status-pending",
        RowStatus::Warn => "status-warn",
        RowStatus::Error => "status-error",
        RowStatus::Done | RowStatus::Unknown => "status-unknown",
    }
}

/// The epoch makes repeated expand/collapse commands observable.
#[derive(Clone, Copy)]
struct TreeExpandCommand(RwSignal<(u32, bool)>);

/// Resource shown in this window's attached detail pane.
#[derive(Clone, Copy)]
struct TreeDetailTarget(RwSignal<Option<DetailTarget>>);

fn subtree_contains(node: &ResourceTreeNode, target: &DetailTarget) -> bool {
    let is_self = node.key.as_deref() == Some(target.key.as_str())
        && node.namespace == target.namespace
        && node.name == target.name;
    is_self || node.children.iter().any(|c| subtree_contains(c, target))
}

#[component]
pub(crate) fn ResourceTreeWindow() -> impl IntoView {
    let tree_open = expect_context::<TreeOpen>().0;
    let (snapshot, closing, do_close) = use_option_overlay(tree_open);
    let tree_detail = TreeDetailTarget(RwSignal::new(None::<DetailTarget>));
    provide_context(tree_detail);
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    crate::app::ui::use_dialog_focus(dialog_ref);

    view! {
        <Show when=move || snapshot.get().is_some()>
            <div class="tree-scrim" class:closing=move || closing.get() on:click=move |_| do_close()></div>
            <div
                class="tree-window"
                class:closing=move || closing.get()
                class:with-detail=move || tree_detail.0.get().is_some()
                node_ref=dialog_ref
                role="dialog"
                aria-modal="true"
                tabindex="-1"
            >
                {move || snapshot.get().map(|target| view! { <TreeContent target=target do_close=do_close /> })}
            </div>
        </Show>
    }
}

#[component]
fn TreeContent(target: DetailTarget, do_close: impl Fn() + Copy + 'static) -> impl IntoView {
    let title = target.name.clone();
    let t = target.clone();
    let root = LocalResource::new(move || {
        let t = t.clone();
        async move {
            let url = format!(
                "/api/resource-tree?key={}&namespace={}&name={}",
                t.key,
                t.namespace
                    .as_deref()
                    .map(data::percent_encode)
                    .unwrap_or_default(),
                data::percent_encode(&t.name),
            );
            data::fetch_json::<ResourceTreeNode>(&url).await
        }
    });

    let expand_command = TreeExpandCommand(RwSignal::new((0u32, true)));
    provide_context(expand_command);
    let fire = move |desired: bool| {
        expand_command.0.update(|(epoch, d)| {
            *epoch += 1;
            *d = desired;
        });
    };

    let tree_detail = expect_context::<TreeDetailTarget>();

    view! {
        <div class="tree-head">
            <span class="tree-title">"Relationships — " {title}</span>
            <button class="tree-close" on:click=move |_| do_close()>"✕"</button>
        </div>
        <div class="tree-toolbar">
            <button on:click=move |_| fire(true)>"Expand all"</button>
            <button on:click=move |_| fire(false)>"Collapse all"</button>
        </div>
        <div class="tree-panes">
            <div class="tree-pane">
                {move || match root.get() {
                    None => view! { <div class="tree-status">"Resolving tree…"</div> }.into_any(),
                    Some(Err(error)) => view! { <div class="tree-status tree-err">{error}</div> }.into_any(),
                    Some(Ok(node)) => view! { <BranchCard node=node is_root=true /> }.into_any(),
                }}
            </div>
            {move || tree_detail.0.get().map(|dt| {
                let name = dt.name.clone();
                view! {
                    <div class="tree-detail-pane">
                        <div class="tree-detail-head">
                            <span class="tree-detail-title">{name}</span>
                            <button class="tree-detail-close" on:click=move |_| tree_detail.0.set(None)>"✕"</button>
                        </div>
                        <RowDetail target=dt on_delete=move || tree_detail.0.set(None) />
                    </div>
                }
            })}
        </div>
    }
}

#[component]
fn BranchCard(node: ResourceTreeNode, is_root: bool) -> AnyView {
    let cmd = expect_context::<TreeExpandCommand>();
    let tree_detail = expect_context::<TreeDetailTarget>();
    let expanded = RwSignal::new(is_root);
    let last_seen_epoch = StoredValue::new(0u32);
    Effect::new(move |_| {
        let (epoch, desired) = cmd.0.get();
        if epoch != last_seen_epoch.get_value() {
            last_seen_epoch.set_value(epoch);
            expanded.set(desired);
        }
    });

    let count = descendant_count(&node);
    let border_class = node
        .status
        .map(status_border_class)
        .unwrap_or("status-unknown");
    let category = node.category.clone();
    let kind = node.kind.clone();
    let name = node.name.clone();
    let relation = node.relation.map(|relation| relation.label());
    let subtitle = match (&node.namespace, relation) {
        (Some(ns), Some(relation)) => format!("{relation} · {} · {ns}", node.kind),
        (None, Some(relation)) => format!("{relation} · {}", node.kind),
        (Some(ns), None) => format!("{} · {ns}", node.kind),
        (None, None) => node.kind.clone(),
    };
    let error = node.error.clone();
    let clickable = node.key.is_some();
    let open_key = node.key.clone();
    let open_namespace = node.namespace.clone();
    let open_name = node.name.clone();
    let node_snapshot = node.clone();
    let selected = move || {
        tree_detail
            .0
            .get()
            .map(|dt| subtree_contains(&node_snapshot, &dt))
            .unwrap_or(false)
    };
    let children = node.children;
    let groups = group_children(children);
    let toggle = move || expanded.update(|value| *value = !*value);

    view! {
        <div
            class=format!("tree-owner-card {border_class}")
            class:tree-selected=selected
            role="button"
            tabindex="0"
            aria-expanded=move || expanded.get().to_string()
            on:click=move |_| toggle()
            on:keydown=move |event: leptos::ev::KeyboardEvent| match event.key().as_str() {
                "Enter" | " " => { event.prevent_default(); toggle(); }
                _ => {}
            }
        >
            <TreeKindIcon category=category kind=kind small=false />
            <div class="tree-owner-text">
                <div class="tree-name">{name}</div>
                <div class="tree-kind-line">{subtitle}</div>
            </div>
            {clickable.then(|| view! {
                <button class="tree-node-open"
                    on:keydown=move |event: leptos::ev::KeyboardEvent| event.stop_propagation()
                    on:click=move |event: leptos::ev::MouseEvent| {
                    event.stop_propagation();
                    if let Some(key) = open_key.clone() {
                        tree_detail.0.set(Some(DetailTarget {
                            key,
                            namespace: open_namespace.clone(),
                            name: open_name.clone(),
                        }));
                    }
                }>"Open"</button>
            })}
            <div class="tree-trailer">
                {move || (!expanded.get()).then(|| view! { <span class="tree-count">{count}</span> })}
                <span class="tree-chevron">{move || if expanded.get() { "\u{25BE}" } else { "\u{25B8}" }}</span>
            </div>
        </div>
        {move || expanded.get().then(|| view! {
            <div class="tree-branch">
                {error.clone().map(|e| view! { <div class="tree-err-text">{e}</div> })}
                {groups.iter().map(render_group).collect_view()}
            </div>
        })}
    }
    .into_any()
}

fn render_group(group: &ChildGroup) -> AnyView {
    match group {
        ChildGroup::Branch(node) => {
            view! { <BranchCard node=node.clone() is_root=false /> }.into_any()
        }
        ChildGroup::Leaves(leaves) => view! {
            <div class="tree-leaf-flow">
                {leaves.iter().map(|leaf| view! { <LeafChip node=leaf.clone() /> }).collect_view()}
            </div>
        }
        .into_any(),
    }
}

#[component]
fn LeafChip(node: ResourceTreeNode) -> impl IntoView {
    let tree_detail = expect_context::<TreeDetailTarget>();
    let clickable = node.key.is_some();
    let node_snapshot = node.clone();
    let selected = move || {
        tree_detail
            .0
            .get()
            .map(|dt| subtree_contains(&node_snapshot, &dt))
            .unwrap_or(false)
    };
    let open = {
        let key = node.key.clone();
        let ns = node.namespace.clone();
        let name = node.name.clone();
        Callback::new(move |()| {
            if let Some(key) = key.clone() {
                tree_detail.0.set(Some(DetailTarget {
                    key,
                    namespace: ns.clone(),
                    name: name.clone(),
                }));
            }
        })
    };
    view! {
        <div
            class="tree-leaf-chip"
            class:tree-leaf-disabled=!clickable
            class:tree-selected=selected
            data-tip=(!clickable).then_some("Kind not found in this cluster's catalog")
            role="button"
            tabindex=if clickable { 0 } else { -1 }
            aria-disabled=(!clickable).then_some("true")
            on:click=move |_| open.run(())
            on:keydown=move |event: leptos::ev::KeyboardEvent| match event.key().as_str() {
                "Enter" | " " if clickable => { event.prevent_default(); open.run(()); }
                _ => {}
            }
        >
            <TreeKindIcon category=node.category kind=node.kind.clone() small=true />
            <div class="tree-owner-text">
                <div class="tree-name">{node.name}</div>
                <div class="tree-kind-line">{match node.relation {
                    Some(relation) => format!("{} · {}", relation.label(), node.kind),
                    None => node.kind,
                }}</div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod shaping_tests {
    use super::*;

    fn leaf(name: &str) -> ResourceTreeNode {
        ResourceTreeNode {
            kind: "ConfigMap".into(),
            group: String::new(),
            name: name.into(),
            namespace: None,
            key: Some("v1/ConfigMap".into()),
            category: None,
            status: None,
            relation: None,
            expandable: false,
            children: Vec::new(),
            error: None,
        }
    }

    fn branch(name: &str, children: Vec<ResourceTreeNode>) -> ResourceTreeNode {
        ResourceTreeNode {
            kind: "Kustomization".into(),
            group: "kustomize.toolkit.fluxcd.io".into(),
            name: name.into(),
            namespace: None,
            key: Some("kustomize.toolkit.fluxcd.io/v1/Kustomization".into()),
            category: None,
            status: Some(RowStatus::Ok),
            relation: None,
            expandable: true,
            children,
            error: None,
        }
    }

    #[test]
    fn all_leaves_become_one_group() {
        let groups = group_children(vec![leaf("a"), leaf("b"), leaf("c")]);
        assert_eq!(groups.len(), 1);
        assert!(matches!(&groups[0], ChildGroup::Leaves(v) if v.len() == 3));
    }

    #[test]
    fn all_owners_become_separate_groups() {
        let groups = group_children(vec![branch("a", vec![]), branch("b", vec![])]);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| matches!(g, ChildGroup::Branch(_))));
    }

    #[test]
    fn interleaved_leaves_and_owners_preserve_order_and_chunk_consecutively() {
        let groups = group_children(vec![
            leaf("a"),
            leaf("b"),
            branch("o1", vec![]),
            leaf("c"),
            branch("o2", vec![]),
            branch("o3", vec![]),
            leaf("d"),
        ]);
        assert_eq!(groups.len(), 6);
        assert!(matches!(&groups[0], ChildGroup::Leaves(v) if v.len() == 2));
        assert!(matches!(&groups[1], ChildGroup::Branch(n) if n.name == "o1"));
        assert!(matches!(&groups[2], ChildGroup::Leaves(v) if v.len() == 1 && v[0].name == "c"));
        assert!(matches!(&groups[3], ChildGroup::Branch(n) if n.name == "o2"));
        assert!(matches!(&groups[4], ChildGroup::Branch(n) if n.name == "o3"));
        assert!(matches!(&groups[5], ChildGroup::Leaves(v) if v.len() == 1 && v[0].name == "d"));
    }

    #[test]
    fn empty_children_produce_no_groups() {
        assert!(group_children(vec![]).is_empty());
    }

    #[test]
    fn descendant_count_is_zero_for_no_children() {
        assert_eq!(descendant_count(&branch("x", vec![])), 0);
    }

    #[test]
    fn descendant_count_counts_flat_children() {
        let node = branch("x", vec![leaf("a"), leaf("b"), leaf("c")]);
        assert_eq!(descendant_count(&node), 3);
    }

    #[test]
    fn descendant_count_is_recursive() {
        let node = branch(
            "x",
            vec![leaf("a"), branch("y", vec![leaf("b"), leaf("c")])],
        );
        assert_eq!(descendant_count(&node), 4);
    }

    #[test]
    fn errored_nested_owner_is_classified_as_owner_not_leaf() {
        // Mirrors `owner_error_node` in `crates/k8s/src/backend/tree.rs`: a
        // nested Kustomization/HelmRelease whose object fetch failed has
        // `status: None, error: Some(_)` — it must still be classified as an
        // `Owner` (so it renders an `OwnerCard` showing the error) rather than
        // silently becoming a plain, clickable `LeafChip` that drops the error.
        let errored_owner = ResourceTreeNode {
            kind: "Kustomization".into(),
            group: "kustomize.toolkit.fluxcd.io".into(),
            name: "broken".into(),
            namespace: Some("flux-system".into()),
            key: Some("kustomize.toolkit.fluxcd.io/v1/Kustomization".into()),
            category: None,
            status: None,
            relation: Some(roder_core::ResourceTreeRelation::Owner),
            expandable: true,
            children: Vec::new(),
            error: Some("could not fetch object: rbac denied".into()),
        };
        let groups = group_children(vec![errored_owner]);
        assert_eq!(groups.len(), 1);
        assert!(
            matches!(&groups[0], ChildGroup::Branch(n) if n.name == "broken" && n.error.is_some())
        );
    }

    #[test]
    fn subtree_contains_true_for_the_node_itself() {
        let node = leaf("a");
        let target = DetailTarget {
            key: "v1/ConfigMap".into(),
            namespace: None,
            name: "a".into(),
        };
        assert!(subtree_contains(&node, &target));
    }

    #[test]
    fn subtree_contains_true_for_a_nested_descendant() {
        let node = branch("root", vec![leaf("a"), branch("mid", vec![leaf("b")])]);
        let target = DetailTarget {
            key: "v1/ConfigMap".into(),
            namespace: None,
            name: "b".into(),
        };
        assert!(subtree_contains(&node, &target));
    }

    #[test]
    fn subtree_contains_false_for_an_unrelated_target() {
        let node = branch("root", vec![leaf("a")]);
        let target = DetailTarget {
            key: "v1/ConfigMap".into(),
            namespace: None,
            name: "z".into(),
        };
        assert!(!subtree_contains(&node, &target));
    }

    #[test]
    fn subtree_contains_false_when_key_matches_but_name_differs() {
        let node = leaf("a");
        let target = DetailTarget {
            key: "v1/ConfigMap".into(),
            namespace: None,
            name: "different".into(),
        };
        assert!(!subtree_contains(&node, &target));
    }
}
