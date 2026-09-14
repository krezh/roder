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
    OwnedChildren,
    WorkloadPods,
    ServiceTargets,
    CnpgCluster,
    CnpgClusterReference,
    CnpgScheduledBackup,
    KubernetesStorage,
    RookStorageClass,
    RookCephCluster,
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
        group: "helm.toolkit.fluxcd.io",
        version: None,
        kind: "HelmRelease",
        provider: RelationshipProvider::FluxHelmRelease,
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
        group: "",
        version: Some("v1"),
        kind: "Service",
        provider: RelationshipProvider::ServiceTargets,
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
                RelationshipProvider::ServiceTargets => {
                    let (mut refs, mut provider_errors) =
                        self.service_relationships(resource, data, semaphore).await;
                    children.append(&mut refs);
                    errors.append(&mut provider_errors);
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
                    children.extend(storage_reference_targets(resource, data).into_iter().map(
                        |target| {
                            self.resolve_resource(
                                target.group.into(),
                                target.version.into(),
                                target.kind.into(),
                                target.name,
                                target.namespace,
                                Some(ResourceTreeRelation::ReferencedResource),
                            )
                        },
                    ));
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
            .filter_map(|child| child.metadata.name)
            .map(|name| ResourceRef {
                group: entry.kind.group.clone(),
                version: entry.kind.version.clone(),
                kind: entry.kind.kind.clone(),
                name,
                namespace: parent.namespace.clone().filter(|_| entry.kind.namespaced),
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
            .filter_map(|object| object.metadata.name)
            .map(|name| ResourceRef {
                group: entry.kind.group.clone(),
                version: entry.kind.version.clone(),
                kind: entry.kind.kind.clone(),
                name,
                namespace: namespace
                    .map(str::to_string)
                    .filter(|_| entry.kind.namespaced),
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
            .filter_map(|object| object.metadata.name)
            .map(|name| ResourceRef {
                group: entry.kind.group.clone(),
                version: entry.kind.version.clone(),
                kind: entry.kind.kind.clone(),
                name,
                namespace: namespace
                    .map(str::to_string)
                    .filter(|_| entry.kind.namespaced),
                key: Some(entry.kind.key.clone()),
                category: Some(entry.kind.category.clone()),
                relation: Some(relation),
                expandable: false,
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
}

fn reference_is_expandable(
    relation: Option<ResourceTreeRelation>,
    group: &str,
    kind: &str,
) -> bool {
    relation == Some(ResourceTreeRelation::Owner)
        || is_flux_owner(group, kind)
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
        _ => Vec::new(),
    }
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

        for kind in ["Pod", "PersistentVolumeClaim", "PersistentVolume"] {
            assert_eq!(
                relationship_providers(&resource(kind)).collect::<Vec<_>>(),
                [RelationshipProvider::KubernetesStorage]
            );
        }
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
}
