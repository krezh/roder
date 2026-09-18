//! Cluster-wide resource recommendation scan.
//!
//! Workloads and their declared requests come from the caller's own RBAC,
//! usage history from Prometheus. One-shot rather than an informer: a two-week
//! query per workload is far too expensive to refresh on a timer.

use futures::StreamExt;
use kube::api::{DynamicObject, ListParams};
use roder_core::{
    sort_advice, ContainerResources, ResourceAdvice, ResourceScan, ScanFailure, WorkloadKind,
};
use serde_json::Value;

use super::Backend;
use crate::client::K8sError;
use crate::metrics::{parse_cpu, parse_mem};
use crate::promql::PromClient;
use crate::recommend::{ContainerSpec, ScanRequest, ScanSettings, Scanner, WorkloadRef};

/// How many workloads are queried at once. Each one is several Prometheus
/// subqueries over the whole window, so this is the knob that decides whether a
/// scan is brisk or buries Prometheus.
const SCAN_CONCURRENCY: usize = 6;

/// The kinds a scan covers, with where each one's pod template lives.
///
/// Bare Jobs are omitted: they belong to a CronJob, which is scanned as a unit.
const SCANNED: &[(&str, &str, WorkloadKind, &[&str])] = &[
    (
        "apps",
        "Deployment",
        WorkloadKind::Deployment,
        &["spec", "template", "spec"],
    ),
    (
        "apps",
        "StatefulSet",
        WorkloadKind::StatefulSet,
        &["spec", "template", "spec"],
    ),
    (
        "apps",
        "DaemonSet",
        WorkloadKind::DaemonSet,
        &["spec", "template", "spec"],
    ),
    (
        "batch",
        "CronJob",
        WorkloadKind::CronJob,
        &["spec", "jobTemplate", "spec", "template", "spec"],
    ),
];

impl Backend {
    /// Scan every supported workload and recommend requests and limits.
    ///
    /// Unlistable workloads are skipped and Prometheus failures collected per
    /// workload, so one bad workload never loses the rest of the report.
    pub async fn resource_scan(
        &self,
        prom: &PromClient,
        settings: ScanSettings,
        namespace: Option<&str>,
    ) -> Result<ResourceScan, K8sError> {
        let autoscaled = self.autoscaled_workloads(namespace).await;
        let requests = self.scan_requests(namespace, &autoscaled).await;
        let scanned_workloads = requests.len();
        let scanner = Scanner::new(prom, settings);

        // Each request is moved into its own future rather than borrowed, so the
        // closure doesn't need to be generic over the borrow's lifetime.
        let results: Vec<_> = futures::stream::iter(requests)
            .map(|request| {
                let scanner = &scanner;
                async move {
                    let result = scanner.scan(&request).await;
                    (request, result)
                }
            })
            .buffer_unordered(SCAN_CONCURRENCY)
            .collect()
            .await;

        let mut rows: Vec<ResourceAdvice> = Vec::new();
        let mut failures = Vec::new();
        for (request, result) in results {
            match result {
                Ok(advice) => rows.extend(advice),
                Err(error) => {
                    tracing::warn!(
                        "recommend: {}/{} failed: {error}",
                        request.workload.namespace,
                        request.workload.name
                    );
                    failures.push(ScanFailure {
                        namespace: request.workload.namespace,
                        workload: request.workload.name,
                        error: error.to_string(),
                    });
                }
            }
        }
        sort_advice(&mut rows);

        Ok(ResourceScan {
            rows,
            history_hours: settings.history.as_secs_f64() / 3600.0,
            cpu_percentile: settings.cpu_percentile,
            scanned_workloads,
            failures,
        })
    }

    /// One [`ScanRequest`] per workload the caller can list.
    async fn scan_requests(
        &self,
        namespace: Option<&str>,
        autoscaled: &[(String, String, String)],
    ) -> Vec<ScanRequest> {
        let kinds = self.kinds();
        let mut requests = Vec::new();

        for (group, kind, workload_kind, template) in SCANNED {
            let Some(entry) = kinds
                .iter()
                .find(|candidate| candidate.group == *group && candidate.kind == *kind)
            else {
                continue;
            };
            let api = match self.dyn_api(&entry.key, namespace) {
                Ok(api) => api,
                Err(error) => {
                    tracing::debug!("recommend: cannot resolve {kind} API: {error}");
                    continue;
                }
            };
            let list = match api.list(&ListParams::default()).await {
                Ok(list) => list,
                Err(error) => {
                    tracing::debug!("recommend: cannot list {kind}: {error}");
                    continue;
                }
            };

            for object in list {
                let Some(request) = scan_request(&object, *workload_kind, template, autoscaled)
                else {
                    continue;
                };
                requests.push(request);
            }
        }
        requests
    }

    /// `(namespace, target kind, target name)` for every HPA the caller can see.
    ///
    /// An unreadable list is not fatal — the scan just also advises on
    /// autoscaled workloads, which is noisier but still truthful.
    async fn autoscaled_workloads(&self, namespace: Option<&str>) -> Vec<(String, String, String)> {
        let kinds = self.kinds();
        let Some(entry) = kinds.iter().find(|candidate| {
            candidate.group == "autoscaling" && candidate.kind == "HorizontalPodAutoscaler"
        }) else {
            return Vec::new();
        };
        let Ok(api) = self.dyn_api(&entry.key, namespace) else {
            return Vec::new();
        };
        let Ok(list) = api.list(&ListParams::default()).await else {
            tracing::debug!("recommend: cannot list HorizontalPodAutoscalers");
            return Vec::new();
        };

        list.into_iter()
            .filter_map(|hpa| {
                let data = serde_json::to_value(&hpa).ok()?;
                let target = data.pointer("/spec/scaleTargetRef")?;
                Some((
                    hpa.metadata.namespace.clone().unwrap_or_default(),
                    target.get("kind")?.as_str()?.to_string(),
                    target.get("name")?.as_str()?.to_string(),
                ))
            })
            .collect()
    }
}

