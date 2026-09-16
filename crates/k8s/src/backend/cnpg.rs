//! CloudNativePG-specific backup and schedule mutations.

use kube::api::{DynamicObject, ListParams, PostParams};
use roder_core::ResourceKind;
use serde_json::{json, Value};

use super::{api_err, Backend};
use crate::client::K8sError;

#[derive(PartialEq)]
struct BackupTemplate {
    spec: Value,
    annotations: serde_json::Map<String, Value>,
}

impl Backend {
    pub async fn cnpg_backup(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
    ) -> Result<(), K8sError> {
        let cluster = self.entry(key)?;
        if cluster.kind.group != "postgresql.cnpg.io" || cluster.kind.kind != "Cluster" {
            return Err(K8sError::Api(
                "CNPG backups require a Cluster target".into(),
            ));
        }
        let namespace = ns
            .filter(|namespace| !namespace.is_empty())
            .ok_or_else(|| K8sError::Api("CNPG Cluster backup requires a namespace".into()))?;
        let cluster_object = self
            .dyn_api(key, Some(namespace))?
            .get(name)
            .await
            .map_err(api_err)?;
        let cluster_object = serde_json::to_value(cluster_object).map_err(api_err)?;
        let backup_key =
            ResourceKind::make_key(&cluster.kind.group, &cluster.kind.version, "Backup");
        let schedule_key = ResourceKind::make_key(
            &cluster.kind.group,
            &cluster.kind.version,
            "ScheduledBackup",
        );
        let schedules = self
            .dyn_api(&schedule_key, Some(namespace))?
            .list(&ListParams::default())
            .await
            .map_err(api_err)?
            .items
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(api_err)?;
        let backup: DynamicObject = serde_json::from_value(build_backup(
            &cluster.kind.group,
            &cluster.kind.version,
            namespace,
            name,
            &cluster_object,
            &schedules,
        )?)
        .map_err(api_err)?;
        self.dyn_api(&backup_key, Some(namespace))?
            .create(&PostParams::default(), &backup)
            .await
            .map_err(api_err)?;
        Ok(())
    }

    pub async fn cnpg_suspend(
        &self,
        key: &str,
        ns: Option<&str>,
        name: &str,
        suspend: bool,
    ) -> Result<(), K8sError> {
        let schedule = self.entry(key)?;
        if schedule.kind.group != "postgresql.cnpg.io" || schedule.kind.kind != "ScheduledBackup" {
            return Err(K8sError::Api(
                "CNPG suspend requires a ScheduledBackup target".into(),
            ));
        }
        let namespace = ns
            .filter(|namespace| !namespace.is_empty())
            .ok_or_else(|| K8sError::Api("CNPG ScheduledBackup requires a namespace".into()))?;
        self.merge_patch(
            key,
            Some(namespace),
            name,
            json!({ "spec": { "suspend": suspend } }),
        )
        .await
    }
}

fn build_backup(
    group: &str,
    version: &str,
    namespace: &str,
    cluster_name: &str,
    cluster: &Value,
    schedules: &[Value],
) -> Result<Value, K8sError> {
    let base: String = cluster_name
        .to_ascii_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(48)
        .collect();
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "cluster" } else { base };
    let template = backup_template(cluster_name, cluster, schedules)?;
    let mut metadata = json!({
        "generateName": format!("{base}-manual-"),
        "namespace": namespace,
    });
    if !template.annotations.is_empty() {
        metadata["annotations"] = Value::Object(template.annotations);
    }

    Ok(json!({
        "apiVersion": format!("{group}/{version}"),
        "kind": "Backup",
        "metadata": metadata,
        "spec": template.spec,
    }))
}

