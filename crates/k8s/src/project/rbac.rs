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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn role_reference_formats_kind_when_present() {
        let cases = [
            (
                json!({"roleRef": {"kind": "Role", "name": "admin"}}),
                "Role/admin",
            ),
            (
                json!({"roleRef": {"kind": "ClusterRole", "name": "view"}}),
                "ClusterRole/view",
            ),
            (json!({"roleRef": {"name": "edit"}}), "edit"),
            (json!({}), ""),
        ];

        for (data, expected) in cases {
            assert_eq!(
                rolebinding_cells(&data),
                (vec![expected.to_string()], RowStatus::Ok),
                "{data}"
            );
        }
    }
}
