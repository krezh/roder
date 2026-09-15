//! Which actions a kind can offer at all, and which RBAC actually permits on a given object or selection.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::resource::ResourceAction;

/// Static operations supported by one GroupVersionKind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceCapabilities(u32);

impl ResourceCapabilities {
    pub fn for_gvk(group: &str, _version: &str, kind: &str) -> Self {
        let mut bits = bit(ResourceAction::Delete);

        if group.is_empty() && kind == "Pod" {
            bits |= bits_for(&[
                ResourceAction::Evict,
                ResourceAction::Logs,
                ResourceAction::Exec,
                ResourceAction::DebugExec,
            ]);
        }
        if group.is_empty() && kind == "Node" {
            bits |= bits_for(&[
                ResourceAction::Cordon,
                ResourceAction::Drain,
                ResourceAction::NodeShell,
            ]);
        }
        if group == "apps"
            && matches!(
                kind,
                "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet"
            )
        {
            bits |= bits_for(&[ResourceAction::Restart, ResourceAction::Logs]);
        }
        if group == "apps" && matches!(kind, "Deployment" | "StatefulSet" | "ReplicaSet") {
            bits |= bit(ResourceAction::Scale);
        }
        if group == "batch" && kind == "Job" {
            bits |= bits_for(&[ResourceAction::JobRerun, ResourceAction::Logs]);
        }
        if group == "batch" && kind == "CronJob" {
            bits |= bit(ResourceAction::CronJobTrigger);
        }
        if flux_reconcile_kind(group, kind) {
            bits |= bit(ResourceAction::FluxReconcile);
        }
        if flux_suspend_kind(group, kind) {
            bits |= bit(ResourceAction::FluxSuspend);
        }
        if matches!(
            (group, kind),
            ("kustomize.toolkit.fluxcd.io", "Kustomization")
                | ("helm.toolkit.fluxcd.io", "HelmRelease")
        ) {
            bits |= bit(ResourceAction::FluxReconcileWithSource);
        }
        if group == "helm.toolkit.fluxcd.io" && kind == "HelmRelease" {
            bits |= bits_for(&[ResourceAction::FluxForce, ResourceAction::FluxReset]);
        }
        if group == "external-secrets.io"
            && matches!(kind, "ExternalSecret" | "ClusterExternalSecret")
        {
            bits |= bit(ResourceAction::ExternalSecretsRefresh);
        }
        if group == "cert-manager.io" && kind == "Certificate" {
            bits |= bit(ResourceAction::CertificateRenew);
        }
        if group == "kopiur.home-operations.com" && kind == "SnapshotPolicy" {
            bits |= bit(ResourceAction::KopiurSnapshotNow);
        }

        Self(bits)
    }

    pub fn supports(self, action: ResourceAction) -> bool {
        self.0 & bit(action) != 0
    }
}

fn flux_reconcile_kind(group: &str, kind: &str) -> bool {
    matches!(
        (group, kind),
        ("kustomize.toolkit.fluxcd.io", "Kustomization")
            | ("helm.toolkit.fluxcd.io", "HelmRelease")
            | ("source.toolkit.fluxcd.io", "GitRepository")
            | ("source.toolkit.fluxcd.io", "OCIRepository")
            | ("source.toolkit.fluxcd.io", "HelmRepository")
            | ("source.toolkit.fluxcd.io", "Bucket")
            | ("source.toolkit.fluxcd.io", "HelmChart")
            | ("image.toolkit.fluxcd.io", "ImageRepository")
            | ("image.toolkit.fluxcd.io", "ImagePolicy")
            | ("image.toolkit.fluxcd.io", "ImageUpdateAutomation")
            | ("notification.toolkit.fluxcd.io", "Receiver")
    )
}

fn flux_suspend_kind(group: &str, kind: &str) -> bool {
    flux_reconcile_kind(group, kind)
        || matches!(
            (group, kind),
            ("notification.toolkit.fluxcd.io", "Alert")
                | ("notification.toolkit.fluxcd.io", "Provider")
        )
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionPermissions {
    pub apply: bool,
    pub actions: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionSummary {
    pub attempted: usize,
    pub succeeded: usize,
    pub forbidden: usize,
    pub failed: usize,
}

impl ActionSummary {
    pub fn merge(&mut self, other: Self) {
        self.attempted += other.attempted;
        self.succeeded += other.succeeded;
        self.forbidden += other.forbidden;
        self.failed += other.failed;
    }
}

impl ActionPermissions {
    pub fn allows(&self, action: ResourceAction) -> bool {
        self.actions
            .get(action.api_name())
            .copied()
            .unwrap_or(false)
    }

    pub fn allows_api_name(&self, name: &str) -> bool {
        ResourceAction::from_api_name(name).is_some_and(|action| self.allows(action))
    }
}

fn bit(action: ResourceAction) -> u32 {
    1 << action as u8
}

fn bits_for(actions: &[ResourceAction]) -> u32 {
    actions.iter().fold(0, |bits, action| bits | bit(*action))
}

/// One resource kind's RBAC access review row in the requested namespace scope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessRow {
    pub kind: String,
    pub group: String,
    pub namespaced: bool,
    /// `None` means the operation does not apply to this kind.
    pub operations: Vec<(String, Option<bool>)>,
}

/// Operations checked by the access review, in display order.
pub const ACCESS_REVIEW_OPERATIONS: &[&str] = &[
    "get", "list", "watch", "create", "patch", "delete", "status", "logs", "exec", "evict", "deps",
];
