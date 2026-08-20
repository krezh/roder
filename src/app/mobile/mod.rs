//! Structurally separate mobile UI, mounted instead of the desktop tree below
//! the 760px breakpoint (see `MobileMode` in `state.rs`). Shares every signal/
//! context provided by `App()` and every pure data/plumbing module (`hooks`,
//! `events`, `state`, `util`) with the desktop tree — only the rendering layer
//! differs. Components not yet given a mobile-native treatment are reused
//! as-is from the desktop tree (progressively replaced phase by phase).

mod action_sheet;
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
use detail_view::MobileDetailView;
use header::MobileHeader;
use logs_view::MobileLogsView;
use resource_list::MobileResourceView;
use search_list::MobileSearchList;
use workspace_view::MobileWorkspaceView;

#[component]
pub(crate) fn MobileShell() -> impl IntoView {
    let nav_open = expect_context::<NavOpen>().0;

    view! {
        <div class="mobile-shell" class:nav-open=move || nav_open.get()>
            <MobileHeader />
            <div class="mobile-sidebar-scrim" class:open=move || nav_open.get()
                on:click=move |_| nav_open.set(false)></div>
            <Sidebar />
            <main class="mobile-main">
                <Routes fallback=|| view! { <p class="empty">"Not found."</p> }>
                    <Route path=StaticSegment("") view=MobileResourceView />
                    <Route path=StaticSegment("search") view=MobileSearchList />
                    <Route path=StaticSegment("workspace") view=MobileWorkspaceView />
                </Routes>
            </main>
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
