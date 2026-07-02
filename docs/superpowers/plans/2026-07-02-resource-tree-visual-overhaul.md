# Resource Tree Visual Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Resource Tree's plain-text ASCII-branch rendering (`├─`/`└─`/`│`) with the icon+card design approved in `docs/superpowers/specs/2026-07-02-resource-tree-visual-design.md`: category-colored icon chips, fixed-width status-bordered cards for Kustomization/HelmRelease "owner" nodes, wrapping compact chips for leaf resources, and a collapsible tree (root expanded by default, everything else collapsed, with an Expand-all/Collapse-all toolbar).

**Architecture:** One additive backend field (`ResourceTreeNode.category`) threads the existing `Category` classification (already used by the sidebar) through the already-implemented recursive tree resolution — no other backend changes. On the frontend, two new pure/testable helpers (`group_children`, `descendant_count`) drive a rewritten row-rendering layer in `src/app/overlays/tree.rs`, a new category→icon mapping in `src/app/components/icons.rs`, and a full rewrite of `style/_tree.scss`. A small broadcast-signal pattern (epoch + desired-bool) lets the toolbar override every row's independently-owned collapse state in one click.

**Tech Stack:** Rust, Leptos 0.8 (component macros, signals, effects), SCSS, existing `roder_core`/`roder_k8s` crates.

## Global Constraints

- Reuse `roder_core::Category` for icon/color — do not invent a parallel per-Kind taxonomy (spec: "Icon & color system").
- No new icon-library dependency — glyphs are inline Unicode characters in styled `<span>` chips (the mockups used Unicode entities directly; treat those as the approved glyph set, not literal SVG paths — SVG was aspirational phrasing in the spec, not what was actually reviewed).
- Owner card fixed width: 280px, not stretched. Leaf chips: content-sized, wrapped via `flex-wrap`, multiple per line.
- Default collapse rule: **only the root node starts expanded; every other owner node, at any depth, starts collapsed.**
- Count badge and chevron must share one flex group with a single `margin-left: auto` (not two independent auto-margins) — this was a real bug hit during mockup review.
- `devenv test` is the authoritative test suite (fmt + clippy `-D warnings` ssr/hydrate + tests + docker build) — never bare `cargo` commands.
- Follow this project's jj workflow: check `jj status` before assuming a clean working copy; don't run `jj new` if there's already an appropriate empty/in-progress change.

---

## File Structure

| File | Change |
|---|---|
| `crates/core/src/lib.rs` | Add `category: Option<Category>` field to `ResourceTreeNode` |
| `crates/k8s/src/backend/tree.rs` | Thread `category` through `ChildRef`, `resolve_child`, `build_owner_node`, `node_for_child`, `owner_error_node`, `resource_tree` |
| `src/app/components/icons.rs` | New: `TreeKindIcon` component + `tree_icon_glyph`/`tree_icon_class` pure functions, with unit tests |
| `src/app/overlays/tree.rs` | Rewrite: replace `TreeRoot`/`TreeRow`/`TreeRowInner` (ASCII branches) with `TreeToolbar`, `TreeLevel`, `OwnerCard`, `LeafChip`, plus new pure helpers `group_children`/`ChildGroup` and `descendant_count` (unit tested) and the `TreeExpandCommand` broadcast signal |
| `style/_tree.scss` | Rewrite: remove grid-column row layout, add card/chip/toolbar/category-color styles matching the approved mockup |

---

## Task 1: Thread `category` through the backend tree resolution

**Files:**
- Modify: `crates/core/src/lib.rs` (the `ResourceTreeNode` struct)
- Modify: `crates/k8s/src/backend/tree.rs` (whole file — `ChildRef`, `resolve_child`, `build_owner_node`, `node_for_child`, `owner_error_node`, `resource_tree`)

**Interfaces:**
- Consumes: `roder_core::Category` (already defined, `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]`), `discovery::CatalogEntry.kind.category` (already exists, `ResourceKind.category: Category`).
- Produces: `ResourceTreeNode.category: Option<Category>` — consumed by Task 3/4's frontend rendering.

- [ ] **Step 1: Add the field to `ResourceTreeNode`**

In `crates/core/src/lib.rs`, find the `ResourceTreeNode` struct (added earlier for this feature) and add `category` right after `key`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceTreeNode {
    pub kind: String,
    pub group: String,
    pub name: String,
    pub namespace: Option<String>,
    pub key: Option<String>,
    /// The kind's sidebar category (Workloads/Flux/Rbac/…), for icon/color
    /// selection. `None` only when `key` is also `None` (kind not in the
    /// current catalog).
    pub category: Option<Category>,
    pub status: Option<RowStatus>,
    pub children: Vec<ResourceTreeNode>,
    pub error: Option<String>,
}
```

- [ ] **Step 2: Verify it compiles (expected error first)**

Run: `direnv exec /home/krezh/repos/roder cargo check -p roder-core -p roder-k8s`
Expected: FAIL — `crates/k8s/src/backend/tree.rs` has several `ResourceTreeNode { .. }` struct literals now missing the `category` field (E0063).

- [ ] **Step 3: Rewrite `crates/k8s/src/backend/tree.rs` to thread `category` through**

Replace the whole file with:

```rust
//! Recursive ownership tree for a Kustomization/HelmRelease (Flux
//! "app-of-apps"): resolves the whole tree server-side in one shot by reading
//! a Kustomization's `status.inventory.entries[]` and a HelmRelease's Helm
//! storage secret (see `helm_release.rs`), recursing into any child that is
//! itself a Kustomization/HelmRelease. Best-effort per node, mirroring the
//! `sanitize`/`flux_reconcile_all` philosophy: a node whose own children can't
//! be resolved gets an inline `error` instead of failing the whole request.

