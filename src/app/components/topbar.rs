//! Top bar: hamburger, brand, connection dot, palette button, error filter,
//! namespace selector, failing-pod badge, cluster usage (with node health),
//! and identity.

use leptos::prelude::*;
use roder_core::{ResourceKind, ResourceRow, RowStatus};

use crate::app::components::icons::ShiftIcon;
use crate::app::hooks::use_sse_subscription;
use crate::app::overlays::confirm::Confirm;
use crate::app::overlays::toast::{show_toast, show_toast_detail, Toast, ToastKind};
use crate::app::state::{
    AccessReviewOpen, AlertsData, AlertsOpen, Catalog, ConnectionState, NavOpen, NsPaletteOpen,
    OnlyProblems, PaletteOpen,
};
use crate::app::util::format::{cluster_usage_pct, pct};
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
            <TopUsage />
            <div class="topbar-group topbar-nav">
                <button class="palette-btn" on:click=move |_| palette_open.set(true)>
                    "Search " <kbd><ShiftIcon />"K"</kbd>
                </button>
                <button class="errfilter" class:active=move || only_problems.get()
                    on:click=move |_| only_problems.update(|o| *o = !*o)>
                    "Errors " <kbd><ShiftIcon />"E"</kbd>
                </button>
                <button class="ns-palette-btn" class:scoped=move || selected_ns.get().is_some()
                    on:click=move |_| ns_palette_open.set(true)>
                    {move || selected_ns.get().unwrap_or_else(|| "All namespaces".to_string())}
                    " " <kbd><ShiftIcon />"N"</kbd>
                </button>
            </div>
            <div class="topbar-group topbar-actions">
                <SanitizeButton />
                <SyncButton />
            </div>
            <div class="topbar-group topbar-health">
                <AlertsButton />
                <AccessReviewButton />
                // Conditional badges stay last so appearing failures never move
                // the permanent health controls out from under the pointer.
                <FailingBadge />
                <FluxFailingBadge />
            </div>
            <div class="topbar-account"><Identity /></div>
        </header>
    }
}

/// Alerts button — hidden entirely when AlertManager is not configured (data is None).
/// Shows total count and a hover tooltip with critical/warning/info breakdown.
#[component]
pub(crate) fn AlertsButton() -> impl IntoView {
    let data = expect_context::<AlertsData>().0;
    let open = expect_context::<AlertsOpen>().0;

    let counts = Memo::new(move |_| {
        data.get().map(|alerts| {
            let active: Vec<_> = alerts.iter().filter(|a| !a.silenced).collect();
            let critical = active.iter().filter(|a| a.severity == "critical").count();
            let warning = active.iter().filter(|a| a.severity == "warning").count();
            let info = active
                .iter()
                .filter(|a| a.severity != "critical" && a.severity != "warning")
                .count();
            (active.len(), critical, warning, info)
        })
    });

    let bouncing = RwSignal::new(false);
    Effect::new(move |prev: Option<usize>| {
        let total = counts.get().map(|(t, _, _, _)| t).unwrap_or(0);
        if prev.is_some_and(|p| p != total) {
            bouncing.set(true);
            set_timeout(
                move || bouncing.set(false),
                std::time::Duration::from_millis(350),
            );
        }
        total
    });

    view! {
        {move || {
            counts.get().map(|(total, critical, warning, info)| {
                view! {
                    <button
                        class="alerts-btn"
                        class:alerts-firing={move || total > 0}
                        class:bouncing=move || bouncing.get()
                        on:click=move |_| open.set(true)
                    >
                        <span class="alerts-count">{total}</span>
                        <span class="tooltip alerts-tip">
                            <span class="tip-row"><span class="sev-dot sev-critical"></span>"Critical: " {critical}</span>
                            <span class="tip-row"><span class="sev-dot sev-warning"></span>"Warning: " {warning}</span>
                            <span class="tip-row"><span class="sev-dot sev-info"></span>"Info: " {info}</span>
                        </span>
                    </button>
                }
            })
        }}
    }
}

