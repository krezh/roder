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

/// Identity of an owner (Kustomization/HelmRelease) node to expand, bundled
/// so `build_owner_node` stays under clippy's arg-count limit.
struct OwnerRef {
    key: String,
    group: String,
    kind: String,
    category: Option<Category>,
    ns: Option<String>,
    name: String,
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
        // Bounds how many owner-node fetches (each a live apiserver GET, or a
        // recursive fan-out of them) are in flight at once for this one tree
        // resolution — it does NOT bound total work, just concurrency, so a
        // wide tree can't burst dozens of simultaneous requests at the API
        // server.
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        Ok(self
            .build_owner_node(
                OwnerRef {
                    key: key.to_string(),
                    group: entry.kind.group.clone(),
                    kind: entry.kind.kind.clone(),
                    category: Some(entry.kind.category.clone()),
                    ns: ns.map(str::to_string),
                    name: name.to_string(),
                },
                0,
                semaphore,
            )
            .await)
    }

    /// Fetch + expand one Kustomization/HelmRelease node: the live object
    /// (cache-first, same as `detail()`) for its status dot, then its children
    /// (inventory or Helm manifest), recursing into any child that's itself an
    /// owner kind. Boxed because async fns can't recurse directly in Rust.
    fn build_owner_node(
        &self,
        owner: OwnerRef,
        depth: usize,
        semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    ) -> BoxFuture<'_, ResourceTreeNode> {
        let OwnerRef {
            key,
            group,
            kind,
            category,
            ns,
            name,
        } = owner;
        Box::pin(async move {
            let obj = match self
                .registry
                .cached_object(&key, ns.as_deref(), &name)
                .await
            {
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

            let children = join_all(
                refs.into_iter()
                    .map(|c| self.node_for_child(c, depth, semaphore.clone())),
            )
            .await;
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
    fn node_for_child(
        &self,
        child: ChildRef,
        depth: usize,
        semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    ) -> BoxFuture<'_, ResourceTreeNode> {
        Box::pin(async move {
            if is_owner_kind(&child.group, &child.kind) {
                return match child.key {
                    Some(key) => {
                        // Acquire a permit before issuing the network call(s)
                        // this recursive step triggers; held for the duration
                        // of the whole subtree fetch so the bound applies
                        // cluster-wide to this one tree resolution, not just
                        // to the immediate GET.
                        let _permit = semaphore
                            .acquire()
                            .await
                            .expect("semaphore never closed");
                        self.build_owner_node(
                            OwnerRef {
                                key,
                                group: child.group,
                                kind: child.kind,
                                category: child.category,
                                ns: child.namespace,
                                name: child.name,
                            },
                            depth + 1,
                            semaphore.clone(),
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
                        error: Some(
                            "kind not found in this cluster's catalog — cannot expand".into(),
                        ),
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
            .map(|(namespace, name, group, kind)| {
                self.resolve_child(group, kind, name, namespace, None)
            })
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
    Some((
        namespace,
        name.to_string(),
        group.to_string(),
        kind.to_string(),
    ))
}

#[cfg(test)]
mod inventory_id_tests {
    use super::parse_inventory_id;

    #[test]
    fn namespaced_with_group() {
        let (ns, name, group, kind) =
            parse_inventory_id("podinfo_podinfo_apps_Deployment").unwrap();
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
