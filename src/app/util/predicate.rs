//! Resource-kind predicates shared by the context menu and the detail view, so the
//! "is this a workload / Flux / ESO / …" logic lives in exactly one place.

use roder_core::{ResourceAction, ResourceCapabilities};

/// A borrowed (group, kind) pair with the kind-classification predicates.
pub(crate) struct KindKind<'a> {
    pub(crate) group: &'a str,
    pub(crate) version: &'a str,
    pub(crate) kind: &'a str,
}

impl<'a> KindKind<'a> {
    pub(crate) fn new(group: &'a str, version: &'a str, kind: &'a str) -> Self {
        Self {
            group,
            version,
            kind,
        }
    }

    pub(crate) fn supports(&self, action: ResourceAction) -> bool {
        ResourceCapabilities::for_gvk(self.group, self.version, self.kind).supports(action)
    }

    pub(crate) fn is_pod(&self) -> bool {
        self.supports(ResourceAction::Exec)
    }

    pub(crate) fn is_node(&self) -> bool {
        self.supports(ResourceAction::Cordon)
    }

    pub(crate) fn is_workload(&self) -> bool {
        self.supports(ResourceAction::Restart)
    }

    pub(crate) fn is_job(&self) -> bool {
        self.supports(ResourceAction::JobRerun)
    }

    pub(crate) fn is_cronjob(&self) -> bool {
        self.supports(ResourceAction::CronJobTrigger)
    }

    pub(crate) fn is_flux(&self) -> bool {
        self.supports(ResourceAction::FluxReconcile)
    }

    pub(crate) fn is_eso(&self) -> bool {
        self.supports(ResourceAction::ExternalSecretsRefresh)
    }

    pub(crate) fn is_certificate(&self) -> bool {
        self.supports(ResourceAction::CertificateRenew)
    }

    pub(crate) fn is_kopiur_snapshot_policy(&self) -> bool {
        self.supports(ResourceAction::KopiurSnapshotNow)
    }

    /// Scalable workloads have spec.replicas (DaemonSets do not).
    pub(crate) fn is_scalable(&self) -> bool {
        self.supports(ResourceAction::Scale)
    }

    /// Only HelmRelease supports `flux reconcile --force` / `--reset`.
    pub(crate) fn is_helmrelease(&self) -> bool {
        self.supports(ResourceAction::FluxForce)
    }

    /// Only Kustomization and HelmRelease reference a source that
    /// `flux reconcile --with-source` can reconcile first.
    pub(crate) fn has_source_ref(&self) -> bool {
        self.supports(ResourceAction::FluxReconcileWithSource)
    }

    pub(crate) fn has_logs(&self) -> bool {
        self.supports(ResourceAction::Logs)
    }
}

#[cfg(test)]
mod tests {
    use super::KindKind;

    #[test]
    fn is_helmrelease_true_for_flux_helmrelease() {
        let kk = KindKind::new("helm.toolkit.fluxcd.io", "v2", "HelmRelease");
        assert!(kk.is_helmrelease());
    }

    #[test]
    fn is_helmrelease_false_for_kustomization() {
        let kk = KindKind::new("kustomize.toolkit.fluxcd.io", "v1", "Kustomization");
        assert!(!kk.is_helmrelease());
    }

    #[test]
    fn is_helmrelease_false_for_non_flux_group() {
        let kk = KindKind::new("apps", "v1", "HelmRelease");
        assert!(!kk.is_helmrelease());
    }

    #[test]
    fn is_kopiur_snapshot_policy_true_for_kopiur_snapshotpolicy() {
        let kk = KindKind::new("kopiur.home-operations.com", "v1alpha1", "SnapshotPolicy");
        assert!(kk.is_kopiur_snapshot_policy());
    }

    #[test]
    fn is_kopiur_snapshot_policy_false_for_other_kopiur_kinds() {
        let kk = KindKind::new("kopiur.home-operations.com", "v1alpha1", "Snapshot");
        assert!(!kk.is_kopiur_snapshot_policy());
    }

    #[test]
    fn is_kopiur_snapshot_policy_false_for_non_kopiur_group() {
        let kk = KindKind::new("external-secrets.io", "v1", "SnapshotPolicy");
        assert!(!kk.is_kopiur_snapshot_policy());
    }

    #[test]
    fn is_certificate_only_matches_cert_manager_certificates() {
        assert!(KindKind::new("cert-manager.io", "v1", "Certificate").is_certificate());
        assert!(!KindKind::new("cert-manager.io", "v1", "CertificateRequest").is_certificate());
        assert!(!KindKind::new("example.com", "v1", "Certificate").is_certificate());
    }

    #[test]
    fn has_source_ref_true_for_kustomization_and_helmrelease() {
        assert!(
            KindKind::new("kustomize.toolkit.fluxcd.io", "v1", "Kustomization").has_source_ref()
        );
        assert!(KindKind::new("helm.toolkit.fluxcd.io", "v2", "HelmRelease").has_source_ref());
    }

    #[test]
    fn has_source_ref_false_for_source_kinds() {
        assert!(!KindKind::new("source.toolkit.fluxcd.io", "v1", "GitRepository").has_source_ref());
        assert!(!KindKind::new("source.toolkit.fluxcd.io", "v1", "OCIRepository").has_source_ref());
        assert!(
            !KindKind::new("source.toolkit.fluxcd.io", "v1", "HelmRepository").has_source_ref()
        );
        assert!(!KindKind::new("source.toolkit.fluxcd.io", "v1", "Bucket").has_source_ref());
    }
}
