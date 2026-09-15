//! The confirmation request a caller raises, and the shape of its buttons.
//! Rendered by `overlays::confirm::ConfirmDialog` on desktop and
//! `mobile::dialogs` on mobile, so it lives here rather than in either.

use std::sync::Arc;

use leptos::prelude::*;

#[derive(Clone)]
pub(crate) struct ConfirmButton {
    pub(crate) label: String,
    pub(crate) on_click: Arc<dyn Fn() + Send + Sync>,
}

impl ConfirmButton {
    pub(crate) fn new(
        label: impl Into<String>,
        on_click: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on_click: Arc::new(on_click),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Confirm {
    pub(crate) message: String,
    pub(crate) buttons: Vec<ConfirmButton>,
}

pub(crate) fn ask_confirm(
    signal: RwSignal<Option<Confirm>>,
    message: impl Into<String>,
    label: impl Into<String>,
    action: impl Fn() + Send + Sync + 'static,
) {
    signal.set(Some(Confirm {
        message: message.into(),
        buttons: vec![ConfirmButton::new(label, action)],
    }));
}
