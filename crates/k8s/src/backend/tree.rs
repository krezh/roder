//! Bounded relationship trees for arbitrary Kubernetes resources. The tree
//! follows owner references upward, known controller ownership downward, and
//! selector relationships for workloads and Services. Flux inventory and Helm
//! manifest expansion remain available through the same endpoint.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::future::{join_all, BoxFuture};
use kube::api::{DynamicObject, ListParams};
use roder_core::{Category, ResourceTreeNode, ResourceTreeRelation};
use serde_json::Value;

use super::{api_err, Backend};
use crate::client::K8sError;

const MAX_DEPTH: usize = 12;
const MAX_NODES: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    All,
    Owners,
    Descendants,
}

#[derive(Clone)]
struct ResourceRef {
    group: String,
    version: String,
    kind: String,
    name: String,
    namespace: Option<String>,
    key: Option<String>,
    category: Option<Category>,
    relation: Option<ResourceTreeRelation>,
    expandable: bool,
}

impl ResourceRef {
    fn identity(&self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            self.group,
            self.version,
            self.kind,
            self.namespace.as_deref().unwrap_or_default(),
            self.name
        )
    }

    fn leaf(self) -> ResourceTreeNode {
        ResourceTreeNode {
            kind: self.kind,
            group: self.group,
            name: self.name,
            namespace: self.namespace,
            key: self.key,
            category: self.category,
            status: None,
            relation: self.relation,
            expandable: false,
            children: Vec::new(),
            error: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RelationshipProvider {
    FluxKustomization,
    FluxHelmRelease,
    FluxReferences,
    OwnedChildren,
    WorkloadPods,
    HpaScaleTarget,
    ServiceTargets,
    EndpointSliceService,
    CnpgCluster,
    CnpgClusterReference,
    CnpgScheduledBackup,
    KubernetesStorage,
    VolumeAttachments,
    RookStorageClass,
    RookCephCluster,
    ObjectBucketClaim,
    Certificate,
    CertificateRequest,
    CertManagerOrder,
    ExternalSecret,
    ClusterExternalSecret,
    KopiurPolicy,
    TupprUpgrade,
    GatewayRoute,
    EnvoyPolicy,
    ReferenceGrant,
    NetworkPolicyPods,
}

#[derive(Clone, Copy)]
struct ProviderRegistration {
    group: &'static str,
    version: Option<&'static str>,
    kind: &'static str,
    provider: RelationshipProvider,
}

const RELATIONSHIP_PROVIDERS: &[ProviderRegistration] = &[
    ProviderRegistration {
        group: "kustomize.toolkit.fluxcd.io",
        version: None,
        kind: "Kustomization",
        provider: RelationshipProvider::FluxKustomization,
    },
    ProviderRegistration {
        group: "kustomize.toolkit.fluxcd.io",
        version: None,
        kind: "Kustomization",
        provider: RelationshipProvider::FluxReferences,
    },
    ProviderRegistration {
        group: "helm.toolkit.fluxcd.io",
        version: None,
        kind: "HelmRelease",
        provider: RelationshipProvider::FluxHelmRelease,
    },
    ProviderRegistration {
        group: "helm.toolkit.fluxcd.io",
        version: None,
        kind: "HelmRelease",
        provider: RelationshipProvider::FluxReferences,
    },
    ProviderRegistration {
        group: "apps",
        version: Some("v1"),
        kind: "Deployment",
        provider: RelationshipProvider::OwnedChildren,
    },
    ProviderRegistration {
        group: "apps",
        version: Some("v1"),
        kind: "Deployment",
        provider: RelationshipProvider::WorkloadPods,
    },
    ProviderRegistration {
        group: "apps",
        version: Some("v1"),
        kind: "ReplicaSet",
        provider: RelationshipProvider::OwnedChildren,
    },
    ProviderRegistration {
        group: "apps",
        version: Some("v1"),
        kind: "ReplicaSet",
        provider: RelationshipProvider::WorkloadPods,
    },
    ProviderRegistration {
        group: "apps",
        version: Some("v1"),
        kind: "StatefulSet",
        provider: RelationshipProvider::OwnedChildren,
    },
    ProviderRegistration {
        group: "apps",
        version: Some("v1"),
        kind: "StatefulSet",
        provider: RelationshipProvider::WorkloadPods,
    },
    ProviderRegistration {
        group: "apps",
        version: Some("v1"),
        kind: "DaemonSet",
        provider: RelationshipProvider::OwnedChildren,
    },
    ProviderRegistration {
        group: "apps",
        version: Some("v1"),
        kind: "DaemonSet",
        provider: RelationshipProvider::WorkloadPods,
    },
    ProviderRegistration {
        group: "batch",
        version: Some("v1"),
        kind: "CronJob",
        provider: RelationshipProvider::OwnedChildren,
    },
    ProviderRegistration {
        group: "batch",
        version: Some("v1"),
        kind: "Job",
        provider: RelationshipProvider::OwnedChildren,
    },
    ProviderRegistration {
        group: "batch",
        version: Some("v1"),
        kind: "Job",
        provider: RelationshipProvider::WorkloadPods,
    },
    ProviderRegistration {
        group: "autoscaling",
        version: None,
        kind: "HorizontalPodAutoscaler",
        provider: RelationshipProvider::HpaScaleTarget,
    },
    ProviderRegistration {
        group: "",
        version: Some("v1"),
        kind: "Service",
        provider: RelationshipProvider::ServiceTargets,
    },
    ProviderRegistration {
        group: "discovery.k8s.io",
        version: Some("v1"),
        kind: "EndpointSlice",
        provider: RelationshipProvider::EndpointSliceService,
    },
    ProviderRegistration {
        group: "postgresql.cnpg.io",
        version: None,
        kind: "Cluster",
        provider: RelationshipProvider::CnpgCluster,
    },
    ProviderRegistration {
        group: "postgresql.cnpg.io",
        version: None,
        kind: "Backup",
        provider: RelationshipProvider::CnpgClusterReference,
    },
    ProviderRegistration {
        group: "postgresql.cnpg.io",
        version: None,
        kind: "Database",
        provider: RelationshipProvider::CnpgClusterReference,
    },
    ProviderRegistration {
        group: "postgresql.cnpg.io",
        version: None,
        kind: "DatabaseRole",
        provider: RelationshipProvider::CnpgClusterReference,
    },
    ProviderRegistration {
        group: "postgresql.cnpg.io",
        version: None,
        kind: "Pooler",
        provider: RelationshipProvider::CnpgClusterReference,
    },
    ProviderRegistration {
        group: "postgresql.cnpg.io",
        version: None,
        kind: "ScheduledBackup",
        provider: RelationshipProvider::CnpgClusterReference,
    },
    ProviderRegistration {
        group: "postgresql.cnpg.io",
        version: None,
        kind: "ScheduledBackup",
        provider: RelationshipProvider::CnpgScheduledBackup,
    },
    ProviderRegistration {
        group: "",
        version: Some("v1"),
        kind: "Pod",
        provider: RelationshipProvider::KubernetesStorage,
    },
    ProviderRegistration {
        group: "",
        version: Some("v1"),
        kind: "PersistentVolumeClaim",
        provider: RelationshipProvider::KubernetesStorage,
    },
    ProviderRegistration {
        group: "",
        version: Some("v1"),
        kind: "PersistentVolume",
        provider: RelationshipProvider::KubernetesStorage,
    },
    ProviderRegistration {
        group: "",
        version: Some("v1"),
        kind: "PersistentVolume",
        provider: RelationshipProvider::VolumeAttachments,
    },
    ProviderRegistration {
        group: "",
        version: Some("v1"),
        kind: "Node",
        provider: RelationshipProvider::VolumeAttachments,
    },
    ProviderRegistration {
        group: "storage.k8s.io",
        version: Some("v1"),
        kind: "VolumeAttachment",
        provider: RelationshipProvider::KubernetesStorage,
    },
    ProviderRegistration {
        group: "storage.k8s.io",
        version: Some("v1"),
        kind: "StorageClass",
        provider: RelationshipProvider::RookStorageClass,
    },
    ProviderRegistration {
        group: "ceph.rook.io",
        version: None,
        kind: "CephCluster",
        provider: RelationshipProvider::RookCephCluster,
    },
    ProviderRegistration {
        group: "objectbucket.io",
        version: None,
        kind: "ObjectBucketClaim",
        provider: RelationshipProvider::ObjectBucketClaim,
    },
    ProviderRegistration {
        group: "cert-manager.io",
        version: None,
        kind: "Certificate",
        provider: RelationshipProvider::Certificate,
    },
    ProviderRegistration {
        group: "cert-manager.io",
        version: None,
        kind: "CertificateRequest",
        provider: RelationshipProvider::CertificateRequest,
    },
    ProviderRegistration {
        group: "acme.cert-manager.io",
        version: None,
        kind: "Order",
        provider: RelationshipProvider::CertManagerOrder,
    },
    ProviderRegistration {
        group: "external-secrets.io",
        version: None,
        kind: "ExternalSecret",
        provider: RelationshipProvider::ExternalSecret,
    },
    ProviderRegistration {
        group: "external-secrets.io",
        version: None,
        kind: "ClusterExternalSecret",
        provider: RelationshipProvider::ClusterExternalSecret,
    },
    ProviderRegistration {
        group: "kopiur.home-operations.com",
        version: None,
        kind: "SnapshotPolicy",
        provider: RelationshipProvider::KopiurPolicy,
    },
    ProviderRegistration {
        group: "tuppr.home-operations.com",
        version: None,
        kind: "KubernetesUpgrade",
        provider: RelationshipProvider::TupprUpgrade,
    },
    ProviderRegistration {
        group: "tuppr.home-operations.com",
        version: None,
        kind: "TalosUpgrade",
        provider: RelationshipProvider::TupprUpgrade,
    },
    ProviderRegistration {
        group: "gateway.networking.k8s.io",
        version: None,
        kind: "HTTPRoute",
        provider: RelationshipProvider::GatewayRoute,
    },
    ProviderRegistration {
        group: "gateway.networking.k8s.io",
        version: None,
        kind: "GRPCRoute",
        provider: RelationshipProvider::GatewayRoute,
    },
    ProviderRegistration {
        group: "gateway.networking.k8s.io",
        version: None,
        kind: "TLSRoute",
        provider: RelationshipProvider::GatewayRoute,
    },
    ProviderRegistration {
        group: "gateway.networking.k8s.io",
        version: None,
        kind: "TCPRoute",
        provider: RelationshipProvider::GatewayRoute,
    },
    ProviderRegistration {
        group: "gateway.networking.k8s.io",
        version: None,
        kind: "UDPRoute",
        provider: RelationshipProvider::GatewayRoute,
    },
    ProviderRegistration {
        group: "gateway.networking.k8s.io",
        version: None,
        kind: "ReferenceGrant",
        provider: RelationshipProvider::ReferenceGrant,
    },
    ProviderRegistration {
        group: "networking.k8s.io",
        version: Some("v1"),
        kind: "NetworkPolicy",
        provider: RelationshipProvider::NetworkPolicyPods,
    },
    ProviderRegistration {
        group: "gateway.envoyproxy.io",
        version: None,
        kind: "BackendTrafficPolicy",
        provider: RelationshipProvider::EnvoyPolicy,
    },
    ProviderRegistration {
        group: "gateway.envoyproxy.io",
        version: None,
        kind: "ClientTrafficPolicy",
        provider: RelationshipProvider::EnvoyPolicy,
    },
    ProviderRegistration {
        group: "gateway.envoyproxy.io",
        version: None,
        kind: "SecurityPolicy",
        provider: RelationshipProvider::EnvoyPolicy,
    },
    ProviderRegistration {
        group: "gateway.envoyproxy.io",
        version: None,
        kind: "EnvoyPatchPolicy",
        provider: RelationshipProvider::EnvoyPolicy,
    },
    ProviderRegistration {
        group: "gateway.envoyproxy.io",
        version: None,
        kind: "EnvoyExtensionPolicy",
        provider: RelationshipProvider::EnvoyPolicy,
    },
];

impl Backend {
    pub async fn resource_tree(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<ResourceTreeNode, K8sError> {
        let entry = self.entry(key)?;
        let root = ResourceRef {
            group: entry.kind.group.clone(),
            version: entry.kind.version.clone(),
            kind: entry.kind.kind.clone(),
            name: name.to_string(),
            namespace: ns.map(str::to_string),
            key: Some(key.to_string()),
            category: Some(entry.kind.category.clone()),
            relation: None,
            expandable: true,
        };
        Ok(self
            .build_node(
                root,
                Direction::All,
                0,
                Vec::new(),
                Arc::new(tokio::sync::Semaphore::new(8)),
                Arc::new(AtomicUsize::new(1)),
            )
            .await)
    }

    fn build_node(
        &self,
        resource: ResourceRef,
        direction: Direction,
        depth: usize,
        mut ancestry: Vec<String>,
        semaphore: Arc<tokio::sync::Semaphore>,
        node_count: Arc<AtomicUsize>,
    ) -> BoxFuture<'_, ResourceTreeNode> {
        Box::pin(async move {
            let identity = resource.identity();
            if ancestry.contains(&identity) {
                return error_node(resource, "relationship cycle detected".into());
            }
            ancestry.push(identity);

            if depth >= MAX_DEPTH {
                return error_node(
                    resource,
                    format!("maximum relationship depth ({MAX_DEPTH}) reached"),
                );
            }

            let Some(key) = resource.key.as_deref() else {
                return error_node(resource, "kind not found in this cluster's catalog".into());
            };
            let object = match self
                .registry
                .cached_object(key, resource.namespace.as_deref(), &resource.name)
                .await
            {
                Some(object) => object,
                None => match self.dyn_api(key, resource.namespace.as_deref()) {
                    Ok(api) => match with_api_permit(&semaphore, api.get(&resource.name)).await {
                        Ok(object) => object,
                        Err(error) => {
                            return error_node(
                                resource,
                                format!("could not fetch resource: {error}"),
                            )
                        }
                    },
                    Err(error) => return error_node(resource, error.to_string()),
                },
            };
            let data = serde_json::to_value(&object).unwrap_or_default();
            let status = Some(crate::project::resource_status(
                &resource.group,
                &resource.kind,
                &object,
            ));
            let mut errors = Vec::new();
            let mut relationships = Vec::new();

            if matches!(direction, Direction::All | Direction::Owners) {
                relationships.extend(self.owner_relationships(&object, &resource));
            }
            if matches!(direction, Direction::All | Direction::Descendants) {
                if descendant_capacity_available(&node_count) {
                    let (mut descendants, mut provider_errors) = self
                        .descendant_relationships(&resource, &object, &data, &semaphore)
                        .await;
                    relationships.append(&mut descendants);
                    errors.append(&mut provider_errors);
                } else {
                    errors.push(format!(
                        "relationship tree limited to {MAX_NODES} resources"
                    ));
                }
            }

            deduplicate_and_sort(&mut relationships);
            let requested = relationships.len();
            let granted = reserve_node_slots(&node_count, requested);
            relationships.truncate(granted);
            if granted < requested {
                errors.push(format!(
                    "relationship tree limited to {MAX_NODES} resources"
                ));
            }
            let children = join_all(relationships.into_iter().map(|child| {
                if !child.expandable {
                    return Box::pin(async move { child.leaf() })
                        as BoxFuture<'_, ResourceTreeNode>;
                }
                let child_direction = if child.relation == Some(ResourceTreeRelation::Owner) {
                    Direction::Owners
                } else {
                    Direction::Descendants
                };
                self.build_node(
                    child,
                    child_direction,
                    depth + 1,
                    ancestry.clone(),
                    semaphore.clone(),
                    node_count.clone(),
                )
            }))
            .await;

            ResourceTreeNode {
                kind: resource.kind,
                group: resource.group,
                name: resource.name,
                namespace: resource.namespace,
                key: resource.key,
                category: resource.category,
                status,
                relation: resource.relation,
                expandable: true,
                children,
                error: (!errors.is_empty()).then(|| errors.join("; ")),
            }
        })
    }

    fn owner_relationships(&self, object: &DynamicObject, child: &ResourceRef) -> Vec<ResourceRef> {
        object
            .metadata
            .owner_references
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|owner| {
                let (group, version) = split_api_version(&owner.api_version);
                self.resolve_resource(
                    group,
                    version,
                    owner.kind.clone(),
                    owner.name.clone(),
                    child.namespace.clone(),
                    Some(ResourceTreeRelation::Owner),
                )
            })
            .collect()
    }

    async fn descendant_relationships(
        &self,
        resource: &ResourceRef,
        object: &DynamicObject,
        data: &Value,
        semaphore: &Arc<tokio::sync::Semaphore>,
    ) -> (Vec<ResourceRef>, Vec<String>) {
        let mut children = Vec::new();
        let mut errors = Vec::new();

        for provider in relationship_providers(resource) {
            match provider {
                RelationshipProvider::FluxKustomization => {
                    match self.kustomization_children(data) {
                        Ok(mut refs) => children.append(&mut refs),
                        Err(error) => errors.push(error),
                    }
                }
                RelationshipProvider::FluxHelmRelease => {
                    match self.helm_release_children(data, semaphore).await {
                        Ok(refs) => children.extend(refs.into_iter().map(|child| {
                            self.resolve_resource(
                                child.group,
                                child.version,
                                child.kind,
                                child.name,
                                child.namespace,
                                Some(ResourceTreeRelation::HelmManifest),
                            )
                        })),
                        Err(error) => errors.push(error),
                    }
                }
                RelationshipProvider::FluxReferences => {
                    children.extend(
                        flux_references(resource, data)
                            .into_iter()
                            .map(|reference| {
                                self.resolve_named_reference(
                                    reference,
                                    ResourceTreeRelation::ReferencedResource,
                                )
                            }),
                    );
                }
                RelationshipProvider::OwnedChildren => {
                    let Some((group, kind)) = owned_child_kind(&resource.group, &resource.kind)
                    else {
                        continue;
                    };
                    match with_api_permit(
                        semaphore,
                        self.list_owned_children(resource, object, group, kind),
                    )
                    .await
                    {
                        Ok(mut owned) => children.append(&mut owned),
                        Err(error) => errors.push(format!("owned resources: {error}")),
                    }
                }
                RelationshipProvider::WorkloadPods => {
                    match super::logs::workload_label_selector(data) {
                        Ok(selector) => match with_api_permit(
                            semaphore,
                            self.list_selected(
                                "",
                                "Pod",
                                resource.namespace.as_deref(),
                                &selector,
                                ResourceTreeRelation::SelectedPod,
                            ),
                        )
                        .await
                        {
                            Ok(mut pods) => children.append(&mut pods),
                            Err(error) => errors.push(format!("selected pods: {error}")),
                        },
                        Err(error) => errors.push(error),
                    }
                }
                RelationshipProvider::HpaScaleTarget => {
                    if let Some((group, version, kind, name)) = hpa_scale_target(data) {
                        children.push(self.resolve_resource(
                            group,
                            version,
                            kind,
                            name,
                            resource.namespace.clone(),
                            Some(ResourceTreeRelation::ScaleTarget),
                        ));
                    }
                }
                RelationshipProvider::ServiceTargets => {
                    let (mut refs, mut provider_errors) =
                        self.service_relationships(resource, data, semaphore).await;
                    children.append(&mut refs);
                    errors.append(&mut provider_errors);
                }
                RelationshipProvider::EndpointSliceService => {
                    if let Some(service) = endpoint_slice_service_name(data) {
                        let mut child = self.resolve_resource(
                            String::new(),
                            "v1".into(),
                            "Service".into(),
                            service.into(),
                            resource.namespace.clone(),
                            Some(ResourceTreeRelation::ReferencedResource),
                        );
                        // Expanding the Service would immediately link back to this slice.
                        child.expandable = false;
                        children.push(child);
                    }
                }
                RelationshipProvider::CnpgCluster => {
                    let (mut refs, mut provider_errors) =
                        self.cnpg_cluster_relationships(resource, semaphore).await;
                    children.append(&mut refs);
                    errors.append(&mut provider_errors);
                }
                RelationshipProvider::CnpgClusterReference => {
                    if let Some(cluster) =
                        data.pointer("/spec/cluster/name").and_then(Value::as_str)
                    {
                        children.push(self.resolve_resource(
                            "postgresql.cnpg.io".into(),
                            String::new(),
                            "Cluster".into(),
                            cluster.into(),
                            resource.namespace.clone(),
                            Some(ResourceTreeRelation::ReferencedResource),
                        ));
                    }
                }
                RelationshipProvider::CnpgScheduledBackup => {
                    match with_api_permit(
                        semaphore,
                        self.list_generated_cnpg_backups(resource, object),
                    )
                    .await
                    {
                        Ok(mut refs) => children.append(&mut refs),
                        Err(error) => errors.push(format!("generated backups: {error}")),
                    }
                }
                RelationshipProvider::KubernetesStorage => {
                    let attachment =
                        resource.group == "storage.k8s.io" && resource.kind == "VolumeAttachment";
                    children.extend(storage_reference_targets(resource, data).into_iter().map(
                        |target| {
                            let mut child = self.resolve_resource(
                                target.group.into(),
                                target.version.into(),
                                target.kind.into(),
                                target.name,
                                target.namespace,
                                Some(ResourceTreeRelation::ReferencedResource),
                            );
                            // Expanding the referenced PV would immediately link back here.
                            if attachment {
                                child.expandable = false;
                            }
                            child
                        },
                    ));
                }
                RelationshipProvider::VolumeAttachments => {
                    match with_api_permit(
                        semaphore,
                        self.list_matching(
                            "storage.k8s.io",
                            "VolumeAttachment",
                            None,
                            ResourceTreeRelation::VolumeAttachment,
                            |attachment| volume_attachment_matches(resource, attachment),
                        ),
                    )
                    .await
                    {
                        Ok(mut refs) => children.append(&mut refs),
                        Err(error) => errors.push(format!("volume attachments: {error}")),
                    }
                }
                RelationshipProvider::RookStorageClass => {
                    let (mut refs, mut provider_errors) =
                        self.rook_storage_relationships(data, semaphore).await;
                    children.append(&mut refs);
                    errors.append(&mut provider_errors);
                }
                RelationshipProvider::RookCephCluster => {
                    let (mut refs, mut provider_errors) =
                        self.rook_cluster_relationships(resource, semaphore).await;
                    children.append(&mut refs);
                    errors.append(&mut provider_errors);
                }
                RelationshipProvider::ObjectBucketClaim => {
                    children.extend(
                        object_bucket_claim_references(resource, data)
                            .into_iter()
                            .map(|reference| {
                                self.resolve_named_reference(
                                    reference,
                                    ResourceTreeRelation::GeneratedResource,
                                )
                            }),
                    );
                }
                RelationshipProvider::Certificate => {
                    children.extend(certificate_references(resource, data).into_iter().map(
                        |reference| {
                            self.resolve_named_reference(
                                reference,
                                ResourceTreeRelation::ReferencedResource,
                            )
                        },
                    ));
                }
                RelationshipProvider::CertificateRequest => {
                    if let Some(reference) = issuer_reference(resource, data) {
                        children.push(self.resolve_named_reference(
                            reference,
                            ResourceTreeRelation::ReferencedResource,
                        ));
                    }
                    match with_api_permit(
                        semaphore,
                        self.list_owned_kind(resource, object, "acme.cert-manager.io", "Order"),
                    )
                    .await
                    {
                        Ok(mut refs) => children.append(&mut refs),
                        Err(error) => errors.push(format!("owned Orders: {error}")),
                    }
                }
                RelationshipProvider::CertManagerOrder => {
                    match with_api_permit(
                        semaphore,
                        self.list_owned_kind(resource, object, "acme.cert-manager.io", "Challenge"),
                    )
                    .await
                    {
                        Ok(mut refs) => children.append(&mut refs),
                        Err(error) => errors.push(format!("owned Challenges: {error}")),
                    }
                }
                RelationshipProvider::ExternalSecret => {
                    children.extend(external_secret_references(resource, data).into_iter().map(
                        |(reference, relation)| self.resolve_named_reference(reference, relation),
                    ));
                }
                RelationshipProvider::ClusterExternalSecret => {
                    children.extend(
                        cluster_external_secret_references(resource, data)
                            .into_iter()
                            .map(|reference| {
                                self.resolve_named_reference(
                                    reference,
                                    ResourceTreeRelation::GeneratedResource,
                                )
                            }),
                    );
                    match with_api_permit(
                        semaphore,
                        self.list_owned_kind(
                            resource,
                            object,
                            "external-secrets.io",
                            "ExternalSecret",
                        ),
                    )
                    .await
                    {
                        Ok(mut refs) => {
                            for reference in &mut refs {
                                reference.relation = Some(ResourceTreeRelation::GeneratedResource);
                            }
                            children.append(&mut refs);
                        }
                        Err(error) => {
                            errors.push(format!("generated ExternalSecrets: {error}"));
                        }
                    }
                }
                RelationshipProvider::KopiurPolicy => {
                    let (mut refs, mut provider_errors) = self
                        .kopiur_policy_relationships(resource, object, data, semaphore)
                        .await;
                    children.append(&mut refs);
                    errors.append(&mut provider_errors);
                }
                RelationshipProvider::TupprUpgrade => {
                    let (mut refs, mut provider_errors) = self
                        .tuppr_upgrade_relationships(resource, data, semaphore)
                        .await;
                    children.append(&mut refs);
                    errors.append(&mut provider_errors);
                }
                RelationshipProvider::GatewayRoute => {
                    children.extend(gateway_route_references(resource, data).into_iter().map(
                        |reference| {
                            self.resolve_named_reference(
                                reference,
                                ResourceTreeRelation::ReferencedResource,
                            )
                        },
                    ));
                }
                RelationshipProvider::EnvoyPolicy => {
                    children.extend(envoy_policy_references(resource, data).into_iter().map(
                        |reference| {
                            self.resolve_named_reference(
                                reference,
                                ResourceTreeRelation::ReferencedResource,
                            )
                        },
                    ));
                }
                RelationshipProvider::ReferenceGrant => {
                    let (mut refs, mut provider_errors) = self
                        .reference_grant_relationships(resource, data, semaphore)
                        .await;
                    children.append(&mut refs);
                    errors.append(&mut provider_errors);
                }
                RelationshipProvider::NetworkPolicyPods => {
                    let selector = data.pointer("/spec/podSelector").unwrap_or(&Value::Null);
                    match super::logs::label_selector(selector, true, "NetworkPolicy") {
                        Ok(selector) => match with_api_permit(
                            semaphore,
                            self.list_selected(
                                "",
                                "Pod",
                                resource.namespace.as_deref(),
                                &selector,
                                ResourceTreeRelation::SelectedPod,
                            ),
                        )
                        .await
                        {
                            Ok(mut refs) => children.append(&mut refs),
                            Err(error) => errors.push(format!("selected pods: {error}")),
                        },
                        Err(error) => errors.push(error),
                    }
                }
            }
        }

        (children, errors)
    }

    async fn list_owned_children(
        &self,
        parent: &ResourceRef,
        object: &DynamicObject,
        group: &str,
        kind: &str,
    ) -> Result<Vec<ResourceRef>, K8sError> {
        self.list_owned_kind(parent, object, group, kind).await
    }

    async fn list_owned_kind(
        &self,
        parent: &ResourceRef,
        object: &DynamicObject,
        group: &str,
        kind: &str,
    ) -> Result<Vec<ResourceRef>, K8sError> {
        let uid = object.metadata.uid.as_deref().unwrap_or_default();
        if uid.is_empty() {
            return Ok(Vec::new());
        }
        let Some(entry) = self.catalog_entry(group, None, kind) else {
            return Ok(Vec::new());
        };
        let api = self.dyn_api(&entry.kind.key, parent.namespace.as_deref())?;
        let list = api.list(&ListParams::default()).await.map_err(api_err)?;
        Ok(list
            .items
            .into_iter()
            .filter(|child| {
                child
                    .metadata
                    .owner_references
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|owner| owner.uid == uid)
            })
            .filter_map(|child| {
                let namespace =
                    child_namespace(&child, parent.namespace.as_deref(), entry.kind.namespaced);
                Some((child.metadata.name?, namespace))
            })
            .map(|(name, namespace)| ResourceRef {
                group: entry.kind.group.clone(),
                version: entry.kind.version.clone(),
                kind: entry.kind.kind.clone(),
                name,
                namespace,
                key: Some(entry.kind.key.clone()),
                category: Some(entry.kind.category.clone()),
                relation: Some(ResourceTreeRelation::OwnedResource),
                expandable: is_expandable_kind(&entry.kind.group, &entry.kind.kind),
            })
            .collect())
    }

    async fn list_selected(
        &self,
        group: &str,
        kind: &str,
        namespace: Option<&str>,
        selector: &str,
        relation: ResourceTreeRelation,
    ) -> Result<Vec<ResourceRef>, K8sError> {
        let Some(entry) = self.catalog_entry(group, None, kind) else {
            return Ok(Vec::new());
        };
        let api = self.dyn_api(&entry.kind.key, namespace)?;
        let list = api
            .list(&ListParams::default().labels(selector))
            .await
            .map_err(api_err)?;
        Ok(list
            .items
            .into_iter()
            .filter_map(|object| {
                let object_namespace = child_namespace(&object, namespace, entry.kind.namespaced);
                Some((object.metadata.name?, object_namespace))
            })
            .map(|(name, object_namespace)| ResourceRef {
                group: entry.kind.group.clone(),
                version: entry.kind.version.clone(),
                kind: entry.kind.kind.clone(),
                name,
                namespace: object_namespace,
                key: Some(entry.kind.key.clone()),
                category: Some(entry.kind.category.clone()),
                relation: Some(relation),
                expandable: reference_is_expandable(Some(relation), group, kind),
            })
            .collect())
    }

    async fn list_matching(
        &self,
        group: &str,
        kind: &str,
        namespace: Option<&str>,
        relation: ResourceTreeRelation,
        matches: impl Fn(&DynamicObject) -> bool,
    ) -> Result<Vec<ResourceRef>, K8sError> {
        let Some(entry) = self.catalog_entry(group, None, kind) else {
            return Ok(Vec::new());
        };
        let api = self.dyn_api(&entry.kind.key, namespace)?;
        let list = api.list(&ListParams::default()).await.map_err(api_err)?;
        Ok(list
            .items
            .into_iter()
            .filter(matches)
            .filter_map(|object| {
                let object_namespace = child_namespace(&object, namespace, entry.kind.namespaced);
                Some((object.metadata.name?, object_namespace))
            })
            .map(|(name, object_namespace)| ResourceRef {
                group: entry.kind.group.clone(),
                version: entry.kind.version.clone(),
                kind: entry.kind.kind.clone(),
                name,
                namespace: object_namespace,
                key: Some(entry.kind.key.clone()),
                category: Some(entry.kind.category.clone()),
                relation: Some(relation),
                expandable: reference_is_expandable(Some(relation), group, kind),
            })
            .collect())
    }

    async fn service_relationships(
        &self,
        resource: &ResourceRef,
        data: &Value,
        semaphore: &tokio::sync::Semaphore,
    ) -> (Vec<ResourceRef>, Vec<String>) {
        let mut children = Vec::new();
        let mut errors = Vec::new();
        if let Some(selector) = service_selector(data) {
            match with_api_permit(
                semaphore,
                self.list_selected(
                    "",
                    "Pod",
                    resource.namespace.as_deref(),
                    &selector,
                    ResourceTreeRelation::SelectedPod,
                ),
            )
            .await
            {
                Ok(mut pods) => children.append(&mut pods),
                Err(error) => errors.push(format!("selected pods: {error}")),
            }
        }
        let selector = format!("kubernetes.io/service-name={}", resource.name);
        match with_api_permit(
            semaphore,
            self.list_selected(
                "discovery.k8s.io",
                "EndpointSlice",
                resource.namespace.as_deref(),
                &selector,
                ResourceTreeRelation::EndpointSlice,
            ),
        )
        .await
        {
            Ok(mut slices) => children.append(&mut slices),
            Err(error) => errors.push(format!("endpoint slices: {error}")),
        }
        (children, errors)
    }

    async fn cnpg_cluster_relationships(
        &self,
        resource: &ResourceRef,
        semaphore: &tokio::sync::Semaphore,
    ) -> (Vec<ResourceRef>, Vec<String>) {
        let mut children = Vec::new();
        let mut errors = Vec::new();
        let selector = format!("cnpg.io/cluster={}", resource.name);
        for (group, kind) in [("", "Pod"), ("", "PersistentVolumeClaim"), ("", "Service")] {
            match with_api_permit(
                semaphore,
                self.list_selected(
                    group,
                    kind,
                    resource.namespace.as_deref(),
                    &selector,
                    ResourceTreeRelation::ClusterResource,
                ),
            )
            .await
            {
                Ok(mut refs) => children.append(&mut refs),
                Err(error) => errors.push(format!("{kind} resources: {error}")),
            }
        }
        for kind in ["Backup", "Database", "DatabaseRole", "Pooler"] {
            match with_api_permit(
                semaphore,
                self.list_matching(
                    "postgresql.cnpg.io",
                    kind,
                    resource.namespace.as_deref(),
                    ResourceTreeRelation::ClusterResource,
                    |object| {
                        object
                            .data
                            .pointer("/spec/cluster/name")
                            .and_then(Value::as_str)
                            == Some(resource.name.as_str())
                    },
                ),
            )
            .await
            {
                Ok(mut refs) => children.append(&mut refs),
                Err(error) => errors.push(format!("{kind} resources: {error}")),
            }
        }
        (children, errors)
    }

    async fn list_generated_cnpg_backups(
        &self,
        resource: &ResourceRef,
        object: &DynamicObject,
    ) -> Result<Vec<ResourceRef>, K8sError> {
        let uid = object.metadata.uid.as_deref().unwrap_or_default();
        self.list_matching(
            "postgresql.cnpg.io",
            "Backup",
            resource.namespace.as_deref(),
            ResourceTreeRelation::GeneratedResource,
            |backup| {
                backup
                    .metadata
                    .owner_references
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|owner| !uid.is_empty() && owner.uid == uid)
                    || backup
                        .metadata
                        .labels
                        .as_ref()
                        .and_then(|labels| labels.get("cnpg.io/scheduled-backup"))
                        .is_some_and(|name| name == &resource.name)
            },
        )
        .await
    }

    async fn rook_storage_relationships(
        &self,
        data: &Value,
        semaphore: &tokio::sync::Semaphore,
    ) -> (Vec<ResourceRef>, Vec<String>) {
        let provisioner = data
            .pointer("/provisioner")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !provisioner.contains("rook-ceph") && !provisioner.ends_with(".ceph.com") {
            return (Vec::new(), Vec::new());
        }
        let Some(namespace) = data
            .pointer("/parameters/clusterID")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return (Vec::new(), Vec::new());
        };
        let mut children = Vec::new();
        if let Some(pool) = data
            .pointer("/parameters/pool")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            children.push(self.resolve_resource(
                "ceph.rook.io".into(),
                String::new(),
                "CephBlockPool".into(),
                pool.into(),
                Some(namespace.into()),
                Some(ResourceTreeRelation::StorageBackend),
            ));
        }
        if let Some(filesystem) = data
            .pointer("/parameters/fsName")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            children.push(self.resolve_resource(
                "ceph.rook.io".into(),
                String::new(),
                "CephFilesystem".into(),
                filesystem.into(),
                Some(namespace.into()),
                Some(ResourceTreeRelation::StorageBackend),
            ));
        }
        let mut errors = Vec::new();
        match with_api_permit(
            semaphore,
            self.list_matching(
                "ceph.rook.io",
                "CephCluster",
                Some(namespace),
                ResourceTreeRelation::StorageBackend,
                |_| true,
            ),
        )
        .await
        {
            Ok(mut refs) => children.append(&mut refs),
            Err(error) => errors.push(format!("CephCluster resources: {error}")),
        }
        (children, errors)
    }

    async fn rook_cluster_relationships(
        &self,
        resource: &ResourceRef,
        semaphore: &tokio::sync::Semaphore,
    ) -> (Vec<ResourceRef>, Vec<String>) {
        let mut children = Vec::new();
        let mut errors = Vec::new();
        for kind in ["CephBlockPool", "CephFilesystem", "CephObjectStore"] {
            match with_api_permit(
                semaphore,
                self.list_matching(
                    "ceph.rook.io",
                    kind,
                    resource.namespace.as_deref(),
                    ResourceTreeRelation::ClusterResource,
                    |_| true,
                ),
            )
            .await
            {
                Ok(mut refs) => children.append(&mut refs),
                Err(error) => errors.push(format!("{kind} resources: {error}")),
            }
        }
        let namespace = resource.namespace.as_deref().unwrap_or_default();
        for (kind, selector) in [
            ("Deployment", format!("rook_cluster={namespace}")),
            ("StatefulSet", format!("rook_cluster={namespace}")),
            ("DaemonSet", format!("rook_cluster={namespace}")),
            ("Deployment", "app=rook-ceph-operator".to_string()),
        ] {
            match with_api_permit(
                semaphore,
                self.list_selected(
                    "apps",
                    kind,
                    resource.namespace.as_deref(),
                    &selector,
                    ResourceTreeRelation::ClusterResource,
                ),
            )
            .await
            {
                Ok(mut refs) => children.append(&mut refs),
                Err(error) => errors.push(format!("Rook {kind} resources: {error}")),
            }
        }
        (children, errors)
    }

    async fn kopiur_policy_relationships(
        &self,
        resource: &ResourceRef,
        object: &DynamicObject,
        data: &Value,
        semaphore: &tokio::sync::Semaphore,
    ) -> (Vec<ResourceRef>, Vec<String>) {
        let mut children: Vec<_> = kopiur_repository_references(resource, data)
            .into_iter()
            .map(|reference| {
                self.resolve_named_reference(reference, ResourceTreeRelation::ReferencedResource)
            })
            .collect();
        let mut errors = Vec::new();
        let labels = object.metadata.labels.clone().unwrap_or_default();
        for kind in ["SnapshotSchedule", "Snapshot", "Restore"] {
            let result = with_api_permit(
                semaphore,
                self.list_matching(
                    "kopiur.home-operations.com",
                    kind,
                    resource.namespace.as_deref(),
                    ResourceTreeRelation::GeneratedResource,
                    |child| kopiur_child_matches_policy(kind, &resource.name, &labels, child),
                ),
            )
            .await;
            match result {
                Ok(mut refs) => children.append(&mut refs),
                Err(error) => errors.push(format!("{kind} resources: {error}")),
            }
        }
        (children, errors)
    }

    async fn tuppr_upgrade_relationships(
        &self,
        resource: &ResourceRef,
        data: &Value,
        semaphore: &tokio::sync::Semaphore,
    ) -> (Vec<ResourceRef>, Vec<String>) {
        let mut children: Vec<_> = tuppr_node_names(&resource.kind, data)
            .into_iter()
            .map(|name| {
                self.resolve_resource(
                    String::new(),
                    "v1".into(),
                    "Node".into(),
                    name,
                    None,
                    Some(ResourceTreeRelation::ClusterResource),
                )
            })
            .collect();
        let mut errors = Vec::new();
        if resource.kind == "KubernetesUpgrade" {
            match with_api_permit(
                semaphore,
                self.list_matching(
                    "",
                    "Node",
                    None,
                    ResourceTreeRelation::ClusterResource,
                    |_| true,
                ),
            )
            .await
            {
                Ok(mut refs) => children.append(&mut refs),
                Err(error) => errors.push(format!("affected Nodes: {error}")),
            }
        } else if resource.kind == "TalosUpgrade" {
            let selector = data.pointer("/spec/nodeSelector").unwrap_or(&Value::Null);
            match super::logs::label_selector(selector, true, "Tuppr node") {
                Ok(selector) => match with_api_permit(
                    semaphore,
                    self.list_selected(
                        "",
                        "Node",
                        None,
                        &selector,
                        ResourceTreeRelation::ClusterResource,
                    ),
                )
                .await
                {
                    Ok(mut refs) => children.append(&mut refs),
                    Err(error) => errors.push(format!("affected Nodes: {error}")),
                },
                Err(error) => errors.push(error),
            }
        }
        (children, errors)
    }

    async fn reference_grant_relationships(
        &self,
        resource: &ResourceRef,
        data: &Value,
        semaphore: &tokio::sync::Semaphore,
    ) -> (Vec<ResourceRef>, Vec<String>) {
        let mut children = Vec::new();
        let mut errors = Vec::new();
        let targets = reference_grant_targets(data);
        for reference in reference_grant_named_targets(resource, data) {
            children.push(
                self.resolve_named_reference(reference, ResourceTreeRelation::ReferencedResource),
            );
        }
        for (group, kind, namespace) in reference_grant_list_targets(resource, data) {
            match with_api_permit(
                semaphore,
                self.list_matching(
                    &group,
                    &kind,
                    namespace.as_deref(),
                    ResourceTreeRelation::ReferencedResource,
                    |_| true,
                ),
            )
            .await
            {
                Ok(mut refs) => children.append(&mut refs),
                Err(error) => errors.push(format!("permitted {kind} resources: {error}")),
            }
        }
        for (group, kind, namespace) in reference_grant_sources(data) {
            match with_api_permit(
                semaphore,
                self.list_matching(
                    &group,
                    &kind,
                    namespace.as_deref(),
                    ResourceTreeRelation::ReferencedResource,
                    |source| {
                        reference_grant_source_matches(
                            source,
                            resource.namespace.as_deref(),
                            &targets,
                        )
                    },
                ),
            )
            .await
            {
                Ok(mut refs) => children.append(&mut refs),
                Err(error) => errors.push(format!("permitted {kind} sources: {error}")),
            }
        }
        (children, errors)
    }

    fn kustomization_children(&self, data: &Value) -> Result<Vec<ResourceRef>, String> {
        let entries = data
            .pointer("/status/inventory/entries")
            .and_then(Value::as_array)
            .ok_or_else(|| "Kustomization has no inventory yet".to_string())?;
        Ok(entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(Value::as_str))
            .filter_map(parse_inventory_id)
            .map(|(namespace, name, group, kind)| {
                self.resolve_resource(
                    group,
                    String::new(),
                    kind,
                    name,
                    namespace,
                    Some(ResourceTreeRelation::FluxInventory),
                )
            })
            .collect())
    }

    fn resolve_resource(
        &self,
        group: String,
        version: String,
        kind: String,
        name: String,
        namespace: Option<String>,
        relation: Option<ResourceTreeRelation>,
    ) -> ResourceRef {
        let entry = self.catalog_entry(
            &group,
            (!version.is_empty()).then_some(version.as_str()),
            &kind,
        );
        let expandable = reference_is_expandable(relation, &group, &kind);
        ResourceRef {
            group,
            version,
            kind,
            name,
            namespace: match &entry {
                Some(entry) if !entry.kind.namespaced => None,
                _ => namespace,
            },
            key: entry.as_ref().map(|entry| entry.kind.key.clone()),
            category: entry.as_ref().map(|entry| entry.kind.category.clone()),
            relation,
            expandable,
        }
    }

    fn resolve_named_reference(
        &self,
        reference: NamedReference,
        relation: ResourceTreeRelation,
    ) -> ResourceRef {
        self.resolve_resource(
            reference.group,
            String::new(),
            reference.kind,
            reference.name,
            reference.namespace,
            Some(relation),
        )
    }

    fn catalog_entry(
        &self,
        group: &str,
        version: Option<&str>,
        kind: &str,
    ) -> Option<crate::discovery::CatalogEntry> {
        let catalog = self.shared.catalog();
        let catalog = catalog.load();
        version
            .and_then(|version| {
                let key = roder_core::ResourceKind::make_key(group, version, kind);
                catalog.by_key.get(&key).cloned()
            })
            .or_else(|| {
                catalog
                    .entries
                    .iter()
                    .find(|entry| entry.kind.group == group && entry.kind.kind == kind)
                    .cloned()
            })
    }
}

