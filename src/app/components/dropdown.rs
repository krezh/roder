//! Reusable morphing dropdown menu: a single box that spring-morphs between
//! two states rather than a button plus a separately-positioned popup —
//! closed, it hugs the trigger label; open, the *same* box grows to the
//! menu's natural size. Extracted from the topbar identity menu (its first
//! and, until now, only user) so other menus — e.g. the delete dialog's
//! Propagation picker — get the same look and mechanics instead of a
//! hand-rolled popup.
//!
//! CSS can't transition to/from `auto`, so opening/closing does a small FLIP
//! dance (see `open_menu`/`close_menu`): both natural sizes are measured up
//! front and the transition runs between explicit pixel values.

use leptos::prelude::*;

/// Covers the close choreography's longest path (0.06s content-leads delay +
/// the 0.34s box-shrink spring) so the cleanup below never fires early.
const CLOSE_MS: u64 = 460;

/// Provided to `children` so a menu item's own click handler can close the
/// menu after acting on a selection (`children` is built at the call site,
/// not inside `Dropdown`, so this travels via context rather than a prop).
#[derive(Clone, Copy)]
pub(crate) struct DropdownClose(pub(crate) Callback<()>);

#[component]
pub(crate) fn Dropdown(
    /// Trigger label, shown on both the closed button and the layout-reserving
    /// spacer. Boxed into a `StoredValue` (mirrors `Coalescer`'s `apply` in
    /// `app::hooks`) so the one closure can be read from both reactive spots.
    label: impl Fn() -> String + Send + Sync + 'static,
    /// Menu content — always mounted, just CSS-hidden until open (no caller
    /// today needs a per-open reset).
    children: Children,
) -> impl IntoView {
    let label: StoredValue<Box<dyn Fn() -> String + Send + Sync>> =
        StoredValue::new(Box::new(label));
    let open = RwSignal::new(false);
    let closing = RwSignal::new(false);
    let anchor_ref = NodeRef::<leptos::html::Div>::new();
    let shell_ref = NodeRef::<leptos::html::Div>::new();
    let btn_ref = NodeRef::<leptos::html::Button>::new();

    // Shared close path for outside-click, Escape, and item selection: play
    // the shrink-back animation, then clear the inline FLIP styles once it's
    // had time to finish.
    let do_close = move || {
        if closing.get_untracked() || !open.get_untracked() {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        close_menu(shell_ref, btn_ref);
        open.set(false);
        closing.set(true);
        #[cfg(target_arch = "wasm32")]
        if let Some(button) = btn_ref.get_untracked() {
            let _ = button.focus();
        }
        set_timeout(
            move || {
                if closing.get_untracked() {
                    #[cfg(target_arch = "wasm32")]
                    if let Some(shell) = shell_ref.get_untracked() {
                        let _ = shell.class_list().remove_1("closing");
                        let style = web_sys::HtmlElement::style(&shell);
                        let _ = style.remove_property("width");
                        let _ = style.remove_property("height");
                    }
                    closing.set(false);
                }
            },
            std::time::Duration::from_millis(CLOSE_MS),
        );
    };
    provide_context(DropdownClose(Callback::new(move |()| do_close())));

    let toggle = move |_: leptos::ev::MouseEvent| {
        if open.get_untracked() {
            do_close();
            return;
        }
        #[cfg(target_arch = "wasm32")]
        open_menu(shell_ref);
        closing.set(false);
        open.set(true);
        #[cfg(target_arch = "wasm32")]
        focus_dropdown_item(shell_ref, 0);
    };

    let handle_keydown = move |_event: leptos::ev::KeyboardEvent| {
        #[cfg(target_arch = "wasm32")]
        if !open.get_untracked() {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        match _event.key().as_str() {
            "ArrowDown" | "j" => {
                _event.prevent_default();
                _event.stop_propagation();
                step_dropdown_focus(shell_ref, 1);
            }
            "ArrowUp" | "k" => {
                _event.prevent_default();
                _event.stop_propagation();
                step_dropdown_focus(shell_ref, -1);
            }
            "Home" => {
                _event.prevent_default();
                _event.stop_propagation();
                focus_dropdown_item(shell_ref, 0);
            }
            "End" => {
                _event.prevent_default();
                _event.stop_propagation();
                focus_dropdown_item(shell_ref, -1);
            }
            "Escape" | "Tab" => {
                _event.prevent_default();
                _event.stop_propagation();
                do_close();
            }
            _ => {}
        }
    };

    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        reserve_width(anchor_ref, shell_ref);

        let handle = window_event_listener(leptos::ev::keydown, move |e| {
            if e.key() == "Escape" {
                do_close();
            }
        });
        on_cleanup(move || handle.remove());
    });

    view! {
        <div class="dropdown-anchor" node_ref=anchor_ref>
            // Reserves the closed button's footprint in normal flow so a
            // parent laid out in a row (e.g. the topbar) never reflows while
            // the shell (position: absolute) morphs on top of it.
            <span class="dropdown-spacer" aria-hidden="true">
                <span>{move || label.with_value(|l| l())}</span>
            </span>
            <div class="dropdown-shell" node_ref=shell_ref on:keydown=handle_keydown>
                <button class="dropdown-face-btn" node_ref=btn_ref on:click=toggle
                    aria-haspopup="menu" aria-expanded=move || open.get().to_string()>
                    <span>{move || label.with_value(|l| l())}</span>
                </button>
                <div class="dropdown-face-menu" role="menu">
                    {children()}
                </div>
            </div>
            {move || open.get().then(|| view! {
                <div class="dropdown-scrim" on:click=move |_| do_close()></div>
            })}
        </div>
    }
}

