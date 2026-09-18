//! Resource request/limit recommendation payloads.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkloadKind {
    Deployment,
    StatefulSet,
    DaemonSet,
    Job,
    CronJob,
    Pod,
}

impl WorkloadKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Deployment => "Deployment",
            Self::StatefulSet => "StatefulSet",
            Self::DaemonSet => "DaemonSet",
            Self::Job => "Job",
            Self::CronJob => "CronJob",
            Self::Pod => "Pod",
        }
    }
}

/// CPU in cores, memory in bytes. `None` means unset: no request declared, or
/// — for a recommended CPU limit — deliberately none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerResources {
    #[serde(default)]
    pub cpu_request: Option<f64>,
    #[serde(default)]
    pub cpu_limit: Option<f64>,
    #[serde(default)]
    pub memory_request: Option<f64>,
    #[serde(default)]
    pub memory_limit: Option<f64>,
}

/// How far a container's current request is from what its history justifies.
/// Ordered worst-last so a scan sorts with `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdviceSeverity {
    /// No recommendation could be computed.
    Unknown,
    Good,
    Ok,
    Warning,
    Critical,
}

impl AdviceSeverity {
    /// The existing `.sev-*` dot class, sharing the alert severity palette.
    /// `Good` and `Ok` deliberately share a colour: worth distinguishing in the
    /// data, not worth a fourth colour in the table.
    pub fn dot_class(self) -> &'static str {
        match self {
            Self::Critical => "sev-critical",
            Self::Warning => "sev-warning",
            Self::Ok | Self::Good => "sev-info",
            Self::Unknown => "sev-",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Critical => "Critical",
            Self::Warning => "Warning",
            Self::Ok => "Ok",
            Self::Good => "Good",
            Self::Unknown => "Unknown",
        }
    }
}

/// One container's current allocation against what its usage history justifies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceAdvice {
    pub namespace: String,
    pub workload: String,
    pub workload_kind: WorkloadKind,
    pub container: String,
    pub current: ContainerResources,
    pub recommended: ContainerResources,
    pub cpu_severity: AdviceSeverity,
    pub memory_severity: AdviceSeverity,
    /// Why there is no recommendation, when there isn't one.
    #[serde(default)]
    pub info: Option<String>,
}

impl ResourceAdvice {
    /// The worse of the two axes, for sorting a scan worst-first.
    pub fn severity(&self) -> AdviceSeverity {
        self.cpu_severity.max(self.memory_severity)
    }

    /// Whether this container has no CPU or memory request at all — the case
    /// worth surfacing regardless of how close the numbers are.
    pub fn missing_requests(&self) -> bool {
        self.current.cpu_request.is_none() || self.current.memory_request.is_none()
    }
}

/// The result of one scan, with the settings it ran under so the numbers can be
/// read in context.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceScan {
    pub rows: Vec<ResourceAdvice>,
    /// Length of the history window, in hours.
    pub history_hours: f64,
    /// The percentile of CPU usage that became the CPU request.
    pub cpu_percentile: f64,
    pub scanned_workloads: usize,
    /// Workloads whose Prometheus queries failed, with the first error.
    #[serde(default)]
    pub failures: Vec<ScanFailure>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanFailure {
    pub namespace: String,
    pub workload: String,
    pub error: String,
}

/// Cluster-level rollup of a scan.
///
/// Only rows with both a current and a recommended value contribute: counting
/// an unset request as "reclaiming" its recommendation would invert the sign.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanTotals {
    /// CPU cores currently requested, across comparable containers.
    pub cpu_current: f64,
    pub cpu_recommended: f64,
    /// Memory bytes currently requested, across comparable containers.
    pub memory_current: f64,
    pub memory_recommended: f64,
    pub critical: usize,
    pub warning: usize,
    pub ok: usize,
    pub good: usize,
    pub unknown: usize,
    /// Containers with no CPU or memory request declared.
    pub unset: usize,
    /// Limits the scan advises removing — KRR never recommends a CPU limit.
    pub droppable_cpu_limits: usize,
}

impl ScanTotals {
    /// Cores freed by applying every recommendation. Negative means the cluster
    /// is under-requesting and would need more.
    pub fn cpu_delta(&self) -> f64 {
        self.cpu_current - self.cpu_recommended
    }

    /// Bytes freed by applying every recommendation.
    pub fn memory_delta(&self) -> f64 {
        self.memory_current - self.memory_recommended
    }
}

impl ResourceScan {
    pub fn totals(&self) -> ScanTotals {
        let mut totals = ScanTotals::default();
        for row in &self.rows {
            match row.severity() {
                AdviceSeverity::Critical => totals.critical += 1,
                AdviceSeverity::Warning => totals.warning += 1,
                AdviceSeverity::Ok => totals.ok += 1,
                AdviceSeverity::Good => totals.good += 1,
                AdviceSeverity::Unknown => totals.unknown += 1,
            }
            if row.missing_requests() {
                totals.unset += 1;
            }
            if row.current.cpu_limit.is_some() && row.recommended.cpu_limit.is_none() {
                totals.droppable_cpu_limits += 1;
            }
            if let (Some(current), Some(recommended)) =
                (row.current.cpu_request, row.recommended.cpu_request)
            {
                totals.cpu_current += current;
                totals.cpu_recommended += recommended;
            }
            if let (Some(current), Some(recommended)) =
                (row.current.memory_request, row.recommended.memory_request)
            {
                totals.memory_current += current;
                totals.memory_recommended += recommended;
            }
        }
        totals
    }
}

