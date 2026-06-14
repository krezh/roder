//! Top bar: hamburger, brand, connection dot, palette button, error filter,
//! namespace selector, failing-pod badge, cluster usage (with node health),
//! and identity.

use leptos::prelude::*;
use roder_core::{ResourceKind, ResourceRow, RowStatus};

use crate::app::hooks::use_sse_subscription;
use crate::app::state::{Catalog, ConnectionState, NavOpen, OnlyProblems, PaletteOpen};
use crate::app::util::format::pct;
use crate::data;

#[component]
pub(crate) fn Topbar() -> impl IntoView {
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let palette_open = expect_context::<PaletteOpen>().0;
    let only_problems = expect_context::<OnlyProblems>().0;
    let namespaces = expect_context::<LocalResource<Result<Vec<String>, String>>>();

    view! {
        <header class="topbar">
            <button class="hamburger" on:click=move |_| nav_open.update(|o| *o = !*o)>"☰"</button>
            <span class="brand" on:click=move |_| selected_kind.set(None)>"Roder"</span>
            <ConnectionDot />
            <button class="palette-btn" on:click=move |_| palette_open.set(true)>
                "Search " <kbd>"⌘K"</kbd>
            </button>
            <button class="errfilter" class:active=move || only_problems.get()
                title="Show only problems (Ctrl+Z)"
                on:click=move |_| only_problems.update(|o| *o = !*o)>
                "⚠ Errors"
            </button>
            <div class="ns-select">
                <select on:change=move |ev| {
                    let v = event_target_value(&ev);
                    selected_ns.set(if v.is_empty() { None } else { Some(v) });
                }>
                    <option value="" prop:selected=move || selected_ns.get().is_none()>"All namespaces"</option>
                    <Suspense>
                        {move || namespaces.get().map(|res| match res {
                            Ok(list) => list.into_iter().map(|ns| {
                                let label = ns.clone();
                                let ns_sel = ns.clone();
                                view! {
                                    <option value=ns prop:selected=move || selected_ns.get().as_deref() == Some(ns_sel.as_str())>
                                        {label}
                                    </option>
                                }
                            }).collect_view().into_any(),
                            Err(_) => ().into_any(),
                        })}
                    </Suspense>
                </select>
            </div>
            <FailingBadge />
            <TopUsage />
            <Identity />
        </header>
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
                                let ready_label = if n.ready { "✓" } else { "✕" };
                                view! {
                                    <div class="tip-row">
                                        <span class="tip-node"
                                            class:tip-node-warn=move || !n.ready>
                                            {ready_label}" "{n.name}
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

/// A small dot showing whether the SSE data stream is live or reconnecting.
#[component]
fn ConnectionDot() -> impl IntoView {
    let connected = expect_context::<ConnectionState>().0;
    view! {
        <span class="conn-dot-wrap">
            <span class="conn-dot" class:conn-dot-ok=move || connected.get()></span>
            <span class="tooltip conn-dot-tip">
                {move || if connected.get() { "Connected" } else { "Reconnecting…" }}
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
                        view! { <span>{who}</span> <a class="logout" href="/auth/logout">"sign out"</a> }.into_any()
                    }
                    Err(_) => ().into_any(),
                })}
            </Suspense>
        </span>
    }
}