fn backup_template(
    cluster_name: &str,
    cluster: &Value,
    schedules: &[Value],
) -> Result<BackupTemplate, K8sError> {
    let mut templates = Vec::new();
    for schedule in schedules.iter().filter(|schedule| {
        schedule
            .pointer("/spec/cluster/name")
            .and_then(Value::as_str)
            == Some(cluster_name)
    }) {
        let template = backup_template_from_schedule(cluster_name, cluster, schedule)?;
        if !templates.contains(&template) {
            templates.push(template);
        }
    }
    if templates.len() == 1 {
        return Ok(templates.remove(0));
    }
    if templates.len() > 1 {
        return Err(K8sError::Api(
            "CNPG Cluster has ScheduledBackups with different backup configurations".into(),
        ));
    }

    let barman = cluster.pointer("/spec/backup/barmanObjectStore").is_some();
    let snapshots = cluster.pointer("/spec/backup/volumeSnapshot").is_some();
    let method = match (barman, snapshots) {
        (true, false) => "barmanObjectStore",
        (false, true) => "volumeSnapshot",
        (true, true) => {
            return Err(K8sError::Api(
                "CNPG Cluster has multiple backup methods and no ScheduledBackup selects one"
                    .into(),
            ));
        }
        (false, false) => {
            return Err(K8sError::Api(
                "CNPG Cluster has no ScheduledBackup or configured backup method".into(),
            ));
        }
    };
    Ok(BackupTemplate {
        spec: json!({ "cluster": { "name": cluster_name }, "method": method }),
        annotations: serde_json::Map::new(),
    })
}

