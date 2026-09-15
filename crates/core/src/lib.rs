//! Shared data-transfer types used by both the Leptos UI (wasm + ssr) and the
//! server/k8s layer. Keep this crate dependency-light and wasm-safe: no tokio,
//! no kube-rs, no anything that can't compile to `wasm32-unknown-unknown`.
//!
//! Grouped by the concern each type serves rather than kept as one flat list,
//! so a change to (say) drain bookkeeping touches one small file. Everything is
//! re-exported at the crate root, so `roder_core::Foo` keeps working.

mod alerts;
mod drain;
mod files;
mod health;
mod metrics;
mod overview;
mod permissions;
mod resource;
mod row;
mod sweep;
mod talos;
mod tree;

pub use alerts::*;
pub use drain::*;
pub use files::*;
pub use health::*;
pub use metrics::*;
pub use overview::*;
pub use permissions::*;
pub use resource::*;
pub use row::*;
pub use sweep::*;
pub use talos::*;
pub use tree::*;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn capabilities(group: &str, version: &str, kind: &str) -> ResourceCapabilities {
        ResourceCapabilities::for_gvk(group, version, kind)
    }

    #[test]
    fn workload_capabilities_distinguish_scaling() {
        let deployment = capabilities("apps", "v1", "Deployment");
        assert!(deployment.supports(ResourceAction::Restart));
        assert!(deployment.supports(ResourceAction::Scale));

        let daemon_set = capabilities("apps", "v1", "DaemonSet");
        assert!(daemon_set.supports(ResourceAction::Restart));
        assert!(!daemon_set.supports(ResourceAction::Scale));

        let pod = capabilities("", "v1", "Pod");
        assert!(pod.supports(ResourceAction::Exec));
        assert!(pod.supports(ResourceAction::DebugExec));

        let node = capabilities("", "v1", "Node");
        assert!(node.supports(ResourceAction::NodeShell));
        assert!(!pod.supports(ResourceAction::NodeShell));
    }

    #[test]
    fn flux_capabilities_distinguish_specialized_operations() {
        let kustomization = capabilities("kustomize.toolkit.fluxcd.io", "v1", "Kustomization");
        assert!(kustomization.supports(ResourceAction::FluxReconcile));
        assert!(kustomization.supports(ResourceAction::FluxReconcileWithSource));
        assert!(!kustomization.supports(ResourceAction::FluxForce));

        let helm_release = capabilities("helm.toolkit.fluxcd.io", "v2", "HelmRelease");
        assert!(helm_release.supports(ResourceAction::FluxForce));
        assert!(helm_release.supports(ResourceAction::FluxReset));

        let source = capabilities("source.toolkit.fluxcd.io", "v1", "GitRepository");
        assert!(source.supports(ResourceAction::FluxReconcile));
        assert!(source.supports(ResourceAction::FluxSuspend));
        assert!(!source.supports(ResourceAction::FluxReconcileWithSource));

        let receiver = capabilities("notification.toolkit.fluxcd.io", "v1", "Receiver");
        assert!(receiver.supports(ResourceAction::FluxReconcile));
        assert!(receiver.supports(ResourceAction::FluxSuspend));
    }

    #[test]
    fn flux_actions_require_exact_groups_and_kinds() {
        let cases = [
            ("source.toolkit.fluxcd.io", "GitRepository", true, true),
            ("source.toolkit.fluxcd.io", "OCIRepository", true, true),
            ("source.toolkit.fluxcd.io", "HelmRepository", true, true),
            ("source.toolkit.fluxcd.io", "Bucket", true, true),
            ("source.toolkit.fluxcd.io", "HelmChart", true, true),
            ("kustomize.toolkit.fluxcd.io", "Kustomization", true, true),
            ("helm.toolkit.fluxcd.io", "HelmRelease", true, true),
            ("image.toolkit.fluxcd.io", "ImageRepository", true, true),
            ("image.toolkit.fluxcd.io", "ImagePolicy", true, true),
            (
                "image.toolkit.fluxcd.io",
                "ImageUpdateAutomation",
                true,
                true,
            ),
            ("notification.toolkit.fluxcd.io", "Receiver", true, true),
            ("notification.toolkit.fluxcd.io", "Alert", false, true),
            ("notification.toolkit.fluxcd.io", "Provider", false, true),
            ("example.fluxcd.io", "HelmRelease", false, false),
        ];
        for (group, kind, reconcile, suspend) in cases {
            let capabilities = capabilities(group, "v1", kind);
            assert_eq!(
                capabilities.supports(ResourceAction::FluxReconcile),
                reconcile,
                "reconcile capability for {group}/{kind}"
            );
            assert_eq!(
                capabilities.supports(ResourceAction::FluxSuspend),
                suspend,
                "suspend capability for {group}/{kind}"
            );
        }
    }

    #[test]
    fn operator_actions_do_not_leak_to_other_kinds() {
        let service = capabilities("", "v1", "Service");
        assert!(service.supports(ResourceAction::Delete));
        assert!(!service.supports(ResourceAction::ExternalSecretsRefresh));
        assert!(!service.supports(ResourceAction::CertificateRenew));
        assert!(!service.supports(ResourceAction::KopiurSnapshotNow));

        let store = capabilities("external-secrets.io", "v1", "SecretStore");
        assert!(!store.supports(ResourceAction::ExternalSecretsRefresh));
        let external_secret = capabilities("external-secrets.io", "v1", "ExternalSecret");
        assert!(external_secret.supports(ResourceAction::ExternalSecretsRefresh));
        let cluster_external_secret =
            capabilities("external-secrets.io", "v1", "ClusterExternalSecret");
        assert!(cluster_external_secret.supports(ResourceAction::ExternalSecretsRefresh));
    }

    #[test]
    fn api_action_names_map_to_semantic_capabilities() {
        assert_eq!(
            ResourceAction::from_api_name("flux-resume"),
            Some(ResourceAction::FluxSuspend)
        );
        assert_eq!(
            ResourceAction::from_api_name("uncordon"),
            Some(ResourceAction::Cordon)
        );
        assert_eq!(ResourceAction::from_api_name("apply"), None);
        assert_eq!(
            ResourceAction::from_api_name("flux-resume")
                .unwrap()
                .api_name(),
            "flux-suspend"
        );
    }

    #[test]
    fn action_permissions_use_canonical_names_for_aliases() {
        let permissions = ActionPermissions {
            apply: false,
            actions: BTreeMap::from([("flux-suspend".into(), true)]),
        };
        assert!(permissions.allows_api_name("flux-suspend"));
        assert!(permissions.allows_api_name("flux-resume"));
        assert!(!permissions.allows_api_name("flux-reconcile"));
    }

    #[test]
    fn job_lifecycle_uses_current_generation_conditions() {
        let job = serde_json::json!({
            "metadata": {"generation": 3},
            "status": {"conditions": [
                {"type": "Complete", "status": "True", "observedGeneration": 2},
                {"type": "FailureTarget", "status": "True", "observedGeneration": 3}
            ]}
        });

        assert_eq!(job_lifecycle(&job), JobLifecycle::Failing);
        assert!(!job_lifecycle(&job).is_terminal());
    }

    #[test]
    fn action_summaries_merge_all_outcomes() {
        let mut summary = ActionSummary {
            attempted: 2,
            succeeded: 1,
            forbidden: 1,
            failed: 0,
        };
        summary.merge(ActionSummary {
            attempted: 1,
            succeeded: 0,
            forbidden: 0,
            failed: 1,
        });
        assert_eq!(summary.attempted, 3);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.forbidden, 1);
        assert_eq!(summary.failed, 1);
    }

    #[test]
    fn sweep_options_select_only_requested_resource_types() {
        let pods = SweepOptions {
            completed_jobs: false,
            failed_jobs: false,
            ..Default::default()
        };
        assert!(pods.includes_pods());
        assert!(!pods.includes_jobs());

        let jobs = SweepOptions {
            terminal_pods: false,
            stuck_pods: false,
            completed_jobs: true,
            ..Default::default()
        };
        assert!(!jobs.includes_pods());
        assert!(jobs.includes_jobs());
    }

    #[test]
    fn format_age_seconds() {
        assert_eq!(format_age_secs(0), "0s");
        assert_eq!(format_age_secs(45), "45s");
        assert_eq!(format_age_secs(59), "59s");
    }

    #[test]
    fn format_age_minutes() {
        assert_eq!(format_age_secs(60), "1m");
        assert_eq!(format_age_secs(90), "1m");
        assert_eq!(format_age_secs(3599), "59m");
    }

    #[test]
    fn format_age_hours() {
        assert_eq!(format_age_secs(3600), "1h0m");
        assert_eq!(format_age_secs(3660), "1h1m");
        assert_eq!(format_age_secs(86399), "23h59m");
    }

    #[test]
    fn format_age_days() {
        assert_eq!(format_age_secs(86400), "1d0h");
        assert_eq!(format_age_secs(86400 + 3600 * 5), "1d5h");
        assert_eq!(format_age_secs(86400 * 7 + 3600 * 12), "7d12h");
    }

    #[test]
    fn drain_options_defaults() {
        let o = DrainOptions::default();
        assert!(!o.force && !o.delete_emptydir_data && !o.disable_eviction);
        assert!(!o.ignore_daemonsets);
        assert_eq!(o.timeout_secs, 0);
        assert_eq!(o.grace_period, None);
        // An empty JSON object deserializes to the same defaults.
        let from_empty: DrainOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(from_empty, o);
    }

    #[test]
    fn drain_options_validates_timeout_boundaries() {
        let mut options = DrainOptions {
            timeout_secs: DRAIN_TIMEOUT_MIN_SECS,
            ..Default::default()
        };
        assert_eq!(options.validate(), Ok(()));

        options.timeout_secs = DRAIN_TIMEOUT_MAX_SECS;
        assert_eq!(options.validate(), Ok(()));

        options.timeout_secs = DRAIN_TIMEOUT_MAX_SECS + 1;
        assert_eq!(
            options.validate(),
            Err("timeout must not exceed 3600 seconds")
        );
    }

    #[test]
    fn drain_options_validates_grace_period_boundary() {
        let mut options = DrainOptions {
            grace_period: Some(DRAIN_GRACE_PERIOD_MAX_SECS),
            ..Default::default()
        };
        assert_eq!(options.validate(), Ok(()));

        options.grace_period = Some(DRAIN_GRACE_PERIOD_MAX_SECS + 1);
        assert_eq!(
            options.validate(),
            Err("grace period must not exceed 86400 seconds")
        );

        options.grace_period = None;
        assert_eq!(options.validate(), Ok(()));
    }

    #[test]
    fn drain_event_round_trips() {
        let ev = DrainEvent {
            seq: 3,
            kind: DrainEventKind::Blocked {
                blockers: vec![DrainBlocker {
                    pod: "standalone".into(),
                    reason: "unmanaged pod".into(),
                    clearable_by: "force".into(),
                }],
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"kind\":\"blocked\""));
        assert_eq!(serde_json::from_str::<DrainEvent>(&json).unwrap(), ev);
    }

    #[test]
    fn alert_silence_duration_requires_an_explicit_kind() {
        let forever: SilenceAlertRequest = serde_json::from_value(serde_json::json!({
            "fingerprint": "abc",
            "duration": { "kind": "forever" },
            "matcher_labels": ["alertname"]
        }))
        .unwrap();
        assert_eq!(forever.duration, AlertSilenceDuration::Forever);

        assert!(
            serde_json::from_value::<SilenceAlertRequest>(serde_json::json!({
                "fingerprint": "abc"
            }))
            .is_err()
        );
    }

    #[test]
    fn firing_alert_accepts_cached_payload_without_resource_targets() {
        let alert: FiringAlert = serde_json::from_value(serde_json::json!({
            "fingerprint": "abc",
            "name": "PodDown",
            "severity": "warning",
            "summary": "",
            "description": "",
            "starts_at": "2026-01-01T00:00:00Z",
            "labels": {},
            "silenced": false
        }))
        .unwrap();

        assert!(alert.targets.is_empty());
        assert!(alert.defining_rules.is_empty());
    }
}
