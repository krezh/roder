//! Dashboard overview: node/pod/namespace counts, recent warning events, and
//! controller health rollups, cached briefly so frequent page (re)connects reuse
//! one snapshot instead of each re-listing the cluster.

use std::collections::BTreeMap;

use futures::future::join_all;
use k8s_openapi::api::core::v1::{Event, Namespace, Node, Pod};
use kube::api::{Api, DynamicObject, ListParams};
use roder_core::{
    Category, ClusterOverview, ControllerHealthGroup, ControllerHealthSignal, HealthRollup,
    NodeSummary, OverviewWarning, ResourceHealthRollup, RowStatus,
};

use crate::metrics::{node_usage, parse_cpu, parse_mem};
use crate::project::{condition, parse_timestamp, resource_status, ts_string};

use super::{api_err, Backend};
use crate::client::K8sError;

impl Backend {
    /// Dashboard overview. Served from a short-lived cache so frequent page
    /// (re)connects reuse one snapshot instead of each re-listing the cluster.
    pub async fn overview(&self) -> Result<ClusterOverview, K8sError> {
        const TTL: std::time::Duration = std::time::Duration::from_secs(8);
        {
            let cache = self.overview_cache.read().await;
            if let Some((at, ov)) = cache.as_ref() {
                if at.elapsed() < TTL {
                    return Ok(ov.clone());
                }
            }
        }

        let _refresh = self.overview_refresh.lock().await;
        {
            let cache = self.overview_cache.read().await;
            if let Some((at, ov)) = cache.as_ref() {
                if at.elapsed() < TTL {
                    return Ok(ov.clone());
                }
            }
        }

        let fresh = self.compute_overview().await?;
        *self.overview_cache.write().await = Some((std::time::Instant::now(), fresh.clone()));
        Ok(fresh)
    }

