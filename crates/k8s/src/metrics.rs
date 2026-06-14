//! Best-effort metrics-server reads. If metrics-server isn't installed, node
//! usage is simply absent and the dashboard renders capacity without usage.

use std::collections::HashMap;

use kube::Client;
use serde::Deserialize;

/// GET `path` and deserialize the body with sonic-rs (SIMD; faster than
/// serde_json for these large metrics / kubelet-stats payloads), into a typed
/// `T`. `None` on any error — request build, transport, non-2xx, or parse — so
/// every caller degrades gracefully (metrics-server absent, RBAC denied, …).
async fn get_json<T: serde::de::DeserializeOwned>(client: &Client, path: &str) -> Option<T> {
    let req = http::Request::get(path).body(Vec::new()).ok()?;
    let body = client.request_text(req).await.ok()?;
    sonic_rs::from_str(&body).ok()
}

// Minimal typed views of the responses, so we never materialise the full (large)
// metrics-server / kubelet-stats JSON into an untyped `serde_json::Value` tree —
// the kubelet `/stats/summary` is huge. serde skips every field we don't declare.

#[derive(Deserialize)]
// Override serde's bound inference: `#[serde(default)]` on a generic field would
// otherwise demand `T: Default`, which the item types don't (and needn't) impl.
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct List<T> {
    #[serde(default)]
    items: Vec<T>,
}

#[derive(Deserialize, Default)]
struct MetaName {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize, Default)]
struct MetaNameNs {
    #[serde(default)]
    name: String,
    #[serde(default)]
    namespace: String,
}

/// metrics-server usage block: cpu/memory as Kubernetes quantity strings.
#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    cpu: String,
    #[serde(default)]
    memory: String,
}

#[derive(Deserialize)]
struct NodeMetric {
    #[serde(default)]
    metadata: MetaName,
    #[serde(default)]
    usage: Usage,
}

/// Map node name -> (cpu cores used, memory bytes used) from metrics-server.
pub async fn node_usage(client: &Client) -> HashMap<String, (f64, f64)> {
    let list: List<NodeMetric> = match get_json(client, "/apis/metrics.k8s.io/v1beta1/nodes").await
    {
        Some(v) => v,
        None => return HashMap::new(), // metrics-server not installed
    };

    let mut out = HashMap::new();
    for item in list.items {
        if item.metadata.name.is_empty() {
            continue;
        }
        // parse_* return 0.0 for empty/invalid, so unconditional calls are safe.
        let usage = (parse_cpu(&item.usage.cpu), parse_mem(&item.usage.memory));
        out.insert(item.metadata.name, usage);
    }
    out
}

#[derive(Deserialize)]
struct PodMetric {
    #[serde(default)]
    metadata: MetaNameNs,
    #[serde(default)]
    containers: Vec<Container>,
}

#[derive(Deserialize)]
struct Container {
    #[serde(default)]
    usage: Usage,
}

/// Map "namespace/pod" -> (cpu cores used, memory bytes used), summed across the
/// pod's containers. Empty if metrics-server isn't installed.
pub async fn pod_usage(client: &Client) -> HashMap<String, (f64, f64)> {
    let list: List<PodMetric> = match get_json(client, "/apis/metrics.k8s.io/v1beta1/pods").await {
        Some(v) => v,
        None => return HashMap::new(),
    };

    let mut out = HashMap::new();
    for item in list.items {
        if item.metadata.name.is_empty() {
            continue;
        }
        let (mut cpu, mut mem) = (0.0, 0.0);
        for c in &item.containers {
            cpu += parse_cpu(&c.usage.cpu);
            mem += parse_mem(&c.usage.memory);
        }
        out.insert(
            format!("{}/{}", item.metadata.namespace, item.metadata.name),
            (cpu, mem),
        );
    }
    out
}