fn error_node(resource: ResourceRef, error: String) -> ResourceTreeNode {
    ResourceTreeNode {
        kind: resource.kind,
        group: resource.group,
        name: resource.name,
        namespace: resource.namespace,
        key: resource.key,
        category: resource.category,
        status: None,
        relation: resource.relation,
        expandable: resource.expandable,
        children: Vec::new(),
        error: Some(error),
    }
}

pub(super) async fn with_api_permit<T>(
    semaphore: &tokio::sync::Semaphore,
    operation: impl std::future::Future<Output = T>,
) -> T {
    let _permit = semaphore.acquire().await.expect("semaphore never closed");
    operation.await
}

fn split_api_version(api_version: &str) -> (String, String) {
    api_version.split_once('/').map_or_else(
        || (String::new(), api_version.to_string()),
        |(group, version)| (group.to_string(), version.to_string()),
    )
}

#[derive(Debug, PartialEq, Eq)]
struct NamedReference {
    group: String,
    kind: String,
    name: String,
    namespace: Option<String>,
}

fn named_reference(
    value: &Value,
    default_group: &str,
    default_kind: &str,
    default_namespace: Option<&str>,
) -> Option<NamedReference> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())?;
    Some(NamedReference {
        group: value
            .get("group")
            .and_then(Value::as_str)
            .unwrap_or(default_group)
            .to_string(),
        kind: value
            .get("kind")
            .and_then(Value::as_str)
            .filter(|kind| !kind.is_empty())
            .unwrap_or(default_kind)
            .to_string(),
        name: name.to_string(),
        namespace: value
            .get("namespace")
            .and_then(Value::as_str)
            .filter(|namespace| !namespace.is_empty())
            .map(str::to_string)
            .or_else(|| default_namespace.map(str::to_string)),
    })
}

