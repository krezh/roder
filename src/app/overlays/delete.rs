//! Delete confirmation dialog with cascade-propagation and force options
//! (`ConfirmDialog`'s cousin — a plain label/callback doesn't leave room for
//! these, so delete gets its own small dialog instead of overloading `Confirm`).

use std::sync::Arc;

use leptos::prelude::*;
use roder_core::DeletePropagation;

use crate::app::components::dropdown::{Dropdown, DropdownClose};

/// A pending delete confirmation. `on_confirm` is a plain `Arc`'d closure
/// (mirrors `ConfirmButton`) so it survives even if the widget that requested
/// it is unmounted before the user responds.
#[derive(Clone)]
pub(crate) struct DeleteRequest {
    pub(crate) message: String,
    pub(crate) on_confirm: Arc<dyn Fn(bool, Option<DeletePropagation>) + Send + Sync>,
}

/// Pop the delete confirmation dialog.
pub(crate) fn ask_delete(
    sig: RwSignal<Option<DeleteRequest>>,
    message: impl Into<String>,
    on_confirm: impl Fn(bool, Option<DeletePropagation>) + Send + Sync + 'static,
) {
    sig.set(Some(DeleteRequest {
        message: message.into(),
        on_confirm: Arc::new(on_confirm),
    }));
}

/// Merge `force`/`propagation` into the `/api/action` request body, same
/// shape as the `extra` argument to `fire_action_with`/`run`.
pub(crate) fn delete_extra(
    force: bool,
    propagation: Option<DeletePropagation>,
) -> serde_json::Value {
    serde_json::json!({ "force": force, "propagation": propagation })
}

#[component]
pub(crate) fn DeleteDialog() -> impl IntoView {
    let del = expect_context::<RwSignal<Option<DeleteRequest>>>();
    let (snapshot, closing, do_close) = super::use_option_overlay(del);

    view! {
        {move || snapshot.get().map(|req| view! {
            <DeleteDialogView req=req closing=closing do_close=do_close />
        })}
    }
}

#[component]
fn DeleteDialogView(
    req: DeleteRequest,
    closing: RwSignal<bool>,
    do_close: impl Fn() + Copy + Send + 'static,
) -> impl IntoView {
    // Fresh per open (this whole component is re-instantiated every time
    // `snapshot` picks up a new request — see `DrainOpenView`), so a stale
    // Force/Propagation choice never leaks into the next delete dialog.
    let force = RwSignal::new(false);
    let propagation = RwSignal::new(String::new()); // "" = server default

    let propagation_label = move || {
        match propagation.get().as_str() {
            "Orphan" => "Orphan",
            "Background" => "Background",
            "Foreground" => "Foreground",
            _ => "Default",
        }
        .to_string()
    };

    let confirm = move |_: leptos::ev::MouseEvent| {
        if closing.get_untracked() {
            return;
        }
        let f = force.get_untracked();
        let p = match propagation.get_untracked().as_str() {
            "Orphan" => Some(DeletePropagation::Orphan),
            "Background" => Some(DeletePropagation::Background),
            "Foreground" => Some(DeletePropagation::Foreground),
            _ => None,
        };
        let on_confirm = req.on_confirm.clone();
        do_close();
        on_confirm(f, p);
    };

    view! {
        <div class="modal-scrim" class:closing=move || closing.get()
            on:click=move |_| do_close()></div>
        <div class="modal delete-modal" class:closing=move || closing.get()>
            <div class="modal-msg">{req.message.clone()}</div>
            <div class="delete-options">
                <div class="delete-opt">
                    <span>"Propagation"</span>
                    <Dropdown label=propagation_label>
                        <PropagationItem propagation=propagation value="" item_label="Default" />
                        <PropagationItem propagation=propagation value="Foreground" item_label="Foreground" />
                        <PropagationItem propagation=propagation value="Background" item_label="Background" />
                        <PropagationItem propagation=propagation value="Orphan" item_label="Orphan" />
                    </Dropdown>
                </div>
                <label class="opt-row">
                    <input type="checkbox" class="check check-static" prop:checked=move || force.get()
                        on:change=move |e| force.set(event_target_checked(&e)) />
                    <span>"Force"</span>
                    <span class="hint">"Delete immediately, bypassing graceful pod termination."</span>
                </label>
            </div>
            <div class="modal-actions">
                <button class="act" on:click=move |_| do_close()>"Cancel"</button>
                <button class="act danger" disabled=move || closing.get() on:click=confirm>
                    "Delete"
                </button>
            </div>
        </div>
    }
}

/// Its own component (rather than inlined into `DeleteDialogView`'s view) so
/// it can pull `DropdownClose` from context — see `AccessItem` in
/// `topbar::identity` for why that needs to be a real child component.
#[component]
fn PropagationItem(
    propagation: RwSignal<String>,
    value: &'static str,
    item_label: &'static str,
) -> impl IntoView {
    let close = expect_context::<DropdownClose>().0;
    view! {
        <button type="button" class="dropdown-item"
            on:click=move |_| { propagation.set(value.to_string()); close.run(()); }>
            {item_label}
        </button>
    }
}
