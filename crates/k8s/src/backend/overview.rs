//! Dashboard overview: node/pod/namespace counts, recent warning events, and
//! Flux/ESO health rollups, cached briefly so frequent page (re)connects reuse
//! one snapshot instead of each re-listing the cluster.

use futures::future::join_all;
use k8s_openapi::api::core::v1::{Event, Namespace, Node, Pod};
use kube::api::{Api, DynamicObject, ListParams};
use roder_core::{
    Category, ClusterOverview, HealthRollup, NodeSummary, OverviewWarning, ResourceHealthRollup,
};

use crate::metrics::{node_usage, parse_cpu, parse_mem};
use crate::project::ts_string;

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

        let (flux_resources, external_secret_resources, kopiur_resources, tuppr_resources) = tokio::join!(
            self.resource_rollups(Category::Flux, None),
            self.resource_rollups(Category::ExternalSecrets, None),
            self.resource_rollups(
                Category::Custom("home-operations.com".to_string()),
                Some("kopiur.home-operations.com"),
            ),
            self.resource_rollups(
                Category::Custom("home-operations.com".to_string()),
                Some("tuppr.home-operations.com"),
            ),
        );

        Ok(ClusterOverview {
            kubernetes_version,
            nodes,
            namespace_count,
            pod_total,
            pod_running,
            pod_pending,
            pod_failed,
            warnings,
            flux_resources,
            external_secret_resources,
            kopiur_resources,
            tuppr_resources,
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

    /// Count reconciliation states for every kind in a CRD family.
    /// Lists all matching kinds concurrently (instead of sequentially) so the
    /// dashboard overview doesn't accumulate per-kind latency.
    async fn rollup(
        &self,
        category: Category,
        group: Option<&str>,
        kind: Option<&str>,
    ) -> HealthRollup {
        let client = self.client();
        let catalog_store = self.shared.catalog();
        let catalog = catalog_store.load();
        let futs = catalog
            .entries
            .iter()
            .filter(|entry| {
                entry.kind.category == category
                    && group.is_none_or(|group| entry.kind.group == group)
                    && kind.is_none_or(|kind| entry.kind.kind == kind)
            })
            .map(|entry| {
                let api: Api<DynamicObject> = Api::all_with(client.clone(), &entry.api_resource);
                async move {
                    let Ok(list) = api.list(&ListParams::default()).await else {
                        return Vec::new();
                    };
                    list.items
                }
            });
        let results = join_all(futs).await;
        let mut rollup = HealthRollup::default();
        for items in results {
            for obj in items {
                rollup.total += 1;
                if obj
                    .data
                    .get("status")
                    .and_then(|status| status.as_object())
                    .is_some_and(|status| !status.is_empty())
                {
                    rollup.with_status += 1;
                }
                let suspended = obj
                    .data
                    .get("spec")
                    .and_then(|s| s.get("suspend"))
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                if suspended {
                    rollup.suspended += 1;
                    continue;
                }
                match reconciliation_state(&obj.data) {
                    ReconciliationState::Ready => rollup.ready += 1,
                    ReconciliationState::Reconciling => rollup.reconciling += 1,
                    ReconciliationState::Failing => rollup.failing += 1,
                    ReconciliationState::Unknown => {}
                }
            }
        }
        rollup
    }

    async fn resource_rollups(
        &self,
        category: Category,
        group: Option<&str>,
    ) -> Vec<ResourceHealthRollup> {
        let catalog_store = self.shared.catalog();
        let catalog = catalog_store.load();
        let mut kinds: Vec<String> = catalog
            .entries
            .iter()
            .filter(|entry| {
                entry.kind.category == category
                    && group.is_none_or(|group| entry.kind.group == group)
            })
            .map(|entry| entry.kind.kind.clone())
            .collect();
        kinds.sort();
        kinds.dedup();

        join_all(kinds.into_iter().map(|kind| {
            let category = category.clone();
            async move {
                let health = self.rollup(category, group, Some(&kind)).await;
                ResourceHealthRollup { kind, health }
            }
        }))
        .await
        .into_iter()
        .filter(|resource| resource.health.with_status > 0)
        .collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReconciliationState {
    Ready,
    Reconciling,
    Failing,
    Unknown,
}

fn reconciliation_state(data: &serde_json::Value) -> ReconciliationState {
    if condition_is_true(data, "Stalled") {
        return ReconciliationState::Failing;
    }
    if condition_is_true(data, "Reconciling") {
        return ReconciliationState::Reconciling;
    }
    match ready_condition(data) {
        Some(true) => ReconciliationState::Ready,
        Some(false) => ReconciliationState::Failing,
        None => ReconciliationState::Unknown,
    }
}

fn condition_is_true(data: &serde_json::Value, condition_type: &str) -> bool {
    data.get("status")
        .and_then(|status| status.get("conditions"))
        .and_then(|conditions| conditions.as_array())
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition["type"] == condition_type && condition["status"] == "True"
            })
        })
}

fn ready_condition(data: &serde_json::Value) -> Option<bool> {
    let conds = data.get("status")?.get("conditions")?.as_array()?;
    let ready = conds.iter().find(|c| c["type"] == "Ready")?;
    match ready["status"].as_str()? {
        "True" => Some(true),
        "False" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconciling_is_not_classified_as_failed() {
        let data = serde_json::json!({
            "status": { "conditions": [
                { "type": "Ready", "status": "False" },
                { "type": "Reconciling", "status": "True" }
            ]}
        });

        assert_eq!(
            reconciliation_state(&data),
            ReconciliationState::Reconciling
        );
    }

    #[test]
    fn stalled_takes_precedence_over_reconciling() {
        let data = serde_json::json!({
            "status": { "conditions": [
                { "type": "Ready", "status": "False" },
                { "type": "Reconciling", "status": "True" },
                { "type": "Stalled", "status": "True" }
            ]}
        });

        assert_eq!(reconciliation_state(&data), ReconciliationState::Failing);
    }
}