use futures::future::{join_all, BoxFuture};
use roder_core::{Category, ResourceKind, ResourceTreeNode};
use serde_json::Value;

use super::Backend;
use crate::client::K8sError;

/// Defensive guard against unexpected cycles. Real Flux app-of-apps trees are
/// typically 2-4 levels deep; 15 is generous headroom, not a realistic depth.
const MAX_DEPTH: usize = 15;

/// A child reference, resolved to a catalog key/category (or not — see
/// `Backend::resolve_child`), ready to become a leaf node or be recursed into.
struct ChildRef {
    group: String,
    kind: String,
    name: String,
    namespace: Option<String>,
    key: Option<String>,
    category: Option<Category>,
}

fn is_owner_kind(group: &str, kind: &str) -> bool {
    (group == "kustomize.toolkit.fluxcd.io" && kind == "Kustomization")
        || (group == "helm.toolkit.fluxcd.io" && kind == "HelmRelease")
}

impl Backend {
    /// Resolve the full recursive ownership tree for a Kustomization or
    /// HelmRelease. Returns `Err` only when the request itself is invalid
    /// (unknown `key`, or a kind that isn't Kustomization/HelmRelease) —
    /// every failure past that point (can't fetch the root object, RBAC-denied
    /// inventory/Helm secret, no deployed revision, depth cap) is embedded as
    /// that node's `error` field, so the caller always gets a 200 with a
    /// (possibly partial) tree once the root kind checks out.
    pub async fn resource_tree(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<ResourceTreeNode, K8sError> {
        let entry = self.entry(key)?;
        if !is_owner_kind(&entry.kind.group, &entry.kind.kind) {
            return Err(K8sError::Api(
                "resource tree is only available for Kustomization/HelmRelease".into(),
            ));
        }
        Ok(self
            .build_owner_node(
                key.to_string(),
                entry.kind.group.clone(),
                entry.kind.kind.clone(),
                Some(entry.kind.category.clone()),
                ns.map(str::to_string),
                name.to_string(),
                0,
            )
            .await)
    }

    /// Fetch + expand one Kustomization/HelmRelease node: the live object
    /// (cache-first, same as `detail()`) for its status dot, then its children
    /// (inventory or Helm manifest), recursing into any child that's itself an
    /// owner kind. Boxed because async fns can't recurse directly in Rust.
    fn build_owner_node(
        &self,
        key: String,
        group: String,
        kind: String,
        category: Option<Category>,
        ns: Option<String>,
        name: String,
        depth: usize,
    ) -> BoxFuture<'_, ResourceTreeNode> {
        Box::pin(async move {
            let obj = match self.registry.cached_object(&key, ns.as_deref(), &name).await {
                Some(o) => o,
                None => match self.dyn_api(&key, ns.as_deref()) {
                    Ok(api) => match api.get(&name).await {
                        Ok(o) => o,
                        Err(e) => {
                            return owner_error_node(
                                group,
                                kind,
                                category,
                                name,
                                ns,
                                Some(key),
                                format!("could not fetch object: {e}"),
                            )
                        }
                    },
                    Err(e) => {
                        return owner_error_node(
                            group,
                            kind,
                            category,
                            name,
                            ns,
                            Some(key),
                            e.to_string(),
                        )
                    }
                },
            };
            let data = serde_json::to_value(&obj).unwrap_or_default();
            let status = Some(crate::project::ready_message_cells(&data).1);

            if depth >= MAX_DEPTH {
                return ResourceTreeNode {
                    kind,
                    group,
                    name,
                    namespace: ns,
                    key: Some(key),
                    category,
                    status,
                    children: Vec::new(),
                    error: Some(format!("max recursion depth ({MAX_DEPTH}) reached")),
                };
            }

            let (refs, err) = if kind == "Kustomization" {
                match self.kustomization_children(&data) {
                    Ok(refs) => (refs, None),
                    Err(e) => (Vec::new(), Some(e)),
                }
            } else {
                match self.helm_release_children(&data).await {
                    Ok(manifest_refs) => {
                        let refs = manifest_refs
                            .into_iter()
                            .map(|r| {
                                self.resolve_child(
                                    r.group,
                                    r.kind,
                                    r.name,
                                    r.namespace,
                                    Some(r.version),
                                )
                            })
                            .collect();
                        (refs, None)
                    }
                    Err(e) => (Vec::new(), Some(e)),
                }
            };

            let children = join_all(refs.into_iter().map(|c| self.node_for_child(c, depth))).await;
            ResourceTreeNode {
                kind,
                group,
                name,
                namespace: ns,
                key: Some(key),
                category,
                status,
                children,
                error: err,
            }
        })
    }