fn object_bucket_claim_references(resource: &ResourceRef, data: &Value) -> Vec<NamedReference> {
    let mut references = ["Secret", "ConfigMap"]
        .into_iter()
        .map(|kind| NamedReference {
            group: String::new(),
            kind: kind.into(),
            name: resource.name.clone(),
            namespace: resource.namespace.clone(),
        })
        .collect::<Vec<_>>();
    if let Some(name) = data
        .pointer("/spec/objectBucketName")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
    {
        references.push(NamedReference {
            group: "objectbucket.io".into(),
            kind: "ObjectBucket".into(),
            name: name.into(),
            namespace: None,
        });
    }
    references
}

fn issuer_reference(resource: &ResourceRef, data: &Value) -> Option<NamedReference> {
    let mut reference = named_reference(
        data.pointer("/spec/issuerRef")?,
        "cert-manager.io",
        "Issuer",
        resource.namespace.as_deref(),
    )?;
    if reference.kind == "ClusterIssuer" {
        reference.namespace = None;
    }
    Some(reference)
}

fn certificate_references(resource: &ResourceRef, data: &Value) -> Vec<NamedReference> {
    let mut references = issuer_reference(resource, data)
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(name) = data
        .pointer("/spec/secretName")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
    {
        references.push(NamedReference {
            group: String::new(),
            kind: "Secret".into(),
            name: name.into(),
            namespace: resource.namespace.clone(),
        });
    }
    references
}

