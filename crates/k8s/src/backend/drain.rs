//! Node drain: cordon + evict every evictable pod on the node, mirroring
//! `kubectl drain`'s default behaviour (skip DaemonSet-owned and mirror pods,
//! respect PodDisruptionBudgets with a short retry).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use k8s_openapi::api::core::v1::{Node, Pod};
use kube::api::{Api, ListParams};
use roder_core::{DrainEventKind, DrainOptions, DrainSummary};

use super::{api_err, Backend};
use crate::client::K8sError;

const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Channel a caller streams progress on; send failures are ignored (a
/// disconnected/dropped receiver shouldn't abort the drain itself).
pub type DrainEvents = tokio::sync::mpsc::UnboundedSender<DrainEventKind>;

impl Backend {
    /// Cordon `name`, then evict every pod scheduled on it except
    /// DaemonSet-owned and mirror (static) pods, which the API can't evict.
    ///
    /// Refuses to touch a node with unmanaged pods, pods using `emptyDir`
    /// volumes, or (unless `ignore_daemonsets`) DaemonSet pods, unless the
    /// matching `options` flag opts in — mirroring `kubectl drain`'s default
    /// safety refusal. Progress is streamed on `events` (best-effort); `cancel`
    /// is polled between pods and in the termination-wait loop, and drain
    /// stops early (returning the partial summary) once it's set.
    pub async fn drain(
        &self,
        key: &str,
        name: &str,
        options: &DrainOptions,
        events: &DrainEvents,
        cancel: &AtomicBool,
    ) -> Result<DrainSummary, K8sError> {
        self.cordon(key, name, true).await?;
        let _ = events.send(DrainEventKind::Cordoned);

        let pod_api: Api<Pod> = Api::all(self.client());
        let lp = ListParams::default().fields(&format!("spec.nodeName={name}"));
        let pods = pod_api.list(&lp).await.map_err(api_err)?;

        let mut summary = DrainSummary::default();
        let deadline = Instant::now() + Duration::from_secs(options.timeout_secs);

        let blockers: Vec<_> = pods
            .items
            .iter()
            .filter_map(|p| drain_blocker(p, options))
            .collect();
        if !blockers.is_empty() {
            summary.failed = blockers
                .iter()
                .map(|b| format!("{}: {}", b.pod, b.reason))
                .collect();
            summary.skipped = pods.items.len();
            let _ = events.send(DrainEventKind::Blocked { blockers });
            return Ok(summary);
        }

        let evictable: Vec<_> = pods.items.iter().filter(|p| is_evictable(p)).collect();
        let total = evictable.len();
        let _ = events.send(DrainEventKind::Started { total });

        for pod in evictable {
            if cancel.load(Ordering::Relaxed) {
                summary.skipped = pods.items.len() - summary.evicted;
                return Ok(summary);
            }
            let pod_name = pod.metadata.name.clone().unwrap_or_default();
            let ns = pod.metadata.namespace.clone().unwrap_or_default();

            let mut last_err = String::new();
            let mut ok = false;
            while Instant::now() < deadline {
                match self.remove_pod(&ns, &pod_name, options).await {
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
                let _ = events.send(DrainEventKind::Evicted {
                    pod: pod_name,
                    done: summary.evicted,
                    total,
                });
            } else {
                summary.failed.push(format!("{pod_name}: {last_err}"));
                let _ = events.send(DrainEventKind::EvictFailed {
                    pod: pod_name,
                    reason: last_err,
                });
            }
        }
        summary.skipped = pods.items.len() - summary.evicted;

        // Eviction acceptance is not completion: wait for evictable pods to
        // disappear so preStop hooks and volume detach can finish before power-off.
        while summary.failed.is_empty() && Instant::now() < deadline {
            if cancel.load(Ordering::Relaxed) {
                return Ok(summary);
            }
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
            let _ = events.send(DrainEventKind::WaitingTermination {
                remaining: names.len(),
            });
            tokio::time::sleep(RETRY_DELAY).await;
        }
        if summary.failed.is_empty() {
            summary
                .failed
                .push("timed out waiting for evicted pods to terminate".into());
        }

        Ok(summary)
    }

    /// Evict `pod`, or DELETE it when eviction is disabled; either way honoring
    /// the grace-period override.
    async fn remove_pod(
        &self,
        ns: &str,
        name: &str,
        options: &DrainOptions,
    ) -> Result<(), K8sError> {
        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        let dp = kube::api::DeleteParams {
            grace_period_seconds: options.grace_period,
            ..Default::default()
        };
        if options.disable_eviction {
            api.delete(name, &dp).await.map_err(api_err)?;
            return Ok(());
        }
        let ep = kube::api::EvictParams {
            delete_options: Some(dp),
            ..Default::default()
        };
        api.evict(name, &ep).await.map_err(api_err)?;
        Ok(())
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

/// Why `pod` blocks the drain under `options`, or None if it doesn't.
fn drain_blocker(pod: &Pod, options: &DrainOptions) -> Option<roder_core::DrainBlocker> {
    let name = pod
        .metadata
        .name
        .clone()
        .unwrap_or_else(|| "unknown pod".into());
    let terminal = matches!(
        pod.status.as_ref().and_then(|s| s.phase.as_deref()),
        Some("Succeeded" | "Failed")
    );
    let mirror = pod
        .metadata
        .annotations
        .as_ref()
        .is_some_and(|a| a.contains_key("kubernetes.io/config.mirror"));
    if terminal || mirror || pod.metadata.deletion_timestamp.is_some() {
        return None;
    }
    let daemonset = pod
        .metadata
        .owner_references
        .as_ref()
        .is_some_and(|refs| refs.iter().any(|o| o.kind == "DaemonSet"));
    if daemonset {
        return (!options.ignore_daemonsets).then(|| roder_core::DrainBlocker {
            pod: name,
            reason: "DaemonSet-managed pod".into(),
            clearable_by: "ignore_daemonsets".into(),
        });
    }
    if pod
        .metadata
        .owner_references
        .as_ref()
        .is_none_or(Vec::is_empty)
        && !options.force
    {
        return Some(roder_core::DrainBlocker {
            pod: name,
            reason: "unmanaged pod".into(),
            clearable_by: "force".into(),
        });
    }
    if !options.delete_emptydir_data
        && pod
            .spec
            .as_ref()
            .and_then(|s| s.volumes.as_ref())
            .is_some_and(|vs| vs.iter().any(|v| v.empty_dir.is_some()))
    {
        return Some(roder_core::DrainBlocker {
            pod: name,
            reason: "uses emptyDir storage".into(),
            clearable_by: "delete_emptydir_data".into(),
        });
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
        let blocker = drain_blocker(&pod, &DrainOptions::default()).unwrap();
        assert_eq!(blocker.clearable_by, "force");

        let forced = DrainOptions {
            force: true,
            ..Default::default()
        };
        assert!(drain_blocker(&pod, &forced).is_none());
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
        let blocker = drain_blocker(&pod, &DrainOptions::default()).unwrap();
        assert_eq!(blocker.clearable_by, "delete_emptydir_data");

        let allowed = DrainOptions {
            delete_emptydir_data: true,
            ..Default::default()
        };
        assert!(drain_blocker(&pod, &allowed).is_none());
    }

    #[test]
    fn blocks_daemonset_pods_when_not_ignored() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": {
                "name": "ds-pod",
                "ownerReferences": [{
                    "apiVersion": "apps/v1", "kind": "DaemonSet",
                    "name": "ds", "uid": "1"
                }]
            },
            "spec": { "containers": [{ "name": "app", "image": "app" }] }
        }))
        .unwrap();
        let strict = DrainOptions {
            ignore_daemonsets: false,
            ..Default::default()
        };
        let blocker = drain_blocker(&pod, &strict).unwrap();
        assert_eq!(blocker.clearable_by, "ignore_daemonsets");

        // Default options ignore DaemonSets.
        assert!(drain_blocker(&pod, &DrainOptions::default()).is_none());
    }

    #[test]
    fn mirror_and_terminal_pods_never_block() {
        let mirror: Pod = serde_json::from_value(json!({
            "metadata": {
                "name": "static-pod",
                "annotations": { "kubernetes.io/config.mirror": "abc" }
            },
            "spec": { "containers": [{ "name": "app", "image": "app" }] }
        }))
        .unwrap();
        assert!(drain_blocker(&mirror, &DrainOptions::default()).is_none());

        let terminal: Pod = serde_json::from_value(json!({
            "metadata": { "name": "done-pod" },
            "spec": { "containers": [{ "name": "app", "image": "app" }] },
            "status": { "phase": "Succeeded" }
        }))
        .unwrap();
        assert!(drain_blocker(&terminal, &DrainOptions::default()).is_none());
    }
}
