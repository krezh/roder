//! Resource request/limit recommendations from Prometheus history.
//!
//! Implements KRR's `simple` strategy: CPU request from a percentile of usage,
//! memory request from the peak plus headroom, no CPU limit. Defaults in
//! [`ScanSettings`] match KRR's so numbers here are comparable to `krr simple`.
//!
//! Requires cAdvisor **and** kube-state-metrics: cAdvisor series are keyed by
//! pod, and over a two-week window most of a workload's pods have been
//! replaced, so the owner walk below is the only way to find which pods count.
//!
//! The caller supplies the workloads and their declared requests, keeping this
//! module a pure function of (workload, Prometheus).

use std::collections::HashMap;
use std::time::Duration;

use roder_core::{AdviceSeverity, ContainerResources, ResourceAdvice, WorkloadKind};
use tracing::warn;

use crate::promql::{
    escape_label_value, escape_regex, promql_duration, PromClient, PromError, Sample,
};

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Tunables for the `simple` strategy. [`Default`] mirrors KRR's own defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanSettings {
    /// How far back to look.
    pub history: Duration,
    /// Subquery resolution. KRR's 1.25min; also the `rate()` window.
    pub step: Duration,
    /// Percentile of CPU usage that becomes the CPU request, 0-100.
    pub cpu_percentile: f64,
    /// Headroom added to peak memory, as a percentage.
    pub memory_buffer_percentage: f64,
    /// Headroom added to the limit a container was OOMKilled at.
    pub oom_buffer_percentage: f64,
    /// Below this many samples the window is too sparse to recommend from.
    pub points_required: u64,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            history: Duration::from_secs(24 * 7 * 2 * 3600), // 336h
            step: Duration::from_secs(75),                   // 1.25min
            cpu_percentile: 95.0,
            memory_buffer_percentage: 15.0,
            oom_buffer_percentage: 25.0,
            points_required: 100,
        }
    }
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadRef {
    pub namespace: String,
    pub kind: WorkloadKind,
    pub name: String,
}

/// kube-state-metrics labels owners by Kubernetes kind name, which is exactly
/// what [`WorkloadKind::label`] already renders.
fn owner_kind(kind: WorkloadKind) -> &'static str {
    kind.label()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContainerSpec {
    pub name: String,
    pub current: ContainerResources,
}

/// One workload to scan, with everything the live API already knows about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanRequest {
    pub workload: WorkloadRef,
    pub containers: Vec<ContainerSpec>,
    /// An HPA already moves this workload's replica count, so recommending
    /// per-container requests from historical usage would fight it.
    pub has_hpa: bool,
}

// ---------------------------------------------------------------------------
// Outputs
// ---------------------------------------------------------------------------

/// Absolute gaps at which a CPU difference changes severity, in cores.
const CPU_CRITICAL: f64 = 0.5;
const CPU_WARNING: f64 = 0.25;
const CPU_OK: f64 = 0.1;
/// …and for memory, in bytes.
const MEM_CRITICAL: f64 = 500.0 * 1_000_000.0;
const MEM_WARNING: f64 = 250.0 * 1_000_000.0;
const MEM_OK: f64 = 100.0 * 1_000_000.0;

fn severity(
    current: Option<f64>,
    recommended: Option<f64>,
    thresholds: (f64, f64, f64),
) -> AdviceSeverity {
    let (critical, warning, ok) = thresholds;
    match (current, recommended) {
        (None, None) => AdviceSeverity::Good,
        // One side unset is always worth surfacing: either nothing is requested,
        // or usage is too thin to justify what is requested.
        (None, Some(_)) | (Some(_), None) => AdviceSeverity::Warning,
        (Some(current), Some(recommended)) => {
            let difference = (current - recommended).abs();
            if difference >= critical {
                AdviceSeverity::Critical
            } else if difference >= warning {
                AdviceSeverity::Warning
            } else if difference >= ok {
                AdviceSeverity::Ok
            } else {
                AdviceSeverity::Good
            }
        }
    }
}

