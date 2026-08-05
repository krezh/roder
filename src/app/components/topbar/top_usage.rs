//! Cluster CPU/mem usage + node health in the top bar, with a hover tooltip of
//! per-node CPU/mem and ready status.

use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::state::Catalog;
use crate::app::util::format::{cluster_usage_pct, pct};
use crate::data;

#[component]
pub(crate) fn TopUsage() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let catalog = expect_context::<Catalog>().0;

    let overview = RwSignal::new(None::<roder_core::ClusterOverview>);
    // True when the most recent fetch failed — the displayed numbers (if any)
    // are then last-known-good rather than current.
    let stale = RwSignal::new(false);
    // Seed from the last-known overview so cluster usage doesn't flash empty on
    // refresh while the first `/api/overview` round-trip is in flight.
    Effect::new(move |_| {
        if let Some(cached) = data::storage_get("roder.overview")
            .and_then(|s| serde_json::from_str::<roder_core::ClusterOverview>(&s).ok())
        {
            overview.set(Some(cached));
        }
    });

    let apply = move |res: Result<roder_core::ClusterOverview, String>| match res {
        Ok(o) => {
            if let Ok(json) = serde_json::to_string(&o) {
                data::storage_set("roder.overview", &json);
            }
            overview.set(Some(o));
            stale.set(false);
        }
        Err(_) => stale.set(true),
    };

    let ov = LocalResource::new(|| async {
        data::fetch_json::<roder_core::ClusterOverview>("/api/overview").await
    });
    Effect::new(move |_| {
        if let Some(res) = ov.get() {
            apply(res);
        }
    });

    // Poll for fresh cluster usage on the same cadence as the backend's overview
    // cache TTL (8s, see overview.rs), so CPU/mem/node counts stay live instead
    // of freezing at whatever they were when the page first loaded.
    Effect::new(move |_| {
        set_interval(
            move || {
                #[cfg(target_arch = "wasm32")]
                leptos::task::spawn_local(async move {
                    apply(data::fetch_json::<roder_core::ClusterOverview>("/api/overview").await);
                });
            },
            std::time::Duration::from_secs(10),
        );
    });

    view! {
            {move || match overview.get() {
                None if stale.get() => view! {
                    <div class="topusage tu-error">"Usage unavailable"</div>
                }.into_any(),
                None => ().into_any(),
                Some(o) => {
                let nodes = o.nodes.clone();
                let (cpu_p, mem_p) = cluster_usage_pct(&nodes);
                let cpu_width = cpu_p.clamp(0.0, 100.0);
                let mem_width = mem_p.clamp(0.0, 100.0);
                let total = nodes.len();
                let ready = nodes.iter().filter(|n| n.ready).count();
                let nodes_ok = ready == total;
                let go_nodes = move |_| {
                    if let Some(nk) = catalog.get_untracked()
                        .into_iter()
                        .find(|k| k.group.is_empty() && k.kind == "Node")
                    {
                        selected_kind.set(Some(nk));
                    }
                };
                view! {
                    <div
                        class="topusage"
                        class:tu-stale=move || stale.get()
                        aria-label=format!(
                            "Cluster usage: CPU {cpu_p:.0}%, memory {mem_p:.0}%, {ready} of {total} nodes ready"
                        )
                    >
                        <span class="tu-meter"
                            class:tu-meter-warn={(75.0..90.0).contains(&cpu_p)}
                            class:tu-meter-error={cpu_p >= 90.0}>
                            <span class="tu-reading">
                                <span class="tu-label">"CPU"</span>
                                <b>{format!("{cpu_p:.0}%")}</b>
                            </span>
                            <span class="tu-track" aria-hidden="true">
                                <span class="tu-fill" style=format!("width:{cpu_width:.0}%")></span>
                            </span>
                        </span>
                        <span class="tu-meter"
                            class:tu-meter-warn={(75.0..90.0).contains(&mem_p)}
                            class:tu-meter-error={mem_p >= 90.0}>
                            <span class="tu-reading">
                                <span class="tu-label">"MEM"</span>
                                <b>{format!("{mem_p:.0}%")}</b>
                            </span>
                            <span class="tu-track" aria-hidden="true">
                                <span class="tu-fill" style=format!("width:{mem_width:.0}%")></span>
                            </span>
                        </span>
                        <button class="tu-nodes"
                            class:tu-nodes-warn=move || !nodes_ok
                            aria-label=format!("{ready} of {total} nodes ready; view nodes")
                            on:click=go_nodes>
                            <span class="tu-node-dot" aria-hidden="true"></span>
                            <b>{ready}"/"{total}</b>
                        </button>
                        {move || stale.get().then(|| view! {
                            <span class="tu-warn" aria-label="Usage data is stale"
                                data-tip="Failed to refresh — showing last known values">"!"
                            </span>
                        })}
                        <div class="tooltip usage-tip">
                            <div class="tip-row tip-head" aria-hidden="true">
                                <span>"Node"</span>
                                <span>"CPU"</span>
                                <span>"Mem"</span>
                            </div>
                            {nodes.into_iter().map(|n| {
                                let c = pct(n.cpu_used, n.cpu_cores);
                                let m = pct(n.mem_used, n.mem_bytes);
                                let c_width = c.clamp(0.0, 100.0);
                                let m_width = m.clamp(0.0, 100.0);
                                view! {
                                    <div class="tip-row">
                                        <span class="tip-node">
                                            <span
                                                class=if n.ready { "tip-node-status tip-node-ok" } else { "tip-node-status tip-node-err" }
                                                aria-label=if n.ready { "Ready" } else { "Not ready" }
                                            ></span>
                                            " "{n.name}
                                        </span>
                                        <span class="tip-usage" style=format!("--usage:{c_width:.0}%")>
                                            {format!("{c:.0}%")}
                                        </span>
                                        <span class="tip-usage" style=format!("--usage:{m_width:.0}%")>
                                            {format!("{m:.0}%")}
                                        </span>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                }.into_any()
                }
            }}
    }
}
