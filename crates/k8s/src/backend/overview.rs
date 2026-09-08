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
use crate::project::{resource_status, ts_string};

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

    async fn rollup(&self, entry: crate::discovery::CatalogEntry) -> ResourceHealthRollup {
        let client = self.client();
        let api: Api<DynamicObject> = Api::all_with(client, &entry.api_resource);
        let result = api
            .list(&ListParams::default())
            .await
            .map(|list| list.items)
            .map_err(|error| error.to_string());
        summarize_resources(entry.kind, result)
    }

    async fn resource_rollups(
        &self,
        category: Category,
        group: Option<&str>,
    ) -> Vec<ResourceHealthRollup> {
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

        join_all(entries.into_iter().map(|entry| self.rollup(entry)))
            .await
            .into_iter()
            .filter(|resource| resource.health.total > 0 || resource.error.is_some())
            .collect()
    }
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

    let status = resource_status(
        &kind.group,
        &kind.kind,
        &object.data,
        object.metadata.deletion_timestamp.is_some(),
    );
    match status {
        roder_core::RowStatus::Ok | roder_core::RowStatus::Done => rollup.ready += 1,
        roder_core::RowStatus::Pending => rollup.reconciling += 1,
        roder_core::RowStatus::Warn => rollup.warning += 1,
        roder_core::RowStatus::Error => rollup.failing += 1,
        roder_core::RowStatus::Unknown => rollup.unknown += 1,
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
    fn rollup_records_unknown_instead_of_treating_it_as_healthy() {
        let api_resource =
            ApiResource::from_gvk(&GroupVersionKind::gvk("example.io", "v1", "Widget"));
        let mut object = DynamicObject::new("widget", &api_resource);
        object.metadata = ObjectMeta::default();
        let kind = resource_kind("example.io", "Widget");
        let mut rollup = HealthRollup::default();

        record_status(&mut rollup, &kind, &object);

        assert_eq!(
            resource_status("example.io", "Widget", &object.data, false),
            RowStatus::Unknown
        );
        assert_eq!(rollup.unknown, 1);
        assert_eq!(rollup.ready, 0);
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
}
