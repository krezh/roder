use leptos::prelude::*;
use roder_core::{ClusterOverview, HealthRollup, NodeSummary, OverviewWarning, ResourceKind};

use crate::app::state::{Catalog, DetailTarget, Tick};
use crate::app::util::format::{
    camel_label, cluster_usage_pct, fmt_cores, fmt_mem, pct, talos_version,
};
use crate::data;

fn select_kind(
    catalog: RwSignal<Vec<ResourceKind>>,
    selected: RwSignal<Option<ResourceKind>>,
    name: &str,
) {
    if let Some(kind) = catalog
        .get_untracked()
        .into_iter()
        .find(|kind| kind.kind == name)
    {
        selected.set(Some(kind));
    }
}

#[component]
pub(crate) fn MobileDashboard() -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected = expect_context::<RwSignal<Option<ResourceKind>>>();
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let tick = expect_context::<Tick>().0;
    let overview = RwSignal::new(None::<ClusterOverview>);
    let error = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        if let Some(value) =
            data::storage_get("roder.overview").and_then(|value| serde_json::from_str(&value).ok())
        {
            overview.set(Some(value));
        }
    });
    let resource =
        LocalResource::new(|| async { data::fetch_json::<ClusterOverview>("/api/overview").await });
    Effect::new(move |_| {
        if let Some(result) = resource.get() {
            match result {
                Ok(value) => {
                    if let Ok(json) = serde_json::to_string(&value) {
                        data::storage_set("roder.overview", &json);
                    }
                    overview.set(Some(value));
                    error.set(None);
                }
                Err(value) => error.set(Some(value)),
            }
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
    view! { <div class="mobile-dashboard">
        <header class="mobile-dashboard-head"><div><small>"Cluster"</small><h1>"Overview"</h1></div><div>
            {move || error.get().map(|_| view! { <span>"Last known data"</span> })}
            <button disabled=move || resource.get().is_none() on:click=move |_| resource.refetch()>{move || if resource.get().is_none() { "Refreshing" } else { "Refresh" }}</button>
        </div></header>
        {move || match overview.get() {
            Some(value) => mobile_dashboard_sections(value, catalog, selected, detail, tick).into_any(),
            None if error.get().is_some() => view! { <div class="mobile-dashboard-error" role="alert"><b>"!"</b><h2>"Cluster overview unavailable"</h2><p>{error.get()}</p><button on:click=move |_| resource.refetch()>"Try again"</button></div> }.into_any(),
            None => view! { <div class="mobile-dashboard-loading" aria-label="Loading cluster overview"><i></i><i></i><i></i></div> }.into_any(),
        }}
    </div> }
}

fn mobile_dashboard_sections(
    overview: ClusterOverview,
    catalog: RwSignal<Vec<ResourceKind>>,
    selected: RwSignal<Option<ResourceKind>>,
    detail: RwSignal<Option<DetailTarget>>,
    tick: RwSignal<u32>,
) -> impl IntoView {
    let nodes = overview.nodes.clone();
    let warnings = overview.warnings.clone();
    let ready = nodes.iter().filter(|node| node.ready).count();
    let controller_failing: usize = overview
        .flux_resources
        .iter()
        .chain(&overview.external_secret_resources)
        .chain(&overview.kopiur_resources)
        .chain(&overview.tuppr_resources)
        .map(|item| item.health.failing as usize)
        .sum();
    let controller_suspended: usize = overview
        .flux_resources
        .iter()
        .chain(&overview.external_secret_resources)
        .chain(&overview.kopiur_resources)
        .chain(&overview.tuppr_resources)
        .map(|item| item.health.suspended as usize)
        .sum();
    let failing =
        overview.pod_failed as usize + controller_failing + nodes.len().saturating_sub(ready);
    let caution = overview.pod_pending as usize + controller_suspended + warnings.len();
    let (health, label, summary) = if failing > 0 {
        (
            "error",
            "Attention needed",
            format!(
                "{failing} failing signal{} across the cluster",
                if failing == 1 { "" } else { "s" }
            ),
        )
    } else if caution > 0 {
        (
            "warning",
            "Review recommended",
            format!(
                "{caution} warning signal{} to review",
                if caution == 1 { "" } else { "s" }
            ),
        )
    } else {
        (
            "ok",
            "Cluster healthy",
            "All tracked systems are operating normally".into(),
        )
    };
    let (cpu, memory) = cluster_usage_pct(&nodes);
    let cpu_available = nodes
        .iter()
        .any(|node| node.cpu_used.is_some() && node.cpu_cores.is_some());
    let memory_available = nodes
        .iter()
        .any(|node| node.mem_used.is_some() && node.mem_bytes.is_some());
    let node_kind = catalog
        .get_untracked()
        .into_iter()
        .find(|kind| kind.group.is_empty() && kind.kind == "Node");
    let event_kind = catalog
        .get_untracked()
        .into_iter()
        .find(|kind| kind.group.is_empty() && kind.kind == "Event");
    view! {
        <section class=format!("mobile-cluster-health {health}")><i></i><div><small>{label}</small><strong>{summary}</strong></div>
            <dl><div><dt>"Nodes"</dt><dd>{ready}"/"{nodes.len()}</dd></div><div><dt>"Pods"</dt><dd>{overview.pod_running}"/"{overview.pod_total}</dd></div><div><dt>"Warnings"</dt><dd>{warnings.len()}</dd></div></dl>
        </section>
        <section class="mobile-dashboard-grid">
            <article class="mobile-dashboard-card mobile-capacity"><header><div><small>"Capacity"</small><h2>"Cluster usage"</h2></div><span>{nodes.len()}" nodes"</span></header>
                {mobile_usage_meter("CPU", cpu, cpu_available)}{mobile_usage_meter("Memory", memory, memory_available)}
                {(!cpu_available || !memory_available).then(|| view! { <p>"Some usage metrics are unavailable. Capacity values are still shown per node."</p> })}
            </article>
            <button class="mobile-dashboard-card mobile-inventory" on:click=move |_| select_kind(catalog, selected, "Pod")><small>"Workloads"</small><h2>"Pods"</h2><strong>{overview.pod_total}</strong>
                <span><i class="ok"></i>{overview.pod_running}" running"</span><span><i class="pending"></i>{overview.pod_pending}" pending"</span><span><i class="error"></i>{overview.pod_failed}" failed"</span>
            </button>
            <button class="mobile-dashboard-card mobile-inventory" on:click=move |_| select_kind(catalog, selected, "Namespace")><small>"Inventory"</small><h2>"Namespaces"</h2><strong>{overview.namespace_count}</strong><span>"Kubernetes "{overview.kubernetes_version}</span></button>
        </section>
        {mobile_controller_group("Flux", overview.flux_resources, catalog, selected)}
        {mobile_controller_group("External Secrets", overview.external_secret_resources, catalog, selected)}
        {mobile_controller_group("Kopiur", overview.kopiur_resources, catalog, selected)}
        {mobile_controller_group("Tuppr", overview.tuppr_resources, catalog, selected)}
        <section class="mobile-dashboard-section"><header><div><small>"Infrastructure"</small><h2>"Nodes"</h2></div><span>{ready}" of "{nodes.len()}" ready"</span></header>
            <div class="mobile-node-list">{nodes.into_iter().map(|node| mobile_node(node, node_kind.clone(), selected, detail)).collect_view()}</div>
        </section>
        {(!warnings.is_empty()).then(|| view! { <section class="mobile-dashboard-section mobile-warning-section"><header><div><small>"Event stream"</small><h2>"Recent warnings"</h2></div>
            <button on:click=move |_| select_kind(catalog, selected, "Event")>"View all"</button></header>
            <div>{warnings.into_iter().map(|warning| mobile_warning(warning, event_kind.clone(), detail, tick)).collect_view()}</div>
        </section> })}
    }
}

fn mobile_usage_meter(label: &'static str, value: f64, available: bool) -> impl IntoView {
    view! { <div class="mobile-capacity-meter" class:warning={value >= 75.0} class:error={value >= 90.0}><span><small>{label}</small><b>{if available { format!("{value:.0}%") } else { "—".into() }}</b></span><i><em style:width=format!("{value:.0}%")></em></i></div> }
}

fn mobile_controller_group(
    title: &'static str,
    resources: Vec<roder_core::ResourceHealthRollup>,
    catalog: RwSignal<Vec<ResourceKind>>,
    selected: RwSignal<Option<ResourceKind>>,
) -> impl IntoView {
    (!resources.is_empty()).then(|| view! { <section class="mobile-dashboard-section mobile-controller-section"><header><div><small>"Controllers"</small><h2>{title}</h2></div></header><div class="mobile-controller-grid">
        {resources.into_iter().map(|resource| mobile_rollup(resource.kind, resource.health, catalog, selected)).collect_view()}
    </div></section> })
}

fn mobile_rollup(
    kind: String,
    health: HealthRollup,
    catalog: RwSignal<Vec<ResourceKind>>,
    selected: RwSignal<Option<ResourceKind>>,
) -> impl IntoView {
    let unknown = health
        .total
        .saturating_sub(health.ready + health.failing)
        .saturating_sub(health.reconciling)
        .saturating_sub(health.suspended);
    let state = if health.failing > 0 {
        "error"
    } else if health.reconciling > 0 {
        "pending"
    } else if health.suspended > 0 {
        "warning"
    } else {
        "ok"
    };
    let target = kind.clone();
    let label = {
        let label = camel_label(&kind);
        if let Some(stem) = label.strip_suffix("Policy") {
            format!("{stem}Policies")
        } else if let Some(stem) = label.strip_suffix("Repository") {
            format!("{stem}Repositories")
        } else if let Some(stem) = label.strip_suffix("Class") {
            format!("{stem}Classes")
        } else {
            format!("{label}s")
        }
    };
    view! { <button class=format!("mobile-controller-card {state}") on:click=move |_| select_kind(catalog, selected, &target)><i></i><span><strong>{label}</strong><b>{health.ready}" / "{health.total}</b><small>"ready"</small></span><em>
        {(health.reconciling > 0).then(|| view! { <span>{health.reconciling}" reconciling"</span> })}{(health.suspended > 0).then(|| view! { <span>{health.suspended}" suspended"</span> })}
        {(health.failing > 0).then(|| view! { <span>{health.failing}" failing"</span> })}{(unknown > 0).then(|| view! { <span>{unknown}" unknown"</span> })}
        {(health.reconciling == 0 && health.suspended == 0 && health.failing == 0 && unknown == 0).then(|| view! { <span>"All reconciled"</span> })}
    </em></button> }
}

fn mobile_node(
    node: NodeSummary,
    kind: Option<ResourceKind>,
    selected: RwSignal<Option<ResourceKind>>,
    detail: RwSignal<Option<DetailTarget>>,
) -> impl IntoView {
    let name = node.name.clone();
    let cpu = pct(node.cpu_used, node.cpu_cores);
    let memory = pct(node.mem_used, node.mem_bytes);
    view! { <button class="mobile-node-card" class:error=!node.ready disabled=kind.is_none() on:click=move |_| if let Some(kind) = kind.clone() {
        detail.set(Some(DetailTarget { key: kind.key.clone(), namespace: None, name: name.clone() })); selected.set(Some(kind));
    }><header><i></i><strong>{node.name}</strong><span>{if node.ready { "Ready" } else { "Not ready" }}</span></header><div class="mobile-node-versions">
        {node.os_image.as_deref().and_then(talos_version).map(|value| view! { <span>"Talos "{value}</span> })}{node.kubelet_version.map(|value| view! { <span>"k8s "{value}</span> })}
    </div>{mobile_node_meter("CPU", fmt_cores(node.cpu_used, node.cpu_cores), cpu)}{mobile_node_meter("Memory", fmt_mem(node.mem_used, node.mem_bytes), memory)}</button> }
}

fn mobile_node_meter(label: &'static str, reading: String, value: f64) -> impl IntoView {
    view! { <div class="mobile-node-meter" class:warning={value >= 75.0} class:error={value >= 90.0}><span><small>{label}</small><b>{reading}</b></span><i><em style:width=format!("{value:.0}%")></em></i></div> }
}

fn mobile_warning(
    warning: OverviewWarning,
    kind: Option<ResourceKind>,
    detail: RwSignal<Option<DetailTarget>>,
    tick: RwSignal<u32>,
) -> impl IntoView {
    let event_name = warning.event_name.clone();
    let namespace = warning.namespace.clone();
    let timestamp = warning.timestamp.clone();
    let object = if warning.involved_kind.is_empty() {
        warning.involved_name.clone()
    } else {
        format!("{} / {}", warning.involved_kind, warning.involved_name)
    };
    view! { <button class="mobile-warning-row" disabled=kind.is_none() || event_name.is_empty() on:click=move |_| if let Some(kind) = kind.clone() { detail.set(Some(DetailTarget { key: kind.key, namespace: namespace.clone(), name: event_name.clone() })); }>
        <i></i><span><span><strong>{warning.reason}</strong><small>{move || { tick.get(); data::humanize_age(&timestamp) }}</small></span><em>
            {warning.namespace.map(|value| view! { <b>{value}</b> })}<span>{object}</span>{(!warning.source.is_empty()).then(|| view! { <span>{warning.source}</span> })}
        </em><p>{warning.message}</p></span>{(warning.count > 1).then(|| view! { <b>"×"{warning.count}</b> })}
    </button> }
}
