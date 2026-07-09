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
                            let label = if t.node_shell { "node shell" }
                                        else if t.pending { "debug shell" }
                                        else { "shell" };
                            format!("{} — {label}", t.pod)
                        }))}
                    </span>
                    <button class="exec-close" on:click=move |_| do_close()>"✕"</button>
                </div>
                {move || snapshot.with(|target| target.as_ref().map(|target| {
                    let ns  = data::percent_encode(&target.namespace);
                    let pod = data::percent_encode(&target.pod);
                    let ctr = target.container.as_deref().map(data::percent_encode).unwrap_or_default();
                    let node_shell = target.node_shell;
                    let src = format!("/terminal?namespace={ns}&pod={pod}&container={ctr}&node_shell={node_shell}");
                    if target.pending {
                        let pending_text = if target.node_shell {
                            "Creating privileged debug pod"
                        } else {
                            "Injecting nicolaka/netshoot"
                        };
                        view! {
                            <div class="exec-pending">
                                <span class="exec-spinner"></span>
                                <span class="exec-pending-text">
                                    {pending_text}
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
