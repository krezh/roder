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
mod cnpg;
mod core;
mod eso;
mod flux;
mod format;
mod gateway;
mod pods;
mod prometheus;
mod rbac;
mod rook;
mod status;
mod workloads;

pub(crate) use self::accessors::{parse_timestamp, ts_string};
use self::certmanager::{acme_state_cells, certificate_cells, certrequest_cells, issuer_cells};
use self::cnpg::{backup_cells, cluster_cells, pooler_cells, scheduled_backup_cells};
use self::core::{
    configmap_cells, endpoints_cells, endpointslice_cells, event_cells, hpa_cells, ingress_cells,
    namespace_cells, node_cells, pdb_cells, pv_cells, pvc_cells, secret_cells, service_cells,
    storageclass_cells,
};
use self::eso::{cluster_external_secret_cells, eso_generic_cells, external_secret_cells};
use self::flux::ready_message_cells;
use self::gateway::{gateway_cells, gatewayclass_cells, httproute_cells, parent_route_cells};
use self::pods::pod_cells;
use self::prometheus::{
    alertmanager_cells, prometheus_agent_cells, prometheus_cells, thanos_ruler_cells,
};
use self::rbac::rolebinding_cells;
use self::rook::{ceph_cluster_cells, ceph_resource_cells, object_bucket_claim_cells};
use self::status::generic_status;
pub(crate) use self::status::{condition, job_lifecycle, JobLifecycle};
use self::workloads::{
    cronjob_cells, daemonset_cells, deployment_cells, job_cells, replicaset_cells,
    statefulset_cells,
};

