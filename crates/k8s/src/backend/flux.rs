//! Flux-specific reconcile/suspend actions, plus resolving a reconciling
//! object's `sourceRef` so `--with-source` can reconcile the source first.

use futures::future::join_all;
use kube::api::{ListParams, Patch, PatchParams};
use roder_core::Category;
use serde_json::json;

use super::{api_err, now_rfc3339, Backend};
use crate::client::{make_api, K8sError};

impl Backend {
    pub async fn flux_suspend(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
        suspend: bool,
    ) -> Result<(), K8sError> {
        self.merge_patch(key, ns, name, json!({ "spec": { "suspend": suspend } }))
            .await
    }

    /// `flux reconcile [--force] [--reset]`: `force`/`reset` mirror the CLI
    /// flags and can be combined in one call.
    pub async fn flux_reconcile(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
        force: bool,
        reset: bool,
    ) -> Result<(), K8sError> {
        let ts = now_rfc3339();
        let mut annotations = json!({ "reconcile.fluxcd.io/requestedAt": ts });
        if force {
            annotations["reconcile.fluxcd.io/forceAt"] = json!(ts);
        }
        if reset {
            annotations["reconcile.fluxcd.io/resetAt"] = json!(ts);
        }
        let patch = json!({ "metadata": { "annotations": annotations }});
        self.merge_patch(key, ns, name, patch).await
    }

    /// `flux reconcile helmrelease --force`: force a one-off Helm install/upgrade.
    pub async fn flux_force(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        self.flux_reconcile(key, ns, name, true, false).await
    }

    /// `flux reconcile helmrelease --reset`: reset the failure count on a stuck HelmRelease.
    pub async fn flux_reset(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        self.flux_reconcile(key, ns, name, false, true).await
    }

    /// `flux reconcile <kind> --with-source`: reconcile the referenced source
    /// (GitRepository/OCIRepository/HelmRepository/Bucket/HelmChart) first,
    /// then the object itself.
    pub async fn flux_reconcile_with_source(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
        force: bool,
        reset: bool,
    ) -> Result<(), K8sError> {
        let obj = self.dyn_api(key, ns)?.get(name).await.map_err(api_err)?;
        let data = serde_json::to_value(&obj).map_err(api_err)?;
        let spec = data
            .get("spec")
            .ok_or_else(|| K8sError::Api("resource has no spec".into()))?;
        let source = extract_source_ref(spec)
            .ok_or_else(|| K8sError::Api("resource has no sourceRef to reconcile".into()))?;
        let source_entry = self.entry_by_kind(&source.kind)?;
        let source_ns = source.namespace.as_deref().or(ns);
        // force/reset are flags on the resource's own reconcile (matching
        // `flux reconcile --with-source --force`), not the source's.
        self.flux_reconcile(
            &source_entry.kind.key,
            source_ns,
            &source.name,
            false,
            false,
        )
        .await?;
        self.flux_reconcile(key, ns, name, force, reset).await
    }

    /// `flux reconcile <kind> --all`, for every Flux kind at once: annotate every
    /// discovered `*.fluxcd.io` resource (optionally scoped to one namespace) with
    /// `reconcile.fluxcd.io/requestedAt`, requesting an immediate reconciliation.
    /// Lists/patches each kind concurrently; best-effort like `sanitize` — a
    /// failed list or patch is skipped rather than aborting the whole sweep.
    pub async fn flux_reconcile_all(&self, namespace: Option<&str>) -> Result<usize, K8sError> {
        let client = self.client();
        let ts = now_rfc3339();
        let catalog_store = self.shared.catalog();
        let catalog = catalog_store.load();
        let futs = catalog
            .entries
            .iter()
            .filter(|e| e.kind.category == Category::Flux)
            .map(|entry| {
                let client = client.clone();
                let ts = ts.clone();
                async move {
                    let list_api = make_api(
                        client.clone(),
                        &entry.api_resource,
                        entry.kind.namespaced,
                        namespace,
                    );
                    let Ok(list) = list_api.list(&ListParams::default()).await else {
                        return 0usize;
                    };
                    let patch = Patch::Merge(json!({ "metadata": { "annotations": {
                        "reconcile.fluxcd.io/requestedAt": ts
                    }}}));
                    let patches = list.items.into_iter().filter_map(|obj| {
                        let name = obj.metadata.name?;
                        let ns = obj.metadata.namespace;
                        let api = make_api(
                            client.clone(),
                            &entry.api_resource,
                            entry.kind.namespaced,
                            ns.as_deref(),
                        );
                        let patch = patch.clone();
                        Some(async move {
                            api.patch(&name, &PatchParams::default(), &patch)
                                .await
                                .is_ok()
                        })
                    });
                    join_all(patches).await.into_iter().filter(|ok| *ok).count()
                }
            });
        Ok(join_all(futs).await.into_iter().sum())
    }
}

