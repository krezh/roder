use leptos::html::Iframe;
use leptos::prelude::*;

use crate::app::state::ExecOpen;
use crate::data;

#[component]
pub(crate) fn ExecWindow() -> impl IntoView {
    let exec_open = expect_context::<ExecOpen>().0;
    let (snapshot, closing, do_close) = crate::app::overlays::use_option_overlay(exec_open);
    let iframe_ref = NodeRef::<Iframe>::new();

    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        if let Some(el) = iframe_ref.get() {
            let _ = el.focus();
        }
    });

    view! {
        <Show when=move || snapshot.get().is_some()>
            <div class="exec-scrim" class:closing=move || closing.get()
                on:click=move |_| do_close()></div>
            <div class="exec-window" class:closing=move || closing.get()>
                <div class="exec-head">
                    <span class="exec-title">
                        {move || snapshot.with(|t| t.as_ref().map(|t| {
                            format!("{} — {}", t.pod, if t.pending { "debug shell" } else { "shell" })
                        }))}
                    </span>
                    <button class="exec-close" on:click=move |_| do_close()>"✕"</button>
                </div>
                {move || snapshot.with(|target| target.as_ref().map(|target| {
                    let ns  = data::percent_encode(&target.namespace);
                    let pod = data::percent_encode(&target.pod);
                    let ctr = target.container.as_deref().map(data::percent_encode).unwrap_or_default();
                    let src = format!("/terminal?namespace={ns}&pod={pod}&container={ctr}");
                    if target.pending {
                        view! {
                            <div class="exec-pending">
                                <span class="exec-spinner"></span>
                                <span class="exec-pending-text">
                                    "Injecting nicolaka/netshoot"
                                    <span class="exec-dot" style="animation-delay:0s">"."</span>
                                    <span class="exec-dot" style="animation-delay:0.3s">"."</span>
                                    <span class="exec-dot" style="animation-delay:0.6s">"."</span>
                                </span>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <iframe node_ref=iframe_ref class="exec-frame" src=src allow="clipboard-read; clipboard-write"></iframe>
                        }.into_any()
                    }
                }))}
            </div>
        </Show>
    }
}
