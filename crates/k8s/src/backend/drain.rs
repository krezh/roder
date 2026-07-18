//! Node drain: cordon + evict every evictable pod on the node, mirroring
//! `kubectl drain`'s default behaviour (skip DaemonSet-owned and mirror pods,
//! respect PodDisruptionBudgets with a short retry).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use k8s_openapi::api::core::v1::{Node, Pod};
use kube::api::{Api, ListParams, Patch, PatchParams};
use roder_core::{DrainBlocker, DrainEventKind, DrainOptions, DrainSummary};

use super::{api_err, Backend};
use crate::client::K8sError;

const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Channel a caller streams progress on; send failures are ignored (a
/// disconnected/dropped receiver shouldn't abort the drain itself).
pub type DrainEvents = tokio::sync::mpsc::UnboundedSender<DrainEventKind>;

/// Kubernetes operations that must keep using the identity captured when a
/// drain or Talos request started, even if [`Backend::set_token`] later swaps
/// the backend's current client.
#[derive(Clone)]
pub struct DrainSession {
    client: kube::Client,
}

impl Backend {
    /// Snapshot the current Kubernetes client for a multi-step operation.
    pub fn drain_session(&self) -> DrainSession {
        DrainSession {
            client: self.client(),
        }
    }

    /// Drain using a snapshot of the client current at method entry.
    pub async fn drain(
        &self,
        key: &str,
        name: &str,
        options: &DrainOptions,
        events: &DrainEvents,
        cancel: &AtomicBool,
    ) -> Result<DrainSummary, K8sError> {
        self.drain_session()
            .drain(key, name, options, events, cancel)
            .await
    }

    /// Wait using a snapshot of the client current at method entry.
    pub async fn wait_for_node_reboot(
        &self,
        name: &str,
        previous_boot_id: Option<&str>,
        timeout: Duration,
    ) -> Result<(), K8sError> {
        self.drain_session()
            .wait_for_node_reboot(name, previous_boot_id, timeout)
            .await
    }
}

impl DrainSession {
    /// Cordon `name`, then evict every pod scheduled on it except
    /// DaemonSet-owned and mirror (static) pods, which the API can't evict.
    ///
    /// Refuses to touch a node with unmanaged pods, pods using `emptyDir`
    /// volumes, or (unless `ignore_daemonsets`) DaemonSet pods, unless the
    /// matching `options` flag opts in — mirroring `kubectl drain`'s default
    /// safety refusal. Progress is streamed on `events` (best-effort); `cancel`
    /// is polled between and within pod retries and in the termination-wait
    /// loop, and drain stops early (returning the partial summary) once it's set.
    pub async fn drain(
        &self,
        _key: &str,
        name: &str,
        options: &DrainOptions,
        events: &DrainEvents,
        cancel: &AtomicBool,
    ) -> Result<DrainSummary, K8sError> {
        options
            .validate()
            .map_err(|message| K8sError::Api(message.into()))?;

        self.cordon(name, true).await?;
        let _ = events.send(DrainEventKind::Cordoned);

        let pod_api: Api<Pod> = Api::all(self.client.clone());
        let lp = ListParams::default().fields(&format!("spec.nodeName={name}"));
        let pods = pod_api.list(&lp).await.map_err(api_err)?;

        let mut summary = DrainSummary::default();
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(options.timeout_secs))
            .ok_or_else(|| K8sError::Api("drain timeout exceeds the supported range".into()))?;
        summary.skipped = skipped_pod_count(&pods.items);

        let blockers: Vec<_> = pods
            .items
            .iter()
            .flat_map(|pod| drain_blockers(pod, options))
            .collect();
        if !blockers.is_empty() {
            summary.failed = blockers
                .iter()
                .map(|b| format!("{}: {}", b.pod, b.reason))
                .collect();
            let _ = events.send(DrainEventKind::Blocked { blockers });
            return Ok(summary);
        }

        let evictable: Vec<_> = pods.items.iter().filter(|p| is_evictable(p)).collect();
        let total = evictable.len();
        let _ = events.send(DrainEventKind::Started { total });
        let mut terminating: Vec<_> = pods
            .items
            .iter()
            .filter(|pod| is_waitable_terminating(pod))
            .map(PodIdentity::from)
            .collect();
        terminating.reserve(total);

        for pod in evictable {
            if cancel.load(Ordering::Relaxed) {
                return Ok(summary);
            }
            let pod_name = pod.metadata.name.clone().unwrap_or_default();
            let ns = pod.metadata.namespace.clone().unwrap_or_default();

            let mut last_err = String::new();
            let mut ok = false;
            while Instant::now() < deadline {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(summary);
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                match tokio::time::timeout(remaining, self.remove_pod(&ns, &pod_name, options))
                    .await
                {
                    Ok(Ok(())) => {
                        ok = true;
                        break;
                    }
                    Ok(Err(e)) => {
                        last_err = e.to_string();
                        if cancel.load(Ordering::Relaxed) {
                            return Ok(summary);
                        }
                        let sleep_for =
                            RETRY_DELAY.min(deadline.saturating_duration_since(Instant::now()));
                        tokio::time::sleep(sleep_for).await;
                        if cancel.load(Ordering::Relaxed) {
                            return Ok(summary);
                        }
                    }
                    Err(_) => {
                        last_err = "drain deadline elapsed while removing pod".into();
                        break;
                    }
                }
            }
            if ok {
                terminating.push(PodIdentity::from(pod));
                summary.evicted += 1;
                let _ = events.send(DrainEventKind::Evicted {
                    pod: pod_name,
                    done: summary.evicted,
                    total,
                });
            } else {
                let reason = if last_err.is_empty() {
                    "drain deadline elapsed before pod removal was accepted".into()
                } else {
                    last_err
                };
                summary.failed.push(format!("{pod_name}: {reason}"));
                let _ = events.send(DrainEventKind::EvictFailed {
                    pod: pod_name,
                    reason,
                });
            }
        }

        // Existing workload pods already terminating are skipped for eviction
        // (and counted as skipped) but still must disappear before power-off.
        while !terminating.is_empty() && Instant::now() < deadline {
            if cancel.load(Ordering::Relaxed) {
                return Ok(summary);
            }
            let budget = deadline.saturating_duration_since(Instant::now());
            let remaining = match tokio::time::timeout(budget, pod_api.list(&lp)).await {
                Ok(result) => result.map_err(api_err)?,
                Err(_) => break,
            };
            let remaining = remaining_terminating_pods(&remaining.items, &terminating);
            if remaining.is_empty() {
                return Ok(summary);
            }
            let _ = events.send(DrainEventKind::WaitingTermination { pods: remaining });
            let sleep_for = RETRY_DELAY.min(deadline.saturating_duration_since(Instant::now()));
            tokio::time::sleep(sleep_for).await;
            if cancel.load(Ordering::Relaxed) {
                return Ok(summary);
            }
        }
        if !terminating.is_empty() {
            summary
                .failed
                .push("timed out waiting for workload pods to terminate".into());
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
        let api: Api<Pod> = Api::namespaced(self.client.clone(), ns);
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

    /// Cordon or uncordon a node with the client captured by this session.
    pub async fn cordon(&self, name: &str, unschedulable: bool) -> Result<(), K8sError> {
        let nodes: Api<Node> = Api::all(self.client.clone());
        nodes
            .patch(
                name,
                &PatchParams::default(),
                &Patch::Merge(serde_json::json!({
                    "spec": { "unschedulable": unschedulable }
                })),
            )
            .await
            .map_err(api_err)?;
        Ok(())
    }

    /// Wait until a rebooting node has first become NotReady and then Ready again.
    pub async fn wait_for_node_reboot(
        &self,
        name: &str,
        previous_boot_id: Option<&str>,
        timeout: std::time::Duration,
    ) -> Result<(), K8sError> {
        let nodes: Api<Node> = Api::all(self.client.clone());
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct PodIdentity {
    namespace: String,
    name: String,
    uid: Option<String>,
}

impl From<&Pod> for PodIdentity {
    fn from(pod: &Pod) -> Self {
        Self {
            namespace: pod.metadata.namespace.clone().unwrap_or_default(),
            name: pod.metadata.name.clone().unwrap_or_default(),
            uid: pod.metadata.uid.clone(),
        }
    }
}

impl PodIdentity {
    fn matches(&self, pod: &Pod) -> bool {
        if let Some(uid) = &self.uid {
            return pod.metadata.uid.as_ref() == Some(uid);
        }
        pod.metadata.namespace.as_deref().unwrap_or_default() == self.namespace
            && pod.metadata.name.as_deref().unwrap_or_default() == self.name
    }
}

fn remaining_terminating_pods(pods: &[Pod], terminating: &[PodIdentity]) -> Vec<String> {
    let mut names: Vec<_> = pods
        .iter()
        .filter(|pod| terminating.iter().any(|identity| identity.matches(pod)))
        .filter_map(|pod| pod.metadata.name.clone())
        .collect();
    names.sort();
    names
}

fn skipped_pod_count(pods: &[Pod]) -> usize {
    pods.iter().filter(|pod| !is_evictable(pod)).count()
}

/// A pre-existing deletion is waited on only when it belongs to a workload
/// that drain would otherwise evict. DaemonSet, mirror, and terminal pods are
/// never part of the power-action termination barrier.
fn is_waitable_terminating(pod: &Pod) -> bool {
    pod.metadata.deletion_timestamp.is_some()
        && !is_daemonset(pod)
        && !is_mirror(pod)
        && !is_terminal(pod)
}

fn is_daemonset(pod: &Pod) -> bool {
    pod.metadata
        .owner_references
        .as_ref()
        .is_some_and(|refs| refs.iter().any(|owner| owner.kind == "DaemonSet"))
}

fn is_mirror(pod: &Pod) -> bool {
    pod.metadata
        .annotations
        .as_ref()
        .is_some_and(|annotations| annotations.contains_key("kubernetes.io/config.mirror"))
}

fn is_terminal(pod: &Pod) -> bool {
    matches!(
        pod.status
            .as_ref()
            .and_then(|status| status.phase.as_deref()),
        Some("Succeeded" | "Failed")
    )
}

/// DaemonSet-owned and mirror (static) pods aren't evictable through the API
/// — deleting them just has the DaemonSet/kubelet recreate them in place.
/// Already-terminal pods need no eviction at all.
fn is_evictable(pod: &Pod) -> bool {
    if pod.metadata.deletion_timestamp.is_some() {
        return false;
    }
    if is_mirror(pod) {
        return false;
    }
    if is_daemonset(pod) {
        return false;
    }
    !is_terminal(pod)
}

/// Every reason `pod` blocks the drain under `options`.
fn drain_blockers(pod: &Pod, options: &DrainOptions) -> Vec<DrainBlocker> {
    let name = pod
        .metadata
        .name
        .clone()
        .unwrap_or_else(|| "unknown pod".into());
    if is_terminal(pod) || is_mirror(pod) || pod.metadata.deletion_timestamp.is_some() {
        return Vec::new();
    }
    if is_daemonset(pod) {
        return if options.ignore_daemonsets {
            Vec::new()
        } else {
            vec![DrainBlocker {
                pod: name,
                reason: "DaemonSet-managed pod".into(),
                clearable_by: "ignore_daemonsets".into(),
            }]
        };
    }
    let mut blockers = Vec::new();
    if pod
        .metadata
        .owner_references
        .as_ref()
        .is_none_or(Vec::is_empty)
        && !options.force
    {
        blockers.push(DrainBlocker {
            pod: name.clone(),
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
        blockers.push(DrainBlocker {
            pod: name,
            reason: "uses emptyDir storage".into(),
            clearable_by: "delete_emptydir_data".into(),
        });
    }
    blockers
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
        let blockers = drain_blockers(&pod, &DrainOptions::default());
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].clearable_by, "force");

        let forced = DrainOptions {
            force: true,
            ..Default::default()
        };
        assert!(drain_blockers(&pod, &forced).is_empty());
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
        let blockers = drain_blockers(&pod, &DrainOptions::default());
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].clearable_by, "delete_emptydir_data");

        let allowed = DrainOptions {
            delete_emptydir_data: true,
            ..Default::default()
        };
        assert!(drain_blockers(&pod, &allowed).is_empty());
    }

    #[test]
    fn reports_all_blockers_for_unmanaged_empty_dir_pod() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": { "name": "standalone-data" },
            "spec": {
                "containers": [{ "name": "app", "image": "app" }],
                "volumes": [{ "name": "cache", "emptyDir": {} }]
            }
        }))
        .unwrap();

        let blockers = drain_blockers(&pod, &DrainOptions::default());
        let clearable_by: Vec<_> = blockers
            .iter()
            .map(|blocker| blocker.clearable_by.as_str())
            .collect();
        assert_eq!(clearable_by, ["force", "delete_emptydir_data"]);
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
        let blockers = drain_blockers(&pod, &strict);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].clearable_by, "ignore_daemonsets");

        // Default options ignore DaemonSets.
        assert!(drain_blockers(&pod, &DrainOptions::default()).is_empty());
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
        assert!(drain_blockers(&mirror, &DrainOptions::default()).is_empty());

        let terminal: Pod = serde_json::from_value(json!({
            "metadata": { "name": "done-pod" },
            "spec": { "containers": [{ "name": "app", "image": "app" }] },
            "status": { "phase": "Succeeded" }
        }))
        .unwrap();
        assert!(drain_blockers(&terminal, &DrainOptions::default()).is_empty());
    }

    #[test]
    fn accepted_deleting_pod_is_still_waiting_for_termination() {
        let accepted_pod: Pod = serde_json::from_value(json!({
            "metadata": {
                "name": "app-pod",
                "namespace": "default",
                "uid": "pod-uid"
            },
            "spec": { "containers": [{ "name": "app", "image": "app" }] }
        }))
        .unwrap();
        let deleting_pod: Pod = serde_json::from_value(json!({
            "metadata": {
                "name": "app-pod",
                "namespace": "default",
                "uid": "pod-uid",
                "deletionTimestamp": "2026-07-18T00:00:00Z"
            },
            "spec": { "containers": [{ "name": "app", "image": "app" }] }
        }))
        .unwrap();

        let accepted = vec![PodIdentity::from(&accepted_pod)];
        assert_eq!(
            remaining_terminating_pods(&[deleting_pod], &accepted),
            ["app-pod"]
        );
        assert!(remaining_terminating_pods(&[], &accepted).is_empty());
    }

    #[test]
    fn replacement_pod_with_same_name_is_not_an_accepted_identity() {
        let original: Pod = serde_json::from_value(json!({
            "metadata": {
                "name": "app-pod",
                "namespace": "default",
                "uid": "original-uid"
            },
            "spec": { "containers": [{ "name": "app", "image": "app" }] }
        }))
        .unwrap();
        let replacement: Pod = serde_json::from_value(json!({
            "metadata": {
                "name": "app-pod",
                "namespace": "default",
                "uid": "replacement-uid"
            },
            "spec": { "containers": [{ "name": "app", "image": "app" }] }
        }))
        .unwrap();

        let accepted = vec![PodIdentity::from(&original)];
        assert!(remaining_terminating_pods(&[replacement], &accepted).is_empty());
    }

    #[test]
    fn only_deleting_workload_pods_join_the_termination_wait() {
        let pods: Vec<Pod> = serde_json::from_value(json!([
            {
                "metadata": {
                    "name": "workload",
                    "deletionTimestamp": "2026-07-18T00:00:00Z",
                    "ownerReferences": [{
                        "apiVersion": "apps/v1", "kind": "ReplicaSet",
                        "name": "app", "uid": "app-owner"
                    }]
                },
                "spec": { "containers": [{ "name": "app", "image": "app" }] }
            },
            {
                "metadata": {
                    "name": "daemon",
                    "deletionTimestamp": "2026-07-18T00:00:00Z",
                    "ownerReferences": [{
                        "apiVersion": "apps/v1", "kind": "DaemonSet",
                        "name": "daemon", "uid": "daemon-owner"
                    }]
                },
                "spec": { "containers": [{ "name": "app", "image": "app" }] }
            },
            {
                "metadata": {
                    "name": "mirror",
                    "deletionTimestamp": "2026-07-18T00:00:00Z",
                    "annotations": { "kubernetes.io/config.mirror": "mirror" }
                },
                "spec": { "containers": [{ "name": "app", "image": "app" }] }
            },
            {
                "metadata": {
                    "name": "terminal",
                    "deletionTimestamp": "2026-07-18T00:00:00Z"
                },
                "spec": { "containers": [{ "name": "app", "image": "app" }] },
                "status": { "phase": "Failed" }
            }
        ]))
        .unwrap();

        assert_eq!(
            pods.iter()
                .filter(|pod| is_waitable_terminating(pod))
                .count(),
            1
        );
        assert!(is_waitable_terminating(&pods[0]));
    }

    #[test]
    fn skipped_count_only_includes_pods_not_requiring_eviction() {
        let pods: Vec<Pod> = serde_json::from_value(json!([
            {
                "metadata": {
                    "name": "daemon",
                    "ownerReferences": [{
                        "apiVersion": "apps/v1", "kind": "DaemonSet",
                        "name": "daemon", "uid": "daemon-owner"
                    }]
                },
                "spec": { "containers": [{ "name": "app", "image": "app" }] }
            },
            {
                "metadata": {
                    "name": "mirror",
                    "annotations": { "kubernetes.io/config.mirror": "mirror" }
                },
                "spec": { "containers": [{ "name": "app", "image": "app" }] }
            },
            {
                "metadata": { "name": "terminal" },
                "spec": { "containers": [{ "name": "app", "image": "app" }] },
                "status": { "phase": "Succeeded" }
            },
            {
                "metadata": {
                    "name": "deleting",
                    "deletionTimestamp": "2026-07-18T00:00:00Z"
                },
                "spec": { "containers": [{ "name": "app", "image": "app" }] }
            },
            {
                "metadata": { "name": "evictable" },
                "spec": { "containers": [{ "name": "app", "image": "app" }] }
            }
        ]))
        .unwrap();

        assert_eq!(skipped_pod_count(&pods), 4);
    }
}
