//! Full-screen mobile replacement for the desktop's `LogSidebar`: one log
//! pane at a time (a chip switcher when more than one source is open)
//! instead of side-by-side panes, since those don't fit a phone's width.

use leptos::prelude::*;

use crate::app::log_stream::{extract_timestamp, use_log_stream};
use crate::app::state::LogPods;
use crate::app::util::color::hue_of;
use crate::app::util::format::{ansi_to_html, log_level, parse_log_line};

#[component]
pub(crate) fn MobileLogsView() -> impl IntoView {
    let log_pods = expect_context::<LogPods>().0;
    let active = RwSignal::new(0usize);
    Effect::new(move |_| {
        let n = log_pods.with(|v| v.len());
        if n > 0 && active.get_untracked() >= n {
            active.set(n - 1);
        }
    });

    view! {
        <div class="mobile-logs" class:open=move || !log_pods.get().is_empty()>
            <div class="mobile-logs-head">
                <button class="mobile-logs-close-all" on:click=move |_| log_pods.set(Vec::new())>
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m15 18-6-6 6-6" /></svg>
                    <span>"Close logs"</span>
                </button>
            </div>
            {move || {
                let pods = log_pods.get();
                (pods.len() > 1).then(|| view! {
                    <div class="mobile-pane-switcher">
                        {pods.iter().enumerate().map(|(i, t)| {
                            let title = if t.aggregate { format!("{} (all)", t.name) } else { t.name.clone() };
                            view! {
                                <span class="mobile-pane-chip" class:active=move || active.get() == i
                                    on:click=move |_| active.set(i)>{title}</span>
                            }
                        }).collect_view()}
                    </div>
                })
            }}
            <div class="mobile-logs-body">
                {move || {
                    let pods = log_pods.get();
                    if pods.is_empty() {
                        return None;
                    }
                    let i = active.get().min(pods.len() - 1);
                    pods.get(i).cloned().map(|t| {
                        let title = if t.aggregate { format!("{} (all)", t.name) } else { t.name.clone() };
                        let url = t.url();
                        view! { <MobileLogPane url=url title=title target=t /> }
                    })
                }}
            </div>
        </div>
    }
}

#[component]
fn MobileLogPane(
    url: String,
    title: String,
    target: crate::app::state::LogTarget,
) -> impl IntoView {
    let log_pods = expect_context::<LogPods>().0;
    let stream = use_log_stream(url);
    let logs_ref = NodeRef::<leptos::html::Div>::new();
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        stream.filtered_lines.track();
        if stream.follow.get() {
            if let Some(element) = logs_ref.get_untracked() {
                request_animation_frame(move || element.set_scroll_top(element.scroll_height()));
            }
        }
    });

    view! {
        <div class="mobile-log-controls">
            <div class="mobile-log-source"><strong>{title}</strong>
                <button aria-label="Close this log" on:click=move |_| log_pods.update(|pods| pods.retain(|pod| pod != &target))>"x"</button>
            </div>
            <input class="mobile-log-filter" type="search" placeholder="Filter log lines"
                prop:value=move || stream.filter.get()
                on:input=move |event| stream.filter.set(event_target_value(&event)) />
            <div class="mobile-log-levels" aria-label="Log level filter">
                {[("", "All"), ("error", "ERR"), ("warn", "WRN"), ("info", "INF"), ("debug", "DBG")]
                    .into_iter().map(|(level, label)| view! {
                        <button class:active=move || stream.level_filter.get() == level
                            on:click=move |_| stream.level_filter.set(level.to_string())>{label}</button>
                    }).collect_view()}
            </div>
            <div class="mobile-log-toggles">
                <button class:on=move || stream.show_timestamps.get()
                    on:click=move |_| stream.show_timestamps.update(|value| *value = !*value)>"Time"</button>
                <button class:on=move || stream.follow.get()
                    on:click=move |_| stream.follow.update(|value| *value = !*value)>"Follow"</button>
                <button class:on=move || stream.wrap.get()
                    on:click=move |_| stream.wrap.update(|value| *value = !*value)>"Wrap"</button>
            </div>
        </div>
        <div class="mobile-log-lines" class:nowrap=move || !stream.wrap.get() node_ref=logs_ref>
            <For each=move || stream.filtered_lines.get() key=|(id, _)| *id let:item>
                {
                    let (pod, message) = item.1.split_once(" │ ")
                        .map(|(pod, message)| (Some(pod.to_string()), message.to_string()))
                        .unwrap_or((None, item.1));
                    let level = log_level(&message);
                    let parsed = parse_log_line(&message);
                    let caller = parsed.caller;
                    let (timestamp, display) = if parsed.is_structured {
                        (parsed.timestamp, parsed.display)
                    } else {
                        extract_timestamp(&parsed.display)
                    };
                    let html = ansi_to_html(&display);
                    let level_label = match level {
                        "error" => Some("ERR"), "warn" => Some("WRN"),
                        "info" => Some("INF"), "debug" => Some("DBG"), _ => None,
                    };
                    view! { <div class="mobile-log-line">
                        {pod.map(|pod| {
                            let color = format!("background:hsl({}deg 45% 28%)", hue_of(&pod));
                            view! { <span class="mobile-log-pod" style=color>{pod}</span> }
                        })}
                        {move || stream.show_timestamps.get().then(|| timestamp.clone().map(|timestamp| view! { <span class="mobile-log-time">{timestamp}</span> }))}
                        {level_label.map(|label| view! { <span class=format!("mobile-log-level {level}")>{label}</span> })}
                        {caller.map(|caller| view! { <span class="mobile-log-caller">{caller}</span> })}
                        <span class="mobile-log-message" inner_html=html></span>
                    </div> }
                }
            </For>
        </div>
    }
}
