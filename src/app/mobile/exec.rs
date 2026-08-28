use leptos::html::Iframe;
use leptos::prelude::*;

use crate::app::state::ExecOpen;
use crate::app::ui::use_option_overlay;
use crate::data;

#[component]
pub(crate) fn MobileExecWindow() -> impl IntoView {
    let signal = expect_context::<ExecOpen>().0;
    let (snapshot, closing, close) = use_option_overlay(signal);
    let frame = NodeRef::<Iframe>::new();
    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        if let Some(frame) = frame.get() {
            let _ = frame.focus();
        }
    });
    view! { <Show when=move || snapshot.get().is_some()>
        <section class="mobile-exec" class:closing=move || closing.get()>
            <header class="mobile-exec-head">
                <button type="button" aria-label="Close shell" on:click=move |_| close()>"‹"</button>
                <strong>{move || snapshot.with(|target| target.as_ref().map(|target| {
                    let label = if target.node_shell { "node shell" } else if target.pending { "debug shell" } else { "shell" };
                    format!("{} · {label}", target.pod)
                }))}</strong>
            </header>
            {move || snapshot.with(|target| target.as_ref().map(|target| {
                if target.pending {
                    let image = target.image.rsplit_once("@sha256:").map(|(repo, _)| repo).unwrap_or(&target.image);
                    let message = if target.node_shell { format!("Creating node shell ({image})") } else { format!("Injecting {image}") };
                    view! { <div class="mobile-exec-pending"><span></span><p>{message}</p></div> }.into_any()
                } else {
                    let namespace = data::percent_encode(&target.namespace);
                    let pod = data::percent_encode(&target.pod);
                    let container = target.container.as_deref().map(data::percent_encode).unwrap_or_default();
                    let src = format!("/terminal?namespace={namespace}&pod={pod}&container={container}&node_shell={}", target.node_shell);
                    view! { <iframe node_ref=frame class="mobile-exec-frame" src=src allow="clipboard-read; clipboard-write"></iframe> }.into_any()
                }
            }))}
        </section>
    </Show> }
}
