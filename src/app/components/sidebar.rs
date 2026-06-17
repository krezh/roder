//! Left navigation: the resource catalog grouped by category with collapsible categories.

use leptos::prelude::*;
use roder_core::{Category, ResourceKind};


use crate::app::state::{Catalog, NavOpen, PaneConfig, WorkspaceConf};
use crate::data;

#[component]
pub(crate) fn Sidebar() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let catalog = expect_context::<Catalog>().0;
    let workspace = expect_context::<WorkspaceConf>().0;

    // (kind_key, x, y) — position + target for the inline kind context menu.
    let kind_ctx: RwSignal<Option<(ResourceKind, i32, i32)>> = RwSignal::new(None);

    // Restore saved open/closed state; default to all collapsed.
    let open_cats = RwSignal::new(
        data::storage_get("roder.nav-cats")
            .and_then(|s| serde_json::from_str::<std::collections::HashSet<Category>>(&s).ok())
            .unwrap_or_default(),
    );
    // Auto-expand the category of the currently selected kind.
    Effect::new(move |_| {
        if let Some(k) = selected_kind.get() {
            open_cats.update(|s| {
                s.insert(k.category);
            });
        }
    });
    // Persist on every change.
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
                (n > 0).then(|| {
                    view! {
                        <a class="sidebar-workspace" href="/workspace">
                            "Workspace"
                            <span class="sidebar-ws-badge">{n}</span>
                        </a>
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
                    // Insert a separator before the first dynamic (CRD) category.
                    if cat.is_dynamic() && !sep_done {
                        sep_done = true;
                        out.push(view! { <hr class="sidebar-sep" /> }.into_any());
                    }
                    let n = kinds.len();
                    let label = cat.label();
                    let cat_open = cat.clone();
                    let cat_click = cat.clone();
                    let items = kinds.into_iter().map(|k| {
                        let k2 = k.clone();
                        let k3 = k.clone();
                        let active = move || selected_kind.get().as_ref().map(|s| s.key == k2.key).unwrap_or(false);
                        let on_click = {
                            let k = k.clone();
                            move |_| {
                                selected_kind.set(Some(k.clone()));
                                nav_open.set(false);
                                // Only navigate away when not already at root — avoid
                                // a full-page reload on every kind click at "/".
                                #[cfg(target_arch = "wasm32")]
                                {
                                    let at_root = web_sys::window()
                                        .and_then(|w| w.location().pathname().ok())
                                        .map(|p| p == "/")
                                        .unwrap_or(true);
                                    if !at_root {
                                        crate::data::storage_set(
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
                        view! { <li class="kind" class:active=active on:click=on_click on:contextmenu=on_ctx>{k.kind.clone()}</li> }
                    }).collect_view();
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
            let key_add = k.key.clone();
            let key_rm = k.key.clone();
            let add = move |_| {
                workspace.update(|w| {
                    if !w.panes.iter().any(|p| p.kind_key == key_add) {
                        w.panes.push(PaneConfig { kind_key: key_add.clone(), namespace: None, selector: None });
                    }
                });
                kind_ctx.set(None);
            };
            let remove = move |_| {
                workspace.update(|w| w.panes.retain(|p| p.kind_key != key_rm));
                kind_ctx.set(None);
            };
            let style = format!("left:{}px;top:{}px", x, y);
            view! {
                <div class="ctx-scrim"
                    on:click=move |_| kind_ctx.set(None)
                    on:contextmenu=move |e: leptos::ev::MouseEvent| { e.prevent_default(); kind_ctx.set(None); }>
                </div>
                <div class="ctx-menu" style=style>
                    <div class="ctx-item ctx-bulk-header">{k.kind.clone()}</div>
                    {if in_ws {
                        view! { <button class="ctx-item" on:click=remove>"Remove from workspace"</button> }.into_any()
                    } else {
                        view! { <button class="ctx-item" on:click=add>"Add to workspace"</button> }.into_any()
                    }}
                </div>
            }
        })}
    }
}
