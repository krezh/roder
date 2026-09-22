use std::collections::{BTreeSet, HashMap};

use leptos::prelude::*;
use roder_core::{ResourceRow, RowStatus};

use crate::app::controllers::detail::DetailTab;
use crate::app::hooks::use_sse_subscription;
use crate::app::state::{Catalog, DetailTarget, Tick};
use crate::app::util::color::dot_class;
use crate::data;

#[component]
pub(crate) fn CnpgBackupsTab(namespace: String, cluster: String) -> impl IntoView {
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let requested_tab = expect_context::<RwSignal<Option<DetailTab>>>();
    let catalog = expect_context::<Catalog>().0;
    let tick = expect_context::<Tick>().0;
    let backup_kind = Memo::new(move |_| {
        catalog
            .get()
            .into_iter()
            .find(|kind| kind.group == "postgresql.cnpg.io" && kind.kind == "Backup")
    });
    let rows = RwSignal::new(HashMap::<String, ResourceRow>::new());
    let columns = RwSignal::new(Vec::<String>::new());
    let entering = RwSignal::new(BTreeSet::new());
    let removing = RwSignal::new(BTreeSet::new());
    let watch_namespace = namespace.clone();
    let watch_error = use_sse_subscription(rows, entering, removing, Some(columns), move || {
        rows.set(HashMap::new());
        let kind = backup_kind.get()?;
        Some(data::watch_url(&kind.key, Some(&watch_namespace), None))
    });
    let shown_uids =
        Memo::new(move |_| matching_backup_uids(&rows.get(), &columns.get(), &cluster));

    view! {
        <div class="rd-body rd-cnpg-backups">
            <div class="cnpg-backups-mini">
                <For each=move || shown_uids.get() key=Clone::clone let:uid>
                    {
                        let row_uid = uid.clone();
                        let row = Memo::new(move |_| rows.with(|rows| rows.get(&row_uid).cloned()));
                        let cell = move |column: &str| {
                            let index = columns.get().iter().position(|candidate| candidate == column)?;
                            row.get()?.cells.get(index).cloned()
                        };
                        let phase = move || cell("Phase").filter(|value| !value.is_empty()).unwrap_or_else(|| "Pending".into());
                        let method = move || cell("Method").unwrap_or_default();
                        let started = move || display_time_cell(cell("Started"), tick);
                        let completed = move || display_time_cell(cell("Completed"), tick);
                        let open = move |_| {
                            if let (Some(row), Some(kind)) = (row.get_untracked(), backup_kind.get_untracked()) {
                                requested_tab.set(Some(DetailTab::Info));
                                detail.set(Some(DetailTarget {
                                    key: kind.key,
                                    namespace: row.namespace,
                                    name: row.name,
                                }));
                            }
                        };
                        view! {
                            <button class="cnpg-backup-row interactive-card" on:click=open>
                                <span class=move || format!("pm-dot {}", dot_class(row.get().map(|row| row.status).unwrap_or(RowStatus::Unknown)))></span>
                                <span class="pm-name">{move || row.get().map(|row| row.name).unwrap_or_default()}</span>
                                <span class="cnpg-backup-phase">{phase}</span>
                                <span class="cnpg-backup-method">{method}</span>
                                <span class="cnpg-backup-time"><small>"Started"</small>{started}</span>
                                <span class="cnpg-backup-time"><small>"Completed"</small>{completed}</span>
                            </button>
                        }
                    }
                </For>
            </div>
            {move || (shown_uids.get().is_empty() && watch_error.get().is_none()).then(|| view! {
                <div class="muted pad">"No Backups found for this Cluster."</div>
            })}
            {move || watch_error.get().map(|error| view! {
                <div class="act-err pad">{format!("Unable to watch Backups: {error}")}</div>
            })}
        </div>
    }
}

fn matching_backup_uids(
    rows: &HashMap<String, ResourceRow>,
    columns: &[String],
    cluster: &str,
) -> Vec<String> {
    let Some(cluster_index) = columns.iter().position(|column| column == "Cluster") else {
        return Vec::new();
    };
    let mut matching = rows
        .values()
        .filter(|row| {
            row.cells
                .get(cluster_index)
                .is_some_and(|value| value == cluster)
        })
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| {
        right
            .created
            .cmp(&left.created)
            .then_with(|| left.name.cmp(&right.name))
    });
    matching.into_iter().map(|row| row.uid.clone()).collect()
}

fn display_time_cell(value: Option<String>, tick: RwSignal<u32>) -> String {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return "-".into();
    };
    if data::cell_needs_tick(&value) {
        tick.get();
    }
    data::humanize_cell(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(uid: &str, cluster: &str, created: &str) -> ResourceRow {
        ResourceRow {
            uid: uid.into(),
            namespace: Some("database".into()),
            name: uid.into(),
            created: Some(created.into()),
            cells: vec![cluster.into()],
            trends: vec![],
            status: RowStatus::Done,
            suspended: false,
            labels: Default::default(),
        }
    }

    #[test]
    fn filters_backups_by_exact_cluster_and_sorts_newest_first() {
        let rows = HashMap::from([
            ("old".into(), row("old", "app", "2026-09-14T00:00:00Z")),
            ("new".into(), row("new", "app", "2026-09-15T00:00:00Z")),
            (
                "other".into(),
                row("other", "app-2", "2026-09-16T00:00:00Z"),
            ),
        ]);
        assert_eq!(
            matching_backup_uids(&rows, &["Cluster".into()], "app"),
            ["new", "old"]
        );
        assert!(matching_backup_uids(&rows, &["Phase".into()], "app").is_empty());
    }
}
