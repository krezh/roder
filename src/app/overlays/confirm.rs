//! In-page confirmation dialog (replaces the browser `confirm()`).

use leptos::prelude::*;

/// One action button in a [`Confirm`] dialog, beyond the always-present Cancel.
#[derive(Clone)]
pub(crate) struct ConfirmButton {
    pub(crate) label: String,
    pub(crate) on_click: std::sync::Arc<dyn Fn() + Send + Sync>,
}

impl ConfirmButton {
    pub(crate) fn new(
        label: impl Into<String>,
        on_click: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on_click: std::sync::Arc::new(on_click),
        }
    }
}

/// An in-page confirmation request. Buttons are plain `Arc`'d closures so they
/// survive even if the widget that requested them (e.g. the context menu) is
/// unmounted before the user responds. Cancel is always rendered alongside
/// `buttons` and needs no entry of its own.
#[derive(Clone)]
pub(crate) struct Confirm {
    pub(crate) message: String,
    pub(crate) buttons: Vec<ConfirmButton>,
}

/// Pop an in-page confirmation with a single labeled action button (plus the
/// implicit Cancel). For more than one action button, build a [`Confirm`]
/// directly with as many [`ConfirmButton`]s as needed.
pub(crate) fn ask_confirm(
    sig: RwSignal<Option<Confirm>>,
    message: impl Into<String>,
    ok_label: impl Into<String>,
    on_ok: impl Fn() + Send + Sync + 'static,
) {
    sig.set(Some(Confirm {
        message: message.into(),
        buttons: vec![ConfirmButton::new(ok_label, on_ok)],
    }));
}

#[component]
pub(crate) fn ConfirmDialog() -> impl IntoView {
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let (snapshot, closing, do_close) = super::use_option_overlay(confirm);

    view! {
        {move || snapshot.get().map(|c| {
            view! {
                <div class="modal-scrim" class:closing=move || closing.get()
                    on:click=move |_| do_close()></div>
                <div class="modal" class:closing=move || closing.get()>
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
