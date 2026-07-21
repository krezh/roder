//! Sort/filter logic shared by every resource-row list: the desktop grid
//! (`kind_table.rs`) and the mobile card lists. Kept as pure functions over
//! `ResourceRow` so both rendering layers can never disagree on ordering or
//! filtering behavior.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

use roder_core::{ResourceRow, RowStatus};

use crate::app::components::table::cmp_cell;
use crate::app::state::{DetailTarget, SortKey};
use crate::app::util::format::parse_key;
use crate::app::util::predicate::KindKind;

/// Natural (human) sort: numeric runs in strings are compared by value so that
/// "osd-2" < "osd-10" rather than "osd-10" < "osd-2" (lexicographic order).
pub(crate) fn nat_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a;
    let mut b = b;
    loop {
        match (a.is_empty(), b.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        let a_digit = a.starts_with(|c: char| c.is_ascii_digit());
        let b_digit = b.starts_with(|c: char| c.is_ascii_digit());
        if a_digit && b_digit {
            let an = a.find(|c: char| !c.is_ascii_digit()).unwrap_or(a.len());
            let bn = b.find(|c: char| !c.is_ascii_digit()).unwrap_or(b.len());
            let av: u64 = a[..an].parse().unwrap_or(0);
            let bv: u64 = b[..bn].parse().unwrap_or(0);
            let ord = av.cmp(&bv);
            if ord != Ordering::Equal {
                return ord;
            }
            a = &a[an..];
            b = &b[bn..];
        } else {
            let ac = a.chars().next().unwrap();
            let bc = b.chars().next().unwrap();
            let ord = ac.cmp(&bc);
            if ord != Ordering::Equal {
                return ord;
            }
            a = &a[ac.len_utf8()..];
            b = &b[bc.len_utf8()..];
        }
    }
}

