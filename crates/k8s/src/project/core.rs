//! Core / storage / networking / autoscaling / policy row projectors, plus the
//! generic status-only fallback.

use roder_core::RowStatus;
use serde_json::Value;

use crate::metrics::PvcUsage;

use super::accessors::{data_count, int_at, intstr_at, str_at};
use super::format::{endpoints_summary, hpa_targets, human_bytes, short_access_mode};
use super::status::{
    cond_to_status, condition_reason, condition_status, status_generation_is_stale,
};

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
    let st = match phase.as_str() {
        "Bound" => RowStatus::Ok,
        "Pending" | "" => RowStatus::Pending,
        "Lost" => RowStatus::Error,
        _ => RowStatus::Unknown,
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

pub(crate) fn event_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let type_ = str_at(data, &["type"]).unwrap_or_default();
    let reason = str_at(data, &["reason"]).unwrap_or_default();
    let kind = str_at(data, &["involvedObject", "kind"]).unwrap_or_default();
    let name = str_at(data, &["involvedObject", "name"]).unwrap_or_default();
    let object = match (kind.is_empty(), name.is_empty()) {
        (false, false) => format!("{kind}/{name}"),
        (_, false) => name,
        _ => String::new(),
    };
    let message = str_at(data, &["message"]).unwrap_or_default();
    let status = match type_.as_str() {
        "Normal" => RowStatus::Ok,
        "Warning" => RowStatus::Warn,
        _ => RowStatus::Unknown,
    };
    (vec![type_, reason, object, message], status)
}

pub(crate) fn endpoints_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let summary = endpoints_summary(data);
    let status = if summary.is_empty() {
        RowStatus::Error
    } else {
        RowStatus::Ok
    };
    (vec![summary], status)
}

pub(crate) fn service_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let ty = str_at(data, &["spec", "type"]).unwrap_or_else(|| "ClusterIP".into());
    let cluster_ip = str_at(data, &["spec", "clusterIP"]).unwrap_or_default();
    let status = if ty == "LoadBalancer" && load_balancer_addresses(data).is_empty() {
        RowStatus::Pending
    } else {
        RowStatus::Ok
    };
    (vec![ty, cluster_ip], status)
}

