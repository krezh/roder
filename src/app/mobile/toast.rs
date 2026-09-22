use leptos::prelude::*;

use crate::app::ui::toast::{Toast, ToastKind, Toasts, TOAST_MS};

#[component]
pub(crate) fn MobileToastView() -> impl IntoView {
    let toasts = expect_context::<Toasts>();

    view! {
        <div class="mobile-toast-container" aria-live="polite" role="region" aria-label="Notifications">
            <For
                each=move || toasts.0.get()
                key=|t| t.id
                let:toast
            >
                <MobileToastItem toast=toast toasts=toasts />
            </For>
        </div>
    }
}

#[component]
fn MobileToastItem(toast: Toast, toasts: Toasts) -> impl IntoView {
    let id = toast.id;
    let kind = toast.kind;
    let title = toast.title;
    let items = toast.items;

    let closing = RwSignal::new(false);

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

    if kind != ToastKind::Progress {
        set_timeout(
            move || {
                do_close();
            },
            std::time::Duration::from_millis(TOAST_MS),
        );
    }

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
        <button
            type="button"
            class="mobile-toast"
            class:error=kind == ToastKind::Err
            class:progress=kind == ToastKind::Progress
            class:closing=move || closing.get()
            on:click=move |_| do_close()
        >
            <span class="mobile-toast-mark">{match kind {
                ToastKind::Ok => "✓",
                ToastKind::Err => "×",
                ToastKind::Progress => "•",
            }}</span>
            <span class="mobile-toast-content">
                <strong>{title}</strong>
                {(!items.is_empty()).then(|| view! {
                    <ul>{items.into_iter().map(|item| view! { <li>{item}</li> }).collect_view()}</ul>
                })}
                {move || detail_val().map(|detail| view! { <small>{detail}</small> })}
                {move || progress_val().map(|progress| view! {
                    <span class="mobile-toast-progress"><i style:width=format!("{progress}%")></i></span>
                })}
            </span>
        </button>
    }
}
