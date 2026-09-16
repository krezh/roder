//! What a resource *is*: its kind identity, the actions it supports, and the condition/status readers those decisions are made from.

use serde::{Deserialize, Serialize};

use crate::health::Category;
use crate::permissions::ResourceCapabilities;

/// A resource type (GroupVersionKind) the user may browse, as surfaced to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceKind {
    /// Stable id used in URLs / SSE: `group/version/kind` (group empty for core).
    pub key: String,
    pub group: String,
    pub version: String,
    pub kind: String,
    pub plural: String,
    pub namespaced: bool,
    pub category: Category,
}

impl ResourceKind {
    pub fn make_key(group: &str, version: &str, kind: &str) -> String {
        format!("{group}/{version}/{kind}")
    }

    pub fn capabilities(&self) -> ResourceCapabilities {
        ResourceCapabilities::for_gvk(&self.group, &self.version, &self.kind)
    }

    pub fn supports(&self, action: ResourceAction) -> bool {
        self.capabilities().supports(action)
    }
}

/// A resource-scoped operation exposed by Roder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ResourceAction {
    Delete,
    Evict,
    Scale,
    Restart,
    Cordon,
    Drain,
    Logs,
    Exec,
    DebugExec,
    NodeShell,
    FluxReconcile,
    FluxReconcileWithSource,
    FluxSuspend,
    FluxForce,
    FluxReset,
    ExternalSecretsRefresh,
    CertificateRenew,
    CronJobTrigger,
    JobRerun,
    KopiurSnapshotNow,
    CnpgBackup,
    CnpgSuspend,
}

impl ResourceAction {
    pub const ALL: [Self; 22] = [
        Self::Delete,
        Self::Evict,
        Self::Scale,
        Self::Restart,
        Self::Cordon,
        Self::Drain,
        Self::Logs,
        Self::Exec,
        Self::DebugExec,
        Self::NodeShell,
        Self::FluxReconcile,
        Self::FluxReconcileWithSource,
        Self::FluxSuspend,
        Self::FluxForce,
        Self::FluxReset,
        Self::ExternalSecretsRefresh,
        Self::CertificateRenew,
        Self::CronJobTrigger,
        Self::JobRerun,
        Self::KopiurSnapshotNow,
        Self::CnpgBackup,
        Self::CnpgSuspend,
    ];

    pub const fn api_name(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Evict => "evict",
            Self::Scale => "scale",
            Self::Restart => "restart",
            Self::Cordon => "cordon",
            Self::Drain => "drain",
            Self::Logs => "logs",
            Self::Exec => "exec",
            Self::DebugExec => "debug-exec",
            Self::NodeShell => "node-shell",
            Self::FluxReconcile => "flux-reconcile",
            Self::FluxReconcileWithSource => "flux-reconcile-with-source",
            Self::FluxSuspend => "flux-suspend",
            Self::FluxForce => "flux-force",
            Self::FluxReset => "flux-reset",
            Self::ExternalSecretsRefresh => "eso-refresh",
            Self::CertificateRenew => "certificate-renew",
            Self::CronJobTrigger => "cronjob-trigger",
            Self::JobRerun => "job-rerun",
            Self::KopiurSnapshotNow => "kopiur-snapshot-now",
            Self::CnpgBackup => "cnpg-backup",
            Self::CnpgSuspend => "cnpg-suspend",
        }
    }

    pub fn from_api_name(name: &str) -> Option<Self> {
        Some(match name {
            "delete" => Self::Delete,
            "evict" => Self::Evict,
            "scale" => Self::Scale,
            "restart" => Self::Restart,
            "cordon" | "uncordon" => Self::Cordon,
            "drain" => Self::Drain,
            "flux-reconcile" => Self::FluxReconcile,
            "flux-reconcile-with-source" => Self::FluxReconcileWithSource,
            "flux-suspend" | "flux-resume" => Self::FluxSuspend,
            "flux-force" => Self::FluxForce,
            "flux-reset" => Self::FluxReset,
            "eso-refresh" => Self::ExternalSecretsRefresh,
            "certificate-renew" => Self::CertificateRenew,
            "cronjob-trigger" => Self::CronJobTrigger,
            "job-rerun" => Self::JobRerun,
            "kopiur-snapshot-now" => Self::KopiurSnapshotNow,
            "cnpg-backup" => Self::CnpgBackup,
            "cnpg-suspend" | "cnpg-resume" => Self::CnpgSuspend,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobLifecycle {
    Complete,
    Failed,
    Failing,
    Suspended,
    Completing,
    Running,
}

impl JobLifecycle {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Failed)
    }
}

pub fn job_lifecycle(data: &serde_json::Value) -> JobLifecycle {
    if condition_is(data, "Failed", "True") {
        JobLifecycle::Failed
    } else if condition_is(data, "FailureTarget", "True") {
        JobLifecycle::Failing
    } else if condition_is(data, "Complete", "True") {
        JobLifecycle::Complete
    } else if data
        .pointer("/spec/suspend")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        || condition_is(data, "Suspended", "True")
    {
        JobLifecycle::Suspended
    } else if condition_is(data, "SuccessCriteriaMet", "True") {
        JobLifecycle::Completing
    } else {
        JobLifecycle::Running
    }
}

pub fn current_condition<'a>(
    data: &'a serde_json::Value,
    type_: &str,
) -> Option<&'a serde_json::Value> {
    let generation = data
        .pointer("/metadata/generation")
        .and_then(serde_json::Value::as_i64);
    let conditions = data.pointer("/status/conditions")?.as_array()?;
    let condition = current_condition_from(conditions, generation, type_)?;
    if condition.get("observedGeneration").is_none() && status_generation_is_stale(data) {
        None
    } else {
        Some(condition)
    }
}

pub fn current_condition_from<'a>(
    conditions: &'a [serde_json::Value],
    generation: Option<i64>,
    type_: &str,
) -> Option<&'a serde_json::Value> {
    let matching = || {
        conditions.iter().rev().filter(|condition| {
            condition.get("type").and_then(serde_json::Value::as_str) == Some(type_)
        })
    };
    let Some(generation) = generation else {
        return matching().next();
    };
    matching()
        .find(|condition| {
            condition
                .get("observedGeneration")
                .and_then(serde_json::Value::as_i64)
                == Some(generation)
        })
        .or_else(|| matching().find(|condition| condition.get("observedGeneration").is_none()))
}

pub fn condition_is(data: &serde_json::Value, type_: &str, status: &str) -> bool {
    current_condition(data, type_)
        .and_then(|condition| condition.get("status"))
        .and_then(serde_json::Value::as_str)
        == Some(status)
}

pub fn status_generation_is_stale(data: &serde_json::Value) -> bool {
    data.pointer("/metadata/generation")
        .and_then(serde_json::Value::as_i64)
        .zip(
            data.pointer("/status/observedGeneration")
                .and_then(serde_json::Value::as_i64),
        )
        .is_some_and(|(generation, observed)| observed < generation)
}