fn external_secret_references(
    resource: &ResourceRef,
    data: &Value,
) -> Vec<(NamedReference, ResourceTreeRelation)> {
    let mut references = Vec::new();
    if let Some(mut store) = data.pointer("/spec/secretStoreRef").and_then(|value| {
        named_reference(
            value,
            "external-secrets.io",
            "SecretStore",
            resource.namespace.as_deref(),
        )
    }) {
        if store.kind == "ClusterSecretStore" {
            store.namespace = None;
        }
        references.push((store, ResourceTreeRelation::ReferencedResource));
    }
    let target_name = data
        .pointer("/spec/target/name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(&resource.name);
    references.push((
        NamedReference {
            group: String::new(),
            kind: "Secret".into(),
            name: target_name.into(),
            namespace: resource.namespace.clone(),
        },
        ResourceTreeRelation::GeneratedResource,
    ));
    references
}

fn cluster_external_secret_references(resource: &ResourceRef, data: &Value) -> Vec<NamedReference> {
    let name = data
        .pointer("/status/externalSecretName")
        .or_else(|| data.pointer("/spec/externalSecretName"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(&resource.name);
    data.pointer("/status/provisionedNamespaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|namespace| !namespace.is_empty())
        .map(|namespace| NamedReference {
            group: "external-secrets.io".into(),
            kind: "ExternalSecret".into(),
            name: name.into(),
            namespace: Some(namespace.into()),
        })
        .collect()
}

fn flux_group(kind: &str) -> &'static str {
    match kind {
        "Kustomization" => "kustomize.toolkit.fluxcd.io",
        "HelmRelease" => "helm.toolkit.fluxcd.io",
        "ImageRepository" | "ImagePolicy" | "ImageUpdateAutomation" => "image.toolkit.fluxcd.io",
        "Alert" | "Provider" | "Receiver" => "notification.toolkit.fluxcd.io",
        _ => "source.toolkit.fluxcd.io",
    }
}

fn flux_references(resource: &ResourceRef, data: &Value) -> Vec<NamedReference> {
    let mut references = [
        "/spec/sourceRef",
        "/spec/chart/spec/sourceRef",
        "/spec/chartRef",
    ]
    .into_iter()
    .filter_map(|path| data.pointer(path))
    .filter_map(|value| {
        let kind = value.get("kind")?.as_str()?;
        named_reference(value, flux_group(kind), kind, resource.namespace.as_deref())
    })
    .collect::<Vec<_>>();
    references.extend(
        data.pointer("/spec/dependsOn")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| {
                named_reference(
                    value,
                    &resource.group,
                    &resource.kind,
                    resource.namespace.as_deref(),
                )
            }),
    );
    references
}

fn kopiur_repository_references(resource: &ResourceRef, data: &Value) -> Vec<NamedReference> {
    data.pointer("/spec/repository")
        .into_iter()
        .chain(
            data.pointer("/spec/repositories")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .filter_map(|value| {
            let mut reference = named_reference(
                value,
                "kopiur.home-operations.com",
                "Repository",
                resource.namespace.as_deref(),
            )?;
            if reference.kind == "ClusterRepository" {
                reference.namespace = None;
            }
            Some(reference)
        })
        .collect()
}

fn kopiur_child_matches_policy(
    kind: &str,
    policy_name: &str,
    policy_labels: &std::collections::BTreeMap<String, String>,
    child: &DynamicObject,
) -> bool {
    match kind {
        "SnapshotSchedule" => {
            child
                .data
                .pointer("/spec/policyRef/name")
                .and_then(Value::as_str)
                == Some(policy_name)
                || child
                    .data
                    .pointer("/spec/policySelector")
                    .is_some_and(|selector| label_selector_matches(selector, policy_labels))
        }
        "Snapshot" => {
            child
                .data
                .pointer("/spec/policyRef/name")
                .and_then(Value::as_str)
                == Some(policy_name)
                || child
                    .metadata
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get("kopiur.home-operations.com/config"))
                    .is_some_and(|name| name == policy_name)
        }
        "Restore" => {
            child
                .data
                .pointer("/spec/source/fromPolicy/name")
                .and_then(Value::as_str)
                == Some(policy_name)
        }
        _ => false,
    }
}

fn label_selector_matches(
    selector: &Value,
    labels: &std::collections::BTreeMap<String, String>,
) -> bool {
    let labels_match = selector
        .get("matchLabels")
        .and_then(Value::as_object)
        .is_none_or(|required| {
            required.iter().all(|(key, value)| {
                value
                    .as_str()
                    .is_some_and(|value| labels.get(key).is_some_and(|actual| actual == value))
            })
        });
    labels_match
        && selector
            .get("matchExpressions")
            .and_then(Value::as_array)
            .is_none_or(|expressions| {
                expressions.iter().all(|expression| {
                    let Some(key) = expression.get("key").and_then(Value::as_str) else {
                        return false;
                    };
                    let values = expression
                        .get("values")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>();
                    match expression.get("operator").and_then(Value::as_str) {
                        Some("In") => labels
                            .get(key)
                            .is_some_and(|value| values.contains(&value.as_str())),
                        Some("NotIn") => labels
                            .get(key)
                            .is_none_or(|value| !values.contains(&value.as_str())),
                        Some("Exists") => labels.contains_key(key),
                        Some("DoesNotExist") => !labels.contains_key(key),
                        _ => false,
                    }
                })
            })
}

fn tuppr_node_names(kind: &str, data: &Value) -> Vec<String> {
    let mut names = Vec::new();
    if kind == "KubernetesUpgrade" {
        if let Some(name) = data
            .pointer("/status/controllerNode")
            .and_then(Value::as_str)
        {
            names.push(name.to_string());
        }
    } else {
        for path in ["/status/currentNode"] {
            if let Some(name) = data.pointer(path).and_then(Value::as_str) {
                names.push(name.to_string());
            }
        }
        for path in ["/status/currentNodes", "/status/completedNodes"] {
            names.extend(
                data.pointer(path)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string),
            );
        }
        for path in [
            "/status/failedNodes",
            "/status/prePulledNodes",
            "/status/rebootingNodes",
        ] {
            names.extend(
                data.pointer(path)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|value| value.get("nodeName").and_then(Value::as_str))
                    .map(str::to_string),
            );
        }
    }
    names.retain(|name| !name.is_empty());
    names.sort();
    names.dedup();
    names
}

