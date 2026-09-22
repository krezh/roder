//! Toast state and the helpers that raise one. Both shells render these:
//! `overlays::toast::ToastView` on desktop, `mobile::toast` on mobile.

use leptos::prelude::*;

/// How long a toast stays on screen before it starts animating out.
pub(crate) const TOAST_MS: u64 = 4000;
/// Maximum number of toasts visible simultaneously in the stack.
pub(crate) const MAX_ACTIVE_TOASTS: usize = 5;
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
pub(crate) struct ProgressToast(pub(crate) u64);

impl From<ProgressToast> for u64 {
    fn from(p: ProgressToast) -> Self {
        p.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Toasts(pub(crate) RwSignal<Vec<Toast>>);

impl Toasts {
    pub(crate) fn new() -> Self {
        Self(RwSignal::new(Vec::new()))
    }

    pub(crate) fn push(&self, toast: Toast) {
        self.0.update(|list| {
            if list.len() >= MAX_ACTIVE_TOASTS {
                if let Some(idx) = list.iter().position(|t| t.kind != ToastKind::Progress) {
                    list.remove(idx);
                } else if !list.is_empty() {
                    list.remove(0);
                }
            }
            list.push(toast);
        });
    }

    pub(crate) fn dismiss(&self, id: u64) {
        self.0.update(|list| list.retain(|t| t.id != id));
    }
}

impl Default for Toasts {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Deref for Toasts {
    type Target = RwSignal<Vec<Toast>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn next_toast_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn show_toast(toasts: Toasts, title: impl Into<String>, kind: ToastKind) {
    show_toast_full(toasts, title, Vec::new(), None::<String>, kind);
}

pub(crate) fn show_toast_detail(
    toasts: Toasts,
    title: impl Into<String>,
    detail: Option<impl Into<String>>,
    kind: ToastKind,
) {
    show_toast_full(toasts, title, Vec::new(), detail, kind);
}

pub(crate) fn show_toast_list(
    toasts: Toasts,
    title: impl Into<String>,
    items: Vec<String>,
    kind: ToastKind,
) {
    show_toast_full(toasts, title, items, None::<String>, kind);
}

pub(crate) fn show_toast_full(
    toasts: Toasts,
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
    toasts.push(Toast {
        id: next_toast_id(),
        title: title.into(),
        items,
        detail: detail.map(Into::into),
        kind,
        progress: None,
    });
}

pub(crate) fn show_progress_toast(
    toasts: Toasts,
    title: impl Into<String>,
    detail: impl Into<String>,
) -> ProgressToast {
    let id = next_toast_id();
    toasts.push(Toast {
        id,
        title: title.into(),
        items: Vec::new(),
        detail: Some(detail.into()),
        kind: ToastKind::Progress,
        progress: Some(0),
    });
    ProgressToast(id)
}

pub(crate) fn update_progress_toast(
    toasts: Toasts,
    handle: ProgressToast,
    detail: impl Into<String>,
    progress: usize,
) {
    let detail = detail.into();
    toasts.0.update(|current| {
        if let Some(toast) = current.iter_mut().find(|toast| toast.id == handle.0) {
            toast.detail = Some(detail);
            toast.progress = Some(progress.min(100));
        }
    });
}

pub(crate) fn dismiss_toast(toasts: Toasts, target: impl Into<u64>) {
    toasts.dismiss(target.into());
}
