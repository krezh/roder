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
mod overlays;
mod state;
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
use overlays::confirm::{Confirm, ConfirmDialog};
use overlays::context_menu::ContextMenu;
use overlays::exec::ExecWindow;
use overlays::ns_palette::NsPalette;
use overlays::palette::CommandPalette;
use overlays::shortcuts::ShortcutsHelp;
use state::{
    Catalog, ConnectionState, CtxMenu, ExecOpen, ExecTarget, FilterFocus, LogPods, LogTarget,
    NavOpen, NsPaletteOpen, OnlyProblems, PaletteOpen, PodModalTarget, ResourceFilter,
    ShortcutsOpen, TableRows, TableSelected, Tick, WorkspaceConf, WorkspaceConfig,
};
use views::resource::ResourceView;
use views::search::SearchResultsView;
use views::workspace::WorkspaceView;

/// The HTML document shell rendered on the server.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <meta name="color-scheme" content="dark light" />
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

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    let selected_kind = RwSignal::new(None::<ResourceKind>);
    let selected_ns = RwSignal::new(None::<String>);
    let detail = RwSignal::new(None::<DetailTarget>);
    let nav_open = RwSignal::new(false);
    let palette_open = RwSignal::new(false);
    let ns_palette_open = RwSignal::new(false);
    provide_context(NsPaletteOpen(ns_palette_open));
    let catalog = RwSignal::new(Vec::<ResourceKind>::new());
    let ctx_menu = RwSignal::new(None::<CtxMenu>);
    let requested_tab = RwSignal::new(None::<Tab>);
    let tick = RwSignal::new(0u32);
    let only_problems = RwSignal::new(false);
    let confirm = RwSignal::new(None::<Confirm>);
    let pod_modal = RwSignal::new(None::<DetailTarget>);
    provide_context(PodModalTarget(pod_modal));
    let exec_open = RwSignal::new(None::<ExecTarget>);
    provide_context(ExecOpen(exec_open));
    let shortcuts_open = RwSignal::new(false);
    provide_context(ShortcutsOpen(shortcuts_open));
    let resource_filter = RwSignal::new(String::new());
    provide_context(ResourceFilter(resource_filter));
    provide_context(FilterFocus(RwSignal::new(0u32)));
    let log_pods = RwSignal::new(Vec::<LogTarget>::new());
    provide_context(LogPods(log_pods));
    provide_context(Tick(tick));
    provide_context(OnlyProblems(only_problems));
    provide_context(confirm);
    provide_context(selected_kind);
    provide_context(selected_ns);
    provide_context(detail);
    provide_context(NavOpen(nav_open));
    provide_context(PaletteOpen(palette_open));
    provide_context(Catalog(catalog));
    provide_context(ctx_menu);
    provide_context(requested_tab);
    provide_context(ConnectionState(RwSignal::new(None::<String>)));
    provide_context(TableSelected(StoredValue::new(None)));
    provide_context(TableRows(StoredValue::new(None)));

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

    view! {
        <Title text="Roder" />
        <Router>
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
                <PodModal />
                <ExecWindow />
                <ShortcutsHelp />
            </div>
            <TooltipLayer />
        </Router>
    }
}
