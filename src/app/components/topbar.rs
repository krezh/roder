//! Top bar: hamburger, brand, connection dot, palette button, error filter,
//! namespace selector, failing-pod badge, cluster usage (with node health),
//! and identity.

use leptos::prelude::*;
use roder_core::{ResourceKind, ResourceRow, RowStatus};

use crate::app::components::icons::ShiftIcon;
use crate::app::hooks::use_sse_subscription;
use crate::app::overlays::confirm::Confirm;
use crate::app::state::{
    Catalog, ConnectionState, NavOpen, NsPaletteOpen, OnlyProblems, PaletteOpen,
};
use crate::app::util::format::pct;
use crate::data;

#[component]
pub(crate) fn Topbar() -> impl IntoView {
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let palette_open = expect_context::<PaletteOpen>().0;
    let ns_palette_open = expect_context::<NsPaletteOpen>().0;
    let only_problems = expect_context::<OnlyProblems>().0;

    view! {
        <header class="topbar">
            <button class="hamburger" on:click=move |_| nav_open.update(|o| *o = !*o)>"☰"</button>
            <Brand />
            <button class="palette-btn" on:click=move |_| palette_open.set(true)>
                "Search " <kbd><ShiftIcon />"K"</kbd>
            </button>
            <button class="errfilter" class:active=move || only_problems.get()
                on:click=move |_| only_problems.update(|o| *o = !*o)>
                "Errors " <kbd><ShiftIcon />"E"</kbd>
            </button>
            <button class="ns-palette-btn" on:click=move |_| ns_palette_open.set(true)>
                {move || selected_ns.get().unwrap_or_else(|| "All namespaces".to_string())}
                " " <kbd><ShiftIcon />"N"</kbd>
            </button>
            <SanitizeButton />
            <FailingBadge />
            <FluxFailingBadge />
            <TopUsage />
            <Identity />
        </header>
    }
}

/// Live count of failing Flux Kustomizations and HelmReleases.
#[component]
fn FluxFailingBadge() -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let only_problems = expect_context::<OnlyProblems>().0;

    let ks_kind = Memo::new(move |_| {
        catalog
            .get()
            .into_iter()
            .find(|k| k.group.ends_with("fluxcd.io") && k.kind == "Kustomization")
    });
    let hr_kind = Memo::new(move |_| {
        catalog
            .get()
            .into_iter()
            .find(|k| k.group.ends_with("fluxcd.io") && k.kind == "HelmRelease")
    });

    let ks_rows = RwSignal::new(std::collections::HashMap::<String, ResourceRow>::new());
    let ks_entering = RwSignal::new(std::collections::BTreeSet::<String>::new());
    let ks_removing = RwSignal::new(std::collections::BTreeSet::<String>::new());
    let hr_rows = RwSignal::new(std::collections::HashMap::<String, ResourceRow>::new());
    let hr_entering = RwSignal::new(std::collections::BTreeSet::<String>::new());
    let hr_removing = RwSignal::new(std::collections::BTreeSet::<String>::new());

    use_sse_subscription(ks_rows, ks_entering, ks_removing, None, move || {
        Some(data::watch_url(&ks_kind.get()?.key, None, None))
    });
    use_sse_subscription(hr_rows, hr_entering, hr_removing, None, move || {
        Some(data::watch_url(&hr_kind.get()?.key, None, None))
    });

    let count = Memo::new(move |_| {
        let ks = ks_rows.with(|m| m.values().filter(|r| r.status == RowStatus::Error).count());
        let hr = hr_rows.with(|m| m.values().filter(|r| r.status == RowStatus::Error).count());
        ks + hr
    });

    view! {
        {move || {
            let n = count.get();
            (n > 0).then(|| {
                let go = move |_| {
                    let kind = if ks_rows.with(|m| m.values().any(|r| r.status == RowStatus::Error)) {
                        ks_kind.get_untracked()
                    } else {
                        hr_kind.get_untracked()
                    };
                    if let Some(k) = kind {
                        only_problems.set(true);
                        selected_kind.set(Some(k));
                    }
                };
                view! {
                    <button class="fluxbadge" on:click=go>
                        "✕ " {n} " Flux"
                        <span class="tooltip">"Flux resources failing — click to view"</span>
                    </button>
                }
            })
        }}
    }
}