/// One source of truth per kind: the extra column headers *and* the projector that
/// fills them, defined together in the same match arm so they can't drift apart.
#[derive(Clone, Copy)]
struct KindView {
    /// Column headers beyond the standard Name / Namespace / Age.
    headers: &'static [&'static str],
    project: Project,
    custom_columns: bool,
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
                custom_columns: false,
            }
        };
    }
    macro_rules! custom_view {
        ($headers:expr, $project:expr) => {
            KindView {
                headers: $headers,
                project: $project,
                custom_columns: true,
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
        ("apps", "Deployment") => view!(&["Ready", "Available"], Plain(deployment_cells)),
        ("apps", "StatefulSet") => view!(&["Ready", "Available"], Plain(statefulset_cells)),
        ("apps", "DaemonSet") => view!(&["Ready", "Available"], Plain(daemonset_cells)),
        ("apps", "ReplicaSet") => view!(&["Ready"], Plain(replicaset_cells)),
        ("batch", "Job") => view!(&["Completions", "Status"], Plain(job_cells)),
        ("batch", "CronJob") => view!(
            &[
                "Schedule",
                "Suspend",
                "Active",
                "Last Schedule",
                "Last Success",
                "Status"
            ],
            Plain(cronjob_cells)
        ),
        ("", "Service") => view!(&["Type", "ClusterIP"], Plain(service_cells)),
        ("", "Node") => view!(&["Status", "Version"], Plain(node_cells)),
        ("", "Namespace") => view!(&["Phase"], Plain(namespace_cells)),
        ("", "PersistentVolumeClaim") => {
            view!(&["Phase", "Capacity", "Usage", "Mount"], Project::Pvc)
        }
        // Flux: Ready (True/False) + the condition's reason (Status) + its message.
        _ if group.ends_with("fluxcd.io") => {
            custom_view!(&["Ready", "Status", "Message"], Plain(ready_message_cells))
        }
        ("external-secrets.io", "ExternalSecret") => custom_view!(
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
            custom_view!(
                &["Ready", "Status", "Store"],
                Plain(cluster_external_secret_cells)
            )
        }
        ("external-secrets.io", _) => {
            custom_view!(&["Ready", "Status"], Plain(eso_generic_cells))
        }
        ("cert-manager.io", "Certificate") => {
            custom_view!(
                &["Ready", "Status", "Expires", "Renews", "Revision", "Secret"],
                Plain(certificate_cells)
            )
        }
        ("cert-manager.io", "ClusterIssuer") | ("cert-manager.io", "Issuer") => {
            custom_view!(&["Ready", "Status"], Plain(issuer_cells))
        }
        ("cert-manager.io", "CertificateRequest") => {
            custom_view!(&["Approved", "Ready", "Issuer"], Plain(certrequest_cells))
        }
        ("acme.cert-manager.io", "Order") | ("acme.cert-manager.io", "Challenge") => {
            custom_view!(&["State", "Reason"], Plain(acme_state_cells))
        }
        ("ceph.rook.io", "CephCluster") => custom_view!(
            &["Phase", "Health", "Version", "Message"],
            Plain(ceph_cluster_cells)
        ),
        ("ceph.rook.io", "CephBlockPool")
        | ("ceph.rook.io", "CephFilesystem")
        | ("ceph.rook.io", "CephNFS")
        | ("ceph.rook.io", "CephObjectStore") => {
            custom_view!(&["Phase", "Message"], Plain(ceph_resource_cells))
        }
        ("objectbucket.io", "ObjectBucketClaim") => custom_view!(
            &["Phase", "Storage Class", "Bucket"],
            Plain(object_bucket_claim_cells)
        ),
        ("postgresql.cnpg.io", "Cluster") => custom_view!(
            &["Instances", "Ready", "Primary", "Phase", "Image"],
            Plain(cluster_cells)
        ),
        ("postgresql.cnpg.io", "Backup") => custom_view!(
            &["Cluster", "Method", "Phase", "Started", "Completed"],
            Plain(backup_cells)
        ),
        ("postgresql.cnpg.io", "ScheduledBackup") => custom_view!(
            &[
                "Cluster",
                "Schedule",
                "Suspended",
                "Last Schedule",
                "Next Schedule",
                "Error"
            ],
            Plain(scheduled_backup_cells)
        ),
        ("postgresql.cnpg.io", "Pooler") => custom_view!(
            &["Cluster", "Type", "Instances", "Phase", "Reason"],
            Plain(pooler_cells)
        ),
        ("monitoring.coreos.com", "Prometheus") => custom_view!(
            &[
                "Version",
                "Desired",
                "Ready",
                "Reconciled",
                "Available",
                "Paused"
            ],
            Plain(prometheus_cells)
        ),
        ("monitoring.coreos.com", "PrometheusAgent") => custom_view!(
            &[
                "Version",
                "Desired",
                "Ready",
                "Reconciled",
                "Available",
                "Paused"
            ],
            Plain(prometheus_agent_cells)
        ),
        ("monitoring.coreos.com", "Alertmanager") => custom_view!(
            &[
                "Version",
                "Replicas",
                "Ready",
                "Reconciled",
                "Available",
                "Paused"
            ],
            Plain(alertmanager_cells)
        ),
        ("monitoring.coreos.com", "ThanosRuler") => custom_view!(
            &[
                "Version",
                "Replicas",
                "Ready",
                "Reconciled",
                "Available",
                "Paused"
            ],
            Plain(thanos_ruler_cells)
        ),
        // Gateway API: HTTPRoute plus its Gateway/GatewayClass and sibling route kinds.
        ("gateway.networking.k8s.io", "HTTPRoute")
        | ("gateway.networking.k8s.io", "GRPCRoute")
        | ("gateway.networking.k8s.io", "TLSRoute") => {
            custom_view!(&["Hostnames", "Gateways", "Status"], Plain(httproute_cells))
        }
        ("gateway.networking.k8s.io", "TCPRoute") | ("gateway.networking.k8s.io", "UDPRoute") => {
            custom_view!(&["Gateways", "Status"], Plain(parent_route_cells))
        }
        ("gateway.networking.k8s.io", "Gateway") => {
            custom_view!(&["Class", "Address", "Programmed"], Plain(gateway_cells))
        }
        ("gateway.networking.k8s.io", "GatewayClass") => {
            custom_view!(&["Controller", "Accepted"], Plain(gatewayclass_cells))
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
        ("", "Event") => view!(
            &["Last Seen", "Type", "Reason", "Object", "Message"],
            Plain(event_cells)
        ),
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
    let handled_crd = explicit_view(group, kind).is_some_and(|view| view.custom_columns);
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
    let projection_data = serde_json::to_value(&object).unwrap_or_else(|_| object.data.clone());
    let (enhancement_cells, enhancement_trends, mut status) =
        enhancement_values(group, kind, &projection_data, deleting, usage, pvc_usage);
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
    let projection_data = serde_json::to_value(object).unwrap_or_else(|_| object.data.clone());
    let (enhancement_cells, enhancement_trends, mut status) =
        enhancement_values(group, kind, &projection_data, deleting, usage, pvc_usage);
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
        ("", "Event") | ("batch", "CronJob") => explicit_view(group, kind)
            .map(|view| view.headers)
            .unwrap_or(&[]),
        _ => explicit_view(group, kind)
            .filter(|view| view.custom_columns)
            .map(|view| view.headers)
            .unwrap_or(&[]),
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
        ("", "Event") => {
            let (cells, status) = event_cells(data);
            (cells, vec![], status)
        }
        ("batch", "CronJob") => {
            let (cells, status) = cronjob_cells(data);
            (cells, vec![], status)
        }
        _ if explicit_view(group, kind).is_some_and(|view| view.custom_columns) => {
            match explicit_view(group, kind) {
                Some(view) => match view.project {
                    Project::Plain(project) => {
                        let (cells, status) = project(data);
                        (cells, vec![], status)
                    }
                    Project::Pod | Project::Pvc => unreachable!("CRDs use plain projectors"),
                },
                None => (vec![], vec![], generic_status(data)),
            }
        }
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

/// Classify one object through the same GVK dispatch used by resource-list rows.
pub(crate) fn resource_status(group: &str, kind: &str, object: &DynamicObject) -> RowStatus {
    let data = serde_json::to_value(object).unwrap_or_else(|_| object.data.clone());
    enhancement_values(
        group,
        kind,
        &data,
        object.metadata.deletion_timestamp.is_some(),
        None,
        None,
    )
    .2
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
    fn explicit_projectors_cover_every_registered_kind() {
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "",
                "Pod",
                &[
                    "Ready", "Status", "Restarts", "CPU", "%CPU/R", "%CPU/L", "MEM", "%MEM/R",
                    "%MEM/L", "IP", "Node",
                ],
            ),
            ("apps", "Deployment", &["Ready", "Available"]),
            ("apps", "StatefulSet", &["Ready", "Available"]),
            ("apps", "DaemonSet", &["Ready", "Available"]),
            ("apps", "ReplicaSet", &["Ready"]),
            ("batch", "Job", &["Completions", "Status"]),
            (
                "batch",
                "CronJob",
                &[
                    "Schedule",
                    "Suspend",
                    "Active",
                    "Last Schedule",
                    "Last Success",
                    "Status",
                ],
            ),
            ("", "Service", &["Type", "ClusterIP"]),
            ("", "Node", &["Status", "Version"]),
            ("", "Namespace", &["Phase"]),
            (
                "",
                "PersistentVolumeClaim",
                &["Phase", "Capacity", "Usage", "Mount"],
            ),
            (
                "kustomize.toolkit.fluxcd.io",
                "Kustomization",
                &["Ready", "Status", "Message"],
            ),
            (
                "source.toolkit.fluxcd.io",
                "GitRepository",
                &["Ready", "Status", "Message"],
            ),
            (
                "external-secrets.io",
                "ExternalSecret",
                &[
                    "Store Type",
                    "Store",
                    "Refresh Interval",
                    "Status",
                    "Ready",
                    "Last Sync",
                ],
            ),
            (
                "external-secrets.io",
                "ClusterExternalSecret",
                &["Ready", "Status", "Store"],
            ),
            ("external-secrets.io", "SecretStore", &["Ready", "Status"]),
            (
                "cert-manager.io",
                "Certificate",
                &["Ready", "Status", "Expires", "Renews", "Revision", "Secret"],
            ),
            ("cert-manager.io", "Issuer", &["Ready", "Status"]),
            ("cert-manager.io", "ClusterIssuer", &["Ready", "Status"]),
            (
                "cert-manager.io",
                "CertificateRequest",
                &["Approved", "Ready", "Issuer"],
            ),
            ("acme.cert-manager.io", "Order", &["State", "Reason"]),
            ("acme.cert-manager.io", "Challenge", &["State", "Reason"]),
            (
                "ceph.rook.io",
                "CephCluster",
                &["Phase", "Health", "Version", "Message"],
            ),
            ("ceph.rook.io", "CephBlockPool", &["Phase", "Message"]),
            ("ceph.rook.io", "CephFilesystem", &["Phase", "Message"]),
            ("ceph.rook.io", "CephNFS", &["Phase", "Message"]),
            ("ceph.rook.io", "CephObjectStore", &["Phase", "Message"]),
            (
                "objectbucket.io",
                "ObjectBucketClaim",
                &["Phase", "Storage Class", "Bucket"],
            ),
            (
                "postgresql.cnpg.io",
                "Cluster",
                &["Instances", "Ready", "Primary", "Phase", "Image"],
            ),
            (
                "postgresql.cnpg.io",
                "Backup",
                &["Cluster", "Method", "Phase", "Started", "Completed"],
            ),
            (
                "postgresql.cnpg.io",
                "ScheduledBackup",
                &[
                    "Cluster",
                    "Schedule",
                    "Suspended",
                    "Last Schedule",
                    "Next Schedule",
                    "Error",
                ],
            ),
            (
                "postgresql.cnpg.io",
                "Pooler",
                &["Cluster", "Type", "Instances", "Phase", "Reason"],
            ),
            (
                "monitoring.coreos.com",
                "Prometheus",
                &[
                    "Version",
                    "Desired",
                    "Ready",
                    "Reconciled",
                    "Available",
                    "Paused",
                ],
            ),
            (
                "monitoring.coreos.com",
                "PrometheusAgent",
                &[
                    "Version",
                    "Desired",
                    "Ready",
                    "Reconciled",
                    "Available",
                    "Paused",
                ],
            ),
            (
                "monitoring.coreos.com",
                "Alertmanager",
                &[
                    "Version",
                    "Replicas",
                    "Ready",
                    "Reconciled",
                    "Available",
                    "Paused",
                ],
            ),
            (
                "monitoring.coreos.com",
                "ThanosRuler",
                &[
                    "Version",
                    "Replicas",
                    "Ready",
                    "Reconciled",
                    "Available",
                    "Paused",
                ],
            ),
            (
                "gateway.networking.k8s.io",
                "HTTPRoute",
                &["Hostnames", "Gateways", "Status"],
            ),
            (
                "gateway.networking.k8s.io",
                "GRPCRoute",
                &["Hostnames", "Gateways", "Status"],
            ),
            (
                "gateway.networking.k8s.io",
                "TLSRoute",
                &["Hostnames", "Gateways", "Status"],
            ),
            (
                "gateway.networking.k8s.io",
                "TCPRoute",
                &["Gateways", "Status"],
            ),
            (
                "gateway.networking.k8s.io",
                "UDPRoute",
                &["Gateways", "Status"],
            ),
            (
                "gateway.networking.k8s.io",
                "Gateway",
                &["Class", "Address", "Programmed"],
            ),
            (
                "gateway.networking.k8s.io",
                "GatewayClass",
                &["Controller", "Accepted"],
            ),
            (
                "",
                "PersistentVolume",
                &[
                    "Capacity",
                    "Access",
                    "Reclaim",
                    "Status",
                    "Claim",
                    "StorageClass",
                ],
            ),
            (
                "storage.k8s.io",
                "StorageClass",
                &["Provisioner", "Reclaim", "Binding Mode", "Expandable"],
            ),
            ("", "Secret", &["Type", "Data"]),
            ("", "ConfigMap", &["Data"]),
            (
                "",
                "Event",
                &["Last Seen", "Type", "Reason", "Object", "Message"],
            ),
            ("", "Endpoints", &["Endpoints"]),
            (
                "discovery.k8s.io",
                "EndpointSlice",
                &["Address Type", "Endpoints", "Ports"],
            ),
            (
                "networking.k8s.io",
                "Ingress",
                &["Class", "Hosts", "Address"],
            ),
            (
                "autoscaling",
                "HorizontalPodAutoscaler",
                &["Reference", "Targets", "Min", "Max", "Replicas"],
            ),
            (
                "policy",
                "PodDisruptionBudget",
                &["Min Available", "Max Unavailable", "Allowed"],
            ),
            ("rbac.authorization.k8s.io", "RoleBinding", &["Role"]),
            ("rbac.authorization.k8s.io", "ClusterRoleBinding", &["Role"]),
        ];

        for (group, kind, headers) in cases {
            let view = explicit_view(group, kind)
                .unwrap_or_else(|| panic!("missing projector for {group}/{kind}"));
            assert_eq!(view.headers, *headers, "headers for {group}/{kind}");
            let cell_count = match view.project {
                Project::Plain(project) => project(&json!({})).0.len(),
                Project::Pod => pod_cells(&json!({}), false, None).0.len(),
                Project::Pvc => pvc_cells(&json!({}), None).0.len(),
            };
            assert_eq!(cell_count, headers.len(), "cell count for {group}/{kind}");
        }
    }

    #[test]
    fn unknown_kinds_use_generic_status_without_enhancements() {
        let data = json!({"status": {"conditions": [{"type": "Failed", "status": "True"}]}});
        for (group, kind) in [
            ("example.io", "Widget"),
            ("ceph.rook.io", "UnknownCephResource"),
        ] {
            assert!(explicit_view(group, kind).is_none());
            assert!(enhancement_headers(group, kind).is_empty());
            let (cells, trends, status) = enhancement_values(group, kind, &data, false, None, None);
            assert!(cells.is_empty());
            assert!(trends.is_empty());
            assert_eq!(status, RowStatus::Error);
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
    fn list_projection_preserves_generation_for_status() {
        let definitions = [column("Name", 0), column("Ready", 0), column("Age", 0)];
        let layout = table_layout("apps", "Deployment", false, &definitions);
        let table_row = TableRow {
            cells: vec![json!("api"), json!("3/3"), json!("1h")],
            object: Some(
                serde_json::from_value(json!({
                    "apiVersion": "apps/v1",
                    "kind": "Deployment",
                    "metadata": {"name": "api", "generation": 3, "uid": "uid-api"},
                    "spec": {"replicas": 3},
                    "status": {"observedGeneration": 2, "replicas": 3, "updatedReplicas": 3, "readyReplicas": 3, "availableReplicas": 3}
                }))
                .unwrap(),
            ),
            ..Default::default()
        };

        let row = project_table_row("apps", "Deployment", &layout, &table_row, None, None)
            .unwrap()
            .0;

        assert_eq!(row.status, RowStatus::Pending);
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
    fn rook_cluster_layout_combines_server_and_semantic_health_columns() {
        let definitions = [
            column("Name", 0),
            column("Phase", 0),
            column("Health", 0),
            column("Age", 0),
        ];
        let layout = table_layout("ceph.rook.io", "CephCluster", true, &definitions);
        assert_eq!(
            layout.columns,
            [
                "Namespace",
                "Name",
                "Phase",
                "Health",
                "Version",
                "Message",
                "Age"
            ]
        );

        let table_row = TableRow {
            cells: vec![json!("rook-ceph"), json!("Ready"), json!("HEALTH_WARN"), json!("2h")],
            object: Some(
                serde_json::from_value(json!({
                    "apiVersion": "ceph.rook.io/v1",
                    "kind": "CephCluster",
                    "metadata": {"name": "rook-ceph", "namespace": "storage", "uid": "rook-1", "creationTimestamp": "2026-09-01T00:00:00Z"},
                    "status": {"phase": "Ready", "ceph": {"health": "HEALTH_WARN"}, "version": {"version": "19.2.3"}, "message": "degraded redundancy"}
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let row = project_table_row(
            "ceph.rook.io",
            "CephCluster",
            &layout,
            &table_row,
            None,
            None,
        )
        .unwrap()
        .0;

        assert_eq!(row.status, RowStatus::Warn);
        assert_eq!(
            row.cells[0..6],
            [
                "storage",
                "rook-ceph",
                "Ready",
                "HEALTH_WARN",
                "19.2.3",
                "degraded redundancy"
            ]
        );
    }

    #[test]
    fn cnpg_cluster_layout_projects_topology_and_replica_health() {
        let definitions = [
            column("Name", 0),
            column("Instances", 0),
            column("Ready", 0),
            column("Phase", 0),
            column("Age", 0),
        ];
        let layout = table_layout("postgresql.cnpg.io", "Cluster", true, &definitions);
        assert_eq!(
            layout.columns,
            [
                "Namespace",
                "Name",
                "Instances",
                "Ready",
                "Primary",
                "Phase",
                "Image",
                "Age"
            ]
        );

        let table_row = TableRow {
            cells: vec![json!("app"), json!(3), json!(2), json!("Cluster in healthy state"), json!("4h")],
            object: Some(
                serde_json::from_value(json!({
                    "apiVersion": "postgresql.cnpg.io/v1",
                    "kind": "Cluster",
                    "metadata": {"name": "app", "namespace": "database", "uid": "cnpg-1", "creationTimestamp": "2026-09-01T00:00:00Z"},
                    "spec": {"instances": 3, "imageName": "ghcr.io/cloudnative-pg/postgresql:18"},
                    "status": {"readyInstances": 2, "currentPrimary": "app-1", "phase": "Cluster in healthy state"}
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let row = project_table_row(
            "postgresql.cnpg.io",
            "Cluster",
            &layout,
            &table_row,
            None,
            None,
        )
        .unwrap()
        .0;

        assert_eq!(row.status, RowStatus::Pending);
        assert_eq!(
            row.cells[0..7],
            [
                "database",
                "app",
                "3",
                "2/3",
                "app-1",
                "Cluster in healthy state",
                "ghcr.io/cloudnative-pg/postgresql:18"
            ]
        );
    }

    #[test]
    fn prometheus_layout_projects_replica_and_reconciliation_health() {
        let definitions = [
            column("Name", 0),
            column("Version", 0),
            column("Desired", 0),
            column("Ready", 0),
            column("Reconciled", 0),
            column("Available", 0),
            column("Age", 0),
            column("Paused", 1),
        ];
        let layout = table_layout("monitoring.coreos.com", "Prometheus", true, &definitions);
        assert_eq!(
            layout.columns,
            [
                "Namespace",
                "Name",
                "Version",
                "Desired",
                "Ready",
                "Reconciled",
                "Available",
                "Paused",
                "Age"
            ]
        );
        let table_row = TableRow {
            cells: vec![
                json!("platform"),
                json!("v3.5.0"),
                json!(2),
                json!(2),
                json!("True"),
                json!("True"),
                json!("1h"),
                json!(false),
            ],
            object: Some(
                serde_json::from_value(json!({
                    "apiVersion": "monitoring.coreos.com/v1",
                    "kind": "Prometheus",
                    "metadata": {
                        "name": "platform",
                        "namespace": "monitoring",
                        "uid": "prometheus-1",
                        "generation": 4,
                        "creationTimestamp": "2026-09-14T10:00:00Z"
                    },
                    "spec": {"version": "v3.5.0", "replicas": 2},
                    "status": {
                        "availableReplicas": 1,
                        "conditions": [
                            {"type": "Reconciled", "status": "True", "observedGeneration": 4},
                            {"type": "Available", "status": "Degraded", "observedGeneration": 4}
                        ]
                    }
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let row = project_table_row(
            "monitoring.coreos.com",
            "Prometheus",
            &layout,
            &table_row,
            None,
            None,
        )
        .unwrap()
        .0;

        assert_eq!(row.status, RowStatus::Warn);
        assert_eq!(
            row.cells,
            [
                "monitoring",
                "platform",
                "v3.5.0",
                "2",
                "1/2",
                "True",
                "Degraded",
                "false",
                "2026-09-14T10:00:00Z"
            ]
        );
    }

    #[test]
    fn cronjob_layout_replaces_server_snapshots_with_live_status() {
        let definitions = [
            column("Name", 0),
            column("Schedule", 0),
            column("Timezone", 0),
            column("Suspend", 0),
            column("Active", 0),
            column("Last Schedule", 0),
            column("Age", 0),
        ];
        let layout = table_layout("batch", "CronJob", true, &definitions);
        assert_eq!(
            layout.columns,
            [
                "Namespace",
                "Name",
                "Schedule",
                "Timezone",
                "Suspend",
                "Active",
                "Last Schedule",
                "Last Success",
                "Status",
                "Age"
            ]
        );
        let table_row = TableRow {
            cells: vec![
                json!("hourly"),
                json!("0 * * * *"),
                json!("UTC"),
                json!(false),
                json!(1),
                json!("5m"),
                json!("2d"),
            ],
            object: Some(
                serde_json::from_value(json!({
                    "apiVersion": "batch/v1",
                    "kind": "CronJob",
                    "metadata": {
                        "name": "hourly",
                        "namespace": "jobs",
                        "uid": "cron-1",
                        "creationTimestamp": "2026-09-14T10:00:00Z"
                    },
                    "spec": {"schedule": "0 * * * *", "timeZone": "UTC"},
                    "status": {
                        "active": [{"name": "hourly-123"}],
                        "lastScheduleTime": "2099-09-14T12:00:00Z",
                        "lastSuccessfulTime": "2099-09-14T11:00:10Z"
                    }
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let row = project_table_row("batch", "CronJob", &layout, &table_row, None, None)
            .unwrap()
            .0;

        assert_eq!(
            &row.cells[4..9],
            [
                "false",
                "hourly-123",
                "2099-09-14T12:00:00Z",
                "2099-09-14T11:00:10Z",
                "Active"
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
