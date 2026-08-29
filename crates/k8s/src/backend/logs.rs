//! Pod/workload log streaming: which containers to stream (skipping ones that
//! haven't run yet or were already reported), live vs. `previous` selection, and
//! merging multiple containers/pods into one prefixed stream.

use std::pin::Pin;

use futures::io::AsyncBufReadExt;
use futures::{Stream, StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams, LogParams};

use super::{api_err, Backend};
use crate::client::K8sError;

impl Backend {
    /// Whether a pod is not (yet) in a terminal phase, so its logs should be
    /// *followed* (a live stream) rather than fetched once. Only `Succeeded`/`Failed`
    /// pods are done for good; everything else — including `Pending` (image still
    /// pulling, init containers running) — may still produce its first log line, or
    /// crash, the moment the container starts. Unknown pods (not in any cache)
    /// default to following, which is safe for live ones.
    pub async fn pod_active(&self, ns: &str, name: &str) -> bool {
        match self.registry.cached_object("/v1/Pod", Some(ns), name).await {
            Some(obj) => !matches!(
                obj.data
                    .get("status")
                    .and_then(|s| s.get("phase"))
                    .and_then(|p| p.as_str()),
                Some("Succeeded") | Some("Failed")
            ),
            None => true,
        }
    }

    /// A pod's full object as JSON. Tries the informer cache first, falls back to
    /// a live API call — used for the `spec`/`status` introspection the log
    /// container list needs (informer objects and `Pod` don't share a type).
    async fn pod_json(&self, ns: &str, pod: &str) -> Option<serde_json::Value> {
        if let Some(obj) = self.registry.cached_object("/v1/Pod", Some(ns), pod).await {
            return Some(obj.data);
        }
        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        serde_json::to_value(api.get(pod).await.ok()?).ok()
    }

