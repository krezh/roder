//! Topbar identity: current user's name, opening a morphing dropdown with
//! Access and Sign out.
//!
//! The dropdown is a single box that spring-morphs between two states rather
//! than a button plus a separately-positioned popup: closed, the box hugs the
//! button label; open, the *same* box grows to the menu's natural size. That
//! requires a small FLIP dance (see `open_menu`/`close_menu` below) since CSS
//! can't transition to/from `auto` — both natural sizes are measured up front
//! and the transition runs between explicit pixel values.

use leptos::prelude::*;

use crate::app::state::AccessReviewOpen;
use crate::data;

/// Covers the close choreography's longest path (0.06s content-leads delay +
/// the 0.34s box-shrink spring) so the cleanup below never fires early.
const CLOSE_MS: u64 = 460;

#[component]
pub(crate) fn Identity() -> impl IntoView {
    let identity = RwSignal::new(None::<serde_json::Value>);
    // Seed from the last-known identity so the name/menu doesn't flash empty
    // on refresh while the first `/api/me` round-trip is in flight.
    Effect::new(move |_| {
        if let Some(cached) = data::storage_get("roder.identity")
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            identity.set(Some(cached));
        }
    });
    let me =
        LocalResource::new(|| async { data::fetch_json::<serde_json::Value>("/api/me").await });
    Effect::new(move |_| {
        if let Some(Ok(v)) = me.get() {
            if let Ok(json) = serde_json::to_string(&v) {
                data::storage_set("roder.identity", &json);
            }
            identity.set(Some(v));
        }
    });

    let open = RwSignal::new(false);
    let closing = RwSignal::new(false);
    let shell_ref = NodeRef::<leptos::html::Div>::new();
    let btn_ref = NodeRef::<leptos::html::Button>::new();
    let access_open = expect_context::<AccessReviewOpen>().0;

    // Shared close path for outside-click, Escape, and item selection: play
    // the shrink-back animation, then clear the inline FLIP styles once it's
    // had time to finish (mirroring the demo's `transitionend` cleanup with a
    // timer, since juggling a real listener here buys nothing simpler).
    let do_close = move || {
        if closing.get_untracked() || !open.get_untracked() {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        close_menu(shell_ref, btn_ref);
        open.set(false);
        closing.set(true);
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

    let toggle = move |_: leptos::ev::MouseEvent| {
        if open.get_untracked() {
            do_close();
            return;
        }
        #[cfg(target_arch = "wasm32")]
        open_menu(shell_ref);
        closing.set(false);
        open.set(true);
    };

    Effect::new(move |_| {
        let handle = window_event_listener(leptos::ev::keydown, move |e| {
            if e.key() == "Escape" {
                do_close();
            }
        });
        on_cleanup(move || handle.remove());
    });

    view! {
        <span class="identity">
            {move || identity.get().map(|v| {
                let who = v.get("email").and_then(|e| e.as_str())
                    .or_else(|| v.get("name").and_then(|n| n.as_str()))
                    .or_else(|| v.get("subject").and_then(|s| s.as_str()))
                    .unwrap_or("anonymous").to_string();
                let who_face = who.clone();
                view! {
                    <div class="dropdown-anchor">
                        // Reserves the closed button's footprint in normal flow so the
                        // topbar row never reflows while the shell (position: absolute)
                        // morphs on top of it.
                        <span class="dropdown-spacer" aria-hidden="true">
                            <span>{who.clone()}</span><span class="dropdown-caret">"▾"</span>
                        </span>
                        <div class="dropdown-shell" node_ref=shell_ref>
                            <button class="dropdown-face-btn" node_ref=btn_ref on:click=toggle>
                                <span>{who_face}</span><span class="dropdown-caret">"▾"</span>
                            </button>
                            <div class="dropdown-face-menu">
                                <div class="dropdown-head">{who}</div>
                                <button class="dropdown-item"
                                    on:click=move |_| { do_close(); access_open.set(true); }>
                                    "Access"
                                </button>
                                <a class="dropdown-item" href="/auth/logout" rel="external">"Sign out"</a>
                            </div>
                        </div>
                        {move || open.get().then(|| view! {
                            <div class="dropdown-scrim" on:click=move |_| do_close()></div>
                        })}
                    </div>
                }
            })}
        </span>
    }
}

/// Measure the shell's natural size with a given class temporarily applied
/// (e.g. `"open"` to get the menu face's size while the button face is still
/// showing), restoring any inline size that was already set. Mirrors the
/// demo's `measure()`.
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
    let size = (f64::from(shell.offset_width()), f64::from(shell.offset_height()));
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
    let from = (f64::from(shell.offset_width()), f64::from(shell.offset_height()));
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
