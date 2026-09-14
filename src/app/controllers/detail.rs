use std::collections::{BTreeSet, HashMap};

use leptos::prelude::*;
use roder_core::{
    ActionPermissions, MetricsPoint, ObjectDetail, ResourceAction, ResourceRow, TalosConfigDiff,
    TalosNode,
};

use crate::app::hooks::use_sse_subscription;
use crate::app::state::{Catalog, DetailTarget};
use crate::app::util::json::json_str;
use crate::data;

#[derive(Clone, Default)]
pub(crate) struct SelectionPermissions {
    selected: usize,
    permitted: HashMap<String, usize>,
}

impl SelectionPermissions {
    pub(crate) fn allows_all(&self, action: ResourceAction) -> bool {
        self.selected > 0 && self.permitted.get(action.api_name()).copied() == Some(self.selected)
    }

    pub(crate) fn count(&self, action: ResourceAction) -> (usize, usize) {
        (
            self.permitted.get(action.api_name()).copied().unwrap_or(0),
            self.selected,
        )
    }
}

pub(crate) fn selection_permissions_resource(
    targets: impl Fn() -> Vec<DetailTarget> + Copy + Send + Sync + 'static,
) -> LocalResource<SelectionPermissions> {
    LocalResource::new(move || {
        let targets = targets();
        async move { fetch_selection_permissions(targets).await }
    })
}

pub(crate) async fn fetch_permissions(target: &DetailTarget) -> Permissions {
    data::fetch_json::<ActionPermissions>(&format!(
        "/api/permissions?key={}&namespace={}&name={}",
        target.key,
        target
            .namespace
            .as_deref()
            .map(data::percent_encode)
            .unwrap_or_default(),
        data::percent_encode(&target.name),
    ))
    .await
    .map(|inner| Permissions { inner })
    .unwrap_or_default()
}

