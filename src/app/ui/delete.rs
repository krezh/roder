//! The delete request raised by callers, with its cascade/force options.
//! Rendered by `overlays::delete::DeleteDialog` and by the mobile dialogs.

use std::sync::Arc;

use leptos::prelude::*;
use roder_core::DeletePropagation;

#[derive(Clone)]
pub(crate) struct DeleteRequest {
    pub(crate) message: String,
    pub(crate) on_confirm: Arc<dyn Fn(bool, Option<DeletePropagation>) + Send + Sync>,
}

pub(crate) fn ask_delete(
    signal: RwSignal<Option<DeleteRequest>>,
    message: impl Into<String>,
    action: impl Fn(bool, Option<DeletePropagation>) + Send + Sync + 'static,
) {
    signal.set(Some(DeleteRequest {
        message: message.into(),
        on_confirm: Arc::new(action),
    }));
}

pub(crate) fn delete_extra(
    force: bool,
    propagation: Option<DeletePropagation>,
) -> serde_json::Value {
    serde_json::json!({ "force": force, "propagation": propagation })
}
