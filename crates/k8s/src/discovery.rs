use kube::discovery::{Discovery, Scope};
use kube::Client;
use roder_core::{Category, ResourceKind};

use crate::client::K8sError;
use crate::printer_columns::{self, ColumnMap};
use crate::project::columns_for;

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
pub async fn build_catalog(
    client: &Client,
    columns: &ColumnMap,
) -> Result<Vec<CatalogEntry>, K8sError> {
    let discovery = Discovery::new(client.clone())
        .run()
        .await
        .map_err(|e| K8sError::Api(format!("discovery failed: {e}")))?;

    let mut out = Vec::new();
    for group in discovery.groups() {
        for (ar, caps) in group.recommended_resources() {
            // Only types we can list (skipping subresources, write-only, etc.).
            if !caps.operations.iter().any(|op| op == "list") {
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
                    columns: columns_for(
                        &ar.group,
                        &ar.kind,
                        printer_columns::cols_for(columns, &ar.group, &ar.kind),
                    ),
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
    if group == "cert-manager.io" || group.ends_with(".cert-manager.io") || group == "acme.cert-manager.io" {
        return Category::CertManager;
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
            "ConfigMap" | "Secret" | "ResourceQuota" | "LimitRange" | "ServiceAccount" => {
                if kind == "ServiceAccount" {
                    Category::Rbac
                } else {
                    Category::Config
                }
            }
            "Service" | "Endpoints" => Category::Network,
            "PersistentVolumeClaim" | "PersistentVolume" => Category::Storage,
            "Node" | "Namespace" | "Event" | "ComponentStatus" => Category::Cluster,
            _ => Category::Cluster,
        };
    }

    Category::Custom
}
