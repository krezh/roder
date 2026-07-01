//! One-click sweep of dead pods and finished jobs, mirroring k9s's `sanitize`.

use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, DeleteParams, ListParams};
use roder_core::CleanupSummary;

use super::{api_err, Backend};
use crate::client::K8sError;

impl Backend {
    /// Delete all "dead" pods (matching k9s's `toastPhases`) and finished Jobs.
    /// Best-effort: individual delete failures are silently skipped.
    pub async fn sanitize(&self, namespace: Option<String>) -> Result<CleanupSummary, K8sError> {
        let pod_api: Api<Pod> = match namespace.as_deref() {
            Some(ns) => Api::namespaced(self.client(), ns),
            None => Api::all(self.client()),
        };
        let pods = pod_api
            .list(&ListParams::default())
            .await
            .map_err(api_err)?;
        let mut pods_deleted = 0usize;
        for pod in pods.items.iter().filter(|p| is_toast_pod(p)) {
            let name = pod.metadata.name.as_deref().unwrap_or_default();
            let ns = pod.metadata.namespace.as_deref().unwrap_or_default();
            if Api::<Pod>::namespaced(self.client(), ns)
                .delete(name, &DeleteParams::default())
                .await
                .is_ok()
            {
                pods_deleted += 1;
            }
        }

        let job_api: Api<Job> = match namespace.as_deref() {
            Some(ns) => Api::namespaced(self.client(), ns),
            None => Api::all(self.client()),
        };
        let jobs = job_api
            .list(&ListParams::default())
            .await
            .map_err(api_err)?;
        let mut jobs_deleted = 0usize;
        for job in jobs.items.iter().filter(|j| is_finished_job(j)) {
            let name = job.metadata.name.as_deref().unwrap_or_default();
            let ns = job.metadata.namespace.as_deref().unwrap_or_default();
            if Api::<Job>::namespaced(self.client(), ns)
                .delete(name, &DeleteParams::default())
                .await
                .is_ok()
            {
                jobs_deleted += 1;
            }
        }

        Ok(CleanupSummary {
            pods_deleted,
            jobs_deleted,
        })
    }
}

/// Matches k9s's `toastPhases`: pods that are dead or permanently stuck.
/// Skips pods that already have a deletion timestamp (already being removed).
fn is_toast_pod(pod: &Pod) -> bool {
    if pod.metadata.deletion_timestamp.is_some() {
        return false;
    }
    let status = pod.status.as_ref();
    if status.and_then(|s| s.reason.as_deref()) == Some("Evicted") {
        return true;
    }
    if let Some("Succeeded") = status.and_then(|s| s.phase.as_deref()) {
        return true;
    }
    let cs = status
        .and_then(|s| s.container_statuses.as_deref())
        .unwrap_or(&[]);
    let ics = status
        .and_then(|s| s.init_container_statuses.as_deref())
        .unwrap_or(&[]);
    cs.iter().chain(ics).any(|c| {
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

fn is_finished_job(job: &Job) -> bool {
    job.status
        .as_ref()
        .and_then(|s| s.conditions.as_deref())
        .unwrap_or(&[])
        .iter()
        .any(|c| matches!(c.type_.as_str(), "Complete" | "Failed") && c.status == "True")
}
