//! Left navigation: the resource catalog grouped by category with collapsible categories.

use std::collections::HashSet;

use leptos::prelude::*;
use roder_core::{Category, ResourceKind};

use crate::app::components::icons::CtrlIcon;
use crate::app::state::{
    pinned_in_catalog_order, Catalog, NavOpen, PaneConfig, PinnedKinds, WorkspaceConf,
};
use crate::data;

/// The digit that reaches favourite `index`, following the keyboard's own row:
/// slots 1-9 take `1`..`9` and the tenth takes `0`. Beyond that there is no key,
/// so the pin simply has no shortcut.
pub(crate) fn hotkey_digit(index: usize) -> Option<char> {
    match index {
        0..=8 => char::from_digit(index as u32 + 1, 10),
        9 => Some('0'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::hotkey_digit;

    /// The chip drawn beside a pin and the key that reaches it come from this
    /// one function, so the row it implies must be exactly 1234567890.
    #[test]
    fn hotkeys_follow_the_number_row() {
        let row: String = (0..10).filter_map(hotkey_digit).collect();
        assert_eq!(row, "1234567890");
    }

    #[test]
    fn eleventh_pin_onwards_has_no_hotkey() {
        assert_eq!(hotkey_digit(9), Some('0'));
        assert_eq!(hotkey_digit(10), None);
        assert_eq!(hotkey_digit(99), None);
    }
}

/// Render a single kind entry `<li>` with navigation and context-menu.
fn kind_li(
    k: ResourceKind,
    selected_kind: RwSignal<Option<ResourceKind>>,
    nav_open: RwSignal<bool>,
    kind_ctx: RwSignal<Option<(ResourceKind, i32, i32)>>,
    // Shown as a `⌃n` chip on pinned entries; `None` everywhere else.
    hotkey: Option<char>,
) -> impl IntoView {
    let k2 = k.clone();
    let k3 = k.clone();
    let kind_name = k.kind.clone();

    let active = move || {
        selected_kind
            .get()
            .as_ref()
            .map(|s| s.key == k2.key)
            .unwrap_or(false)
    };
    let on_click = {
        let k = k.clone();
        move |_| {
            selected_kind.set(Some(k.clone()));
            nav_open.set(false);
            #[cfg(target_arch = "wasm32")]
            {
                let at_root = web_sys::window()
                    .and_then(|w| w.location().pathname().ok())
                    .map(|p| p == "/")
                    .unwrap_or(true);
                if !at_root {
                    data::storage_set(
                        "roder.nav",
                        &serde_json::json!({ "kind": k.key }).to_string(),
                    );
                    if let Some(win) = web_sys::window() {
                        let _ = win.location().set_href("/");
                    }
                }
            }
        }
    };
    let on_ctx = move |e: leptos::ev::MouseEvent| {
        e.prevent_default();
        e.stop_propagation();
        kind_ctx.set(Some((k3.clone(), e.client_x(), e.client_y())));
    };

    view! {
        <li>
            <button type="button" class="kind" class:active=active
                on:click=on_click on:contextmenu=on_ctx>
                {kind_name}
                // The same `<kbd>` chip the topbar uses for its hints: a drawn
                // modifier icon plus the key, exactly as its "⇧K" is built. The
                // literal "⌃" character degraded into a stray-looking caret at
                // this size, hence the SVG.
                {hotkey.map(|d| view! {
                    <kbd class="kind-hotkey"><CtrlIcon />{d.to_string()}</kbd>
                })}
            </button>
        </li>
    }
}

#[component]
pub(crate) fn Sidebar(#[prop(optional)] filter: Option<Signal<String>>) -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let catalog = expect_context::<Catalog>().0;
    let workspace = expect_context::<WorkspaceConf>().0;
    let kind_ctx: RwSignal<Option<(ResourceKind, i32, i32)>> = RwSignal::new(None);

    // Pinned favorites. Owned by `App` (and persisted there) so the keyboard
    // dispatcher can bind Ctrl+1..Ctrl+0 to the same set.
    let pinned = expect_context::<PinnedKinds>().0;

    // --- Category open/closed state ---
    let open_cats = RwSignal::new(
        data::storage_get("roder.nav-cats")
            .and_then(|s| serde_json::from_str::<HashSet<Category>>(&s).ok())
            .unwrap_or_default(),
    );
    Effect::new(move |_| {
        if let Some(k) = selected_kind.get() {
            open_cats.update(|s| {
                s.insert(k.category);
            });
        }
    });
    Effect::new(move |_| {
        let cats = open_cats.get();
        if let Ok(json) = serde_json::to_string(&cats) {
            data::storage_set("roder.nav-cats", &json);
        }
    });

    let groups = Memo::new(move |_| {
        let query = filter
            .map(|filter| filter.get().trim().to_lowercase())
            .unwrap_or_default();
        let mut groups: Vec<(Category, Vec<ResourceKind>)> = Vec::new();
        for k in catalog.get().into_iter().filter(|kind| {
            query.is_empty()
                || kind.kind.to_lowercase().contains(&query)
                || kind.plural.to_lowercase().contains(&query)
                || kind.group.to_lowercase().contains(&query)
                || kind.category.label().to_lowercase().contains(&query)
        }) {
            match groups.last_mut() {
                Some((c, v)) if *c == k.category => v.push(k),
                _ => groups.push((k.category.clone(), vec![k])),
            }
        }
        groups
    });
    let searching =
        Signal::derive(move || filter.is_some_and(|filter| !filter.get().trim().is_empty()));

    view! {
        <nav class="sidebar">
            // Workspace shortcut — only shown when there are panes.
            {move || {
                let n = workspace.with(|w| w.panes.len());
                (n > 0).then(|| view! {
                    <a class="sidebar-workspace" href="/workspace">
                        "Workspace"
                        <span class="sidebar-ws-badge">{n}</span>
                    </a>
                })
            }}
            // Pinned favorites section — hidden when empty.
            {move || {
                let cat = catalog.get();
                // Number every pin from the unfiltered list, so a pin's hotkey
                // is stable while the sidebar search box is narrowing the view.
                let numbered: Vec<(usize, ResourceKind)> =
                    pinned_in_catalog_order(&cat, &pinned.get())
                        .into_iter()
                        .enumerate()
                        .collect();
                let pins: Vec<(usize, ResourceKind)> = {
                    let query = filter
                        .map(|filter| filter.get().trim().to_lowercase())
                        .unwrap_or_default();
                    numbered
                        .into_iter()
                        .filter(|(_, k)| {
                            query.is_empty()
                                || k.kind.to_lowercase().contains(&query)
                                || k.plural.to_lowercase().contains(&query)
                        })
                        .collect()
                };
                (!pins.is_empty()).then(|| {
                    let n = pins.len();
                    let items = pins
                        .into_iter()
                        .map(|(i, k)| kind_li(k, selected_kind, nav_open, kind_ctx, hotkey_digit(i)))
                        .collect_view();
                    view! {
                            <div class="cat cat-favorites open">
                                <div class="cat-label">
                                    <span class="cat-caret"></span>
                                    "Favorites"
                                    <span class="cat-count">{n}</span>
                                </div>
                            <div class="cat-items" style=format!("--list-h: calc(var(--item-h) * {n})")>
                                <ul>{items}</ul>
                            </div>
                        </div>
                    }
                })
            }}
            {move || {
                let gs = groups.get();
                if gs.is_empty() {
                    let message = if searching.get() {
                        "No matching resources"
                    } else {
                        "Loading…"
                    };
                    return view! { <div class="sidebar-empty">{message}</div> }.into_any();
                }
                let mut out: Vec<AnyView> = Vec::with_capacity(gs.len() + 2);
                let mut core_done = false;
                let mut extensions_done = false;
                for (cat, kinds) in gs {
                    let label = cat.label();
                    if cat.is_dynamic() && !extensions_done {
                        extensions_done = true;
                        out.push(view! { <div class="sidebar-section">"Extensions"</div> }.into_any());
                    } else if !cat.is_dynamic() && !core_done {
                        core_done = true;
                        out.push(view! { <div class="sidebar-section">"Core"</div> }.into_any());
                    }
                    let n = kinds.len();
                    let cat_open = cat.clone();
                    let cat_expanded = cat.clone();
                    let cat_active = cat.clone();
                    let cat_click = cat.clone();
                    let items = kinds
                        .into_iter()
                        .map(|k| kind_li(k, selected_kind, nav_open, kind_ctx, None))
                        .collect_view();
                    out.push(view! {
                        <div class="cat"
                            class:open=move || searching.get() || open_cats.get().contains(&cat_open)
                            class:active-category=move || selected_kind.get()
                                .is_some_and(|k| k.category == cat_active)>
                            <button type="button" class="cat-label"
                                aria-expanded=move || searching.get() || open_cats.get().contains(&cat_expanded)
                                on:click=move |_| open_cats.update(|s| {
                                if !s.remove(&cat_click) { s.insert(cat_click.clone()); }
                            })>
                                <span class="cat-caret"></span>
                                {label}
                                <span class="cat-count">{n}</span>
                            </button>
                            <div class="cat-items" style=format!("--list-h: calc(var(--item-h) * {n})")>
                                <ul>{items}</ul>
                            </div>
                        </div>
                    }.into_any());
                }
                out.into_any()
            }}
        </nav>
        // Kind-level right-click popup.
        {move || kind_ctx.get().map(|(k, x, y)| {
            let key = k.key.clone();
            let in_ws = workspace.with(|w| w.panes.iter().any(|p| p.kind_key == key));
            let is_pinned = pinned.with(|p| p.contains(&key));
            let key_add = k.key.clone();
            let key_rm = k.key.clone();
            let key_pin = k.key.clone();
            let key_unpin = k.key.clone();
            let add = move |_| {
                workspace.update(|w| {
                    if !w.panes.iter().any(|p| p.kind_key == key_add) {
                        w.panes.push(PaneConfig {
                            kind_key: key_add.clone(),
                            namespace: None,
                            selector: None,
                        });
                    }
                });
                kind_ctx.set(None);
            };
            let remove = move |_| {
                workspace.update(|w| w.panes.retain(|p| p.kind_key != key_rm));
                kind_ctx.set(None);
            };
            let pin = move |_| {
                pinned.update(|p| { p.insert(key_pin.clone()); });
                kind_ctx.set(None);
            };
            let unpin = move |_| {
                pinned.update(|p| { p.remove(&key_unpin); });
                kind_ctx.set(None);
            };
            let style = format!("left:{}px;top:{}px", x, y);
            view! {
                <div class="ctx-scrim"
                    on:click=move |_| kind_ctx.set(None)
                    on:contextmenu=move |e: leptos::ev::MouseEvent| {
                        e.prevent_default();
                        kind_ctx.set(None);
                    }>
                </div>
                <div class="ctx-menu" style=style>
                    <div class="ctx-item ctx-bulk-header">{k.kind.clone()}</div>
                    {if in_ws {
                        view! { <button class="ctx-item" on:click=remove>"Remove from workspace"</button> }.into_any()
                    } else {
                        view! { <button class="ctx-item" on:click=add>"Add to workspace"</button> }.into_any()
                    }}
                    {if is_pinned {
                        view! { <button class="ctx-item" on:click=unpin>"Unpin from favorites"</button> }.into_any()
                    } else {
                        view! { <button class="ctx-item" on:click=pin>"Pin to favorites"</button> }.into_any()
                    }}
                </div>
            }
        })}
    }
}