    /// Turn a resolved child reference into a node: recurse if it's itself an
    /// owner kind (and its key resolved), otherwise it's a leaf.
    fn node_for_child(&self, child: ChildRef, depth: usize) -> BoxFuture<'_, ResourceTreeNode> {
        Box::pin(async move {
            if is_owner_kind(&child.group, &child.kind) {
                return match child.key {
                    Some(key) => {
                        self.build_owner_node(
                            key,
                            child.group,
                            child.kind,
                            child.category,
                            child.namespace,
                            child.name,
                            depth + 1,
                        )
                        .await
                    }
                    None => ResourceTreeNode {
                        kind: child.kind,
                        group: child.group,
                        name: child.name,
                        namespace: child.namespace,
                        key: None,
                        category: child.category,
                        status: None,
                        children: Vec::new(),
                        error: Some("kind not found in this cluster's catalog — cannot expand".into()),
                    },
                };
            }
            ResourceTreeNode {
                kind: child.kind,
                group: child.group,
                name: child.name,
                namespace: child.namespace,
                key: child.key,
                category: child.category,
                status: None,
                children: Vec::new(),
                error: None,
            }
        })
    }

    /// Parse a Kustomization's `status.inventory.entries[]` (already present on
    /// the object we just fetched — no extra API call) into resolved children.
    fn kustomization_children(&self, data: &Value) -> Result<Vec<ChildRef>, String> {
        let entries = data
            .get("status")
            .and_then(|s| s.get("inventory"))
            .and_then(|i| i.get("entries"))
            .and_then(|e| e.as_array())
            .ok_or_else(|| "Kustomization has no inventory yet (not reconciled?)".to_string())?;
        Ok(entries
            .iter()
            .filter_map(|e| e.get("id").and_then(|v| v.as_str()))
            .filter_map(parse_inventory_id)
            .map(|(namespace, name, group, kind)| self.resolve_child(group, kind, name, namespace, None))
            .collect())
    }

    /// Resolve `(group, kind[, version])` to a catalog entry (key + category),
    /// and correct the namespace for the case a resolved kind turns out to be
    /// cluster-scoped (a Helm-manifest leaf defaults its namespace from the
    /// release when a doc omits one — wrong for e.g. ClusterRole).
    fn resolve_child(
        &self,
        group: String,
        kind: String,
        name: String,
        namespace: Option<String>,
        version: Option<String>,
    ) -> ChildRef {
        let catalog = self.catalog.load();
        let entry = match &version {
            Some(v) => {
                let candidate = ResourceKind::make_key(&group, v, &kind);
                catalog.by_key.get(&candidate).cloned()
            }
            None => catalog
                .entries
                .iter()
                .find(|e| e.kind.group == group && e.kind.kind == kind)
                .cloned(),
        };
        let key = entry.as_ref().map(|e| e.kind.key.clone());
        let category = entry.as_ref().map(|e| e.kind.category.clone());
        let namespace = match &entry {
            Some(e) if !e.kind.namespaced => None,
            _ => namespace,
        };
        ChildRef {
            group,
            kind,
            name,
            namespace,
            key,
            category,
        }
    }
}

fn owner_error_node(
    group: String,
    kind: String,
    category: Option<Category>,
    name: String,
    ns: Option<String>,
    key: Option<String>,
    msg: String,
) -> ResourceTreeNode {
    ResourceTreeNode {
        kind,
        group,
        name,
        namespace: ns,
        key,
        category,
        status: None,
        children: Vec::new(),
        error: Some(msg),
    }
}

/// One entry from a Kustomization's `status.inventory.entries[]`, in cli-utils
/// `ObjMetadata` format: `<namespace>_<name>_<group>_<kind>` (namespace empty
/// for cluster-scoped → leading `_`; group empty for core/v1 → `__` before the
/// kind). Kubernetes object/domain names never contain `_`, so a plain 4-way
/// split on `_` is exact — no escaping ambiguity.
fn parse_inventory_id(id: &str) -> Option<(Option<String>, String, String, String)> {
    let parts: [&str; 4] = id.splitn(4, '_').collect::<Vec<_>>().try_into().ok()?;
    let [ns, name, group, kind] = parts;
    if name.is_empty() || kind.is_empty() {
        return None;
    }
    let namespace = (!ns.is_empty()).then(|| ns.to_string());
    Some((namespace, name.to_string(), group.to_string(), kind.to_string()))
}

#[cfg(test)]
mod inventory_id_tests {
    use super::parse_inventory_id;

    #[test]
    fn namespaced_with_group() {
        let (ns, name, group, kind) = parse_inventory_id("podinfo_podinfo_apps_Deployment").unwrap();
        assert_eq!(ns.as_deref(), Some("podinfo"));
        assert_eq!(name, "podinfo");
        assert_eq!(group, "apps");
        assert_eq!(kind, "Deployment");
    }

    #[test]
    fn namespaced_core_kind_has_empty_group() {
        let (ns, name, group, kind) = parse_inventory_id("infra_frontend__Service").unwrap();
        assert_eq!(ns.as_deref(), Some("infra"));
        assert_eq!(name, "frontend");
        assert_eq!(group, "");
        assert_eq!(kind, "Service");
    }

    #[test]
    fn cluster_scoped_with_group() {
        let (ns, name, group, kind) =
            parse_inventory_id("_flux-system_kustomize.toolkit.fluxcd.io_Kustomization").unwrap();
        assert_eq!(ns, None);
        assert_eq!(name, "flux-system");
        assert_eq!(group, "kustomize.toolkit.fluxcd.io");
        assert_eq!(kind, "Kustomization");
    }

    #[test]
    fn cluster_scoped_core_kind() {
        let (ns, name, group, kind) = parse_inventory_id("_infra-ns__Namespace").unwrap();
        assert_eq!(ns, None);
        assert_eq!(name, "infra-ns");
        assert_eq!(group, "");
        assert_eq!(kind, "Namespace");
    }

    #[test]
    fn malformed_ids_return_none() {
        assert!(parse_inventory_id("").is_none());
        assert!(parse_inventory_id("too_few_parts").is_none());
        assert!(parse_inventory_id("__too_short").is_none()); // 4 parts but empty name+kind
    }
}
```

- [ ] **Step 4: Verify it compiles and existing tests still pass**

Run: `direnv exec /home/krezh/repos/roder cargo test -p roder-k8s -p roder-core`
Expected: PASS — all pre-existing tests (including `inventory_id_tests::*`) still pass; no new tests were added in this task (category threading has no cluster-independent unit-testable surface, same reasoning as the original `resolve_child`/`build_owner_node`).

- [ ] **Step 5: Commit**

```bash
jj commit -m "feat(tree): thread category through resource-tree resolution"
```
(If the current jj change already has an in-progress description for this feature, use `jj describe` to extend it instead — check `jj status`/`jj log` first per project convention.)

---

## Task 2: Category icon system

**Files:**
- Modify: `src/app/components/icons.rs`

**Interfaces:**
- Consumes: `roder_core::Category` (Task 1's field type).
- Produces: `pub(crate) fn tree_icon_class(category: Option<&Category>) -> &'static str`, `pub(crate) fn tree_icon_glyph(category: Option<&Category>, kind: &str) -> &'static str`, `#[component] pub(crate) fn TreeKindIcon(category: Option<Category>, kind: String, small: bool) -> impl IntoView` — consumed by Task 4.

