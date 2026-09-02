use leptos::prelude::*;

use crate::app::files::TargetFileBrowser;
use crate::app::state::FileBrowserOpen;
use crate::app::ui::use_option_overlay;

#[component]
pub(crate) fn MobileFileBrowserWindow() -> impl IntoView {
    let open = expect_context::<FileBrowserOpen>().0;
    let (snapshot, closing, close) = use_option_overlay(open);

    view! {
        <Show when=move || snapshot.get().is_some()>
            <section class="mobile-files-window" class:open=move || snapshot.get().is_some() class:closing=move || closing.get() aria-label="Container files">
                <header class="mobile-files-head">
                    <button type="button" aria-label="Close file browser" on:click=move |_| close()>"<"</button>
                    <div><span>"Container files"</span><strong>{move || snapshot.get().map(|target| target.name).unwrap_or_default()}</strong></div>
                </header>
                <div class="mobile-files-body">
                    {move || snapshot.get().map(|target| view! { <TargetFileBrowser target /> })}
                </div>
            </section>
        </Show>
    }
}
