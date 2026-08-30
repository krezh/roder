//! Keyboard shortcuts help overlay (triggered by `?`).
//!
//! Rendered entirely from `keys::BINDINGS` — the same table the dispatcher
//! reads — so the help cannot drift from what the keys actually do. Adding a
//! binding to that table is what puts it on this screen.

use leptos::prelude::*;

use crate::app::keys::{Group, BINDINGS};
use crate::app::state::ShortcutsOpen;

#[component]
pub(crate) fn ShortcutsHelp() -> impl IntoView {
    let open = expect_context::<ShortcutsOpen>().0;
    let (visible, closing, do_close) = super::use_bool_overlay(open);
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    crate::app::ui::use_dialog_focus(dialog_ref);

    view! {
        {move || visible.get().then(|| view! {
            <div class="shortcuts-scrim" class:closing=move || closing.get()
                on:click=move |_| do_close()></div>
            <div class="shortcuts-modal" class:closing=move || closing.get() node_ref=dialog_ref
                role="dialog" aria-modal="true" tabindex="-1"
                on:click=move |e: leptos::ev::MouseEvent| e.stop_propagation()>
                <div class="shortcuts-head">
                    <span class="shortcuts-title">"Keyboard Shortcuts"</span>
                    <span class="shortcuts-hint">
                        "Motions take a count — "<kbd>"5j"</kbd>" moves down five rows"
                    </span>
                    <button class="shortcuts-close" on:click=move |_| do_close()>"✕"</button>
                </div>
                <div class="shortcuts-body">
                    {Group::ORDER.into_iter().map(|group| {
                        view! {
                            <div class="shortcuts-section">
                                <h3>{group.title()}</h3>
                                {BINDINGS.iter().filter(|b| b.group == group).map(|binding| view! {
                                    <div class="shortcut-row">
                                        <span class="shortcut-keys">
                                            {binding.keys.iter().map(|k| view! {
                                                <kbd>{*k}</kbd>
                                            }).collect_view()}
                                        </span>
                                        <span>
                                            {binding.label}
                                            {binding.counted.then(|| view! {
                                                <span class="shortcut-count">"{count}"</span>
                                            })}
                                        </span>
                                    </div>
                                }).collect_view()}
                            </div>
                        }
                    }).collect_view()}
                </div>
            </div>
        })}
    }
}
