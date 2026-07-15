//! Node drain: cordon + evict every evictable pod on the node, mirroring
//! `kubectl drain`'s default behaviour (skip DaemonSet-owned and mirror pods,
//! respect PodDisruptionBudgets with a short retry).

use k8s_openapi::api::core::v1::{Node, Pod};
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
    ///
    /// Unless `force` is set, refuses to touch a node running unmanaged pods
    /// or pods with `emptyDir` volumes (mirroring `kubectl drain`'s default
    /// safety refusal, which normally requires `--force`/`--delete-emptydir-data`
    /// to override).
    pub async fn drain(&self, key: &str, name: &str, force: bool) -> Result<DrainSummary, K8sError> {
        self.cordon(key, name, true).await?;

        let pod_api: Api<Pod> = Api::all(self.client());
        let lp = ListParams::default().fields(&format!("spec.nodeName={name}"));
        let pods = pod_api.list(&lp).await.map_err(api_err)?;

        let mut summary = DrainSummary::default();
        let deadline = std::time::Instant::now() + DRAIN_BUDGET;

        if !force {
            for pod in &pods.items {
                if let Some(reason) = unsafe_drain_blocker(pod) {
                    summary.failed.push(format!(
                        "{}: {reason}",
                        pod.metadata.name.as_deref().unwrap_or("unknown pod")
                    ));
                }
            }
            if !summary.failed.is_empty() {
                summary.skipped = pods.items.len();
                return Ok(summary);
            }
        }

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
        summary.skipped = pods.items.len() - summary.evicted;

        // Eviction acceptance is not completion: wait for evictable pods to
        // disappear so preStop hooks and volume detach can finish before power-off.
        while summary.failed.is_empty() && std::time::Instant::now() < deadline {
            let remaining = pod_api.list(&lp).await.map_err(api_err)?;
            let names: Vec<_> = remaining
                .items
                .iter()
                .filter(|pod| is_evictable(pod))
                .filter_map(|pod| pod.metadata.name.clone())
                .collect();
            if names.is_empty() {
                return Ok(summary);
            }
            tokio::time::sleep(RETRY_DELAY).await;
        }
        if summary.failed.is_empty() {
            summary
                .failed
                .push("timed out waiting for evicted pods to terminate".into());
        }

        Ok(summary)
    }

    /// Wait until a rebooting node has first become NotReady and then Ready again.
    pub async fn wait_for_node_reboot(
        &self,
        name: &str,
        previous_boot_id: Option<&str>,
        timeout: std::time::Duration,
    ) -> Result<(), K8sError> {
        let nodes: Api<Node> = Api::all(self.client());
        let deadline = std::time::Instant::now() + timeout;
        let mut saw_not_ready = false;
        while std::time::Instant::now() < deadline {
            let node = match nodes.get(name).await {
                Ok(node) => node,
                Err(_) => {
                    saw_not_ready = true;
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
            };
            let ready = node
                .status
                .as_ref()
                .and_then(|status| status.conditions.as_ref())
                .and_then(|conditions| {
                    conditions
                        .iter()
                        .find(|condition| condition.type_ == "Ready")
                })
                .is_some_and(|condition| condition.status == "True");
            let boot_changed = previous_boot_id.is_some_and(|before| {
                node.status
                    .as_ref()
                    .and_then(|status| status.node_info.as_ref())
                    .is_some_and(|info| info.boot_id != before)
            });
            if !ready {
                saw_not_ready = true;
            } else if saw_not_ready || boot_changed {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        Err(K8sError::Api(format!(
            "node {name} did not complete a NotReady-to-Ready reboot cycle before timeout"
        )))
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

fn unsafe_drain_blocker(pod: &Pod) -> Option<&'static str> {
    let terminal = matches!(
        pod.status
            .as_ref()
            .and_then(|status| status.phase.as_deref()),
        Some("Succeeded" | "Failed")
    );
    let mirror = pod
        .metadata
        .annotations
        .as_ref()
        .is_some_and(|annotations| annotations.contains_key("kubernetes.io/config.mirror"));
    let daemonset = pod
        .metadata
        .owner_references
        .as_ref()
        .is_some_and(|refs| refs.iter().any(|owner| owner.kind == "DaemonSet"));
    if terminal || mirror || daemonset || pod.metadata.deletion_timestamp.is_some() {
        return None;
    }
    if pod
        .metadata
        .owner_references
        .as_ref()
        .is_none_or(Vec::is_empty)
    {
        return Some("unmanaged pod requires an explicit force drain");
    }
    if pod
        .spec
        .as_ref()
        .and_then(|spec| spec.volumes.as_ref())
        .is_some_and(|volumes| volumes.iter().any(|volume| volume.empty_dir.is_some()))
    {
        return Some("pod uses emptyDir storage and requires explicit data-loss approval");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blocks_unmanaged_pods() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": { "name": "standalone" },
            "spec": { "containers": [{ "name": "app", "image": "app" }] }
        }))
        .unwrap();
        assert_eq!(
            unsafe_drain_blocker(&pod),
            Some("unmanaged pod requires an explicit force drain")
        );
    }

    #[test]
    fn blocks_empty_dir_data_loss() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": {
                "name": "local-data",
                "ownerReferences": [{
                    "apiVersion": "apps/v1", "kind": "ReplicaSet",
                    "name": "app", "uid": "1"
                }]
            },
            "spec": {
                "containers": [{ "name": "app", "image": "app" }],
                "volumes": [{ "name": "cache", "emptyDir": {} }]
            }
        }))
        .unwrap();
        assert_eq!(
            unsafe_drain_blocker(&pod),
            Some("pod uses emptyDir storage and requires explicit data-loss approval")
        );
    }
}