fn gateway_route_references(resource: &ResourceRef, data: &Value) -> Vec<NamedReference> {
    let mut references = data
        .pointer("/spec/parentRefs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            named_reference(
                value,
                "gateway.networking.k8s.io",
                "Gateway",
                resource.namespace.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    references.extend(
        data.pointer("/spec/rules")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|rule| {
                rule.get("backendRefs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(|value| {
                named_reference(value, "", "Service", resource.namespace.as_deref())
            }),
    );
    references
}

fn envoy_policy_references(resource: &ResourceRef, data: &Value) -> Vec<NamedReference> {
    data.pointer("/spec/targetRef")
        .into_iter()
        .chain(
            data.pointer("/spec/targetRefs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .filter_map(|value| {
            named_reference(
                value,
                "gateway.networking.k8s.io",
                "Gateway",
                resource.namespace.as_deref(),
            )
        })
        .collect()
}

fn reference_grant_named_targets(resource: &ResourceRef, data: &Value) -> Vec<NamedReference> {
    data.pointer("/spec/to")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|value| value.get("name").is_some())
        .filter_map(|value| named_reference(value, "", "Service", resource.namespace.as_deref()))
        .collect()
}

fn reference_grant_list_targets(
    resource: &ResourceRef,
    data: &Value,
) -> Vec<(String, String, Option<String>)> {
    data.pointer("/spec/to")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|value| value.get("name").is_none())
        .filter_map(|value| {
            Some((
                value.get("group")?.as_str()?.to_string(),
                value.get("kind")?.as_str()?.to_string(),
                resource.namespace.clone(),
            ))
        })
        .collect()
}