- [ ] **Step 1: Write the failing tests**

Append to `src/app/components/icons.rs`:

```rust
#[cfg(test)]
mod tree_icon_tests {
    use super::*;
    use roder_core::Category;

    #[test]
    fn flux_kustomization_and_helmrelease_share_color_but_differ_in_glyph() {
        assert_eq!(tree_icon_class(Some(&Category::Flux)), "cat-flux");
        assert_ne!(
            tree_icon_glyph(Some(&Category::Flux), "Kustomization"),
            tree_icon_glyph(Some(&Category::Flux), "HelmRelease"),
        );
    }

    #[test]
    fn secret_bearing_kinds_get_the_lock_glyph_regardless_of_category() {
        let lock = tree_icon_glyph(Some(&Category::Config), "Secret");
        assert_eq!(lock, tree_icon_glyph(Some(&Category::CertManager), "Certificate"));
        assert_eq!(lock, tree_icon_glyph(Some(&Category::ExternalSecrets), "ExternalSecret"));
        assert_ne!(lock, tree_icon_glyph(Some(&Category::Config), "ConfigMap"));
    }

    #[test]
    fn none_category_falls_back() {
        assert_eq!(tree_icon_class(None), "cat-fallback");
    }

    #[test]
    fn custom_category_falls_back() {
        assert_eq!(tree_icon_class(Some(&Category::Custom("example.com".into()))), "cat-fallback");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `direnv exec /home/krezh/repos/roder cargo test -p roder --features hydrate --lib tree_icon_tests`
Expected: FAIL with "cannot find function `tree_icon_class`" (doesn't exist yet).

- [ ] **Step 3: Implement**

Replace the whole file `src/app/components/icons.rs` with:

```rust
use leptos::prelude::*;
use roder_core::Category;

/// Shift key arrow icon (inline SVG, no external deps).
#[component]
pub(crate) fn ShiftIcon() -> impl IntoView {
    view! {
        <svg class="key-shift" viewBox="0 0 10 10" fill="currentColor" aria-hidden="true">
            <path d="M5 0L10 5H6.5V10H3.5V5H0Z" />
        </svg>
    }
}

/// CSS class selecting the icon chip's background/foreground color pair for
/// a resource's sidebar `Category` — see `style/_tree.scss` for the `.cat-*`
/// rules. Reuses the same category taxonomy as the sidebar rather than a
/// separate per-Kind color system (roder's first per-kind iconography is
/// deliberately scoped to ~11 category buckets, not hundreds of kinds).
pub(crate) fn tree_icon_class(category: Option<&Category>) -> &'static str {
    match category {
        Some(Category::Flux) => "cat-flux",
        Some(Category::Workloads) => "cat-workloads",
        Some(Category::Network) => "cat-network",
        Some(Category::Config) => "cat-config",
        Some(Category::Rbac) => "cat-rbac",
        Some(Category::Storage) => "cat-storage",
        Some(Category::ExternalSecrets) => "cat-externalsecrets",
        Some(Category::CertManager) => "cat-certmanager",
        Some(Category::Rook) => "cat-rook",
        Some(Category::Cluster) => "cat-cluster",
        Some(Category::Custom(_)) | None => "cat-fallback",
    }
}

/// Glyph for a resource's icon chip. Mostly per-Category, with a few
/// kind-level overrides where the shape carries real meaning even though the
/// color is shared: Kustomization vs HelmRelease (both `Category::Flux`), and
/// anything that holds credentials (Secret/Certificate/ExternalSecret) always
/// gets the lock glyph regardless of which category it lives in.
pub(crate) fn tree_icon_glyph(category: Option<&Category>, kind: &str) -> &'static str {
    if matches!(kind, "Secret" | "Certificate" | "ExternalSecret") {
        return "\u{1F512}"; // 🔒
    }
    match (category, kind) {
        (Some(Category::Flux), "HelmRelease") => "\u{25C6}", // ◆
        (Some(Category::Flux), _) => "\u{25A3}",             // ▣
        (Some(Category::Workloads), _) => "\u{25A2}",        // ▢
        (Some(Category::Network), _) => "\u{21C4}",          // ⇄
        (Some(Category::Config), _) => "\u{25A4}",           // ▤
        (Some(Category::Rbac), _) => "\u{25C8}",             // ◈
        (Some(Category::Storage), _) => "\u{26C1}",          // ⛁
        (Some(Category::CertManager), _) => "\u{1F512}",     // 🔒
        (Some(Category::ExternalSecrets), _) => "\u{1F512}", // 🔒
        (Some(Category::Cluster), _) => "\u{2B21}",          // ⬡
        (Some(Category::Rook), _) => "\u{2B22}",             // ⬢
        _ => "\u{25CF}",                                     // ●
    }
}

