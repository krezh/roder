use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};

use crate::app::state::{NavOpen, NsPaletteOpen, PaletteOpen, WorkspaceConf};

#[component]
pub(crate) fn MobileBottomNav() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<roder_core::ResourceKind>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let palette_open = expect_context::<PaletteOpen>().0;
    let namespace_open = expect_context::<NsPaletteOpen>().0;
    let selected_namespace = expect_context::<RwSignal<Option<String>>>();
    let workspace = expect_context::<WorkspaceConf>().0;
    let pathname = use_location().pathname;
    let navigate = use_navigate();
    let go_overview = navigate.clone();
    let go_workspace = navigate;

    view! {
        <nav class="mobile-bottom-nav" aria-label="Primary navigation"
            class:overlay-open=move || palette_open.get() || namespace_open.get()>
            <button class="mobile-tab"
                class:active=move || {
                    !palette_open.get()
                        && !namespace_open.get()
                        && (nav_open.get()
                            || (pathname.get() == "/" && selected_kind.get().is_some()))
                }
                on:click=move |_| {
                    palette_open.set(false);
                    namespace_open.set(false);
                    nav_open.set(true);
                }>
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <rect x="4" y="4" width="6" height="6" rx="1" />
                    <rect x="14" y="4" width="6" height="6" rx="1" />
                    <rect x="4" y="14" width="6" height="6" rx="1" />
                    <rect x="14" y="14" width="6" height="6" rx="1" />
                </svg>
                <span>"Browse"</span>
            </button>
            <button class="mobile-tab"
                class:active=move || {
                    !namespace_open.get()
                        && (palette_open.get() || pathname.get() == "/search")
                }
                on:click=move |_| {
                    namespace_open.set(false);
                    palette_open.update(|open| *open = !*open);
                }>
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <circle cx="10.5" cy="10.5" r="6" />
                    <path d="m15 15 4.5 4.5" />
                </svg>
                <span>"Find"</span>
            </button>
            <button class="mobile-tab"
                class:active=move || {
                    !nav_open.get()
                        && !palette_open.get()
                        && !namespace_open.get()
                        && pathname.get() == "/"
                        && selected_kind.get().is_none()
                }
                on:click=move |_| {
                    palette_open.set(false);
                    namespace_open.set(false);
                    selected_kind.set(None);
                    go_overview("/", Default::default());
                }>
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M4 11.5 12 5l8 6.5V20h-5v-5H9v5H4Z" />
                </svg>
                <span>"Overview"</span>
            </button>
            <button class="mobile-tab"
                class:active=move || namespace_open.get()
                aria-label=move || format!(
                    "Namespace scope: {}",
                    selected_namespace.get().unwrap_or_else(|| "All namespaces".to_string())
                )
                on:click=move |_| {
                    palette_open.set(false);
                    namespace_open.update(|open| *open = !*open);
                }>
                <span class="mobile-tab-icon-wrap">
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                        <rect x="5" y="4" width="14" height="6" rx="2" />
                        <rect x="5" y="14" width="14" height="6" rx="2" />
                    </svg>
                    {move || selected_namespace.get().is_some().then(|| view! {
                        <span class="mobile-tab-scope-dot"></span>
                    })}
                </span>
                <span>"Scope"</span>
            </button>
            <button class="mobile-tab"
                class:active=move || {
                    !nav_open.get()
                        && !palette_open.get()
                        && !namespace_open.get()
                        && pathname.get() == "/workspace"
                }
                on:click=move |_| {
                    palette_open.set(false);
                    namespace_open.set(false);
                    go_workspace("/workspace", Default::default());
                }>
                <span class="mobile-tab-icon-wrap">
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                        <path d="M4 5h7v14H4ZM13 5h7v6h-7ZM13 13h7v6h-7Z" />
                    </svg>
                    {move || {
                        let count = workspace.with(|workspace| workspace.panes.len());
                        (count > 0).then(|| view! { <span class="mobile-tab-badge">{count}</span> })
                    }}
                </span>
                <span>"Workspace"</span>
            </button>
        </nav>
    }
}
