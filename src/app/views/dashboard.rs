//! Cluster overview start page: health, capacity, inventory, nodes, and warnings.

use leptos::prelude::*;
use roder_core::{
    ClusterOverview, ControllerHealthSignal, NodeSummary, OverviewWarning, ResourceHealthRollup,
    ResourceKind, RowStatus,
};

use crate::app::alert_utils::elapsed_since_ms;
use crate::app::components::table::StatusDot;
use crate::app::overview::{
    cluster_health, controller_rollup, controller_signal_state, core_kind, kind_for_target,
    ControllerState, HealthState, OverviewState,
};
use crate::app::state::{Catalog, DetailTarget, Tick};
use crate::app::ui::StalenessRing;
use crate::app::util::format::{cluster_usage_pct, fmt_cores, fmt_mem, pct, talos_version};
use crate::data;

#[component]
pub(crate) fn Dashboard() -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let tick = expect_context::<Tick>().0;
    let overview = expect_context::<OverviewState>();

    view! {
        <div class="dashboard">
            <header class="dashboard-head">
                <div>
                    <span class="dashboard-eyebrow">"Cluster"</span>
                    <h1>"Overview"</h1>
                </div>
                <div class="dashboard-actions">
                    {move || (overview.error.get().is_some() && overview.data.get().is_some()).then(|| view! {
                        <span class="dashboard-stale" role="status">"Showing last known data"</span>
                    })}
                    <button type="button" class="dashboard-refresh"
                        disabled=move || overview.refreshing()
                        data-tip=move || {
                            tick.track();
                            refresh_status(overview.last_refresh.get())
                        }
                        aria-label=move || {
                            tick.track();
                            format!("Refresh cluster overview. {}", refresh_status(overview.last_refresh.get()))
                        }
                        on:click=move |_| overview.refresh()>
                        // Label stays fixed so the button keeps its width; the
                        // ring carries how stale the data is.
                        "Refresh"
                        <StalenessRing
                            next_refresh=overview.next_refresh
                            period_secs=crate::app::overview::OVERVIEW_POLL_SECS
                        />
                    </button>
                </div>
            </header>

            {move || match overview.data.get() {
                Some(value) => dashboard_view(value, catalog, selected_kind, detail, tick).into_any(),
                None if overview.error.get().is_some() => {
                    let message = overview.error.get().unwrap_or_default();
                    view! {
                        <div class="dashboard-load-state dashboard-load-error" role="alert">
                            <span class="load-state-mark">"!"</span>
                            <h2>"Cluster overview unavailable"</h2>
                            <p>{message}</p>
                            <button type="button" class="dashboard-refresh"
                                disabled=move || overview.refreshing()
                                on:click=move |_| overview.refresh()>"Try again"</button>
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

fn refresh_status(last_refresh_ms: Option<f64>) -> String {
    let Some(timestamp) = last_refresh_ms else {
        return "Not refreshed yet".to_string();
    };
    elapsed_since_ms(timestamp).map_or_else(
        || "Last refreshed".into(),
        |age| format!("Last refreshed {age} ago"),
    )
}

fn select_kind(
    catalog: RwSignal<Vec<ResourceKind>>,
    selected_kind: RwSignal<Option<ResourceKind>>,
    key_or_kind: &str,
) {
    if let Some(resource) = kind_for_target(&catalog.get_untracked(), key_or_kind) {
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
    let controller_groups = o.controller_groups.clone();
    let (cpu_p, mem_p) = cluster_usage_pct(&nodes);
    let cpu_available = nodes
        .iter()
        .any(|node| node.cpu_used.is_some() && node.cpu_cores.is_some());
    let mem_available = nodes
        .iter()
        .any(|node| node.mem_used.is_some() && node.mem_bytes.is_some());
    let health = cluster_health(&o);
    let health_class = match health.state {
        HealthState::Ok => "health-ok",
        HealthState::Warning => "health-warn",
        HealthState::Error => "health-error",
    };
    let catalog_snapshot = catalog.get();
    let node_kind = core_kind(&catalog_snapshot, "Node");
    let event_kind = core_kind(&catalog_snapshot, "Event");

    view! {
        <section class=format!("cluster-health {health_class}") aria-label="Cluster health">
            <div class="health-mark" aria-hidden="true"></div>
            <div class="health-copy">
                <span class="health-label">{health.label}</span>
                <strong>{health.summary}</strong>
            </div>
            <div class="health-facts">
                <span><b>{health.ready_nodes}"/"{nodes.len()}</b> " nodes ready"</span>
                <span><b>{o.pod_running}"/"{o.pod_total}</b> " pods running"</span>
                <span><b>{warnings.len()}</b> " recent warnings"</span>
            </div>
        </section>

        <section class="dashboard-section" aria-labelledby="infrastructure-heading">
            <div class="section-heading">
                <span id="infrastructure-heading" class="section-kicker">"Infrastructure"</span>
            </div>
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

                <button type="button" class="card dashboard-card inventory-card interactive-card"
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

                <button type="button" class="card dashboard-card inventory-card interactive-card"
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

            <section class="dashboard-section" aria-labelledby="nodes-heading">
                <div class="section-heading">
                    <h2 id="nodes-heading">"Nodes"</h2>
                    <span class="section-caption">{health.ready_nodes}" of "{nodes.len()}" ready"</span>
                </div>
                <div class="nodes">
                    {nodes.into_iter().map(|node| {
                        node_card(node, node_kind.clone(), selected_kind, detail)
                    }).collect_view()}
                </div>
            </section>
        </section>

        {(!controller_groups.is_empty()).then(|| view! {
                <section class="dashboard-section" aria-labelledby="controllers-heading">
                    <div class="section-heading">
                        <span id="controllers-heading" class="section-kicker">"Controllers"</span>
                    </div>
                    <div class="controller-groups">
                        {controller_groups.into_iter().map(|group| view! {
                            <div class="controller-group">
                                <h2>{group.name}</h2>
                                <div class="controller-grid">
                                    {group.resources.into_iter().map(|resource| {
                                        rollup_card(resource, catalog, selected_kind)
                                    }).collect_view()}
                                    {group.signals.into_iter().map(|signal| {
                                        signal_card(signal, tick)
                                    }).collect_view()}
                                </div>
                            </div>
                        }).collect_view()}
                    </div>
                </section>
            })}

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
        <button type="button" class="warning-row interactive-card" role="listitem" disabled=!can_open
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
    resource: ResourceHealthRollup,
    catalog: RwSignal<Vec<ResourceKind>>,
    selected_kind: RwSignal<Option<ResourceKind>>,
) -> impl IntoView {
    let derived = controller_rollup(&resource);
    let label = derived.label;
    let target = derived.target;
    let rollup = resource.health;
    let error = resource.error.unwrap_or_default();
    let unreadable = derived.unreadable;
    let only_unclassified = derived.only_unclassified;
    let state = match derived.state {
        ControllerState::Ok => "controller-ok",
        ControllerState::Neutral => "controller-neutral",
        ControllerState::Warning => "controller-warn",
        ControllerState::Pending => "controller-pending",
        ControllerState::Error => "controller-error",
    };
    view! {
        <button type="button" class=format!("card controller-card interactive-card {state}")
            data-tip=error
            on:click=move |_| select_kind(catalog, selected_kind, &target)>
            <div class="controller-status" aria-hidden="true"></div>
            <div class="controller-main">
                <div class="card-heading">
                    <h3>{label}</h3>
                </div>
                <strong>{if only_unclassified {
                    rollup.total.to_string()
                } else {
                    format!("{} / {}", rollup.ready, rollup.total)
                }}</strong>
                <span>{if only_unclassified { "resources" } else { "ready" }}</span>
            </div>
            <div class="controller-counts">
                {(rollup.reconciling > 0).then(|| view! {
                    <span class="pending">{rollup.reconciling}" reconciling"</span>
                })}
                {(rollup.suspended > 0).then(|| view! {
                    <span class="warn">{rollup.suspended}" suspended"</span>
                })}
                {(rollup.warning > 0).then(|| view! {
                    <span class="warn">{rollup.warning}" warning"</span>
                })}
                {(rollup.failing > 0).then(|| view! {
                    <span class="error">{rollup.failing}" failing"</span>
                })}
                {(rollup.unknown > 0).then(|| view! {
                    <span class="warn">{rollup.unknown}" health status unrecognized"</span>
                })}
                {(rollup.unreported > 0).then(|| view! {
                    <span class="unknown">{rollup.unreported}" health status not reported"</span>
                })}
                {unreadable.then(|| view! {
                    <span class="error">"unreadable"</span>
                })}
                {(rollup.reconciling == 0 && rollup.suspended == 0 && rollup.warning == 0 && rollup.failing == 0 && rollup.unknown == 0 && rollup.unreported == 0 && !unreadable).then(|| view! {
                    <span class="ok">"All reconciled"</span>
                })}
            </div>
        </button>
    }
}

fn signal_card(signal: ControllerHealthSignal, tick: RwSignal<u32>) -> impl IntoView {
    let state = match controller_signal_state(signal.status) {
        ControllerState::Ok => "controller-ok",
        ControllerState::Neutral => "controller-neutral",
        ControllerState::Warning => "controller-warn",
        ControllerState::Pending => "controller-pending",
        ControllerState::Error => "controller-error",
    };
    let timestamp = signal.timestamp;
    let fallback = signal.value;
    view! {
        <article class=format!("card controller-card controller-signal {state}") data-tip=signal.message>
            <div class="controller-status" aria-hidden="true"></div>
            <div class="controller-main">
                <div class="card-heading"><h3>{signal.label}</h3></div>
                <strong>{move || {
                    tick.get();
                    timestamp
                        .as_ref()
                        .map(|value| data::humanize_age(&Some(value.clone())))
                        .unwrap_or_else(|| fallback.clone())
                }}</strong>
            </div>
        </article>
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
        <button type="button" class="card node interactive-card"
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
