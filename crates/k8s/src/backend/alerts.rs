use std::collections::{BTreeSet, HashMap};

use kube::api::{DynamicObject, ListParams};
use roder_core::{AlertResourceTarget, FiringAlert, ResourceKind};

use super::Backend;

const RULE_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const STANDARD_TARGET_LABELS: &[(&str, &str, &str)] = &[
    ("pod", "", "Pod"),
    ("service", "", "Service"),
    ("node", "", "Node"),
    ("persistentvolumeclaim", "", "PersistentVolumeClaim"),
    ("deployment", "apps", "Deployment"),
    ("statefulset", "apps", "StatefulSet"),
    ("daemonset", "apps", "DaemonSet"),
    ("replicaset", "apps", "ReplicaSet"),
    ("replicationcontroller", "", "ReplicationController"),
    ("cronjob", "batch", "CronJob"),
    ("job_name", "batch", "Job"),
];

#[derive(Clone)]
struct RuleDefinition {
    alert_name: String,
    target: AlertResourceTarget,
    labels: HashMap<String, String>,
}

impl Backend {
    /// Resolve alert labels and defining rules without making enrichment mandatory.
    pub async fn enrich_alerts(&self, alerts: &mut [FiringAlert]) {
        let kinds = self.kinds();
        for alert in alerts.iter_mut() {
            alert.targets = resolve_targets(&alert.labels, &kinds);
            alert.defining_rules.clear();
        }
        if alerts.is_empty() {
            return;
        }

        let Some(rule_kind) = kinds
            .iter()
            .find(|kind| kind.group == "monitoring.coreos.com" && kind.kind == "PrometheusRule")
        else {
            return;
        };
        let api = match self.dyn_api(&rule_kind.key, None) {
            Ok(api) => api,
            Err(error) => {
                tracing::debug!("alerts: cannot resolve PrometheusRule API: {error}");
                return;
            }
        };
        let objects =
            match tokio::time::timeout(RULE_LOOKUP_TIMEOUT, api.list(&ListParams::default())).await
            {
                Ok(Ok(objects)) => objects.items,
                Ok(Err(error)) => {
                    tracing::debug!("alerts: cannot list PrometheusRules: {error}");
                    return;
                }
                Err(_) => {
                    tracing::debug!("alerts: PrometheusRule lookup timed out");
                    return;
                }
            };
        let definitions = rule_definitions(&objects, rule_kind);
        for alert in alerts {
            alert.defining_rules = matching_rule_targets(alert, &definitions);
        }
    }
}

fn resolve_targets(
    labels: &HashMap<String, String>,
    kinds: &[ResourceKind],
) -> Vec<AlertResourceTarget> {
    let mut targets = BTreeSet::new();

    for &(label, group, kind) in STANDARD_TARGET_LABELS {
        if let Some(name) = nonempty_label(labels, label) {
            if let Some(target) = target_for(kinds, group, None, kind, name, labels) {
                targets.insert(target);
            }
        }
    }

    if let (Some(name), Some(kind)) = (
        nonempty_label(labels, "workload"),
        nonempty_label(labels, "workload_type").or_else(|| nonempty_label(labels, "workload_kind")),
    ) {
        if let Some((group, kind)) = workload_gvk(kind) {
            if let Some(target) = target_for(kinds, group, None, kind, name, labels) {
                targets.insert(target);
            }
        }
    }

    if let (Some(group), Some(version), Some(kind), Some(name)) = (
        nonempty_label(labels, "customresource_group"),
        nonempty_label(labels, "customresource_version"),
        nonempty_label(labels, "customresource_kind"),
        nonempty_label(labels, "customresource")
            .or_else(|| nonempty_label(labels, "customresource_name"))
            .or_else(|| nonempty_label(labels, "name")),
    ) {
        if let Some(target) = target_for(kinds, group, Some(version), kind, name, labels) {
            targets.insert(target);
        }
    }

    targets.into_iter().collect()
}

fn nonempty_label<'a>(labels: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    labels
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

