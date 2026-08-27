//! Project watched `DynamicObject`s into generic [`ResourceRow`]s. Working from the
//! apiserver Table rows into Roder's row model, preserving Kubernetes' values
//! while retaining Roder's established column layout and enhancements.

use kube::api::DynamicObject;
use roder_core::{ResourceRow, RowStatus, Trend};
use serde_json::Value;

use crate::informers::UsageEntry;
use crate::metrics::PvcUsage;
use crate::table::{TableColumnDefinition, TableRow};

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
    configmap_cells, endpoints_cells, endpointslice_cells, hpa_cells, ingress_cells,
    namespace_cells, node_cells, pdb_cells, pv_cells, pvc_cells, secret_cells, service_cells,
    storageclass_cells,
};
use self::eso::{cluster_external_secret_cells, eso_generic_cells, external_secret_cells};
pub(crate) use self::flux::ready_message_cells;
use self::gateway::{gateway_cells, gatewayclass_cells, httproute_cells, parent_route_cells};
use self::pods::pod_cells;
use self::rbac::rolebinding_cells;
use self::status::generic_status;
use self::workloads::{
    cronjob_cells, daemonset_cells, job_cells, replicaset_cells, workload_cells,
};

/// One source of truth per kind: the extra column headers *and* the projector that
/// fills them, defined together in the same match arm so they can't drift apart.
#[derive(Clone, Copy)]
struct KindView {
    /// Column headers beyond the standard Name / Namespace / Age.
    headers: &'static [&'static str],
    project: Project,
}

/// How a kind's cells are produced. Most project purely from the object body; Pod
/// additionally needs its deletion state and live metrics-server usage; PVC needs
/// its live kubelet-reported filesystem usage.
#[derive(Clone, Copy)]
enum Project {
    Plain(fn(&Value) -> (Vec<String>, RowStatus)),
    Pod,
    Pvc,
}

