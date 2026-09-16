//! Ctrl+K kind navigator: fuzzy-search the resource catalog and jump to a kind.

use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::icons::TreeKindIcon;
use crate::app::state::{Catalog, PaletteOpen};
use crate::app::ui::{filter_kinds, highlight};

pub(crate) fn use_palette_scroll(cursor: RwSignal<usize>) -> NodeRef<leptos::html::Ul> {
    let list_ref = NodeRef::<leptos::html::Ul>::new();
    Effect::new(move |_| {
        cursor.track();
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast as _;
            if let Some(list) = list_ref.get_untracked() {
                if let Some(el) = list
                    .query_selector(".palette-item-active")
                    .ok()
                    .flatten()
                    .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let item = el.get_bounding_client_rect();
                    let container = list.get_bounding_client_rect();
                    if item.bottom() > container.bottom() {
                        let delta = (item.bottom() - container.bottom()).ceil() as i32;
                        list.set_scroll_top(list.scroll_top() + delta);
                    } else if item.top() < container.top() {
                        let delta = (container.top() - item.top()).ceil() as i32;
                        list.set_scroll_top(list.scroll_top() - delta);
                    }
                }
            }
        }
    });
    list_ref
}

#[component]
pub(crate) fn CommandPalette() -> impl IntoView {
    let palette_open = expect_context::<PaletteOpen>().0;
    let catalog = expect_context::<Catalog>().0;
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let query = RwSignal::new(String::new());
    let cursor = RwSignal::new(0usize);
    let input_ref = NodeRef::<leptos::html::Input>::new();
    let (visible, closing, do_close) = crate::app::ui::use_bool_overlay(palette_open);

    Effect::new(move |_| {
        if palette_open.get() {
            query.set(String::new());
            cursor.set(0);
            if let Some(el) = input_ref.get() {
                let _ = el.focus();
            }
        }
    });

    let matches = Memo::new(move |_| filter_kinds(&catalog.get(), &query.get()));
    let list_ref = use_palette_scroll(cursor);

    Effect::new(move |_| {
        matches.track();
        cursor.set(0);
    });

    let navigate = move |k: ResourceKind| {
        selected_kind.set(Some(k));
        do_close();
    };

    let handle_keydown = move |e: leptos::ev::KeyboardEvent| {
        let n = matches.with(|v| v.len());
        match e.key().as_str() {
            "ArrowDown" => {
                if n > 0 {
                    cursor.update(|i| *i = (*i + 1) % n);
                    e.prevent_default();
                }
            }
            "ArrowUp" => {
                if n > 0 {
                    cursor.update(|i| *i = if *i == 0 { n - 1 } else { *i - 1 });
                    e.prevent_default();
                }
            }
            "Enter" => {
                if let Some((k, _)) = matches.with(|v| v.get(cursor.get()).cloned()) {
                    selected_kind.set(Some(k));
                    do_close();
                }
                e.prevent_default();
            }
            _ => {}
        }
    };

    view! {
        <Show when=move || visible.get() fallback=|| ()>
            <div class="palette-scrim" class:closing=move || closing.get()
                on:click=move |_| do_close()></div>
            <div class="palette palette-command" class:closing=move || closing.get()>
                <div class="palette-mobile-head">
                    <div class="palette-mobile-title">
                        <span>"Explore"</span>
                        <strong>"Resources"</strong>
                    </div>
                    {move || (!query.get().is_empty()).then(|| view! {
                        <span class="palette-result-count">{matches.with(|items| items.len())}</span>
                    })}
                </div>
                <div class="palette-input-wrap">
                    <svg class="palette-search-icon" viewBox="0 0 24 24" aria-hidden="true">
                        <circle cx="11" cy="11" r="6.5" />
                        <path d="m16 16 4 4" />
                    </svg>
                    <input class="palette-input" node_ref=input_ref
                        placeholder="Search resource kinds"
                        aria-label="Search resource kinds"
                        prop:value=move || query.get()
                        on:input=move |e| query.set(event_target_value(&e))
                        on:keydown=handle_keydown />
                </div>
                <ul class="palette-list" node_ref=list_ref>
                    {move || matches.get().into_iter().enumerate().map(|(idx, (k, positions))| {
                        let is_active = move || cursor.get() == idx;
                        let group = if k.group.is_empty() { "core".to_string() } else { k.group.clone() };
                        let segs = highlight(&k.kind, &positions);
                        let icon_category = k.category.clone();
                        let icon_kind = k.kind.clone();
                        view! {
                            <li class="palette-item"
                                class:palette-item-active=is_active
                                on:click=move |_| navigate(k.clone())>
                                <TreeKindIcon category=Some(icon_category) kind=icon_kind small=false />
                                <span class="pi-main">
                                    <span class="pi-kind">
                                        {segs.into_iter().map(|(s, matched)| {
                                            if matched {
                                                view! { <span class="highlight">{s}</span> }.into_any()
                                            } else {
                                                view! { <span>{s}</span> }.into_any()
                                            }
                                        }).collect_view()}
                                    </span>
                                    <span class="pi-group">{group}</span>
                                </span>
                                <span class="pi-cat">{k.category.label()}</span>
                            </li>
                        }
                    }).collect_view()}
                    {move || matches.with(|items| items.is_empty()).then(|| view! {
                        <li class="palette-empty">
                            <strong>"No resources found"</strong>
                            <span>"Try a kind, API group, or plural name."</span>
                        </li>
                    })}
                </ul>
                <div class="palette-hints">
                    <span class="hint"><kbd>"↑↓"</kbd>" navigate"</span>
                    <span class="hint"><kbd>"enter"</kbd>" go"</span>
                    <span class="hint"><kbd>"esc"</kbd>" close"</span>
                </div>
            </div>
        </Show>
    }
}
