//! The open/close lifecycle shared by every overlay, and the focus trap that
//! keeps keyboard navigation inside an open dialog.
//!
//! Overlays animate out rather than vanishing, so closing is two-phase: the
//! hooks hand back a snapshot that outlives the trigger signal plus a
//! `closing` flag, and only drop the snapshot once the transition is done.

use leptos::prelude::*;

/// How long an overlay spends animating out before its snapshot is dropped.
/// Must stay in step with the transition durations in `style/_modals.scss`.
const CLOSE_MS: u64 = 160;

pub(crate) fn use_bool_overlay(
    open: RwSignal<bool>,
) -> (RwSignal<bool>, RwSignal<bool>, impl Fn() + Copy) {
    let visible = RwSignal::new(false);
    let closing = RwSignal::new(false);
    let do_close = move || {
        if !closing.get_untracked() {
            closing.set(true);
            open.set(false);
            set_timeout(
                move || {
                    if closing.get_untracked() {
                        visible.set(false);
                        closing.set(false);
                    }
                },
                std::time::Duration::from_millis(CLOSE_MS),
            );
        }
    };
    Effect::new(move |_| {
        if open.get() {
            visible.set(true);
            closing.set(false);
        } else if visible.get_untracked() && !closing.get_untracked() {
            closing.set(true);
            set_timeout(
                move || {
                    if closing.get_untracked() {
                        visible.set(false);
                        closing.set(false);
                    }
                },
                std::time::Duration::from_millis(CLOSE_MS),
            );
        }
    });
    (visible, closing, do_close)
}

pub(crate) fn use_option_overlay<T: Clone + Send + Sync + 'static>(
    signal: RwSignal<Option<T>>,
) -> (RwSignal<Option<T>>, RwSignal<bool>, impl Fn() + Copy) {
    let snapshot = RwSignal::new(None::<T>);
    let closing = RwSignal::new(false);
    let do_close = move || {
        if !closing.get_untracked() {
            closing.set(true);
            signal.set(None);
            set_timeout(
                move || {
                    if closing.get_untracked() {
                        snapshot.set(None);
                        closing.set(false);
                    }
                },
                std::time::Duration::from_millis(CLOSE_MS),
            );
        }
    };
    Effect::new(move |_| {
        let value = signal.get();
        if value.is_some() {
            snapshot.set(value);
            closing.set(false);
        } else if snapshot.get_untracked().is_some() && !closing.get_untracked() {
            closing.set(true);
            set_timeout(
                move || {
                    if closing.get_untracked() {
                        snapshot.set(None);
                        closing.set(false);
                    }
                },
                std::time::Duration::from_millis(CLOSE_MS),
            );
        }
    });
    (snapshot, closing, do_close)
}

pub(crate) fn use_dialog_focus(dialog_ref: NodeRef<leptos::html::Div>) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;

        Effect::new(move |_| {
            let Some(dialog) = dialog_ref.get() else {
                return;
            };
            let items = dialog_focusables(&dialog);
            if let Some(first) = items.first() {
                let _ = first.focus();
            } else {
                let _ = dialog.focus();
            }
        });

        Effect::new(move |_| {
            let handle = window_event_listener(leptos::ev::keydown, move |event| {
                if event.key() != "Tab" {
                    return;
                }
                let Some(dialog) = dialog_ref.get_untracked() else {
                    return;
                };
                let items = dialog_focusables(&dialog);
                if items.is_empty() {
                    event.prevent_default();
                    let _ = dialog.focus();
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
                let next = match (current, event.shift_key()) {
                    (Some(0), true) | (None, true) => items.len() - 1,
                    (Some(index), true) => index - 1,
                    (Some(index), false) if index + 1 < items.len() => index + 1,
                    _ => 0,
                };
                event.prevent_default();
                let _ = items[next].focus();
            });
            on_cleanup(move || handle.remove());
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = dialog_ref;
}

#[cfg(target_arch = "wasm32")]
fn dialog_focusables(dialog: &web_sys::HtmlDivElement) -> Vec<web_sys::HtmlElement> {
    use wasm_bindgen::JsCast;

    let Ok(nodes) = dialog.query_selector_all(
        "button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
    ) else {
        return Vec::new();
    };
    (0..nodes.length())
        .filter_map(|index| nodes.item(index))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .filter(|element| element.offset_width() > 0 || element.offset_height() > 0)
        .collect()
}
