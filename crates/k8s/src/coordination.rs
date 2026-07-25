use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta};
use kube::api::{Api, ListParams, Patch, PatchParams, PostParams};
use kube::Client;
use tokio::sync::oneshot;

const LEASE_DURATION_SECS: i32 = 45;
const RENEW_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoderPod {
    pub name: String,
    pub uid: String,
    pub node: String,
    pub ip: String,
    pub ready: bool,
    /// SHA-256 fingerprint of this pod's self-signed mTLS cert (annotation
    /// `roder.io/tls-fingerprint`). `None` if the pod hasn't published it
    /// yet — typically only briefly at startup before the first patch
    /// completes. Peer connections to such a pod fail their TLS handshake
    /// until the watcher refreshes.
    pub tls_fingerprint: Option<String>,
}

/// Annotation key under which each roder pod publishes the SHA-256 fingerprint
/// of its self-signed mTLS certificate. Read by `NodeCoordinator::pods()` and
/// written once at pod startup by the roder binary itself.
pub const TLS_FINGERPRINT_ANNOTATION: &str = "roder.io/tls-fingerprint";

#[derive(Debug, thiserror::Error)]
pub enum AcquireError {
    #[error("another Roder operation is already active for this node")]
    Busy,
    #[error("Kubernetes coordination failed: {0}")]
    Kubernetes(#[from] kube::Error),
}

#[derive(Clone)]
pub struct NodeCoordinator {
    client: Client,
    selector: Arc<str>,
    holder: Arc<str>,
    scope: Arc<str>,
}

pub struct HeldNodeLease {
    coordinator: NodeCoordinator,
    name: String,
    stop: Option<oneshot::Sender<()>>,
    renew_task: Option<tokio::task::JoinHandle<()>>,
    cancel_on_loss: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

impl NodeCoordinator {
    pub fn new(
        client: Client,
        selector: impl Into<Arc<str>>,
        holder: impl Into<Arc<str>>,
        scope: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            client,
            selector: selector.into(),
            holder: holder.into(),
            scope: scope.into(),
        }
    }

    pub async fn pods(&self) -> Result<Vec<RoderPod>, kube::Error> {
        let pods: Api<Pod> = Api::default_namespaced(self.client.clone());
        let list = pods
            .list(&ListParams::default().labels(&self.selector))
            .await?;
        Ok(list
            .items
            .into_iter()
            .filter_map(|pod| {
                let status = pod.status?;
                let ready = pod.metadata.deletion_timestamp.is_none()
                    && status.phase.as_deref() == Some("Running")
                    && status.conditions.as_ref().is_some_and(|conditions| {
                        conditions.iter().any(|condition| {
                            condition.type_ == "Ready" && condition.status == "True"
                        })
                    });
                let tls_fingerprint =
                    pod.metadata.annotations.as_ref().and_then(|annotations| {
                        annotations.get(TLS_FINGERPRINT_ANNOTATION).cloned()
                    });
                Some(RoderPod {
                    name: pod.metadata.name?,
                    uid: pod.metadata.uid?,
                    node: pod.spec?.node_name?,
                    ip: status.pod_ip?,
                    ready,
                    tls_fingerprint,
                })
            })
            .collect())
    }

    pub async fn pod(&self, name: &str) -> Result<Option<RoderPod>, kube::Error> {
        Ok(self.pods().await?.into_iter().find(|pod| pod.name == name))
    }

    pub async fn acquire(&self, target: &str) -> Result<HeldNodeLease, AcquireError> {
        let name = lease_name(&self.scope, target);
        for _ in 0..4 {
            let leases: Api<Lease> = Api::default_namespaced(self.client.clone());
            let current = leases.get_opt(&name).await?;
            if let Some(lease) = current.as_ref() {
                if let Some(holder) = lease
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.holder_identity.as_deref())
                {
                    let pods = self.pods().await?;
                    if holder_pod_exists(holder, &pods) {
                        // Never steal from a live pod, even if clocks disagree or
                        // renewal temporarily cannot reach the API server.
                        return Err(AcquireError::Busy);
                    }
                }
            }

            let result = match current {
                None => {
                    leases
                        .create(
                            &PostParams::default(),
                            &new_lease(&name, target, &self.holder),
                        )
                        .await
                }
                Some(lease) => {
                    let transitions = lease
                        .spec
                        .as_ref()
                        .and_then(|spec| spec.lease_transitions)
                        .unwrap_or(0)
                        + i32::from(
                            lease
                                .spec
                                .as_ref()
                                .and_then(|s| s.holder_identity.as_deref())
                                != Some(&self.holder),
                        );
                    leases
                        .patch(
                            &name,
                            &PatchParams::default(),
                            &Patch::Merge(serde_json::json!({
                                "metadata": { "resourceVersion": lease.metadata.resource_version },
                                "spec": {
                                    "holderIdentity": self.holder.as_ref(),
                                    "leaseDurationSeconds": LEASE_DURATION_SECS,
                                    "acquireTime": now_microtime(),
                                    "renewTime": now_microtime(),
                                    "leaseTransitions": transitions
                                }
                            })),
                        )
                        .await
                }
            };
            match result {
                Ok(_) => return Ok(self.held(name)),
                Err(kube::Error::Api(error)) if error.code == 409 => continue,
                Err(error) => return Err(AcquireError::Kubernetes(error)),
            }
        }
        Err(AcquireError::Busy)
    }