/// Build a scan request from a workload object, or `None` when it declares no
/// containers at all.
fn scan_request(
    object: &DynamicObject,
    kind: WorkloadKind,
    template: &[&str],
    autoscaled: &[(String, String, String)],
) -> Option<ScanRequest> {
    let name = object.metadata.name.clone()?;
    let namespace = object.metadata.namespace.clone().unwrap_or_default();
    let data = serde_json::to_value(object).ok()?;

    let containers = template_containers(&data, template);
    if containers.is_empty() {
        return None;
    }

    let has_hpa = autoscaled
        .iter()
        .any(|(hpa_namespace, hpa_kind, hpa_name)| {
            hpa_namespace == &namespace && hpa_kind == kind.label() && hpa_name == &name
        });

    Some(ScanRequest {
        workload: WorkloadRef {
            namespace,
            kind,
            name,
        },
        containers,
        has_hpa,
    })
}

/// Every container in a pod template, with its declared requests and limits.
///
/// Init containers are left out: they run once, so a percentile over the whole
/// window recommends nearly nothing for them.
fn template_containers(data: &Value, template: &[&str]) -> Vec<ContainerSpec> {
    let mut pointer = data;
    for segment in template {
        let Some(next) = pointer.get(segment) else {
            return Vec::new();
        };
        pointer = next;
    }

    pointer
        .get("containers")
        .and_then(Value::as_array)
        .map(|containers| {
            containers
                .iter()
                .filter_map(|container| {
                    Some(ContainerSpec {
                        name: container.get("name")?.as_str()?.to_string(),
                        current: container_resources(container),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Read one container's `resources` block into cores and bytes.
///
/// An absent key stays `None`, not zero: "no request set" is itself a finding,
/// and `parse_cpu`/`parse_mem` return 0.0 for anything unreadable.
fn container_resources(container: &Value) -> ContainerResources {
    let quantity = |kind: &str, key: &str, parse: fn(&str) -> f64| {
        container
            .get("resources")
            .and_then(|resources| resources.get(kind))
            .and_then(|block| block.get(key))
            .and_then(Value::as_str)
            .map(parse)
    };

    ContainerResources {
        cpu_request: quantity("requests", "cpu", parse_cpu),
        cpu_limit: quantity("limits", "cpu", parse_cpu),
        memory_request: quantity("requests", "memory", parse_mem),
        memory_limit: quantity("limits", "memory", parse_mem),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn deployment(resources: Value) -> Value {
        json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": {"name": "web", "namespace": "prod"},
            "spec": {"template": {"spec": {"containers": [
                {"name": "app", "resources": resources},
                {"name": "sidecar"}
            ]}}}
        })
    }

    #[test]
    fn declared_requests_and_limits_are_read_per_container() {
        let data = deployment(json!({
            "requests": {"cpu": "100m", "memory": "256Mi"},
            "limits": {"memory": "512Mi"}
        }));
        let containers = template_containers(&data, &["spec", "template", "spec"]);

        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].name, "app");
        assert_eq!(containers[0].current.cpu_request, Some(0.1));
        assert_eq!(
            containers[0].current.memory_request,
            Some(256.0 * 1024.0 * 1024.0)
        );
        // An unset CPU limit stays unset — which is what KRR recommends anyway.
        assert_eq!(containers[0].current.cpu_limit, None);
    }

    #[test]
    fn a_container_with_no_resources_block_reports_everything_unset() {
        let data = deployment(json!({}));
        let containers = template_containers(&data, &["spec", "template", "spec"]);
        let sidecar = &containers[1];

        assert_eq!(sidecar.name, "sidecar");
        assert_eq!(sidecar.current, ContainerResources::default());
    }

    #[test]
    fn cronjob_templates_are_found_one_level_deeper() {
        let data = json!({
            "metadata": {"name": "backup", "namespace": "prod"},
            "spec": {"jobTemplate": {"spec": {"template": {"spec": {"containers": [
                {"name": "backup", "resources": {"requests": {"cpu": "2"}}}
            ]}}}}}
        });
        let containers =
            template_containers(&data, &["spec", "jobTemplate", "spec", "template", "spec"]);

        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].current.cpu_request, Some(2.0));
    }

    #[test]
    fn a_missing_template_path_yields_no_containers() {
        let data = json!({"metadata": {"name": "web"}, "spec": {}});
        assert!(template_containers(&data, &["spec", "template", "spec"]).is_empty());
    }

    #[test]
    fn hpa_targets_are_matched_by_namespace_kind_and_name() {
        let autoscaled = vec![(
            "prod".to_string(),
            "Deployment".to_string(),
            "web".to_string(),
        )];
        let object: DynamicObject = serde_json::from_value(deployment(json!({
            "requests": {"cpu": "100m"}
        })))
        .expect("valid object");

        let request = scan_request(
            &object,
            WorkloadKind::Deployment,
            &["spec", "template", "spec"],
            &autoscaled,
        )
        .expect("request");
        assert!(request.has_hpa);

        // The same name under a different kind is a different workload.
        let other = scan_request(
            &object,
            WorkloadKind::StatefulSet,
            &["spec", "template", "spec"],
            &autoscaled,
        )
        .expect("request");
        assert!(!other.has_hpa);
    }
}