#[cfg(target_arch = "wasm32")]
fn dropdown_items(shell: &web_sys::HtmlDivElement) -> Vec<web_sys::HtmlElement> {
    use wasm_bindgen::JsCast;

    let Ok(nodes) = shell.query_selector_all(".dropdown-face-menu .dropdown-item") else {
        return Vec::new();
    };
    (0..nodes.length())
        .filter_map(|index| nodes.item(index))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .collect()
}

#[cfg(target_arch = "wasm32")]
fn focus_dropdown_item(shell_ref: NodeRef<leptos::html::Div>, index: i32) {
    let Some(shell) = shell_ref.get_untracked() else {
        return;
    };
    let items = dropdown_items(&shell);
    if items.is_empty() {
        return;
    }
    let index = index.rem_euclid(items.len() as i32) as usize;
    let _ = items[index].focus();
}

#[cfg(target_arch = "wasm32")]
fn step_dropdown_focus(shell_ref: NodeRef<leptos::html::Div>, delta: i32) {
    use wasm_bindgen::JsCast;

    let Some(shell) = shell_ref.get_untracked() else {
        return;
    };
    let items = dropdown_items(&shell);
    if items.is_empty() {
        return;
    }
    let active = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.active_element());
    let current = items.iter().position(|item| {
        active
            .as_ref()
            .is_some_and(|active| item.dyn_ref::<web_sys::Element>() == Some(active))
    });
    let next = match current {
        Some(index) => (index as i32 + delta).rem_euclid(items.len() as i32),
        None if delta >= 0 => 0,
        None => items.len() as i32 - 1,
    };
    let _ = items[next as usize].focus();
}

/// Measure the shell's natural size with a given class temporarily applied
/// (e.g. `"open"` to get the menu face's size while the button face is still
/// showing), restoring any inline size that was already set.
#[cfg(target_arch = "wasm32")]
fn measure(shell: &web_sys::HtmlDivElement) -> (f64, f64) {
    let class_list = shell.class_list();
    let _ = class_list.add_1("measuring");
    // `web_sys::HtmlElement::style` fully qualified: it'd otherwise resolve to
    // tachys's own `ElementExt::style` builder method (same name, different
    // signature) since that blanket trait impl is found by autoderef before
    // reaching the inherent method on `HtmlElement`.
    let style = web_sys::HtmlElement::style(shell);
    let prev_w = style.get_property_value("width").unwrap_or_default();
    let prev_h = style.get_property_value("height").unwrap_or_default();
    let _ = style.set_property("width", "max-content");
    let _ = style.set_property("height", "auto");
    let size = (
        f64::from(shell.offset_width()),
        f64::from(shell.offset_height()),
    );
    if prev_w.is_empty() {
        let _ = style.remove_property("width");
    } else {
        let _ = style.set_property("width", &prev_w);
    }
    if prev_h.is_empty() {
        let _ = style.remove_property("height");
    } else {
        let _ = style.set_property("height", &prev_h);
    }
    let _ = class_list.remove_1("measuring");
    size
}

#[cfg(target_arch = "wasm32")]
fn reserve_width(anchor_ref: NodeRef<leptos::html::Div>, shell_ref: NodeRef<leptos::html::Div>) {
    let (Some(anchor), Some(shell)) = (anchor_ref.get_untracked(), shell_ref.get_untracked())
    else {
        return;
    };
    let width = f64::from(shell.offset_width()).max(measure(&shell).0);
    let width = format!("{width}px");
    let _ = web_sys::HtmlElement::style(&anchor).set_property("width", &width);
    let _ = web_sys::HtmlElement::style(&shell).set_property("min-width", &width);
}

/// FLIP the shell from its current (button) footprint to the menu's natural
/// size: measure both, pin the start size as an explicit pixel value, force a
/// reflow so that's the committed starting point, then set the target pixel
/// value so the CSS `transition` on `width`/`height` actually has a delta to
/// animate between (a transition to/from `auto` doesn't animate at all).
#[cfg(target_arch = "wasm32")]
fn open_menu(shell_ref: NodeRef<leptos::html::Div>) {
    let Some(shell) = shell_ref.get_untracked() else {
        return;
    };
    let from = (
        f64::from(shell.offset_width()),
        f64::from(shell.offset_height()),
    );
    let to = measure(&shell);
    let style = web_sys::HtmlElement::style(&shell);
    let _ = style.set_property("width", &format!("{}px", from.0));
    let _ = style.set_property("height", &format!("{}px", from.1));
    let _ = shell.offset_height(); // force reflow: commit the start size
    let class_list = shell.class_list();
    let _ = class_list.remove_1("closing");
    let _ = class_list.add_1("open");
    let _ = style.set_property("width", &format!("{}px", to.0));
    let _ = style.set_property("height", &format!("{}px", to.1));
}

/// Shrink the shell back to the button face's own natural size (the button
/// face stays laid out — just absolutely positioned and invisible — while
/// open, so its `offset_width`/`offset_height` are readable without a measure
/// pass). The `+2` accounts for the shell's 1px border on each side, which
/// the button face itself (`all: unset`) doesn't include.
#[cfg(target_arch = "wasm32")]
fn close_menu(shell_ref: NodeRef<leptos::html::Div>, btn_ref: NodeRef<leptos::html::Button>) {
    let Some(shell) = shell_ref.get_untracked() else {
        return;
    };
    let class_list = shell.class_list();
    let _ = class_list.add_1("closing");
    let _ = class_list.remove_1("open");
    if let Some(btn) = btn_ref.get_untracked() {
        let w = f64::from(btn.offset_width()) + 2.0;
        let h = f64::from(btn.offset_height()) + 2.0;
        let style = web_sys::HtmlElement::style(&shell);
        let _ = style.set_property("width", &format!("{w}px"));
        let _ = style.set_property("height", &format!("{h}px"));
    }
}