    /// Dashboard overview, computed from a handful of list calls + metrics-server.
    async fn compute_overview(&self) -> Result<ClusterOverview, K8sError> {
        let client = self.client();
        let kubernetes_version = self.cluster.probe().await.unwrap_or_default();

        // Nodes + (best-effort) usage.
        let usage = node_usage(&client).await;
        let node_list = Api::<Node>::all(client.clone())
            .list(&ListParams::default())
            .await
            .map_err(api_err)?;
        let nodes = node_list
            .items
            .iter()
            .map(|n| {
                let name = n.metadata.name.clone().unwrap_or_default();
                let ready = n
                    .status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .map(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
                    .unwrap_or(false);
                let cap = n.status.as_ref().and_then(|s| s.capacity.as_ref());
                let cpu_cores = cap.and_then(|c| c.get("cpu")).map(|q| parse_cpu(&q.0));
                let mem_bytes = cap.and_then(|c| c.get("memory")).map(|q| parse_mem(&q.0));
                let (cpu_used, mem_used) = match usage.get(&name) {
                    Some((c, m)) => (Some(*c), Some(*m)),
                    None => (None, None),
                };
                let info = n.status.as_ref().and_then(|s| s.node_info.as_ref());
                let kubelet_version = info
                    .map(|i| i.kubelet_version.clone())
                    .filter(|s| !s.is_empty());
                let os_image = info.map(|i| i.os_image.clone()).filter(|s| !s.is_empty());
                NodeSummary {
                    name,
                    ready,
                    cpu_cores,
                    cpu_used,
                    mem_bytes,
                    mem_used,
                    kubelet_version,
                    os_image,
                }
            })
            .collect();

        // Pods by phase.
        let pods = Api::<Pod>::all(client.clone())
            .list(&ListParams::default())
            .await
            .map_err(api_err)?;
        let (mut pod_running, mut pod_pending, mut pod_failed) = (0u32, 0u32, 0u32);
        for p in &pods.items {
            match p.status.as_ref().and_then(|s| s.phase.as_deref()) {
                Some("Running") => pod_running += 1,
                Some("Pending") => pod_pending += 1,
                Some("Failed") => pod_failed += 1,
                _ => {}
            }
        }
        let pod_total = pods.items.len() as u32;

        // Namespaces.
        let namespace_count = Api::<Namespace>::all(client.clone())
            .list(&ListParams::default())
            .await
            .map(|l| l.items.len() as u32)
            .unwrap_or(0);

        // Recent warning events.
        let warnings = self.recent_warnings().await.unwrap_or_default();

        let groups = [
            ("Flux", Category::Flux, None),
            ("External Secrets", Category::ExternalSecrets, None),
            ("cert-manager", Category::CertManager, None),
            ("Rook Ceph", Category::Rook, None),
            (
                "Kopiur",
                Category::Custom("home-operations.com".to_string()),
                Some("kopiur.home-operations.com"),
            ),
            (
                "Tuppr",
                Category::Custom("home-operations.com".to_string()),
                Some("tuppr.home-operations.com"),
            ),
        ];
        let (mut controller_groups, cnpg_group, prometheus_group) = tokio::join!(
            join_all(
                groups
                    .into_iter()
                    .map(|(name, category, group)| async move {
                        ControllerHealthGroup {
                            name: name.to_string(),
                            resources: self.resource_rollups(category, group).await,
                            signals: Vec::new(),
                        }
                    })
            ),
            self.cnpg_group(),
            self.prometheus_group(),
        );
        controller_groups.insert(4, cnpg_group);
        controller_groups.insert(5, prometheus_group);
        controller_groups.retain(|group| !group.resources.is_empty() || !group.signals.is_empty());

        Ok(ClusterOverview {
            kubernetes_version,
            nodes,
            namespace_count,
            pod_total,
            pod_running,
            pod_pending,
            pod_failed,
            warnings,
            controller_groups,
        })
    }

    async fn recent_warnings(&self) -> Result<Vec<OverviewWarning>, K8sError> {
        let client = self.client();
        let lp = ListParams::default().fields("type=Warning");
        let list = Api::<Event>::all(client).list(&lp).await.map_err(api_err)?;
        let mut events: Vec<OverviewWarning> = list
            .items
            .into_iter()
            .map(|e| {
                let timestamp = e
                    .series
                    .as_ref()
                    .and_then(|series| series.last_observed_time.as_ref())
                    .and_then(ts_string)
                    .or_else(|| e.event_time.as_ref().and_then(ts_string))
                    .or_else(|| e.last_timestamp.as_ref().and_then(ts_string));
                let count = e
                    .series
                    .as_ref()
                    .and_then(|series| series.count)
                    .or(e.count)
                    .unwrap_or(1)
                    .max(1) as u32;
                let source = e
                    .reporting_component
                    .or_else(|| e.source.and_then(|source| source.component))
                    .unwrap_or_default();
                OverviewWarning {
                    event_name: e.metadata.name.unwrap_or_default(),
                    namespace: e.metadata.namespace.or(e.involved_object.namespace),
                    involved_kind: e.involved_object.kind.unwrap_or_default(),
                    involved_name: e.involved_object.name.unwrap_or_default(),
                    reason: e.reason.unwrap_or_default(),
                    message: e.message.unwrap_or_default(),
                    source,
                    timestamp,
                    count,
                }
            })
            .collect();
        events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        events.truncate(8);
        Ok(events)
    }

    async fn resource_snapshot(
        &self,
        entry: crate::discovery::CatalogEntry,
    ) -> (roder_core::ResourceKind, Result<Vec<DynamicObject>, String>) {
        let client = self.client();
        let api: Api<DynamicObject> = Api::all_with(client, &entry.api_resource);
        let result = api
            .list(&ListParams::default())
            .await
            .map(|list| list.items)
            .map_err(|error| error.to_string());
        (entry.kind, result)
    }

    async fn resource_snapshots(
        &self,
        category: Category,
        group: Option<&str>,
    ) -> Vec<(roder_core::ResourceKind, Result<Vec<DynamicObject>, String>)> {
        let catalog_store = self.shared.catalog();
        let catalog = catalog_store.load();
        let entries = catalog
            .entries
            .iter()
            .filter(|entry| {
                entry.kind.category == category
                    && group.is_none_or(|group| entry.kind.group == group)
            })
            .cloned()
            .collect::<Vec<_>>();

        join_all(
            entries
                .into_iter()
                .map(|entry| self.resource_snapshot(entry)),
        )
        .await
    }

    async fn resource_rollups(
        &self,
        category: Category,
        group: Option<&str>,
    ) -> Vec<ResourceHealthRollup> {
        self.resource_snapshots(category, group)
            .await
            .into_iter()
            .map(|(kind, objects)| summarize_resources(kind, objects))
            .filter(|resource| resource.health.total > 0 || resource.error.is_some())
            .collect()
    }

    async fn cnpg_group(&self) -> ControllerHealthGroup {
        let snapshots = self.resource_snapshots(Category::CloudNativePg, None).await;
        let signals = summarize_cnpg_signals(
            snapshot_objects(&snapshots, "Cluster"),
            snapshot_objects(&snapshots, "Backup"),
            snapshot_objects(&snapshots, "ScheduledBackup"),
        );
        let resources = snapshots
            .into_iter()
            .map(|(kind, objects)| summarize_resources(kind, objects))
            .filter(|resource| resource.health.total > 0 || resource.error.is_some())
            .collect();
        ControllerHealthGroup {
            name: "CloudNativePG".to_string(),
            resources,
            signals,
        }
    }

    async fn prometheus_group(&self) -> ControllerHealthGroup {
        let catalog_store = self.shared.catalog();
        let catalog = catalog_store.load();
        let entries = catalog
            .entries
            .iter()
            .filter(|entry| {
                entry.kind.group == "monitoring.coreos.com"
                    && matches!(
                        entry.kind.kind.as_str(),
                        "Prometheus" | "PrometheusAgent" | "Alertmanager" | "ThanosRuler"
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        let resources = join_all(
            entries
                .into_iter()
                .map(|entry| self.resource_snapshot(entry)),
        )
        .await
        .into_iter()
        .map(|(kind, objects)| summarize_resources(kind, objects))
        .filter(|resource| resource.health.total > 0 || resource.error.is_some())
        .collect();
        ControllerHealthGroup {
            name: "Prometheus Operator".to_string(),
            resources,
            signals: Vec::new(),
        }
    }
}

fn snapshot_objects<'a>(
    snapshots: &'a [(roder_core::ResourceKind, Result<Vec<DynamicObject>, String>)],
    kind: &str,
) -> &'a [DynamicObject] {
    snapshots
        .iter()
        .find(|(resource, _)| resource.kind == kind)
        .and_then(|(_, objects)| objects.as_ref().ok())
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn summarize_cnpg_signals(
    clusters: &[DynamicObject],
    backups: &[DynamicObject],
    schedules: &[DynamicObject],
) -> Vec<ControllerHealthSignal> {
    let mut latest_backups = BTreeMap::<(String, String), (time::OffsetDateTime, String)>::new();
    for backup in backups {
        let phase = backup
            .data
            .pointer("/status/phase")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !matches!(
            phase.to_ascii_lowercase().as_str(),
            "completed" | "succeeded"
        ) {
            continue;
        }
        let Some(cluster) = backup
            .data
            .pointer("/spec/cluster/name")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(timestamp) = backup
            .data
            .pointer("/status/stoppedAt")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(parsed) = parse_timestamp(timestamp) else {
            continue;
        };
        let key = (
            backup.metadata.namespace.clone().unwrap_or_default(),
            cluster.to_string(),
        );
        if latest_backups
            .get(&key)
            .is_none_or(|(current, _)| parsed > *current)
        {
            latest_backups.insert(key, (parsed, timestamp.to_string()));
        }
    }

    let mut latest_schedules = BTreeMap::<(String, String), time::OffsetDateTime>::new();
    for schedule in schedules {
        let Some(cluster) = schedule
            .data
            .pointer("/spec/cluster/name")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(timestamp) = schedule
            .data
            .pointer("/status/lastScheduleTime")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_timestamp)
        else {
            continue;
        };
        let key = (
            schedule.metadata.namespace.clone().unwrap_or_default(),
            cluster.to_string(),
        );
        latest_schedules
            .entry(key)
            .and_modify(|current| *current = (*current).max(timestamp))
            .or_insert(timestamp);
    }

    let mut signals = Vec::new();
    for cluster in clusters {
        let namespace = cluster.metadata.namespace.clone().unwrap_or_default();
        let name = cluster.metadata.name.clone().unwrap_or_default();
        let key = (namespace, name.clone());
        let cluster_data = serde_json::to_value(cluster).unwrap_or_else(|_| cluster.data.clone());
        let archival = condition(&cluster_data, "ContinuousArchiving");
        if let Some(archival) = archival {
            let archival_status = match archival.get("status").and_then(serde_json::Value::as_str) {
                Some("True") => RowStatus::Ok,
                Some("False") => RowStatus::Error,
                Some(_) | None => RowStatus::Pending,
            };
            let archival_message = archival
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            signals.push(ControllerHealthSignal {
                label: format!("{name} archival"),
                value: match archival_status {
                    RowStatus::Ok => "Healthy",
                    RowStatus::Error => "Failing",
                    _ => "Pending",
                }
                .to_string(),
                status: archival_status,
                timestamp: None,
                message: archival_message,
            });
        }

        let backup = latest_backups.get(&key);
        let scheduled = latest_schedules.get(&key);
        if backup.is_none() && scheduled.is_none() {
            continue;
        }
        let status = match (backup, scheduled) {
            (None, Some(_)) => RowStatus::Error,
            (Some((completed, _)), Some(last_run)) if completed < last_run => RowStatus::Warn,
            (Some(_), _) => RowStatus::Ok,
            (None, None) => unreachable!("unconfigured backup signals are skipped"),
        };
        let message = match (backup, scheduled, status) {
            (None, Some(_), _) => "No successful backup after a scheduled run",
            (None, None, _) => unreachable!("unconfigured backup signals are skipped"),
            (Some(_), Some(_), RowStatus::Warn) => {
                "The latest scheduled run has no completed backup"
            }
            _ => "Latest successful recovery point",
        };
        signals.push(ControllerHealthSignal {
            label: format!("{name} backup RPO"),
            value: backup
                .map(|_| "Recovery point age")
                .unwrap_or("No recovery point")
                .to_string(),
            status,
            timestamp: backup.map(|(_, timestamp)| timestamp.clone()),
            message: message.to_string(),
        });
    }
    signals.sort_by(|left, right| left.label.cmp(&right.label));
    signals
}

fn summarize_resources(
    kind: roder_core::ResourceKind,
    result: Result<Vec<DynamicObject>, String>,
) -> ResourceHealthRollup {
    let mut health = HealthRollup::default();
    let error = match result {
        Ok(objects) => {
            for object in objects {
                record_status(&mut health, &kind, &object);
            }
            None
        }
        Err(error) => Some(error),
    };
    ResourceHealthRollup {
        key: kind.key,
        kind: kind.kind,
        health,
        error,
    }
}

fn record_status(
    rollup: &mut HealthRollup,
    kind: &roder_core::ResourceKind,
    object: &DynamicObject,
) {
    rollup.total += 1;
    let suspended = object
        .data
        .pointer("/spec/suspend")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if suspended {
        rollup.suspended += 1;
        return;
    }

    let status = resource_status(&kind.group, &kind.kind, object);
    match status {
        roder_core::RowStatus::Ok | roder_core::RowStatus::Done => rollup.ready += 1,
        roder_core::RowStatus::Pending => rollup.reconciling += 1,
        roder_core::RowStatus::Warn => rollup.warning += 1,
        roder_core::RowStatus::Error => rollup.failing += 1,
        roder_core::RowStatus::Unknown if status_is_reported(object) => rollup.unknown += 1,
        roder_core::RowStatus::Unknown => rollup.unreported += 1,
    }
}

fn status_is_reported(object: &DynamicObject) -> bool {
    match object.data.get("status") {
        Some(serde_json::Value::Object(status)) => !status.is_empty(),
        Some(serde_json::Value::Null) | None => false,
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::api::ObjectMeta;
    use kube::core::{ApiResource, GroupVersionKind};
    use roder_core::{Category, ResourceKind, RowStatus};

    fn resource_kind(group: &str, kind: &str) -> ResourceKind {
        ResourceKind {
            key: ResourceKind::make_key(group, "v1", kind),
            group: group.into(),
            version: "v1".into(),
            kind: kind.into(),
            plural: format!("{}s", kind.to_lowercase()),
            namespaced: true,
            category: Category::Custom(group.into()),
        }
    }

    fn object(group: &str, kind: &str, name: &str, data: serde_json::Value) -> DynamicObject {
        let api_resource = ApiResource::from_gvk(&GroupVersionKind::gvk(group, "v1", kind));
        let mut object = DynamicObject::new(name, &api_resource);
        object.metadata.namespace = Some("database".into());
        object.data = data;
        object
    }

    #[test]
    fn rollup_uses_the_row_projector_status() {
        let api_resource = ApiResource::from_gvk(&GroupVersionKind::gvk(
            "kustomize.toolkit.fluxcd.io",
            "v1",
            "Kustomization",
        ));
        let mut object = DynamicObject::new("release", &api_resource);
        object.data = serde_json::json!({
            "status": { "conditions": [
                { "type": "Ready", "status": "False" },
                { "type": "Reconciling", "status": "True" }
            ]}
        });
        let kind = ResourceKind {
            key: "kustomize.toolkit.fluxcd.io/v1/Kustomization".into(),
            group: "kustomize.toolkit.fluxcd.io".into(),
            version: "v1".into(),
            kind: "Kustomization".into(),
            plural: "kustomizations".into(),
            namespaced: true,
            category: Category::Flux,
        };
        let mut rollup = HealthRollup::default();

        record_status(&mut rollup, &kind, &object);

        assert_eq!(rollup.failing, 1);
        assert_eq!(rollup.reconciling, 0);
    }

    #[test]
    fn rollup_records_missing_status_as_unreported() {
        let api_resource =
            ApiResource::from_gvk(&GroupVersionKind::gvk("example.io", "v1", "Widget"));
        let mut object = DynamicObject::new("widget", &api_resource);
        object.metadata = ObjectMeta::default();
        let kind = resource_kind("example.io", "Widget");
        let mut rollup = HealthRollup::default();

        record_status(&mut rollup, &kind, &object);

        assert_eq!(
            resource_status("example.io", "Widget", &object),
            RowStatus::Unknown
        );
        assert_eq!(rollup.unreported, 1);
        assert_eq!(rollup.unknown, 0);
        assert_eq!(rollup.ready, 0);
    }

    #[test]
    fn rollup_keeps_an_unrecognized_reported_status_as_unknown() {
        let api_resource =
            ApiResource::from_gvk(&GroupVersionKind::gvk("example.io", "v1", "Widget"));
        let mut object = DynamicObject::new("widget", &api_resource);
        object.data = serde_json::json!({"status": {"phase": "Unexpected"}});
        let kind = resource_kind("example.io", "Widget");
        let mut rollup = HealthRollup::default();

        record_status(&mut rollup, &kind, &object);

        assert_eq!(rollup.unknown, 1);
        assert_eq!(rollup.unreported, 0);
    }

    #[test]
    fn rollup_recognizes_synced_conditions_and_applied_flags() {
        let cases = [
            (
                resource_kind("trust.cert-manager.io", "Bundle"),
                object(
                    "trust.cert-manager.io",
                    "Bundle",
                    "roots",
                    serde_json::json!({"status": {"conditions": [{
                        "type": "Synced",
                        "status": "True"
                    }]}}),
                ),
            ),
            (
                resource_kind("postgresql.cnpg.io", "Database"),
                object(
                    "postgresql.cnpg.io",
                    "Database",
                    "app",
                    serde_json::json!({"status": {"applied": true}}),
                ),
            ),
        ];

        for (kind, object) in cases {
            let mut rollup = HealthRollup::default();
            record_status(&mut rollup, &kind, &object);
            assert_eq!(rollup.ready, 1, "{}", kind.kind);
            assert_eq!(rollup.unknown, 0, "{}", kind.kind);
            assert_eq!(rollup.unreported, 0, "{}", kind.kind);
        }
    }

    #[test]
    fn unreadable_rollup_keeps_its_canonical_resource_key_and_error() {
        let rollup = summarize_resources(
            resource_kind("example.io", "Widget"),
            Err("403 Forbidden".into()),
        );

        assert_eq!(rollup.key, "example.io/v1/Widget");
        assert_eq!(rollup.health.total, 0);
        assert_eq!(rollup.error.as_deref(), Some("403 Forbidden"));
    }

    #[test]
    fn prometheus_rollups_use_readiness_semantics() {
        let kind = resource_kind("monitoring.coreos.com", "Prometheus");
        let objects = vec![
            object(
                "monitoring.coreos.com",
                "Prometheus",
                "ready",
                serde_json::json!({
                    "spec": {"replicas": 2},
                    "status": {"availableReplicas": 2, "conditions": [
                        {"type": "Reconciled", "status": "True"},
                        {"type": "Available", "status": "True"}
                    ]}
                }),
            ),
            object(
                "monitoring.coreos.com",
                "Prometheus",
                "degraded",
                serde_json::json!({
                    "status": {"availableReplicas": 1, "conditions": [
                        {"type": "Available", "status": "Degraded"}
                    ]}
                }),
            ),
            object(
                "monitoring.coreos.com",
                "Prometheus",
                "failed",
                serde_json::json!({
                    "status": {"conditions": [
                        {"type": "Reconciled", "status": "False"}
                    ]}
                }),
            ),
            object(
                "monitoring.coreos.com",
                "Prometheus",
                "unreported",
                serde_json::json!({}),
            ),
        ];

        let rollup = summarize_resources(kind, Ok(objects));
        assert_eq!(rollup.health.ready, 1);
        assert_eq!(rollup.health.warning, 1);
        assert_eq!(rollup.health.failing, 1);
        assert_eq!(rollup.health.unreported, 1);
    }

    #[test]
    fn cnpg_signals_report_archival_failure_and_stale_recovery_point() {
        let clusters = [object(
            "postgresql.cnpg.io",
            "Cluster",
            "app",
            serde_json::json!({"status": {"conditions": [{
                "type": "ContinuousArchiving",
                "status": "False",
                "message": "WAL upload failed"
            }]}}),
        )];
        let backups = [object(
            "postgresql.cnpg.io",
            "Backup",
            "older",
            serde_json::json!({
                "spec": {"cluster": {"name": "app"}},
                "status": {"phase": "completed", "stoppedAt": "2026-09-01T02:00:00Z"}
            }),
        )];
        let schedules = [object(
            "postgresql.cnpg.io",
            "ScheduledBackup",
            "daily",
            serde_json::json!({
                "spec": {"cluster": {"name": "app"}},
                "status": {"lastScheduleTime": "2026-09-02T02:00:00Z"}
            }),
        )];

        let signals = summarize_cnpg_signals(&clusters, &backups, &schedules);

        assert_eq!(signals.len(), 2);
        assert_eq!(signals[0].label, "app archival");
        assert_eq!(signals[0].status, RowStatus::Error);
        assert_eq!(signals[0].message, "WAL upload failed");
        assert_eq!(signals[1].label, "app backup RPO");
        assert_eq!(signals[1].status, RowStatus::Warn);
        assert_eq!(
            signals[1].timestamp.as_deref(),
            Some("2026-09-01T02:00:00Z")
        );
    }

    #[test]
    fn cnpg_signal_fails_when_a_schedule_has_never_produced_a_backup() {
        let clusters = [object(
            "postgresql.cnpg.io",
            "Cluster",
            "app",
            serde_json::json!({}),
        )];
        let schedules = [object(
            "postgresql.cnpg.io",
            "ScheduledBackup",
            "daily",
            serde_json::json!({
                "spec": {"cluster": {"name": "app"}},
                "status": {"lastScheduleTime": "2026-09-02T02:00:00Z"}
            }),
        )];

        let signals = summarize_cnpg_signals(&clusters, &[], &schedules);

        assert_eq!(signals[0].status, RowStatus::Error);
        assert_eq!(signals[0].value, "No recovery point");
    }
}
