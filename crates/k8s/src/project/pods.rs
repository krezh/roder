//! Pod row projection: kubectl-style READY / STATUS / restarts plus live CPU/mem
//! against the pod's requests and limits.

use roder_core::{RowStatus, Trend};
use serde_json::Value;

use super::accessors::str_at;
use crate::informers::UsageEntry;

/// Sum the pod's container resource requests/limits → ((cpu_req, cpu_lim) cores,
/// (mem_req, mem_lim) bytes). A zero means "unset" (treated as n/a downstream).
fn pod_resources(data: &Value) -> ((f64, f64), (f64, f64)) {
    use crate::metrics::{parse_cpu, parse_mem};
    let (mut cr, mut cl, mut mr, mut ml) = (0.0, 0.0, 0.0, 0.0);
    if let Some(conts) = data
        .get("spec")
        .and_then(|s| s.get("containers"))
        .and_then(|c| c.as_array())
    {
        for c in conts {
            let res = c.get("resources");
            let get = |kind: &str, key: &str| {
                res.and_then(|r| r.get(kind))
                    .and_then(|r| r.get(key))
                    .and_then(|v| v.as_str())
            };
            cr += get("requests", "cpu").map(parse_cpu).unwrap_or(0.0);
            cl += get("limits", "cpu").map(parse_cpu).unwrap_or(0.0);
            mr += get("requests", "memory").map(parse_mem).unwrap_or(0.0);
            ml += get("limits", "memory").map(parse_mem).unwrap_or(0.0);
        }
    }
    ((cr, cl), (mr, ml))
}

/// kubectl-style init-container status: `Init:N/M` while init containers run,
/// `Init:<reason>` / `Init:ExitCode:N` / `Init:Signal:N` on failure. Returns None
/// once all init containers have completed successfully.
fn init_status(data: &Value) -> Option<String> {
    let inits = data
        .get("status")
        .and_then(|s| s.get("initContainerStatuses"))
        .and_then(|c| c.as_array())?;
    let total = data
        .get("spec")
        .and_then(|s| s.get("initContainers"))
        .and_then(|c| c.as_array())
        .map(|a| a.len())
        .unwrap_or(inits.len());
    for (i, c) in inits.iter().enumerate() {
        let state = c.get("state");
        if let Some(t) = state.and_then(|s| s.get("terminated")) {
            let exit = t.get("exitCode").and_then(|e| e.as_i64()).unwrap_or(0);
            if exit == 0 {
                continue; // this init container finished OK
            }
            let r = t.get("reason").and_then(|r| r.as_str()).unwrap_or("");
            if !r.is_empty() {
                return Some(format!("Init:{r}"));
            }
            let signal = t.get("signal").and_then(|s| s.as_i64()).unwrap_or(0);
            return Some(if signal != 0 {
                format!("Init:Signal:{signal}")
            } else {
                format!("Init:ExitCode:{exit}")
            });
        }
        if let Some(w) = state.and_then(|s| s.get("waiting")) {
            let r = w.get("reason").and_then(|r| r.as_str()).unwrap_or("");
            if !r.is_empty() && r != "PodInitializing" {
                return Some(format!("Init:{r}"));
            }
        }
        // Sidecar init containers (restartPolicy: Always, KEP-753) stay running
        // after their init phase completes. The API sets started=true once
        // initialization is done — treat them as "completed" like kubectl does.
        if is_restartable_init(data, c) && c.get("started").and_then(|s| s.as_bool()) == Some(true)
        {
            continue;
        }
        return Some(format!("Init:{i}/{total}"));
    }
    None
}

fn is_restartable_init(data: &Value, status: &Value) -> bool {
    let Some(name) = status.get("name").and_then(Value::as_str) else {
        return false;
    };
    data.get("spec")
        .and_then(|spec| spec.get("initContainers"))
        .and_then(Value::as_array)
        .is_some_and(|containers| {
            containers.iter().any(|container| {
                container.get("name").and_then(Value::as_str) == Some(name)
                    && container.get("restartPolicy").and_then(Value::as_str) == Some("Always")
            })
        })
}

fn initialized(data: &Value) -> bool {
    data.get("status")
        .and_then(|status| status.get("conditions"))
        .and_then(Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition.get("type").and_then(Value::as_str) == Some("Initialized")
                    && condition.get("status").and_then(Value::as_str) == Some("True")
            })
        })
}