/// Cluster CPU/mem usage + node health in the top bar, with a hover tooltip of
/// per-node CPU/mem and ready status.
#[component]
fn TopUsage() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let catalog = expect_context::<Catalog>().0;
    let ov = LocalResource::new(|| async {
        data::fetch_json::<roder_core::ClusterOverview>("/api/overview").await
    });
    view! {
        <Suspense>
            {move || ov.get().and_then(|res| res.ok()).map(|o| {
                let nodes = o.nodes.clone();
                let cpu_p = pct(
                    Some(nodes.iter().filter_map(|n| n.cpu_used).sum()),
                    Some(nodes.iter().filter_map(|n| n.cpu_cores).sum()),
                );
                let mem_p = pct(
                    Some(nodes.iter().filter_map(|n| n.mem_used).sum()),
                    Some(nodes.iter().filter_map(|n| n.mem_bytes).sum()),
                );
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
                    <div class="topusage">
                        <span class="tu-stat">"CPU " <b>{format!("{cpu_p:.0}%")}</b></span>
                        <span class="tu-stat">"Mem " <b>{format!("{mem_p:.0}%")}</b></span>
                        <span class="tu-stat tu-nodes"
                            class:tu-nodes-warn=move || !nodes_ok
                            on:click=go_nodes>
                            <b>{ready}</b>"/"<b>{total}</b>" nodes"
                        </span>
                        <div class="tooltip usage-tip">
                            {nodes.into_iter().map(|n| {
                                let c = pct(n.cpu_used, n.cpu_cores);
                                let m = pct(n.mem_used, n.mem_bytes);
                                view! {
                                    <div class="tip-row">
                                        <span class="tip-node">
                                            <span class=if n.ready { "tip-node-ok" } else { "tip-node-err" }>
                                                {if n.ready { "✓" } else { "✕" }}
                                            </span>
                                            " "{n.name}
                                        </span>
                                        <span>"CPU "{format!("{c:.0}%")}</span>
                                        <span>"Mem "{format!("{m:.0}%")}</span>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                }
            })}
        </Suspense>
    }
}

/// One-click sweep of dead pods + finished jobs, mirroring k9s's sanitize command.
#[component]
fn SanitizeButton() -> impl IntoView {
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();

    let do_sanitize = move || {
        let ns = selected_ns.get_untracked();
        let payload = serde_json::json!({ "action": "sanitize", "namespace": ns });
        leptos::task::spawn_local(async move {
            let _ = data::post_action(&payload).await;
        });
    };

    view! {
        <button class="sweep-btn"
            on:click=move |_| {
                confirm.set(Some(Confirm {
                    message: "Delete all dead pods and finished jobs?".into(),
                    ok_label: Some("Sweep".into()),
                    on_ok: std::sync::Arc::new(do_sanitize),
                }));
            }>
            "Sweep"
            <span class="tooltip">"Delete dead pods and finished jobs"</span>
        </button>
    }
}

/// Live cluster-wide count of failing pods. Clicking jumps to the Pods view
/// filtered to problems. Backed by a single all-namespace pod watch.
#[component]
fn FailingBadge() -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let only_problems = expect_context::<OnlyProblems>().0;

    let pod_kind = Memo::new(move |_| {
        catalog
            .get()
            .into_iter()
            .find(|k| k.group.is_empty() && k.kind == "Pod")
    });
    let rows = RwSignal::new(std::collections::HashMap::<String, ResourceRow>::new());
    let entering = RwSignal::new(std::collections::BTreeSet::<String>::new());
    let removing = RwSignal::new(std::collections::BTreeSet::<String>::new());

    use_sse_subscription(rows, entering, removing, None, move || {
        let pk = pod_kind.get()?;
        Some(data::watch_url(&pk.key, None, None))
    });
    let count = Memo::new(move |_| {
        rows.with(|m| m.values().filter(|r| r.status == RowStatus::Error).count())
    });

    view! {
        {move || {
            let n = count.get();
            (n > 0).then(|| {
                let go = move |_| {
                    if let Some(pk) = pod_kind.get_untracked() {
                        selected_ns.set(None);
                        only_problems.set(true);
                        selected_kind.set(Some(pk));
                    }
                };
                view! {
                    <button class="failbadge" on:click=go
                        title="Pods failing — click to view">
                        "✕ " {n} " failing"
                    </button>
                }
            })
        }}
    }
}

#[component]
fn Brand() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let connected = expect_context::<ConnectionState>().0;
    view! {
        <span class="brand"
              class:brand-disconnected=move || connected.get().is_some()
              on:click=move |_| selected_kind.set(None)>
            <span style="--i:0">"R"</span>
            <span style="--i:1">"o"</span>
            <span style="--i:2">"d"</span>
            <span style="--i:3">"e"</span>
            <span style="--i:4">"r"</span>
            <span class="brand-tip tooltip">
                {move || connected.get().unwrap_or_else(|| "Connected".to_string())}
            </span>
        </span>
    }
}

#[component]
fn Identity() -> impl IntoView {
    let me =
        LocalResource::new(|| async { data::fetch_json::<serde_json::Value>("/api/me").await });
    view! {
        <span class="identity">
            <Suspense>
                {move || me.get().map(|res| match res {
                    Ok(v) => {
                        let who = v.get("email").and_then(|e| e.as_str())
                            .or_else(|| v.get("name").and_then(|n| n.as_str()))
                            .or_else(|| v.get("subject").and_then(|s| s.as_str()))
                            .unwrap_or("anonymous").to_string();
                        view! { <span>{who}</span> <a class="logout" href="/auth/logout" rel="external">"sign out"</a> }.into_any()
                    }
                    Err(_) => ().into_any(),
                })}
            </Suspense>
        </span>
    }
}
