//! Project watched `DynamicObject`s into generic [`ResourceRow`]s. Working from the
//! informer cache (not the apiserver Table printer) keeps everything etcd-kind:
//! one watch per type feeds both the snapshot and live deltas. Columns come from a
//! hand-written projector when we have one, otherwise the CRD's own
//! `additionalPrinterColumns` (see [`crate::printer_columns`]), otherwise a generic
//! status — with hand-written columns layered on as additions to a CRD's own.

use kube::api::DynamicObject;
use roder_core::{ResourceRow, RowStatus, Trend};
use serde_json::Value;

use crate::informers::UsageEntry;
use crate::printer_columns::{self, PrinterCol};

mod accessors;
mod certmanager;
mod core;
mod eso;
mod flux;
mod format;
mod gateway;
mod pods;
mod rbac;
mod status;
mod workloads;

pub(crate) use self::accessors::ts_string;
use self::certmanager::{acme_state_cells, certificate_cells, certrequest_cells, issuer_cells};
use self::core::{
    configmap_cells, endpoints_cells, endpointslice_cells, generic_cells, hpa_cells, ingress_cells,
    namespace_cells, node_cells, pdb_cells, pv_cells, pvc_cells, secret_cells, service_cells,
    storageclass_cells,
};
use self::eso::{eso_generic_cells, external_secret_cells};
use self::flux::ready_message_cells;
use self::format::humanize_since;
use self::gateway::{gateway_cells, gatewayclass_cells, httproute_cells};
use self::pods::pod_cells;
use self::rbac::rolebinding_cells;
use self::status::generic_status;
use self::workloads::{cronjob_cells, daemonset_cells, job_cells, replicaset_cells, workload_cells};

/// One source of truth per kind: the extra column headers *and* the projector that
/// fills them, defined together in the same match arm so they can't drift apart.
#[derive(Clone, Copy)]
struct KindView {
    /// Column headers beyond the standard Name / Namespace / Age.
    headers: &'static [&'static str],
    project: Project,
}

/// How a kind's cells are produced. Most project purely from the object body; Pod
/// additionally needs its deletion state and live metrics-server usage.
#[derive(Clone, Copy)]
enum Project {
    Plain(fn(&Value) -> (Vec<String>, RowStatus)),
    Pod,
}

/// The generic fallback view for a kind with no hand-written projector and no CRD
/// printer columns: just a best-effort status reason.
const GENERIC_VIEW: KindView = KindView {
    headers: &["Status"],
    project: Project::Plain(generic_cells),
};

/// Resolve the view for a (group, kind), or `None` if there's no hand-written
/// projector (so callers can fall back to CRD printer columns, then [`GENERIC_VIEW`]).
fn view_for(group: &str, kind: &str) -> KindView {
    explicit_view(group, kind).unwrap_or(GENERIC_VIEW)
}