fn init_container_complete(data: &Value, status: &Value) -> bool {
    status
        .get("state")
        .and_then(|state| state.get("terminated"))
        .and_then(|terminated| terminated.get("exitCode"))
        .and_then(Value::as_i64)
        == Some(0)
        || is_restartable_init(data, status)
            && status.get("started").and_then(Value::as_bool) == Some(true)
}

fn restart_info(data: &Value) -> (i64, Option<time::OffsetDateTime>) {
    let container_statuses = data
        .get("status")
        .and_then(|s| s.get("containerStatuses"))
        .and_then(|c| c.as_array());
    let init_statuses = data
        .get("status")
        .and_then(|s| s.get("initContainerStatuses"))
        .and_then(|c| c.as_array());

    let still_initializing = init_status(data).is_some() && !initialized(data);
    let mut statuses = Vec::new();
    if still_initializing {
        for status in init_statuses.into_iter().flatten() {
            statuses.push(status);
            if !init_container_complete(data, status) {
                break;
            }
        }
    } else {
        statuses.extend(
            init_statuses
                .into_iter()
                .flatten()
                .filter(|status| is_restartable_init(data, status)),
        );
        statuses.extend(container_statuses.into_iter().flatten());
    }

    let mut count = 0;
    let mut latest = None;
    for status in statuses {
        let restarts = status
            .get("restartCount")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        count += restarts;
        if restarts == 0 {
            continue;
        }
        let Some(finished_at) = status
            .get("lastState")
            .and_then(|state| state.get("terminated"))
            .and_then(|terminated| terminated.get("finishedAt"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if let Ok(timestamp) =
            time::OffsetDateTime::parse(finished_at, &time::format_description::well_known::Rfc3339)
        {
            if latest.is_none_or(|current| timestamp > current) {
                latest = Some(timestamp);
            }
        }
    }

    (count, latest)
}

pub(crate) fn pod_cells(
    data: &Value,
    deleting: bool,
    usage: Option<UsageEntry>,
) -> (Vec<String>, Vec<Trend>, RowStatus) {
    let statuses = data
        .get("status")
        .and_then(|s| s.get("containerStatuses"))
        .and_then(|c| c.as_array());
    // Sidecar init containers (restartPolicy: Always, KEP-753) that have
    // started=true have completed initialization and now behave like regular
    // long-running containers — kubectl includes them in the ready count.
    let init_statuses = data
        .get("status")
        .and_then(|s| s.get("initContainerStatuses"))
        .and_then(|c| c.as_array());
    let sidecars = init_statuses
        .into_iter()
        .flatten()
        .filter(|status| is_restartable_init(data, status))
        .collect::<Vec<_>>();
    let regular_total = data
        .get("spec")
        .and_then(|spec| spec.get("containers"))
        .and_then(Value::as_array)
        .map_or_else(|| statuses.map_or(0, Vec::len), Vec::len);
    let sidecar_total = data
        .get("spec")
        .and_then(|spec| spec.get("initContainers"))
        .and_then(Value::as_array)
        .map(|containers| {
            containers
                .iter()
                .filter(|container| {
                    container.get("restartPolicy").and_then(Value::as_str) == Some("Always")
                })
                .count()
        })
        .unwrap_or(0);
    let total = regular_total + sidecar_total;
    let ready = statuses
        .map(|a| {
            a.iter()
                .filter(|c| c["ready"].as_bool() == Some(true))
                .count()
        })
        .unwrap_or(0)
        + sidecars
            .iter()
            .filter(|c| {
                c.get("started").and_then(Value::as_bool) == Some(true)
                    && c.get("ready").and_then(Value::as_bool) == Some(true)
            })
            .count();
    let (restarts, last_restart_time) = restart_info(data);

    let restarts_cell = if restarts > 0 {
        if let Some(t) = last_restart_time {
            let timestamp = t
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default();
            format!("{restarts}\x1f{timestamp}")
        } else {
            restarts.to_string()
        }
    } else {
        "0".to_string()
    };

    let node = str_at(data, &["spec", "nodeName"]).unwrap_or_default();
    let phase = str_at(data, &["status", "phase"]).unwrap_or_default();

    // Compute the displayed status the way kubectl does. Init containers come first
    // (Init:0/1, Init:Error, …); only once they're done do the main container states
    // (waiting/terminated reason, incl. PodInitializing/ContainerCreating) apply.
    let mut reason = if let Some(init) = init_status(data) {
        init
    } else {
        let mut reason = str_at(data, &["status", "reason"])
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| phase.clone());
        if let Some(arr) = statuses {
            for c in arr {
                let state = c.get("state");
                if let Some(w) = state.and_then(|s| s.get("waiting")) {
                    if let Some(r) = w.get("reason").and_then(|x| x.as_str()) {
                        if !r.is_empty() {
                            reason = r.to_string();
                        }
                    }
                } else if let Some(t) = state.and_then(|s| s.get("terminated")) {
                    reason = terminated_reason(t);
                } else if c["ready"].as_bool() != Some(true) {
                    // Running but not ready and previously crashed → surface that reason.
                    if let Some(lt) = c.get("lastState").and_then(|s| s.get("terminated")) {
                        if lt.get("exitCode").and_then(|e| e.as_i64()).unwrap_or(0) != 0 {
                            reason = terminated_reason(lt);
                        }
                    }
                }
            }
        }
        reason
    };

    if deleting {
        reason = "Terminating".to_string();
    }

    let pod_ip = str_at(data, &["status", "podIP"]).unwrap_or_default();
    let status = pod_status(&reason, &phase, ready, total, deleting);

    // Live usage vs the pod's requests/limits. "n/a" when usage or the base is absent.
    let ((cpu_r, cpu_l), (mem_r, mem_l)) = pod_resources(data);
    let pct = |used: Option<f64>, base: f64| match used {
        Some(u) if base > 0.0 => format!("{}", (u / base * 100.0).round() as i64),
        _ => "n/a".to_string(),
    };
    let cpu_trend = usage.map_or(Trend::None, |e| {
        if e.has_prev() {
            e.trend_cpu()
        } else {
            Trend::None
        }
    });
    let mem_trend = usage.map_or(Trend::None, |e| {
        if e.has_prev() {
            e.trend_mem()
        } else {
            Trend::None
        }
    });
    let (cpu_used, mem_used) = match usage {
        Some(e) => (Some(e.cpu), Some(e.mem)),
        None => (None, None),
    };
    let cpu_cell = cpu_used
        .map(|c| format!("{}", (c * 1000.0).round() as i64))
        .unwrap_or_else(|| "n/a".into());
    let mem_cell = mem_used
        .map(|m| format!("{}", (m / (1024.0 * 1024.0)).round() as i64))
        .unwrap_or_else(|| "n/a".into());

    let trends = vec![
        Trend::None,                                       // Ready
        Trend::None,                                       // Status
        Trend::None,                                       // Restarts
        cpu_trend,                                         // CPU
        if cpu_r > 0.0 { cpu_trend } else { Trend::None }, // %CPU/R
        if cpu_l > 0.0 { cpu_trend } else { Trend::None }, // %CPU/L
        mem_trend,                                         // MEM
        if mem_r > 0.0 { mem_trend } else { Trend::None }, // %MEM/R
        if mem_l > 0.0 { mem_trend } else { Trend::None }, // %MEM/L
        Trend::None,                                       // IP
        Trend::None,                                       // Node
    ];

    (
        vec![
            format!("{ready}/{total}"),
            reason,
            restarts_cell,
            cpu_cell,
            pct(cpu_used, cpu_r),
            pct(cpu_used, cpu_l),
            mem_cell,
            pct(mem_used, mem_r),
            pct(mem_used, mem_l),
            pod_ip,
            node,
        ],
        trends,
        status,
    )
}

fn terminated_reason(t: &Value) -> String {
    if let Some(r) = t.get("reason").and_then(|x| x.as_str()) {
        if !r.is_empty() {
            return r.to_string();
        }
    }
    let signal = t.get("signal").and_then(|x| x.as_i64()).unwrap_or(0);
    let code = t.get("exitCode").and_then(|x| x.as_i64()).unwrap_or(0);
    if signal != 0 {
        format!("Signal:{signal}")
    } else if code == 0 {
        "Completed".to_string()
    } else {
        "Error".to_string()
    }
}

fn pod_status(reason: &str, phase: &str, ready: usize, total: usize, deleting: bool) -> RowStatus {
    if deleting {
        return RowStatus::Warn;
    }
    const RED: &[&str] = &[
        "Error",
        "Failed",
        "CrashLoopBackOff",
        "ImagePullBackOff",
        "ErrImagePull",
        "ImageInspectError",
        "InvalidImageName",
        "CreateContainerError",
        "CreateContainerConfigError",
        "RunContainerError",
        "OOMKilled",
        "ContainerStatusUnknown",
        "NodeLost",
        "Evicted",
        "DeadlineExceeded",
    ];
    // An `Init:<reason>` failure (e.g. Init:CrashLoopBackOff) is still red; Init:N/M
    // progress falls through to the warn case below.
    let bare = reason.strip_prefix("Init:").unwrap_or(reason);
    if RED.contains(&bare) || bare.starts_with("Signal:") || bare.starts_with("ExitCode:") {
        RowStatus::Error
    } else if reason == "Completed" || phase == "Succeeded" {
        // Finished successfully (e.g. Job pods) — neutral/gray, not active-green.
        RowStatus::Done
    } else if (reason == "Running" || phase == "Running") && total > 0 && ready == total {
        RowStatus::Ok
    } else {
        // Pending, ContainerCreating, PodInitializing, running-but-not-ready, etc.
        RowStatus::Warn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::format_description::well_known::Rfc3339;

    fn timestamp(value: &str) -> time::OffsetDateTime {
        time::OffsetDateTime::parse(value, &Rfc3339).unwrap()
    }

    #[test]
    fn initialized_pod_counts_main_containers_and_restartable_init_sidecars() {
        let data = json!({
            "spec": {
                "containers": [{"name": "app"}],
                "initContainers": [
                    {"name": "setup"},
                    {"name": "logger", "restartPolicy": "Always"}
                ]
            },
            "status": {
                "conditions": [{"type": "Initialized", "status": "True"}],
                "initContainerStatuses": [
                    {
                        "name": "setup",
                        "restartCount": 7,
                        "state": {"terminated": {"exitCode": 0}},
                        "lastState": {"terminated": {"finishedAt": "2026-08-27T12:00:00Z"}}
                    },
                    {
                        "name": "logger",
                        "started": true,
                        "restartCount": 2,
                        "state": {"running": {}},
                        "lastState": {"terminated": {"finishedAt": "2026-08-27T11:00:00Z"}}
                    }
                ],
                "containerStatuses": [{
                    "name": "app",
                    "restartCount": 3,
                    "lastState": {"terminated": {"finishedAt": "2026-08-27T13:00:00Z"}}
                }]
            }
        });

        assert_eq!(
            restart_info(&data),
            (5, Some(timestamp("2026-08-27T13:00:00Z")))
        );
    }

    #[test]
    fn initializing_pod_counts_regular_init_restarts_only() {
        let data = json!({
            "spec": {
                "containers": [{"name": "app"}],
                "initContainers": [{"name": "setup"}, {"name": "later"}]
            },
            "status": {
                "initContainerStatuses": [
                    {
                        "name": "setup",
                        "restartCount": 4,
                        "state": {"waiting": {"reason": "PodInitializing"}},
                        "lastState": {"terminated": {"finishedAt": "2026-08-27T14:00:00Z"}}
                    },
                    {
                        "name": "later",
                        "restartCount": 8,
                        "lastState": {"terminated": {"finishedAt": "2026-08-27T16:00:00Z"}}
                    }
                ],
                "containerStatuses": [{
                    "name": "app",
                    "restartCount": 9,
                    "lastState": {"terminated": {"finishedAt": "2026-08-27T15:00:00Z"}}
                }]
            }
        });

        assert_eq!(
            restart_info(&data),
            (4, Some(timestamp("2026-08-27T14:00:00Z")))
        );
    }

    #[test]
    fn started_status_does_not_turn_a_regular_init_container_into_a_sidecar() {
        let data = json!({
            "spec": {"initContainers": [{"name": "setup"}]},
            "status": {"initContainerStatuses": [{
                "name": "setup",
                "started": true,
                "state": {"running": {}}
            }]}
        });

        assert_eq!(init_status(&data).as_deref(), Some("Init:0/1"));
    }

    #[test]
    fn restartable_init_sidecar_contributes_to_ready_total() {
        let data = json!({
            "spec": {
                "containers": [{"name": "app"}],
                "initContainers": [
                    {"name": "setup"},
                    {"name": "logger", "restartPolicy": "Always"}
                ]
            },
            "status": {
                "phase": "Running",
                "initContainerStatuses": [
                    {
                        "name": "setup",
                        "state": {"terminated": {"exitCode": 0}}
                    },
                    {
                        "name": "logger",
                        "started": true,
                        "ready": true,
                        "state": {"running": {}}
                    }
                ],
                "containerStatuses": [{
                    "name": "app",
                    "ready": true,
                    "state": {"running": {}}
                }]
            }
        });

        let (cells, _, status) = pod_cells(&data, false, None);
        assert_eq!(cells[0], "2/2");
        assert_eq!(status, RowStatus::Ok);
    }
}