    fn held(&self, name: String) -> HeldNodeLease {
        let (stop, mut stopped) = oneshot::channel();
        let coordinator = self.clone();
        let renew_name = name.clone();
        let cancel_on_loss = Arc::new(Mutex::new(None::<Arc<AtomicBool>>));
        let renewal_cancel = Arc::clone(&cancel_on_loss);
        let renew_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(RENEW_INTERVAL);
            let mut failures = 0;
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    _ = interval.tick() => {
                        match coordinator.renew(&renew_name).await {
                            Ok(()) => failures = 0,
                            Err(_) => {
                                failures += 1;
                                if failures >= 3 {
                                    if let Some(cancel) = renewal_cancel.lock().unwrap().as_ref() {
                                        cancel.store(true, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        HeldNodeLease {
            coordinator: self.clone(),
            name,
            stop: Some(stop),
            renew_task: Some(renew_task),
            cancel_on_loss,
        }
    }

    async fn renew(&self, name: &str) -> Result<(), AcquireError> {
        let leases: Api<Lease> = Api::default_namespaced(self.client.clone());
        let Some(lease) = leases.get_opt(name).await? else {
            return Err(AcquireError::Busy);
        };
        if lease
            .spec
            .as_ref()
            .and_then(|s| s.holder_identity.as_deref())
            != Some(&self.holder)
        {
            return Err(AcquireError::Busy);
        }
        leases
            .patch(
                name,
                &PatchParams::default(),
                &Patch::Merge(serde_json::json!({
                    "metadata": { "resourceVersion": lease.metadata.resource_version },
                    "spec": { "renewTime": now_microtime() }
                })),
            )
            .await?;
        Ok(())
    }
}

impl HeldNodeLease {
    pub fn cancel_on_loss(&self, cancel: Arc<AtomicBool>) {
        *self.cancel_on_loss.lock().unwrap() = Some(cancel);
    }

    pub async fn release(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.renew_task.take() {
            let _ = task.await;
        }
        let leases: Api<Lease> = Api::default_namespaced(self.coordinator.client.clone());
        if let Ok(Some(lease)) = leases.get_opt(&self.name).await {
            if lease
                .spec
                .as_ref()
                .and_then(|s| s.holder_identity.as_deref())
                == Some(&self.coordinator.holder)
            {
                let _ = leases
                    .patch(
                        &self.name,
                        &PatchParams::default(),
                        &Patch::Merge(serde_json::json!({
                            "metadata": { "resourceVersion": lease.metadata.resource_version },
                            "spec": { "holderIdentity": null }
                        })),
                    )
                    .await;
            }
        }
    }
}

impl Drop for HeldNodeLease {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn new_lease(name: &str, target: &str, holder: &str) -> Lease {
    Lease {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            annotations: Some(std::collections::BTreeMap::from([(
                "roder.io/target-node".to_string(),
                target.to_string(),
            )])),
            ..Default::default()
        },
        spec: Some(LeaseSpec {
            holder_identity: Some(holder.to_string()),
            lease_duration_seconds: Some(LEASE_DURATION_SECS),
            acquire_time: Some(now_microtime()),
            renew_time: Some(now_microtime()),
            lease_transitions: Some(0),
            ..Default::default()
        }),
    }
}

fn now_microtime() -> MicroTime {
    serde_json::from_value(serde_json::Value::String(
        time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .expect("RFC3339 formatting cannot fail"),
    ))
    .expect("RFC3339 timestamp is a Kubernetes MicroTime")
}

fn lease_name(scope: &str, target: &str) -> String {
    // Stable FNV-1a rather than DefaultHasher, whose algorithm may change
    // between Rust releases and split rolling deployments into different locks.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in scope.bytes().chain([0]).chain(target.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let suffix = format!("{hash:016x}");
    let target: String = target
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(220)
        .collect();
    format!("roder-node-{}-{suffix}", target.trim_matches('-'))
}

fn holder_pod_exists(holder: &str, pods: &[RoderPod]) -> bool {
    let Some((name, uid)) = holder.split_once('/') else {
        return true;
    };
    pods.iter().any(|pod| pod.name == name && pod.uid == uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_names_are_stable_and_bounded() {
        assert_eq!(
            lease_name("release", "node-a"),
            lease_name("release", "node-a")
        );
        assert_ne!(
            lease_name("release", "node-a"),
            lease_name("release", "node-b")
        );
        assert!(lease_name("release", &"n".repeat(300)).len() <= 253);
    }

    #[test]
    fn holder_identity_is_fenced_by_pod_uid() {
        let pods = vec![RoderPod {
            name: "roder-a".into(),
            uid: "uid-a".into(),
            node: "node-a".into(),
            ip: "10.0.0.1".into(),
            ready: false,
            tls_fingerprint: None,
        }];
        assert!(holder_pod_exists("roder-a/uid-a", &pods));
        assert!(!holder_pod_exists("roder-a/old-uid", &pods));
        assert!(holder_pod_exists("malformed", &pods));
    }
}
