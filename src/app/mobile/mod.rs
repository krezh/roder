//! Structurally separate mobile UI, mounted instead of the desktop tree below
//! the 760px breakpoint (see `MobileMode` in `state.rs`). Shares every signal/
//! context provided by `App()` and every pure data/plumbing module (`hooks`,
//! `events`, `state`, `util`) with the desktop tree — only the rendering layer
//! differs. Components not yet given a mobile-native treatment are reused
//! as-is from the desktop tree (progressively replaced phase by phase).

mod action_sheet;
mod alerts;
mod bottom_nav;
mod bulk_bar;
mod dashboard;
mod detail_view;
mod dialogs;
mod drain;
mod exec;
mod header;
mod list_actions;
mod logs_view;
mod palettes;
mod pods;
mod resource_list;
mod row_card;
mod search_list;
mod shortcuts;
mod sidebar;
mod status;
mod toast;
mod tree;
mod workspace_view;

use leptos::prelude::*;
use leptos_router::components::{Route, Routes};
use leptos_router::StaticSegment;

use crate::app::state::NavOpen;

use action_sheet::MobileActionSheet;
use alerts::MobileAlertsPanel;
use bottom_nav::MobileBottomNav;
use detail_view::MobileDetailView;
use dialogs::{MobileConfirmDialog, MobileDeleteDialog};
use drain::MobileDrainOverlay;
use exec::MobileExecWindow;
use header::MobileHeader;
use logs_view::MobileLogsView;
use palettes::{MobileCommandPalette, MobileNsPalette};
use pods::MobilePodModal;
use resource_list::MobileResourceView;
use search_list::MobileSearchList;
use shortcuts::MobileShortcutsHelp;
use sidebar::MobileSidebar;
use toast::MobileToastView;
use tree::MobileRelationshipTree;
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
                <MobileSidebar filter=Signal::derive(move || nav_filter.get()) />
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
            <MobileCommandPalette />
            <MobileNsPalette />
            <MobileActionSheet />
            <MobileRelationshipTree />
            <MobileConfirmDialog />
            <MobileDeleteDialog />
            <MobileDrainOverlay />
            <MobilePodModal />
            <MobileExecWindow />
            <MobileShortcutsHelp />
            <MobileAlertsPanel />
            <MobileToastView />
        </div>
    }
}
