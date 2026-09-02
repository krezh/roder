use leptos::prelude::*;

use crate::app::files::TargetFileBrowser;
use crate::app::state::FileBrowserOpen;

#[component]
pub(crate) fn FileBrowserWindow() -> impl IntoView {
    let open = expect_context::<FileBrowserOpen>().0;
    let (snapshot, closing, close) = crate::app::overlays::use_option_overlay(open);

    view! {
        <Show when=move || snapshot.get().is_some()>
            <div class="files-scrim" class:closing=move || closing.get() on:click=move |_| close()></div>
            <section class="files-window" class:closing=move || closing.get() aria-label="Container files">
                <header class="files-window-head">
                    <div><span>"Container files"</span><strong>{move || snapshot.get().map(|target| target.name).unwrap_or_default()}</strong></div>
                    <button type="button" aria-label="Close file browser" on:click=move |_| close()>"x"</button>
                </header>
                <div class="files-window-body">
                    {move || snapshot.get().map(|target| view! { <TargetFileBrowser target /> })}
                </div>
            </section>
        </Show>
    }
}
