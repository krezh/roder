//! Resource-kind predicates shared by the context menu and the detail view, so the
//! "is this a workload / Flux / ESO / …" logic lives in exactly one place.

/// A borrowed (group, kind) pair with the kind-classification predicates.
pub(crate) struct KindKind<'a> {
    pub(crate) group: &'a str,
    pub(crate) kind: &'a str,
}

impl<'a> KindKind<'a> {
    pub(crate) fn new(group: &'a str, kind: &'a str) -> Self {
        Self { group, kind }
    }

    pub(crate) fn is_pod(&self) -> bool {
        self.group.is_empty() && self.kind == "Pod"
    }

    pub(crate) fn is_workload(&self) -> bool {
        self.group == "apps"
            && matches!(self.kind, "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet")
    }

    pub(crate) fn is_job(&self) -> bool {
        self.group == "batch" && self.kind == "Job"
    }

    pub(crate) fn is_cronjob(&self) -> bool {
        self.group == "batch" && self.kind == "CronJob"
    }

    pub(crate) fn is_flux(&self) -> bool {
        self.group.ends_with("fluxcd.io")
    }

    pub(crate) fn is_eso(&self) -> bool {
        self.group == "external-secrets.io"
    }
}
