//! Small GVK capability adapter used by keyboard and log-opening paths.

use roder_core::{ResourceAction, ResourceCapabilities};

/// A borrowed (group, kind) pair with the kind-classification predicates.
pub(crate) struct KindKind<'a> {
    pub(crate) group: &'a str,
    pub(crate) version: &'a str,
    pub(crate) kind: &'a str,
}

impl<'a> KindKind<'a> {
    pub(crate) fn new(group: &'a str, version: &'a str, kind: &'a str) -> Self {
        Self {
            group,
            version,
            kind,
        }
    }

    pub(crate) fn supports(&self, action: ResourceAction) -> bool {
        ResourceCapabilities::for_gvk(self.group, self.version, self.kind).supports(action)
    }

    pub(crate) fn is_pod(&self) -> bool {
        self.supports(ResourceAction::Exec)
    }

    pub(crate) fn has_logs(&self) -> bool {
        self.supports(ResourceAction::Logs)
    }
}
