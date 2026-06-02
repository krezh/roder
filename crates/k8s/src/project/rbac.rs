//! RBAC binding row projector (RoleBinding / ClusterRoleBinding).

use roder_core::RowStatus;
use serde_json::Value;

use super::accessors::str_at;

pub(crate) fn rolebinding_cells(data: &Value) -> (Vec<String>, RowStatus) {
    let kind = str_at(data, &["roleRef", "kind"]).unwrap_or_default();
    let name = str_at(data, &["roleRef", "name"]).unwrap_or_default();
    let role = if kind.is_empty() {
        name
    } else {
        format!("{kind}/{name}")
    };
    (vec![role], RowStatus::Ok)
}
