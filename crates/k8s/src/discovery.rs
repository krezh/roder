use kube::discovery::{Discovery, Scope};
use kube::Client;
use roder_core::{Category, ResourceKind};

use crate::client::K8sError;

/// Internal catalog entry: the public [`ResourceKind`] plus the bits needed to
/// build a dynamic `Api` (the `kube::core::ApiResource`).
#[derive(Clone)]
pub struct CatalogEntry {
    pub kind: ResourceKind,
    pub api_resource: kube::core::ApiResource,
}

/// Enumerate every listable resource type in the cluster (core, extensions, and
/// every installed CRD) via the discovery API. This is how roder browses any
/// resource/CRD without per-type code.
pub async fn build_catalog(client: &Client) -> Result<Vec<CatalogEntry>, K8sError> {
    let discovery = Discovery::new(client.clone())
        .run()
        .await
        .map_err(|e| K8sError::Api(format!("discovery failed: {e}")))?;

    let mut out = Vec::new();
    for group in discovery.groups() {
        for (ar, caps) in group.recommended_resources() {
            // Only types we can list AND watch. Skipping subresources,
            // write-only resources, and metrics-style resources (e.g. PodMetrics
            // from metrics-server) that list but don't watch
            let ops = &caps.operations;
            if !ops.iter().any(|op| op == "list") || !ops.iter().any(|op| op == "watch") {
                continue;
            }
            let namespaced = matches!(caps.scope, Scope::Namespaced);
            let category = classify(&ar.group, &ar.kind);
            let key = ResourceKind::make_key(&ar.group, &ar.version, &ar.kind);

            out.push(CatalogEntry {
                kind: ResourceKind {
                    key,
                    group: ar.group.clone(),
                    version: ar.version.clone(),
                    kind: ar.kind.clone(),
                    plural: ar.plural.clone(),
                    namespaced,
                    category,
                },
                api_resource: ar,
            });
        }
    }

    out.sort_by(|a, b| {
        a.kind
            .category
            .order()
            .cmp(&b.kind.category.order())
            // Within dynamic categories, sort by the group label (domain) so each
            // operator's kinds are grouped contiguously.
            .then_with(|| a.kind.category.label().cmp(&b.kind.category.label()))
            .then_with(|| a.kind.kind.cmp(&b.kind.kind))
    });
    Ok(out)
}

/// Map a (group, kind) to a sidebar category.
fn classify(group: &str, kind: &str) -> Category {
    // Group-based first.
    if group.ends_with("fluxcd.io") {
        return Category::Flux;
    }
    if group == "external-secrets.io" || group.ends_with(".external-secrets.io") {
        return Category::ExternalSecrets;
    }
    if group == "cert-manager.io"
        || group.ends_with(".cert-manager.io")
        || group == "acme.cert-manager.io"
    {
        return Category::CertManager;
    }
    // Rook bundles several API groups (ceph.rook.io plus the object-bucket
    // provisioner's objectbucket.io); collapse them all into one section, the
    // same way every `*.fluxcd.io` group folds into Flux above.
    if group == "ceph.rook.io"
        || group.ends_with(".rook.io")
        || group == "objectbucket.io"
        || group.ends_with(".objectbucket.io")
        || group.ends_with(".ceph.io")
    {
        return Category::Rook;
    }
    if group == "cnpg.io" || group.ends_with(".cnpg.io") {
        return Category::CloudNativePg;
    }
    if group == "rbac.authorization.k8s.io" {
        return Category::Rbac;
    }
    if group == "storage.k8s.io" {
        return Category::Storage;
    }
    if group == "networking.k8s.io" || group == "gateway.networking.k8s.io" {
        return Category::Network;
    }
    if group == "apps" || group == "batch" {
        return Category::Workloads;
    }

    // Core group: classify by kind.
    if group.is_empty() {
        return match kind {
            "Pod" | "ReplicationController" => Category::Workloads,
            "ConfigMap" | "Secret" | "ResourceQuota" | "LimitRange" => Category::Config,
            "ServiceAccount" => Category::Rbac,
            "Service" | "Endpoints" => Category::Network,
            "PersistentVolumeClaim" | "PersistentVolume" => Category::Storage,
            _ => Category::Cluster,
        };
    }

    Category::Custom(group_base_domain(group))
}

/// Derive a short, human-readable label from a CRD API group by keeping only
/// the registrable domain (last two dot-separated components).
///
/// Examples: `monitoring.coreos.com` → `coreos.com`, `kyverno.io` → `kyverno.io`
fn group_base_domain(group: &str) -> String {
    if let Some((prefix, last)) = group.rsplit_once('.') {
        if let Some((_, second)) = prefix.rsplit_once('.') {
            return format!("{second}.{last}");
        }
    }
    group.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_rook_api_groups_together() {
        assert_eq!(classify("ceph.rook.io", "CephCluster"), Category::Rook);
        assert_eq!(
            classify("objectbucket.io", "ObjectBucketClaim"),
            Category::Rook
        );
    }

    #[test]
    fn classifies_cnpg_api_groups_together() {
        assert_eq!(
            classify("postgresql.cnpg.io", "Cluster"),
            Category::CloudNativePg
        );
        assert_eq!(
            classify("barmancloud.cnpg.io", "ObjectStore"),
            Category::CloudNativePg
        );
    }
}
