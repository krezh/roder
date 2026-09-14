use leptos::prelude::*;
use roder_core::{ClusterOverview, ResourceHealthRollup, ResourceKind, RowStatus};

use crate::app::util::format::camel_label;
use crate::data;

const OVERVIEW_STORAGE_KEY: &str = "roder.overview";
const OVERVIEW_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Copy)]
pub(crate) struct OverviewState {
    pub(crate) data: RwSignal<Option<ClusterOverview>>,
    pub(crate) error: RwSignal<Option<String>>,
    pub(crate) stale: RwSignal<bool>,
    resource: LocalResource<Result<ClusterOverview, String>>,
}

impl OverviewState {
    pub(crate) fn new() -> Self {
        let data = RwSignal::new(None);
        let error = RwSignal::new(None);
        let stale = RwSignal::new(false);

        Effect::new(move |_| {
            if let Some(cached) = data::storage_get(OVERVIEW_STORAGE_KEY)
                .and_then(|value| serde_json::from_str(&value).ok())
            {
                data.set(Some(cached));
            }
        });

        let resource = LocalResource::new(|| async {
            data::fetch_json::<ClusterOverview>("/api/overview").await
        });
        let state = Self {
            data,
            error,
            stale,
            resource,
        };

        Effect::new(move |_| {
            let Some(result) = state.resource.get() else {
                return;
            };
            match result {
                Ok(overview) => {
                    if let Ok(json) = serde_json::to_string(&overview) {
                        data::storage_set(OVERVIEW_STORAGE_KEY, &json);
                    }
                    state.data.set(Some(overview));
                    state.error.set(None);
                    state.stale.set(false);
                }
                Err(error) => {
                    state.error.set(Some(error));
                    state.stale.set(true);
                }
            }
        });

        Effect::new(move |_| {
            if let Ok(handle) =
                set_interval_with_handle(move || state.resource.refetch(), OVERVIEW_POLL_INTERVAL)
            {
                on_cleanup(move || handle.clear());
            }
        });

        state
    }

    pub(crate) fn refresh(self) {
        self.resource.refetch();
    }