/// Filter (problems-only + name substring) and sort rows into the uids to
/// display, in order. `filter_text_lower` must already be lowercased. Takes
/// an iterator (not a `&HashMap`) so callers whose rows live in a
/// differently-shaped map (e.g. keyed by kind-prefixed uid, or wrapping each
/// row in extra fields) don't have to clone into a fresh `HashMap` just to
/// call this.
pub(crate) fn shown_uids<'a>(
    rows: impl IntoIterator<Item = &'a ResourceRow>,
    sort_key: SortKey,
    asc: bool,
    only_problems: bool,
    filter_text_lower: &str,
) -> Vec<String> {
    let mut v: Vec<&ResourceRow> = rows
        .into_iter()
        .filter(|r| !only_problems || matches!(r.status, RowStatus::Error | RowStatus::Warn))
        .filter(|r| {
            filter_text_lower.is_empty() || r.name.to_lowercase().contains(filter_text_lower)
        })
        .collect();
    v.sort_by(|a, b| {
        let ord = compare_rows(a, b, sort_key);
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
    v.into_iter().map(|r| r.uid.clone()).collect()
}

/// The mixed-kind equivalent of [`shown_uids`], preserving each map key rather
/// than returning the row's raw uid.
pub(crate) fn shown_row_keys<'a>(
    rows: impl IntoIterator<Item = (&'a String, &'a ResourceRow)>,
    sort_key: SortKey,
    asc: bool,
    only_problems: bool,
    filter_text_lower: &str,
) -> Vec<String> {
    let mut rows = rows
        .into_iter()
        .filter(|(_, row)| {
            !only_problems || matches!(row.status, RowStatus::Error | RowStatus::Warn)
        })
        .filter(|(_, row)| {
            filter_text_lower.is_empty() || row.name.to_lowercase().contains(filter_text_lower)
        })
        .collect::<Vec<_>>();
    rows.sort_by(|(a_key, a), (b_key, b)| {
        let ord = compare_rows(a, b, sort_key).then_with(|| a_key.cmp(b_key));
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
    rows.into_iter().map(|(key, _)| key.clone()).collect()
}

fn compare_rows(a: &ResourceRow, b: &ResourceRow, sort_key: SortKey) -> Ordering {
    match sort_key {
        SortKey::Namespace => nat_cmp(
            a.namespace.as_deref().unwrap_or(""),
            b.namespace.as_deref().unwrap_or(""),
        )
        .then_with(|| nat_cmp(&a.name, &b.name)),
        SortKey::Name => nat_cmp(&a.name, &b.name),
        SortKey::Age => a
            .created
            .cmp(&b.created)
            .then_with(|| nat_cmp(&a.name, &b.name)),
        SortKey::Cell(i) => {
            cmp_cell(a.cells.get(i), b.cells.get(i)).then_with(|| nat_cmp(&a.name, &b.name))
        }
    }
    .then_with(|| a.uid.cmp(&b.uid))
}

/// Pick up to 2 columns to surface on a mobile card: a status-ish column
/// (Phase/Status/Ready) first if present, then fill remaining slots in
/// column order. Returns (label, value, colored-by-status) triples.
pub(crate) fn surfaced_cells(columns: &[String], cells: &[String]) -> Vec<(String, String, bool)> {
    let mut idx: Vec<usize> = Vec::new();
    if let Some(i) = columns
        .iter()
        .position(|c| matches!(c.as_str(), "Phase" | "Status" | "Ready"))
    {
        idx.push(i);
    }
    for i in 0..columns.len() {
        if idx.len() >= 2 {
            break;
        }
        if !idx.contains(&i) {
            idx.push(i);
        }
    }
    idx.into_iter()
        .filter_map(|i| {
            let label = columns.get(i)?.clone();
            let value = cells.get(i).cloned().unwrap_or_default();
            let colored = matches!(label.as_str(), "Phase" | "Status" | "Ready");
            Some((label, value, colored))
        })
        .collect()
}

/// Resolve a set of selected uids (single-kind table: every row shares `key`)
/// into bulk-action targets — shared by every mobile list's "do_bulk" closure.
pub(crate) fn bulk_targets(
    key: &str,
    rows: &HashMap<String, ResourceRow>,
    uids: &BTreeSet<String>,
) -> Vec<DetailTarget> {
    rows.values()
        .filter(|r| uids.contains(&r.uid))
        .map(|r| DetailTarget {
            key: key.to_string(),
            namespace: r.namespace.clone(),
            name: r.name.clone(),
        })
        .collect()
}

pub(crate) struct ResolvedActionTargets {
    pub(crate) uids: Vec<String>,
    pub(crate) targets: Vec<DetailTarget>,
}

/// Resolve the clicked row or its active multi-selection into action targets.
/// `row_targets` supplies kind-aware metadata for mixed-kind tables.
pub(crate) fn resolve_action_targets(
    clicked_uid: &str,
    clicked_target: &DetailTarget,
    selected: Option<&BTreeSet<String>>,
    rows: &HashMap<String, ResourceRow>,
    row_targets: &HashMap<String, DetailTarget>,
) -> ResolvedActionTargets {
    let uids = match selected {
        Some(selected) if selected.len() > 1 && selected.contains(clicked_uid) => {
            selected.iter().cloned().collect()
        }
        _ => vec![clicked_uid.to_string()],
    };
    let targets = uids
        .iter()
        .filter_map(|uid| {
            row_targets.get(uid).cloned().or_else(|| {
                rows.get(uid).map(|row| DetailTarget {
                    key: clicked_target.key.clone(),
                    namespace: row.namespace.clone(),
                    name: row.name.clone(),
                })
            })
        })
        .collect::<Vec<_>>();

    ResolvedActionTargets {
        uids,
        targets: if targets.is_empty() {
            vec![clicked_target.clone()]
        } else {
            targets
        },
    }
}

/// Whether every resolved target supports a kind-specific action. Mixed-kind
/// selections may only expose an action when it is valid for every target.
pub(crate) fn targets_all(
    targets: &[DetailTarget],
    predicate: impl for<'a> Fn(KindKind<'a>) -> bool,
) -> bool {
    !targets.is_empty()
        && targets.iter().all(|target| {
            let (group, kind) = parse_key(&target.key);
            predicate(KindKind::new(&group, &kind))
        })
}

#[cfg(test)]
mod tests {
    use super::{resolve_action_targets, targets_all};
    use crate::app::state::DetailTarget;
    use roder_core::{ResourceRow, RowStatus};
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    fn row(uid: &str, name: &str) -> ResourceRow {
        ResourceRow {
            uid: uid.to_string(),
            namespace: Some("default".to_string()),
            name: name.to_string(),
            created: None,
            cells: Vec::new(),
            trends: Vec::new(),
            status: RowStatus::Ok,
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn action_targets_preserve_each_selected_rows_kind() {
        let deployment_uid = "apps/v1/Deployment/deploy-uid";
        let service_uid = "/v1/Service/service-uid";
        let rows = HashMap::from([
            (deployment_uid.to_string(), row("deploy-uid", "api")),
            (service_uid.to_string(), row("service-uid", "api")),
        ]);
        let row_targets = HashMap::from([
            (
                deployment_uid.to_string(),
                DetailTarget {
                    key: "apps/v1/Deployment".to_string(),
                    namespace: Some("default".to_string()),
                    name: "api".to_string(),
                },
            ),
            (
                service_uid.to_string(),
                DetailTarget {
                    key: "/v1/Service".to_string(),
                    namespace: Some("default".to_string()),
                    name: "api".to_string(),
                },
            ),
        ]);
        let selected = BTreeSet::from([deployment_uid.to_string(), service_uid.to_string()]);

        let resolved = resolve_action_targets(
            deployment_uid,
            row_targets.get(deployment_uid).unwrap(),
            Some(&selected),
            &rows,
            &row_targets,
        );

        assert_eq!(resolved.targets.len(), 2);
        assert_eq!(resolved.targets[0].key, "/v1/Service");
        assert_eq!(resolved.targets[1].key, "apps/v1/Deployment");
    }

    #[test]
    fn kind_specific_actions_require_every_selected_target_to_match() {
        let targets = [
            DetailTarget {
                key: "kustomize.toolkit.fluxcd.io/v1/Kustomization".to_string(),
                namespace: Some("default".to_string()),
                name: "apps".to_string(),
            },
            DetailTarget {
                key: "batch/v1/CronJob".to_string(),
                namespace: Some("default".to_string()),
                name: "backup".to_string(),
            },
        ];
        assert!(!targets_all(&targets, |kind| kind.is_flux()));
        assert!(!targets_all(&targets, |kind| kind.is_cronjob()));
    }

    #[test]
    fn mixed_kind_sorting_returns_merged_keys() {
        let rows = HashMap::from([
            ("/v1/Pod/same".to_string(), row("same", "pod")),
            ("/v1/Service/same".to_string(), row("same", "service")),
        ]);

        let keys = super::shown_row_keys(
            rows.iter(),
            crate::app::state::SortKey::Name,
            true,
            false,
            "",
        );

        assert_eq!(keys, ["/v1/Pod/same", "/v1/Service/same"]);
    }
}