/// A Flux source reference resolved from a reconciling object's spec.
struct SourceRef {
    kind: String,
    name: String,
    namespace: Option<String>,
}

/// Find the sourceRef a Kustomization or HelmRelease reconciles against, trying
/// each field Flux supports in turn: `spec.sourceRef` (Kustomization),
/// `spec.chart.spec.sourceRef` (HelmRelease templated chart), and
/// `spec.chartRef` (HelmRelease direct OCIRepository/HelmChart reference).
fn extract_source_ref(spec: &serde_json::Value) -> Option<SourceRef> {
    const PATHS: &[&[&str]] = &[
        &["sourceRef"],
        &["chart", "spec", "sourceRef"],
        &["chartRef"],
    ];
    for path in PATHS {
        let mut cur = Some(spec);
        for seg in *path {
            cur = cur.and_then(|c| c.get(seg));
        }
        let Some(cur) = cur else { continue };
        let kind = cur.get("kind").and_then(|v| v.as_str());
        let name = cur.get("name").and_then(|v| v.as_str());
        if let (Some(kind), Some(name)) = (kind, name) {
            return Some(SourceRef {
                kind: kind.to_string(),
                name: name.to_string(),
                namespace: cur
                    .get("namespace")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            });
        }
    }
    None
}

#[cfg(test)]
mod source_ref_tests {
    use super::extract_source_ref;
    use serde_json::json;

    #[test]
    fn kustomization_source_ref() {
        let spec = json!({ "sourceRef": { "kind": "GitRepository", "name": "flux-system" } });
        let sr = extract_source_ref(&spec).expect("source ref");
        assert_eq!(sr.kind, "GitRepository");
        assert_eq!(sr.name, "flux-system");
        assert_eq!(sr.namespace, None);
    }

    #[test]
    fn kustomization_source_ref_with_namespace() {
        let spec = json!({ "sourceRef": {
            "kind": "GitRepository", "name": "podinfo", "namespace": "apps"
        }});
        let sr = extract_source_ref(&spec).expect("source ref");
        assert_eq!(sr.namespace.as_deref(), Some("apps"));
    }

    #[test]
    fn helmrelease_templated_chart_source_ref() {
        let spec = json!({ "chart": { "spec": {
            "chart": "podinfo",
            "sourceRef": { "kind": "HelmRepository", "name": "podinfo" }
        }}});
        let sr = extract_source_ref(&spec).expect("source ref");
        assert_eq!(sr.kind, "HelmRepository");
        assert_eq!(sr.name, "podinfo");
    }

    #[test]
    fn helmrelease_direct_chart_ref() {
        let spec = json!({ "chartRef": { "kind": "OCIRepository", "name": "podinfo" } });
        let sr = extract_source_ref(&spec).expect("source ref");
        assert_eq!(sr.kind, "OCIRepository");
        assert_eq!(sr.name, "podinfo");
    }

    #[test]
    fn prefers_source_ref_over_chart_ref_when_both_present() {
        // Shouldn't happen in practice, but sourceRef (Kustomization's own
        // field) should win if a spec somehow has both.
        let spec = json!({
            "sourceRef": { "kind": "GitRepository", "name": "a" },
            "chartRef": { "kind": "OCIRepository", "name": "b" },
        });
        let sr = extract_source_ref(&spec).expect("source ref");
        assert_eq!(sr.kind, "GitRepository");
    }

    #[test]
    fn none_when_no_source_ref_present() {
        assert!(extract_source_ref(&json!({ "suspend": false })).is_none());
    }

    #[test]
    fn none_when_source_ref_missing_required_fields() {
        assert!(extract_source_ref(&json!({ "sourceRef": { "kind": "GitRepository" } })).is_none());
    }
}
