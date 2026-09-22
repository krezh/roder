//! Transient toast notifications (success/error/progress), stacking above each other
//! and auto-dismissing after a few seconds.

use leptos::prelude::*;

use crate::app::ui::toast::{Toast, ToastKind, Toasts, TOAST_MS};

#[component]
pub(crate) fn ToastView() -> impl IntoView {
    let toasts = expect_context::<Toasts>();

    view! {
        <div class="toast-container" aria-live="polite" role="region" aria-label="Notifications">
            <For
                each=move || toasts.0.get()
                key=|t| t.id
                let:toast
            >
                <ToastItem toast=toast toasts=toasts />
            </For>
        </div>
    }
}

#[component]
fn ToastItem(toast: Toast, toasts: Toasts) -> impl IntoView {
    let id = toast.id;
    let kind = toast.kind;
    let title = toast.title;
    let items = toast.items;

    let closing = RwSignal::new(false);
    let hovered = RwSignal::new(false);
    let should_close = RwSignal::new(false);

    let do_close = move || {
        if closing.get_untracked() {
            return;
        }
        closing.set(true);
        set_timeout(
            move || {
                toasts.dismiss(id);
            },
            std::time::Duration::from_millis(160),
        );
    };

    // Auto-dismiss each non-progress toast after TOAST_MS, paused while hovered.
    if kind != ToastKind::Progress {
        set_timeout(
            move || {
                should_close.set(true);
            },
            std::time::Duration::from_millis(TOAST_MS),
        );

        Effect::new(move |_| {
            if should_close.get() && !hovered.get() {
                do_close();
            }
        });
    }

    // Detail and progress may be updated dynamically (e.g. for progress toasts).
    let detail_val = move || {
        toasts.0.with(|list| {
            list.iter()
                .find(|t| t.id == id)
                .and_then(|t| t.detail.clone())
        })
    };
    let progress_val = move || {
        toasts
            .0
            .with(|list| list.iter().find(|t| t.id == id).and_then(|t| t.progress))
    };

    view! {
        <div
            class="toast"
            class:toast-err=kind == ToastKind::Err
            class:toast-progress-active=kind == ToastKind::Progress
            class:closing=move || closing.get()
            on:pointerenter=move |_| hovered.set(true)
            on:pointerleave=move |_| hovered.set(false)
            on:click=move |_| do_close()
        >
            <span class="toast-icon">{match kind {
                ToastKind::Ok => "\u{2713}",
                ToastKind::Err => "\u{2715}",
                ToastKind::Progress => "\u{2022}",
            }}</span>
            <span class="toast-body">
                <span class="toast-title">{title}</span>
                {(!items.is_empty()).then(|| {
                    view! {
                        <ul class="toast-items">
                            {items.into_iter().map(|item| view! { <li>{item}</li> }).collect_view()}
                        </ul>
                    }
                })}
                {move || detail_val().map(|d| view! { <span class="toast-detail">{d}</span> })}
                {move || progress_val().map(|progress| view! {
                    <span class="toast-progress">
                        <span class="toast-progress-fill"
                            style:width=format!("{progress}%")></span>
                    </span>
                })}
            </span>
        </div>
    }
}
