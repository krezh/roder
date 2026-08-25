//! Shared merged-row state transitions for desktop and mobile search results.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use leptos::prelude::*;
use roder_core::{ResourceKind, ResourceRow, WatchEvent};

use crate::app::events::UidSet;
use crate::app::overlays::toast::{show_toast_detail, Toast, ToastKind};

#[derive(Clone, PartialEq)]
pub(crate) struct MergedRow {
    pub(crate) kind: Arc<ResourceKind>,
    pub(crate) row: ResourceRow,
}

#[derive(Debug, PartialEq, Eq)]
enum PendingTransition {
    ClearEntering(String),
    Remove(String),
}

fn merged_key(kind_key: &str, uid: &str) -> String {
    format!("{kind_key}/{uid}")
}

fn reduce_columns(schemas: &mut HashMap<String, Vec<String>>, kind_key: &str, event: &WatchEvent) {
    if let WatchEvent::Snapshot { columns, .. } = event {
        schemas.insert(kind_key.to_string(), columns.clone());
    }
}

fn reduce_event(
    rows: &mut HashMap<String, MergedRow>,
    entering: &mut BTreeSet<String>,
    removing: &mut BTreeSet<String>,
    kind: &Arc<ResourceKind>,
    event: WatchEvent,
) -> Vec<PendingTransition> {
    let prefix = format!("{}/", kind.key);
    match event {
        WatchEvent::Snapshot { rows: snapshot, .. } => {
            rows.retain(|key, _| !key.starts_with(&prefix));
            entering.retain(|key| !key.starts_with(&prefix));
            removing.retain(|key| !key.starts_with(&prefix));
            for row in snapshot {
                rows.insert(
                    merged_key(&kind.key, &row.uid),
                    MergedRow {
                        kind: kind.clone(),
                        row,
                    },
                );
            }
            Vec::new()
        }
        WatchEvent::Applied { row } => {
            let key = merged_key(&kind.key, &row.uid);
            let is_new = !rows.contains_key(&key);
            rows.insert(
                key.clone(),
                MergedRow {
                    kind: kind.clone(),
                    row,
                },
            );
            removing.remove(&key);
            if is_new {
                entering.insert(key.clone());
                vec![PendingTransition::ClearEntering(key)]
            } else {
                Vec::new()
            }
        }
        WatchEvent::Deleted { uid } => {
            let key = merged_key(&kind.key, &uid);
            if rows.contains_key(&key) {
                removing.insert(key.clone());
                vec![PendingTransition::Remove(key)]
            } else {
                Vec::new()
            }
        }
        WatchEvent::Forbidden { .. } | WatchEvent::Error { .. } => Vec::new(),
    }
}

fn finish_removal_values(
    rows: &mut HashMap<String, MergedRow>,
    removing: &mut BTreeSet<String>,
    key: &str,
) {
    if removing.remove(key) {
        rows.remove(key);
    }
}

pub(crate) fn apply_event(
    rows: RwSignal<HashMap<String, MergedRow>>,
    entering: UidSet,
    removing: UidSet,
    columns: RwSignal<HashMap<String, Vec<String>>>,
    toast: RwSignal<Option<Toast>>,
    kind: Arc<ResourceKind>,
    event: WatchEvent,
) {
    if let WatchEvent::Forbidden { message } = &event {
        leptos::logging::warn!("watch forbidden for {}: {message}", kind.key);
    }
    if let WatchEvent::Error { message } = &event {
        leptos::logging::error!("watch failed for {}: {message}", kind.key);
        show_toast_detail(
            toast,
            format!("{} watch failed", kind.kind),
            Some(message.clone()),
            ToastKind::Err,
        );
    }
    columns.update(|schemas| reduce_columns(schemas, &kind.key, &event));

    let mut pending = Vec::new();
    rows.update(|row_map| {
        entering.update(|entering_set| {
            removing.update(|removing_set| {
                pending = reduce_event(row_map, entering_set, removing_set, &kind, event);
            });
        });
    });

    for transition in pending {
        match transition {
            PendingTransition::ClearEntering(key) => set_timeout(
                move || {
                    entering.update(|uids| {
                        uids.remove(&key);
                    });
                },
                Duration::from_millis(280),
            ),
            PendingTransition::Remove(key) => set_timeout(
                move || finish_removal(rows, removing, &key),
                Duration::from_millis(500),
            ),
        }
    }
}