/// Parse a Kubernetes CPU quantity into cores ("100m" -> 0.1, "2" -> 2.0).
pub fn parse_cpu(s: &str) -> f64 {
    if let Some(n) = s.strip_suffix('n') {
        n.parse::<f64>().unwrap_or(0.0) / 1e9
    } else if let Some(u) = s.strip_suffix('u') {
        u.parse::<f64>().unwrap_or(0.0) / 1e6
    } else if let Some(m) = s.strip_suffix('m') {
        m.parse::<f64>().unwrap_or(0.0) / 1e3
    } else {
        s.parse::<f64>().unwrap_or(0.0)
    }
}

/// Filesystem bytes used inside a PVC. The "%" baseline is the PVC's bound
/// capacity (not this number — that excludes reserved blocks/journal/inodes).
#[derive(Clone, Copy, Default)]
pub struct PvcUsage {
    pub used: f64,
}

// Minimal view of a kubelet `/stats/summary`: we only need each pod's volumes
// that carry a `pvcRef` and their `usedBytes`. Everything else (node stats,
// per-container cpu/mem, network, ephemeral storage, …) is skipped.
#[derive(Deserialize)]
struct StatsSummary {
    #[serde(default)]
    pods: Vec<PodStats>,
}

#[derive(Deserialize)]
struct PodStats {
    #[serde(default)]
    volume: Vec<VolumeStats>,
}

#[derive(Deserialize)]
struct VolumeStats {
    #[serde(default, rename = "pvcRef")]
    pvc_ref: Option<MetaNameNs>,
    #[serde(default, rename = "usedBytes")]
    used_bytes: f64,
}

#[derive(Deserialize)]
struct NodeItem {
    #[serde(default)]
    metadata: MetaName,
}

/// Map "namespace/name" of a bound PVC to its current filesystem usage, by
/// scraping each node's kubelet stats summary. Each volume entry in the
/// summary already carries a `pvcRef`, so we never need to walk pod specs.
/// Empty if the API server can't be reached, kubelet's `VolumeStats` is
/// disabled, or the service account lacks `nodes/proxy` RBAC.
pub async fn pvc_usage(client: &Client) -> HashMap<String, PvcUsage> {
    // List nodes first (names only); one cheap LIST, then a per-node proxy GET.
    let nodes: List<NodeItem> = match get_json(client, "/api/v1/nodes").await {
        Some(v) => v,
        None => return HashMap::new(),
    };
    let names: Vec<String> = nodes
        .items
        .into_iter()
        .map(|n| n.metadata.name)
        .filter(|n| !n.is_empty())
        .collect();

    // Fetch every node's stats summary concurrently. The kube proxy path is
    // /api/v1/nodes/{name}/proxy/stats/summary. Any 403/404 just yields an
    // empty node; we still want the rest of the cluster's data.
    let fetches = names.iter().map(|name| {
        let client = client.clone();
        let name = name.clone();
        async move {
            let path = format!("/api/v1/nodes/{}/proxy/stats/summary", name);
            let summary: StatsSummary = match get_json(&client, &path).await {
                Some(v) => v,
                None => return Vec::new(),
            };
            let mut out = Vec::new();
            for pod in &summary.pods {
                for vol in &pod.volume {
                    let Some(pvc) = &vol.pvc_ref else { continue };
                    if pvc.name.is_empty() {
                        continue;
                    }
                    // `capacityBytes` from kubelet is the *filesystem* size
                    // (excludes reserved blocks, journal, inode tables) — it
                    // doesn't match the user's mental model of "150Gi". The row
                    // UI uses the PVC's bound capacity as the % base instead.
                    out.push((
                        format!("{}/{}", pvc.namespace, pvc.name),
                        PvcUsage {
                            used: vol.used_bytes,
                        },
                    ));
                }
            }
            out
        }
    });
    let per_node = futures::future::join_all(fetches).await;

    // Last writer wins. In practice every node sees its own pods' volumes, and
    // a PVC is mounted on exactly one node, so collisions are rare; when they
    // do happen (RWX volumes mounted on multiple nodes), taking the most
    // recent sample is the right behaviour.
    let mut out = HashMap::new();
    for entries in per_node {
        for (k, v) in entries {
            out.insert(k, v);
        }
    }
    out
}