/// Sort worst-first, then by namespace and name so equal severities stay in a
/// stable, readable order.
pub fn sort_advice(rows: &mut [ResourceAdvice]) {
    rows.sort_by(|left, right| {
        right
            .severity()
            .cmp(&left.severity())
            .then_with(|| left.namespace.cmp(&right.namespace))
            .then_with(|| left.workload.cmp(&right.workload))
            .then_with(|| left.container.cmp(&right.container))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advice(cpu: AdviceSeverity, memory: AdviceSeverity, workload: &str) -> ResourceAdvice {
        ResourceAdvice {
            namespace: "prod".to_string(),
            workload: workload.to_string(),
            workload_kind: WorkloadKind::Deployment,
            container: "app".to_string(),
            current: ContainerResources::default(),
            recommended: ContainerResources::default(),
            cpu_severity: cpu,
            memory_severity: memory,
            info: None,
        }
    }

    #[test]
    fn worst_axis_decides_the_row() {
        let row = advice(AdviceSeverity::Good, AdviceSeverity::Critical, "web");
        assert_eq!(row.severity(), AdviceSeverity::Critical);
    }

    #[test]
    fn scans_sort_worst_first_then_alphabetically() {
        let mut rows = vec![
            advice(AdviceSeverity::Good, AdviceSeverity::Good, "zebra"),
            advice(AdviceSeverity::Critical, AdviceSeverity::Good, "web"),
            advice(AdviceSeverity::Good, AdviceSeverity::Good, "api"),
        ];
        sort_advice(&mut rows);

        assert_eq!(rows[0].workload, "web");
        assert_eq!(rows[1].workload, "api");
        assert_eq!(rows[2].workload, "zebra");
    }

    #[test]
    fn unset_requests_are_detectable_independently_of_severity() {
        let mut row = advice(AdviceSeverity::Good, AdviceSeverity::Good, "web");
        assert!(row.missing_requests());
        row.current.cpu_request = Some(0.1);
        assert!(row.missing_requests());
        row.current.memory_request = Some(1.0);
        assert!(!row.missing_requests());
    }

    fn scan(rows: Vec<ResourceAdvice>) -> ResourceScan {
        ResourceScan {
            rows,
            history_hours: 336.0,
            cpu_percentile: 95.0,
            scanned_workloads: 1,
            failures: Vec::new(),
        }
    }

    #[test]
    fn totals_sum_only_comparable_rows() {
        let mut sized = advice(AdviceSeverity::Critical, AdviceSeverity::Good, "web");
        sized.current = ContainerResources {
            cpu_request: Some(1.0),
            memory_request: Some(1000.0),
            ..ContainerResources::default()
        };
        sized.recommended = ContainerResources {
            cpu_request: Some(0.25),
            memory_request: Some(400.0),
            ..ContainerResources::default()
        };
        // No current request, so there is nothing to compare and nothing to add.
        let mut unset = advice(AdviceSeverity::Warning, AdviceSeverity::Warning, "api");
        unset.recommended = ContainerResources {
            cpu_request: Some(5.0),
            ..ContainerResources::default()
        };

        let totals = scan(vec![sized, unset]).totals();
        assert_eq!(totals.cpu_current, 1.0);
        assert_eq!(totals.cpu_recommended, 0.25);
        assert_eq!(totals.cpu_delta(), 0.75);
        assert_eq!(totals.memory_delta(), 600.0);
        assert_eq!(totals.critical, 1);
        assert_eq!(totals.warning, 1);
        assert_eq!(totals.unset, 1);
    }

    #[test]
    fn under_requested_clusters_report_a_negative_delta() {
        let mut row = advice(AdviceSeverity::Critical, AdviceSeverity::Good, "web");
        row.current.cpu_request = Some(0.1);
        row.recommended.cpu_request = Some(0.9);

        assert_eq!(scan(vec![row]).totals().cpu_delta(), -0.8);
    }

    #[test]
    fn a_cpu_limit_the_scan_wants_removed_is_counted() {
        let mut row = advice(AdviceSeverity::Good, AdviceSeverity::Good, "web");
        row.current.cpu_limit = Some(2.0);
        // KRR never recommends a CPU limit, so `None` here means "drop it".
        row.recommended.cpu_limit = None;

        assert_eq!(scan(vec![row]).totals().droppable_cpu_limits, 1);
    }

    #[test]
    fn severities_reuse_the_existing_alert_palette() {
        assert_eq!(AdviceSeverity::Critical.dot_class(), "sev-critical");
        assert_eq!(AdviceSeverity::Warning.dot_class(), "sev-warning");
        assert_eq!(AdviceSeverity::Good.dot_class(), "sev-info");
        assert_eq!(AdviceSeverity::Unknown.dot_class(), "sev-");
    }
}
