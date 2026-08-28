use leptos::prelude::*;

use crate::app::ui::{use_option_overlay, Toast, ToastKind, TOAST_MS};

#[component]
pub(crate) fn MobileToastView() -> impl IntoView {
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let (snapshot, closing, close) = use_option_overlay(toast);
    Effect::new(move |_| {
        let Some(current) = toast.get() else {
            return;
        };
        if current.kind != ToastKind::Progress {
            set_timeout(
                move || {
                    if toast
                        .get_untracked()
                        .is_some_and(|value| value.id == current.id)
                    {
                        close();
                    }
                },
                std::time::Duration::from_millis(TOAST_MS),
            );
        }
    });
    view! { {move || snapshot.get().map(|toast| view! {
        <button type="button" class="mobile-toast"
            class:error=toast.kind == ToastKind::Err class:progress=toast.kind == ToastKind::Progress
            class:closing=move || closing.get() on:click=move |_| close()>
            <span class="mobile-toast-mark">{match toast.kind { ToastKind::Ok => "✓", ToastKind::Err => "×", ToastKind::Progress => "•" }}</span>
            <span class="mobile-toast-content"><strong>{toast.title}</strong>
                {(!toast.items.is_empty()).then(|| view! { <ul>{toast.items.into_iter().map(|item| view! { <li>{item}</li> }).collect_view()}</ul> })}
                {toast.detail.map(|detail| view! { <small>{detail}</small> })}
                {toast.progress.map(|progress| view! { <span class="mobile-toast-progress"><i style:width=format!("{progress}%")></i></span> })}
            </span>
        </button>
    })} }
}