/// Small colored chip showing a resource's category-derived icon. Used by the
/// Resource Tree; `small` selects the leaf-chip size (16px) vs. the owner-card
/// size (20px) — see `.tree-icon`/`.tree-icon-sm` in `style/_tree.scss`.
#[component]
pub(crate) fn TreeKindIcon(category: Option<Category>, kind: String, small: bool) -> impl IntoView {
    let class = tree_icon_class(category.as_ref());
    let glyph = tree_icon_glyph(category.as_ref(), &kind);
    let size_class = if small { "tree-icon tree-icon-sm" } else { "tree-icon" };
    view! { <span class=format!("{size_class} {class}")>{glyph}</span> }
}

#[cfg(test)]
mod tree_icon_tests {
    use super::*;
    use roder_core::Category;

    #[test]
    fn flux_kustomization_and_helmrelease_share_color_but_differ_in_glyph() {
        assert_eq!(tree_icon_class(Some(&Category::Flux)), "cat-flux");
        assert_ne!(
            tree_icon_glyph(Some(&Category::Flux), "Kustomization"),
            tree_icon_glyph(Some(&Category::Flux), "HelmRelease"),
        );
    }

    #[test]
    fn secret_bearing_kinds_get_the_lock_glyph_regardless_of_category() {
        let lock = tree_icon_glyph(Some(&Category::Config), "Secret");
        assert_eq!(lock, tree_icon_glyph(Some(&Category::CertManager), "Certificate"));
        assert_eq!(lock, tree_icon_glyph(Some(&Category::ExternalSecrets), "ExternalSecret"));
        assert_ne!(lock, tree_icon_glyph(Some(&Category::Config), "ConfigMap"));
    }

    #[test]
    fn none_category_falls_back() {
        assert_eq!(tree_icon_class(None), "cat-fallback");
    }

