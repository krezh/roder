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

    pub(crate) fn is_node(&self) -> bool {
        self.group.is_empty() && self.kind == "Node"
    }

    pub(crate) fn is_workload(&self) -> bool {
        self.group == "apps"
            && matches!(
                self.kind,
                "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet"
            )
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

    /// Scalable workloads have spec.replicas (DaemonSets do not).
    pub(crate) fn is_scalable(&self) -> bool {
        self.group == "apps" && matches!(self.kind, "Deployment" | "StatefulSet" | "ReplicaSet")
    }

    /// Only HelmRelease supports `flux reconcile --force` / `--reset`.
    pub(crate) fn is_helmrelease(&self) -> bool {
        self.is_flux() && self.kind == "HelmRelease"
    }

    /// Only Kustomization and HelmRelease can show a "Resource Tree".
    pub(crate) fn is_kustomization(&self) -> bool {
        self.is_flux() && self.kind == "Kustomization"
    }

    /// Only Kustomization and HelmRelease reference a source that
    /// `flux reconcile --with-source` can reconcile first.
    pub(crate) fn has_source_ref(&self) -> bool {
        self.is_flux() && matches!(self.kind, "Kustomization" | "HelmRelease")
    }
}

#[cfg(test)]
mod tests {
    use super::KindKind;

    #[test]
    fn is_helmrelease_true_for_flux_helmrelease() {
        let kk = KindKind::new("helm.toolkit.fluxcd.io", "HelmRelease");
        assert!(kk.is_helmrelease());
    }

    #[test]
    fn is_helmrelease_false_for_kustomization() {
        let kk = KindKind::new("kustomize.toolkit.fluxcd.io", "Kustomization");
        assert!(!kk.is_helmrelease());
    }

    #[test]
    fn is_helmrelease_false_for_non_flux_group() {
        let kk = KindKind::new("apps", "HelmRelease");
        assert!(!kk.is_helmrelease());
    }

    #[test]
    fn is_kustomization_true_for_flux_kustomization() {
        let kk = KindKind::new("kustomize.toolkit.fluxcd.io", "Kustomization");
        assert!(kk.is_kustomization());
    }

    #[test]
    fn is_kustomization_false_for_helmrelease() {
        let kk = KindKind::new("helm.toolkit.fluxcd.io", "HelmRelease");
        assert!(!kk.is_kustomization());
    }

    #[test]
    fn is_kustomization_false_for_non_flux_group() {
        let kk = KindKind::new("apps", "Kustomization");
        assert!(!kk.is_kustomization());
    }

    #[test]
    fn has_source_ref_true_for_kustomization_and_helmrelease() {
        assert!(KindKind::new("kustomize.toolkit.fluxcd.io", "Kustomization").has_source_ref());
        assert!(KindKind::new("helm.toolkit.fluxcd.io", "HelmRelease").has_source_ref());
    }

    #[test]
    fn has_source_ref_false_for_source_kinds() {
        assert!(!KindKind::new("source.toolkit.fluxcd.io", "GitRepository").has_source_ref());
        assert!(!KindKind::new("source.toolkit.fluxcd.io", "OCIRepository").has_source_ref());
        assert!(!KindKind::new("source.toolkit.fluxcd.io", "HelmRepository").has_source_ref());
        assert!(!KindKind::new("source.toolkit.fluxcd.io", "Bucket").has_source_ref());
    }
}