fn reference_grant_sources(data: &Value) -> Vec<(String, String, Option<String>)> {
    data.pointer("/spec/from")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            Some((
                value.get("group")?.as_str()?.to_string(),
                value.get("kind")?.as_str()?.to_string(),
                Some(value.get("namespace")?.as_str()?.to_string()),
            ))
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct ReferenceGrantTarget {
    group: String,
    kind: String,
    name: Option<String>,
}

fn reference_grant_targets(data: &Value) -> Vec<ReferenceGrantTarget> {
    data.pointer("/spec/to")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            Some(ReferenceGrantTarget {
                group: value.get("group")?.as_str()?.to_string(),
                kind: value.get("kind")?.as_str()?.to_string(),
                name: value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect()
}

fn reference_grant_source_matches(
    source: &DynamicObject,
    target_namespace: Option<&str>,
    targets: &[ReferenceGrantTarget],
) -> bool {
    let Some(target_namespace) = target_namespace else {
        return false;
    };
    source
        .data
        .get("spec")
        .is_some_and(|spec| value_references_grant_target(spec, target_namespace, targets))
}

fn value_references_grant_target(
    value: &Value,
    target_namespace: &str,
    targets: &[ReferenceGrantTarget],
) -> bool {
    match value {
        Value::Array(values) => values
            .iter()
            .any(|value| value_references_grant_target(value, target_namespace, targets)),
        Value::Object(object) => {
            let matches = object.get("namespace").and_then(Value::as_str) == Some(target_namespace)
                && object
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| {
                        let group = object.get("group").and_then(Value::as_str).unwrap_or("");
                        let kind = object
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("Service");
                        targets.iter().any(|target| {
                            target.group == group
                                && target.kind == kind
                                && target.name.as_deref().is_none_or(|target| target == name)
                        })
                    });
            matches
                || object
                    .values()
                    .any(|value| value_references_grant_target(value, target_namespace, targets))
        }
        _ => false,
    }
}

fn child_namespace(
    object: &DynamicObject,
    fallback: Option<&str>,
    namespaced: bool,
) -> Option<String> {
    namespaced.then(|| {
        object
            .metadata
            .namespace
            .clone()
            .or_else(|| fallback.map(str::to_string))
    })?
}

fn relationship_providers(
    resource: &ResourceRef,
) -> impl Iterator<Item = RelationshipProvider> + '_ {
    RELATIONSHIP_PROVIDERS
        .iter()
        .filter(|registration| {
            registration.group == resource.group
                && registration.kind == resource.kind
                && registration
                    .version
                    .is_none_or(|version| version == resource.version)
        })
        .map(|registration| registration.provider)
}

fn is_flux_owner(group: &str, kind: &str) -> bool {
    (group == "kustomize.toolkit.fluxcd.io" && kind == "Kustomization")
        || (group == "helm.toolkit.fluxcd.io" && kind == "HelmRelease")
}

fn is_workload(group: &str, kind: &str) -> bool {
    (group == "apps"
        && matches!(
            kind,
            "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet"
        ))
        || (group == "batch" && kind == "Job")
}

fn is_expandable_kind(group: &str, kind: &str) -> bool {
    is_flux_owner(group, kind)
        || is_workload(group, kind)
        || (group == "batch" && kind == "CronJob")
        || (group.is_empty() && kind == "Service")
        || (group == "acme.cert-manager.io" && kind == "Order")
        || (group == "external-secrets.io" && kind == "ExternalSecret")
        || (group == "gateway.networking.k8s.io"
            && matches!(
                kind,
                "HTTPRoute" | "GRPCRoute" | "TLSRoute" | "TCPRoute" | "UDPRoute"
            ))
}

fn reference_is_expandable(
    relation: Option<ResourceTreeRelation>,
    group: &str,
    kind: &str,
) -> bool {
    relation == Some(ResourceTreeRelation::Owner)
        || is_flux_owner(group, kind)
        || (relation.is_some()
            && relation != Some(ResourceTreeRelation::FluxInventory)
            && is_expandable_kind(group, kind))
        || (relation == Some(ResourceTreeRelation::ReferencedResource)
            && group.is_empty()
            && matches!(kind, "PersistentVolumeClaim" | "PersistentVolume"))
        || (group.is_empty() && kind == "Pod" && relation.is_some())
}

#[derive(Debug, PartialEq, Eq)]
struct StorageReference {
    group: &'static str,
    version: &'static str,
    kind: &'static str,
    name: String,
    namespace: Option<String>,
}

fn storage_reference_targets(resource: &ResourceRef, data: &Value) -> Vec<StorageReference> {
    match (resource.group.as_str(), resource.kind.as_str()) {
        ("", "Pod") => data
            .pointer("/spec/volumes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|volume| {
                let claim = volume
                    .pointer("/persistentVolumeClaim/claimName")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        volume.get("ephemeral")?;
                        let volume_name = volume.get("name")?.as_str()?;
                        (!resource.name.is_empty() && !volume_name.is_empty())
                            .then(|| format!("{}-{volume_name}", resource.name))
                    })?;
                Some(StorageReference {
                    group: "",
                    version: "v1",
                    kind: "PersistentVolumeClaim",
                    name: claim,
                    namespace: resource.namespace.clone(),
                })
            })
            .collect(),
        ("", "PersistentVolumeClaim") => {
            if let Some(name) = data
                .pointer("/spec/volumeName")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
            {
                vec![StorageReference {
                    group: "",
                    version: "v1",
                    kind: "PersistentVolume",
                    name: name.to_string(),
                    namespace: None,
                }]
            } else {
                storage_class_reference(data).into_iter().collect()
            }
        }
        ("", "PersistentVolume") => storage_class_reference(data).into_iter().collect(),
        ("storage.k8s.io", "VolumeAttachment") => {
            let mut targets = Vec::new();
            if let Some(name) = data
                .pointer("/spec/source/persistentVolumeName")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
            {
                targets.push(StorageReference {
                    group: "",
                    version: "v1",
                    kind: "PersistentVolume",
                    name: name.to_string(),
                    namespace: None,
                });
            }
            if let Some(name) = data
                .pointer("/spec/nodeName")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
            {
                targets.push(StorageReference {
                    group: "",
                    version: "v1",
                    kind: "Node",
                    name: name.to_string(),
                    namespace: None,
                });
            }
            targets
        }
        _ => Vec::new(),
    }
}

fn volume_attachment_matches(resource: &ResourceRef, attachment: &DynamicObject) -> bool {
    let path = match (resource.group.as_str(), resource.kind.as_str()) {
        ("", "PersistentVolume") => "/spec/source/persistentVolumeName",
        ("", "Node") => "/spec/nodeName",
        _ => return false,
    };
    attachment.data.pointer(path).and_then(Value::as_str) == Some(resource.name.as_str())
}

fn storage_class_reference(data: &Value) -> Option<StorageReference> {
    let name = data
        .pointer("/spec/storageClassName")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())?;
    Some(StorageReference {
        group: "storage.k8s.io",
        version: "v1",
        kind: "StorageClass",
        name: name.to_string(),
        namespace: None,
    })
}

fn owned_child_kind(group: &str, kind: &str) -> Option<(&'static str, &'static str)> {
    match (group, kind) {
        ("apps", "Deployment") => Some(("apps", "ReplicaSet")),
        ("apps", "ReplicaSet" | "StatefulSet" | "DaemonSet") => Some(("", "Pod")),
        ("batch", "CronJob") => Some(("batch", "Job")),
        ("batch", "Job") => Some(("", "Pod")),
        _ => None,
    }
}

fn service_selector(data: &Value) -> Option<String> {
    let labels = data.pointer("/spec/selector")?.as_object()?;
    let mut requirements: Vec<_> = labels
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|value| format!("{key}={value}")))
        .collect();
    requirements.sort();
    (!requirements.is_empty()).then(|| requirements.join(","))
}

fn endpoint_slice_service_name(data: &Value) -> Option<&str> {
    data.pointer("/metadata/labels/kubernetes.io~1service-name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

fn hpa_scale_target(data: &Value) -> Option<(String, String, String, String)> {
    let api_version = data
        .pointer("/spec/scaleTargetRef/apiVersion")?
        .as_str()
        .filter(|value| !value.is_empty())?;
    let kind = data
        .pointer("/spec/scaleTargetRef/kind")?
        .as_str()
        .filter(|value| !value.is_empty())?;
    let name = data
        .pointer("/spec/scaleTargetRef/name")?
        .as_str()
        .filter(|value| !value.is_empty())?;
    let (group, version) = split_api_version(api_version);
    Some((group, version, kind.into(), name.into()))
}

fn deduplicate_and_sort(resources: &mut Vec<ResourceRef>) {
    resources.sort_by(|left, right| {
        left.relation
            .map(ResourceTreeRelation::label)
            .cmp(&right.relation.map(ResourceTreeRelation::label))
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.namespace.cmp(&right.namespace))
            .then_with(|| left.name.cmp(&right.name))
    });
    let mut seen = HashSet::new();
    resources.retain(|resource| seen.insert((resource.relation, resource.identity())));
}

fn reserve_node_slots(count: &AtomicUsize, requested: usize) -> usize {
    let mut granted = 0;
    let _ = count.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        granted = requested.min(MAX_NODES.saturating_sub(current));
        Some(current + granted)
    });
    granted
}

fn descendant_capacity_available(count: &AtomicUsize) -> bool {
    count.load(Ordering::Relaxed) < MAX_NODES
}

