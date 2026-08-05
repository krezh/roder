//! Modal window showing the full recursive ownership tree of a Kustomization/
//! HelmRelease (resolved in one shot by `/api/resource-tree`). Kustomization/
//! HelmRelease ("owner") nodes render as fixed-width status-bordered cards,
//! collapsible (root expanded by default, everything else collapsed); leaf
//! resources render as compact chips that wrap several-per-line. See
//! `docs/superpowers/specs/2026-07-02-resource-tree-visual-design.md`.

use leptos::prelude::*;
use roder_core::{ResourceTreeNode, RowStatus};

use crate::app::components::icons::TreeKindIcon;
use crate::app::detail::RowDetail;
use crate::app::overlays::use_option_overlay;
use crate::app::state::{DetailTarget, TreeOpen};
use crate::app::util::predicate::KindKind;
use crate::data;

/// A run of consecutive same-shape siblings, in original order: leaf
/// resources (which wrap several-per-line) get batched together; each owner
/// (Kustomization/HelmRelease, which needs its own full-width card) stands
/// alone. "Owner" is classified by *kind* (Kustomization/HelmRelease),
/// regardless of whether its `status` resolved — a nested owner whose object
/// fetch failed still has an owner kind, just `status: None, error: Some(_)`,
/// and must still render as a card (showing its error) rather than a leaf.
pub(crate) enum ChildGroup {
    Leaves(Vec<ResourceTreeNode>),
    Owner(ResourceTreeNode),
}

pub(crate) fn group_children(children: Vec<ResourceTreeNode>) -> Vec<ChildGroup> {
    let mut groups = Vec::new();
    let mut pending_leaves = Vec::new();
    for child in children {
        let kk = KindKind::new(&child.group, &child.kind);
        if kk.is_kustomization() || kk.is_helmrelease() {
            if !pending_leaves.is_empty() {
                groups.push(ChildGroup::Leaves(std::mem::take(&mut pending_leaves)));
            }
            groups.push(ChildGroup::Owner(child));
        } else {
            pending_leaves.push(child);
        }
    }
    if !pending_leaves.is_empty() {
        groups.push(ChildGroup::Leaves(pending_leaves));
    }
    groups
}

/// Total number of resources under `node`, recursively — shown as the count
/// badge on a collapsed owner card. Computed client-side from the already-
/// fetched tree; no backend/wire change needed.
pub(crate) fn descendant_count(node: &ResourceTreeNode) -> usize {
    node.children.len() + node.children.iter().map(descendant_count).sum::<usize>()
}

/// Status → CSS class for an owner card's border color. Mirrors
/// `crate::app::util::color::dot_class` (used for the plain status dot
/// elsewhere) but named distinctly since it's applied to a card border here,
/// not a dot.
fn status_border_class(status: RowStatus) -> &'static str {
    match status {
        RowStatus::Ok => "status-ok",
        RowStatus::Pending => "status-pending",
        RowStatus::Warn => "status-warn",
        RowStatus::Error => "status-error",
        RowStatus::Done | RowStatus::Unknown => "status-unknown",
    }
}

/// Broadcasts "set every row's expand state to `desired`" — an epoch bump
/// (rather than just the bool) is how each row's `Effect` tells "the toolbar
/// fired again" apart from "no change yet", so clicking the same button twice
/// in a row (e.g. Collapse-all, Collapse-all) still re-applies.
#[derive(Clone, Copy)]
struct TreeExpandCommand(RwSignal<(u32, bool)>);

/// The resource currently shown in the tree's own attached detail pane —
/// scoped to one tree-window instance, entirely separate from the
/// app-global `DetailTarget` signal the main `DetailDrawer` reads. Opening a
/// leaf from within the tree must never touch (or be touched by) the global
/// drawer elsewhere in the app.
#[derive(Clone, Copy)]
struct TreeDetailTarget(RwSignal<Option<DetailTarget>>);