/// Live count of failing Flux Kustomizations and HelmReleases.
#[component]
pub(crate) fn FluxFailingBadge() -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
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

    // Seed the badge's count from the last-known value so it doesn't vanish and
    // then pop back up across a refresh while the SSE snapshots are in flight.
    let cached_count = RwSignal::new(0usize);
    Effect::new(move |_| {
        if let Some(n) = data::storage_get("roder.flux_failing_count").and_then(|s| s.parse().ok())
        {
            cached_count.set(n);
        }
    });
    // Unlike pods, a cluster can legitimately have zero Kustomizations/HelmReleases,
    // so `loaded` may never flip true — but `cached_count` is then also correctly 0.
    let loaded =
        Memo::new(move |_| ks_rows.with(|m| !m.is_empty()) || hr_rows.with(|m| !m.is_empty()));
    let count = Memo::new(move |_| {
        if loaded.get() {
            let ks = ks_rows.with(|m| m.values().filter(|r| r.status == RowStatus::Error).count());
            let hr = hr_rows.with(|m| m.values().filter(|r| r.status == RowStatus::Error).count());
            ks + hr
        } else {
            cached_count.get()
        }
    });
    Effect::new(move |_| {
        if loaded.get() {
            data::storage_set("roder.flux_failing_count", &count.get().to_string());
        }
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
                        selected_ns.set(None);
                        only_problems.set(true);
                        selected_kind.set(Some(k));
                    }
                };
                view! {
                    <button class="fluxbadge" on:click=go>
                        {n} " Flux"
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
                            <span class="tu-warn" aria-label="Usage data is stale">"!"
                                <span class="tooltip">"Failed to refresh — showing last known values"</span>
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

/// One-click sweep of dead pods + finished jobs, mirroring k9s's sanitize command.
#[component]
fn SanitizeButton() -> impl IntoView {
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let toast = expect_context::<RwSignal<Option<Toast>>>();

    let do_sanitize = move || {
        let ns = selected_ns.get_untracked();
        let payload = serde_json::json!({ "action": "sanitize", "namespace": ns });
        leptos::task::spawn_local(async move {
            match data::post_action(&payload).await {
                Ok(body) => {
                    let summary: roder_core::CleanupSummary =
                        serde_json::from_str(&body).unwrap_or_default();
                    let total = summary.pods_deleted + summary.jobs_deleted;
                    if total == 0 {
                        show_toast(toast, "Nothing to sweep", ToastKind::Ok);
                    } else {
                        show_toast(
                            toast,
                            format!(
                                "Swept {} pod(s), {} job(s)",
                                summary.pods_deleted, summary.jobs_deleted
                            ),
                            ToastKind::Ok,
                        );
                    }
                }
                Err(e) => show_toast_detail(toast, "Sweep failed", Some(e), ToastKind::Err),
            }
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

/// One-click reconciliation sweep across every Flux resource (optionally
/// scoped to the selected namespace), mirroring `flux reconcile --all`.
#[component]
fn SyncButton() -> impl IntoView {
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let toast = expect_context::<RwSignal<Option<Toast>>>();

    let do_sync = move |_| {
        let ns = selected_ns.get_untracked();
        let payload = serde_json::json!({ "action": "flux-reconcile-all", "namespace": ns });
        leptos::task::spawn_local(async move {
            match data::post_action(&payload).await {
                Ok(body) => {
                    let n: usize = body.trim().parse().unwrap_or(0);
                    if n == 0 {
                        show_toast(toast, "No Flux resources reconciled", ToastKind::Err);
                    } else {
                        show_toast(
                            toast,
                            format!("Reconcile requested for {n} resource(s)"),
                            ToastKind::Ok,
                        );
                    }
                }
                Err(e) => show_toast_detail(toast, "Sync failed", Some(e), ToastKind::Err),
            }
        });
    };

    view! {
        <button class="sync-btn" on:click=do_sync>
            "Sync"
            <span class="tooltip">"Reconcile all Flux resources"</span>
        </button>
    }
}

/// Live cluster-wide count of failing pods. Clicking jumps to the Pods view
/// filtered to problems. Backed by a single all-namespace pod watch.
#[component]
pub(crate) fn FailingBadge() -> impl IntoView {
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

    // Seed the badge's count from the last-known value so it doesn't vanish and
    // then pop back up across a refresh while the SSE snapshot is in flight.
    let cached_count = RwSignal::new(0usize);
    Effect::new(move |_| {
        if let Some(n) = data::storage_get("roder.failing_count").and_then(|s| s.parse().ok()) {
            cached_count.set(n);
        }
    });
    // `rows` holds every pod cluster-wide, which is never truly empty in a real
    // cluster — a non-empty map is a reliable proxy for "the first SSE snapshot
    // has landed", at which point the live count takes over from the cache.
    let loaded = Memo::new(move |_| rows.with(|m| !m.is_empty()));
    let count = Memo::new(move |_| {
        if loaded.get() {
            rows.with(|m| m.values().filter(|r| r.status == RowStatus::Error).count())
        } else {
            cached_count.get()
        }
    });
    Effect::new(move |_| {
        if loaded.get() {
            data::storage_set("roder.failing_count", &count.get().to_string());
        }
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
                        {n} " failing"
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

/// Opens the RBAC access review overlay ("what can I do?", given OIDC passthrough).
#[component]
fn AccessReviewButton() -> impl IntoView {
    let open = expect_context::<AccessReviewOpen>().0;
    view! {
        <button class="access-btn" on:click=move |_| open.set(true)>
            "Access"
            <span class="tooltip">"RBAC access review — what can I do?"</span>
        </button>
    }
}

#[component]
fn Identity() -> impl IntoView {
    let identity = RwSignal::new(None::<serde_json::Value>);
    // Seed from the last-known identity so the name/sign-out link doesn't flash
    // empty on refresh while the first `/api/me` round-trip is in flight.
    Effect::new(move |_| {
        if let Some(cached) = data::storage_get("roder.identity")
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            identity.set(Some(cached));
        }
    });
    let me =
        LocalResource::new(|| async { data::fetch_json::<serde_json::Value>("/api/me").await });
    Effect::new(move |_| {
        if let Some(Ok(v)) = me.get() {
            if let Ok(json) = serde_json::to_string(&v) {
                data::storage_set("roder.identity", &json);
            }
            identity.set(Some(v));
        }
    });
    view! {
        <span class="identity">
            {move || identity.get().map(|v| {
                let who = v.get("email").and_then(|e| e.as_str())
                    .or_else(|| v.get("name").and_then(|n| n.as_str()))
                    .or_else(|| v.get("subject").and_then(|s| s.as_str()))
                    .unwrap_or("anonymous").to_string();
                view! { <span>{who}</span> <a class="logout" href="/auth/logout" rel="external">"sign out"</a> }
            })}
        </span>
    }
}
