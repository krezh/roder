//! Transient toast notifications (success/error), auto-dismissing after a few seconds.

use leptos::prelude::*;

const TOAST_MS: u64 = 4000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Ok,
    Err,
    Progress,
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
    pub(crate) progress: Option<usize>,
}

#[derive(Clone, Copy)]
pub(crate) struct ProgressToast(u64);

fn next_toast_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
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

    let id = next_toast_id();
    sig.set(Some(Toast {
        id,
        title: title.into(),
        items,
        detail: detail.map(Into::into),
        kind,
        progress: None,
    }));
}

/// Show a persistent progress toast. Updates through the returned handle only
/// apply while this toast is still visible, so unrelated newer toasts are not
/// overwritten by a background job tick.
pub(crate) fn show_progress_toast(
    sig: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    detail: impl Into<String>,
) -> ProgressToast {
    let id = next_toast_id();
    sig.set(Some(Toast {
        id,
        title: title.into(),
        items: Vec::new(),
        detail: Some(detail.into()),
        kind: ToastKind::Progress,
        progress: Some(0),
    }));
    ProgressToast(id)
}

pub(crate) fn update_progress_toast(
    sig: RwSignal<Option<Toast>>,
    handle: ProgressToast,
    detail: impl Into<String>,
    progress: usize,
) {
    let detail = detail.into();
    sig.update(|current| {
        let Some(current) = current.as_mut().filter(|toast| toast.id == handle.0) else {
            return;
        };
        current.detail = Some(detail);
        current.progress = Some(progress.min(100));
    });
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
        if current.kind == ToastKind::Progress {
            return;
        }
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
