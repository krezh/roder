use leptos::prelude::*;

use crate::app::state::ShortcutsOpen;
use crate::app::ui::use_bool_overlay;

const SHORTCUTS: &[(&str, &str, &str)] = &[
    ("Navigation", "⇧K", "Kind palette"),
    ("Navigation", "⇧N", "Namespace switcher"),
    ("Navigation", "/", "Filter current view"),
    ("Navigation", "Esc", "Close overlays"),
    ("Navigation", "?", "Show this help"),
    ("Resource table", "⇧E", "Toggle problem filter"),
    ("Resource table", "⌘C", "Copy selected resource name"),
    ("Resource table", "Enter", "Open selected row"),
    ("Resource table", "L", "Open logs for selected"),
    ("Resource table", "Shift+Click", "Range select"),
    ("Resource table", "⌘+Click", "Toggle selection"),
];

#[component]
pub(crate) fn MobileShortcutsHelp() -> impl IntoView {
    let open = expect_context::<ShortcutsOpen>().0;
    let (visible, closing, close) = use_bool_overlay(open);
    view! { <Show when=move || visible.get()>
        <div class="mobile-modal-scrim" class:closing=move || closing.get() on:click=move |_| close()></div>
        <section class="mobile-shortcuts" class:closing=move || closing.get()>
            <header><div><small>"Reference"</small><strong>"Keyboard shortcuts"</strong></div><button on:click=move |_| close()>"×"</button></header>
            {["Navigation", "Resource table"].into_iter().map(|section| view! {
                <div class="mobile-shortcut-group"><h2>{section}</h2>
                    {SHORTCUTS.iter().filter(|item| item.0 == section).map(|(_, key, label)| view! {
                        <div class="mobile-shortcut-row"><span>{*label}</span><kbd>{*key}</kbd></div>
                    }).collect_view()}
                </div>
            }).collect_view()}
        </section>
    </Show> }
}
