//! Interactive shell access: exec into a container (probing for the best
//! available shell), and injecting an ephemeral `nicolaka/netshoot` debug
//! container for pods that have nothing exec-able of their own.

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams, Patch, PatchParams};

use super::{api_err, Backend};
use crate::client::K8sError;

impl Backend {
    /// Open an interactive exec session into a pod container. Returns an
    /// `AttachedProcess` whose stdin/stdout can be proxied over a WebSocket.
    /// Probes for the best available shell (bash › sh › ash) before opening
    /// the interactive session.
    pub async fn exec(
        &self,
        ns: &str,
        pod: &str,
        container: Option<&str>,
    ) -> Result<kube::api::AttachedProcess, K8sError> {
        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        let shell = detect_shell(&api, pod, container).await;
        let mut ap = AttachParams::default()
            .stdin(true)
            .stdout(true)
            .stderr(false)
            .tty(true);
        if let Some(c) = container {
            ap = ap.container(c);
        }
        api.exec(pod, vec![shell.as_str()], &ap)
            .await
            .map_err(api_err)
    }

    /// Inject a `nicolaka/netshoot` ephemeral container into `pod`, wait for it
    /// to reach Running, and return its name for use with [`exec`].
    pub async fn inject_debug_container(&self, ns: &str, pod: &str) -> Result<String, K8sError> {
        let api: Api<Pod> = Api::namespaced(self.client(), ns);

        // Fetch current ephemeral containers; the patch replaces the whole
        // array, so we must include any that already exist.
        let pod_obj = api.get(pod).await.map_err(api_err)?;
        let mut containers: Vec<serde_json::Value> = pod_obj
            .spec
            .and_then(|s| s.ephemeral_containers)
            .map(|ecs| {
                ecs.into_iter()
                    .filter_map(|ec| serde_json::to_value(ec).ok())
                    .collect()
            })
            .unwrap_or_default();

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        // Build the set of names already in use so we never push a duplicate.
        // Ephemeral containers are permanent once added; collisions cause a 422.
        let existing: std::collections::HashSet<String> = containers
            .iter()
            .filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(str::to_owned))
            .collect();
        let name = (0u32..)
            .map(|i| {
                if i == 0 {
                    format!("debug-{:06x}", ts & 0x00ff_ffff)
                } else {
                    format!("debug-{:06x}-{i}", ts & 0x00ff_ffff)
                }
            })
            .find(|n| !existing.contains(n))
            .expect("infinite iterator always yields a free name");

        containers.push(serde_json::json!({
            "name": name,
            "image": "nicolaka/netshoot",
            "stdin": true,
            "tty": true,
            "terminationMessagePolicy": "File"
        }));

        api.patch_subresource::<serde_json::Value>(
            "ephemeralcontainers",
            pod,
            &PatchParams::default(),
            &Patch::Merge(serde_json::json!({
                "spec": { "ephemeralContainers": containers }
            })),
        )
        .await
        .map_err(api_err)?;

        // Poll until the container reaches Running (up to 60 s).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            if std::time::Instant::now() > deadline {
                return Err(api_err(format!(
                    "debug container {name} did not start within 60s"
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            // Swallow transient API errors (429, 503, blip) rather than aborting;
            // the container was already injected and cannot be removed, so a brief
            // apiserver hiccup during polling should not strand it permanently.
            if let Ok(p) = api.get(pod).await {
                let running = p
                    .status
                    .and_then(|s| s.ephemeral_container_statuses)
                    .unwrap_or_default()
                    .iter()
                    .any(|cs| {
                        cs.name == name
                            && cs.state.as_ref().and_then(|s| s.running.as_ref()).is_some()
                    });
                if running {
                    return Ok(name);
                }
            }
        }
    }
}

/// Probe the container to find the best available interactive shell.
/// Starts each candidate with no arguments, no stdin, no TTY — the shell reads
/// EOF and exits immediately without loading interactive configs (avoids
/// oh-my-zsh and similar frameworks breaking the probe). Whichever join()
/// returns Ok first wins. Falls back to `/bin/sh` if all probes fail or time out.
async fn detect_shell(api: &Api<Pod>, pod: &str, container: Option<&str>) -> String {
    for shell in ["/bin/bash", "/bin/ash", "/bin/zsh", "/bin/sh"] {
        let mut ap = AttachParams::default()
            .stdin(false)
            .stdout(false)
            .stderr(false)
            .tty(false);
        if let Some(c) = container {
            ap = ap.container(c);
        }
        let Ok(probe) = api.exec(pod, vec![shell], &ap).await else {
            continue;
        };
        match tokio::time::timeout(std::time::Duration::from_secs(2), probe.join()).await {
            Ok(Ok(_)) => return shell.to_string(),
            _ => continue,
        }
    }
    "/bin/sh".to_string()
}