/// A row carrying no recommendation, with the reason why.
fn undefined(request: &ScanRequest, container: &ContainerSpec, info: &str) -> ResourceAdvice {
    ResourceAdvice {
        namespace: request.workload.namespace.clone(),
        workload: request.workload.name.clone(),
        workload_kind: request.workload.kind,
        container: container.name.clone(),
        current: container.current,
        recommended: ContainerResources::default(),
        cpu_severity: AdviceSeverity::Unknown,
        memory_severity: AdviceSeverity::Unknown,
        info: Some(info.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Query construction
// ---------------------------------------------------------------------------

/// Past this, the selector regex costs more than the extra history is worth.
/// Reached almost only by CronJobs that run every minute.
const MAX_PODS_IN_SELECTOR: usize = 500;

/// `pod=~"a|b|c"`, each name escaped as a regex literal and then for the
/// surrounding string literal.
fn pod_selector(pods: &[String]) -> String {
    let joined = pods
        .iter()
        .map(|pod| escape_regex(pod))
        .collect::<Vec<_>>()
        .join("|");
    escape_label_value(&joined)
}

fn owner_query(
    metric: &str,
    namespace: &str,
    owner_kind: &str,
    owner_name: &str,
    history: Duration,
) -> String {
    format!(
        r#"last_over_time({metric}{{namespace="{namespace}", owner_kind="{owner_kind}", owner_name="{owner_name}"}}[{history}])"#,
        namespace = escape_label_value(namespace),
        owner_name = escape_label_value(owner_name),
        history = promql_duration(history),
    )
}

/// Pods owned by any of `owner_names`, matched as a regex alternation.
fn pod_owner_query(
    namespace: &str,
    owner_kind: &str,
    owner_names: &[String],
    history: Duration,
) -> String {
    format!(
        r#"last_over_time(kube_pod_owner{{namespace="{namespace}", owner_kind="{owner_kind}", owner_name=~"{owners}"}}[{history}])"#,
        namespace = escape_label_value(namespace),
        owners = pod_selector(owner_names),
        history = promql_duration(history),
    )
}

/// CPU request candidate: the percentile of per-pod CPU rate over the window.
///
/// Grouped `by (container, pod, job)` with no container filter, so one query
/// answers for every container in the workload at once.
fn cpu_percentile_query(namespace: &str, pods: &[String], settings: &ScanSettings) -> String {
    format!(
        r#"quantile_over_time({quantile}, max(rate(container_cpu_usage_seconds_total{{namespace="{namespace}", pod=~"{pods}", container!=""}}[{step}])) by (container, pod, job)[{history}:{step}])"#,
        quantile = settings.cpu_percentile / 100.0,
        namespace = escape_label_value(namespace),
        pods = pod_selector(pods),
        step = promql_duration(settings.step),
        history = promql_duration(settings.history),
    )
}

/// Memory request candidate: peak working set over the window.
fn memory_max_query(namespace: &str, pods: &[String], settings: &ScanSettings) -> String {
    format!(
        r#"max_over_time(max(container_memory_working_set_bytes{{namespace="{namespace}", pod=~"{pods}", container!=""}}) by (container, pod, job)[{history}:{step}])"#,
        namespace = escape_label_value(namespace),
        pods = pod_selector(pods),
        step = promql_duration(settings.step),
        history = promql_duration(settings.history),
    )
}

/// How many samples actually exist, so a workload that only ran for an hour is
/// reported as "not enough data" rather than recommended from noise.
fn point_count_query(namespace: &str, pods: &[String], settings: &ScanSettings) -> String {
    format!(
        r#"count_over_time(max(container_memory_working_set_bytes{{namespace="{namespace}", pod=~"{pods}", container!=""}}) by (container, pod, job)[{history}:{step}])"#,
        namespace = escape_label_value(namespace),
        pods = pod_selector(pods),
        step = promql_duration(settings.step),
        history = promql_duration(settings.history),
    )
}

/// The memory limit in force when a container was last OOMKilled.
///
/// A stronger signal than observed usage, which by definition never exceeded it.
fn oomkill_query(namespace: &str, pods: &[String], settings: &ScanSettings) -> String {
    format!(
        r#"max_over_time(max(max(kube_pod_container_resource_limits{{resource="memory", namespace="{namespace}", pod=~"{pods}"}}) by (pod, container, job) * on(pod, container, job) group_left(reason) max(kube_pod_container_status_last_terminated_reason{{reason="OOMKilled", namespace="{namespace}", pod=~"{pods}"}}) by (pod, container, job, reason)) by (container, pod, job)[{history}:{step}])"#,
        namespace = escape_label_value(namespace),
        pods = pod_selector(pods),
        step = promql_duration(settings.step),
        history = promql_duration(settings.history),
    )
}

// ---------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------

/// Collapse an instant vector to the highest finite value per `container`.
///
/// One series per pod, so this is the worst any replica did — KRR recommends
/// per workload, not per replica.
fn max_by_container(samples: &[Sample]) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    for sample in samples {
        let Some(container) = sample.label("container") else {
            continue;
        };
        if !sample.value.is_finite() {
            continue;
        }
        out.entry(container.to_string())
            .and_modify(|current| *current = current.max(sample.value))
            .or_insert(sample.value);
    }
    out
}

/// Total sample count per container, summed across the workload's pods.
fn sum_by_container(samples: &[Sample]) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    for sample in samples {
        let Some(container) = sample.label("container") else {
            continue;
        };
        if !sample.value.is_finite() {
            continue;
        }
        *out.entry(container.to_string()).or_insert(0.0) += sample.value;
    }
    out
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

pub struct Scanner<'a> {
    prom: &'a PromClient,
    settings: ScanSettings,
}

impl<'a> Scanner<'a> {
    pub fn new(prom: &'a PromClient, settings: ScanSettings) -> Self {
        Self { prom, settings }
    }

    /// Recommend requests and limits for every container of one workload.
    ///
    /// Returns a row per container either way: a container with too little
    /// history is reported as undefined with a reason, never silently dropped.
    pub async fn scan(&self, request: &ScanRequest) -> Result<Vec<ResourceAdvice>, PromError> {
        if request.has_hpa {
            return Ok(self.undefined_rows(request, "HPA detected"));
        }

        let pods = self.resolve_pods(&request.workload).await?;
        if pods.is_empty() {
            return Ok(self.undefined_rows(request, "No pod history"));
        }

        let namespace = &request.workload.namespace;
        let cpu_query = cpu_percentile_query(namespace, &pods, &self.settings);
        let memory_query = memory_max_query(namespace, &pods, &self.settings);
        let points_query = point_count_query(namespace, &pods, &self.settings);
        let oom_query = oomkill_query(namespace, &pods, &self.settings);
        let (cpu, memory, points, oom) = futures::try_join!(
            self.prom.query(&cpu_query),
            self.prom.query(&memory_query),
            self.prom.query(&points_query),
            self.prom.query(&oom_query),
        )?;

        let cpu = max_by_container(&cpu);
        let memory = max_by_container(&memory);
        let points = sum_by_container(&points);
        let oom = max_by_container(&oom);

        Ok(request
            .containers
            .iter()
            .map(|container| self.recommend(request, container, &cpu, &memory, &points, &oom))
            .collect())
    }

    fn recommend(
        &self,
        request: &ScanRequest,
        container: &ContainerSpec,
        cpu: &HashMap<String, f64>,
        memory: &HashMap<String, f64>,
        points: &HashMap<String, f64>,
        oom: &HashMap<String, f64>,
    ) -> ResourceAdvice {
        let observed = points.get(&container.name).copied().unwrap_or(0.0);
        if observed < self.settings.points_required as f64 {
            return undefined(request, container, "Not enough data");
        }

        let cpu_request = cpu.get(&container.name).copied();
        // After an OOMKill, usage stops being evidence — the kernel intervened —
        // so the limit that killed it sets the floor instead.
        let memory_request = memory.get(&container.name).copied().map(|peak| {
            let with_headroom = peak * (1.0 + self.settings.memory_buffer_percentage / 100.0);
            match oom.get(&container.name) {
                Some(limit) => {
                    with_headroom.max(limit * (1.0 + self.settings.oom_buffer_percentage / 100.0))
                }
                None => with_headroom,
            }
        });

        let recommended = ContainerResources {
            cpu_request,
            // KRR leaves CPU limits unset on purpose: a throttled container is
            // slow in ways that are far harder to diagnose than a noisy one.
            cpu_limit: None,
            memory_request,
            memory_limit: memory_request,
        };

        ResourceAdvice {
            namespace: request.workload.namespace.clone(),
            workload: request.workload.name.clone(),
            workload_kind: request.workload.kind,
            container: container.name.clone(),
            current: container.current,
            recommended,
            cpu_severity: severity(
                container.current.cpu_request,
                recommended.cpu_request,
                (CPU_CRITICAL, CPU_WARNING, CPU_OK),
            ),
            memory_severity: severity(
                container.current.memory_request,
                recommended.memory_request,
                (MEM_CRITICAL, MEM_WARNING, MEM_OK),
            ),
            info: None,
        }
    }

    fn undefined_rows(&self, request: &ScanRequest, info: &str) -> Vec<ResourceAdvice> {
        request
            .containers
            .iter()
            .map(|container| undefined(request, container, info))
            .collect()
    }

    /// Every pod that belonged to this workload during the window.
    ///
    /// Walks kube-state-metrics ownership, not a live label selector: the pods
    /// carrying most of the history no longer exist.
    async fn resolve_pods(&self, workload: &WorkloadRef) -> Result<Vec<String>, PromError> {
        let history = self.settings.history;
        let namespace = &workload.namespace;

        let pods = match workload.kind {
            // The pod's owner is a ReplicaSet, so the Deployment is one hop away.
            WorkloadKind::Deployment => {
                let replicasets = self
                    .label_values(
                        &owner_query(
                            "kube_replicaset_owner",
                            namespace,
                            owner_kind(workload.kind),
                            &workload.name,
                            history,
                        ),
                        "replicaset",
                    )
                    .await?;
                if replicasets.is_empty() {
                    return Ok(Vec::new());
                }
                self.pods_owned_by(namespace, "ReplicaSet", &replicasets, history)
                    .await?
            }
            // A CronJob owns Jobs, which own the pods.
            WorkloadKind::CronJob => {
                let jobs = self
                    .label_values(
                        &owner_query(
                            "kube_job_owner",
                            namespace,
                            owner_kind(workload.kind),
                            &workload.name,
                            history,
                        ),
                        "job_name",
                    )
                    .await?;
                if jobs.is_empty() {
                    return Ok(Vec::new());
                }
                self.pods_owned_by(namespace, "Job", &jobs, history).await?
            }
            // A bare pod is its own history.
            WorkloadKind::Pod => vec![workload.name.clone()],
            // StatefulSet, DaemonSet and Job own their pods directly.
            kind => {
                self.pods_owned_by(
                    namespace,
                    owner_kind(kind),
                    std::slice::from_ref(&workload.name),
                    history,
                )
                .await?
            }
        };

        Ok(truncate_pods(pods, workload))
    }

    async fn pods_owned_by(
        &self,
        namespace: &str,
        owner_kind: &str,
        owner_names: &[String],
        history: Duration,
    ) -> Result<Vec<String>, PromError> {
        self.label_values(
            &pod_owner_query(namespace, owner_kind, owner_names, history),
            "pod",
        )
        .await
    }

    /// Run an instant query and collect one label's distinct values.
    async fn label_values(&self, query: &str, label: &str) -> Result<Vec<String>, PromError> {
        let samples = self.prom.query(query).await?;
        let mut values: Vec<String> = samples
            .iter()
            .filter_map(|sample| sample.label(label))
            .map(str::to_string)
            .collect();
        values.sort_unstable();
        values.dedup();
        Ok(values)
    }
}

/// Keep the newest names when a workload has outgrown the selector. Sorted
/// order puts the most recent generation last for hash-suffixed pod names.
fn truncate_pods(mut pods: Vec<String>, workload: &WorkloadRef) -> Vec<String> {
    if pods.len() > MAX_PODS_IN_SELECTOR {
        warn!(
            "recommend: {}/{} has {} historical pods, scanning the most recent {}",
            workload.namespace,
            workload.name,
            pods.len(),
            MAX_PODS_IN_SELECTOR
        );
        pods.drain(..pods.len() - MAX_PODS_IN_SELECTOR);
    }
    pods
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(container: &str, pod: &str, value: f64) -> Sample {
        Sample {
            labels: HashMap::from([
                ("container".to_string(), container.to_string()),
                ("pod".to_string(), pod.to_string()),
            ]),
            timestamp: 1_700_000_000.0,
            value,
        }
    }

    fn settings() -> ScanSettings {
        ScanSettings::default()
    }

    #[test]
    fn defaults_match_krr() {
        let settings = settings();
        assert_eq!(settings.history, Duration::from_secs(336 * 3600));
        assert_eq!(settings.step, Duration::from_secs(75));
        assert_eq!(settings.cpu_percentile, 95.0);
        assert_eq!(settings.memory_buffer_percentage, 15.0);
        assert_eq!(settings.points_required, 100);
    }

    #[test]
    fn cpu_query_carries_percentile_history_and_step() {
        let query = cpu_percentile_query("prod", &["api-0".to_string()], &settings());
        assert!(query.starts_with("quantile_over_time(0.95, max(rate("));
        assert!(query.contains("[336h:75s]"));
        assert!(query.contains(r#"namespace="prod""#));
        assert!(query.contains(r#"pod=~"api-0""#));
        assert!(query.contains("by (container, pod, job)"));
    }

    #[test]
    fn memory_query_takes_peak_working_set() {
        let query = memory_max_query("prod", &["api-0".to_string()], &settings());
        assert!(query.starts_with("max_over_time(max(container_memory_working_set_bytes"));
        assert!(query.contains("[336h:75s]"));
    }

    #[test]
    fn pod_selectors_escape_regex_metacharacters() {
        // A dot in a pod name must not match any character.
        let selector = pod_selector(&["web-1.2".to_string(), "web-3".to_string()]);
        assert_eq!(selector, r"web-1\\.2|web-3");
        // Alternation between names survives escaping.
        assert!(selector.contains('|'));
    }

    #[test]
    fn deployment_owner_walk_queries_replicasets_first() {
        let query = owner_query(
            "kube_replicaset_owner",
            "prod",
            "Deployment",
            "web",
            Duration::from_secs(336 * 3600),
        );
        assert!(query.contains("kube_replicaset_owner"));
        assert!(query.contains(r#"owner_kind="Deployment""#));
        assert!(query.contains(r#"owner_name="web""#));
        assert!(query.starts_with("last_over_time("));
        assert!(query.contains("[336h]"));
    }

    #[test]
    fn oomkill_query_joins_limits_to_termination_reason() {
        let query = oomkill_query("prod", &["api-0".to_string()], &settings());
        assert!(query.contains("kube_pod_container_resource_limits"));
        assert!(query.contains(r#"reason="OOMKilled""#));
        assert!(query.contains("group_left(reason)"));
    }

    #[test]
    fn per_container_aggregation_takes_the_worst_replica() {
        let samples = [
            sample("api", "api-0", 0.2),
            sample("api", "api-1", 0.5),
            sample("sidecar", "api-0", 0.01),
        ];
        let max = max_by_container(&samples);
        assert_eq!(max["api"], 0.5);
        assert_eq!(max["sidecar"], 0.01);
    }

    #[test]
    fn non_finite_samples_do_not_poison_aggregates() {
        let samples = [
            sample("api", "api-0", f64::NAN),
            sample("api", "api-1", 0.3),
        ];
        assert_eq!(max_by_container(&samples)["api"], 0.3);
        assert_eq!(sum_by_container(&samples)["api"], 0.3);
    }

    #[test]
    fn point_counts_sum_across_pods() {
        let samples = [sample("api", "api-0", 60.0), sample("api", "api-1", 50.0)];
        assert_eq!(sum_by_container(&samples)["api"], 110.0);
    }

    #[test]
    fn severity_grades_on_absolute_gap() {
        let cpu = (CPU_CRITICAL, CPU_WARNING, CPU_OK);
        assert_eq!(
            severity(Some(1.0), Some(0.4), cpu),
            AdviceSeverity::Critical
        );
        assert_eq!(severity(Some(1.0), Some(0.7), cpu), AdviceSeverity::Warning);
        assert_eq!(severity(Some(1.0), Some(0.85), cpu), AdviceSeverity::Ok);
        assert_eq!(severity(Some(1.0), Some(0.95), cpu), AdviceSeverity::Good);
    }

    #[test]
    fn unset_requests_are_always_flagged() {
        let cpu = (CPU_CRITICAL, CPU_WARNING, CPU_OK);
        // No request set but usage observed — the case the issue asks to flag.
        assert_eq!(severity(None, Some(0.01), cpu), AdviceSeverity::Warning);
        // Nothing set and nothing recommended is not a problem.
        assert_eq!(severity(None, None, cpu), AdviceSeverity::Good);
    }

    #[test]
    fn memory_severity_uses_byte_thresholds() {
        let mem = (MEM_CRITICAL, MEM_WARNING, MEM_OK);
        let gib = 1024.0 * 1024.0 * 1024.0;
        assert_eq!(
            severity(Some(gib), Some(gib - 1.0), mem),
            AdviceSeverity::Good
        );
        assert_eq!(
            severity(Some(gib), Some(gib - MEM_CRITICAL), mem),
            AdviceSeverity::Critical
        );
    }

    #[test]
    fn worst_axis_wins_for_sorting() {
        let mut severities = [
            AdviceSeverity::Good,
            AdviceSeverity::Critical,
            AdviceSeverity::Ok,
        ];
        severities.sort();
        assert_eq!(severities[2], AdviceSeverity::Critical);
        assert!(AdviceSeverity::Critical > AdviceSeverity::Warning);
    }

    #[test]
    fn oversized_pod_lists_keep_the_most_recent() {
        let workload = WorkloadRef {
            namespace: "prod".to_string(),
            kind: WorkloadKind::CronJob,
            name: "backup".to_string(),
        };
        let pods: Vec<String> = (0..MAX_PODS_IN_SELECTOR + 10)
            .map(|index| format!("backup-{index:05}"))
            .collect();
        let kept = truncate_pods(pods.clone(), &workload);

        assert_eq!(kept.len(), MAX_PODS_IN_SELECTOR);
        assert_eq!(kept.last(), pods.last());
    }
}
