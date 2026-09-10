//! Ctrl+N namespace switcher: fuzzy-search namespaces and set the active one.

use leptos::prelude::*;

use crate::app::state::NsPaletteOpen;
use crate::app::ui::{filter_namespaces, highlight};

#[component]
pub(crate) fn NsPalette() -> impl IntoView {
    let open = expect_context::<NsPaletteOpen>().0;
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let namespaces = expect_context::<LocalResource<Result<Vec<String>, String>>>();
    let query = RwSignal::new(String::new());
    let cursor = RwSignal::new(0usize);
    let input_ref = NodeRef::<leptos::html::Input>::new();
    let (visible, closing, do_close) = super::use_bool_overlay(open);

    Effect::new(move |_| {
        if open.get() {
            query.set(String::new());
            cursor.set(0);
            if let Some(el) = input_ref.get() {
                let _ = el.focus();
            }
        }
    });

    let matches = Memo::new(move |_| {
        filter_namespaces(
            namespaces.get().and_then(Result::ok).unwrap_or_default(),
            selected_ns.get(),
            &query.get(),
        )
    });

    let list_ref = super::palette::use_palette_scroll(cursor);

    Effect::new(move |_| {
        matches.track();
        cursor.set(0);
    });

    let select = move |ns: Option<String>| {
        selected_ns.set(ns);
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
                let chosen =
                    matches.with(|v| v.get(cursor.get()).and_then(|(ns, _, _)| ns.clone()));
                select(chosen);
                e.prevent_default();
            }
            _ => {}
        }
    };

    view! {
        <Show when=move || visible.get() fallback=|| ()>
            <div class="palette-scrim" class:closing=move || closing.get()
                on:click=move |_| do_close()></div>
            <div class="palette palette-namespace" class:closing=move || closing.get()>
                <div class="palette-mobile-head">
                    <div class="palette-mobile-title">
                        <span>"Scope"</span>
                        <strong>"Namespaces"</strong>
                    </div>
                    <span class="palette-result-count">{move || {
                        let count = matches.with(|items| items.len());
                        if query.get().is_empty() { count.saturating_sub(1) } else { count }
                    }}</span>
                </div>
                <div class="palette-input-wrap">
                    <svg class="palette-search-icon" viewBox="0 0 24 24" aria-hidden="true">
                        <circle cx="11" cy="11" r="6.5" />
                        <path d="m16 16 4 4" />
                    </svg>
                    <input class="palette-input" node_ref=input_ref
                        placeholder="Search namespaces"
                        aria-label="Search namespaces"
                        prop:value=move || query.get()
                        on:input=move |e| query.set(event_target_value(&e))
                        on:keydown=handle_keydown />
                </div>
                <ul class="palette-list" node_ref=list_ref>
                    {move || matches.get().into_iter().enumerate().map(|(idx, (ns, label, positions))| {
                        let is_active = move || cursor.get() == idx;
                        let cur_ns = selected_ns.get();
                        let is_selected = ns == cur_ns;
                        let ns_click = ns.clone();
                        let segs = highlight(&label, &positions);
                        view! {
                            <li class="palette-item"
                                class:palette-item-active=is_active
                                class:palette-item-selected=is_selected
                                on:click=move |_| select(ns_click.clone())>
                                <span class="ns-scope-icon" aria-hidden="true"></span>
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
                                    <span class="pi-group">{if ns.is_none() { "Cluster-wide scope" } else { "Namespace" }}</span>
                                </span>
                                {is_selected.then(|| view! { <span class="ns-active-check" aria-label="Active">"✓"</span> })}
                            </li>
                        }
                    }).collect_view()}
                    {move || matches.with(|items| items.is_empty()).then(|| view! {
                        <li class="palette-empty">
                            <strong>"No namespaces found"</strong>
                            <span>"Check the spelling and try again."</span>
                        </li>
                    })}
                </ul>
                <div class="palette-hints">
                    <span class="hint"><kbd>"↑↓"</kbd>" navigate"</span>
                    <span class="hint"><kbd>"enter"</kbd>" switch"</span>
                    <span class="hint"><kbd>"esc"</kbd>" close"</span>
                </div>
            </div>
        </Show>
    }
}
