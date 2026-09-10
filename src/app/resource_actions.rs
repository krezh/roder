use std::collections::HashMap;

use roder_core::{ResourceAction, ResourceCapabilities, ResourceRow, RowStatus};

use crate::app::state::DetailTarget;
use crate::app::table_logic::node_is_control_plane;
use crate::app::util::format::parse_key;

/// Actions supported by every target in a resource selection.
pub(crate) struct AvailableActions(Vec<ResourceAction>);

impl AvailableActions {
    pub(crate) fn for_targets(targets: &[DetailTarget]) -> Self {
        let actions = ResourceAction::ALL
            .into_iter()
            .filter(|action| {
                !targets.is_empty()
                    && targets.iter().all(|target| {
                        let (group, version, kind) = parse_key(&target.key);
                        ResourceCapabilities::for_gvk(&group, &version, &kind).supports(*action)
                    })
            })
            .collect();
        Self(actions)
    }

    pub(crate) fn supports(&self, action: ResourceAction) -> bool {
        self.0.contains(&action)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActionSurface {
    Desktop,
    Mobile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourceMenuAction {
    OpenDetails,
    Relationships,
    Logs,
    Shell,
    BrowseFiles,
    DebugShell,
    NodeShell,
    GoToNamespace,
    GoToNode,
    CopyNames,
    Restart,
    Scale,
    FluxReconcile,
    FluxReconcileWithSource,
    FluxForce,
    FluxReset,
    FluxSuspend,
    FluxResume,
    ExternalSecretsRefresh,
    CertificateRenew,
    CronJobTrigger,
    JobRerun,
    KopiurSnapshotNow,
    Cordon,
    Uncordon,
    Drain,
    TalosEtcdDefrag,
    TalosReboot,
    TalosShutdown,
    Evict,
    Delete,
}

impl ResourceMenuAction {
    const ALL: [Self; 31] = [
        Self::OpenDetails,
        Self::Relationships,
        Self::Logs,
        Self::Shell,
        Self::BrowseFiles,
        Self::DebugShell,
        Self::NodeShell,
        Self::GoToNamespace,
        Self::GoToNode,
        Self::CopyNames,
        Self::Restart,
        Self::Scale,
        Self::FluxReconcile,
        Self::FluxReconcileWithSource,
        Self::FluxForce,
        Self::FluxReset,
        Self::FluxSuspend,
        Self::FluxResume,
        Self::ExternalSecretsRefresh,
        Self::CertificateRenew,
        Self::CronJobTrigger,
        Self::JobRerun,
        Self::KopiurSnapshotNow,
        Self::Cordon,
        Self::Uncordon,
        Self::Drain,
        Self::TalosEtcdDefrag,
        Self::TalosReboot,
        Self::TalosShutdown,
        Self::Evict,
        Self::Delete,
    ];

    fn supported_on(self, surface: ActionSurface) -> bool {
        match surface {
            ActionSurface::Desktop => true,
            ActionSurface::Mobile => !matches!(
                self,
                Self::DebugShell
                    | Self::NodeShell
                    | Self::Cordon
                    | Self::Uncordon
                    | Self::Drain
                    | Self::TalosEtcdDefrag
                    | Self::TalosReboot
                    | Self::TalosShutdown
                    | Self::Evict
            ),
        }
    }
}

pub(crate) struct ResourceActionModel {
    actions: Vec<ResourceMenuAction>,
    pub(crate) control_plane: bool,
}

impl ResourceActionModel {
    pub(crate) fn for_selection(
        surface: ActionSurface,
        targets: &[DetailTarget],
        target_uids: &[String],
        rows: Option<&HashMap<String, ResourceRow>>,
        node: Option<&str>,
        talos_actions: bool,
        permitted: impl Fn(ResourceAction) -> bool,
    ) -> Self {
        let available = AvailableActions::for_targets(targets);
        let is_bulk = targets.len() > 1;
        let is_pod = available.supports(ResourceAction::Exec);
        let is_node = available.supports(ResourceAction::Cordon);
        let row_state = |read: fn(&ResourceRow) -> bool| {
            rows.and_then(|rows| {
                let mut states = target_uids.iter().filter_map(|uid| rows.get(uid)).map(read);
                let first = states.next()?;
                states.all(|state| state == first).then_some(first)
            })
        };
        let suspended = row_state(|row| row.suspended);
        let cordoned = row_state(|row| row.status == RowStatus::Warn);
        let jobs_terminal = available.supports(ResourceAction::JobRerun)
            && rows.is_some_and(|rows| {
                target_uids.iter().all(|uid| {
                    rows.get(uid)
                        .is_some_and(|row| matches!(row.status, RowStatus::Ok | RowStatus::Error))
                })
            });
        let control_plane = rows.is_some_and(|rows| {
            target_uids
                .first()
                .and_then(|uid| rows.get(uid))
                .is_some_and(node_is_control_plane)
        });

        let eligible = |action| match action {
            ResourceMenuAction::OpenDetails | ResourceMenuAction::Relationships => !is_bulk,
            ResourceMenuAction::Logs => {
                available.supports(ResourceAction::Logs) && permitted(ResourceAction::Logs)
            }
            ResourceMenuAction::Shell | ResourceMenuAction::BrowseFiles => {
                !is_bulk && is_pod && permitted(ResourceAction::Exec)
            }
            ResourceMenuAction::DebugShell => {
                !is_bulk && is_pod && permitted(ResourceAction::DebugExec)
            }
            ResourceMenuAction::NodeShell => {
                !is_bulk && is_node && permitted(ResourceAction::NodeShell)
            }
            ResourceMenuAction::GoToNamespace => {
                !is_bulk
                    && targets
                        .first()
                        .is_some_and(|target| target.namespace.is_some())
            }
            ResourceMenuAction::GoToNode => !is_bulk && is_pod && node.is_some(),
            ResourceMenuAction::CopyNames => true,
            ResourceMenuAction::Restart => {
                available.supports(ResourceAction::Restart) && permitted(ResourceAction::Restart)
            }
            ResourceMenuAction::Scale => {
                !is_bulk
                    && available.supports(ResourceAction::Scale)
                    && permitted(ResourceAction::Scale)
            }
            ResourceMenuAction::FluxReconcile => {
                available.supports(ResourceAction::FluxReconcile)
                    && permitted(ResourceAction::FluxReconcile)
            }
            ResourceMenuAction::FluxReconcileWithSource => {
                available.supports(ResourceAction::FluxReconcileWithSource)
                    && permitted(ResourceAction::FluxReconcileWithSource)
            }
            ResourceMenuAction::FluxForce => {
                available.supports(ResourceAction::FluxForce)
                    && permitted(ResourceAction::FluxForce)
            }
            ResourceMenuAction::FluxReset => {
                available.supports(ResourceAction::FluxReset)
                    && permitted(ResourceAction::FluxReset)
            }
            ResourceMenuAction::FluxSuspend => {
                available.supports(ResourceAction::FluxSuspend)
                    && suspended != Some(true)
                    && permitted(ResourceAction::FluxSuspend)
            }
            ResourceMenuAction::FluxResume => {
                available.supports(ResourceAction::FluxSuspend)
                    && suspended != Some(false)
                    && permitted(ResourceAction::FluxSuspend)
            }
            ResourceMenuAction::ExternalSecretsRefresh => {
                available.supports(ResourceAction::ExternalSecretsRefresh)
                    && permitted(ResourceAction::ExternalSecretsRefresh)
            }
            ResourceMenuAction::CertificateRenew => {
                available.supports(ResourceAction::CertificateRenew)
                    && permitted(ResourceAction::CertificateRenew)
            }
            ResourceMenuAction::CronJobTrigger => {
                available.supports(ResourceAction::CronJobTrigger)
                    && permitted(ResourceAction::CronJobTrigger)
            }
            ResourceMenuAction::JobRerun => jobs_terminal && permitted(ResourceAction::JobRerun),
            ResourceMenuAction::KopiurSnapshotNow => {
                available.supports(ResourceAction::KopiurSnapshotNow)
                    && permitted(ResourceAction::KopiurSnapshotNow)
            }
            ResourceMenuAction::Cordon => {
                is_node && cordoned != Some(true) && permitted(ResourceAction::Cordon)
            }
            ResourceMenuAction::Uncordon => {
                is_node && cordoned != Some(false) && permitted(ResourceAction::Cordon)
            }
            ResourceMenuAction::Drain => !is_bulk && is_node && permitted(ResourceAction::Drain),
            ResourceMenuAction::TalosEtcdDefrag => {
                !is_bulk && is_node && talos_actions && control_plane
            }
            ResourceMenuAction::TalosReboot | ResourceMenuAction::TalosShutdown => {
                !is_bulk && is_node && talos_actions
            }
            ResourceMenuAction::Evict => is_pod && permitted(ResourceAction::Evict),
            ResourceMenuAction::Delete => permitted(ResourceAction::Delete),
        };
        let actions = ResourceMenuAction::ALL
            .into_iter()
            .filter(|action| action.supported_on(surface) && eligible(*action))
            .collect();

        Self {
            actions,
            control_plane,
        }
    }

    pub(crate) fn supports(&self, action: ResourceMenuAction) -> bool {
        self.actions.contains(&action)
    }

    pub(crate) fn supports_any(&self, actions: &[ResourceMenuAction]) -> bool {
        actions.iter().any(|action| self.supports(*action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(key: &str) -> DetailTarget {
        DetailTarget {
            key: key.to_string(),
            namespace: Some("default".to_string()),
            name: "example".to_string(),
        }
    }

    #[test]
    fn selection_actions_are_the_capability_intersection() {
        let deployments = [target("apps/v1/Deployment"), target("apps/v1/Deployment")];
        let actions = AvailableActions::for_targets(&deployments);
        assert!(actions.supports(ResourceAction::Restart));
        assert!(actions.supports(ResourceAction::Scale));

        let mixed = [target("apps/v1/Deployment"), target("apps/v1/DaemonSet")];
        let actions = AvailableActions::for_targets(&mixed);
        assert!(actions.supports(ResourceAction::Restart));
        assert!(!actions.supports(ResourceAction::Scale));
    }

    #[test]
    fn desktop_explicitly_supports_every_menu_action() {
        assert!(ResourceMenuAction::ALL
            .into_iter()
            .all(|action| action.supported_on(ActionSurface::Desktop)));
    }

    #[test]
    fn mobile_exclusions_are_explicit() {
        let excluded = ResourceMenuAction::ALL
            .into_iter()
            .filter(|action| !action.supported_on(ActionSurface::Mobile))
            .collect::<Vec<_>>();
        assert_eq!(
            excluded,
            vec![
                ResourceMenuAction::DebugShell,
                ResourceMenuAction::NodeShell,
                ResourceMenuAction::Cordon,
                ResourceMenuAction::Uncordon,
                ResourceMenuAction::Drain,
                ResourceMenuAction::TalosEtcdDefrag,
                ResourceMenuAction::TalosReboot,
                ResourceMenuAction::TalosShutdown,
                ResourceMenuAction::Evict,
            ]
        );
    }

    #[test]
    fn selection_state_controls_suspend_and_terminal_job_actions() {
        let targets = [target("batch/v1/Job")];
        let mut rows = HashMap::new();
        let mut row = ResourceRow {
            uid: "job-uid".into(),
            name: "example".into(),
            namespace: Some("default".into()),
            created: None,
            cells: Vec::new(),
            trends: Vec::new(),
            status: RowStatus::Ok,
            suspended: false,
            labels: Default::default(),
        };
        rows.insert(row.uid.clone(), row.clone());
        let model = ResourceActionModel::for_selection(
            ActionSurface::Desktop,
            &targets,
            &[row.uid.clone()],
            Some(&rows),
            None,
            false,
            |_| true,
        );
        assert!(model.supports(ResourceMenuAction::JobRerun));

        row.status = RowStatus::Pending;
        rows.insert(row.uid.clone(), row.clone());
        let model = ResourceActionModel::for_selection(
            ActionSurface::Desktop,
            &targets,
            &[row.uid],
            Some(&rows),
            None,
            false,
            |_| true,
        );
        assert!(!model.supports(ResourceMenuAction::JobRerun));
    }

    #[test]
    fn surfaces_share_eligibility_except_for_explicit_mobile_exclusions() {
        let targets = [target("/v1/Pod")];
        let desktop = ResourceActionModel::for_selection(
            ActionSurface::Desktop,
            &targets,
            &[],
            None,
            Some("worker-1"),
            false,
            |_| true,
        );
        let mobile = ResourceActionModel::for_selection(
            ActionSurface::Mobile,
            &targets,
            &[],
            None,
            Some("worker-1"),
            false,
            |_| true,
        );

        for action in ResourceMenuAction::ALL {
            if action.supported_on(ActionSurface::Mobile) {
                assert_eq!(
                    desktop.supports(action),
                    mobile.supports(action),
                    "{action:?}"
                );
            }
        }
    }

    #[test]
    fn node_shell_requires_its_dedicated_permission() {
        let targets = [target("/v1/Node")];
        let model = ResourceActionModel::for_selection(
            ActionSurface::Desktop,
            &targets,
            &[],
            None,
            None,
            false,
            |action| action != ResourceAction::NodeShell,
        );

        assert!(!model.supports(ResourceMenuAction::NodeShell));
    }
}