fn target_for(
    kinds: &[ResourceKind],
    group: &str,
    version: Option<&str>,
    kind: &str,
    name: &str,
    labels: &HashMap<String, String>,
) -> Option<AlertResourceTarget> {
    let matching_kind = || {
        kinds
            .iter()
            .filter(|resource| resource.group == group && resource.kind == kind)
    };
    let resource = version
        .and_then(|version| matching_kind().find(|resource| resource.version == version))
        .or_else(|| matching_kind().next())?;
    let namespace = if resource.namespaced {
        Some(nonempty_label(labels, "namespace")?.to_string())
    } else {
        None
    };
    Some(AlertResourceTarget {
        key: resource.key.clone(),
        kind: resource.kind.clone(),
        namespace,
        name: name.to_string(),
    })
}

fn workload_gvk(kind: &str) -> Option<(&'static str, &'static str)> {
    let normalized: String = kind
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    Some(match normalized.as_str() {
        "pod" => ("", "Pod"),
        "deployment" => ("apps", "Deployment"),
        "statefulset" => ("apps", "StatefulSet"),
        "daemonset" => ("apps", "DaemonSet"),
        "replicaset" => ("apps", "ReplicaSet"),
        "replicationcontroller" => ("", "ReplicationController"),
        "cronjob" => ("batch", "CronJob"),
        "job" => ("batch", "Job"),
        _ => return None,
    })
}

fn rule_definitions(objects: &[DynamicObject], rule_kind: &ResourceKind) -> Vec<RuleDefinition> {
    let mut definitions = Vec::new();
    for object in objects {
        let (Some(name), Some(namespace)) = (
            object.metadata.name.as_ref(),
            object.metadata.namespace.as_ref(),
        ) else {
            continue;
        };
        let target = AlertResourceTarget {
            key: rule_kind.key.clone(),
            kind: rule_kind.kind.clone(),
            namespace: Some(namespace.clone()),
            name: name.clone(),
        };
        let Some(groups) = object
            .data
            .pointer("/spec/groups")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for rule in groups
            .iter()
            .filter_map(|group| group.get("rules"))
            .filter_map(serde_json::Value::as_array)
            .flatten()
        {
            let Some(alert_name) = rule.get("alert").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let labels = rule
                .get("labels")
                .and_then(serde_json::Value::as_object)
                .into_iter()
                .flatten()
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.clone(), value.to_string()))
                })
                .collect();
            definitions.push(RuleDefinition {
                alert_name: alert_name.to_string(),
                target: target.clone(),
                labels,
            });
        }
    }
    definitions
}