pub(crate) fn node_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let ready = condition_status(data, "Ready");
    let version = str_at(data, &["status", "nodeInfo", "kubeletVersion"]).unwrap_or_default();
    let cordoned = data
        .get("spec")
        .and_then(|s| s.get("unschedulable"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let label = match (ready.as_deref() == Some("True"), cordoned) {
        (true, true) => "Ready,SchedulingDisabled".to_string(),
        (true, false) => "Ready".to_string(),
        (false, true) => "NotReady,SchedulingDisabled".to_string(),
        (false, false) => "NotReady".to_string(),
    };
    // Cordoned overrides the Ready-derived status so the context menu can key
    // off `RowStatus::Warn` to decide which of Cordon/Uncordon to show — the
    // same convention already used for Flux's suspended state.
    let status = if cordoned {
        RowStatus::Warn
    } else {
        cond_to_status(ready.as_deref())
    };
    (vec![label, version], status)
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
    let endpoint_values = data.get("endpoints").and_then(|e| e.as_array());
    let has_ready_endpoint = endpoint_values.is_some_and(|endpoints| {
        endpoints.iter().any(|endpoint| {
            let has_address = endpoint
                .get("addresses")
                .and_then(Value::as_array)
                .is_some_and(|addresses| !addresses.is_empty());
            let ready = endpoint
                .pointer("/conditions/ready")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            has_address && ready
        })
    });
    let endpoints = endpoint_values
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
    let status = if has_ready_endpoint {
        RowStatus::Ok
    } else {
        RowStatus::Error
    };
    (vec![addr_type, endpoints, ports], status)
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
    let addresses = load_balancer_addresses(data);
    let address = addresses.join("\n");
    let status = if addresses.is_empty() {
        RowStatus::Pending
    } else {
        RowStatus::Ok
    };
    (vec![class, hosts, address], status)
}

fn load_balancer_addresses(data: &Value) -> Vec<String> {
    data.get("status")
        .and_then(|s| s.get("loadBalancer"))
        .and_then(|l| l.get("ingress"))
        .and_then(|i| i.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    x.get("ip")
                        .and_then(|v| v.as_str())
                        .or_else(|| x.get("hostname").and_then(|v| v.as_str()))
                        .filter(|address| !address.is_empty())
                })
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
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
    let st = if status_generation_is_stale(data) {
        RowStatus::Pending
    } else {
        let able = condition_status(data, "AbleToScale");
        let active = condition_status(data, "ScalingActive");
        let limited = condition_status(data, "ScalingLimited");
        match (able.as_deref(), active.as_deref(), limited.as_deref()) {
            (Some("False"), _, _) | (_, Some("False"), _) => RowStatus::Error,
            (Some("True"), Some("True"), Some("True")) => RowStatus::Warn,
            (Some("True"), Some("True"), Some("False")) => RowStatus::Ok,
            _ => RowStatus::Pending,
        }
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
    let allowed = int_at(data, &["status", "disruptionsAllowed"]);
    let current = int_at(data, &["status", "currentHealthy"]);
    let desired = int_at(data, &["status", "desiredHealthy"]);
    let condition = condition_status(data, "DisruptionAllowed");
    let st = if status_generation_is_stale(data) {
        RowStatus::Pending
    } else if condition_reason(data, "DisruptionAllowed").as_deref() == Some("SyncFailed")
        || current
            .zip(desired)
            .is_some_and(|(current, desired)| current < desired)
    {
        RowStatus::Error
    } else {
        match (condition.as_deref(), allowed, current, desired) {
            (Some("True"), Some(allowed), _, _) if allowed > 0 => RowStatus::Ok,
            (Some("True" | "False"), Some(0), Some(_), Some(_)) => RowStatus::Warn,
            (None, Some(allowed), Some(_), Some(_)) if allowed > 0 => RowStatus::Ok,
            (Some("Unknown"), _, _, _) | (None, None, _, _) => RowStatus::Pending,
            _ => RowStatus::Pending,
        }
    };
    (vec![min, max, allowed.unwrap_or_default().to_string()], st)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn endpoints_require_a_ready_address() {
        let cases = [
            (json!({}), RowStatus::Error),
            (
                json!({"subsets": [{"notReadyAddresses": [{"ip": "10.0.0.1"}]}]}),
                RowStatus::Error,
            ),
            (
                json!({"subsets": [{"addresses": [{"ip": "10.0.0.1"}]}]}),
                RowStatus::Ok,
            ),
        ];

        for (data, expected) in cases {
            assert_eq!(endpoints_cells(&data).1, expected, "{data}");
        }
    }

    #[test]
    fn event_type_drives_health() {
        for (type_, expected) in [
            ("Normal", RowStatus::Ok),
            ("Warning", RowStatus::Warn),
            ("Unexpected", RowStatus::Unknown),
        ] {
            assert_eq!(event_cells(&json!({"type": type_})).1, expected, "{type_}");
        }
    }

    #[test]
    fn endpoint_slices_follow_ready_condition_semantics() {
        let cases = [
            (json!({}), RowStatus::Error),
            (
                json!({"endpoints": [{
                    "addresses": ["10.0.0.1"],
                    "conditions": {"ready": false}
                }]}),
                RowStatus::Error,
            ),
            (
                json!({"endpoints": [{"conditions": {"ready": true}}]}),
                RowStatus::Error,
            ),
            (
                json!({"endpoints": [{"addresses": ["10.0.0.1"]}]}),
                RowStatus::Ok,
            ),
            (
                json!({"endpoints": [{
                    "addresses": ["10.0.0.1"],
                    "conditions": {"ready": true}
                }]}),
                RowStatus::Ok,
            ),
        ];

        for (data, expected) in cases {
            assert_eq!(endpointslice_cells(&data).1, expected, "{data}");
        }
    }

    #[test]
    fn pvc_phase_drives_health() {
        let cases = [
            ("Bound", RowStatus::Ok),
            ("Pending", RowStatus::Pending),
            ("Lost", RowStatus::Error),
            ("Unexpected", RowStatus::Unknown),
        ];

        for (phase, expected) in cases {
            let data = json!({"status": {"phase": phase}});
            assert_eq!(pvc_cells(&data, None).1, expected, "{phase}");
        }
        assert_eq!(pvc_cells(&json!({}), None).1, RowStatus::Pending);
    }

    #[test]
    fn load_balancers_are_pending_until_they_have_an_address() {
        let pending_service = json!({"spec": {"type": "LoadBalancer"}});
        let ready_service = json!({
            "spec": {"type": "LoadBalancer"},
            "status": {"loadBalancer": {"ingress": [{"hostname": "lb.example.com"}]}}
        });
        assert_eq!(service_cells(&pending_service).1, RowStatus::Pending);
        assert_eq!(service_cells(&ready_service).1, RowStatus::Ok);
        assert_eq!(service_cells(&json!({})).1, RowStatus::Ok);

        assert_eq!(ingress_cells(&json!({})).1, RowStatus::Pending);
        assert_eq!(
            ingress_cells(&json!({
                "status": {"loadBalancer": {"ingress": [{"ip": "10.0.0.1"}]}}
            }))
            .1,
            RowStatus::Ok
        );
    }

    #[test]
    fn hpa_health_follows_controller_conditions() {
        let conditions = |able: &str, active: &str, limited: &str| {
            json!({"status": {"conditions": [
                {"type": "AbleToScale", "status": able},
                {"type": "ScalingActive", "status": active},
                {"type": "ScalingLimited", "status": limited}
            ]}})
        };
        let cases = [
            (conditions("True", "True", "False"), RowStatus::Ok),
            (conditions("True", "True", "True"), RowStatus::Warn),
            (conditions("False", "True", "False"), RowStatus::Error),
            (conditions("True", "False", "False"), RowStatus::Error),
            (conditions("Unknown", "True", "False"), RowStatus::Pending),
            (json!({}), RowStatus::Pending),
            (
                json!({
                    "metadata": {"generation": 2},
                    "status": {"observedGeneration": 1}
                }),
                RowStatus::Pending,
            ),
        ];

        for (data, expected) in cases {
            assert_eq!(hpa_cells(&data).1, expected, "{data}");
        }
    }

    #[test]
    fn pdb_health_distinguishes_violations_from_blocked_disruptions() {
        let cases = [
            (
                json!({"status": {
                    "currentHealthy": 3,
                    "desiredHealthy": 2,
                    "disruptionsAllowed": 1,
                    "conditions": [{"type": "DisruptionAllowed", "status": "True"}]
                }}),
                RowStatus::Ok,
            ),
            (
                json!({"status": {
                    "currentHealthy": 2,
                    "desiredHealthy": 2,
                    "disruptionsAllowed": 0,
                    "conditions": [{"type": "DisruptionAllowed", "status": "False"}]
                }}),
                RowStatus::Warn,
            ),
            (
                json!({"status": {
                    "currentHealthy": 1,
                    "desiredHealthy": 2,
                    "disruptionsAllowed": 0,
                    "conditions": [{"type": "DisruptionAllowed", "status": "False"}]
                }}),
                RowStatus::Error,
            ),
            (
                json!({"status": {
                    "conditions": [{
                        "type": "DisruptionAllowed",
                        "status": "False",
                        "reason": "SyncFailed"
                    }]
                }}),
                RowStatus::Error,
            ),
            (json!({}), RowStatus::Pending),
        ];

        for (data, expected) in cases {
            assert_eq!(pdb_cells(&data).1, expected, "{data}");
        }
    }
}
