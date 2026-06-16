pub(crate) mod confirm;
pub(crate) mod context_menu;
pub(crate) mod ns_palette;
pub(crate) mod palette;
pub(crate) mod shortcuts;

use leptos::prelude::*;

/// How long the exit animation plays before the element is removed from the DOM.
/// Must match the longest `animation-duration` in the `.closing` CSS rules.
const CLOSE_MS: u64 = 160;

/// Shared close logic for bool-gated overlays.
///
/// Returns `(visible, closing, do_close)`.  Render with `visible`; apply
/// `class:closing=closing` to the overlay element(s).  Call `do_close()`
/// instead of setting the signal directly so the exit animation plays first.
/// An internal Effect also catches external closes (e.g. the global Escape
/// handler) and plays the animation there too.
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
            // Closed externally (e.g. Escape key) — animate out.
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

/// Shared close logic for Option-gated overlays (confirm, pod modal, context menu).
///
/// Returns `(snapshot, closing, do_close)`.  Render from `snapshot` so the
/// content stays alive while the exit animation plays.  Apply
/// `class:closing=closing` to the overlay element(s).
pub(crate) fn use_option_overlay<T: Clone + Send + Sync + 'static>(
    signal: RwSignal<Option<T>>,
) -> (RwSignal<Option<T>>, RwSignal<bool>, impl Fn() + Copy) {
    let closing = RwSignal::new(false);
    let snapshot = RwSignal::new(None::<T>);

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
        let val = signal.get();
        if val.is_some() {
            snapshot.set(val);
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