    /// Every container to stream logs for, in the order the pod runs them: init
    /// containers first, then main containers — paired with whether that
    /// container's output can only be reached via `previous`. Containers that
    /// haven't run at all yet (still waiting their turn) are omitted, and a
    /// container whose crash/attempt was already streamed before is omitted too —
    /// see [`Backend::already_reported`].
    async fn pod_log_containers(&self, ns: &str, pod: &str) -> Vec<(String, bool)> {
        let Some(data) = self.pod_json(ns, pod).await else {
            return Vec::new();
        };
        let names = |field: &str| -> Vec<String> {
            data.get("spec")
                .and_then(|s| s.get(field))
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| c.get("name")?.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut out = Vec::new();
        for name in names("initContainers")
            .into_iter()
            .chain(names("containers"))
        {
            match container_log_plan(&data, &name) {
                LogPlan::Skip => {}
                LogPlan::Live => out.push((name, false)),
                LogPlan::Static {
                    previous,
                    signature,
                } => {
                    let key = (ns.to_string(), pod.to_string(), name.clone());
                    if !self.already_reported(key, signature).await {
                        out.push((name, previous));
                    }
                }
            }
        }
        out
    }

    /// Whether `signature` for `(namespace, pod, container)` was already reported
    /// last time — and if not, remembers it. Used so a crashed/stuck container's
    /// static output isn't re-sent on every reconnect to a still-broken pod.
    async fn already_reported(&self, key: (String, String, String), signature: String) -> bool {
        let mut seen = self.log_seen.write().await;
        if seen.get(&key) == Some(&signature) {
            true
        } else {
            seen.insert(key, signature);
            false
        }
    }

    /// Open one container's log stream, tagging each line with `prefix` (empty for
    /// a single-container view). On failure, produce a one-line placeholder
    /// instead of erroring the whole pane — deduped the same way as
    /// [`Backend::pod_log_containers`], so a container that's still stuck doesn't
    /// repeat the same message on every reconnect.
    async fn open_container_log(
        &self,
        ns: &str,
        pod: &str,
        name: &str,
        previous: bool,
        follow: bool,
        prefix: &str,
    ) -> Pin<Box<dyn Stream<Item = String> + Send>> {
        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        let lp = LogParams {
            follow,
            tail_lines: Some(500),
            timestamps: false,
            container: Some(name.to_string()),
            previous,
            ..Default::default()
        };
        match api.log_stream(pod, &lp).await {
            Ok(reader) => {
                let prefix = prefix.to_string();
                Box::pin(
                    reader
                        .lines()
                        .filter_map(|r| async move { r.ok() })
                        .map(move |line| format!("{prefix}{line}")),
                )
            }
            Err(e) => {
                let key = (ns.to_string(), pod.to_string(), name.to_string());
                let msg = e.to_string();
                if self.already_reported(key, msg.clone()).await {
                    return Box::pin(futures::stream::empty());
                }
                tracing::debug!("failed to open log stream for container {name}: {msg}");
                let prefix = prefix.to_string();
                Box::pin(futures::stream::once(async move {
                    format!("{prefix}[roder] failed to stream logs: {msg}")
                }))
            }
        }
    }

    /// Live pod logs as SSE. When a specific container is requested it is streamed
    /// without a prefix. Otherwise every container in the pod is streamed — init
    /// containers first (in spec order), then main containers — with `container │ `
    /// line prefixes, so an init error (or a container that hasn't started yet) is
    /// visible immediately instead of only once the main container finally starts.
    /// A container that hasn't run at all yet is omitted rather than reported as an
    /// error (it's just waiting its turn), and a crashed/stuck container's output
    /// is only ever streamed once per attempt — see [`Backend::pod_log_containers`].
    pub async fn logs(
        &self,
        ns: &str,
        pod: &str,
        container: Option<String>,
        follow: bool,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, K8sError> {
        if let Some(name) = container {
            // Explicit container selection — stream it without any prefix. (No UI
            // path sends this today; kept for completeness / direct API use.)
            let previous = match self.pod_json(ns, pod).await {
                Some(data) => {
                    matches!(container_log_plan(&data, &name), LogPlan::Static { previous, .. } if previous)
                }
                None => false,
            };
            let api: Api<Pod> = Api::namespaced(self.client(), ns);
            let lp = LogParams {
                follow,
                tail_lines: Some(500),
                timestamps: false,
                container: Some(name),
                previous,
                ..Default::default()
            };
            let lines = api
                .log_stream(pod, &lp)
                .await
                .map_err(api_err)?
                .lines()
                .filter_map(|r| async move { r.ok() });
            return Ok(Box::pin(lines));
        }

        let containers = self.pod_log_containers(ns, pod).await;
        if containers.is_empty() {
            // Nothing to show right now — every container is either still waiting
            // its turn, or its last attempt was already streamed. An empty stream
            // ends at once; the caller's `follow`/`eof` handling decides whether to
            // retry (see `server::api::logs`).
            return Ok(Box::pin(futures::stream::empty()));
        }

        // Prefix lines only when merging more than one container, matching the
        // pre-existing single-container (no pill) presentation.
        let prefixed = containers.len() > 1;
        let mut streams: Vec<Pin<Box<dyn Stream<Item = String> + Send>>> = Vec::new();
        for (name, previous) in containers {
            let prefix = if prefixed {
                format!("{name} │ ")
            } else {
                String::new()
            };
            streams.push(
                self.open_container_log(ns, pod, &name, previous, follow, &prefix)
                    .await,
            );
        }
        Ok(Box::pin(futures::stream::select_all(streams)))
    }

    /// Aggregated logs for a workload: resolve its pods by `spec.selector` and merge
    /// every pod's log stream into one, each line prefixed `pod │ `.
    pub async fn logs_workload(
        &self,
        key: &str,
        ns: &str,
        name: &str,
        follow: bool,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, K8sError> {
        let obj = self
            .dyn_api(key, Some(ns))?
            .get(name)
            .await
            .map_err(api_err)?;
        let data = serde_json::to_value(&obj).map_err(api_err)?;
        let selector = workload_label_selector(&data).map_err(K8sError::Api)?;

        let api: Api<Pod> = Api::namespaced(self.client(), ns);
        let pods = api
            .list(&ListParams::default().labels(&selector))
            .await
            .map_err(api_err)?;

        let mut streams: Vec<Pin<Box<dyn Stream<Item = String> + Send>>> = Vec::new();
        for p in pods.items {
            let pod = p.metadata.name.unwrap_or_default();
            let container = if p.spec.as_ref().map(|s| s.containers.len()).unwrap_or(0) > 1 {
                p.spec
                    .and_then(|s| s.containers.into_iter().next())
                    .map(|c| c.name)
            } else {
                None
            };
            let lp = LogParams {
                follow,
                tail_lines: Some(200),
                timestamps: false,
                container,
                ..Default::default()
            };
            match api.log_stream(&pod, &lp).await {
                Ok(reader) => {
                    let s = reader
                        .lines()
                        .filter_map(|r| async move { r.ok() })
                        .map(move |line| format!("{pod} │ {line}"));
                    streams.push(Box::pin(s));
                }
                Err(e) => {
                    tracing::debug!("failed to open log stream for pod {pod}: {e}");
                    let msg = format!("{pod} │ [roder] failed to stream logs: {e}");
                    streams.push(Box::pin(futures::stream::once(async move { msg })));
                }
            }
        }
        Ok(Box::pin(futures::stream::select_all(streams)))
    }
}

pub(super) fn workload_label_selector(data: &serde_json::Value) -> Result<String, String> {
    let selector = data
        .get("spec")
        .and_then(|spec| spec.get("selector"))
        .ok_or_else(|| "workload has no label selector".to_string())?;
    let mut requirements = Vec::new();

    if let Some(labels) = selector
        .get("matchLabels")
        .and_then(|value| value.as_object())
    {
        for (key, value) in labels {
            let value = value
                .as_str()
                .ok_or_else(|| format!("workload selector label {key} is not a string"))?;
            requirements.push(format!("{key}={value}"));
        }
    }

    if let Some(expressions) = selector
        .get("matchExpressions")
        .and_then(|value| value.as_array())
    {
        for expression in expressions {
            let key = expression
                .get("key")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "workload selector expression is missing key".to_string())?;
            let operator = expression
                .get("operator")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    format!("workload selector expression for {key} is missing operator")
                })?;
            let values = expression
                .get("values")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .map(|value| {
                            value.as_str().ok_or_else(|| {
                                format!(
                                    "workload selector expression for {key} has a non-string value"
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            let requirement = match operator {
                "In" | "NotIn" if values.is_empty() => {
                    return Err(format!(
                        "workload selector expression {operator} for {key} has no values"
                    ));
                }
                "In" => format!("{key} in ({})", values.join(",")),
                "NotIn" => format!("{key} notin ({})", values.join(",")),
                "Exists" => key.to_string(),
                "DoesNotExist" => format!("!{key}"),
                other => {
                    return Err(format!(
                        "workload selector expression for {key} has unsupported operator {other}"
                    ));
                }
            };
            requirements.push(requirement);
        }
    }

    requirements.sort();
    if requirements.is_empty() {
        return Err("workload label selector is empty".into());
    }
    Ok(requirements.join(","))
}

/// How to log a container given its current status.
enum LogPlan {
    /// Hasn't run at all yet — just waiting its turn; not an error, don't report it.
    Skip,
    /// Currently running — stream it live.
    Live,
    /// A finished attempt: terminated (and not yet superseded by a new attempt),
    /// or — if already backing off toward a restart — only reachable via
    /// `previous`. `signature` fingerprints the attempt so a repeat fetch of the
    /// same crash can be recognized and skipped.
    Static { previous: bool, signature: String },
}

fn container_log_plan(pod_data: &serde_json::Value, name: &str) -> LogPlan {
    let Some(status) = ["initContainerStatuses", "containerStatuses"]
        .iter()
        .find_map(|field| {
            pod_data
                .get("status")?
                .get(*field)?
                .as_array()?
                .iter()
                .find(|c| c.get("name").and_then(|n| n.as_str()) == Some(name))
        })
    else {
        return LogPlan::Skip;
    };
    let state = status.get("state");
    if state.and_then(|s| s.get("running")).is_some() {
        return LogPlan::Live;
    }
    if let Some(terminated) = state.and_then(|s| s.get("terminated")) {
        return LogPlan::Static {
            previous: false,
            signature: attempt_signature(terminated),
        };
    }
    // `waiting`: nothing current to show; fall back to the last completed attempt.
    match status.get("lastState").and_then(|s| s.get("terminated")) {
        Some(terminated) => LogPlan::Static {
            previous: true,
            signature: attempt_signature(terminated),
        },
        None => LogPlan::Skip,
    }
}

/// A stable fingerprint for one container attempt (exit code + time it ended), so
/// a repeat fetch of the same crash can be recognized and skipped.
fn attempt_signature(terminated: &serde_json::Value) -> String {
    let code = terminated
        .get("exitCode")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let at = terminated
        .get("finishedAt")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    format!("{code}@{at}")
}

#[cfg(test)]
mod tests {
    use super::workload_label_selector;
    use serde_json::json;

    #[test]
    fn workload_selector_includes_labels_and_expressions() {
        let data = json!({
            "spec": {"selector": {
                "matchLabels": {"app": "api"},
                "matchExpressions": [
                    {"key": "tier", "operator": "In", "values": ["web", "worker"]},
                    {"key": "deprecated", "operator": "DoesNotExist"},
                    {"key": "track", "operator": "NotIn", "values": ["canary"]},
                    {"key": "managed", "operator": "Exists"}
                ]
            }}
        });
        assert_eq!(
            workload_label_selector(&data).unwrap(),
            "!deprecated,app=api,managed,tier in (web,worker),track notin (canary)"
        );
    }

    #[test]
    fn match_expressions_only_selector_remains_scoped() {
        let data = json!({
            "spec": {"selector": {"matchExpressions": [
                {"key": "app", "operator": "Exists"}
            ]}}
        });
        assert_eq!(workload_label_selector(&data).unwrap(), "app");
    }

    #[test]
    fn empty_or_malformed_selector_is_rejected() {
        assert!(workload_label_selector(&json!({"spec": {"selector": {}}})).is_err());
        assert!(workload_label_selector(&json!({
            "spec": {"selector": {"matchExpressions": [
                {"key": "app", "operator": "In", "values": []}
            ]}}
        }))
        .is_err());
    }
}