pub(crate) fn finish_removal(
    rows: RwSignal<HashMap<String, MergedRow>>,
    removing: UidSet,
    key: &str,
) {
    rows.update(|row_map| {
        removing.update(|removing_set| finish_removal_values(row_map, removing_set, key));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use roder_core::{Category, RowStatus};
    use std::collections::BTreeMap;

    fn kind(key: &str, name: &str) -> Arc<ResourceKind> {
        Arc::new(ResourceKind {
            key: key.to_string(),
            group: String::new(),
            version: "v1".to_string(),
            kind: name.to_string(),
            plural: format!("{}s", name.to_lowercase()),
            namespaced: true,
            category: Category::Custom("test".to_string()),
        })
    }

    fn row(uid: &str, name: &str) -> ResourceRow {
        ResourceRow {
            uid: uid.to_string(),
            namespace: Some("default".to_string()),
            name: name.to_string(),
            created: None,
            cells: Vec::new(),
            trends: Vec::new(),
            status: RowStatus::Ok,
            suspended: false,
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn same_uid_from_different_kinds_stays_distinct() {
        let pod = kind("/v1/Pod", "Pod");
        let service = kind("/v1/Service", "Service");
        let mut rows = HashMap::new();
        let mut entering = BTreeSet::new();
        let mut removing = BTreeSet::new();

        reduce_event(
            &mut rows,
            &mut entering,
            &mut removing,
            &pod,
            WatchEvent::Applied {
                row: row("same", "pod"),
            },
        );
        reduce_event(
            &mut rows,
            &mut entering,
            &mut removing,
            &service,
            WatchEvent::Applied {
                row: row("same", "service"),
            },
        );

        assert_eq!(rows.len(), 2);
        assert!(rows.contains_key("/v1/Pod/same"));
        assert!(rows.contains_key("/v1/Service/same"));
    }

    #[test]
    fn recreation_cancels_pending_removal_and_timeout() {
        let pod = kind("/v1/Pod", "Pod");
        let key = "/v1/Pod/fallback-uid";
        let mut rows = HashMap::new();
        let mut entering = BTreeSet::new();
        let mut removing = BTreeSet::new();

        reduce_event(
            &mut rows,
            &mut entering,
            &mut removing,
            &pod,
            WatchEvent::Applied {
                row: row("fallback-uid", "old"),
            },
        );
        reduce_event(
            &mut rows,
            &mut entering,
            &mut removing,
            &pod,
            WatchEvent::Deleted {
                uid: "fallback-uid".to_string(),
            },
        );
        reduce_event(
            &mut rows,
            &mut entering,
            &mut removing,
            &pod,
            WatchEvent::Applied {
                row: row("fallback-uid", "new"),
            },
        );
        finish_removal_values(&mut rows, &mut removing, key);

        assert_eq!(rows.get(key).unwrap().row.name, "new");
        assert!(!removing.contains(key));
    }

    #[test]
    fn removal_timeout_drops_row_and_marker() {
        let pod = kind("/v1/Pod", "Pod");
        let key = "/v1/Pod/gone";
        let mut rows = HashMap::new();
        let mut entering = BTreeSet::new();
        let mut removing = BTreeSet::new();

        reduce_event(
            &mut rows,
            &mut entering,
            &mut removing,
            &pod,
            WatchEvent::Applied {
                row: row("gone", "gone"),
            },
        );
        reduce_event(
            &mut rows,
            &mut entering,
            &mut removing,
            &pod,
            WatchEvent::Deleted {
                uid: "gone".to_string(),
            },
        );
        finish_removal_values(&mut rows, &mut removing, key);

        assert!(!rows.contains_key(key));
        assert!(!removing.contains(key));
    }

    #[test]
    fn snapshot_replaces_only_its_kinds_rows() {
        let pod = kind("/v1/Pod", "Pod");
        let service = kind("/v1/Service", "Service");
        let mut rows = HashMap::new();
        let mut entering = BTreeSet::new();
        let mut removing = BTreeSet::new();
        reduce_event(
            &mut rows,
            &mut entering,
            &mut removing,
            &service,
            WatchEvent::Applied {
                row: row("service", "service"),
            },
        );

        reduce_event(
            &mut rows,
            &mut entering,
            &mut removing,
            &pod,
            WatchEvent::Snapshot {
                columns: vec!["Namespace".into(), "Name".into(), "Age".into()],
                rows: vec![row("pod", "pod")],
            },
        );

        assert!(rows.contains_key("/v1/Pod/pod"));
        assert!(rows.contains_key("/v1/Service/service"));
    }

    #[test]
    fn snapshot_schema_replaces_only_its_kind() {
        let mut schemas = HashMap::from([
            ("/v1/Pod".to_string(), vec!["Name".to_string()]),
            ("/v1/Service".to_string(), vec!["Name".to_string()]),
        ]);
        let event = WatchEvent::Snapshot {
            columns: vec![
                "Namespace".into(),
                "Name".into(),
                "Ready".into(),
                "Age".into(),
            ],
            rows: Vec::new(),
        };

        reduce_columns(&mut schemas, "/v1/Pod", &event);

        assert_eq!(schemas["/v1/Pod"], ["Namespace", "Name", "Ready", "Age"]);
        assert_eq!(schemas["/v1/Service"], ["Name"]);
    }
}
