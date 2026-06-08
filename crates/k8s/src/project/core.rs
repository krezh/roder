//! Core / storage / networking / autoscaling / policy row projectors, plus the
//! generic status-only fallback.

use roder_core::RowStatus;
use serde_json::Value;

use crate::metrics::PvcUsage;

use super::accessors::{data_count, int_at, intstr_at, str_at};
use super::format::{endpoints_summary, hpa_targets, human_bytes, short_access_mode};
use super::status::{cond_to_status, condition_status, generic_status};

pub(crate) fn namespace_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let phase = str_at(data, &["status", "phase"]).unwrap_or_default();
    let st = if phase == "Active" {
        RowStatus::Ok
    } else {
        RowStatus::Warn
    };
    (vec![phase], st)
}

pub(crate) fn pvc_cells(data: &Value, usage: Option<PvcUsage>) -> (Vec<String>, RowStatus) {
    let phase = str_at(data, &["status", "phase"]).unwrap_or_default();
    // The "total" should match the user's mental model: what the PVC was
    // requested for, or what was actually provisioned (status.capacity). We do
    // NOT use kubelet's `capacityBytes` here because that reports the
    // filesystem capacity, which is smaller than the volume by reserved
    // blocks (typically 5%), the journal, and inode tables — so a 150Gi
    // volume would render as "146.6Gi" and confuse the user.
    let total_str = str_at(data, &["status", "capacity", "storage"])
        .or_else(|| str_at(data, &["spec", "resources", "requests", "storage"]))
        .unwrap_or_default();
    let total_bytes = crate::metrics::parse_mem(&total_str);

    // A PVC is "in use" iff kubelet's volume scan saw a pvcRef for it — which
    // only happens when a pod currently mounts the volume. So presence in the
    // usage map IS the "in use" signal; we render the % cell from `used` and
    // a separate Mount cell from the same presence check.
    let in_use = usage.is_some();
    let (capacity_cell, pct_cell) = match (usage, total_bytes > 0.0) {
        (Some(u), true) if u.used > 0.0 => {
            let pct = (u.used / total_bytes * 100.0).clamp(0.0, 999.0);
            (
                format!("{} / {}", human_bytes(u.used), total_str),
                format!("{pct:.0}%"),
            )
        }
        // Mounted but the filesystem reports no bytes (freshly mounted, or
        // the kubelet doesn't have a snapshot yet): show a placeholder dot.
        (Some(_), _) => (total_str.clone(), "·".to_string()),
        // Not mounted (or no kubelet access): show capacity only.
        _ => (total_str.clone(), String::new()),
    };
    let mount_cell = if in_use { "true" } else { "false" }.to_string();
    let st = if phase == "Bound" {
        RowStatus::Ok
    } else {
        RowStatus::Pending
    };
    (vec![phase, capacity_cell, pct_cell, mount_cell], st)
}

pub(crate) fn secret_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let ty = str_at(data, &["type"]).unwrap_or_else(|| "Opaque".into());
    (
        vec![ty, data_count(data, &["data", "stringData"]).to_string()],
        RowStatus::Ok,
    )
}

pub(crate) fn configmap_cells(data: &Value) -> (Vec<String>, RowStatus) {
    (
        vec![data_count(data, &["data", "binaryData"]).to_string()],
        RowStatus::Ok,
    )
}

pub(crate) fn endpoints_cells(data: &Value) -> (Vec<String>, RowStatus) {
    (vec![endpoints_summary(data)], RowStatus::Ok)
}

pub(crate) fn generic_cells(data: &Value) -> (Vec<String>, RowStatus) {
    (vec![], generic_status(data))
}

pub(crate) fn service_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let ty = str_at(data, &["spec", "type"]).unwrap_or_else(|| "ClusterIP".into());
    let cluster_ip = str_at(data, &["spec", "clusterIP"]).unwrap_or_default();
    (vec![ty, cluster_ip], RowStatus::Ok)
}

pub(crate) fn node_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let ready = condition_status(data, "Ready");
    let version = str_at(data, &["status", "nodeInfo", "kubeletVersion"]).unwrap_or_default();
    let label = if ready.as_deref() == Some("True") {
        "Ready".to_string()
    } else {
        "NotReady".to_string()
    };
    (vec![label, version], cond_to_status(ready.as_deref()))
}