    #[test]
    fn custom_category_falls_back() {
        assert_eq!(tree_icon_class(Some(&Category::Custom("example.com".into()))), "cat-fallback");
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `direnv exec /home/krezh/repos/roder cargo test -p roder --features hydrate --lib tree_icon_tests`
Expected: PASS (4 tests).

Note: `icons.rs` isn't wired into any `mod.rs` export list yet for `TreeKindIcon`/`tree_icon_class`/`tree_icon_glyph` beyond `pub(crate)` visibility — confirm `src/app/components/mod.rs` already does `pub(crate) mod icons;` or `pub(crate) use icons::...` (it does, since `ShiftIcon` is already consumed elsewhere); no change needed there.

- [ ] **Step 5: Commit**

```bash
jj commit -m "feat(tree): add category-based icon chip for the resource tree"
```

---

## Task 3: Pure tree-shaping helpers — `group_children` and `descendant_count`

**Files:**
- Modify: `src/app/overlays/tree.rs` (additive — these live alongside the existing component code that Task 4 will rewrite)

**Interfaces:**
- Consumes: `roder_core::ResourceTreeNode` (existing).
- Produces: `pub(crate) enum ChildGroup { Leaves(Vec<ResourceTreeNode>), Owner(ResourceTreeNode) }`, `pub(crate) fn group_children(children: Vec<ResourceTreeNode>) -> Vec<ChildGroup>`, `pub(crate) fn descendant_count(node: &ResourceTreeNode) -> usize` — consumed by Task 4.

- [ ] **Step 1: Write the failing tests**

Add near the bottom of `src/app/overlays/tree.rs` (before the final closing of the file):

```rust
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
            status: Some(roder_core::RowStatus::Ok),
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
        // [Leaves(a,b), Owner(o1), Leaves(c), Owner(o2), Owner(o3), Leaves(d)]
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
        let node = owner(
            "x",
            vec![leaf("a"), owner("y", vec![leaf("b"), leaf("c")])],
        );
        // a, y, b, c = 4
        assert_eq!(descendant_count(&node), 4);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `direnv exec /home/krezh/repos/roder cargo test -p roder --features hydrate --lib shaping_tests`
Expected: FAIL with "cannot find enum `ChildGroup`" / "cannot find function `group_children`" / "cannot find function `descendant_count`".

- [ ] **Step 3: Implement**

Add to `src/app/overlays/tree.rs` (near the top, after the `use` statements — Task 4 will reorganize the file further, this task only needs the helpers to exist and compile):

```rust
/// A run of consecutive same-shape siblings, in original order: leaf
/// resources (which wrap several-per-line) get batched together; each owner
/// (Kustomization/HelmRelease, which needs its own full-width card) stands
/// alone. "Owner" is exactly `status.is_some()` — the backend only sets a
/// status on Kustomization/HelmRelease nodes.
pub(crate) enum ChildGroup {
    Leaves(Vec<ResourceTreeNode>),
    Owner(ResourceTreeNode),
}

pub(crate) fn group_children(children: Vec<ResourceTreeNode>) -> Vec<ChildGroup> {
    let mut groups = Vec::new();
    let mut pending_leaves = Vec::new();
    for child in children {
        if child.status.is_some() {
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
    node.children.len()
        + node
            .children
            .iter()
            .map(descendant_count)
            .sum::<usize>()
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `direnv exec /home/krezh/repos/roder cargo test -p roder --features hydrate --lib shaping_tests`
Expected: PASS (7 tests). Also run `direnv exec /home/krezh/repos/roder cargo clippy -p roder --no-default-features --features hydrate -- -D warnings` to confirm no unused-code warnings slip through (these helpers are unused until Task 4 wires them in, which will trip `dead_code` — see note below).

Note: since Task 4 immediately consumes both helpers, a transient `dead_code` warning between Task 3 and Task 4 is expected and fine — don't run the full `devenv test` gate until Task 4 is also done, since `-D warnings` will fail on it otherwise. Commit anyway; the working tree is expected to be momentarily non-green mid-feature, same as any multi-commit TDD sequence.

- [ ] **Step 5: Commit**

```bash
jj commit -m "feat(tree): add group_children/descendant_count helpers"
```

---

## Task 4: Rewrite the tree rendering — cards, chips, collapse, toolbar

**Files:**
- Modify: `src/app/overlays/tree.rs` (full rewrite of the component layer; keeps Task 3's helpers)

**Interfaces:**
- Consumes: `group_children`/`ChildGroup`/`descendant_count` (Task 3), `TreeKindIcon`/`tree_icon_class` (Task 2), `roder_core::{ResourceTreeNode, RowStatus}`, existing `DetailTarget`/`TreeOpen` state, existing `crate::app::util::color::dot_class`-equivalent status→CSS-class mapping (reused for the owner card's border color).
- Produces: `ResourceTreeWindow` (unchanged export name — still mounted from `src/app/mod.rs`, no changes needed there).

**Behavior change (documented in the spec, flagged here so it isn't mistaken for a bug during review):** owner cards no longer open the detail drawer on click — the whole card's click toggles expand/collapse instead, matching every reviewed mockup. Only leaf chips open the drawer on click. This is intentional, not a regression to fix.

- [ ] **Step 1: Replace the whole file**

```rust
//! Modal window showing the full recursive ownership tree of a Kustomization/
//! HelmRelease (resolved in one shot by `/api/resource-tree`). Kustomization/
//! HelmRelease ("owner") nodes render as fixed-width status-bordered cards,
//! collapsible (root expanded by default, everything else collapsed); leaf
//! resources render as compact chips that wrap several-per-line. See
//! `docs/superpowers/specs/2026-07-02-resource-tree-visual-design.md`.

use leptos::prelude::*;
use roder_core::{ResourceTreeNode, RowStatus};

use crate::app::components::icons::TreeKindIcon;
use crate::app::overlays::use_option_overlay;
use crate::app::state::{DetailTarget, TreeOpen};
use crate::data;

/// A run of consecutive same-shape siblings, in original order: leaf
/// resources (which wrap several-per-line) get batched together; each owner
/// (Kustomization/HelmRelease, which needs its own full-width card) stands
/// alone. "Owner" is exactly `status.is_some()` — the backend only sets a
/// status on Kustomization/HelmRelease nodes.
pub(crate) enum ChildGroup {
    Leaves(Vec<ResourceTreeNode>),
    Owner(ResourceTreeNode),
}

pub(crate) fn group_children(children: Vec<ResourceTreeNode>) -> Vec<ChildGroup> {
    let mut groups = Vec::new();
    let mut pending_leaves = Vec::new();
    for child in children {
        if child.status.is_some() {
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
    node.children.len()
        + node
            .children
            .iter()
            .map(descendant_count)
            .sum::<usize>()
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

#[component]
pub(crate) fn ResourceTreeWindow() -> impl IntoView {
    let tree_open = expect_context::<TreeOpen>().0;
    let (snapshot, closing, do_close) = use_option_overlay(tree_open);

    view! {
        <Show when=move || snapshot.get().is_some()>
            <div class="tree-scrim" class:closing=move || closing.get() on:click=move |_| do_close()></div>
            <div class="tree-window" class:closing=move || closing.get()>
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
                t.namespace.as_deref().map(data::percent_encode).unwrap_or_default(),
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

    view! {
        <div class="tree-head">
            <span class="tree-title">"Resource Tree — " {title}</span>
            <button class="tree-close" on:click=move |_| do_close()>"✕"</button>
        </div>
        <div class="tree-toolbar">
            <button on:click=move |_| fire(true)>"Expand all"</button>
            <button on:click=move |_| fire(false)>"Collapse all"</button>
        </div>
        <div class="tree-body">
            {move || match root.get() {
                None => view! { <div class="tree-status">"Resolving tree…"</div> }.into_any(),
                Some(None) => view! { <div class="tree-status tree-err">"Failed to load resource tree."</div> }.into_any(),
                Some(Some(node)) => view! { <OwnerCard node=node is_root=true /> }.into_any(),
            }}
        </div>
    }
}

/// An owner (Kustomization/HelmRelease) card: icon, name, kind/namespace,
/// status-colored border, and a trailer that's either just a chevron
/// (expanded) or a descendant-count badge + chevron (collapsed). Clicking the
/// card toggles its own children; clicking a leaf's name (in `LeafChip`)
/// opens the detail drawer instead — cards themselves aren't drawer-clickable
/// since their primary click action is expand/collapse.
///
/// Returns `AnyView` (not `impl IntoView`): this component's children can
/// recurse back into `OwnerCard` (via `group_children`), and an opaque
/// `impl IntoView` return type can't participate in that recursion (E0720).
#[component]
fn OwnerCard(node: ResourceTreeNode, is_root: bool) -> AnyView {
    let cmd = expect_context::<TreeExpandCommand>();
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
    let children = node.children;
    let groups = group_children(children);

    view! {
        <div class=format!("tree-owner-card {border_class}") on:click=move |_| expanded.update(|e| *e = !*e)>
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
        ChildGroup::Owner(node) => view! { <OwnerCard node=node.clone() is_root=false /> }.into_any(),
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
/// opens the resource in the (separate, right-docked) detail drawer; the tree
/// window stays open. Structurally identical to `OwnerCard`'s text block
/// (name above kind, same classes/sizes) but with the smaller icon and no
/// border/trailer — leaves carry no live status, by design (see spec).
#[component]
fn LeafChip(node: ResourceTreeNode) -> impl IntoView {
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let clickable = node.key.is_some();
    let open = {
        let key = node.key.clone();
        let ns = node.namespace.clone();
        let name = node.name.clone();
        move |_| {
            if let Some(key) = key.clone() {
                detail.set(Some(DetailTarget {
                    key,
                    namespace: ns.clone(),
                    name: name.clone(),
                }));
            }
        }
    };
    view! {
        <div class="tree-leaf-chip" class:tree-leaf-disabled=!clickable on:click=open>
            <TreeKindIcon category=node.category kind=node.kind.clone() small=true />
            <div class="tree-owner-text">
                <div class="tree-name">{node.name}</div>
                <div class="tree-kind-line">{node.kind}</div>
            </div>
            {(!clickable).then(|| view! { <span class="tooltip">"Kind not found in this cluster's catalog"</span> })}
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
        let node = owner(
            "x",
            vec![leaf("a"), owner("y", vec![leaf("b"), leaf("c")])],
        );
        assert_eq!(descendant_count(&node), 4);
    }
}
```

- [ ] **Step 2: Run the shaping tests again (should still pass — same tests, now exercised through the real module)**

Run: `direnv exec /home/krezh/repos/roder cargo test -p roder --features hydrate --lib shaping_tests`
Expected: PASS (7 tests).

- [ ] **Step 3: Type-check the ssr and hydrate feature sets**

Run: `direnv exec /home/krezh/repos/roder cargo check -p roder --no-default-features --features hydrate`
Run: `direnv exec /home/krezh/repos/roder cargo check -p roder --features ssr`
Expected: both PASS. If `crate::app::components::icons::TreeKindIcon` fails to resolve, check `src/app/components/mod.rs` exposes `icons` as `pub(crate) mod icons;` (it already does per Task 2's note) and that `TreeKindIcon` itself is `pub(crate)` (it is, per Task 2's code).

- [ ] **Step 4: Commit**

```bash
jj commit -m "feat(tree): replace ASCII branch rendering with cards, chips, and collapse"
```

---

## Task 5: Rewrite `style/_tree.scss`

**Files:**
- Modify: `style/_tree.scss` (full rewrite)

**Interfaces:**
- Consumes: class names produced by Task 4's markup (`tree-toolbar`, `tree-owner-card`, `status-{ok,warn,error,pending,unknown}`, `tree-owner-text`, `tree-name`, `tree-kind-line`, `tree-trailer`, `tree-count`, `tree-chevron`, `tree-branch`, `tree-leaf-flow`, `tree-leaf-chip`, `tree-leaf-disabled`, `tree-err-text`) and Task 2's icon classes (`tree-icon`, `tree-icon-sm`, `cat-flux`, `cat-workloads`, `cat-network`, `cat-config`, `cat-rbac`, `cat-storage`, `cat-externalsecrets`, `cat-certmanager`, `cat-rook`, `cat-cluster`, `cat-fallback`).
- Produces: none (leaf of the dependency graph).

- [ ] **Step 1: Replace the whole file**

```scss
/* ---- resource tree modal ---- */
.tree-window {
  position: fixed;
  inset: 0;
  margin: auto;
  z-index: 65;
  width: min(70vw, 980px);
  height: 74vh;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.6);
  overflow: hidden;
  animation: overlay-in 0.28s cubic-bezier(0.34, 1.56, 0.64, 1);
}
.tree-scrim {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  z-index: 64;
  animation: scrim-in 0.15s ease;
}
.tree-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
  padding: 0.55rem 1rem;
  border-bottom: 1px solid var(--border);
  flex: 0 0 auto;
}
.tree-title {
  font-weight: 600;
  font-size: 0.88rem;
}
.tree-close {
  background: none;
  border: none;
  color: var(--muted);
  cursor: pointer;
  font-size: 1rem;
  flex: 0 0 auto;
  &:hover {
    color: var(--error);
  }
}
.tree-toolbar {
  display: flex;
  gap: 0.4rem;
  padding: 0.5rem 1rem;
  border-bottom: 1px solid var(--border);
  flex: 0 0 auto;
  button {
    background: var(--panel-2);
    border: 1px solid var(--border);
    color: var(--muted);
    font-size: 0.72rem;
    padding: 0.2rem 0.6rem;
    border-radius: 6px;
    cursor: pointer;
    &:hover {
      color: var(--fg);
      border-color: var(--accent);
    }
  }
}
.tree-body {
  flex: 1 1 0;
  overflow: auto;
  padding: 0.75rem;
}
.tree-status {
  padding: 1rem;
  color: var(--muted);
  text-align: center;
}
.tree-status.tree-err {
  color: var(--error);
}

/* ---- icon chip: category color (background/foreground) ---- */
.tree-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  border-radius: 6px;
  font-size: 11px;
  flex-shrink: 0;
  line-height: 1;
}
.tree-icon-sm {
  width: 16px;
  height: 16px;
  font-size: 10px;
}
.cat-flux { background: #1f3a5f; color: #79c0ff; }
.cat-workloads { background: #2a2440; color: #b083f0; }
.cat-network { background: #3a2f1f; color: #e0a84c; }
.cat-config { background: #1b212b; color: #8b949e; }
.cat-rbac { background: #3a1f2a; color: #f27a9e; }
.cat-storage { background: #1a2f2a; color: #5ce0a8; }
.cat-externalsecrets { background: #2f2a1a; color: #e0c25c; }
.cat-certmanager { background: #1f2f3a; color: #5cc8e0; }
.cat-cluster { background: #2a1f3a; color: #a888e0; }
.cat-rook { background: #1f2a3a; color: #7ab8e0; }
.cat-fallback { background: #1b212b; color: #6e7681; }

/* ---- owner card (Kustomization/HelmRelease) ---- */
.tree-owner-card {
  width: 280px;
  box-sizing: border-box;
  background: var(--panel-2);
  border: 1.5px solid var(--border);
  border-radius: 9px;
  padding: 6px 9px;
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
  cursor: pointer;
  &:hover {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  &.status-ok { border-color: color-mix(in srgb, var(--ok) 55%, var(--border)); }
  &.status-warn { border-color: color-mix(in srgb, var(--warn) 55%, var(--border)); }
  &.status-error { border-color: color-mix(in srgb, var(--error) 55%, var(--border)); }
  &.status-pending { border-color: color-mix(in srgb, var(--pending) 55%, var(--border)); }
  &.status-unknown { border-color: var(--border); }
}
.tree-owner-text {
  min-width: 0;
  flex: 1 1 auto;
}
.tree-name {
  font-size: 0.8rem;
  font-weight: 600;
  color: var(--fg);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tree-kind-line {
  font-size: 0.66rem;
  color: var(--muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tree-trailer {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}
.tree-chevron {
  color: var(--muted);
  font-size: 0.7rem;
}
.tree-count {
  font-size: 0.62rem;
  font-weight: 700;
  color: var(--fg-dim, var(--muted));
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 1px 7px;
}

/* ---- indentation for an expanded owner's children ---- */
.tree-branch {
  margin-left: 20px;
  border-left: 2px solid var(--border);
  padding-left: 14px;
}
.tree-err-text {
  color: var(--warn);
  font-style: italic;
  font-size: 0.78rem;
  padding: 2px 0 6px;
}

/* ---- leaf chips: content-sized, wrap several per line ---- */
.tree-leaf-flow {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 8px;
}
.tree-leaf-chip {
  width: auto;
  flex: 0 0 auto;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--panel-2);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 4px 9px;
  cursor: pointer;
  &:hover {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
}
.tree-leaf-disabled {
  cursor: default;
  opacity: 0.5;
  position: relative;
  &:hover {
    border-color: var(--border);
  }
  .tooltip {
    display: none;
    position: absolute;
    top: 100%;
    left: 0.5rem;
    margin-top: 4px;
  }
  &:hover .tooltip {
    display: block;
  }
}
```

- [ ] **Step 2: Add to `style/main.scss` and check the closing-animation selectors in `style/_modals.scss` already include `.tree-window`/`.tree-scrim`**

`@use "tree";` and the `.tree-window.closing`/`.tree-scrim.closing` rules were already added when the Resource Tree feature was first built — confirm with:

Run: `grep -n "tree" /home/krezh/repos/roder/style/main.scss /home/krezh/repos/roder/style/_modals.scss`
Expected: `main.scss` has `@use "tree";`; `_modals.scss` has `.tree-window.closing` and `.tree-scrim.closing` in the shared selector lists. No changes needed if so.

- [ ] **Step 3: Commit**

```bash
jj commit -m "style(tree): card/chip/toolbar styling for the resource tree"
```

---

## Task 6: Full verification

**Files:** none (verification only)

- [ ] **Step 1: Run the authoritative test suite**

Run: `devenv test`
Expected: PASS — fmt clean, clippy `-D warnings` clean on both `ssr` and `hydrate` feature sets, all unit tests pass (including the 4 new `tree_icon_tests` and 7 `shaping_tests`), docker build succeeds.

If `cargo fmt --all -- --check` fails (expected the first time, given the amount of hand-formatted code in this plan): run `direnv exec /home/krezh/repos/roder cargo fmt --all`, review the diff is whitespace-only, then re-run `devenv test`.

- [ ] **Step 2: Manual verification against the live cluster**

Start the dev server if not already running: `devenv processes list` (if `dev` isn't `ready`, run `devenv up` in the background or `devenv processes start dev`). Confirm it's serving: `devenv processes logs dev` should show `roder listening on http://0.0.0.0:8080` and `connected to cluster at startup, N kinds discovered`.

Using Playwright MCP (`browser_navigate` to `http://localhost:8080`, `browser_click`/`browser_type` to search "Kustomization", right-click a Kustomization row with known nested children — e.g. `cluster-apps` in `flux-system` if using the same cluster as the original design review — click "Resource Tree", `browser_take_screenshot` targeting `.tree-window`), confirm:

1. The root row is expanded automatically; its direct children (both leaf chips and owner cards) are visible.
2. Every owner card (Kustomization/HelmRelease) below the root starts collapsed, showing a descendant-count badge and a `▸` chevron.
3. Clicking a collapsed owner card expands it (chevron flips to `▾`, count badge disappears, children appear indented below).
4. Leaf resources render as compact chips, several per line, wrapping — not one-per-line.
5. Icon chips are colored distinctly per category (Flux blue, Workloads purple, Network amber, Config grey, Secret-bearing kinds show the lock glyph even inside Config/CertManager/ExternalSecrets categories).
6. "Expand all" expands every card in the tree at once; "Collapse all" collapses every card (including the root) at once.
7. Clicking a leaf chip's name opens it in the right-docked detail drawer, with the tree window staying open on top (same behavior as before this overhaul — regression check).
8. A node with `error: Some(...)` (e.g. temporarily point at a HelmRelease with no deployed revision, or just confirm the message renders correctly for whatever real error case exists in the cluster) shows the error text in place of that node's children, without breaking the rest of the tree.

Report back with a screenshot and a short pass/fail summary against this list — this replaces the original spec's now-superseded ASCII-tree verification steps.

- [ ] **Step 3: Clean up any screenshots taken outside the scratchpad directory**

If verification screenshots were saved into the repo working tree, remove them before the final commit:

Run: `jj status` and confirm no stray `.png`/temp files are staged; if any appear, `rm` them.

- [ ] **Step 4: Final commit**

```bash
jj commit -m "test(tree): verify resource-tree visual overhaul against live cluster"
```

(If Step 1-2 required no code changes — i.e. everything passed first try — this step may have nothing to commit; skip it in that case.)