    pub(crate) fn refreshing(self) -> bool {
        self.resource.get().is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HealthState {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ClusterHealthSummary {
    pub(crate) state: HealthState,
    pub(crate) label: &'static str,
    pub(crate) summary: String,
    pub(crate) ready_nodes: usize,
    pub(crate) failing: usize,
    pub(crate) caution: usize,
}

pub(crate) fn cluster_health(overview: &ClusterOverview) -> ClusterHealthSummary {
    let ready_nodes = overview.nodes.iter().filter(|node| node.ready).count();
    let unready_nodes = overview.nodes.len().saturating_sub(ready_nodes);
    let controller_failing = overview
        .controller_groups
        .iter()
        .flat_map(|group| &group.resources)
        .map(|resource| resource.health.failing as usize)
        .sum::<usize>()
        + overview
            .controller_groups
            .iter()
            .flat_map(|group| &group.signals)
            .filter(|signal| signal.status == RowStatus::Error)
            .count();
    let controller_suspended = overview
        .controller_groups
        .iter()
        .flat_map(|group| &group.resources)
        .map(|resource| resource.health.suspended as usize)
        .sum::<usize>();
    let controller_warning = overview
        .controller_groups
        .iter()
        .flat_map(|group| &group.resources)
        .map(|resource| (resource.health.warning + resource.health.unknown) as usize)
        .sum::<usize>()
        + overview
            .controller_groups
            .iter()
            .flat_map(|group| &group.signals)
            .filter(|signal| matches!(signal.status, RowStatus::Pending | RowStatus::Warn))
            .count();
    let controller_unreadable = overview
        .controller_groups
        .iter()
        .flat_map(|group| &group.resources)
        .filter(|resource| resource.error.is_some())
        .count();
    let failing =
        overview.pod_failed as usize + controller_failing + controller_unreadable + unready_nodes;
    let caution = overview.pod_pending as usize
        + controller_suspended
        + controller_warning
        + overview.warnings.len();

    let (state, label, summary) = if failing > 0 {
        (
            HealthState::Error,
            "Attention needed",
            format!(
                "{failing} failing signal{} across the cluster",
                if failing == 1 { "" } else { "s" }
            ),
        )
    } else if caution > 0 {
        (
            HealthState::Warning,
            "Review recommended",
            format!(
                "{caution} warning signal{} to review",
                if caution == 1 { "" } else { "s" }
            ),
        )
    } else {
        (
            HealthState::Ok,
            "Cluster healthy",
            "All tracked systems are operating normally".to_string(),
        )
    };

    ClusterHealthSummary {
        state,
        label,
        summary,
        ready_nodes,
        failing,
        caution,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControllerState {
    Ok,
    Neutral,
    Warning,
    Pending,
    Error,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ControllerRollup {
    pub(crate) state: ControllerState,
    pub(crate) label: String,
    pub(crate) target: String,
    pub(crate) unreadable: bool,
    pub(crate) only_unclassified: bool,
}

pub(crate) fn controller_rollup(resource: &ResourceHealthRollup) -> ControllerRollup {
    let unreadable = resource.error.is_some();
    let only_unclassified = resource.health.total > 0
        && resource.health.unknown + resource.health.unreported == resource.health.total;
    let state = if unreadable || resource.health.failing > 0 {
        ControllerState::Error
    } else if resource.health.reconciling > 0 {
        ControllerState::Pending
    } else if resource.health.suspended > 0
        || resource.health.warning > 0
        || resource.health.unknown > 0
    {
        ControllerState::Warning
    } else if resource.health.unreported > 0 {
        ControllerState::Neutral
    } else {
        ControllerState::Ok
    };

    ControllerRollup {
        state,
        label: resource_plural_label(&resource.kind),
        target: if resource.key.is_empty() {
            resource.kind.clone()
        } else {
            resource.key.clone()
        },
        unreadable,
        only_unclassified,
    }
}

pub(crate) fn controller_signal_state(status: RowStatus) -> ControllerState {
    match status {
        RowStatus::Error => ControllerState::Error,
        RowStatus::Pending => ControllerState::Pending,
        RowStatus::Warn | RowStatus::Unknown => ControllerState::Warning,
        RowStatus::Ok | RowStatus::Done => ControllerState::Ok,
    }
}

pub(crate) fn resource_plural_label(kind: &str) -> String {
    let label = camel_label(kind);
    if let Some(stem) = label.strip_suffix("Policy") {
        format!("{stem}Policies")
    } else if let Some(stem) = label.strip_suffix("Repository") {
        format!("{stem}Repositories")
    } else if let Some(stem) = label.strip_suffix("Class") {
        format!("{stem}Classes")
    } else {
        format!("{label}s")
    }
}

pub(crate) fn kind_for_target(catalog: &[ResourceKind], key_or_kind: &str) -> Option<ResourceKind> {
    catalog
        .iter()
        .find(|resource| resource.key == key_or_kind || resource.kind == key_or_kind)
        .cloned()
}

pub(crate) fn core_kind(catalog: &[ResourceKind], kind: &str) -> Option<ResourceKind> {
    catalog
        .iter()
        .find(|resource| resource.group.is_empty() && resource.kind == kind)
        .cloned()
}

#[cfg(test)]
mod tests {
    use roder_core::{
        Category, ControllerHealthGroup, ControllerHealthSignal, HealthRollup, NodeSummary,
        OverviewWarning,
    };

    use super::*;

    #[test]
    fn cluster_health_combines_every_tracked_failure_and_warning() {
        let overview = ClusterOverview {
            nodes: vec![
                NodeSummary {
                    ready: true,
                    ..Default::default()
                },
                NodeSummary::default(),
            ],
            pod_failed: 2,
            pod_pending: 3,
            warnings: vec![OverviewWarning::default()],
            controller_groups: vec![ControllerHealthGroup {
                name: "Flux".into(),
                resources: vec![ResourceHealthRollup {
                    health: HealthRollup {
                        failing: 4,
                        suspended: 5,
                        warning: 6,
                        unknown: 7,
                        ..Default::default()
                    },
                    error: Some("forbidden".into()),
                    ..Default::default()
                }],
                signals: vec![
                    ControllerHealthSignal {
                        label: String::new(),
                        value: String::new(),
                        status: RowStatus::Error,
                        timestamp: None,
                        message: String::new(),
                    },
                    ControllerHealthSignal {
                        label: String::new(),
                        value: String::new(),
                        status: RowStatus::Warn,
                        timestamp: None,
                        message: String::new(),
                    },
                ],
            }],
            ..Default::default()
        };

        let summary = cluster_health(&overview);

        assert_eq!(summary.state, HealthState::Error);
        assert_eq!(summary.ready_nodes, 1);
        assert_eq!(summary.failing, 9);
        assert_eq!(summary.caution, 23);
        assert_eq!(summary.summary, "9 failing signals across the cluster");
    }

    #[test]
    fn controller_rollup_uses_shared_priority_and_label() {
        let resource = ResourceHealthRollup {
            key: "source.toolkit.fluxcd.io/v1/GitRepository".into(),
            kind: "GitRepository".into(),
            health: HealthRollup {
                total: 2,
                reconciling: 1,
                suspended: 1,
                ..Default::default()
            },
            error: None,
        };

        let rollup = controller_rollup(&resource);

        assert_eq!(rollup.state, ControllerState::Pending);
        assert_eq!(rollup.label, "Git Repositories");
        assert_eq!(rollup.target, resource.key);
        assert!(!rollup.only_unclassified);
    }

    #[test]
    fn resource_labels_pluralize_special_endings() {
        assert_eq!(resource_plural_label("NetworkPolicy"), "Network Policies");
        assert_eq!(resource_plural_label("StorageClass"), "Storage Classes");
        assert_eq!(resource_plural_label("Pod"), "Pods");
    }

    #[test]
    fn kind_lookup_supports_keys_and_core_kinds() {
        let custom_node = kind("example.io/v1/Node", "example.io", "Node");
        let core_node = kind("/v1/Node", "", "Node");
        let catalog = vec![custom_node.clone(), core_node.clone()];

        assert_eq!(
            kind_for_target(&catalog, "example.io/v1/Node"),
            Some(custom_node)
        );
        assert_eq!(core_kind(&catalog, "Node"), Some(core_node));
    }

    fn kind(key: &str, group: &str, kind: &str) -> ResourceKind {
        ResourceKind {
            key: key.into(),
            group: group.into(),
            version: "v1".into(),
            kind: kind.into(),
            plural: format!("{}s", kind.to_lowercase()),
            namespaced: false,
            category: Category::Cluster,
        }
    }
}
