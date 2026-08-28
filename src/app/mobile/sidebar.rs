use std::collections::HashSet;

use leptos::prelude::*;
use roder_core::{Category, ResourceKind};

use crate::app::state::{Catalog, NavOpen, PaneConfig, WorkspaceConf};
use crate::data;

fn matches_query(kind: &ResourceKind, query: &str) -> bool {
    query.is_empty()
        || kind.kind.to_lowercase().contains(query)
        || kind.plural.to_lowercase().contains(query)
        || kind.group.to_lowercase().contains(query)
        || kind.category.label().to_lowercase().contains(query)
}

#[component]
fn MobileKindLink(
    kind: ResourceKind,
    pinned: RwSignal<HashSet<String>>,
    workspace: RwSignal<crate::app::state::WorkspaceConfig>,
) -> impl IntoView {
    let selected = expect_context::<RwSignal<Option<ResourceKind>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let key = kind.key.clone();
    let select_kind = kind.clone();
    let workspace_key = key.clone();
    let workspace_active_key = workspace_key.clone();
    let pin_key = key.clone();
    let pin_active_key = pin_key.clone();
    view! {
        <li class="mobile-kind-row">
            <button type="button" class="mobile-kind-link"
                class:active=move || selected.get().is_some_and(|selected| selected.key == key)
                on:click=move |_| {
                    selected.set(Some(select_kind.clone()));
                    nav_open.set(false);
                    #[cfg(target_arch = "wasm32")]
                    if web_sys::window().and_then(|window| window.location().pathname().ok()).is_some_and(|path| path != "/") {
                        data::storage_set("roder.nav", &serde_json::json!({ "kind": select_kind.key }).to_string());
                        if let Some(window) = web_sys::window() { let _ = window.location().set_href("/"); }
                    }
                }>{kind.kind}</button>
            <button type="button" class="mobile-kind-action" aria-label="Toggle workspace"
                class:active=move || workspace.with(|value| value.panes.iter().any(|pane| pane.kind_key == workspace_active_key))
                on:click=move |_| workspace.update(|value| {
                    if value.panes.iter().any(|pane| pane.kind_key == workspace_key) {
                        value.panes.retain(|pane| pane.kind_key != workspace_key);
                    } else {
                        value.panes.push(PaneConfig { kind_key: workspace_key.clone(), namespace: None, selector: None });
                    }
                })>"▦"</button>
            <button type="button" class="mobile-kind-action" aria-label="Toggle favorite"
                class:active=move || pinned.with(|value| value.contains(&pin_active_key))
                on:click=move |_| pinned.update(|value| { if !value.remove(&pin_key) { value.insert(pin_key.clone()); } })>"★"</button>
        </li>
    }
}

#[component]
pub(crate) fn MobileSidebar(filter: Signal<String>) -> impl IntoView {
    let catalog = expect_context::<Catalog>().0;
    let selected = expect_context::<RwSignal<Option<ResourceKind>>>();
    let workspace = expect_context::<WorkspaceConf>().0;
    let pinned: RwSignal<HashSet<String>> = RwSignal::new(
        data::storage_get("roder.pinned-kinds")
            .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
            .map(|values| values.into_iter().collect())
            .unwrap_or_default(),
    );
    let open_categories = RwSignal::new(
        data::storage_get("roder.nav-cats")
            .and_then(|value| serde_json::from_str::<HashSet<Category>>(&value).ok())
            .unwrap_or_default(),
    );
    Effect::new(move |_| {
        let mut values: Vec<_> = pinned.get().into_iter().collect();
        values.sort();
        if let Ok(value) = serde_json::to_string(&values) {
            data::storage_set("roder.pinned-kinds", &value);
        }
    });
    Effect::new(move |_| {
        if let Some(kind) = selected.get() {
            open_categories.update(|values| {
                values.insert(kind.category);
            });
        }
    });
    Effect::new(move |_| {
        if let Ok(value) = serde_json::to_string(&open_categories.get()) {
            data::storage_set("roder.nav-cats", &value);
        }
    });
    let groups = Memo::new(move |_| {
        let query = filter.get().trim().to_lowercase();
        let mut groups: Vec<(Category, Vec<ResourceKind>)> = Vec::new();
        for kind in catalog
            .get()
            .into_iter()
            .filter(|kind| matches_query(kind, &query))
        {
            match groups.last_mut() {
                Some((category, kinds)) if *category == kind.category => kinds.push(kind),
                _ => groups.push((kind.category.clone(), vec![kind])),
            }
        }
        groups
    });
    let searching = Signal::derive(move || !filter.get().trim().is_empty());
    view! {
        <nav class="mobile-resource-nav">
            {move || (workspace.with(|value| value.panes.len()) > 0).then(|| view! {
                <a class="mobile-workspace-link" href="/workspace"><span>"Workspace"</span><b>{workspace.with(|value| value.panes.len())}</b></a>
            })}
            {move || {
                let query = filter.get().trim().to_lowercase();
                let favorites: Vec<_> = catalog.get().into_iter().filter(|kind| pinned.with(|values| values.contains(&kind.key)) && matches_query(kind, &query)).collect();
                (!favorites.is_empty()).then(|| view! {
                    <section class="mobile-nav-group open"><header><span>"Favorites"</span><b>{favorites.len()}</b></header><ul>
                        {favorites.into_iter().map(|kind| view! { <MobileKindLink kind pinned workspace /> }).collect_view()}
                    </ul></section>
                })
            }}
            {move || {
                let values = groups.get();
                if values.is_empty() { return view! { <p class="mobile-nav-empty">{if searching.get() { "No matching resources" } else { "Loading…" }}</p> }.into_any(); }
                values.into_iter().map(|(category, kinds)| {
                    let expanded_category = category.clone();
                    let toggle_category = category.clone();
                    let active_category = category.clone();
                    let count = kinds.len();
                    view! { <section class="mobile-nav-group"
                        class:open=move || searching.get() || open_categories.with(|values| values.contains(&expanded_category))
                        class:active=move || selected.get().is_some_and(|kind| kind.category == active_category)>
                        <button type="button" class="mobile-nav-group-head" aria-expanded=move || searching.get() || open_categories.with(|values| values.contains(&category))
                            on:click=move |_| open_categories.update(|values| { if !values.remove(&toggle_category) { values.insert(toggle_category.clone()); } })>
                            <span>{category.label()}</span><b>{count}</b><i>"⌄"</i>
                        </button>
                        <ul>{kinds.into_iter().map(|kind| view! { <MobileKindLink kind pinned workspace /> }).collect_view()}</ul>
                    </section> }
                }).collect_view().into_any()
            }}
        </nav>
    }
}