/// The hand-written projector for a kind, if one exists. Arms mirror kubectl's
/// columns, specialised for the resources this cluster runs. Adding a kind = one
/// arm here. Returns `None` for anything we haven't special-cased.
fn explicit_view(group: &str, kind: &str) -> Option<KindView> {
    use Project::Plain;
    macro_rules! view {
        ($headers:expr, $project:expr) => {
            KindView {
                headers: $headers,
                project: $project,
            }
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
        ("", "PersistentVolumeClaim") => {
            view!(&["Phase", "Capacity", "Usage", "Mount"], Project::Pvc)
        }
        // Flux: Ready (True/False) + the condition's reason (Status) + its message.
        _ if group.ends_with("fluxcd.io") => {
            view!(&["Ready", "Status", "Message"], Plain(ready_message_cells))
        }
        ("external-secrets.io", "ExternalSecret") => view!(
            &[
                "Store Type",
                "Store",
                "Refresh Interval",
                "Status",
                "Ready",
                "Last Sync"
            ],
            Plain(external_secret_cells)
        ),
        ("external-secrets.io", "ClusterExternalSecret") => {
            view!(
                &["Ready", "Status", "Store"],
                Plain(cluster_external_secret_cells)
            )
        }
        ("external-secrets.io", _) => {
            view!(&["Ready", "Status"], Plain(eso_generic_cells))
        }
        ("cert-manager.io", "Certificate") => {
            view!(
                &["Ready", "Status", "Expires", "Renews", "Revision", "Secret"],
                Plain(certificate_cells)
            )
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
        | ("gateway.networking.k8s.io", "TLSRoute") => {
            view!(&["Hostnames", "Gateways", "Status"], Plain(httproute_cells))
        }
        ("gateway.networking.k8s.io", "TCPRoute") | ("gateway.networking.k8s.io", "UDPRoute") => {
            view!(&["Gateways", "Status"], Plain(parent_route_cells))
        }
        ("gateway.networking.k8s.io", "Gateway") => {
            view!(&["Class", "Address", "Programmed"], Plain(gateway_cells))
        }
        ("gateway.networking.k8s.io", "GatewayClass") => {
            view!(&["Controller", "Accepted"], Plain(gatewayclass_cells))
        }
        // Core / storage / networking.
        ("", "PersistentVolume") => view!(
            &[
                "Capacity",
                "Access",
                "Reclaim",
                "Status",
                "Claim",
                "StorageClass"
            ],
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
            view!(
                &["Address Type", "Endpoints", "Ports"],
                Plain(endpointslice_cells)
            )
        }
        ("networking.k8s.io", "Ingress") => {
            view!(&["Class", "Hosts", "Address"], Plain(ingress_cells))
        }
        // Autoscaling / policy.
        ("autoscaling", "HorizontalPodAutoscaler") => {
            view!(
                &["Reference", "Targets", "Min", "Max", "Replicas"],
                Plain(hpa_cells)
            )
        }
        ("policy", "PodDisruptionBudget") => {
            view!(
                &["Min Available", "Max Unavailable", "Allowed"],
                Plain(pdb_cells)
            )
        }
        // RBAC bindings (Role/ClusterRole rules render in the detail pane).
        ("rbac.authorization.k8s.io", "RoleBinding")
        | ("rbac.authorization.k8s.io", "ClusterRoleBinding") => {
            view!(&["Role"], Plain(rolebinding_cells))
        }
        // The apiserver Table remains the complete baseline.
        _ => return None,
    };
    Some(view)
}

/// Returns true for objects that should be hidden from the default view.
///
/// Helm stores each release revision as a `helm.sh/release.v1` Secret; these are
/// internal bookkeeping, never useful to browse, and typically outnumber real secrets
/// by an order of magnitude in Flux-managed clusters.
pub fn should_hide(group: &str, kind: &str, obj: &DynamicObject) -> bool {
    if group.is_empty() && kind == "Secret" {
        let ty = obj.data.get("type").and_then(|v| v.as_str()).unwrap_or("");
        return ty == "helm.sh/release.v1";
    }
    false
}

#[derive(Clone)]
enum TableCellSource {
    Namespace,
    Server(usize),
    Enhancement(usize),
    Age(Option<usize>),
}

/// Maps server Table cells and Roder enhancements into the displayed column order.
#[derive(Clone)]
pub(crate) struct TableLayout {
    pub(crate) columns: Vec<String>,
    sources: Vec<TableCellSource>,
}

pub(crate) fn table_layout(
    group: &str,
    kind: &str,
    all_namespaces: bool,
    definitions: &[TableColumnDefinition],
) -> TableLayout {
    let mut columns = Vec::new();
    let mut sources = Vec::new();
    if all_namespaces {
        columns.push("Namespace".to_string());
        sources.push(TableCellSource::Namespace);
    }
    let handled_crd = is_augmented_crd(group) && explicit_view(group, kind).is_some();
    let include_wide = handled_crd;
    let visible = definitions
        .iter()
        .enumerate()
        .filter(|(_, definition)| include_wide || definition.priority == 0)
        .collect::<Vec<_>>();

    for (index, definition) in &visible {
        if same_column(&definition.name, "Name") {
            columns.push(definition.name.clone());
            sources.push(TableCellSource::Server(*index));
        }
    }
    if handled_crd {
        let headers = enhancement_headers(group, kind);
        for (index, header) in headers.iter().enumerate() {
            columns.push((*header).to_string());
            sources.push(TableCellSource::Enhancement(index));
        }
        for (index, definition) in &visible {
            if !matches_identity_column(&definition.name)
                && !headers
                    .iter()
                    .any(|header| same_column(&definition.name, header))
            {
                columns.push(definition.name.clone());
                sources.push(TableCellSource::Server(*index));
            }
        }
    } else {
        for (index, definition) in &visible {
            if !matches_identity_column(&definition.name) {
                columns.push(definition.name.clone());
                sources.push(TableCellSource::Server(*index));
            }
        }

        let headers = enhancement_headers(group, kind);
        for (index, header) in headers.iter().enumerate() {
            if let Some(position) = columns
                .iter()
                .position(|column| same_column(column, header))
            {
                sources[position] = TableCellSource::Enhancement(index);
                continue;
            }
            let insert_at = headers[index + 1..]
                .iter()
                .find_map(|later| columns.iter().position(|column| same_column(column, later)))
                .unwrap_or(columns.len());
            columns.insert(insert_at, (*header).to_string());
            sources.insert(insert_at, TableCellSource::Enhancement(index));
        }
    }

    let server_age = visible
        .iter()
        .find(|(_, definition)| same_column(&definition.name, "Age"))
        .map(|(index, _)| *index);
    if handled_crd || server_age.is_some() {
        columns.push("Age".to_string());
        sources.push(TableCellSource::Age(server_age));
    }

    TableLayout { columns, sources }
}

pub(crate) fn project_table_row(
    group: &str,
    kind: &str,
    layout: &TableLayout,
    table_row: &TableRow,
    usage: Option<UsageEntry>,
    pvc_usage: Option<PvcUsage>,
) -> Option<(ResourceRow, DynamicObject)> {
    let mut object = table_row.object.clone()?;
    object.metadata.managed_fields = None;
    if should_hide(group, kind, &object) {
        return None;
    }

    let deleting = object.metadata.deletion_timestamp.is_some();
    let (enhancement_cells, enhancement_trends, mut status) =
        enhancement_values(group, kind, &object.data, deleting, usage, pvc_usage);
    if deleting {
        status = RowStatus::Warn;
    }

    let created = object
        .metadata
        .creation_timestamp
        .as_ref()
        .and_then(ts_string);
    let cells = layout
        .sources
        .iter()
        .map(|source| match source {
            TableCellSource::Namespace => object.metadata.namespace.clone().unwrap_or_default(),
            TableCellSource::Server(index) => table_row
                .cells
                .get(*index)
                .map(format_table_cell)
                .unwrap_or_else(|| "<none>".to_string()),
            TableCellSource::Enhancement(index) => {
                enhancement_cells.get(*index).cloned().unwrap_or_default()
            }
            TableCellSource::Age(server_index) => created.clone().unwrap_or_else(|| {
                server_index
                    .and_then(|index| table_row.cells.get(index))
                    .map(format_table_cell)
                    .unwrap_or_default()
            }),
        })
        .collect::<Vec<_>>();
    let trends = layout
        .sources
        .iter()
        .map(|source| match source {
            TableCellSource::Enhancement(index) => enhancement_trends
                .get(*index)
                .copied()
                .unwrap_or(Trend::None),
            TableCellSource::Namespace | TableCellSource::Server(_) | TableCellSource::Age(_) => {
                Trend::None
            }
        })
        .collect();

    let name = object.metadata.name.clone().unwrap_or_default();
    let namespace = object.metadata.namespace.clone();
    let uid = object
        .metadata
        .uid
        .clone()
        .unwrap_or_else(|| format!("{}/{}", namespace.clone().unwrap_or_default(), name));
    let labels = object.metadata.labels.clone().unwrap_or_default();
    Some((
        ResourceRow {
            uid,
            namespace,
            name,
            created,
            cells,
            trends,
            status,
            suspended: flux_suspended(group, &object.data),
            labels,
        },
        object,
    ))
}

pub(crate) fn reproject_table_row(
    group: &str,
    kind: &str,
    layout: &TableLayout,
    object: &DynamicObject,
    current: &ResourceRow,
    usage: Option<UsageEntry>,
    pvc_usage: Option<PvcUsage>,
) -> ResourceRow {
    let deleting = object.metadata.deletion_timestamp.is_some();
    let (enhancement_cells, enhancement_trends, mut status) =
        enhancement_values(group, kind, &object.data, deleting, usage, pvc_usage);
    if deleting {
        status = RowStatus::Warn;
    }

    let mut row = current.clone();
    for (cell_index, source) in layout.sources.iter().enumerate() {
        let TableCellSource::Enhancement(enhancement_index) = source else {
            continue;
        };
        if let Some(cell) = row.cells.get_mut(cell_index) {
            *cell = enhancement_cells
                .get(*enhancement_index)
                .cloned()
                .unwrap_or_default();
        }
        if let Some(trend) = row.trends.get_mut(cell_index) {
            *trend = enhancement_trends
                .get(*enhancement_index)
                .copied()
                .unwrap_or(Trend::None);
        }
    }
    row.status = status;
    row.suspended = flux_suspended(group, &object.data);
    row
}

fn flux_suspended(group: &str, data: &Value) -> bool {
    group.ends_with("fluxcd.io")
        && data
            .get("spec")
            .and_then(|spec| spec.get("suspend"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn enhancement_headers(group: &str, kind: &str) -> &'static [&'static str] {
    match (group, kind) {
        ("", "Pod") => &[
            "Restarts", "CPU", "%CPU/R", "%CPU/L", "MEM", "%MEM/R", "%MEM/L", "IP", "Node",
        ],
        ("", "PersistentVolumeClaim") => &["Usage", "Mount"],
        _ if is_augmented_crd(group) => explicit_view(group, kind)
            .map(|view| view.headers)
            .unwrap_or(&[]),
        _ => &[],
    }
}

fn enhancement_values(
    group: &str,
    kind: &str,
    data: &Value,
    deleting: bool,
    usage: Option<UsageEntry>,
    pvc_usage: Option<PvcUsage>,
) -> (Vec<String>, Vec<Trend>, RowStatus) {
    match (group, kind) {
        ("", "Pod") => {
            let (cells, trends, status) = pod_cells(data, deleting, usage);
            (cells[2..].to_vec(), trends[2..].to_vec(), status)
        }
        ("", "PersistentVolumeClaim") => {
            let (cells, status) = pvc_cells(data, pvc_usage);
            (vec![cells[2].clone(), cells[3].clone()], vec![], status)
        }
        _ if is_augmented_crd(group) => match explicit_view(group, kind) {
            Some(view) => match view.project {
                Project::Plain(project) => {
                    let (cells, status) = project(data);
                    (cells, vec![], status)
                }
                Project::Pod | Project::Pvc => unreachable!("CRDs use plain projectors"),
            },
            None => (vec![], vec![], generic_status(data)),
        },
        _ => {
            let status = match explicit_view(group, kind) {
                Some(view) => match view.project {
                    Project::Plain(project) => project(data).1,
                    Project::Pod => pod_cells(data, deleting, usage).2,
                    Project::Pvc => pvc_cells(data, pvc_usage).1,
                },
                None => generic_status(data),
            };
            (vec![], vec![], status)
        }
    }
}

fn is_augmented_crd(group: &str) -> bool {
    group.ends_with("fluxcd.io")
        || matches!(
            group,
            "external-secrets.io"
                | "cert-manager.io"
                | "acme.cert-manager.io"
                | "gateway.networking.k8s.io"
        )
}

fn same_column(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    normalize(left) == normalize(right)
}

fn matches_identity_column(column: &str) -> bool {
    ["Namespace", "Name", "Age"]
        .iter()
        .any(|identity| same_column(column, identity))
}

fn format_table_cell(value: &Value) -> String {
    match value {
        Value::Null => "<none>".to_string(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{TableColumnDefinition, TableRow};
    use serde_json::json;

    fn column(name: &str, priority: i32) -> TableColumnDefinition {
        TableColumnDefinition {
            name: name.to_string(),
            priority,
            ..Default::default()
        }
    }

    #[test]
    fn service_layout_and_cells_follow_server_table_exactly() {
        let definitions = [
            column("Name", 0),
            column("Type", 0),
            column("Cluster-IP", 0),
            column("External-IP", 0),
            column("Port(s)", 0),
            column("Age", 0),
            column("Selector", 1),
        ];
        let layout = table_layout("", "Service", true, &definitions);
        assert_eq!(
            layout.columns,
            [
                "Namespace",
                "Name",
                "Type",
                "Cluster-IP",
                "External-IP",
                "Port(s)",
                "Age",
            ]
        );

        let table_row = TableRow {
            cells: vec![
                json!("kube-api"),
                json!("LoadBalancer"),
                json!("10.97.108.17"),
                json!("192.168.25.20"),
                json!("6443:32647/TCP"),
                json!("6d7h"),
                json!("app=kube-api"),
            ],
            object: Some(
                serde_json::from_value(json!({
                    "apiVersion": "v1",
                    "kind": "Service",
                    "metadata": {
                        "name": "kube-api",
                        "namespace": "kube-system",
                        "uid": "uid-1"
                    },
                    "spec": {"type": "LoadBalancer"}
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let row = project_table_row("", "Service", &layout, &table_row, None, None)
            .unwrap()
            .0;
        assert_eq!(
            row.cells,
            [
                "kube-system",
                "kube-api",
                "LoadBalancer",
                "10.97.108.17",
                "192.168.25.20",
                "6443:32647/TCP",
                "6d7h",
            ]
        );
    }

    #[test]
    fn pod_enhancements_keep_their_legacy_position_before_network_columns() {
        let definitions = [
            column("Name", 0),
            column("Ready", 0),
            column("Status", 0),
            column("Restarts", 0),
            column("Age", 0),
            column("IP", 0),
            column("Node", 0),
        ];
        let layout = table_layout("", "Pod", false, &definitions);
        assert_eq!(
            layout.columns,
            [
                "Name", "Ready", "Status", "Restarts", "CPU", "%CPU/R", "%CPU/L", "MEM", "%MEM/R",
                "%MEM/L", "IP", "Node", "Age",
            ]
        );
    }

    #[test]
    fn pod_restart_enhancement_replaces_server_snapshot() {
        let definitions = [
            column("Name", 0),
            column("Ready", 0),
            column("Status", 0),
            column("Restarts", 0),
            column("Age", 0),
        ];
        let layout = table_layout("", "Pod", false, &definitions);
        let table_row = TableRow {
            cells: vec![
                json!("api"),
                json!("1/1"),
                json!("Running"),
                json!("99 (1h ago)"),
                json!("1h"),
            ],
            object: Some(
                serde_json::from_value(json!({
                    "apiVersion": "v1",
                    "kind": "Pod",
                    "metadata": {
                        "name": "api",
                        "namespace": "default",
                        "uid": "uid-pod"
                    },
                    "spec": {"containers": [{"name": "api"}]},
                    "status": {
                        "phase": "Running",
                        "containerStatuses": [{
                            "name": "api",
                            "ready": true,
                            "restartCount": 2,
                            "state": {"running": {}},
                            "lastState": {"terminated": {
                                "finishedAt": "2026-08-27T00:00:00Z"
                            }}
                        }]
                    }
                }))
                .unwrap(),
            ),
            ..Default::default()
        };

        let row = project_table_row("", "Pod", &layout, &table_row, None, None)
            .unwrap()
            .0;
        assert!(row.cells[3].starts_with("2\x1f"), "{:?}", row.cells[3]);
    }

    #[test]
    fn handled_crd_uses_projector_cells_before_server_only_columns() {
        let definitions = [
            column("Name", 0),
            column("Age", 0),
            column("Ready", 0),
            column("Status", 0),
            column("Revision", 1),
        ];
        let layout = table_layout(
            "kustomize.toolkit.fluxcd.io",
            "Kustomization",
            false,
            &definitions,
        );
        assert_eq!(
            layout.columns,
            ["Name", "Ready", "Status", "Message", "Revision", "Age"]
        );

        let table_row = TableRow {
            cells: vec![
                json!("apps"),
                json!("2h"),
                json!("True"),
                json!("Applied revision main@sha1:abc"),
                json!("main@sha1:abc"),
            ],
            object: Some(
                serde_json::from_value(json!({
                    "apiVersion": "kustomize.toolkit.fluxcd.io/v1",
                    "kind": "Kustomization",
                    "metadata": {
                        "name": "apps",
                        "uid": "uid-2",
                        "creationTimestamp": "2026-08-20T10:00:00Z"
                    },
                    "status": {"conditions": [{
                        "type": "Ready",
                        "status": "True",
                        "reason": "ReconciliationSucceeded",
                        "message": "Applied revision main@sha1:abc"
                    }]}
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let row = project_table_row(
            "kustomize.toolkit.fluxcd.io",
            "Kustomization",
            &layout,
            &table_row,
            None,
            None,
        )
        .unwrap()
        .0;
        assert_eq!(
            row.cells,
            [
                "apps",
                "True",
                "ReconciliationSucceeded",
                "Applied revision main@sha1:abc",
                "main@sha1:abc",
                "2026-08-20T10:00:00Z",
            ]
        );
    }

    #[test]
    fn external_secret_gets_projected_columns_and_age_when_server_omits_age() {
        let definitions = [
            column("Name", 0),
            column("StoreType", 0),
            column("Store", 0),
            column("Ready", 0),
            column("Last Sync", 0),
        ];
        let layout = table_layout("external-secrets.io", "ExternalSecret", true, &definitions);
        assert_eq!(
            layout.columns,
            [
                "Namespace",
                "Name",
                "Store Type",
                "Store",
                "Refresh Interval",
                "Status",
                "Ready",
                "Last Sync",
                "Age",
            ]
        );

        let table_row = TableRow {
            cells: vec![
                json!("database"),
                json!("ClusterSecretStore"),
                json!("vault"),
                json!("True"),
                json!("5m"),
            ],
            object: Some(
                serde_json::from_value(json!({
                    "apiVersion": "external-secrets.io/v1",
                    "kind": "ExternalSecret",
                    "metadata": {
                        "name": "database",
                        "namespace": "default",
                        "uid": "uid-3",
                        "creationTimestamp": "2026-08-01T00:00:00Z"
                    },
                    "spec": {
                        "secretStoreRef": {"kind": "ClusterSecretStore", "name": "vault"},
                        "refreshInterval": "1h"
                    },
                    "status": {
                        "refreshTime": "2026-08-26T10:00:00Z",
                        "conditions": [{
                            "type": "Ready",
                            "status": "True",
                            "reason": "SecretSynced"
                        }]
                    }
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let row = project_table_row(
            "external-secrets.io",
            "ExternalSecret",
            &layout,
            &table_row,
            None,
            None,
        )
        .unwrap()
        .0;
        assert_eq!(row.cells[2], "ClusterSecretStore");
        assert_eq!(row.cells[5], "SecretSynced");
        assert_eq!(row.cells[7], "2026-08-26T10:00:00Z");
        assert_eq!(row.cells[8], "2026-08-01T00:00:00Z");
    }

    #[test]
    fn route_projector_formats_arrays_and_unknown_conditions_as_pending() {
        let definitions = [column("Name", 0), column("Hostnames", 0), column("AGE", 0)];
        let layout = table_layout("gateway.networking.k8s.io", "HTTPRoute", true, &definitions);
        assert_eq!(
            layout.columns,
            [
                "Namespace",
                "Name",
                "Hostnames",
                "Gateways",
                "Status",
                "Age"
            ]
        );

        let table_row = TableRow {
            cells: vec![json!("api"), json!(["b.example", "a.example"]), json!("2h")],
            object: Some(
                serde_json::from_value(json!({
                    "apiVersion": "gateway.networking.k8s.io/v1",
                    "kind": "HTTPRoute",
                    "metadata": {
                        "name": "api",
                        "namespace": "default",
                        "uid": "uid-4",
                        "creationTimestamp": "2026-08-26T08:00:00Z"
                    },
                    "spec": {
                        "hostnames": ["b.example", "a.example"],
                        "parentRefs": [{"name": "public"}]
                    },
                    "status": {"parents": [{"conditions": [{
                        "type": "Accepted",
                        "status": "Unknown",
                        "message": "Waiting for controller"
                    }]}]}
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let row = project_table_row(
            "gateway.networking.k8s.io",
            "HTTPRoute",
            &layout,
            &table_row,
            None,
            None,
        )
        .unwrap()
        .0;
        assert_eq!(row.cells[2], "a.example\nb.example");
        assert_eq!(row.cells[3], "public");
        assert_eq!(row.cells[4], "Waiting for controller");
        assert_eq!(row.status, RowStatus::Pending);
    }

    #[test]
    fn denied_certificate_request_is_an_error() {
        let data = json!({
            "spec": {"issuerRef": {"name": "issuer"}},
            "status": {"conditions": [{"type": "Denied", "status": "True"}]}
        });
        assert_eq!(certrequest_cells(&data).1, RowStatus::Error);
    }

    #[test]
    fn cluster_external_secret_reads_nested_store_reference() {
        let data = json!({
            "spec": {"externalSecretSpec": {"secretStoreRef": {"name": "vault"}}}
        });
        assert_eq!(cluster_external_secret_cells(&data).0[2], "vault");
    }

    #[test]
    fn udp_route_has_parent_route_columns() {
        assert_eq!(
            explicit_view("gateway.networking.k8s.io", "UDPRoute")
                .unwrap()
                .headers,
            ["Gateways", "Status"]
        );
    }

    #[test]
    fn flux_suspension_is_separate_from_warning_status() {
        assert!(flux_suspended(
            "helm.toolkit.fluxcd.io",
            &json!({"spec": {"suspend": true}})
        ));
        assert!(!flux_suspended(
            "helm.toolkit.fluxcd.io",
            &json!({"metadata": {"deletionTimestamp": "2026-08-26T10:00:00Z"}})
        ));
    }
}