fn parse_inventory_id(id: &str) -> Option<(Option<String>, String, String, String)> {
    let [namespace, name, group, kind]: [&str; 4] =
        id.splitn(4, '_').collect::<Vec<_>>().try_into().ok()?;
    if name.is_empty() || kind.is_empty() {
        return None;
    }
    Some((
        (!namespace.is_empty()).then(|| namespace.to_string()),
        name.to_string(),
        group.to_string(),
        kind.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_grouped_and_core_api_versions() {
        assert_eq!(split_api_version("apps/v1"), ("apps".into(), "v1".into()));
        assert_eq!(split_api_version("v1"), (String::new(), "v1".into()));
    }

    #[test]
    fn service_selector_is_sorted_and_rejects_empty_selectors() {
        assert_eq!(
            service_selector(&json!({"spec": {"selector": {"tier": "web", "app": "api"}}})),
            Some("app=api,tier=web".into())
        );
        assert_eq!(service_selector(&json!({"spec": {"selector": {}}})), None);
    }

    #[test]
    fn endpoint_slice_registry_and_label_resolve_the_service() {
        let mut resource = ResourceRef {
            group: "discovery.k8s.io".into(),
            version: "v1".into(),
            kind: "EndpointSlice".into(),
            name: "api-abc12".into(),
            namespace: Some("app".into()),
            key: None,
            category: Some(Category::Network),
            relation: None,
            expandable: true,
        };
        assert_eq!(
            relationship_providers(&resource).collect::<Vec<_>>(),
            [RelationshipProvider::EndpointSliceService]
        );
        resource.version = "v1beta1".into();
        assert_eq!(relationship_providers(&resource).count(), 0);

        assert_eq!(
            endpoint_slice_service_name(&json!({
                "metadata": {"labels": {"kubernetes.io/service-name": "api"}}
            })),
            Some("api")
        );
        assert_eq!(endpoint_slice_service_name(&json!({})), None);
        assert_eq!(
            endpoint_slice_service_name(&json!({
                "metadata": {"labels": {"kubernetes.io/service-name": ""}}
            })),
            None
        );
    }

    #[test]
    fn hpa_registry_and_reference_resolve_the_scale_target() {
        let mut resource = ResourceRef {
            group: "autoscaling".into(),
            version: "v2".into(),
            kind: "HorizontalPodAutoscaler".into(),
            name: "api".into(),
            namespace: Some("app".into()),
            key: None,
            category: Some(Category::Workloads),
            relation: None,
            expandable: true,
        };
        assert_eq!(
            relationship_providers(&resource).collect::<Vec<_>>(),
            [RelationshipProvider::HpaScaleTarget]
        );
        resource.version = "v1".into();
        assert_eq!(relationship_providers(&resource).count(), 1);

        assert_eq!(
            hpa_scale_target(&json!({"spec": {"scaleTargetRef": {
                "apiVersion": "apps/v1",
                "kind": "Deployment",
                "name": "api"
            }}})),
            Some((
                "apps".into(),
                "v1".into(),
                "Deployment".into(),
                "api".into()
            ))
        );
        assert!(hpa_scale_target(&json!({"spec": {"scaleTargetRef": {
            "apiVersion": "apps/v1",
            "kind": "Deployment"
        }}}))
        .is_none());
        assert!(reference_is_expandable(
            Some(ResourceTreeRelation::ScaleTarget),
            "apps",
            "Deployment"
        ));
    }

    #[test]
    fn known_controller_children_are_bounded() {
        assert_eq!(
            owned_child_kind("apps", "Deployment"),
            Some(("apps", "ReplicaSet"))
        );
        assert_eq!(owned_child_kind("batch", "CronJob"), Some(("batch", "Job")));
        assert_eq!(owned_child_kind("example.io", "Widget"), None);
    }

    #[test]
    fn flux_inventory_only_expands_nested_flux_owners() {
        assert!(!reference_is_expandable(
            Some(ResourceTreeRelation::FluxInventory),
            "apps",
            "Deployment"
        ));
        assert!(reference_is_expandable(
            Some(ResourceTreeRelation::FluxInventory),
            "kustomize.toolkit.fluxcd.io",
            "Kustomization"
        ));
        assert!(reference_is_expandable(
            Some(ResourceTreeRelation::Owner),
            "example.io",
            "Widget"
        ));
    }

    #[test]
    fn parses_flux_inventory_ids() {
        assert_eq!(
            parse_inventory_id("infra_frontend__Service"),
            Some((
                Some("infra".into()),
                "frontend".into(),
                String::new(),
                "Service".into()
            ))
        );
        assert!(parse_inventory_id("too_few_parts").is_none());
    }

    #[test]
    fn relationship_deduplication_preserves_distinct_edges() {
        let make = |relation| ResourceRef {
            group: String::new(),
            version: "v1".into(),
            kind: "Pod".into(),
            name: "api-1".into(),
            namespace: Some("default".into()),
            key: Some("/v1/Pod".into()),
            category: None,
            relation: Some(relation),
            expandable: false,
        };
        let mut resources = vec![
            make(ResourceTreeRelation::SelectedPod),
            make(ResourceTreeRelation::SelectedPod),
            make(ResourceTreeRelation::OwnedResource),
        ];
        deduplicate_and_sort(&mut resources);
        assert_eq!(resources.len(), 2);
    }

    #[test]
    fn node_budget_truncates_without_exceeding_the_limit() {
        let count = AtomicUsize::new(MAX_NODES - 2);
        assert!(descendant_capacity_available(&count));
        assert_eq!(reserve_node_slots(&count, 5), 2);
        assert!(!descendant_capacity_available(&count));
        assert_eq!(reserve_node_slots(&count, 1), 0);
        assert_eq!(count.load(Ordering::Relaxed), MAX_NODES);
    }

    #[tokio::test]
    async fn api_permit_is_released_after_each_operation() {
        let semaphore = tokio::sync::Semaphore::new(1);
        with_api_permit(&semaphore, async {}).await;
        with_api_permit(&semaphore, async {}).await;
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[test]
    fn relationship_registry_is_gvk_keyed_and_supports_multiple_providers() {
        let resource = ResourceRef {
            group: "postgresql.cnpg.io".into(),
            version: "v1".into(),
            kind: "ScheduledBackup".into(),
            name: "daily".into(),
            namespace: Some("database".into()),
            key: Some("postgresql.cnpg.io/v1/ScheduledBackup".into()),
            category: Some(Category::CloudNativePg),
            relation: None,
            expandable: true,
        };

        assert_eq!(
            relationship_providers(&resource).collect::<Vec<_>>(),
            [
                RelationshipProvider::CnpgClusterReference,
                RelationshipProvider::CnpgScheduledBackup,
            ]
        );
    }

    #[test]
    fn storage_class_registry_requires_the_registered_version() {
        let mut resource = ResourceRef {
            group: "storage.k8s.io".into(),
            version: "v1".into(),
            kind: "StorageClass".into(),
            name: "ceph".into(),
            namespace: None,
            key: Some("storage.k8s.io/v1/StorageClass".into()),
            category: Some(Category::Storage),
            relation: None,
            expandable: true,
        };
        assert_eq!(relationship_providers(&resource).count(), 1);
        resource.version = "v1beta1".into();
        assert_eq!(relationship_providers(&resource).count(), 0);
    }

    #[test]
    fn storage_registry_and_expansion_cover_the_full_chain() {
        let resource = |kind: &str| ResourceRef {
            group: String::new(),
            version: "v1".into(),
            kind: kind.into(),
            name: "resource".into(),
            namespace: Some("app".into()),
            key: None,
            category: Some(Category::Storage),
            relation: None,
            expandable: true,
        };

        for kind in ["Pod", "PersistentVolumeClaim"] {
            assert_eq!(
                relationship_providers(&resource(kind)).collect::<Vec<_>>(),
                [RelationshipProvider::KubernetesStorage]
            );
        }
        assert_eq!(
            relationship_providers(&resource("PersistentVolume")).collect::<Vec<_>>(),
            [
                RelationshipProvider::KubernetesStorage,
                RelationshipProvider::VolumeAttachments,
            ]
        );
        assert_eq!(
            relationship_providers(&resource("Node")).collect::<Vec<_>>(),
            [RelationshipProvider::VolumeAttachments]
        );
        let attachment = ResourceRef {
            group: "storage.k8s.io".into(),
            version: "v1".into(),
            kind: "VolumeAttachment".into(),
            ..resource("VolumeAttachment")
        };
        assert_eq!(
            relationship_providers(&attachment).collect::<Vec<_>>(),
            [RelationshipProvider::KubernetesStorage]
        );
        assert!(reference_is_expandable(
            Some(ResourceTreeRelation::ReferencedResource),
            "",
            "PersistentVolumeClaim"
        ));
        assert!(reference_is_expandable(
            Some(ResourceTreeRelation::ReferencedResource),
            "",
            "PersistentVolume"
        ));
        assert!(!reference_is_expandable(
            Some(ResourceTreeRelation::ReferencedResource),
            "storage.k8s.io",
            "StorageClass"
        ));
    }

    #[test]
    fn pod_storage_references_keep_the_pod_namespace() {
        let pod = ResourceRef {
            group: String::new(),
            version: "v1".into(),
            kind: "Pod".into(),
            name: "api".into(),
            namespace: Some("app".into()),
            key: None,
            category: None,
            relation: None,
            expandable: true,
        };
        let targets = storage_reference_targets(
            &pod,
            &json!({"spec": {"volumes": [
                {"name": "data", "persistentVolumeClaim": {"claimName": "api-data"}},
                {"name": "cache", "persistentVolumeClaim": {"claimName": "api-cache"}},
                {"name": "scratch", "ephemeral": {"volumeClaimTemplate": {"spec": {}}}},
                {"name": "config", "configMap": {"name": "api"}}
            ]}}),
        );

        assert_eq!(targets.len(), 3);
        assert!(targets.iter().all(|target| {
            target.kind == "PersistentVolumeClaim" && target.namespace.as_deref() == Some("app")
        }));
        assert!(targets.iter().any(|target| target.name == "api-scratch"));
    }

    #[test]
    fn claims_and_volumes_resolve_cluster_scoped_storage() {
        let mut resource = ResourceRef {
            group: String::new(),
            version: "v1".into(),
            kind: "PersistentVolumeClaim".into(),
            name: "data".into(),
            namespace: Some("app".into()),
            key: None,
            category: None,
            relation: None,
            expandable: true,
        };
        let bound = storage_reference_targets(
            &resource,
            &json!({"spec": {"volumeName": "pv-data", "storageClassName": "fast"}}),
        );
        assert_eq!(bound[0].kind, "PersistentVolume");
        assert_eq!(bound[0].name, "pv-data");
        assert_eq!(bound[0].namespace, None);

        let unbound =
            storage_reference_targets(&resource, &json!({"spec": {"storageClassName": "fast"}}));
        assert_eq!(unbound[0].kind, "StorageClass");
        assert_eq!(unbound[0].name, "fast");
        assert_eq!(unbound[0].namespace, None);

        resource.kind = "PersistentVolume".into();
        let class =
            storage_reference_targets(&resource, &json!({"spec": {"storageClassName": "fast"}}));
        assert_eq!(class[0].group, "storage.k8s.io");
        assert_eq!(class[0].kind, "StorageClass");
    }

    #[test]
    fn volume_attachments_reference_their_volume_and_node() {
        let attachment = ResourceRef {
            group: "storage.k8s.io".into(),
            version: "v1".into(),
            kind: "VolumeAttachment".into(),
            name: "csi-123".into(),
            namespace: None,
            key: None,
            category: Some(Category::Storage),
            relation: None,
            expandable: true,
        };
        let targets = storage_reference_targets(
            &attachment,
            &json!({"spec": {
                "source": {"persistentVolumeName": "pv-data"},
                "nodeName": "worker-1"
            }}),
        );

        assert_eq!(targets.len(), 2);
        assert!(targets.iter().all(|target| target.namespace.is_none()));
        assert!(targets
            .iter()
            .any(|target| target.kind == "PersistentVolume" && target.name == "pv-data"));
        assert!(targets
            .iter()
            .any(|target| target.kind == "Node" && target.name == "worker-1"));

        let node_only =
            storage_reference_targets(&attachment, &json!({"spec": {"nodeName": "worker-1"}}));
        assert_eq!(node_only.len(), 1);
        assert_eq!(node_only[0].kind, "Node");
    }

    #[test]
    fn reverse_volume_attachment_matching_uses_the_parent_reference() {
        let attachment: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "storage.k8s.io/v1",
            "kind": "VolumeAttachment",
            "metadata": {"name": "csi-123"},
            "spec": {
                "source": {"persistentVolumeName": "pv-data"},
                "nodeName": "worker-1"
            }
        }))
        .unwrap();
        let resource = |kind: &str, name: &str| ResourceRef {
            group: String::new(),
            version: "v1".into(),
            kind: kind.into(),
            name: name.into(),
            namespace: None,
            key: None,
            category: None,
            relation: None,
            expandable: true,
        };

        assert!(volume_attachment_matches(
            &resource("PersistentVolume", "pv-data"),
            &attachment
        ));
        assert!(volume_attachment_matches(
            &resource("Node", "worker-1"),
            &attachment
        ));
        assert!(!volume_attachment_matches(
            &resource("PersistentVolume", "pv-other"),
            &attachment
        ));
        assert!(!volume_attachment_matches(
            &resource("Service", "worker-1"),
            &attachment
        ));
    }

    fn test_resource(group: &str, kind: &str, namespace: Option<&str>) -> ResourceRef {
        ResourceRef {
            group: group.into(),
            version: "v1alpha1".into(),
            kind: kind.into(),
            name: "resource".into(),
            namespace: namespace.map(str::to_string),
            key: None,
            category: None,
            relation: None,
            expandable: true,
        }
    }

    #[test]
    fn relationship_registry_covers_operator_and_network_resources() {
        let cases = [
            (
                "objectbucket.io",
                "ObjectBucketClaim",
                RelationshipProvider::ObjectBucketClaim,
            ),
            (
                "cert-manager.io",
                "Certificate",
                RelationshipProvider::Certificate,
            ),
            (
                "cert-manager.io",
                "CertificateRequest",
                RelationshipProvider::CertificateRequest,
            ),
            (
                "acme.cert-manager.io",
                "Order",
                RelationshipProvider::CertManagerOrder,
            ),
            (
                "external-secrets.io",
                "ExternalSecret",
                RelationshipProvider::ExternalSecret,
            ),
            (
                "external-secrets.io",
                "ClusterExternalSecret",
                RelationshipProvider::ClusterExternalSecret,
            ),
            (
                "kopiur.home-operations.com",
                "SnapshotPolicy",
                RelationshipProvider::KopiurPolicy,
            ),
            (
                "tuppr.home-operations.com",
                "KubernetesUpgrade",
                RelationshipProvider::TupprUpgrade,
            ),
            (
                "tuppr.home-operations.com",
                "TalosUpgrade",
                RelationshipProvider::TupprUpgrade,
            ),
            (
                "gateway.networking.k8s.io",
                "HTTPRoute",
                RelationshipProvider::GatewayRoute,
            ),
            (
                "gateway.networking.k8s.io",
                "ReferenceGrant",
                RelationshipProvider::ReferenceGrant,
            ),
            (
                "gateway.envoyproxy.io",
                "SecurityPolicy",
                RelationshipProvider::EnvoyPolicy,
            ),
        ];
        for (group, kind, provider) in cases {
            assert!(
                relationship_providers(&test_resource(group, kind, Some("app")))
                    .any(|registered| registered == provider),
                "{group}/{kind}"
            );
        }
        let mut network_policy = test_resource("networking.k8s.io", "NetworkPolicy", Some("app"));
        network_policy.version = "v1".into();
        assert_eq!(
            relationship_providers(&network_policy).collect::<Vec<_>>(),
            [RelationshipProvider::NetworkPolicyPods]
        );
    }

    #[test]
    fn object_bucket_claim_links_generated_resources() {
        let resource = test_resource("objectbucket.io", "ObjectBucketClaim", Some("storage"));
        let references = object_bucket_claim_references(
            &resource,
            &json!({"spec": {"objectBucketName": "obc-storage-media"}}),
        );
        assert_eq!(references.len(), 3);
        assert!(references.iter().any(|reference| reference.kind == "Secret"
            && reference.name == "resource"
            && reference.namespace.as_deref() == Some("storage")));
        assert!(references
            .iter()
            .any(|reference| reference.kind == "ConfigMap" && reference.name == "resource"));
        assert!(references
            .iter()
            .any(|reference| reference.kind == "ObjectBucket"
                && reference.name == "obc-storage-media"
                && reference.namespace.is_none()));
    }

    #[test]
    fn certificates_link_secrets_and_default_or_cluster_issuers() {
        let resource = test_resource("cert-manager.io", "Certificate", Some("app"));
        let references = certificate_references(
            &resource,
            &json!({"spec": {"secretName": "api-tls", "issuerRef": {"name": "letsencrypt"}}}),
        );
        assert!(references
            .iter()
            .any(|reference| reference.kind == "Secret" && reference.name == "api-tls"));
        assert!(references
            .iter()
            .any(|reference| reference.kind == "Issuer"
                && reference.namespace.as_deref() == Some("app")));

        let cluster = issuer_reference(
            &resource,
            &json!({"spec": {"issuerRef": {"name": "root", "kind": "ClusterIssuer"}}}),
        )
        .unwrap();
        assert_eq!(cluster.namespace, None);
        assert_eq!(cluster.group, "cert-manager.io");
    }

    #[test]
    fn external_secrets_link_store_and_generated_secret() {
        let resource = test_resource("external-secrets.io", "ExternalSecret", Some("app"));
        let references = external_secret_references(
            &resource,
            &json!({"spec": {
                "secretStoreRef": {"name": "vault", "kind": "ClusterSecretStore"},
                "target": {"name": "database"}
            }}),
        );
        assert!(references.iter().any(|(reference, relation)| reference.kind
            == "ClusterSecretStore"
            && reference.namespace.is_none()
            && *relation == ResourceTreeRelation::ReferencedResource));
        assert!(references
            .iter()
            .any(|(reference, relation)| reference.kind == "Secret"
                && reference.name == "database"
                && *relation == ResourceTreeRelation::GeneratedResource));

        let cluster = test_resource("external-secrets.io", "ClusterExternalSecret", None);
        let generated = cluster_external_secret_references(
            &cluster,
            &json!({"status": {"externalSecretName": "shared", "provisionedNamespaces": ["a", "b"]}}),
        );
        assert_eq!(generated.len(), 2);
        assert!(generated
            .iter()
            .all(|reference| reference.kind == "ExternalSecret" && reference.name == "shared"));
    }

    #[test]
    fn flux_links_sources_charts_and_dependencies() {
        let resource = test_resource("helm.toolkit.fluxcd.io", "HelmRelease", Some("apps"));
        let references = flux_references(
            &resource,
            &json!({"spec": {
                "chartRef": {"kind": "OCIRepository", "name": "charts", "namespace": "flux-system"},
                "dependsOn": [{"name": "database"}]
            }}),
        );
        assert!(references
            .iter()
            .any(|reference| reference.group == "source.toolkit.fluxcd.io"
                && reference.kind == "OCIRepository"
                && reference.namespace.as_deref() == Some("flux-system")));
        assert!(references
            .iter()
            .any(|reference| reference.group == "helm.toolkit.fluxcd.io"
                && reference.kind == "HelmRelease"
                && reference.name == "database"
                && reference.namespace.as_deref() == Some("apps")));
    }

    #[test]
    fn kopiur_policy_links_repositories_and_operational_children() {
        let resource = test_resource(
            "kopiur.home-operations.com",
            "SnapshotPolicy",
            Some("backup"),
        );
        let repositories = kopiur_repository_references(
            &resource,
            &json!({"spec": {"repositories": [
                {"kind": "Repository", "name": "local"},
                {"kind": "ClusterRepository", "name": "archive"}
            ]}}),
        );
        assert_eq!(repositories[0].namespace.as_deref(), Some("backup"));
        assert_eq!(repositories[1].namespace, None);

        let labels = std::collections::BTreeMap::from([("backup".into(), "daily".into())]);
        let schedule: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "kopiur.home-operations.com/v1alpha1", "kind": "SnapshotSchedule",
            "metadata": {"name": "daily"},
            "spec": {"policySelector": {"matchLabels": {"backup": "daily"}}}
        }))
        .unwrap();
        assert!(kopiur_child_matches_policy(
            "SnapshotSchedule",
            "resource",
            &labels,
            &schedule
        ));
        let restore: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "kopiur.home-operations.com/v1alpha1", "kind": "Restore",
            "metadata": {"name": "restore"},
            "spec": {"source": {"fromPolicy": {"name": "resource"}}}
        }))
        .unwrap();
        assert!(kopiur_child_matches_policy(
            "Restore", "resource", &labels, &restore
        ));
    }

    #[test]
    fn tuppr_links_every_reported_node_without_duplicates() {
        let names = tuppr_node_names(
            "TalosUpgrade",
            &json!({"status": {
                "currentNode": "worker-1",
                "currentNodes": ["worker-1", "worker-2"],
                "completedNodes": ["worker-0"],
                "failedNodes": [{"nodeName": "worker-3"}],
                "rebootingNodes": [{"nodeName": "worker-4"}]
            }}),
        );
        assert_eq!(
            names,
            ["worker-0", "worker-1", "worker-2", "worker-3", "worker-4"]
        );
        assert_eq!(
            tuppr_node_names(
                "KubernetesUpgrade",
                &json!({"status": {"controllerNode": "control-1"}})
            ),
            ["control-1"]
        );
        assert!(tuppr_node_names(
            "KubernetesUpgrade",
            &json!({"status": {"controllerNode": ""}})
        )
        .is_empty());
    }

    #[test]
    fn gateway_envoy_and_reference_grant_refs_apply_api_defaults() {
        let route = test_resource("gateway.networking.k8s.io", "HTTPRoute", Some("app"));
        let references = gateway_route_references(
            &route,
            &json!({"spec": {
                "parentRefs": [{"name": "public"}],
                "rules": [{"backendRefs": [
                    {"name": "api"},
                    {"group": "example.io", "kind": "Backend", "name": "external", "namespace": "shared"}
                ]}]
            }}),
        );
        assert!(references
            .iter()
            .any(|reference| reference.kind == "Gateway"
                && reference.group == "gateway.networking.k8s.io"));
        assert!(references
            .iter()
            .any(|reference| reference.kind == "Service"
                && reference.group.is_empty()
                && reference.namespace.as_deref() == Some("app")));
        assert!(references
            .iter()
            .any(|reference| reference.kind == "Backend"
                && reference.namespace.as_deref() == Some("shared")));

        let policy = test_resource("gateway.envoyproxy.io", "SecurityPolicy", Some("app"));
        let targets = envoy_policy_references(
            &policy,
            &json!({"spec": {"targetRefs": [
                {"kind": "HTTPRoute", "name": "api"},
                {"group": "", "kind": "Service", "name": "auth"}
            ]}}),
        );
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].group, "gateway.networking.k8s.io");
        assert!(targets[1].group.is_empty());

        let grant = test_resource(
            "gateway.networking.k8s.io",
            "ReferenceGrant",
            Some("shared"),
        );
        let data = json!({"spec": {
            "from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "app"}],
            "to": [
                {"group": "", "kind": "Service", "name": "api"},
                {"group": "", "kind": "Secret"}
            ]
        }});
        let named = reference_grant_named_targets(&grant, &data);
        assert_eq!(named[0].namespace.as_deref(), Some("shared"));
        let listed = reference_grant_list_targets(&grant, &data);
        assert_eq!(
            listed,
            [(String::new(), "Secret".into(), Some("shared".into()))]
        );
        let sources = reference_grant_sources(&data);
        assert_eq!(
            sources,
            [(
                "gateway.networking.k8s.io".into(),
                "HTTPRoute".into(),
                Some("app".into())
            )]
        );

        let matching_source: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "app", "namespace": "app"},
            "spec": {"rules": [{"backendRefs": [
                {"name": "api", "namespace": "shared"}
            ]}]}
        }))
        .unwrap();
        let unrelated_source: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "other", "namespace": "app"},
            "spec": {"rules": [{"backendRefs": [
                {"name": "other", "namespace": "elsewhere"}
            ]}]}
        }))
        .unwrap();
        let targets = reference_grant_targets(&data);
        assert!(reference_grant_source_matches(
            &matching_source,
            Some("shared"),
            &targets
        ));
        assert!(!reference_grant_source_matches(
            &unrelated_source,
            Some("shared"),
            &targets
        ));
    }
}
