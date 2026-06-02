//! Top bar: hamburger, brand, palette button, error filter, namespace selector,
//! failing-pod badge, cluster usage, and identity.

use leptos::prelude::*;
use roder_core::{ResourceKind, ResourceRow, RowStatus};

use crate::app::hooks::use_sse_subscription;
use crate::app::state::{Catalog, NavOpen, OnlyProblems, PaletteOpen};
use crate::app::util::format::pct;
use crate::data;

#[component]
pub(crate) fn Topbar() -> impl IntoView {
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let palette_open = expect_context::<PaletteOpen>().0;
    let only_problems = expect_context::<OnlyProblems>().0;
    let namespaces =
        LocalResource::new(|| async { data::fetch_json::<Vec<String>>("/api/namespaces").await });

    view! {
        <header class="topbar">
            <button class="hamburger" on:click=move |_| nav_open.update(|o| *o = !*o)>"☰"</button>
            <span class="brand" on:click=move |_| selected_kind.set(None)>"Roder"</span>
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

/// Cluster CPU/mem usage in the top bar, with a hover tooltip of per-node usage.
#[component]
fn TopUsage() -> impl IntoView {
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
                view! {
                    <div class="topusage">
                        <span class="tu-stat">"CPU " <b>{format!("{cpu_p:.0}%")}</b></span>
                        <span class="tu-stat">"Mem " <b>{format!("{mem_p:.0}%")}</b></span>
                        <div class="tooltip usage-tip">
                            {nodes.into_iter().map(|n| {
                                let c = pct(n.cpu_used, n.cpu_cores);
                                let m = pct(n.mem_used, n.mem_bytes);
                                view! {
                                    <div class="tip-row">
                                        <span class="tip-node">{n.name}</span>
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
        catalog.get().into_iter().find(|k| k.group.is_empty() && k.kind == "Pod")
    });
    let rows = RwSignal::new(std::collections::HashMap::<String, ResourceRow>::new());
    let entering = RwSignal::new(std::collections::BTreeSet::<String>::new());
    let removing = RwSignal::new(std::collections::BTreeSet::<String>::new());
    use_sse_subscription(rows, entering, removing, move || {
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
fn Identity() -> impl IntoView {
    let me = LocalResource::new(|| async {
        data::fetch_json::<serde_json::Value>("/api/me").await
    });
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
