//! Cluster overview start page: usage, node cards, Flux/ESO rollups, warnings.

use leptos::prelude::*;
use roder_core::{ClusterOverview, HealthRollup, NodeSummary, RowStatus};

use crate::app::components::table::StatusDot;
use crate::app::util::format::{cluster_usage_pct, fmt_cores, fmt_mem, pct, talos_version};
use crate::data;

#[component]
pub(crate) fn Dashboard() -> impl IntoView {
    let ov =
        LocalResource::new(|| async { data::fetch_json::<ClusterOverview>("/api/overview").await });
    view! {
        <div class="dashboard">
            <Suspense fallback=|| view! { <div class="pad muted">"Loading cluster overview…"</div> }>
                {move || ov.get().map(|res| match res {
                    Err(e) => view! { <div class="error pad">{format!("Failed to load overview: {e}")}</div> }.into_any(),
                    Ok(o) => dashboard_view(o).into_any(),
                })}
            </Suspense>
        </div>
    }
}

fn dashboard_view(o: ClusterOverview) -> impl IntoView {
    let nodes = o.nodes.clone();
    let (cpu_p, mem_p) = cluster_usage_pct(&nodes);
    view! {
        <div class="cards">
            <div class="card stat usage">
                <div class="stat-label">"Cluster usage"</div>
                <div class="usage-meter">
                    <span class="um-label">"CPU"</span>
                    <div class="bar"><div class="fill" style=format!("width:{cpu_p:.0}%")></div></div>
                    <span class="um-pct">{format!("{cpu_p:.0}%")}</span>
                </div>
                <div class="usage-meter">
                    <span class="um-label">"Mem"</span>
                    <div class="bar"><div class="fill" style=format!("width:{mem_p:.0}%")></div></div>
                    <span class="um-pct">{format!("{mem_p:.0}%")}</span>
                </div>
            </div>
            <div class="card stat">
                <div class="stat-num">{o.kubernetes_version.clone()}</div>
                <div class="stat-label">"Kubernetes"</div>
            </div>
            <div class="card stat">
                <div class="stat-num">{o.namespace_count}</div>
                <div class="stat-label">"Namespaces"</div>
            </div>
            <div class="card stat">
                <div class="stat-num">{o.pod_total}</div>
                <div class="stat-label">"Pods"</div>
                <div class="stat-sub">
                    <span class="ok">{o.pod_running}" running"</span>
                    {(o.pod_pending > 0).then(|| view!{ <span class="pending">", "{o.pod_pending}" pending"</span> })}
                    {(o.pod_failed > 0).then(|| view!{ <span class="error">", "{o.pod_failed}" failed"</span> })}
                </div>
            </div>
            {rollup_card("Flux", o.flux.clone())}
            {rollup_card("External Secrets", o.external_secrets.clone())}
        </div>

        <h3 class="section">"Nodes"</h3>
        <div class="nodes">{nodes.into_iter().map(node_card).collect_view()}</div>

        {(!o.warnings.is_empty()).then(|| view! {
            <div>
                <h3 class="section">"Recent warnings"</h3>
                <div class="warnings">
                    {o.warnings.into_iter().map(|w| view!{ <div class="warn-line">{w}</div> }).collect_view()}
                </div>
            </div>
        })}
    }
}

fn rollup_card(label: &str, r: HealthRollup) -> impl IntoView {
    let label = label.to_string();
    view! {
        <div class="card stat">
            <div class="stat-num">{r.ready}" / "{r.total}</div>
            <div class="stat-label">{label}" ready"</div>
            <div class="stat-sub">
                {(r.suspended > 0).then(|| view!{ <span class="warn">{r.suspended}" suspended "</span> })}
                {(r.failing > 0).then(|| view!{ <span class="error">{r.failing}" failing"</span> })}
            </div>
        </div>
    }
}

fn node_card(n: NodeSummary) -> impl IntoView {
    let cpu_pct = pct(n.cpu_used, n.cpu_cores);
    let mem_pct = pct(n.mem_used, n.mem_bytes);
    let status = if n.ready {
        RowStatus::Ok
    } else {
        RowStatus::Error
    };
    let talos = n.os_image.as_deref().and_then(talos_version);
    let k8s = n.kubelet_version.clone();
    view! {
        <div class="card node">
            <div class="node-head">
                <StatusDot status=status />
                <span class="node-name">{n.name.clone()}</span>
            </div>
            <div class="node-ver">
                {talos.map(|t| view! { <span class="ver-chip">"Talos " {t}</span> })}
                {k8s.map(|k| view! { <span class="ver-chip">"k8s " {k}</span> })}
            </div>
            <div class="meter">
                <div class="meter-label">"CPU "{fmt_cores(n.cpu_used, n.cpu_cores)}</div>
                <div class="bar"><div class="fill" style=format!("width:{cpu_pct:.0}%")></div></div>
            </div>
            <div class="meter">
                <div class="meter-label">"Mem "{fmt_mem(n.mem_used, n.mem_bytes)}</div>
                <div class="bar"><div class="fill" style=format!("width:{mem_pct:.0}%")></div></div>
            </div>
        </div>
    }
}
