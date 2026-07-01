//! Transient toast notifications (success/error), auto-dismissing after a few seconds.

use leptos::prelude::*;

const TOAST_MS: u64 = 4000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Ok,
    Err,
}

/// Cap on how many names a toast will list individually; beyond this a "+N more"
/// row is appended instead of letting the toast grow unbounded.
const MAX_TOAST_ITEMS: usize = 6;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Toast {
    id: u64,
    pub(crate) title: String,
    pub(crate) items: Vec<String>,
    pub(crate) detail: Option<String>,
    pub(crate) kind: ToastKind,
}

/// Show a toast; auto-dismisses after a few seconds, or is replaced by a newer one.
pub(crate) fn show_toast(sig: RwSignal<Option<Toast>>, title: impl Into<String>, kind: ToastKind) {
    show_toast_full(sig, title, Vec::new(), None::<String>, kind);
}

/// Show a toast with a secondary detail line (e.g. an error message), rendered
/// smaller and below the title instead of run together in one sentence.
pub(crate) fn show_toast_detail(
    sig: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    detail: Option<impl Into<String>>,
    kind: ToastKind,
) {
    show_toast_full(sig, title, Vec::new(), detail, kind);
}

/// Show a toast whose title refers to several items; the items are rendered as
/// a proper list (one per line, capped and summarized past [`MAX_TOAST_ITEMS`])
/// instead of a comma-joined run-on sentence.
pub(crate) fn show_toast_list(
    sig: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    items: Vec<String>,
    kind: ToastKind,
) {
    show_toast_full(sig, title, items, None::<String>, kind);
}

/// Show a toast combining a list of items and a detail line (e.g. failed names
/// alongside the error that caused the failure).
pub(crate) fn show_toast_full(
    sig: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    mut items: Vec<String>,
    detail: Option<impl Into<String>>,
    kind: ToastKind,
) {
    if items.len() > MAX_TOAST_ITEMS {
        let rest = items.len() - (MAX_TOAST_ITEMS - 1);
        items.truncate(MAX_TOAST_ITEMS - 1);
        items.push(format!("+{rest} more"));
    }

    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    sig.set(Some(Toast {
        id,
        title: title.into(),
        items,
        detail: detail.map(Into::into),
        kind,
    }));
}

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
        set_timeout(
            move || {
                if toast.get_untracked().is_some_and(|t| t.id == current.id) {
                    do_close();
                }
            },
            std::time::Duration::from_millis(TOAST_MS),
        );
    });

    view! {
        {move || snapshot.get().map(|t| {
            view! {
                <div class="toast" class:toast-err=move || t.kind == ToastKind::Err
                    class:closing=move || closing.get()
                    on:click=move |_| do_close()>
                    <span class="toast-icon">{if t.kind == ToastKind::Err { "\u{2715}" } else { "\u{2713}" }}</span>
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
                    </span>
                </div>
            }
        })}
    }
}
