//! In-page confirmation dialog (replaces the browser `confirm()`).

use leptos::prelude::*;

/// An in-page confirmation request. The action is a plain `Arc`'d closure so it
/// survives even if the widget that requested it (e.g. the context menu) is
/// unmounted before the user confirms.
#[derive(Clone)]
pub(crate) struct Confirm {
    pub(crate) message: String,
    pub(crate) on_ok: std::sync::Arc<dyn Fn() + Send + Sync>,
}

/// Pop an in-page confirmation; runs `on_ok` if the user confirms.
pub(crate) fn ask_confirm(
    sig: RwSignal<Option<Confirm>>,
    message: impl Into<String>,
    on_ok: impl Fn() + Send + Sync + 'static,
) {
    sig.set(Some(Confirm {
        message: message.into(),
        on_ok: std::sync::Arc::new(on_ok),
    }));
}

#[component]
pub(crate) fn ConfirmDialog() -> impl IntoView {
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    view! {
        {move || confirm.get().map(|c| {
            let on_ok = c.on_ok.clone();
            view! {
                <div class="modal-scrim" on:click=move |_| confirm.set(None)></div>
                <div class="modal">
                    <div class="modal-msg">{c.message.clone()}</div>
                    <div class="modal-actions">
                        <button class="act" on:click=move |_| confirm.set(None)>"Cancel"</button>
                        <button class="act danger" on:click=move |_| { on_ok(); confirm.set(None); }>"Delete"</button>
                    </div>
                </div>
            }
        })}
    }
}
