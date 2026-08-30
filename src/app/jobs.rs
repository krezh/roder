use leptos::prelude::*;
use roder_core::{ResourceTreeNode, ResourceTreeRelation, RowStatus};

use crate::app::state::DetailTarget;
use crate::app::util::color::dot_class;
use crate::data;

#[component]
pub(crate) fn CronJobJobs(target: DetailTarget) -> impl IntoView {
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let request = StoredValue::new(target);
    let jobs = LocalResource::new(move || {
        let target = request.get_value();
        async move {
            let namespace = target
                .namespace
                .as_deref()
                .map(data::percent_encode)
                .unwrap_or_default();
            let url = format!(
                "/api/resource-tree?key={}&namespace={namespace}&name={}",
                data::percent_encode(&target.key),
                data::percent_encode(&target.name),
            );
            data::fetch_json::<ResourceTreeNode>(&url).await
        }
    });

    view! {
        <div class="rd-body rd-jobs">
            <Suspense fallback=|| view! { <div class="muted pad">"Loading jobs..."</div> }>
                {move || jobs.get().map(|result| match result {
                        Err(error) => view! { <div class="error pad">{error}</div> }.into_any(),
                        Ok(tree) => {
                            let error = tree.error.clone();
                            let jobs = owned_jobs(tree);
                            if jobs.is_empty() {
                                match error {
                                    Some(error) => view! { <div class="error pad">{error}</div> }.into_any(),
                                    None => view! { <div class="muted pad">"No Jobs created by this CronJob."</div> }.into_any(),
                                }
                            } else {
                                view! {
                                    <div class="jobs-mini">
                                        {jobs.into_iter().map(|job| {
                                            let status = job.status.unwrap_or(RowStatus::Unknown);
                                            let status_label = match status {
                                                RowStatus::Ok => "Complete",
                                                RowStatus::Error => "Failed",
                                                RowStatus::Pending => "Running",
                                                RowStatus::Warn => "Warning",
                                                RowStatus::Done => "Complete",
                                                RowStatus::Unknown => "Unknown",
                                            };
                                            let job_target = job.key.map(|key| DetailTarget {
                                                key,
                                                namespace: job.namespace,
                                                name: job.name.clone(),
                                            });
                                            view! {
                                                <button class="job-mini-row" disabled=job_target.is_none()
                                                    on:click=move |_| detail.set(job_target.clone())>
                                                    <span class=format!("pm-dot {}", dot_class(status))></span>
                                                    <span class="pm-name">{job.name}</span>
                                                    <span class="pm-phase">{status_label}</span>
                                                </button>
                                            }
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            }
                        }
                })}
            </Suspense>
        </div>
    }
}

fn owned_jobs(tree: ResourceTreeNode) -> Vec<ResourceTreeNode> {
    tree.children
        .into_iter()
        .filter(|node| {
            node.group == "batch"
                && node.kind == "Job"
                && node.relation == Some(ResourceTreeRelation::OwnedResource)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: &str, relation: Option<ResourceTreeRelation>) -> ResourceTreeNode {
        ResourceTreeNode {
            kind: kind.into(),
            group: "batch".into(),
            name: kind.to_lowercase(),
            namespace: Some("default".into()),
            key: Some(format!("batch/v1/{kind}")),
            category: None,
            status: None,
            relation,
            expandable: false,
            children: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn keeps_only_jobs_owned_by_the_cronjob() {
        let mut tree = node("CronJob", None);
        tree.children = vec![
            node("Job", Some(ResourceTreeRelation::OwnedResource)),
            node("Job", Some(ResourceTreeRelation::Owner)),
            node("CronJob", Some(ResourceTreeRelation::OwnedResource)),
        ];

        let jobs = owned_jobs(tree);

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "job");
    }
}
