//! Best-effort metrics-server reads. If metrics-server isn't installed, node
//! usage is simply absent and the dashboard renders capacity without usage.

use std::collections::HashMap;

use kube::Client;
use serde_json::Value;

/// Map node name -> (cpu cores used, memory bytes used) from metrics-server.
pub async fn node_usage(client: &Client) -> HashMap<String, (f64, f64)> {
    let req = match http::Request::get("/apis/metrics.k8s.io/v1beta1/nodes").body(Vec::new()) {
        Ok(r) => r,
        Err(_) => return HashMap::new(),
    };
    let body: Value = match client.request::<Value>(req).await {
        Ok(v) => v,
        Err(_) => return HashMap::new(), // metrics-server not installed
    };

    let mut out = HashMap::new();
    if let Some(items) = body.get("items").and_then(|i| i.as_array()) {
        for item in items {
            let name = item
                .get("metadata")
                .and_then(|m| m.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string();
            let usage = item.get("usage");
            let cpu = usage
                .and_then(|u| u.get("cpu"))
                .and_then(|c| c.as_str())
                .map(parse_cpu)
                .unwrap_or(0.0);
            let mem = usage
                .and_then(|u| u.get("memory"))
                .and_then(|m| m.as_str())
                .map(parse_mem)
                .unwrap_or(0.0);
            if !name.is_empty() {
                out.insert(name, (cpu, mem));
            }
        }
    }
    out
}

/// Map "namespace/pod" -> (cpu cores used, memory bytes used), summed across the
/// pod's containers. Empty if metrics-server isn't installed.
pub async fn pod_usage(client: &Client) -> HashMap<String, (f64, f64)> {
    let req = match http::Request::get("/apis/metrics.k8s.io/v1beta1/pods").body(Vec::new()) {
        Ok(r) => r,
        Err(_) => return HashMap::new(),
    };
    let body: Value = match client.request::<Value>(req).await {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };

    let mut out = HashMap::new();
    if let Some(items) = body.get("items").and_then(|i| i.as_array()) {
        for item in items {
            let meta = item.get("metadata");
            let ns = meta
                .and_then(|m| m.get("namespace"))
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            let name = meta
                .and_then(|m| m.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let (mut cpu, mut mem) = (0.0, 0.0);
            if let Some(conts) = item.get("containers").and_then(|c| c.as_array()) {
                for c in conts {
                    let u = c.get("usage");
                    cpu += u
                        .and_then(|u| u.get("cpu"))
                        .and_then(|c| c.as_str())
                        .map(parse_cpu)
                        .unwrap_or(0.0);
                    mem += u
                        .and_then(|u| u.get("memory"))
                        .and_then(|m| m.as_str())
                        .map(parse_mem)
                        .unwrap_or(0.0);
                }
            }
            out.insert(format!("{ns}/{name}"), (cpu, mem));
        }
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

/// Map "namespace/name" of a bound PVC to its current filesystem usage, by
/// scraping each node's kubelet stats summary. Each volume entry in the
/// summary already carries a `pvcRef`, so we never need to walk pod specs.
/// Empty if the API server can't be reached, kubelet's `VolumeStats` is
/// disabled, or the service account lacks `nodes/proxy` RBAC.
pub async fn pvc_usage(client: &Client) -> HashMap<String, PvcUsage> {
    // List nodes first; one cheap LIST, then a per-node proxy GET in parallel.
    let nodes_req = match http::Request::get("/api/v1/nodes").body(Vec::new()) {
        Ok(r) => r,
        Err(_) => return HashMap::new(),
    };
    let body: Value = match client.request::<Value>(nodes_req).await {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };
    let names: Vec<String> = body
        .get("items")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|n| {
                    n.get("metadata")
                        .and_then(|m| m.get("name"))
                        .and_then(|n| n.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    // Fetch every node's stats summary concurrently. The kube proxy path is
    // /api/v1/nodes/{name}/proxy/stats/summary. Any 403/404 just yields an
    // empty node; we still want the rest of the cluster's data.
    let fetches = names.iter().map(|name| {
        let client = client.clone();
        let name = name.clone();
        async move {
            let path = format!("/api/v1/nodes/{}/proxy/stats/summary", name);
            let req = match http::Request::get(&path).body(Vec::new()) {
                Ok(r) => r,
                Err(_) => return Vec::new(),
            };
            let body: Value = match client.request::<Value>(req).await {
                Ok(v) => v,
                Err(_) => return Vec::new(),
            };
            let mut out = Vec::new();
            if let Some(pods) = body.get("pods").and_then(|p| p.as_array()) {
                for pod in pods {
                    let Some(volumes) = pod.get("volume").and_then(|v| v.as_array()) else {
                        continue;
                    };
                    for vol in volumes {
                        let Some(pvc_ref) = vol.get("pvcRef") else {
                            continue;
                        };
                        let Some(pvc_ns) = pvc_ref.get("namespace").and_then(|n| n.as_str()) else {
                            continue;
                        };
                        let Some(pvc_name) = pvc_ref.get("name").and_then(|n| n.as_str()) else {
                            continue;
                        };
                        let used = vol.get("usedBytes").and_then(|n| n.as_f64()).unwrap_or(0.0);
                        // `capacityBytes` from kubelet is the *filesystem* size
                        // (excludes reserved blocks, journal, inode tables) — it
                        // doesn't match the user's mental model of "150Gi".
                        // The row UI uses the PVC's bound capacity as the % base.
                        out.push((format!("{pvc_ns}/{pvc_name}"), PvcUsage { used }));
                    }
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
