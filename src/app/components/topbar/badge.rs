//! Shared "closing" animation helper for topbar badges that appear and
//! disappear reactively (failing-pod count, Flux failure count).

use leptos::prelude::*;

const FAILURE_BADGE_ANIMATION_MS: u64 = 440;

pub(crate) fn use_animated_badge<T: Clone + Send + Sync + 'static>(
    value: Memo<Option<T>>,
) -> (RwSignal<Option<T>>, RwSignal<bool>) {
    let snapshot = RwSignal::new(None::<T>);
    let closing = RwSignal::new(false);
    let generation = RwSignal::new(0u64);

    Effect::new(move |_| match value.get() {
        Some(value) => {
            generation.update(|n| *n += 1);
            snapshot.set(Some(value));
            closing.set(false);
        }
        None if snapshot.get_untracked().is_some() && !closing.get_untracked() => {
            generation.update(|n| *n += 1);
            let current = generation.get_untracked();
            closing.set(true);
            set_timeout(
                move || {
                    let still_closing = closing.try_get_untracked().unwrap_or(false);
                    let same_generation = generation
                        .try_get_untracked()
                        .is_some_and(|generation| generation == current);
                    if still_closing && same_generation {
                        let _ = snapshot.try_set(None);
                        let _ = closing.try_set(false);
                    }
                },
                std::time::Duration::from_millis(FAILURE_BADGE_ANIMATION_MS),
            );
        }
        None => {}
    });

    (snapshot, closing)
}
