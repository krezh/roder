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
        if c.get("started").and_then(|s| s.as_bool()) == Some(true) {
            continue;
        }
        return Some(format!("Init:{i}/{total}"));
    }
    None
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
    let sidecars = data
        .get("status")
        .and_then(|s| s.get("initContainerStatuses"))
        .and_then(|c| c.as_array())
        .map(|a| {
            a.iter()
                .filter(|c| c.get("started").and_then(|s| s.as_bool()) == Some(true))
                .collect::<Vec<_>>()
        });
    let total =
        statuses.map(|a| a.len()).unwrap_or(0) + sidecars.as_ref().map(|s| s.len()).unwrap_or(0);
    let ready = statuses
        .map(|a| {
            a.iter()
                .filter(|c| c["ready"].as_bool() == Some(true))
                .count()
        })
        .unwrap_or(0)
        + sidecars
            .as_ref()
            .map(|s| {
                s.iter()
                    .filter(|c| c["ready"].as_bool() == Some(true))
                    .count()
            })
            .unwrap_or(0);
    let restarts: i64 = statuses
        .map(|a| {
            a.iter()
                .map(|c| c["restartCount"].as_i64().unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
        + sidecars
            .as_ref()
            .map(|s| {
                s.iter()
                    .map(|c| c["restartCount"].as_i64().unwrap_or(0))
                    .sum::<i64>()
            })
            .unwrap_or(0);
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
            restarts.to_string(),
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
