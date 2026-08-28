use leptos::prelude::*;
use roder_core::ResourceKind;

use crate::app::state::{Catalog, NsPaletteOpen, PaletteOpen};
use crate::app::ui::{filter_kinds, filter_namespaces, highlight, use_bool_overlay};

fn use_cursor_scroll(cursor: RwSignal<usize>) -> NodeRef<leptos::html::Ul> {
    let list = NodeRef::<leptos::html::Ul>::new();
    Effect::new(move |_| {
        cursor.track();
        #[cfg(target_arch = "wasm32")]
        if let Some(list) = list.get_untracked() {
            use wasm_bindgen::JsCast as _;
            if let Some(item) = list
                .query_selector(".mobile-picker-item.active")
                .ok()
                .flatten()
                .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
            {
                item.scroll_into_view();
            }
        }
    });
    list
}

fn highlighted(text: String, positions: Vec<usize>) -> impl IntoView {
    highlight(&text, &positions)
        .into_iter()
        .map(|(segment, matched)| {
            if matched {
                view! { <mark>{segment}</mark> }.into_any()
            } else {
                segment.into_any()
            }
        })
        .collect_view()
}

#[component]
pub(crate) fn MobileCommandPalette() -> impl IntoView {
    let open = expect_context::<PaletteOpen>().0;
    let catalog = expect_context::<Catalog>().0;
    let selected = expect_context::<RwSignal<Option<ResourceKind>>>();
    let query = RwSignal::new(String::new());
    let cursor = RwSignal::new(0usize);
    let input = NodeRef::<leptos::html::Input>::new();
    let list = use_cursor_scroll(cursor);
    let (visible, closing, close) = use_bool_overlay(open);
    let matches = Memo::new(move |_| filter_kinds(&catalog.get(), &query.get()));
    Effect::new(move |_| {
        if open.get() {
            query.set(String::new());
            cursor.set(0);
            if let Some(input) = input.get() {
                let _ = input.focus();
            }
        }
    });
    Effect::new(move |_| {
        matches.track();
        cursor.set(0);
    });
    let choose = move |kind: ResourceKind| {
        selected.set(Some(kind));
        close();
    };
    let keyboard = move |event: leptos::ev::KeyboardEvent| {
        let count = matches.with(|items| items.len());
        match event.key().as_str() {
            "ArrowDown" if count > 0 => {
                cursor.update(|value| *value = (*value + 1) % count);
                event.prevent_default();
            }
            "ArrowUp" if count > 0 => {
                cursor.update(|value| *value = if *value == 0 { count - 1 } else { *value - 1 });
                event.prevent_default();
            }
            "Enter" => {
                if let Some((kind, _)) = matches.with(|items| items.get(cursor.get()).cloned()) {
                    choose(kind);
                }
                event.prevent_default();
            }
            _ => {}
        }
    };
    view! { <Show when=move || visible.get()>
        <section class="mobile-picker mobile-command-picker" class:closing=move || closing.get()>
            <header class="mobile-picker-head"><div><small>"Explore"</small><strong>"Resources"</strong></div><button on:click=move |_| close()>"×"</button></header>
            <label class="mobile-picker-search"><span aria-hidden="true">"⌕"</span><input node_ref=input placeholder="Search resource kinds" aria-label="Search resource kinds"
                prop:value=move || query.get() on:input=move |event| query.set(event_target_value(&event)) on:keydown=keyboard /></label>
            <ul class="mobile-picker-list" node_ref=list>
                {move || matches.get().into_iter().enumerate().map(|(index, (kind, positions))| {
                    let display = kind.kind.clone();
                    let group = if kind.group.is_empty() { "core".to_string() } else { kind.group.clone() };
                    view! { <li><button class="mobile-picker-item" class:active=move || cursor.get() == index on:click=move |_| choose(kind.clone())>
                        <span class="mobile-kind-glyph">{display.chars().next().unwrap_or('?')}</span>
                        <span><strong>{highlighted(display, positions)}</strong><small>{group}</small></span><em>{kind.category.label()}</em>
                    </button></li> }
                }).collect_view()}
                {move || matches.with(|items| items.is_empty()).then(|| view! { <li class="mobile-picker-empty"><strong>"No resources found"</strong><span>"Try a kind, API group, or plural name."</span></li> })}
            </ul>
        </section>
    </Show> }
}

#[component]
pub(crate) fn MobileNsPalette() -> impl IntoView {
    let open = expect_context::<NsPaletteOpen>().0;
    let selected = expect_context::<RwSignal<Option<String>>>();
    let namespaces = expect_context::<LocalResource<Result<Vec<String>, String>>>();
    let query = RwSignal::new(String::new());
    let cursor = RwSignal::new(0usize);
    let input = NodeRef::<leptos::html::Input>::new();
    let list = use_cursor_scroll(cursor);
    let (visible, closing, close) = use_bool_overlay(open);
    let matches = Memo::new(move |_| {
        filter_namespaces(
            namespaces.get().and_then(Result::ok).unwrap_or_default(),
            selected.get(),
            &query.get(),
        )
    });
    Effect::new(move |_| {
        if open.get() {
            query.set(String::new());
            cursor.set(0);
            if let Some(input) = input.get() {
                let _ = input.focus();
            }
        }
    });
    Effect::new(move |_| {
        matches.track();
        cursor.set(0);
    });
    let choose = move |namespace: Option<String>| {
        selected.set(namespace);
        close();
    };
    let keyboard = move |event: leptos::ev::KeyboardEvent| {
        let count = matches.with(|items| items.len());
        match event.key().as_str() {
            "ArrowDown" if count > 0 => {
                cursor.update(|value| *value = (*value + 1) % count);
                event.prevent_default();
            }
            "ArrowUp" if count > 0 => {
                cursor.update(|value| *value = if *value == 0 { count - 1 } else { *value - 1 });
                event.prevent_default();
            }
            "Enter" => {
                let value =
                    matches.with(|items| items.get(cursor.get()).and_then(|item| item.0.clone()));
                choose(value);
                event.prevent_default();
            }
            _ => {}
        }
    };
    view! { <Show when=move || visible.get()>
        <section class="mobile-picker mobile-namespace-picker" class:closing=move || closing.get()>
            <header class="mobile-picker-head"><div><small>"Scope"</small><strong>"Namespaces"</strong></div><button on:click=move |_| close()>"×"</button></header>
            <label class="mobile-picker-search"><span aria-hidden="true">"⌕"</span><input node_ref=input placeholder="Search namespaces" aria-label="Search namespaces"
                prop:value=move || query.get() on:input=move |event| query.set(event_target_value(&event)) on:keydown=keyboard /></label>
            <ul class="mobile-picker-list" node_ref=list>
                {move || matches.get().into_iter().enumerate().map(|(index, (namespace, label, positions))| {
                    let active = namespace == selected.get();
                    let value = namespace.clone();
                    view! { <li><button class="mobile-picker-item" class:active=move || cursor.get() == index class:selected=active on:click=move |_| choose(value.clone())>
                        <span class="mobile-ns-glyph" aria-hidden="true"></span><span><strong>{highlighted(label, positions)}</strong><small>{if namespace.is_none() { "Cluster-wide scope" } else { "Namespace" }}</small></span>
                        {active.then(|| view! { <em class="mobile-picker-check">"✓"</em> })}
                    </button></li> }
                }).collect_view()}
                {move || matches.with(|items| items.is_empty()).then(|| view! { <li class="mobile-picker-empty"><strong>"No namespaces found"</strong><span>"Check the spelling and try again."</span></li> })}
            </ul>
        </section>
    </Show> }
}
