//! Configurable sweep of terminal, unhealthy, or restarted pods and finished jobs.

use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, DeleteParams, ListParams};
use roder_core::{CleanupSummary, SweepCounts, SweepOptions};

use super::{api_err, Backend};
use crate::client::K8sError;

impl Backend {
    async fn sweep_candidates(
        &self,
        namespace: Option<&str>,
        options: SweepOptions,
    ) -> Result<(Vec<Pod>, Vec<Job>), K8sError> {
        let pods = if options.includes_pods() {
            let api: Api<Pod> = match namespace {
                Some(ns) => Api::namespaced(self.client(), ns),
                None => Api::all(self.client()),
            };
            api.list(&ListParams::default())
                .await
                .map_err(api_err)?
                .items
                .into_iter()
                .filter(|pod| should_sweep_pod(pod, options))
                .collect()
        } else {
            Vec::new()
        };

        let jobs = if options.includes_jobs() {
            let api: Api<Job> = match namespace {
                Some(ns) => Api::namespaced(self.client(), ns),
                None => Api::all(self.client()),
            };
            api.list(&ListParams::default())
                .await
                .map_err(api_err)?
                .items
                .into_iter()
                .filter(|job| should_sweep_job(job, options))
                .collect()
        } else {
            Vec::new()
        };
        Ok((pods, jobs))
    }

    /// Count resources that currently match a sweep without deleting them.
    pub async fn sanitize_preview(
        &self,
        namespace: Option<String>,
        options: SweepOptions,
    ) -> Result<SweepCounts, K8sError> {
        let (pods, jobs) = self.sweep_candidates(namespace.as_deref(), options).await?;
        Ok(SweepCounts {
            pods: pods.len(),
            jobs: jobs.len(),
        })
    }

    /// Delete the selected categories of pods and finished Jobs.
    pub async fn sanitize(
        &self,
        namespace: Option<String>,
        options: SweepOptions,
    ) -> Result<CleanupSummary, K8sError> {
        let (pods, jobs) = self.sweep_candidates(namespace.as_deref(), options).await?;
        let mut summary = CleanupSummary::default();
        for pod in &pods {
            let name = pod.metadata.name.as_deref().unwrap_or_default();
            let ns = pod.metadata.namespace.as_deref().unwrap_or_default();
            match Api::<Pod>::namespaced(self.client(), ns)
                .delete(name, &DeleteParams::default())
                .await
            {
                Ok(_) => summary.pods_deleted += 1,
                Err(error) => record_delete_error(&mut summary, "Pod", ns, name, error),
            }
        }

        for job in &jobs {
            let name = job.metadata.name.as_deref().unwrap_or_default();
            let ns = job.metadata.namespace.as_deref().unwrap_or_default();
            match Api::<Job>::namespaced(self.client(), ns)
                .delete(name, &DeleteParams::default())
                .await
            {
                Ok(_) => summary.jobs_deleted += 1,
                Err(error) => record_delete_error(&mut summary, "Job", ns, name, error),
            }
        }

        Ok(summary)
    }
}

fn record_delete_error(
    summary: &mut CleanupSummary,
    kind: &str,
    namespace: &str,
    name: &str,
    error: kube::Error,
) {
    let message = format!("{kind} {namespace}/{name}: {error}");
    if matches!(&error, kube::Error::Api(response) if response.code == 403) {
        summary.forbidden.push(message);
    } else {
        summary.failed.push(message);
    }
}

