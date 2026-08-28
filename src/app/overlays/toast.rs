//! Transient toast notifications (success/error), auto-dismissing after a few seconds.

use leptos::prelude::*;

pub(crate) use crate::app::ui::{
    show_progress_toast, show_toast, show_toast_detail, show_toast_full, show_toast_list,
    update_progress_toast, Toast, ToastKind,
};

#[component]
pub(crate) fn ToastView() -> impl IntoView {
    let toast = expect_context::<RwSignal<Option<Toast>>>();
    let (snapshot, closing, do_close) = super::use_option_overlay(toast);

    // Auto-dismiss each toast a few seconds after it lands, unless a newer one
    // has already replaced it (compared by id, since two toasts can share text).
    Effect::new(move |_| {
        let Some(current) = toast.get() else {
            return;
        };
        if current.kind == ToastKind::Progress {
            return;
        }
        set_timeout(
            move || {
                if toast.get_untracked().is_some_and(|t| t.id == current.id) {
                    do_close();
                }
            },
            std::time::Duration::from_millis(crate::app::ui::TOAST_MS),
        );
    });

    view! {
        {move || snapshot.get().map(|t| {
            view! {
                <div class="toast" class:toast-err=move || t.kind == ToastKind::Err
                    class:toast-progress-active=move || t.kind == ToastKind::Progress
                    class:closing=move || closing.get()
                    on:click=move |_| do_close()>
                    <span class="toast-icon">{match t.kind {
                        ToastKind::Ok => "\u{2713}",
                        ToastKind::Err => "\u{2715}",
                        ToastKind::Progress => "\u{2022}",
                    }}</span>
                    <span class="toast-body">
                        <span class="toast-title">{t.title.clone()}</span>
                        {(!t.items.is_empty()).then(|| {
                            let items = t.items.clone();
                            view! {
                                <ul class="toast-items">
                                    {items.into_iter().map(|item| view! { <li>{item}</li> }).collect_view()}
                                </ul>
                            }
                        })}
                        {t.detail.clone().map(|d| view! { <span class="toast-detail">{d}</span> })}
                        {t.progress.map(|progress| view! {
                            <span class="toast-progress">
                                <span class="toast-progress-fill"
                                    style:width=format!("{progress}%")></span>
                            </span>
                        })}
                    </span>
                </div>
            }
        })}
    }
}