/// The hand-written projector for a kind, if one exists. Arms mirror kubectl's
/// columns, specialised for the resources this cluster runs. Adding a kind = one
/// arm here. Returns `None` for anything we haven't special-cased.
fn explicit_view(group: &str, kind: &str) -> Option<KindView> {
    use Project::Plain;
    macro_rules! view {
        ($headers:expr, $project:expr) => {
            KindView { headers: $headers, project: $project }
        };
    }
    let view = match (group, kind) {
        ("", "Pod") => view!(
            &[
                "Ready", "Status", "Restarts", "CPU", "%CPU/R", "%CPU/L", "MEM", "%MEM/R",
                "%MEM/L", "IP", "Node",
            ],
            Project::Pod
        ),
        ("apps", "Deployment") | ("apps", "StatefulSet") => {
            view!(&["Ready", "Available"], Plain(workload_cells))
        }
        ("apps", "DaemonSet") => view!(&["Ready", "Available"], Plain(daemonset_cells)),
        ("apps", "ReplicaSet") => view!(&["Ready"], Plain(replicaset_cells)),
        ("batch", "Job") => view!(&["Completions", "Status"], Plain(job_cells)),
        ("batch", "CronJob") => view!(&["Schedule", "Suspended"], Plain(cronjob_cells)),
        ("", "Service") => view!(&["Type", "ClusterIP"], Plain(service_cells)),
        ("", "Node") => view!(&["Status", "Version"], Plain(node_cells)),
        ("", "Namespace") => view!(&["Phase"], Plain(namespace_cells)),
        ("", "PersistentVolumeClaim") => view!(&["Phase", "Capacity"], Plain(pvc_cells)),
        // Flux: Ready (True/False) + the condition's reason (Status) + its message.
        _ if group.ends_with("fluxcd.io") => {
            view!(&["Ready", "Status", "Message"], Plain(ready_message_cells))
        }
        ("external-secrets.io", "ExternalSecret") => view!(
            &["Store Type", "Store", "Refresh Interval", "Status", "Ready", "Last Sync"],
            Plain(external_secret_cells)
        ),
        ("external-secrets.io", _) => view!(&["Ready", "Status", "Store"], Plain(eso_generic_cells)),
        ("cert-manager.io", "Certificate") => {
            view!(&["Ready", "Status", "Secret"], Plain(certificate_cells))
        }
        ("cert-manager.io", "ClusterIssuer") | ("cert-manager.io", "Issuer") => {
            view!(&["Ready", "Status"], Plain(issuer_cells))
        }
        ("cert-manager.io", "CertificateRequest") => {
            view!(&["Approved", "Ready", "Issuer"], Plain(certrequest_cells))
        }
        ("acme.cert-manager.io", "Order") | ("acme.cert-manager.io", "Challenge") => {
            view!(&["State", "Reason"], Plain(acme_state_cells))
        }
        // Gateway API: HTTPRoute plus its Gateway/GatewayClass and sibling route kinds.
        ("gateway.networking.k8s.io", "HTTPRoute")
        | ("gateway.networking.k8s.io", "GRPCRoute")
        | ("gateway.networking.k8s.io", "TLSRoute")
        | ("gateway.networking.k8s.io", "TCPRoute") => {
            view!(&["Hostnames", "Gateways", "Status"], Plain(httproute_cells))
        }
        ("gateway.networking.k8s.io", "Gateway") => {
            view!(&["Class", "Address", "Programmed"], Plain(gateway_cells))
        }
        ("gateway.networking.k8s.io", "GatewayClass") => {
            view!(&["Controller", "Accepted"], Plain(gatewayclass_cells))
        }
        // Core / storage / networking.
        ("", "PersistentVolume") => view!(
            &["Capacity", "Access", "Reclaim", "Status", "Claim", "StorageClass"],
            Plain(pv_cells)
        ),
        ("storage.k8s.io", "StorageClass") => view!(
            &["Provisioner", "Reclaim", "Binding Mode", "Expandable"],
            Plain(storageclass_cells)
        ),
        ("", "Secret") => view!(&["Type", "Data"], Plain(secret_cells)),
        ("", "ConfigMap") => view!(&["Data"], Plain(configmap_cells)),
        ("", "Endpoints") => view!(&["Endpoints"], Plain(endpoints_cells)),
        ("discovery.k8s.io", "EndpointSlice") => {
            view!(&["Address Type", "Endpoints", "Ports"], Plain(endpointslice_cells))
        }
        ("networking.k8s.io", "Ingress") => {
            view!(&["Class", "Hosts", "Address"], Plain(ingress_cells))
        }
        // Autoscaling / policy.
        ("autoscaling", "HorizontalPodAutoscaler") => {
            view!(&["Reference", "Targets", "Min", "Max", "Replicas"], Plain(hpa_cells))
        }
        ("policy", "PodDisruptionBudget") => {
            view!(&["Min Available", "Max Unavailable", "Allowed"], Plain(pdb_cells))
        }
        // RBAC bindings (Role/ClusterRole rules render in the detail pane).
        ("rbac.authorization.k8s.io", "RoleBinding")
        | ("rbac.authorization.k8s.io", "ClusterRoleBinding") => {
            view!(&["Role"], Plain(rolebinding_cells))
        }
        // No hand-written projector: let CRD printer columns (or the generic
        // status fallback) take over.
        _ => return None,
    };
    Some(view)
}