/// Pods that are dead or permanently stuck, and so safe to sweep.
/// Skips pods that already have a deletion timestamp (already being removed).
fn should_sweep_pod(pod: &Pod, options: SweepOptions) -> bool {
    if pod.metadata.deletion_timestamp.is_some() {
        return false;
    }
    let status = pod.status.as_ref();
    let terminal = status.and_then(|s| s.reason.as_deref()) == Some("Evicted")
        || matches!(
            status.and_then(|s| s.phase.as_deref()),
            Some("Succeeded" | "Failed")
        );
    if options.terminal_pods && terminal {
        return true;
    }
    let cs = status
        .and_then(|s| s.container_statuses.as_deref())
        .unwrap_or(&[]);
    let ics = status
        .and_then(|s| s.init_container_statuses.as_deref())
        .unwrap_or(&[]);
    let containers = || cs.iter().chain(ics);
    if options.restarted_pods && containers().any(|container| container.restart_count > 0) {
        return true;
    }
    options.stuck_pods
        && containers().any(|c| {
            if let Some(w) = c.state.as_ref().and_then(|s| s.waiting.as_ref()) {
                matches!(
                    w.reason.as_deref(),
                    Some(
                        "CrashLoopBackOff"
                            | "Error"
                            | "ImagePullBackOff"
                            | "ErrImagePull"
                            | "ContainerStatusUnknown"
                    )
                )
            } else if let Some(t) = c.state.as_ref().and_then(|s| s.terminated.as_ref()) {
                t.reason.as_deref() == Some("OOMKilled")
            } else {
                false
            }
        })
}

fn should_sweep_job(job: &Job, options: SweepOptions) -> bool {
    let Ok(data) = serde_json::to_value(job) else {
        return false;
    };
    match crate::project::job_lifecycle(&data) {
        crate::project::JobLifecycle::Complete => options.completed_jobs,
        crate::project::JobLifecycle::Failed => options.failed_jobs,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::PodStatus;
    use serde_json::json;

    fn pod_in_phase(phase: &str) -> Pod {
        Pod {
            status: Some(PodStatus {
                phase: Some(phase.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn sweeps_terminally_failed_pods() {
        assert!(should_sweep_pod(
            &pod_in_phase("Failed"),
            SweepOptions::default()
        ));
    }

    #[test]
    fn does_not_sweep_running_or_pending_pods_by_phase() {
        assert!(!should_sweep_pod(
            &pod_in_phase("Running"),
            SweepOptions::default()
        ));
        assert!(!should_sweep_pod(
            &pod_in_phase("Pending"),
            SweepOptions::default()
        ));
    }

    #[test]
    fn terminal_pods_can_be_excluded() {
        let options = SweepOptions {
            terminal_pods: false,
            ..Default::default()
        };
        assert!(!should_sweep_pod(&pod_in_phase("Failed"), options));
    }

    #[test]
    fn restarted_pods_are_opt_in() {
        let pod: Pod = serde_json::from_value(json!({
            "status": {"containerStatuses": [{
                "name": "app",
                "image": "app",
                "imageID": "",
                "ready": true,
                "restartCount": 2,
                "started": true,
                "state": {"running": {}}
            }]}
        }))
        .unwrap();
        assert!(!should_sweep_pod(&pod, SweepOptions::default()));
        assert!(should_sweep_pod(
            &pod,
            SweepOptions {
                terminal_pods: false,
                stuck_pods: false,
                restarted_pods: true,
                completed_jobs: false,
                failed_jobs: false,
            }
        ));
    }

    #[test]
    fn stuck_pods_can_be_selected_independently() {
        let pod: Pod = serde_json::from_value(json!({
            "status": {"containerStatuses": [{
                "name": "app",
                "image": "app",
                "imageID": "",
                "ready": false,
                "restartCount": 3,
                "started": false,
                "state": {"waiting": {"reason": "CrashLoopBackOff"}}
            }]}
        }))
        .unwrap();
        assert!(should_sweep_pod(
            &pod,
            SweepOptions {
                terminal_pods: false,
                ..Default::default()
            }
        ));
        assert!(!should_sweep_pod(
            &pod,
            SweepOptions {
                terminal_pods: false,
                stuck_pods: false,
                ..Default::default()
            }
        ));
    }

    #[test]
    fn completed_and_failed_jobs_are_independent() {
        let job = |condition: &str| -> Job {
            serde_json::from_value(json!({
                "spec": {"template": {"spec": {"containers": [], "restartPolicy": "Never"}}},
                "status": {"conditions": [{
                    "type": condition,
                    "status": "True",
                    "lastProbeTime": null,
                    "lastTransitionTime": null
                }]}
            }))
            .unwrap()
        };
        let completed_only = SweepOptions {
            failed_jobs: false,
            ..Default::default()
        };
        assert!(should_sweep_job(&job("Complete"), completed_only));
        assert!(!should_sweep_job(&job("Failed"), completed_only));
    }
}
