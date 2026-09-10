use leptos::prelude::*;
use roder_core::{ResourceKind, RowStatus};

use crate::app::failure_watch::FailureWatchRows;
use crate::app::overview::{core_kind, OverviewState};
use crate::app::state::{AlertsData, AlertsOpen, Catalog, OnlyProblems};
use crate::app::util::format::cluster_usage_pct;

fn go_to_kind(
    name: &str,
    catalog: RwSignal<Vec<ResourceKind>>,
    selected: RwSignal<Option<ResourceKind>>,
    problems: RwSignal<bool>,
) {
    if let Some(kind) = catalog
        .get_untracked()
        .into_iter()
        .find(|kind| kind.kind == name)
    {
        expect_context::<RwSignal<Option<String>>>().set(None);
        problems.set(true);
        selected.set(Some(kind));
    }
}

#[component]
pub(crate) fn MobileAlertActions() -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let problems = expect_context::<OnlyProblems>().0;
    let watches = expect_context::<FailureWatchRows>();
    let alerts = expect_context::<AlertsData>().0;
    let alerts_open = expect_context::<AlertsOpen>().0;
    let pods = Memo::new(move |_| {
        watches.pods.with(|rows| {
            rows.values()
                .filter(|row| row.status == RowStatus::Error)
                .count()
        })
    });
    let helm = Memo::new(move |_| {
        watches.helm_releases.with(|rows| {
            rows.values()
                .filter(|row| row.status == RowStatus::Error)
                .count()
        })
    });
    let kustomizations = Memo::new(move |_| {
        watches.kustomizations.with(|rows| {
            rows.values()
                .filter(|row| row.status == RowStatus::Error)
                .count()
        })
    });
    let alert_count = Memo::new(move |_| {
        alerts
            .get()
            .map(|values| values.iter().filter(|alert| !alert.silenced).count())
    });
    view! { <div class="mobile-alert-actions">
        {move || (pods.get() > 0).then(|| view! { <button class="mobile-failure-pill pods" on:click=move |_| {
            selected_ns.set(None); go_to_kind("Pod", catalog, selected, problems);
        }>{pods.get()}" Pods"</button> })}
        {move || {
            let count = helm.get() + kustomizations.get();
            (count > 0).then(|| view! { <button class="mobile-failure-pill flux" on:click=move |_| {
                selected_ns.set(None);
                let kind = if helm.get_untracked() > 0 { "HelmRelease" } else { "Kustomization" };
                go_to_kind(kind, catalog, selected, problems);
            }>{move || match (helm.get(), kustomizations.get()) {
                (helm, 0) => format!("HR {helm}"), (0, ks) => format!("KS {ks}"), (helm, ks) => format!("HR {helm} · KS {ks}"),
            }}</button> })
        }}
        {move || alert_count.get().map(|count| view! { <button class="mobile-alert-pill" class:firing={count > 0} aria-label=format!("{count} active alerts") on:click=move |_| alerts_open.set(true)>
            <span>"Alerts"</span><b>{count}</b>
        </button> })}
    </div> }
}

#[component]
pub(crate) fn MobileStatusRow() -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected = expect_context::<RwSignal<Option<ResourceKind>>>();
    let overview = expect_context::<OverviewState>();
    view! { <div class="mobile-status-row">{move || match overview.data.get() {
        None if overview.stale.get() => view! { <span class="mobile-usage-unavailable">"Usage unavailable"</span> }.into_any(),
        None => ().into_any(),
        Some(data) => {
            let (cpu, memory) = cluster_usage_pct(&data.nodes);
            let total = data.nodes.len(); let ready = data.nodes.iter().filter(|node| node.ready).count();
            view! { <div class="mobile-usage" class:stale=move || overview.stale.get() aria-label=format!("Cluster usage: CPU {cpu:.0}%, memory {memory:.0}%, {ready} of {total} nodes ready")>
                <span><small>"CPU"</small><b>{format!("{cpu:.0}%")}</b><i><em style:width=format!("{}%", cpu.clamp(0.0, 100.0))></em></i></span>
                <span><small>"MEM"</small><b>{format!("{memory:.0}%")}</b><i><em style:width=format!("{}%", memory.clamp(0.0, 100.0))></em></i></span>
                <button class:warning=ready != total on:click=move |_| if let Some(kind) = core_kind(&catalog.get_untracked(), "Node") { selected.set(Some(kind)); }>
                    <i></i><b>{ready}"/"{total}</b>
                </button>
            </div> }.into_any()
        }
    }}</div> }
}
