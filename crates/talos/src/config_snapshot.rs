//! A non-secret, order-stable projection of a node's active machine
//! configuration: every value under an allow-listed set of config roots is
//! kept as-is, except secret-shaped keys, which are replaced by a fingerprint
//! — so two nodes' configs can be diffed without ever handling their secrets.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::error::TalosError;
use crate::Backend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSnapshot {
    pub fingerprint: String,
    pub fields: BTreeMap<String, ConfigField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigField {
    Plain(String),
    Sensitive(String),
}

impl Backend {
    /// Return a non-secret projection of the active machine configuration.
    pub async fn config_snapshot(&self, node: &str) -> Result<ConfigSnapshot, TalosError> {
        let resources = crate::cosi::list(
            self.current_channel().await?,
            node,
            "config",
            "MachineConfigs.config.talos.dev",
        )
        .await?;
        let config = resources.into_iter().next().ok_or_else(|| {
            TalosError::Upstream("machine configuration resource is empty".into())
        })?;
        let spec = machine_config_document(&config.spec).ok_or_else(|| {
            TalosError::Upstream("machine configuration document is missing".into())
        })?;
        Ok(config_snapshot(spec))
    }
}

fn machine_config_document(spec: &serde_json::Value) -> Option<&serde_json::Value> {
    let documents = spec
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(spec));
    documents
        .iter()
        .find(|document| document.get("machine").is_some() || document.get("cluster").is_some())
}

fn config_snapshot(config: &serde_json::Value) -> ConfigSnapshot {
    const SAFE_ROOTS: &[&str] = &[
        "/machine/type",
        "/machine/install",
        "/machine/network",
        "/machine/kubelet",
        "/machine/features",
        "/machine/sysctls",
        "/machine/kernel/modules",
        "/cluster/clusterName",
        "/cluster/network",
        "/cluster/discovery",
        "/cluster/apiServer",
        "/cluster/controllerManager",
        "/cluster/scheduler",
        "/cluster/etcd",
    ];

    let mut fields = BTreeMap::new();
    for root in SAFE_ROOTS {
        if let Some(value) = config.pointer(root) {
            flatten_config(root, value, &mut fields);
        }
    }
    let mut digest = Sha256::new();
    for (path, value) in &fields {
        digest.update(path.as_bytes());
        digest.update([0]);
        match value {
            ConfigField::Plain(value) | ConfigField::Sensitive(value) => {
                digest.update(value.as_bytes())
            }
        }
        digest.update([0]);
    }
    ConfigSnapshot {
        fingerprint: hex::encode(digest.finalize()),
        fields,
    }
}

fn flatten_config(
    path: &str,
    value: &serde_json::Value,
    fields: &mut BTreeMap<String, ConfigField>,
) {
    if sensitive_path(path) {
        let mut digest = Sha256::new();
        digest.update(serde_json::to_vec(value).unwrap_or_default());
        fields.insert(
            path.into(),
            ConfigField::Sensitive(hex::encode(digest.finalize())),
        );
        return;
    }
    match value {
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                flatten_config(&format!("{path}/{key}"), value, fields);
            }
        }
        serde_json::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                flatten_config(&format!("{path}/{index}"), value, fields);
            }
        }
        serde_json::Value::String(value) => {
            fields.insert(path.into(), ConfigField::Plain(value.clone()));
        }
        value => {
            fields.insert(path.into(), ConfigField::Plain(value.to_string()));
        }
    }
}

fn sensitive_path(path: &str) -> bool {
    let key = path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    key == "ca"
        || key.contains("token")
        || key.contains("secret")
        || key.contains("password")
        || key.contains("credential")
        || key.contains("auth")
        || key.contains("private")
        || key == "psk"
        || key.ends_with("psk")
        || key.contains("certificate")
        || key.ends_with("key")
        || key.ends_with("crt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_snapshot_redacts_sensitive_values() {
        let first = config_snapshot(&serde_json::json!({
            "machine": {
                "type": "controlplane",
                "network": { "hostname": "node-1", "wireguard": { "privateKey": "secret-a" } }
            },
            "cluster": { "clusterName": "prod" }
        }));
        let second = config_snapshot(&serde_json::json!({
            "machine": {
                "type": "controlplane",
                "network": { "hostname": "node-1", "wireguard": { "privateKey": "secret-b" } }
            },
            "cluster": { "clusterName": "prod" }
        }));
        assert!(matches!(
            first.fields.get("/machine/network/wireguard/privateKey"),
            Some(ConfigField::Sensitive(_))
        ));
        assert_ne!(first.fingerprint, second.fingerprint);
        assert!(!format!("{first:?}").contains("secret-a"));
    }

    #[test]
    fn config_snapshot_redacts_common_credential_names() {
        let snapshot = config_snapshot(&serde_json::json!({
            "machine": {
                "network": {
                    "credential": "credential-value",
                    "auth": "auth-value",
                    "wifi": { "psk": "psk-value" },
                    "privateMaterial": "private-value"
                }
            }
        }));
        for path in [
            "/machine/network/credential",
            "/machine/network/auth",
            "/machine/network/wifi/psk",
            "/machine/network/privateMaterial",
        ] {
            assert!(matches!(
                snapshot.fields.get(path),
                Some(ConfigField::Sensitive(_))
            ));
        }
        let debug = format!("{snapshot:?}");
        for secret in [
            "credential-value",
            "auth-value",
            "psk-value",
            "private-value",
        ] {
            assert!(!debug.contains(secret));
        }
    }

    #[test]
    fn finds_machine_config_in_multi_document_resource() {
        let spec = serde_json::json!([
            { "apiVersion": "v1alpha1", "kind": "ExtensionServiceConfig" },
            { "machine": { "type": "worker" }, "cluster": { "clusterName": "prod" } }
        ]);

        let config = machine_config_document(&spec).unwrap();
        assert_eq!(config.pointer("/machine/type").unwrap(), "worker");
    }
}
