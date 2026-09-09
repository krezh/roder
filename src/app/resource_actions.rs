use roder_core::{ResourceAction, ResourceCapabilities};

use crate::app::state::DetailTarget;
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
}
