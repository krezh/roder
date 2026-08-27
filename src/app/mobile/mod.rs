//! Structurally separate mobile UI, mounted instead of the desktop tree below
//! the 760px breakpoint (see `MobileMode` in `state.rs`). Shares every signal/
//! context provided by `App()` and every pure data/plumbing module (`hooks`,
//! `events`, `state`, `util`) with the desktop tree — only the rendering layer
//! differs. Components not yet given a mobile-native treatment are reused
//! as-is from the desktop tree (progressively replaced phase by phase).

mod action_sheet;
mod bottom_nav;
mod bulk_bar;
mod detail_view;
mod disclosure;
mod header;
mod logs_view;
mod resource_list;
mod row_card;
mod search_list;
mod status;
mod workspace_view;

use leptos::prelude::*;
use leptos_router::components::{Route, Routes};
use leptos_router::StaticSegment;

use crate::app::components::sidebar::Sidebar;
use crate::app::detail::pods::PodModal;
use crate::app::overlays::confirm::ConfirmDialog;
use crate::app::overlays::delete::DeleteDialog;
use crate::app::overlays::drain::DrainOverlay;
use crate::app::overlays::exec::ExecWindow;
use crate::app::overlays::ns_palette::NsPalette;
use crate::app::overlays::palette::CommandPalette;
use crate::app::overlays::shortcuts::ShortcutsHelp;
use crate::app::overlays::toast::ToastView;
use crate::app::overlays::AlertsPanel;
use crate::app::state::NavOpen;

use action_sheet::MobileActionSheet;
use bottom_nav::MobileBottomNav;
use detail_view::MobileDetailView;
use header::MobileHeader;
use logs_view::MobileLogsView;
use resource_list::MobileResourceView;
use search_list::MobileSearchList;
use workspace_view::MobileWorkspaceView;

#[component]
pub(crate) fn MobileShell() -> impl IntoView {
    let nav_open = expect_context::<NavOpen>().0;
    let selected_kind = expect_context::<RwSignal<Option<roder_core::ResourceKind>>>();
    let nav_filter = RwSignal::new(String::new());
    Effect::new(move |_| {
        if !nav_open.get() {
            nav_filter.set(String::new());
        }
    });

    view! {
        <div class="mobile-shell" class:nav-open=move || nav_open.get()>
            <MobileHeader />
            <div class="mobile-sidebar-scrim" class:open=move || nav_open.get()
                on:click=move |_| nav_open.set(false)></div>
            <div class="mobile-nav-panel">
                <div class="mobile-nav-head">
                    <div>
                        <span class="mobile-nav-eyebrow">"Roder"</span>
                        <strong>"Resources"</strong>
                    </div>
                    <button class="mobile-round-btn" aria-label="Close navigation"
                        on:click=move |_| nav_open.set(false)>
                        <span aria-hidden="true">"×"</span>
                    </button>
                </div>
                <label class="mobile-nav-search">
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                        <circle cx="11" cy="11" r="6.5" />
                        <path d="m16 16 4 4" />
                    </svg>
                    <input type="search" placeholder="Find a resource"
                        aria-label="Find a resource"
                        prop:value=move || nav_filter.get()
                        on:input=move |event| nav_filter.set(event_target_value(&event)) />
                    {move || (!nav_filter.get().is_empty()).then(|| view! {
                        <button type="button" aria-label="Clear resource search"
                            on:click=move |_| nav_filter.set(String::new())>"×"</button>
                    })}
                </label>
                <button class="mobile-home-link" class:active=move || selected_kind.get().is_none()
                    on:click=move |_| {
                        selected_kind.set(None);
                        nav_open.set(false);
                    }>
                    <span class="mobile-home-icon" aria-hidden="true"></span>
                    <span>"Cluster overview"</span>
                </button>
                <Sidebar filter=Signal::derive(move || nav_filter.get()) />
            </div>
            <main class="mobile-main">
                <Routes fallback=|| view! { <p class="empty">"Not found."</p> }>
                    <Route path=StaticSegment("") view=MobileResourceView />
                    <Route path=StaticSegment("search") view=MobileSearchList />
                    <Route path=StaticSegment("workspace") view=MobileWorkspaceView />
                </Routes>
            </main>
            <MobileBottomNav />
            <MobileDetailView />
            <MobileLogsView />
            <CommandPalette />
            <NsPalette />
            <MobileActionSheet />
            <ConfirmDialog />
            <DeleteDialog />
            <DrainOverlay />
            <PodModal />
            <ExecWindow />
            <ShortcutsHelp />
            <AlertsPanel />
            <ToastView />
        </div>
    }
}