fn backup_template_from_schedule(
    cluster_name: &str,
    cluster: &Value,
    schedule: &Value,
) -> Result<BackupTemplate, K8sError> {
    let source = schedule
        .get("spec")
        .and_then(Value::as_object)
        .ok_or_else(|| K8sError::Api("CNPG ScheduledBackup has no specification".into()))?;
    let method = source
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("barmanObjectStore");
    if method == "plugin" && !source.contains_key("pluginConfiguration") {
        return Err(K8sError::Api(
            "CNPG plugin ScheduledBackup has no plugin configuration".into(),
        ));
    }
    let target = source
        .get("target")
        .or_else(|| cluster.pointer("/spec/backup/target"))
        .cloned()
        .unwrap_or_else(|| Value::String("prefer-standby".into()));
    let mut spec = json!({
        "cluster": { "name": cluster_name },
        "method": method,
        "target": target,
    });
    if let Some(value) = source.get("pluginConfiguration") {
        spec["pluginConfiguration"] = value.clone();
    }
    let mut annotations: serde_json::Map<String, Value> = schedule
        .pointer("/metadata/annotations")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(key, _)| key.starts_with("backup.cnpg.io/"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if method == "volumeSnapshot" {
        spec["online"] = source
            .get("online")
            .or_else(|| cluster.pointer("/spec/backup/volumeSnapshot/online"))
            .cloned()
            .unwrap_or(Value::Bool(true));
        let mut online_configuration = source
            .get("onlineConfiguration")
            .and_then(Value::as_object)
            .or_else(|| {
                cluster
                    .pointer("/spec/backup/volumeSnapshot/onlineConfiguration")
                    .and_then(Value::as_object)
            })
            .cloned()
            .unwrap_or_default();
        online_configuration
            .entry("waitForArchive")
            .or_insert(Value::Bool(true));
        online_configuration
            .entry("immediateCheckpoint")
            .or_insert(Value::Bool(false));
        spec["onlineConfiguration"] = Value::Object(online_configuration);
        annotations
            .entry("backup.cnpg.io/volumeSnapshotDeadline")
            .or_insert_with(|| Value::String("10".into()));
    }
    Ok(BackupTemplate { spec, annotations })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_backup_references_the_cluster_and_uses_a_bounded_generated_name() {
        let cluster_name = format!("{}!", "Database".repeat(20));
        let cluster = json!({
            "spec": { "backup": { "barmanObjectStore": { "destinationPath": "s3://db" } } }
        });
        let backup = build_backup(
            "postgresql.cnpg.io",
            "v1",
            "database",
            &cluster_name,
            &cluster,
            &[],
        )
        .unwrap();

        assert_eq!(backup["apiVersion"], "postgresql.cnpg.io/v1");
        assert_eq!(backup["kind"], "Backup");
        assert_eq!(backup["metadata"]["namespace"], "database");
        assert_eq!(backup["spec"]["cluster"]["name"], cluster_name);
        assert_eq!(backup["spec"]["method"], "barmanObjectStore");
        let generate_name = backup["metadata"]["generateName"].as_str().unwrap();
        assert!(generate_name.ends_with("-manual-"));
        assert!(generate_name.len() <= 56);
    }

    #[test]
    fn manual_backup_copies_the_scheduled_plugin_configuration() {
        let schedule = json!({
            "metadata": { "annotations": {
                "backup.cnpg.io/volumeSnapshotDeadline": "30m",
                "unrelated.example.io/owner": "gitops"
            } },
            "spec": {
                "cluster": { "name": "postgres" },
                "method": "plugin",
                "pluginConfiguration": {
                    "name": "barman-cloud.cloudnative-pg.io",
                    "parameters": { "barmanObjectName": "garage-store" }
                }
            }
        });
        let backup = build_backup(
            "postgresql.cnpg.io",
            "v1",
            "database",
            "postgres",
            &json!({ "spec": {} }),
            &[schedule],
        )
        .unwrap();

        assert_eq!(backup["spec"]["method"], "plugin");
        assert_eq!(
            backup["spec"]["pluginConfiguration"],
            json!({
                "name": "barman-cloud.cloudnative-pg.io",
                "parameters": { "barmanObjectName": "garage-store" }
            })
        );
        assert_eq!(
            backup["metadata"]["annotations"],
            json!({ "backup.cnpg.io/volumeSnapshotDeadline": "30m" })
        );
    }

    #[test]
    fn manual_backup_rejects_clusters_without_backup_configuration() {
        let result = build_backup(
            "postgresql.cnpg.io",
            "v1",
            "database",
            "postgres",
            &json!({ "spec": {} }),
            &[],
        );

        assert!(result.is_err());
    }

    #[test]
    fn manual_backup_does_not_infer_method_from_wal_archiver_plugin() {
        let cluster = json!({
            "spec": {
                "backup": { "volumeSnapshot": { "online": true } },
                "plugins": [{
                    "enabled": true,
                    "isWALArchiver": true,
                    "name": "wal-only.example.io"
                }]
            }
        });
        let backup = build_backup(
            "postgresql.cnpg.io",
            "v1",
            "database",
            "postgres",
            &cluster,
            &[],
        )
        .unwrap();

        assert_eq!(backup["spec"]["method"], "volumeSnapshot");
    }

    #[test]
    fn equivalent_schedule_defaults_do_not_create_an_ambiguity() {
        let cluster = json!({ "spec": { "backup": { "target": "primary" } } });
        let schedules = [
            json!({ "spec": {
                "cluster": { "name": "postgres" },
                "method": "barmanObjectStore"
            } }),
            json!({ "spec": {
                "cluster": { "name": "postgres" },
                "method": "barmanObjectStore",
                "target": "primary"
            } }),
        ];

        let backup = build_backup(
            "postgresql.cnpg.io",
            "v1",
            "database",
            "postgres",
            &cluster,
            &schedules,
        )
        .unwrap();

        assert_eq!(backup["spec"]["target"], "primary");
    }

    #[test]
    fn equivalent_volume_snapshot_defaults_do_not_create_an_ambiguity() {
        let schedules = [
            json!({ "spec": {
                "cluster": { "name": "postgres" },
                "method": "volumeSnapshot"
            } }),
            json!({
                "metadata": { "annotations": {
                    "backup.cnpg.io/volumeSnapshotDeadline": "10"
                } },
                "spec": {
                    "cluster": { "name": "postgres" },
                    "method": "volumeSnapshot",
                    "online": true,
                    "onlineConfiguration": {
                        "waitForArchive": true,
                        "immediateCheckpoint": false
                    }
                }
            }),
        ];

        let backup = build_backup(
            "postgresql.cnpg.io",
            "v1",
            "database",
            "postgres",
            &json!({ "spec": {} }),
            &schedules,
        )
        .unwrap();

        assert_eq!(backup["spec"]["online"], true);
        assert_eq!(
            backup["spec"]["onlineConfiguration"]["waitForArchive"],
            true
        );
        assert_eq!(
            backup["metadata"]["annotations"]["backup.cnpg.io/volumeSnapshotDeadline"],
            "10"
        );
    }

    #[test]
    fn scheduled_online_configuration_replaces_cluster_configuration() {
        let cluster = json!({ "spec": { "backup": { "volumeSnapshot": {
            "onlineConfiguration": { "immediateCheckpoint": true }
        } } } });
        let schedule = json!({ "spec": {
            "cluster": { "name": "postgres" },
            "method": "volumeSnapshot",
            "onlineConfiguration": { "waitForArchive": false }
        } });

        let backup = build_backup(
            "postgresql.cnpg.io",
            "v1",
            "database",
            "postgres",
            &cluster,
            &[schedule],
        )
        .unwrap();

        assert_eq!(
            backup["spec"]["onlineConfiguration"]["waitForArchive"],
            false
        );
        assert_eq!(
            backup["spec"]["onlineConfiguration"]["immediateCheckpoint"],
            false
        );
    }
}
