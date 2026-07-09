//! Node drain: cordon + evict every evictable pod on the node, mirroring
//! `kubectl drain`'s default behaviour (skip DaemonSet-owned and mirror pods,
//! respect PodDisruptionBudgets with a short retry).

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};
use roder_core::DrainSummary;

use super::{api_err, Backend};
use crate::client::K8sError;

/// Overall wall-clock budget for the eviction retry loop, so a node stuck
/// behind an unsatisfiable PodDisruptionBudget can't hang the request forever.
const DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

impl Backend {
    /// Cordon `name`, then evict every pod scheduled on it except
    /// DaemonSet-owned and mirror (static) pods, which the API can't evict.
    pub async fn drain(&self, key: &str, name: &str) -> Result<DrainSummary, K8sError> {
        self.cordon(key, name, true).await?;

        let pod_api: Api<Pod> = Api::all(self.client());
        let lp = ListParams::default().fields(&format!("spec.nodeName={name}"));
        let pods = pod_api.list(&lp).await.map_err(api_err)?;

        let mut summary = DrainSummary::default();
        let deadline = std::time::Instant::now() + DRAIN_BUDGET;

        for pod in pods.items.iter().filter(|p| is_evictable(p)) {
            let pod_name = pod.metadata.name.clone().unwrap_or_default();
            let ns = pod.metadata.namespace.clone().unwrap_or_default();

            let mut last_err = String::new();
            let mut ok = false;
            while std::time::Instant::now() < deadline {
                match self.evict_pod(&ns, &pod_name).await {
                    Ok(()) => {
                        ok = true;
                        break;
                    }
                    Err(e) => {
                        last_err = e.to_string();
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                }
            }
            if ok {
                summary.evicted += 1;
            } else {
                summary.failed.push(format!("{pod_name}: {last_err}"));
            }
        }
        summary.skipped = pods.items.len() - summary.evicted - summary.failed.len();

        Ok(summary)
    }
}

/// DaemonSet-owned and mirror (static) pods aren't evictable through the API
/// — deleting them just has the DaemonSet/kubelet recreate them in place.
/// Already-terminal pods need no eviction at all.
fn is_evictable(pod: &Pod) -> bool {
    if pod.metadata.deletion_timestamp.is_some() {
        return false;
    }
    if pod
        .metadata
        .annotations
        .as_ref()
        .is_some_and(|a| a.contains_key("kubernetes.io/config.mirror"))
    {
        return false;
    }
    if pod
        .metadata
        .owner_references
        .as_ref()
        .is_some_and(|refs| refs.iter().any(|o| o.kind == "DaemonSet"))
    {
        return false;
    }
    !matches!(
        pod.status.as_ref().and_then(|s| s.phase.as_deref()),
        Some("Succeeded" | "Failed")
    )
}