pub(crate) async fn fetch_selection_permissions(
    targets: Vec<DetailTarget>,
) -> SelectionPermissions {
    let mut selection = SelectionPermissions {
        selected: targets.len(),
        ..Default::default()
    };
    for target in targets {
        let permissions = fetch_permissions(&target).await;
        for action in ResourceAction::ALL {
            if permissions.allows(action) {
                *selection
                    .permitted
                    .entry(action.api_name().to_string())
                    .or_default() += 1;
            }
        }
    }
    selection
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DetailTab {
    Info,
    Yaml,
    Logs,
    Metrics,
    Talos,
    Jobs,
}

#[derive(Clone, Default)]
pub(crate) struct Permissions {
    inner: ActionPermissions,
}

impl Permissions {
    pub(crate) fn allows(&self, action: ResourceAction) -> bool {
        self.inner.allows(action)
    }

    pub(crate) fn apply(&self) -> bool {
        self.inner.apply
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ResourceDetailController {
    pub(crate) object: LocalResource<Option<ObjectDetail>>,
    pub(crate) permissions: LocalResource<Permissions>,
    pub(crate) status: RwSignal<Option<Result<String, String>>>,
}

impl ResourceDetailController {
    pub(crate) fn new(target: DetailTarget) -> Self {
        let object_target = target.clone();
        let object = LocalResource::new(move || {
            let target = object_target.clone();
            async move {
                data::fetch_json(&data::detail_url(
                    &target.key,
                    target.namespace.as_deref(),
                    &target.name,
                ))
                .await
                .ok()
            }
        });
        let permission_target = target;
        let permissions = LocalResource::new(move || {
            let target = permission_target.clone();
            async move { fetch_permissions(&target).await }
        });
        Self {
            object,
            permissions,
            status: RwSignal::new(None),
        }
    }

    pub(crate) fn run(
        self,
        target: DetailTarget,
        action: &'static str,
        extra: serde_json::Value,
        on_delete: impl Fn() + Send + Sync + 'static,
    ) {
        leptos::task::spawn_local(async move {
            let mut body = serde_json::json!({
                "action": action,
                "key": target.key,
                "namespace": target.namespace,
                "name": target.name,
            });
            if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
                body.extend(extra.clone());
            }
            match data::post_action(&body).await {
                Ok(_) => {
                    self.status.set(Some(Ok(format!("{action} requested"))));
                    if action == "delete" {
                        on_delete();
                    }
                    if matches!(action, "flux-suspend" | "flux-resume" | "certificate-renew") {
                        self.object.refetch();
                    }
                }
                Err(error) => self.status.set(Some(Err(error))),
            }
        });
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PodWatch {
    pub(crate) rows: RwSignal<HashMap<String, ResourceRow>>,
    pub(crate) shown_uids: Memo<Vec<String>>,
    pub(crate) pod_kind: Memo<Option<roder_core::ResourceKind>>,
}

pub(crate) fn use_pod_watch(namespace: String, selector: String) -> PodWatch {
    let catalog = expect_context::<Catalog>().0;
    let pod_kind = Memo::new(move |_| {
        catalog
            .get()
            .into_iter()
            .find(|kind| kind.group.is_empty() && kind.kind == "Pod")
    });
    let rows = RwSignal::new(HashMap::new());
    let entering = RwSignal::new(BTreeSet::new());
    let removing = RwSignal::new(BTreeSet::new());
    use_sse_subscription(rows, entering, removing, None, move || {
        rows.set(HashMap::new());
        let kind = pod_kind.get()?;
        Some(data::watch_url(
            &kind.key,
            Some(&namespace),
            Some(&selector),
        ))
    });
    let shown_uids = Memo::new(move |_| {
        rows.with(|rows| {
            let mut rows = rows.values().collect::<Vec<_>>();
            rows.sort_by(|a, b| a.name.cmp(&b.name));
            rows.into_iter().map(|row| row.uid.clone()).collect()
        })
    });
    PodWatch {
        rows,
        shown_uids,
        pod_kind,
    }
}

pub(crate) fn use_metrics(namespace: String, name: String) -> RwSignal<Option<Vec<MetricsPoint>>> {
    let refresh = RwSignal::new(0u64);
    let reconnect = RwSignal::new(0u64);
    let watch_namespace = namespace.clone();
    let watch_name = name.clone();
    Effect::new(move |_previous: Option<Option<data::SseHandle>>| {
        reconnect.track();
        let watched_name = watch_name.clone();
        data::subscribe_with_error(
            &data::watch_url("/v1/Pod", Some(&watch_namespace), None),
            move |event| {
                if let roder_core::WatchEvent::Applied { row } = event {
                    if row.name == watched_name {
                        refresh.update(|tick| *tick = tick.wrapping_add(1));
                    }
                }
            },
            move || {
                set_timeout(
                    move || reconnect.update(|attempt| *attempt = attempt.wrapping_add(1)),
                    data::reconnect_delay(),
                )
            },
        )
    });
    let resource = LocalResource::new(move || {
        refresh.track();
        let namespace = namespace.clone();
        let name = name.clone();
        async move {
            data::fetch_json::<Vec<MetricsPoint>>(&format!(
                "/api/metrics?namespace={namespace}&name={name}"
            ))
            .await
            .ok()
        }
    });
    let visible = RwSignal::new(None);
    Effect::new(move |_| {
        if let Some(points) = resource.get().flatten() {
            if visible.get_untracked().as_ref() != Some(&points) {
                visible.set(Some(points));
            }
        }
    });
    visible
}

pub(crate) fn talos_node(node: String) -> LocalResource<Result<TalosNode, String>> {
    LocalResource::new(move || {
        let node = node.clone();
        async move {
            data::fetch_json(&format!(
                "/api/talos/node?node={}",
                data::percent_encode(&node)
            ))
            .await
        }
    })
}

pub(crate) fn talos_config_diff(
    node: String,
    enabled: RwSignal<bool>,
) -> LocalResource<Result<Option<TalosConfigDiff>, String>> {
    LocalResource::new(move || {
        let node = node.clone();
        async move {
            if !enabled.get() {
                return Ok(None);
            }
            data::fetch_json(&format!(
                "/api/talos/config-diff?node={}",
                data::percent_encode(&node)
            ))
            .await
            .map(Some)
        }
    })
}

pub(crate) fn talos_action(
    node: String,
    action: String,
    service: Option<String>,
    status: RwSignal<Option<Result<String, String>>>,
    pending: RwSignal<Option<String>>,
    refresh: Option<Callback<()>>,
) {
    leptos::task::spawn_local(async move {
        status.set(None);
        pending.set(Some(action.clone()));
        let mut body = serde_json::json!({"action": action, "name": node});
        if let Some(service) = service {
            if let Some(body) = body.as_object_mut() {
                body.insert("service".into(), service.into());
            }
        }
        let result = data::post_action(&body)
            .await
            .map(|_| "request accepted".into());
        if result.is_ok() {
            if let Some(refresh) = refresh {
                refresh.run(());
            }
        }
        status.set(Some(result));
        pending.set(None);
    });
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub(crate) fn short_fingerprint(fingerprint: &str) -> String {
    fingerprint.chars().take(12).collect()
}

pub(crate) struct CertificateSummary {
    pub(crate) state: String,
    pub(crate) state_class: &'static str,
    pub(crate) not_before: String,
    pub(crate) not_before_raw: String,
    pub(crate) not_after: String,
    pub(crate) not_after_raw: String,
    pub(crate) renewal_time: String,
    pub(crate) renewal_time_raw: String,
    pub(crate) revision: String,
    pub(crate) secret: String,
}

pub(crate) struct CnpgClusterSummary {
    pub(crate) phase: String,
    pub(crate) phase_class: &'static str,
    pub(crate) phase_reason: String,
    pub(crate) topology: String,
    pub(crate) topology_note: String,
    pub(crate) primary: String,
    pub(crate) primary_note: String,
    pub(crate) image: String,
    pub(crate) storage: String,
    pub(crate) storage_note: String,
    pub(crate) replication: String,
    pub(crate) replication_note: String,
}

pub(crate) fn cnpg_cluster_summary(object: &serde_json::Value) -> Option<CnpgClusterSummary> {
    let api_version = json_str(object, &["apiVersion"])?;
    if json_str(object, &["kind"]).as_deref() != Some("Cluster")
        || api_version.split('/').next() != Some("postgresql.cnpg.io")
    {
        return None;
    }

    let desired = json_str(object, &["spec", "instances"])
        .or_else(|| json_str(object, &["status", "instances"]))
        .unwrap_or_else(|| "1".to_string());
    let ready = json_str(object, &["status", "readyInstances"]).unwrap_or_else(|| "0".into());
    let phase = json_str(object, &["status", "phase"]).unwrap_or_else(|| "Not reported".into());
    let phase_reason = json_str(object, &["status", "phaseReason"]).unwrap_or_default();
    let phase_class = cnpg_phase_class(&phase, &ready, &desired);
    let nodes = json_str(object, &["status", "topology", "nodesUsed"]);
    let current_primary = json_str(object, &["status", "currentPrimary"]);
    let target_primary = json_str(object, &["status", "targetPrimary"])
        .filter(|target| Some(target) != current_primary.as_ref());
    let image = json_str(object, &["status", "image"])
        .or_else(|| json_str(object, &["spec", "imageName"]))
        .unwrap_or_else(|| "-".into());
    let storage = json_str(object, &["spec", "storage", "size"])
        .or_else(|| {
            json_str(
                object,
                &[
                    "spec",
                    "storage",
                    "pvcTemplate",
                    "resources",
                    "requests",
                    "storage",
                ],
            )
        })
        .unwrap_or_else(|| "-".into());
    let storage_class = json_str(object, &["spec", "storage", "storageClass"]).or_else(|| {
        json_str(
            object,
            &["spec", "storage", "pvcTemplate", "storageClassName"],
        )
    });
    let wal_size = json_str(object, &["spec", "walStorage", "size"]).or_else(|| {
        json_str(
            object,
            &[
                "spec",
                "walStorage",
                "pvcTemplate",
                "resources",
                "requests",
                "storage",
            ],
        )
    });
    let wal_class = json_str(object, &["spec", "walStorage", "storageClass"]).or_else(|| {
        json_str(
            object,
            &["spec", "walStorage", "pvcTemplate", "storageClassName"],
        )
    });
    let mut storage_notes = Vec::new();
    if let Some(class) = storage_class {
        storage_notes.push(format!("{class} storage class"));
    }
    if wal_size.is_some() || wal_class.is_some() {
        storage_notes.push(format!(
            "WAL {}{}",
            wal_size.unwrap_or_else(|| "storage".into()),
            wal_class
                .map(|class| format!(" on {class}"))
                .unwrap_or_default()
        ));
    }
    let ephemeral = object
        .get("spec")
        .and_then(|spec| spec.get("ephemeralVolumeSource"));
    let (storage, storage_note) = if ephemeral.is_some() {
        ("Ephemeral".to_string(), "data is not persisted".to_string())
    } else {
        (storage, storage_notes.join("; "))
    };
    let status_count = |name: &str| {
        object
            .pointer(&format!("/status/instancesStatus/{name}"))
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len)
    };
    let instances_status_reported = object
        .pointer("/status/instancesStatus")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|statuses| !statuses.is_empty());
    let desired_count = desired.parse::<usize>().unwrap_or(1);
    let standbys = desired_count.saturating_sub(1);
    let replicating = status_count("replicating");
    let failed = status_count("failed");
    let sync_topology = object
        .pointer("/status/conditions")
        .and_then(serde_json::Value::as_array)
        .and_then(|conditions| {
            conditions.iter().rev().find(|condition| {
                condition.get("type").and_then(serde_json::Value::as_str)
                    == Some("SyncReplicationTopologySatisfied")
            })
        });
    let (replication, replication_note) = if failed > 0 {
        (
            format!("{failed} failed"),
            format!("{replicating}/{standbys} standbys replicating"),
        )
    } else if instances_status_reported {
        let note = match sync_topology
            .and_then(|condition| condition.get("status"))
            .and_then(serde_json::Value::as_str)
        {
            Some("False") => "synchronous topology is not satisfied",
            Some("True") => "synchronous topology satisfied",
            _ => "streaming status reported by the operator",
        };
        (format!("{replicating}/{standbys} replicating"), note.into())
    } else {
        (
            "Not reported".into(),
            format!("{ready}/{desired} instances ready"),
        )
    };

    Some(CnpgClusterSummary {
        phase,
        phase_class,
        phase_reason,
        topology: format!("{desired} instances"),
        topology_note: nodes
            .map(|nodes| format!("across {nodes} nodes"))
            .unwrap_or_default(),
        primary: current_primary.unwrap_or_else(|| "-".into()),
        primary_note: target_primary
            .map(|target| format!("switching to {target}"))
            .unwrap_or_default(),
        image,
        storage,
        storage_note,
        replication,
        replication_note,
    })
}

fn cnpg_phase_class(phase: &str, ready: &str, desired: &str) -> &'static str {
    let phase = phase.to_ascii_lowercase();
    if phase.contains("unrecoverable")
        || phase.contains("invalid")
        || phase.contains("unable to create")
        || phase.contains("plugin rollout failed")
        || phase.contains("unknown state")
        || phase.contains("cannot proceed")
        || phase.contains("unknown plugin")
        || phase.contains("missing architecture")
        || phase.contains("interaction failed")
    {
        "error"
    } else if phase.contains("degraded")
        || phase.contains("upgrade delayed")
        || phase.contains("waiting for user action")
    {
        "warning"
    } else if phase.contains("healthy") && ready == desired {
        "ok"
    } else {
        "pending"
    }
}

pub(crate) fn certificate_summary(object: &serde_json::Value) -> CertificateSummary {
    let condition = |type_: &str| {
        object
            .pointer("/status/conditions")
            .and_then(serde_json::Value::as_array)
            .and_then(|conditions| {
                conditions.iter().find(|condition| {
                    condition.get("type").and_then(serde_json::Value::as_str) == Some(type_)
                })
            })
    };
    let issuing = condition("Issuing").is_some_and(|condition| {
        condition.get("status").and_then(serde_json::Value::as_str) == Some("True")
    });
    let ready = condition("Ready");
    let ready_status = ready
        .and_then(|condition| condition.get("status"))
        .and_then(serde_json::Value::as_str);
    let ready_reason = ready
        .and_then(|condition| condition.get("reason"))
        .and_then(serde_json::Value::as_str);
    let (state, state_class) = if issuing {
        ("Renewing".to_string(), "pending")
    } else if ready_reason.is_some_and(|reason| reason.eq_ignore_ascii_case("expired")) {
        ("Expired".to_string(), "error")
    } else {
        match ready_status {
            Some("True") => ("Valid".to_string(), "ok"),
            Some("False") => (ready_reason.unwrap_or("Failed").to_string(), "error"),
            _ => ("Pending".to_string(), "pending"),
        }
    };
    let not_before_raw = json_str(object, &["status", "notBefore"]).unwrap_or_default();
    let not_after_raw = json_str(object, &["status", "notAfter"]).unwrap_or_default();
    let renewal_time_raw = json_str(object, &["status", "renewalTime"]).unwrap_or_default();

    CertificateSummary {
        state,
        state_class,
        not_before: display_certificate_time(&not_before_raw),
        not_before_raw,
        not_after: display_certificate_time(&not_after_raw),
        not_after_raw,
        renewal_time: display_certificate_time(&renewal_time_raw),
        renewal_time_raw,
        revision: json_str(object, &["status", "revision"]).unwrap_or_else(|| "-".to_string()),
        secret: json_str(object, &["spec", "secretName"]).unwrap_or_else(|| "-".to_string()),
    }
}

fn display_certificate_time(value: &str) -> String {
    if value.is_empty() {
        return "-".to_string();
    }
    value.strip_suffix('Z').unwrap_or(value).replace('T', " ")
}