/// Parse a Kubernetes memory quantity into bytes ("128Mi", "1Gi", "1024Ki", "1000000").
pub fn parse_mem(s: &str) -> f64 {
    const SUFFIXES: &[(&str, f64)] = &[
        ("Ki", 1024.0),
        ("Mi", 1024.0 * 1024.0),
        ("Gi", 1024.0 * 1024.0 * 1024.0),
        ("Ti", 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("K", 1e3),
        ("M", 1e6),
        ("G", 1e9),
        ("T", 1e12),
        ("k", 1e3),
    ];
    for (suffix, mult) in SUFFIXES {
        if let Some(n) = s.strip_suffix(suffix) {
            return n.parse::<f64>().unwrap_or(0.0) * mult;
        }
    }
    s.parse::<f64>().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_alloc::min_delta;
    use serde_json::json;

    /// A synthetic kubelet `/stats/summary`: many pods, each with the usual
    /// container / cpu / memory / network stats plus one PVC volume. roder needs
    /// only the volume's `pvcRef` + `usedBytes`; everything else is the bloat the
    /// typed `StatsSummary` skips.
    fn stats_summary_json(pods: usize) -> String {
        let cont = json!({
            "name": "c",
            "cpu": { "usageNanoCores": 1, "usageCoreNanoSeconds": 1 },
            "memory": { "usageBytes": 1, "workingSetBytes": 1, "rssBytes": 1 },
            "rootfs": { "availableBytes": 1, "capacityBytes": 1, "usedBytes": 1, "inodes": 1, "inodesFree": 1, "inodesUsed": 1 },
            "logs": { "availableBytes": 1, "capacityBytes": 1, "usedBytes": 1 },
        });
        let pod_arr: Vec<_> = (0..pods)
            .map(|i| {
                json!({
                    "podRef": { "name": format!("pod{i}"), "namespace": "default", "uid": "u" },
                    "cpu": { "usageNanoCores": 1, "usageCoreNanoSeconds": 1 },
                    "memory": { "usageBytes": 1, "workingSetBytes": 1, "rssBytes": 1, "pageFaults": 1 },
                    "network": { "name": "eth0", "rxBytes": 1, "txBytes": 1, "interfaces": [{ "name": "eth0", "rxBytes": 1, "txBytes": 1 }] },
                    "containers": [cont.clone(), cont.clone(), cont.clone()],
                    "volume": [{
                        "name": "data",
                        "pvcRef": { "name": format!("pvc{i}"), "namespace": "default" },
                        "usedBytes": 1024, "capacityBytes": 2048, "availableBytes": 1024,
                        "inodes": 1, "inodesFree": 1, "inodesUsed": 1
                    }],
                })
            })
            .collect();
        json!({ "node": { "nodeName": "n1" }, "pods": pod_arr }).to_string()
    }

    #[test]
    fn stats_summary_typed_is_far_lighter_than_value() {
        let json = stats_summary_json(200);
        // Sanity: the typed view extracts exactly the volumes we need.
        let s: StatsSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(s.pods.len(), 200);
        let pvc = s.pods[0].volume[0].pvc_ref.as_ref().unwrap();
        assert_eq!(pvc.name, "pvc0");
        assert_eq!(s.pods[0].volume[0].used_bytes, 1024.0);

        // The kubelet summary was the top memory consumer in the heaptrack
        // profile. Parsing it into the minimal `StatsSummary` (volumes only)
        // allocates a small fraction of parsing the full `serde_json::Value`
        // tree. Same parser both sides, so this isolates the typed-struct win.
        let full = min_delta(|| serde_json::from_str::<serde_json::Value>(&json).unwrap());
        let typed = min_delta(|| serde_json::from_str::<StatsSummary>(&json).unwrap());
        assert!(
            typed.saturating_mul(8) < full,
            "typed StatsSummary should allocate a small fraction of the full Value: typed={typed} full={full}"
        );
    }
}
