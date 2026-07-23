use leptos::ev;
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Title};
use leptos_router::components::{Route, Router, Routes};
use leptos_router::StaticSegment;
use roder_core::ResourceKind;

use crate::data;

mod components;
mod detail;
mod events;
mod hooks;
mod logs;
mod mobile;
mod overlays;
mod search_state;
mod state;
mod table_logic;
mod util;
mod views;

pub use state::DetailTarget;

use components::sidebar::Sidebar;
use components::tooltip::TooltipLayer;
use components::topbar::Topbar;
use detail::pods::PodModal;
use detail::DetailDrawer;
use detail::Tab;
use logs::LogSidebar;
use mobile::MobileShell;
use overlays::access_review::AccessReview;
use overlays::confirm::{Confirm, ConfirmDialog};
use overlays::context_menu::ContextMenu;
use overlays::delete::{DeleteDialog, DeleteRequest};
use overlays::drain::DrainOverlay;
use overlays::exec::ExecWindow;
use overlays::ns_palette::NsPalette;
use overlays::palette::CommandPalette;
use overlays::shortcuts::ShortcutsHelp;
use overlays::toast::{Toast, ToastView};
use overlays::tree::ResourceTreeWindow;
use overlays::AlertsPanel;
use state::{
    AccessReviewOpen, AlertsData, AlertsLastRefresh, AlertsOpen, Catalog, ConnectionState,
    Connectivity, CtxMenu, DrainOpen, DrainTarget, ExecOpen, ExecTarget, FilterFocus, LogPods,
    LogTarget, NavOpen, NsPaletteOpen, OnlyProblems, PaletteOpen, PodModalTarget, ResourceFilter,
    ShortcutsOpen, TableRows, TableSelected, TableTargets, Tick, TreeOpen, WorkspaceConf,
    WorkspaceConfig,
};
use views::resource::ResourceView;
use views::search::SearchResultsView;
use views::workspace::WorkspaceView;

/// Set once at startup (`main.rs`) from `AppState::asset_version` — the
/// build-time hash embedded into every SSR page so a hydrated tab can detect
/// when the server it's talking to has been redeployed. A module-level
/// static, not a `shell` parameter, because `shell` must keep the exact
/// `fn(LeptosOptions) -> impl IntoView` signature that
/// `leptos_axum::file_and_error_handler` requires.
#[cfg(feature = "ssr")]
static ASSET_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Called once at startup from `main.rs`.
#[cfg(feature = "ssr")]
pub fn set_asset_version(v: String) {
    let _ = ASSET_VERSION.set(v);
}

/// Empty on the wasm/hydrate build, where `shell` is compiled but never called.
fn asset_version() -> String {
    #[cfg(feature = "ssr")]
    {
        ASSET_VERSION.get().cloned().unwrap_or_default()
    }
    #[cfg(not(feature = "ssr"))]
    {
        String::new()
    }
}

#[cfg(target_arch = "wasm32")]
async fn fetch_alerts(force_refresh: bool) -> Result<Vec<roder_core::FiringAlert>, String> {
    let url = if force_refresh {
        "/api/alerts?refresh=true"
    } else {
        "/api/alerts"
    };
    data::fetch_json(url).await
}

#[cfg(target_arch = "wasm32")]
fn update_alerts(
    data: RwSignal<Option<Vec<roder_core::FiringAlert>>>,
    last_refresh: RwSignal<Option<f64>>,
    alerts: Vec<roder_core::FiringAlert>,
) {
    if let Ok(json) = serde_json::to_string(&alerts) {
        crate::data::storage_set("roder.alerts", &json);
    }
    data.set(Some(alerts));
    last_refresh.set(Some(js_sys::Date::now()));
}

/// The HTML document shell rendered on the server.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    let asset_version = asset_version();
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <meta name="color-scheme" content="dark light" />
                <meta name="roder-asset-version" content=asset_version />
                <link rel="icon" type="image/svg+xml" href="/favicon.svg" />
                <AutoReload options=options.clone() />
                <HydrationScripts options=options.clone() />
                <leptos_meta::HashedStylesheet options id="leptos" />
                <MetaTags />
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

