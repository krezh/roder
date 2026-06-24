//! Left navigation: the resource catalog grouped by category with collapsible categories.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use roder_core::{Category, KindStats, ResourceKind};

use crate::app::state::{Catalog, NavOpen, PaneConfig, WorkspaceConf};
use crate::data;

/// Render a single kind entry `<li>` with navigation, context-menu, count badge, and status dot.
fn kind_li(
    k: ResourceKind,
    selected_kind: RwSignal<Option<ResourceKind>>,
    nav_open: RwSignal<bool>,
    kind_ctx: RwSignal<Option<(ResourceKind, i32, i32)>>,
    stats: RwSignal<HashMap<String, KindStats>>,
) -> impl IntoView {
    let k2 = k.clone();
    let k3 = k.clone();
    let key = k.key.clone();
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
        <li class="kind" class:active=active on:click=on_click on:contextmenu=on_ctx>
            {kind_name}
            {move || stats.with(|m| m.get(&key).copied()).and_then(|s| {
                if s.errors > 0 {
                    Some(view! { <span class="kind-err" title=format!("{} error(s)", s.errors)></span> }.into_any())
                } else if s.warnings > 0 {
                    Some(view! { <span class="kind-warn" title=format!("{} warning(s)", s.warnings)></span> }.into_any())
                } else {
                    None
                }
            })}
        </li>
    }
}

#[component]
pub(crate) fn Sidebar() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let catalog = expect_context::<Catalog>().0;
    let workspace = expect_context::<WorkspaceConf>().0;
    let kind_ctx: RwSignal<Option<(ResourceKind, i32, i32)>> = RwSignal::new(None);

    // --- Pinned favorites ---
    let pinned: RwSignal<HashSet<String>> = RwSignal::new(
        data::storage_get("roder.pinned-kinds")
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default(),
    );
    Effect::new(move |_| {
        let mut v: Vec<String> = pinned.get().into_iter().collect();
        v.sort();
        if let Ok(json) = serde_json::to_string(&v) {
            data::storage_set("roder.pinned-kinds", &json);
        }
    });

    // --- Kind stats (count badges + error/warn indicators), refreshed every 30 s ---
    let stats: RwSignal<HashMap<String, KindStats>> = RwSignal::new(HashMap::new());
    #[cfg(target_arch = "wasm32")]
    {
        let selected_ns = expect_context::<RwSignal<Option<String>>>();
        Effect::new(move |_| {
            set_interval(
                move || {
                    let ns = selected_ns.get_untracked();
                    leptos::task::spawn_local(async move {
                        let url = match ns.as_deref() {
                            Some(ns) if !ns.is_empty() => format!(
                                "/api/catalog-stats?namespace={}",
                                data::percent_encode(ns)
                            ),
                            _ => "/api/catalog-stats".to_string(),
                        };
                        if let Ok(map) =
                            data::fetch_json::<HashMap<String, KindStats>>(&url).await
                        {
                            stats.set(map);
                        }
                    });
                },
                std::time::Duration::from_secs(30),
            );
        });
    }

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
        let mut groups: Vec<(Category, Vec<ResourceKind>)> = Vec::new();
        for k in catalog.get() {
            match groups.last_mut() {
                Some((c, v)) if *c == k.category => v.push(k),
                _ => groups.push((k.category.clone(), vec![k])),
            }
        }
        groups
    });

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
                let pins: Vec<ResourceKind> = {
                    let p = pinned.get();
                    cat.iter().filter(|k| p.contains(&k.key)).cloned().collect()
                };
                (!pins.is_empty()).then(|| {
                    let n = pins.len();
                    let items = pins
                        .into_iter()
                        .map(|k| kind_li(k, selected_kind, nav_open, kind_ctx, stats))
                        .collect_view();
                    view! {
                        <div class="cat cat-favorites open">
                            <div class="cat-label">
                                <span class="cat-caret"></span>
                                "Favorites"
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
                    return view! { <div class="muted pad">"Loading…"</div> }.into_any();
                }
                let mut out: Vec<AnyView> = Vec::with_capacity(gs.len() + 1);
                let mut sep_done = false;
                for (cat, kinds) in gs {
                    if cat.is_dynamic() && !sep_done {
                        sep_done = true;
                        out.push(view! { <hr class="sidebar-sep" /> }.into_any());
                    }
                    let n = kinds.len();
                    let label = cat.label();
                    let cat_open = cat.clone();
                    let cat_click = cat.clone();
                    let items = kinds
                        .into_iter()
                        .map(|k| kind_li(k, selected_kind, nav_open, kind_ctx, stats))
                        .collect_view();
                    let is_open = move || open_cats.get().contains(&cat_open);
                    out.push(view! {
                        <div class="cat" class:open=is_open>
                            <div class="cat-label" on:click=move |_| open_cats.update(|s| {
                                if !s.remove(&cat_click) { s.insert(cat_click.clone()); }
                            })>
                                <span class="cat-caret"></span>
                                {label}
                            </div>
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
