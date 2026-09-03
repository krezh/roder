//! In-page confirmation dialog (replaces the browser `confirm()`).

use leptos::prelude::*;

pub(crate) use crate::app::ui::{ask_confirm, Confirm};

#[component]
pub(crate) fn ConfirmDialog() -> impl IntoView {
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let (snapshot, closing, do_close) = super::use_option_overlay(confirm);
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    crate::app::ui::use_dialog_focus(dialog_ref);

    view! {
        {move || snapshot.get().map(|c| {
            view! {
                <div class="modal-scrim" class:closing=move || closing.get()
                    on:click=move |_| do_close()></div>
                <div class="modal" class:closing=move || closing.get() node_ref=dialog_ref
                    role="alertdialog" aria-modal="true" tabindex="-1">
                    <div class="modal-msg">{c.message.clone()}</div>
                    <div class="modal-actions">
                        <button class="act" on:click=move |_| do_close()>"Cancel"</button>
                        {c.buttons.iter().cloned().map(|b| {
                            view! {
                                <button class="act danger" disabled=move || closing.get() on:click=move |_| {
                                    if !closing.get_untracked() {
                                        do_close();
                                        (b.on_click)();
                                    }
                                }>
                                    {b.label.clone()}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </div>
            }
        })}
    }
}
