//! The resource relationship graph.

use serde::{Deserialize, Serialize};

use crate::health::Category;
use crate::row::RowStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceTreeRelation {
    Owner,
    OwnedResource,
    SelectedPod,
    EndpointSlice,
    FluxInventory,
    HelmManifest,
    ReferencedResource,
    ClusterResource,
    GeneratedResource,
    StorageBackend,
    VolumeAttachment,
}

impl ResourceTreeRelation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::OwnedResource => "Owned resource",
            Self::SelectedPod => "Selected pod",
            Self::EndpointSlice => "Endpoint slice",
            Self::FluxInventory => "Flux inventory",
            Self::HelmManifest => "Helm manifest",
            Self::ReferencedResource => "Referenced resource",
            Self::ClusterResource => "Cluster resource",
            Self::GeneratedResource => "Generated resource",
            Self::StorageBackend => "Storage backend",
            Self::VolumeAttachment => "Volume attachment",
        }
    }
}

/// One node in a server-resolved resource relationship tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceTreeNode {
    /// The resource's Kind, e.g. "Kustomization", "HelmRelease", "Deployment".
    pub kind: String,
    /// The resource's API group (empty for core/v1 kinds).
    pub group: String,
    pub name: String,
    pub namespace: Option<String>,
    /// Resolved "group/version/Kind" catalog key (matches every other `key`
    /// used across the app, e.g. `ResourceKind::key`/`DetailTarget::key`), for
    /// opening this node in the detail drawer. `None` when the kind isn't
    /// currently discoverable in the cluster's catalog — the row renders
    /// greyed/non-clickable with a tooltip instead of erroring.
    pub key: Option<String>,
    /// The kind's sidebar category (Workloads/Flux/Rbac/…), for icon/color
    /// selection. `None` only when `key` is also `None` (kind not in the
    /// current catalog).
    pub category: Option<Category>,
    /// Status projected from the fetched object. Unresolved leaves carry `None`.
    pub status: Option<RowStatus>,
    /// How this node relates to its parent. The root has no relation.
    pub relation: Option<ResourceTreeRelation>,
    /// Whether this node should render as a collapsible branch even when its
    /// children could not be resolved or it has no relationships.
    pub expandable: bool,
    pub children: Vec<ResourceTreeNode>,
    /// Best-effort note when this node's *children* couldn't be (fully)
    /// resolved — RBAC denied reading the inventory/Helm secret, HelmRelease
    /// has no deployed revision yet, the recursion depth cap was hit, etc.
    /// `None` means either a leaf (no children expected) or children resolved
    /// cleanly (which may still mean zero children, e.g. an empty inventory).
    pub error: Option<String>,
}