fn matching_rule_targets(
    alert: &FiringAlert,
    definitions: &[RuleDefinition],
) -> Vec<AlertResourceTarget> {
    let candidates: Vec<_> = definitions
        .iter()
        .filter(|definition| definition.alert_name == alert.name)
        .collect();
    let compatible: Vec<_> = candidates
        .iter()
        .copied()
        .filter_map(|definition| {
            definition
                .labels
                .iter()
                .filter(|(_, value)| !value.contains("{{"))
                .try_fold(0usize, |score, (name, value)| {
                    (alert.labels.get(name) == Some(value)).then_some(score + 1)
                })
                .map(|score| (definition, score))
        })
        .collect();
    let selected: Vec<_> = if let Some(max_score) = compatible.iter().map(|(_, score)| *score).max()
    {
        compatible
            .into_iter()
            .filter_map(|(definition, score)| (score == max_score).then_some(definition))
            .collect()
    } else {
        candidates
    };
    selected
        .into_iter()
        .map(|definition| definition.target.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use kube::core::{ApiResource, DynamicObject, GroupVersionKind};

    use super::*;
    use roder_core::Category;

    fn kind(group: &str, version: &str, kind: &str, namespaced: bool) -> ResourceKind {
        ResourceKind {
            key: ResourceKind::make_key(group, version, kind),
            group: group.to_string(),
            version: version.to_string(),
            kind: kind.to_string(),
            plural: format!("{}s", kind.to_ascii_lowercase()),
            namespaced,
            category: Category::Cluster,
        }
    }

    fn alert(name: &str, labels: HashMap<String, String>) -> FiringAlert {
        FiringAlert {
            fingerprint: "abc".into(),
            name: name.into(),
            severity: "warning".into(),
            summary: String::new(),
            description: String::new(),
            starts_at: String::new(),
            labels,
            silenced: false,
            targets: Vec::new(),
            defining_rules: Vec::new(),
        }
    }

    #[test]
    fn resolves_standard_targets_and_requires_namespaces() {
        let kinds = vec![
            kind("", "v1", "Pod", true),
            kind("", "v1", "Node", false),
            kind("apps", "v1", "Deployment", true),
            kind("batch", "v1", "Job", true),
        ];
        let labels = HashMap::from([
            ("namespace".into(), "production".into()),
            ("pod".into(), "api-123".into()),
            ("node".into(), "worker-1".into()),
            ("deployment".into(), "api".into()),
            ("job_name".into(), "migration".into()),
            ("job".into(), "kube-state-metrics".into()),
        ]);

        let targets = resolve_targets(&labels, &kinds);

        assert_eq!(targets.len(), 4);
        assert!(targets.iter().any(|target| {
            target.kind == "Node" && target.namespace.is_none() && target.name == "worker-1"
        }));
        assert!(!targets
            .iter()
            .any(|target| target.name == "kube-state-metrics"));
        assert!(
            resolve_targets(&HashMap::from([("pod".into(), "api-123".into())]), &kinds).is_empty()
        );
    }

    #[test]
    fn resolves_workload_and_custom_resource_conventions() {
        let kinds = vec![
            kind("apps", "v1", "StatefulSet", true),
            kind("example.io", "v1alpha1", "Widget", true),
        ];
        let labels = HashMap::from([
            ("namespace".into(), "default".into()),
            ("workload".into(), "database".into()),
            ("workload_type".into(), "stateful_set".into()),
            ("customresource_group".into(), "example.io".into()),
            ("customresource_version".into(), "v1alpha1".into()),
            ("customresource_kind".into(), "Widget".into()),
            ("name".into(), "sample".into()),
        ]);

        let targets = resolve_targets(&labels, &kinds);

        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|target| target.name == "database"));
        assert!(targets.iter().any(|target| target.name == "sample"));

        let mut older_version = labels;
        older_version.insert("customresource_version".into(), "v1beta1".into());
        let targets = resolve_targets(&older_version, &kinds);
        assert!(targets.iter().any(|target| {
            target.name == "sample" && target.key == "example.io/v1alpha1/Widget"
        }));
    }

    #[test]
    fn links_alerts_to_compatible_prometheus_rules() {
        let rule_kind = kind("monitoring.coreos.com", "v1", "PrometheusRule", true);
        let api_resource = ApiResource::from_gvk(&GroupVersionKind::gvk(
            "monitoring.coreos.com",
            "v1",
            "PrometheusRule",
        ));
        let mut object = DynamicObject::new("workload-rules", &api_resource);
        object.metadata.namespace = Some("monitoring".into());
        object.data = serde_json::json!({
            "spec": { "groups": [{ "rules": [
                { "alert": "ApiDown", "labels": { "severity": "critical" } },
                { "record": "api:requests:rate", "expr": "vector(0)" }
            ] }] }
        });
        let mut other = DynamicObject::new("warning-rules", &api_resource);
        other.metadata.namespace = Some("monitoring".into());
        other.data = serde_json::json!({
            "spec": { "groups": [{ "rules": [
                { "alert": "ApiDown", "labels": { "severity": "warning" } }
            ] }] }
        });
        let definitions = rule_definitions(&[object, other], &rule_kind);

        let critical = alert(
            "ApiDown",
            HashMap::from([("severity".into(), "critical".into())]),
        );
        let targets = matching_rule_targets(&critical, &definitions);

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "workload-rules");
        assert!(matching_rule_targets(&alert("Other", HashMap::new()), &definitions).is_empty());
    }
}