pub(crate) fn pv_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let cap = str_at(data, &["spec", "capacity", "storage"]).unwrap_or_default();
    let modes = data
        .get("spec")
        .and_then(|s| s.get("accessModes"))
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(short_access_mode)
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let reclaim = str_at(data, &["spec", "persistentVolumeReclaimPolicy"]).unwrap_or_default();
    let phase = str_at(data, &["status", "phase"]).unwrap_or_default();
    let claim = match (
        str_at(data, &["spec", "claimRef", "namespace"]),
        str_at(data, &["spec", "claimRef", "name"]),
    ) {
        (Some(ns), Some(n)) => format!("{ns}/{n}"),
        (None, Some(n)) => n,
        _ => String::new(),
    };
    let sc = str_at(data, &["spec", "storageClassName"]).unwrap_or_default();
    let st = match phase.as_str() {
        "Bound" => RowStatus::Ok,
        "Available" => RowStatus::Pending,
        "Released" => RowStatus::Warn,
        "Failed" => RowStatus::Error,
        _ => RowStatus::Unknown,
    };
    (vec![cap, modes, reclaim, phase, claim, sc], st)
}

pub(crate) fn storageclass_cells(data: &Value) -> (Vec<String>, RowStatus) {
    // StorageClass carries these at the top level, not under spec.
    let provisioner = str_at(data, &["provisioner"]).unwrap_or_default();
    let reclaim = str_at(data, &["reclaimPolicy"]).unwrap_or_else(|| "Delete".into());
    let binding = str_at(data, &["volumeBindingMode"]).unwrap_or_else(|| "Immediate".into());
    let expandable = data
        .get("allowVolumeExpansion")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    (
        vec![provisioner, reclaim, binding, expandable.to_string()],
        RowStatus::Ok,
    )
}

pub(crate) fn endpointslice_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let addr_type = str_at(data, &["addressType"]).unwrap_or_default();
    let endpoints = data
        .get("endpoints")
        .and_then(|e| e.as_array())
        .map(|a| {
            a.iter()
                .flat_map(|e| {
                    e.get("addresses")
                        .and_then(|x| x.as_array())
                        .map(|v| {
                            v.iter()
                                .filter_map(|s| s.as_str())
                                .map(String::from)
                                .collect()
                        })
                        .unwrap_or_else(Vec::new)
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let ports = data
        .get("ports")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|p| p.get("port").and_then(|v| v.as_i64()))
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    (vec![addr_type, endpoints, ports], RowStatus::Ok)
}

pub(crate) fn ingress_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let class = str_at(data, &["spec", "ingressClassName"]).unwrap_or_default();
    let hosts = data
        .get("spec")
        .and_then(|s| s.get("rules"))
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|r| r.get("host").and_then(|v| v.as_str()))
                .map(String::from)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let address = data
        .get("status")
        .and_then(|s| s.get("loadBalancer"))
        .and_then(|l| l.get("ingress"))
        .and_then(|i| i.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    x.get("ip")
                        .and_then(|v| v.as_str())
                        .or_else(|| x.get("hostname").and_then(|v| v.as_str()))
                })
                .map(String::from)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    (vec![class, hosts, address], RowStatus::Ok)
}

pub(crate) fn hpa_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let kind = str_at(data, &["spec", "scaleTargetRef", "kind"]).unwrap_or_default();
    let name = str_at(data, &["spec", "scaleTargetRef", "name"]).unwrap_or_default();
    let reference = if kind.is_empty() {
        name
    } else {
        format!("{kind}/{name}")
    };
    let min = int_at(data, &["spec", "minReplicas"]).unwrap_or(1);
    let max = int_at(data, &["spec", "maxReplicas"]).unwrap_or(0);
    let current = int_at(data, &["status", "currentReplicas"]).unwrap_or(0);
    let targets = hpa_targets(data);
    // At the ceiling = note it (yellow); otherwise the autoscaler is operating normally.
    let st = if max > 0 && current >= max {
        RowStatus::Warn
    } else {
        RowStatus::Ok
    };
    (
        vec![
            reference,
            targets,
            min.to_string(),
            max.to_string(),
            current.to_string(),
        ],
        st,
    )
}

pub(crate) fn pdb_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let min = intstr_at(data, &["spec", "minAvailable"]).unwrap_or_else(|| "N/A".into());
    let max = intstr_at(data, &["spec", "maxUnavailable"]).unwrap_or_else(|| "N/A".into());
    let allowed = int_at(data, &["status", "disruptionsAllowed"]).unwrap_or(0);
    // No disruptions currently allowed → the budget is blocking (yellow).
    let st = if allowed == 0 {
        RowStatus::Warn
    } else {
        RowStatus::Ok
    };
    (vec![min, max, allowed.to_string()], st)
}
