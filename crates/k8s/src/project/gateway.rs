//! Gateway API row projectors: HTTPRoute (and sibling routes) / Gateway / GatewayClass.

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::{int_at, str_at};
use super::status::{cond_to_status, condition_from, condition_status, ready_label};

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
    let generation = int_at(data, &["metadata", "generation"]);
    let mut accepted = false;
    for parent in parents {
        let Some(conditions) = parent.get("conditions").and_then(Value::as_array) else {
            continue;
        };
        for type_ in ["Accepted", "ResolvedRefs"] {
            let Some(condition) = condition_from(conditions, generation, type_) else {
                continue;
            };
            let status = condition
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("");
            let message = condition
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("");
            if status != "True" {
                let row_status = if status == "False" {
                    RowStatus::Error
                } else {
                    RowStatus::Pending
                };
                return (message.to_string(), row_status);
            }
            if type_ == "Accepted" {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn route_conditions_drive_health() {
        let route = |conditions: Value| {
            json!({
                "metadata": {"generation": 2},
                "spec": {
                    "hostnames": ["www.example.com", "api.example.com"],
                    "parentRefs": [{"name": "public"}, {"name": "private"}]
                },
                "status": {"parents": [{"conditions": conditions}]}
            })
        };
        let cases = [
            (
                route(json!([{
                    "type": "Accepted", "status": "True", "observedGeneration": 2
                }])),
                "Accepted",
                RowStatus::Ok,
            ),
            (
                route(json!([{
                    "type": "Accepted",
                    "status": "False",
                    "message": "No matching listener",
                    "observedGeneration": 2
                }])),
                "No matching listener",
                RowStatus::Error,
            ),
            (
                route(json!([{
                    "type": "Accepted",
                    "status": "Unknown",
                    "message": "Waiting for controller",
                    "observedGeneration": 2
                }])),
                "Waiting for controller",
                RowStatus::Pending,
            ),
            (
                route(json!([
                    {"type": "Accepted", "status": "True", "observedGeneration": 2},
                    {
                        "type": "ResolvedRefs",
                        "status": "False",
                        "message": "Backend not found",
                        "observedGeneration": 2
                    }
                ])),
                "Backend not found",
                RowStatus::Error,
            ),
            (
                route(json!([{
                    "type": "Accepted", "status": "True", "observedGeneration": 1
                }])),
                "",
                RowStatus::Unknown,
            ),
        ];

        for (data, expected_message, expected_status) in cases {
            let (cells, status) = httproute_cells(&data);
            assert_eq!(cells[0], "api.example.com\nwww.example.com", "{data}");
            assert_eq!(cells[1], "public\nprivate", "{data}");
            assert_eq!(cells[2], expected_message, "{data}");
            assert_eq!(status, expected_status, "{data}");

            let (parent_cells, parent_status) = parent_route_cells(&data);
            assert_eq!(parent_cells, cells[1..], "{data}");
            assert_eq!(parent_status, status, "{data}");
        }
    }

    #[test]
    fn gateway_projections_include_addresses_and_condition_health() {
        let gateway = json!({
            "spec": {"gatewayClassName": "envoy"},
            "status": {
                "addresses": [{"value": "192.0.2.10"}, {"value": "gateway.example.com"}],
                "conditions": [{"type": "Programmed", "status": "True"}]
            }
        });
        assert_eq!(
            gateway_cells(&gateway),
            (
                vec![
                    "envoy".to_string(),
                    "192.0.2.10\ngateway.example.com".to_string(),
                    "True".to_string()
                ],
                RowStatus::Ok
            )
        );

        let class = json!({
            "spec": {"controllerName": "gateway.envoyproxy.io/gatewayclass-controller"},
            "status": {"conditions": [{"type": "Accepted", "status": "False"}]}
        });
        assert_eq!(
            gatewayclass_cells(&class),
            (
                vec![
                    "gateway.envoyproxy.io/gatewayclass-controller".to_string(),
                    "False".to_string()
                ],
                RowStatus::Error
            )
        );
    }
}
