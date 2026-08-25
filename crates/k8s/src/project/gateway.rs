//! Gateway API row projectors: HTTPRoute (and sibling routes) / Gateway / GatewayClass.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::str_at;
use super::status::{cond_to_status, condition_status, ready_label};

pub(crate) fn httproute_cells(data: &Value) -> (Vec<String>, RowStatus) {
    // Newline-joined list values: the table shows a compact comma form, the
    // reusable tooltip renders them as a list. Hostnames are sorted ascending.
    let mut hosts: Vec<&str> = data
        .get("spec")
        .and_then(|s| s.get("hostnames"))
        .and_then(|h| h.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    hosts.sort_unstable();
    let hostnames = hosts.join("\n");
    // The Gateway(s) this route attaches to (spec.parentRefs[].name).
    let gateways = data
        .get("spec")
        .and_then(|s| s.get("parentRefs"))
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|r| r.get("name").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let (msg, st) = httproute_status(data);
    (vec![hostnames, gateways, msg], st)
}

pub(crate) fn parent_route_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let (cells, status) = httproute_cells(data);
    (cells[1..].to_vec(), status)
}

/// HTTPRoute acceptance: a not-True Accepted/ResolvedRefs condition is an error
/// (surfacing its message); an Accepted=True route is healthy.
fn httproute_status(data: &Value) -> (String, RowStatus) {
    let Some(parents) = data
        .get("status")
        .and_then(|s| s.get("parents"))
        .and_then(|p| p.as_array())
    else {
        return (String::new(), RowStatus::Unknown);
    };
    let mut accepted = false;
    for p in parents {
        let Some(conds) = p.get("conditions").and_then(|c| c.as_array()) else {
            continue;
        };
        for c in conds {
            let typ = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let status = c.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let message = c.get("message").and_then(|v| v.as_str()).unwrap_or("");
            if matches!(typ, "Accepted" | "ResolvedRefs") && status != "True" {
                let row_status = if status == "False" {
                    RowStatus::Error
                } else {
                    RowStatus::Pending
                };
                return (message.to_string(), row_status);
            }
            if typ == "Accepted" && status == "True" {
                accepted = true;
            }
        }
    }
    if accepted {
        ("Accepted".to_string(), RowStatus::Ok)
    } else {
        (String::new(), RowStatus::Unknown)
    }
}

pub(crate) fn gateway_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let class = str_at(data, &["spec", "gatewayClassName"]).unwrap_or_default();
    let address = data
        .get("status")
        .and_then(|s| s.get("addresses"))
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("value").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let programmed = condition_status(data, "Programmed");
    (
        vec![class, address, ready_label(&programmed)],
        cond_to_status(programmed.as_deref()),
    )
}

pub(crate) fn gatewayclass_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let controller = str_at(data, &["spec", "controllerName"]).unwrap_or_default();
    let accepted = condition_status(data, "Accepted");
    (
        vec![controller, ready_label(&accepted)],
        cond_to_status(accepted.as_deref()),
    )
}
