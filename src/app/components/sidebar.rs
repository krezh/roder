//! Left navigation: the resource catalog grouped by category with collapsible categories.

use leptos::prelude::*;
use roder_core::{Category, ResourceKind};

use crate::app::state::{Catalog, NavOpen};

#[component]
pub(crate) fn Sidebar() -> impl IntoView {
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let nav_open = expect_context::<NavOpen>().0;
    let catalog = expect_context::<Catalog>().0;

    // Categories are collapsed by default; the one containing the current resource
    // auto-expands (and manual clicks toggle any category).
    let open_cats = RwSignal::new(std::collections::HashSet::<Category>::new());
    Effect::new(move |_| {
        if let Some(k) = selected_kind.get() {
            open_cats.update(|s| {
                s.insert(k.category);
            });
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
                        let active = move || selected_kind.get().as_ref().map(|s| s.key == k2.key).unwrap_or(false);
                        let on_click = {
                            let k = k.clone();
                            move |_| { selected_kind.set(Some(k.clone())); nav_open.set(false); }
                        };
                        view! { <li class="kind" class:active=active on:click=on_click>{k.kind.clone()}</li> }
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
    }
}