/// True if `target` is `node` itself, or lives anywhere in its subtree —
/// used to highlight the full ownership path from the tree's root down to
/// whatever's currently selected in the attached detail pane, not just the
/// selected leaf itself. A childless leaf's own "subtree" is just itself, so
/// this one function covers both leaf-exact-match and owner-ancestor checks.
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

    view! {
        <Show when=move || snapshot.get().is_some()>
            <div class="tree-scrim" class:closing=move || closing.get() on:click=move |_| do_close()></div>
            <div
                class="tree-window"
                class:closing=move || closing.get()
                class:with-detail=move || tree_detail.0.get().is_some()
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
            data::fetch_json::<ResourceTreeNode>(&url).await.ok()
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
            <span class="tree-title">"Resource Tree — " {title}</span>
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
                    Some(None) => view! { <div class="tree-status tree-err">"Failed to load resource tree."</div> }.into_any(),
                    Some(Some(node)) => view! { <OwnerCard node=node is_root=true /> }.into_any(),
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

/// An owner (Kustomization/HelmRelease) card: icon, name, kind/namespace,
/// status-colored border, and a trailer that's either just a chevron
/// (expanded) or a descendant-count badge + chevron (collapsed). Clicking the
/// card toggles its own children; clicking a leaf's name (in `LeafChip`)
/// opens the attached detail pane instead — cards themselves don't open the
/// detail pane since their primary click action is expand/collapse. Gets a
/// `tree-selected` highlight when the detail pane's target is this node or
/// anywhere in its subtree, so the whole ownership path to a selection is
/// visible, not just the selected leaf.
///
/// Returns `AnyView` (not `impl IntoView`): this component's children can
/// recurse back into `OwnerCard` (via `group_children`), and an opaque
/// `impl IntoView` return type can't participate in that recursion (E0720).
#[component]
fn OwnerCard(node: ResourceTreeNode, is_root: bool) -> AnyView {
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
    let subtitle = match &node.namespace {
        Some(ns) => format!("{} · {}", node.kind, ns),
        None => node.kind.clone(),
    };
    let error = node.error.clone();
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

    view! {
        <div
            class=format!("tree-owner-card {border_class}")
            class:tree-selected=selected
            on:click=move |_| expanded.update(|e| *e = !*e)
        >
            <TreeKindIcon category=category kind=kind small=false />
            <div class="tree-owner-text">
                <div class="tree-name">{name}</div>
                <div class="tree-kind-line">{subtitle}</div>
            </div>
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
        ChildGroup::Owner(node) => {
            view! { <OwnerCard node=node.clone() is_root=false /> }.into_any()
        }
        ChildGroup::Leaves(leaves) => view! {
            <div class="tree-leaf-flow">
                {leaves.iter().map(|leaf| view! { <LeafChip node=leaf.clone() /> }).collect_view()}
            </div>
        }
        .into_any(),
    }
}

/// A leaf resource: a compact, content-sized chip (not a full card) — several
/// wrap onto one line via the parent `.tree-leaf-flow` container. Clicking it
/// opens the resource in the tree's own attached detail pane (`TreeDetailTarget`,
/// separate from the app-global `DetailTarget` drawer) alongside the tree list.
/// Structurally identical to `OwnerCard`'s text block (name above kind, same
/// classes/sizes) but with the smaller icon and no border/trailer — leaves
/// carry no live status, by design (see spec). Gets a `tree-selected`
/// highlight when it's the current detail-pane target.
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
        move |_| {
            if let Some(key) = key.clone() {
                tree_detail.0.set(Some(DetailTarget {
                    key,
                    namespace: ns.clone(),
                    name: name.clone(),
                }));
            }
        }
    };
    view! {
        <div
            class="tree-leaf-chip"
            class:tree-leaf-disabled=!clickable
            class:tree-selected=selected
            data-tip=(!clickable).then_some("Kind not found in this cluster's catalog")
            on:click=open
        >
            <TreeKindIcon category=node.category kind=node.kind.clone() small=true />
            <div class="tree-owner-text">
                <div class="tree-name">{node.name}</div>
                <div class="tree-kind-line">{node.kind}</div>
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
            children: Vec::new(),
            error: None,
        }
    }

    fn owner(name: &str, children: Vec<ResourceTreeNode>) -> ResourceTreeNode {
        ResourceTreeNode {
            kind: "Kustomization".into(),
            group: "kustomize.toolkit.fluxcd.io".into(),
            name: name.into(),
            namespace: None,
            key: Some("kustomize.toolkit.fluxcd.io/v1/Kustomization".into()),
            category: None,
            status: Some(RowStatus::Ok),
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
        let groups = group_children(vec![owner("a", vec![]), owner("b", vec![])]);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| matches!(g, ChildGroup::Owner(_))));
    }

    #[test]
    fn interleaved_leaves_and_owners_preserve_order_and_chunk_consecutively() {
        let groups = group_children(vec![
            leaf("a"),
            leaf("b"),
            owner("o1", vec![]),
            leaf("c"),
            owner("o2", vec![]),
            owner("o3", vec![]),
            leaf("d"),
        ]);
        assert_eq!(groups.len(), 6);
        assert!(matches!(&groups[0], ChildGroup::Leaves(v) if v.len() == 2));
        assert!(matches!(&groups[1], ChildGroup::Owner(n) if n.name == "o1"));
        assert!(matches!(&groups[2], ChildGroup::Leaves(v) if v.len() == 1 && v[0].name == "c"));
        assert!(matches!(&groups[3], ChildGroup::Owner(n) if n.name == "o2"));
        assert!(matches!(&groups[4], ChildGroup::Owner(n) if n.name == "o3"));
        assert!(matches!(&groups[5], ChildGroup::Leaves(v) if v.len() == 1 && v[0].name == "d"));
    }

    #[test]
    fn empty_children_produce_no_groups() {
        assert!(group_children(vec![]).is_empty());
    }

    #[test]
    fn descendant_count_is_zero_for_no_children() {
        assert_eq!(descendant_count(&owner("x", vec![])), 0);
    }

    #[test]
    fn descendant_count_counts_flat_children() {
        let node = owner("x", vec![leaf("a"), leaf("b"), leaf("c")]);
        assert_eq!(descendant_count(&node), 3);
    }

    #[test]
    fn descendant_count_is_recursive() {
        let node = owner("x", vec![leaf("a"), owner("y", vec![leaf("b"), leaf("c")])]);
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
            children: Vec::new(),
            error: Some("could not fetch object: rbac denied".into()),
        };
        let groups = group_children(vec![errored_owner]);
        assert_eq!(groups.len(), 1);
        assert!(
            matches!(&groups[0], ChildGroup::Owner(n) if n.name == "broken" && n.error.is_some())
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
        let node = owner("root", vec![leaf("a"), owner("mid", vec![leaf("b")])]);
        let target = DetailTarget {
            key: "v1/ConfigMap".into(),
            namespace: None,
            name: "b".into(),
        };
        assert!(subtree_contains(&node, &target));
    }

    #[test]
    fn subtree_contains_false_for_an_unrelated_target() {
        let node = owner("root", vec![leaf("a")]);
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
