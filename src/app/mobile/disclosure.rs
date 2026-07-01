//! Wraps an existing component so a tap reveals its embedded `.tooltip`
//! content — the mobile equivalent of the desktop's `:hover`-only tooltips,
//! which never fire on touch. Doesn't know or care what's inside; it just
//! toggles a class that mobile CSS uses to force-show any nested `.tooltip`.

use leptos::prelude::*;

#[component]
pub(crate) fn TapReveal(children: Children) -> impl IntoView {
    let open = RwSignal::new(false);
    view! {
        <div class="tap-reveal" class:open=move || open.get()
            on:click=move |_| open.update(|o| *o = !*o)>
            {children()}
        </div>
    }
}