/// Extra column headers for a kind: the CRD's own `additionalPrinterColumns` (when
/// it's a CRD) plus any hand-written columns as additions, else the hand-written or
/// generic view. `crd` is empty for built-ins. Header set must mirror [`project_row`].
pub fn columns_for(group: &str, kind: &str, crd: &[PrinterCol]) -> Vec<String> {
    merged_headers(group, kind, crd)
}

/// Shared header resolution so `columns_for` and `project_row` never drift: for a
/// CRD, exactly its own declared printer columns; for a built-in (`crd` empty), the
/// hand-written / generic view (built-ins have no CRD to read columns from).
fn merged_headers(group: &str, kind: &str, crd: &[PrinterCol]) -> Vec<String> {
    if crd.is_empty() {
        return view_for(group, kind)
            .headers
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    crd.iter().map(|c| c.name.clone()).collect()
}

/// Produce a row's cells (and status) matching [`merged_headers`]: for a CRD, the
/// values of its own declared printer columns; for a built-in, the hand-written /
/// generic projector. Row *coloring* still uses a hand-written projector's tuned
/// status when one exists (e.g. Flux Suspended → yellow) — that's not a column.
fn project_cells(
    group: &str,
    kind: &str,
    data: &Value,
    deleting: bool,
    usage: Option<UsageEntry>,
    crd: &[PrinterCol],
) -> (Vec<String>, Vec<Trend>, RowStatus) {
    if crd.is_empty() {
        return match view_for(group, kind).project {
            Project::Pod => pod_cells(data, deleting, usage),
            Project::Plain(f) => {
                let (cells, status) = f(data);
                (cells, vec![], status)
            }
        };
    }

    // CRD's own columns, evaluating each declared jsonPath against the object.
    let cells: Vec<String> = crd
        .iter()
        .map(|c| {
            let raw = printer_columns::eval(&c.json_path, data);
            if c.col_type.eq_ignore_ascii_case("date") && !raw.is_empty() {
                humanize_since(&raw)
            } else {
                raw
            }
        })
        .collect();

    // Keep a hand-written projector's tuned row color where we have one; else derive
    // it from the Ready/phase status generically.
    let status = match explicit_view(group, kind) {
        Some(view) => match view.project {
            Project::Pod => pod_cells(data, deleting, usage).2,
            Project::Plain(f) => f(data).1,
        },
        None => generic_status(data),
    };
    (cells, vec![], status)
}

/// Project an object into a row for the given (group, kind). `usage` carries the
/// pod's live metrics entry (current + previous) from metrics-server, when available;
/// `crd` is the kind's declared printer columns (empty for built-ins).
pub fn project_row(
    group: &str,
    kind: &str,
    obj: &DynamicObject,
    usage: Option<UsageEntry>,
    crd: &[PrinterCol],
) -> ResourceRow {
    let name = obj.metadata.name.clone().unwrap_or_default();
    let namespace = obj.metadata.namespace.clone();
    let uid = obj
        .metadata
        .uid
        .clone()
        .unwrap_or_else(|| format!("{}/{}", namespace.clone().unwrap_or_default(), name));
    let created = obj
        .metadata
        .creation_timestamp
        .as_ref()
        .and_then(|t| ts_string(t));

    // A resource with a deletionTimestamp is being terminated; surface that
    // distinctly (yellow) instead of leaving it green/Running until it vanishes.
    let deleting = obj.metadata.deletion_timestamp.is_some();
    let data = &obj.data;
    let (cells, trends, mut status) = project_cells(group, kind, data, deleting, usage, crd);
    debug_assert_eq!(
        cells.len(),
        merged_headers(group, kind, crd).len(),
        "cell/header count mismatch for {group}/{kind}"
    );

    // Anything mid-deletion reads as terminating (yellow), whatever its phase says.
    if deleting {
        status = RowStatus::Warn;
    }

    ResourceRow {
        uid,
        namespace,
        name,
        created,
        cells,
        trends,
        status,
    }
}
