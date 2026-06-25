//! Ctrl+N namespace switcher: fuzzy-search namespaces and set the active one.

use leptos::prelude::*;

use crate::app::overlays::palette::{fuzzy_match, highlight};
use crate::app::state::NsPaletteOpen;

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

    // Build the list: "All namespaces" sentinel + actual namespaces, filtered+scored by query.
    // Each entry is (ns, display_label, highlight_positions).
    let matches = Memo::new(move |_| {
        let q = query.get();
        let mut list: Vec<Option<String>> = vec![None]; // None = "All namespaces"
        if let Some(Ok(nss)) = namespaces.get() {
            for ns in nss {
                list.push(Some(ns));
            }
        }
        let mut scored: Vec<(Option<String>, String, Vec<usize>, i32)> = list
            .into_iter()
            .filter_map(|ns| {
                let label = ns.as_deref().unwrap_or("All namespaces").to_string();
                if q.is_empty() {
                    return Some((ns, label, vec![], 0));
                }
                fuzzy_match(&q, &label).map(|(pos, score)| (ns, label, pos, score))
            })
            .collect();
        scored.sort_by(|a, b| {
            // Keep "All namespaces" (None) pinned to top when query is empty,
            // otherwise sort by descending score.
            if q.is_empty() {
                std::cmp::Ordering::Equal
            } else {
                b.3.cmp(&a.3)
            }
        });
        scored
            .into_iter()
            .map(|(ns, label, pos, _)| (ns, label, pos))
            .collect::<Vec<_>>()
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
            <div class="palette" class:closing=move || closing.get()>
                <div class="palette-input-wrap">
                    <input class="palette-input" node_ref=input_ref
                        placeholder="Switch namespace…"
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
                                on:click=move |_| select(ns_click.clone())>
                                <span class="pi-kind">
                                    {segs.into_iter().map(|(s, matched)| {
                                        if matched {
                                            view! { <span class="highlight">{s}</span> }.into_any()
                                        } else {
                                            view! { <span>{s}</span> }.into_any()
                                        }
                                    }).collect_view()}
                                </span>
                                {is_selected.then(|| view! { <span class="pi-cat">"✓ active"</span> })}
                            </li>
                        }
                    }).collect_view()}
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
