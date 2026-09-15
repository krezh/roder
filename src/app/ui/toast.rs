//! Toast state and the helpers that raise one. Both shells render these:
//! `overlays::toast::ToastView` on desktop, `mobile::toast` on mobile.

use leptos::prelude::*;

/// How long a toast stays on screen before it starts animating out.
pub(crate) const TOAST_MS: u64 = 4000;
/// Cap on the lines a list toast renders; the rest collapse into a count.
const MAX_TOAST_ITEMS: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Ok,
    Err,
    Progress,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Toast {
    pub(crate) id: u64,
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

pub(crate) fn show_toast(
    signal: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    kind: ToastKind,
) {
    show_toast_full(signal, title, Vec::new(), None::<String>, kind);
}

pub(crate) fn show_toast_detail(
    signal: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    detail: Option<impl Into<String>>,
    kind: ToastKind,
) {
    show_toast_full(signal, title, Vec::new(), detail, kind);
}

pub(crate) fn show_toast_list(
    signal: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    items: Vec<String>,
    kind: ToastKind,
) {
    show_toast_full(signal, title, items, None::<String>, kind);
}

pub(crate) fn show_toast_full(
    signal: RwSignal<Option<Toast>>,
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
    signal.set(Some(Toast {
        id: next_toast_id(),
        title: title.into(),
        items,
        detail: detail.map(Into::into),
        kind,
        progress: None,
    }));
}

pub(crate) fn show_progress_toast(
    signal: RwSignal<Option<Toast>>,
    title: impl Into<String>,
    detail: impl Into<String>,
) -> ProgressToast {
    let id = next_toast_id();
    signal.set(Some(Toast {
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
    signal: RwSignal<Option<Toast>>,
    handle: ProgressToast,
    detail: impl Into<String>,
    progress: usize,
) {
    let detail = detail.into();
    signal.update(|current| {
        if let Some(current) = current.as_mut().filter(|toast| toast.id == handle.0) {
            current.detail = Some(detail);
            current.progress = Some(progress.min(100));
        }
    });
}
