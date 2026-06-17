//! Keyboard shortcuts help overlay (triggered by ? key).

use leptos::prelude::*;

use crate::app::components::icons::ShiftIcon;
use crate::app::state::ShortcutsOpen;

#[component]
pub(crate) fn ShortcutsHelp() -> impl IntoView {
    let open = expect_context::<ShortcutsOpen>().0;
    let (visible, closing, do_close) = super::use_bool_overlay(open);

    view! {
        {move || visible.get().then(|| view! {
            <div class="shortcuts-scrim" class:closing=move || closing.get()
                on:click=move |_| do_close()></div>
            <div class="shortcuts-modal" class:closing=move || closing.get()
                on:click=move |e: leptos::ev::MouseEvent| e.stop_propagation()>
                <div class="shortcuts-head">
                    <span class="shortcuts-title">"Keyboard Shortcuts"</span>
                    <button class="shortcuts-close" on:click=move |_| do_close()>"✕"</button>
                </div>
                <div class="shortcuts-body">
                    <div class="shortcuts-section">
                        <h3>"Navigation"</h3>
                        <div class="shortcut-row">
                            <kbd><ShiftIcon />"K"</kbd>
                            <span>"Kind palette"</span>
                        </div>
                        <div class="shortcut-row">
                            <kbd><ShiftIcon />"N"</kbd>
                            <span>"Namespace switcher"</span>
                        </div>
                        <div class="shortcut-row">
                            <kbd>"/"</kbd>
                            <span>"Filter current view"</span>
                        </div>
                        <div class="shortcut-row">
                            <kbd>"Esc"</kbd>
                            <span>"Close overlays"</span>
                        </div>
                        <div class="shortcut-row">
                            <kbd>"?"</kbd>
                            <span>"Show this help"</span>
                        </div>
                    </div>
                    <div class="shortcuts-section">
                        <h3>"Resource Table"</h3>
                        <div class="shortcut-row">
                            <kbd><ShiftIcon />"E"</kbd>
                            <span>"Toggle problem filter"</span>
                        </div>
                        <div class="shortcut-row">
                            <kbd>"⌘C"</kbd>
                            <span>"Copy selected resource name"</span>
                        </div>
                        <div class="shortcut-row">
                            <kbd>"Enter"</kbd>
                            <span>"Open details for selected row"</span>
                        </div>
                        <div class="shortcut-row">
                            <kbd>"L"</kbd>
                            <span>"Open logs for selected (pods / workloads)"</span>
                        </div>
                        <div class="shortcut-row">
                            <kbd>"Shift+Click"</kbd>
                            <span>"Range select"</span>
                        </div>
                        <div class="shortcut-row">
                            <kbd>"⌘+Click"</kbd>
                            <span>"Toggle selection"</span>
                        </div>
                    </div>
                </div>
            </div>
        })}
    }
}
