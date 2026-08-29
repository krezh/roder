//! Cluster overview start page: health, capacity, inventory, nodes, and warnings.

use leptos::prelude::*;
use roder_core::{
    ClusterOverview, HealthRollup, NodeSummary, OverviewWarning, ResourceKind, RowStatus,
};

use crate::app::components::table::StatusDot;
use crate::app::state::{Catalog, DetailTarget, Tick};
use crate::app::util::format::{
    camel_label, cluster_usage_pct, fmt_cores, fmt_mem, pct, talos_version,
};
use crate::data;

#[component]
pub(crate) fn Dashboard() -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let tick = expect_context::<Tick>().0;
    let overview = RwSignal::new(None::<ClusterOverview>);
    let load_error = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        if let Some(cached) = data::storage_get("roder.overview")
            .and_then(|value| serde_json::from_str::<ClusterOverview>(&value).ok())
        {
            overview.set(Some(cached));
        }
    });

    let resource =
        LocalResource::new(|| async { data::fetch_json::<ClusterOverview>("/api/overview").await });
    Effect::new(move |_| {
        let Some(result) = resource.get() else {
            return;
        };
        match result {
            Ok(value) => {
                if let Ok(json) = serde_json::to_string(&value) {
                    data::storage_set("roder.overview", &json);
                }
                overview.set(Some(value));
                load_error.set(None);
            }
            Err(error) => load_error.set(Some(error)),
        }
    });

    Effect::new(move |_| {
        if let Ok(handle) = set_interval_with_handle(
            move || resource.refetch(),
            std::time::Duration::from_secs(15),
        ) {
            on_cleanup(move || handle.clear());
        }
    });

    view! {
        <div class="dashboard">
            <header class="dashboard-head">
                <div>
                    <span class="dashboard-eyebrow">"Cluster"</span>
                    <h1>"Overview"</h1>
                </div>
                <div class="dashboard-actions">
                    {move || load_error.get().map(|_| view! {
                        <span class="dashboard-stale" role="status">"Showing last known data"</span>
                    })}
                    <button type="button" class="dashboard-refresh"
                        disabled=move || resource.get().is_none()
                        on:click=move |_| resource.refetch()>
                        {move || if resource.get().is_none() { "Refreshing" } else { "Refresh" }}
                    </button>
                </div>
            </header>

            {move || match overview.get() {
                Some(value) => dashboard_view(value, catalog, selected_kind, detail, tick).into_any(),
                None if load_error.get().is_some() => {
                    let message = load_error.get().unwrap_or_default();
                    view! {
                        <div class="dashboard-load-state dashboard-load-error" role="alert">
                            <span class="load-state-mark">"!"</span>
                            <h2>"Cluster overview unavailable"</h2>
                            <p>{message}</p>
                            <button type="button" class="dashboard-refresh"
                                on:click=move |_| resource.refetch()>"Try again"</button>
                        </div>
                    }.into_any()
                }
                None => view! {
                    <div class="dashboard-skeleton" aria-label="Loading cluster overview">
                        <div class="skeleton-block skeleton-wide"></div>
                        <div class="skeleton-block"></div>
                        <div class="skeleton-block"></div>
                        <div class="skeleton-block"></div>
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

fn select_kind(
    catalog: RwSignal<Vec<ResourceKind>>,
    selected_kind: RwSignal<Option<ResourceKind>>,
    kind: &str,
) {
    if let Some(resource) = catalog
        .get_untracked()
        .into_iter()
        .find(|resource| resource.kind == kind)
    {
        selected_kind.set(Some(resource));
    }
}

fn dashboard_view(
    o: ClusterOverview,
    catalog: RwSignal<Vec<ResourceKind>>,
    selected_kind: RwSignal<Option<ResourceKind>>,
    detail: RwSignal<Option<DetailTarget>>,
    tick: RwSignal<u32>,
) -> impl IntoView {
    let nodes = o.nodes.clone();
    let warnings = o.warnings.clone();
    let (cpu_p, mem_p) = cluster_usage_pct(&nodes);
    let cpu_available = nodes
        .iter()
        .any(|node| node.cpu_used.is_some() && node.cpu_cores.is_some());
    let mem_available = nodes
        .iter()
        .any(|node| node.mem_used.is_some() && node.mem_bytes.is_some());
    let ready_nodes = nodes.iter().filter(|node| node.ready).count();
    let unready_nodes = nodes.len().saturating_sub(ready_nodes);
    let controller_failing: usize = o
        .flux_resources
        .iter()
        .chain(&o.external_secret_resources)
        .chain(&o.kopiur_resources)
        .chain(&o.tuppr_resources)
        .map(|resource| resource.health.failing as usize)
        .sum();
    let controller_suspended: usize = o
        .flux_resources
        .iter()
        .chain(&o.external_secret_resources)
        .chain(&o.kopiur_resources)
        .chain(&o.tuppr_resources)
        .map(|resource| resource.health.suspended as usize)
        .sum();
    let failing = o.pod_failed as usize + controller_failing + unready_nodes;
    let caution = o.pod_pending as usize + controller_suspended + warnings.len();
    let (health_class, health_label, health_summary) = if failing > 0 {
        (
            "health-error",
            "Attention needed",
            format!(
                "{failing} failing signal{} across the cluster",
                if failing == 1 { "" } else { "s" }
            ),
        )
    } else if caution > 0 {
        (
            "health-warn",
            "Review recommended",
            format!(
                "{caution} warning signal{} to review",
                if caution == 1 { "" } else { "s" }
            ),
        )
    } else {
        (
            "health-ok",
            "Cluster healthy",
            "All tracked systems are operating normally".to_string(),
        )
    };

    let node_kind = catalog
        .get_untracked()
        .into_iter()
        .find(|resource| resource.group.is_empty() && resource.kind == "Node");
    let event_kind = catalog
        .get_untracked()
        .into_iter()
        .find(|resource| resource.group.is_empty() && resource.kind == "Event");

    view! {
        <section class=format!("cluster-health {health_class}") aria-label="Cluster health">
            <div class="health-mark" aria-hidden="true"></div>
            <div class="health-copy">
                <span class="health-label">{health_label}</span>
                <strong>{health_summary}</strong>
            </div>
            <div class="health-facts">
                <span><b>{ready_nodes}"/"{nodes.len()}</b> " nodes ready"</span>
                <span><b>{o.pod_running}"/"{o.pod_total}</b> " pods running"</span>
                <span><b>{warnings.len()}</b> " recent warnings"</span>
            </div>
        </section>

        <div class="dashboard-grid">
            <section class="card dashboard-card capacity-card">
                <div class="card-heading">
                    <div>
                        <span class="card-kicker">"Capacity"</span>
                        <h2>"Cluster usage"</h2>
                    </div>
                    <span class="card-meta">{nodes.len()} " nodes"</span>
                </div>
                {usage_meter("CPU", cpu_p, cpu_available)}
                {usage_meter("Memory", mem_p, mem_available)}
                {(!cpu_available || !mem_available).then(|| view! {
                    <p class="metrics-note">"Some usage metrics are unavailable. Capacity values are still shown per node."</p>
                })}
            </section>

            <button type="button" class="card dashboard-card inventory-card"
                on:click=move |_| select_kind(catalog, selected_kind, "Pod")>
                <div class="card-heading">
                    <div>
                        <span class="card-kicker">"Workloads"</span>
                        <h2>"Pods"</h2>
                    </div>
                </div>
                <strong class="inventory-total">{o.pod_total}</strong>
                <div class="inventory-breakdown">
                    <span class="ok"><i></i>{o.pod_running}" running"</span>
                    <span class="pending"><i></i>{o.pod_pending}" pending"</span>
                    <span class="error"><i></i>{o.pod_failed}" failed"</span>
                </div>
            </button>

            <button type="button" class="card dashboard-card inventory-card"
                on:click=move |_| select_kind(catalog, selected_kind, "Namespace")>
                <div class="card-heading">
                    <div>
                        <span class="card-kicker">"Inventory"</span>
                        <h2>"Namespaces"</h2>
                    </div>
                </div>
                <strong class="inventory-total">{o.namespace_count}</strong>
                <p class="inventory-caption">"Across Kubernetes " {o.kubernetes_version}</p>
            </button>
        </div>

        {(!o.flux_resources.is_empty()
            || !o.external_secret_resources.is_empty()
            || !o.kopiur_resources.is_empty()
            || !o.tuppr_resources.is_empty())
            .then(|| view! {
                <section class="dashboard-section" aria-labelledby="controllers-heading">
                    <div class="section-heading">
                        <span id="controllers-heading" class="section-kicker">"Controllers"</span>
                    </div>
                    <div class="controller-groups">
                        {(!o.flux_resources.is_empty()).then(|| view! {
                            <div class="controller-group">
                                <h2>"Flux"</h2>
                                <div class="controller-grid">
                                    {o.flux_resources.clone().into_iter().map(|resource| {
                                        rollup_card(
                                            resource_label(&resource.kind), resource.kind,
                                            resource.health, catalog, selected_kind
                                        )
                                    }).collect_view()}
                                </div>
                            </div>
                        })}
                        {(!o.external_secret_resources.is_empty()).then(|| view! {
                            <div class="controller-group">
                                <h2>"External Secrets"</h2>
                                <div class="controller-grid">
                                    {o.external_secret_resources.clone().into_iter().map(|resource| {
                                        rollup_card(
                                            resource_label(&resource.kind), resource.kind,
                                            resource.health, catalog, selected_kind
                                        )
                                    }).collect_view()}
                                </div>
                            </div>
                        })}
                        {(!o.kopiur_resources.is_empty()).then(|| view! {
                            <div class="controller-group">
                                <h2>"Kopiur"</h2>
                                <div class="controller-grid">
                                    {o.kopiur_resources.clone().into_iter().map(|resource| {
                                        rollup_card(
                                            resource_label(&resource.kind), resource.kind,
                                            resource.health, catalog, selected_kind
                                        )
                                    }).collect_view()}
                                </div>
                            </div>
                        })}
                        {(!o.tuppr_resources.is_empty()).then(|| view! {
                            <div class="controller-group">
                                <h2>"Tuppr"</h2>
                                <div class="controller-grid">
                                    {o.tuppr_resources.clone().into_iter().map(|resource| {
                                        rollup_card(
                                            resource_label(&resource.kind), resource.kind,
                                            resource.health, catalog, selected_kind
                                        )
                                    }).collect_view()}
                                </div>
                            </div>
                        })}
                    </div>
                </section>
            })}

        <section class="dashboard-section" aria-labelledby="nodes-heading">
            <div class="section-heading">
                <div>
                    <span class="section-kicker">"Infrastructure"</span>
                    <h2 id="nodes-heading">"Nodes"</h2>
                </div>
                <span class="section-caption">{ready_nodes}" of "{nodes.len()}" ready"</span>
            </div>
            <div class="nodes">
                {nodes.into_iter().map(|node| {
                    node_card(node, node_kind.clone(), selected_kind, detail)
                }).collect_view()}
            </div>
        </section>

        {(!warnings.is_empty()).then(|| view! {
            <section class="dashboard-section warnings-section" aria-labelledby="warnings-heading">
                <div class="section-heading">
                    <div>
                        <span class="section-kicker">"Event stream"</span>
                        <h2 id="warnings-heading">"Recent warnings"</h2>
                    </div>
                    <button type="button" class="section-link"
                        on:click=move |_| select_kind(catalog, selected_kind, "Event")>
                        "View all events"
                    </button>
                </div>
                <div class="warnings" role="list">
                    {warnings.into_iter().map(|warning| {
                        warning_row(warning, event_kind.clone(), detail, tick)
                    }).collect_view()}
                </div>
            </section>
        })}
    }
}

fn warning_row(
    warning: OverviewWarning,
    event_kind: Option<ResourceKind>,
    detail: RwSignal<Option<DetailTarget>>,
    tick: RwSignal<u32>,
) -> impl IntoView {
    let event_name = warning.event_name.clone();
    let namespace = warning.namespace.clone();
    let timestamp = warning.timestamp.clone();
    let can_open = event_kind.is_some() && !event_name.is_empty();
    let object = if warning.involved_kind.is_empty() {
        warning.involved_name.clone()
    } else {
        format!("{} / {}", warning.involved_kind, warning.involved_name)
    };

    view! {
        <button type="button" class="warning-row" role="listitem" disabled=!can_open
            on:click=move |_| {
                let Some(kind) = event_kind.clone() else { return; };
                detail.set(Some(DetailTarget {
                    key: kind.key.clone(),
                    namespace: namespace.clone(),
                    name: event_name.clone(),
                }));
            }>
            <span class="event-signal" aria-hidden="true"></span>
            <span class="event-content">
                <span class="event-head">
                    <strong>{warning.reason}</strong>
                    <span class="event-age">{move || {
                        tick.get();
                        data::humanize_age(&timestamp)
                    }}</span>
                </span>
                <span class="event-context">
                    {warning.namespace.map(|value| view! { <span class="event-namespace">{value}</span> })}
                    <span class="event-object">{object}</span>
                    {(!warning.source.is_empty()).then(|| view! { <span class="event-source">{warning.source}</span> })}
                </span>
                <span class="event-message">{warning.message}</span>
            </span>
            {(warning.count > 1).then(|| view! {
                <span class="event-count" data-tip="Occurrences">"×"{warning.count}</span>
            })}
        </button>
    }
}

fn usage_meter(label: &'static str, value: f64, available: bool) -> impl IntoView {
    let level = if value >= 90.0 {
        "meter-error"
    } else if value >= 75.0 {
        "meter-warn"
    } else {
        ""
    };
    let reading = if available {
        format!("{value:.0}%")
    } else {
        "—".to_string()
    };
    view! {
        <div class=format!("capacity-meter {level}") class:meter-unavailable=!available>
            <div class="capacity-reading">
                <span>{label}</span>
                <b>{reading}</b>
            </div>
            <div class="bar" role=available.then_some("meter") aria-label=format!("{label} usage")
                aria-valuenow=available.then(|| format!("{value:.0}")) aria-valuemin="0" aria-valuemax="100">
                <div class="fill" style=format!("width:{value:.0}%")></div>
            </div>
        </div>
    }
}

fn rollup_card(
    label: String,
    target_kind: String,
    rollup: HealthRollup,
    catalog: RwSignal<Vec<ResourceKind>>,
    selected_kind: RwSignal<Option<ResourceKind>>,
) -> impl IntoView {
    let unknown = rollup
        .total
        .saturating_sub(rollup.ready.saturating_add(rollup.failing))
        .saturating_sub(rollup.reconciling)
        .saturating_sub(rollup.suspended);
    let state = if rollup.failing > 0 {
        "controller-error"
    } else if rollup.reconciling > 0 {
        "controller-pending"
    } else if rollup.suspended > 0 {
        "controller-warn"
    } else {
        "controller-ok"
    };
    view! {
        <button type="button" class=format!("card controller-card {state}")
            on:click=move |_| select_kind(catalog, selected_kind, &target_kind)>
            <div class="controller-status" aria-hidden="true"></div>
            <div class="controller-main">
                <div class="card-heading">
                    <h3>{label}</h3>
                </div>
                <strong>{rollup.ready}" / "{rollup.total}</strong>
                <span>"ready"</span>
            </div>
            <div class="controller-counts">
                {(rollup.reconciling > 0).then(|| view! {
                    <span class="pending">{rollup.reconciling}" reconciling"</span>
                })}
                {(rollup.suspended > 0).then(|| view! {
                    <span class="warn">{rollup.suspended}" suspended"</span>
                })}
                {(rollup.failing > 0).then(|| view! {
                    <span class="error">{rollup.failing}" failing"</span>
                })}
                {(unknown > 0).then(|| view! {
                    <span class="unknown">{unknown}" status unknown"</span>
                })}
                {(rollup.reconciling == 0 && rollup.suspended == 0 && rollup.failing == 0 && unknown == 0).then(|| view! {
                    <span class="ok">"All reconciled"</span>
                })}
            </div>
        </button>
    }
}

fn resource_label(kind: &str) -> String {
    let label = camel_label(kind);
    if let Some(stem) = label.strip_suffix("Policy") {
        format!("{stem}Policies")
    } else if let Some(stem) = label.strip_suffix("Repository") {
        format!("{stem}Repositories")
    } else if let Some(stem) = label.strip_suffix("Class") {
        format!("{stem}Classes")
    } else {
        format!("{label}s")
    }
}

fn node_card(
    node: NodeSummary,
    node_kind: Option<ResourceKind>,
    selected_kind: RwSignal<Option<ResourceKind>>,
    detail: RwSignal<Option<DetailTarget>>,
) -> impl IntoView {
    let cpu_pct = pct(node.cpu_used, node.cpu_cores);
    let mem_pct = pct(node.mem_used, node.mem_bytes);
    let status = if node.ready {
        RowStatus::Ok
    } else {
        RowStatus::Error
    };
    let status_label = if node.ready { "Ready" } else { "Not ready" };
    let talos = node.os_image.as_deref().and_then(talos_version);
    let k8s = node.kubelet_version.clone();
    let name = node.name.clone();

    view! {
        <button type="button" class="card node"
            class:node-unready=!node.ready
            disabled=node_kind.is_none()
            on:click=move |_| {
                let Some(kind) = node_kind.clone() else { return; };
                detail.set(Some(DetailTarget {
                    key: kind.key.clone(),
                    namespace: None,
                    name: name.clone(),
                }));
                selected_kind.set(Some(kind));
            }>
            <div class="node-head">
                <StatusDot status=status />
                <span class="node-name">{node.name}</span>
                <span class="node-state">{status_label}</span>
            </div>
            <div class="node-ver">
                {talos.map(|version| view! { <span class="ver-chip">"Talos " {version}</span> })}
                {k8s.map(|version| view! { <span class="ver-chip">"k8s " {version}</span> })}
            </div>
            {node_meter("CPU", fmt_cores(node.cpu_used, node.cpu_cores), cpu_pct)}
            {node_meter("Memory", fmt_mem(node.mem_used, node.mem_bytes), mem_pct)}
        </button>
    }
}

fn node_meter(label: &'static str, reading: String, value: f64) -> impl IntoView {
    let level = if value >= 90.0 {
        "meter-error"
    } else if value >= 75.0 {
        "meter-warn"
    } else {
        ""
    };
    view! {
        <div class=format!("meter {level}")>
            <div class="meter-label">
                <span>{label}</span>
                <b>{reading}</b>
            </div>
            <div class="bar"><div class="fill" style=format!("width:{value:.0}%")></div></div>
        </div>
    }
}