#[cfg(target_arch = "wasm32")]
fn check_connectivity(state: RwSignal<Connectivity>, generation: RwSignal<u32>) {
    let Some(window) = web_sys::window() else {
        state.set(Connectivity::Error("Browser window unavailable".into()));
        return;
    };
    if !window.navigator().on_line() {
        generation.update(|n| *n = n.wrapping_add(1));
        state.set(Connectivity::Offline);
        return;
    }

    let request_generation = generation.get_untracked().wrapping_add(1);
    generation.set(request_generation);
    if !matches!(state.get_untracked(), Connectivity::Connected) {
        state.set(Connectivity::Checking);
    }
    leptos::task::spawn_local(async move {
        let result = data::fetch_json::<serde_json::Value>("/api/health").await;
        if generation.get_untracked() != request_generation {
            return;
        }
        match result {
            Ok(_) => state.set(Connectivity::Connected),
            Err(error) => state.set(Connectivity::Error(format!(
                "Cluster connection failed: {error}"
            ))),
        }
    });
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    let selected_kind = RwSignal::new(None::<ResourceKind>);
    let selected_ns = RwSignal::new(None::<String>);
    let detail = RwSignal::new(None::<DetailTarget>);
    let nav_open = RwSignal::new(false);
    // Same on server and first client paint (hydration-safe), corrected
    // client-side below by a live `matchMedia` listener.
    let is_mobile = RwSignal::new(false);
    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            use send_wrapper::SendWrapper;
            use wasm_bindgen::closure::Closure;
            use wasm_bindgen::JsCast;
            let Some(mql) =
                web_sys::window().and_then(|w| w.match_media("(max-width: 760px)").ok().flatten())
            else {
                return;
            };
            is_mobile.set(mql.matches());
            let mql_for_cleanup = mql.clone();
            let cb = Closure::<dyn FnMut(web_sys::MediaQueryListEvent)>::new(
                move |e: web_sys::MediaQueryListEvent| {
                    is_mobile.set(e.matches());
                },
            );
            let cb_fn: js_sys::Function = cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let _ = mql.add_event_listener_with_callback("change", &cb_fn);
            let cleanup = SendWrapper::new((mql_for_cleanup, cb_fn, cb));
            on_cleanup(move || {
                let (mql, cb_fn, cb) = cleanup.take();
                let _ = mql.remove_event_listener_with_callback("change", &cb_fn);
                drop(cb);
            });
        }
    });
    let palette_open = RwSignal::new(false);
    let ns_palette_open = RwSignal::new(false);
    provide_context(NsPaletteOpen(ns_palette_open));
    let catalog = RwSignal::new(Vec::<ResourceKind>::new());
    let ctx_menu = RwSignal::new(None::<CtxMenu>);
    let requested_tab = RwSignal::new(None::<Tab>);
    let tick = RwSignal::new(0u32);
    let only_problems = RwSignal::new(false);
    let confirm = RwSignal::new(None::<Confirm>);
    let delete_confirm = RwSignal::new(None::<DeleteRequest>);
    let toast = RwSignal::new(None::<Toast>);
    let pod_modal = RwSignal::new(None::<DetailTarget>);
    provide_context(PodModalTarget(pod_modal));
    let exec_open = RwSignal::new(None::<ExecTarget>);
    provide_context(ExecOpen(exec_open));
    let tree_open = RwSignal::new(None::<DetailTarget>);
    provide_context(TreeOpen(tree_open));
    let drain_open = RwSignal::new(None::<DrainTarget>);
    provide_context(DrainOpen(drain_open));
    let shortcuts_open = RwSignal::new(false);
    provide_context(ShortcutsOpen(shortcuts_open));
    let alerts_open = RwSignal::new(false);
    provide_context(AlertsOpen(alerts_open));
    let access_review_open = RwSignal::new(false);
    provide_context(AccessReviewOpen(access_review_open));
    let alerts_data: RwSignal<Option<Vec<roder_core::FiringAlert>>> = RwSignal::new(None);
    provide_context(AlertsData(alerts_data));
    let alerts_last_refresh = RwSignal::new(None::<f64>);
    provide_context(AlertsLastRefresh(alerts_last_refresh));
    let _alertmanager_enabled = RwSignal::new(false);
    let resource_filter = RwSignal::new(String::new());
    provide_context(ResourceFilter(resource_filter));
    provide_context(FilterFocus(RwSignal::new(0u32)));
    let log_pods = RwSignal::new(Vec::<LogTarget>::new());
    provide_context(LogPods(log_pods));
    provide_context(Tick(tick));
    provide_context(OnlyProblems(only_problems));
    provide_context(confirm);
    provide_context(delete_confirm);
    provide_context(toast);
    provide_context(selected_kind);
    provide_context(selected_ns);
    provide_context(detail);
    provide_context(NavOpen(nav_open));
    provide_context(PaletteOpen(palette_open));
    provide_context(Catalog(catalog));
    provide_context(ctx_menu);
    provide_context(requested_tab);
    let connection = RwSignal::new(Connectivity::Checking);
    provide_context(ConnectionState(connection));
    provide_context(TableSelected(StoredValue::new(None)));
    provide_context(TableRows(StoredValue::new(None)));
    provide_context(TableTargets(StoredValue::new(None)));

    // Keep an end-to-end status alive independently of whichever view/SSE streams
    // happen to be open. Browser network events update immediately; the periodic
    // probe catches a reachable Roder server whose Kubernetes connection failed.
    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            let generation = RwSignal::new(0u32);
            check_connectivity(connection, generation);
            set_interval(
                move || check_connectivity(connection, generation),
                std::time::Duration::from_secs(15),
            );
            let online = window_event_listener(ev::online, move |_| {
                connection.set(Connectivity::Checking);
                check_connectivity(connection, generation);
            });
            let offline = window_event_listener(ev::offline, move |_| {
                generation.update(|n| *n = n.wrapping_add(1));
                connection.set(Connectivity::Offline);
            });
            let focus = window_event_listener(ev::focus, move |_| {
                check_connectivity(connection, generation)
            });
            on_cleanup(move || {
                online.remove();
                offline.remove();
                focus.remove();
            });
        }
    });

    // Namespace list shared across the topbar selector and all workspace pane dropdowns.
    // Fetched once here so individual panes don't each open a separate HTTP connection.
    let ns_resource: LocalResource<Result<Vec<String>, String>> =
        LocalResource::new(|| async { data::fetch_json::<Vec<String>>("/api/namespaces").await });
    provide_context(ns_resource);

    // Workspace config: start empty (same on server and client, avoids hydration mismatch),
    // restore from localStorage client-side after mount, then persist on every change.
    let workspace = RwSignal::new(WorkspaceConfig::default());
    provide_context(WorkspaceConf(workspace));
    // workspace_ready prevents the persist effect from clobbering localStorage with the
    // empty default before the restore effect has run.
    let workspace_ready = RwSignal::new(false);
    Effect::new(move |_| {
        if let Some(saved) = data::storage_get("roder.workspace")
            .and_then(|s| serde_json::from_str::<WorkspaceConfig>(&s).ok())
        {
            workspace.set(saved);
        }
        workspace_ready.set(true);
    });
    Effect::new(move |_| {
        if !workspace_ready.get() {
            return;
        }
        let w = workspace.get();
        if let Ok(json) = serde_json::to_string(&w) {
            data::storage_set("roder.workspace", &json);
        }
    });

    // Namespace has its own persist/restore lifecycle independent of the catalog.
    // Using a ready guard (same pattern as workspace) ensures we never clobber
    // the stored value on startup before the restore effect has run.
    let ns_ready = RwSignal::new(false);
    Effect::new(move |_| {
        // Prefer roder.ns; fall back to roder.nav for backwards compat.
        let ns = data::storage_get("roder.ns")
            .and_then(|s| serde_json::from_str::<Option<String>>(&s).ok())
            .flatten()
            .or_else(|| {
                data::storage_get("roder.nav")
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|v| v.get("ns").and_then(|x| x.as_str()).map(String::from))
            });
        if let Some(n) = ns {
            selected_ns.set(Some(n));
        }
        ns_ready.set(true);
    });
    Effect::new(move |_| {
        if !ns_ready.get() {
            return;
        }
        let ns = selected_ns.get();
        if let Ok(s) = serde_json::to_string(&ns) {
            data::storage_set("roder.ns", &s);
        }
    });

    // Restore kind/detail from a previous session.
    let saved = data::storage_get("roder.nav")
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let saved_kind = saved
        .as_ref()
        .and_then(|v| v.get("kind").and_then(|x| x.as_str()).map(String::from));
    let saved_detail = saved.as_ref().and_then(|v| v.get("detail").cloned());
    let restored = RwSignal::new(false);

    // Load the resource catalog once; restore the selected kind/detail once it loads.
    let cat_res = LocalResource::new(|| async {
        data::fetch_json::<Vec<ResourceKind>>("/api/resources").await
    });
    Effect::new(move |_| {
        if let Some(Ok(list)) = cat_res.get() {
            if !restored.get_untracked() {
                if let Some(k) = saved_kind.as_ref() {
                    if let Some(kind) = list.iter().find(|x| &x.key == k).cloned() {
                        selected_kind.set(Some(kind));
                    }
                }
                if let Some(dv) = saved_detail.as_ref() {
                    if let (Some(key), Some(name)) = (
                        dv.get("key").and_then(|x| x.as_str()),
                        dv.get("name").and_then(|x| x.as_str()),
                    ) {
                        detail.set(Some(DetailTarget {
                            key: key.to_string(),
                            namespace: dv.get("ns").and_then(|x| x.as_str()).map(String::from),
                            name: name.to_string(),
                        }));
                    }
                }
                restored.set(true);
            }
            catalog.set(list);
        }
    });

    // Periodically re-pull the catalog so newly-installed operators (new CRDs)
    // appear in the sidebar — and removed ones disappear — without a reload. The
    // server keeps it current via a CRD watch; this just refreshes the client's
    // copy. (Open tables get their columns live from the watch snapshots, so
    // this is only for the sidebar's kind list.)
    Effect::new(move |_| {
        set_interval(
            move || {
                #[cfg(target_arch = "wasm32")]
                leptos::task::spawn_local(async move {
                    if let Ok(list) = data::fetch_json::<Vec<ResourceKind>>("/api/resources").await
                    {
                        catalog.set(list);
                    }
                });
            },
            std::time::Duration::from_secs(30),
        );
    });

    // Discover optional integrations before touching their endpoints. Disabled
    // integrations stay invisible and produce no background request noise.
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let enabled = data::fetch_json::<serde_json::Value>("/api/features")
            .await
            .ok()
            .and_then(|v| v.get("alertmanager").and_then(|v| v.as_bool()))
            .unwrap_or(false);
        _alertmanager_enabled.set(enabled);
        if !enabled {
            alerts_data.set(None);
            data::storage_remove("roder.alerts");
            return;
        }

        // Seed from the last-known list while the first request is in flight.
        if let Some(cached) = data::storage_get("roder.alerts")
            .and_then(|s| serde_json::from_str::<Vec<roder_core::FiringAlert>>(&s).ok())
        {
            alerts_data.set(Some(cached));
        }
        if let Ok(list) = fetch_alerts(false).await {
            update_alerts(alerts_data, alerts_last_refresh, list);
        }
    });

    // Poll for firing alerts every 30 s so the panel stays current.
    Effect::new(move |_| {
        set_interval(
            move || {
                #[cfg(target_arch = "wasm32")]
                leptos::task::spawn_local(async move {
                    if !_alertmanager_enabled.get_untracked() {
                        return;
                    }
                    if let Ok(list) = fetch_alerts(false).await {
                        update_alerts(alerts_data, alerts_last_refresh, list);
                    }
                });
            },
            std::time::Duration::from_secs(30),
        );
    });

    // Persist kind/detail on change (only after restore, so we don't clobber them).
    Effect::new(move |_| {
        let kind = selected_kind.get();
        let det = detail.get();
        if !restored.get() {
            return;
        }
        let blob = serde_json::json!({
            "kind": kind.map(|k| k.key),
            "detail": det.map(|t| serde_json::json!({ "key": t.key, "ns": t.namespace, "name": t.name })),
        });
        data::storage_set("roder.nav", &blob.to_string());
    });

    // Global keyboard shortcuts.
    let filter_focus = expect_context::<FilterFocus>().0;
    Effect::new(move |_| {
        let handle = window_event_listener(ev::keydown, move |e| {
            let key = e.key();
            if key == "K" && !data::is_text_input_focused() {
                e.prevent_default();
                palette_open.update(|o| *o = !*o);
            } else if key == "N" && !data::is_text_input_focused() {
                e.prevent_default();
                ns_palette_open.update(|o| *o = !*o);
            } else if key == "E" && !data::is_text_input_focused() {
                e.prevent_default();
                only_problems.update(|o| *o = !*o);
            } else if key == "/" && !data::is_text_input_focused() {
                e.prevent_default();
                filter_focus.update(|n| *n += 1);
            } else if key == "?" && !data::is_text_input_focused() {
                shortcuts_open.update(|o| *o = !*o);
            } else if key == "Escape" {
                palette_open.set(false);
                ns_palette_open.set(false);
                shortcuts_open.set(false);
                alerts_open.set(false);
                access_review_open.set(false);
                ctx_menu.set(None);
                detail.set(None);
            }
        });
        on_cleanup(move || handle.remove());
    });

    // Tick once a second so the Age column updates live.
    Effect::new(move |_| {
        set_interval(
            move || tick.update(|t| *t = t.wrapping_add(1)),
            std::time::Duration::from_secs(1),
        );
    });

    // Session heartbeat: periodically hit a cheap endpoint so the server can
    // refresh + re-seal the session cookie (and keep the cluster token fresh)
    // even on long-lived, SSE-only views that otherwise make no requests. The
    // response is ignored — the point is the round-trip through `require_auth`.
    Effect::new(move |_| {
        set_interval(
            move || {
                #[cfg(target_arch = "wasm32")]
                leptos::task::spawn_local(async {
                    let _ = data::fetch_json::<serde_json::Value>("/api/me").await;
                });
            },
            std::time::Duration::from_secs(45),
        );
    });

    // Namespaces can appear/disappear as the cluster changes, but the list is
    // fetched once (see `ns_resource` above). Re-poll periodically so the
    // namespace switcher stays current without a full browser refresh.
    Effect::new(move |_| {
        set_interval(
            move || {
                #[cfg(target_arch = "wasm32")]
                ns_resource.refetch();
            },
            std::time::Duration::from_secs(30),
        );
    });

    view! {
        <Title text="Roder" />
        <Router>
            {move || if is_mobile.get() {
                view! { <MobileShell /> }.into_any()
            } else {
                view! {
                    <div class="app" class:nav-open=move || nav_open.get()>
                        <Topbar />
                        <div class="body">
                            <Sidebar />
                            <main class="main">
                                <Routes fallback=|| view! { <p class="empty">"Not found."</p> }>
                                    <Route path=StaticSegment("") view=ResourceView />
                                    <Route path=StaticSegment("search") view=SearchResultsView />
                                    <Route path=StaticSegment("workspace") view=WorkspaceView />
                                </Routes>
                            </main>
                            <DetailDrawer />
                            <LogSidebar />
                        </div>
                        <CommandPalette />
                        <NsPalette />
                        <ContextMenu />
                        <ConfirmDialog />
                        <DeleteDialog />
                        <DrainOverlay />
                        <PodModal />
                        <ExecWindow />
                        <ResourceTreeWindow />
                        <ShortcutsHelp />
                        <AlertsPanel />
                        <AccessReview />
                        <ToastView />
                    </div>
                    <TooltipLayer />
                }.into_any()
            }}
        </Router>
    }
}
