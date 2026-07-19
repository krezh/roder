//! Dashboard overview: node/pod/namespace counts, recent warning events, and
//! Flux/ESO health rollups, cached briefly so frequent page (re)connects reuse
//! one snapshot instead of each re-listing the cluster.

use futures::future::join_all;
use k8s_openapi::api::core::v1::{Event, Namespace, Node, Pod};
use kube::api::{Api, DynamicObject, ListParams};
use roder_core::{Category, ClusterOverview, HealthRollup, NodeSummary};

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
        let fresh = self.compute_overview().await?;
        let mut cache = self.overview_cache.write().await;
        // Re-check under the write lock: another concurrent caller may have
        // already populated a fresh entry while we were computing.
        if cache
            .as_ref()
            .map(|(at, _)| at.elapsed() >= TTL)
            .unwrap_or(true)
        {
            *cache = Some((std::time::Instant::now(), fresh.clone()));
        }
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

        let (flux, external_secrets) = tokio::join!(
            self.rollup(Category::Flux),
            self.rollup(Category::ExternalSecrets),
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
            flux,
            external_secrets,
        })
    }

    async fn recent_warnings(&self) -> Result<Vec<String>, K8sError> {
        let client = self.client();
        let lp = ListParams::default().fields("type=Warning");
        let list = Api::<Event>::all(client).list(&lp).await.map_err(api_err)?;
        let mut events: Vec<(Option<String>, String)> = list
            .items
            .into_iter()
            .map(|e| {
                let ts = e.last_timestamp.as_ref().and_then(ts_string);
                let ns = e
                    .involved_object
                    .namespace
                    .clone()
                    .map(|n| format!("{n}/"))
                    .unwrap_or_default();
                let obj = e.involved_object.name.clone().unwrap_or_default();
                (
                    ts,
                    format!(
                        "{ns}{obj}: {} {}",
                        e.reason.unwrap_or_default(),
                        e.message.unwrap_or_default()
                    ),
                )
            })
            .collect();
        events.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(events.into_iter().take(8).map(|(_, m)| m).collect())
    }

    /// Count Ready/suspended/failing for every kind in a CRD family (Flux, ESO).
    /// Lists all matching kinds concurrently (instead of sequentially) so the
    /// dashboard overview doesn't accumulate per-kind latency.
    async fn rollup(&self, category: Category) -> HealthRollup {
        let client = self.client();
        let catalog_store = self.shared.catalog();
        let catalog = catalog_store.load();
        let futs = catalog
            .entries
            .iter()
            .filter(|e| e.kind.category == category)
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
                let suspended = obj
                    .data
                    .get("spec")
                    .and_then(|s| s.get("suspend"))
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                if suspended {
                    rollup.suspended += 1;
                }
                match ready_condition(&obj.data) {
                    Some(true) => rollup.ready += 1,
                    Some(false) => rollup.failing += 1,
                    None => {}
                }
            }
        }
        rollup
    }
}

/// Read a `Ready` status condition: Some(true)=Ready, Some(false)=not Ready, None=absent/unknown.
fn ready_condition(data: &serde_json::Value) -> Option<bool> {
    let conds = data.get("status")?.get("conditions")?.as_array()?;
    let ready = conds.iter().find(|c| c["type"] == "Ready")?;
    match ready["status"].as_str()? {
        "True" => Some(true),
        "False" => Some(false),
        _ => None,
    }
}
