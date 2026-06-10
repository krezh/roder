//! Right-docked, drag-resizable log sidebar and the individual streaming log pane.

use leptos::ev;
use leptos::prelude::*;

use crate::app::state::{LogPods, LogTarget};
use crate::app::util::color::hue_of;
use crate::app::util::format::{ansi_to_html, log_level};
use crate::data;

/// Right-docked, drag-resizable log sidebar. Holds one streaming `LogsView` pane
/// per open pod, so several pods can be tailed side by side.
#[component]
pub(crate) fn LogSidebar() -> impl IntoView {
    let log_pods = expect_context::<LogPods>().0;
    let width = RwSignal::new(520i32);
    let dragging = RwSignal::new(false);

    #[cfg(target_arch = "wasm32")]
    {
        let mv = window_event_listener(ev::mousemove, move |e: ev::MouseEvent| {
            if !dragging.get_untracked() {
                return;
            }
            let vw = web_sys::window()
                .and_then(|w| w.inner_width().ok())
                .and_then(|v| v.as_f64())
                .unwrap_or(1280.0);
            let w = (vw - e.client_x() as f64).clamp(300.0, vw * 0.85);
            width.set(w as i32);
            e.prevent_default();
        });
        let up = window_event_listener(ev::mouseup, move |_| {
            if dragging.get_untracked() {
                dragging.set(false);
            }
        });
        on_cleanup(move || {
            mv.remove();
            up.remove();
        });
    }

    view! {
        // Always mounted; `.open` slides it in. `width` drives the style in place
        // (no rebuild → log streams aren't restarted while resizing).
        <div class="logbar"
            class:open=move || !log_pods.get().is_empty()
            class:dragging=move || dragging.get()
            style=move || format!("width:{}px", width.get())>
            <div class="logbar-resize" on:mousedown=move |e: ev::MouseEvent| { e.prevent_default(); dragging.set(true); }></div>
            <div class="logbar-head">
                <span class="logbar-title">"Logs"</span>
                <button class="logbar-close" on:click=move |_| log_pods.set(Vec::new())>"✕"</button>
            </div>
            <div class="logbar-body">
                <For each=move || log_pods.get()
                    key=|t| format!("{}/{}/{}/{}", t.key, t.namespace, t.name, t.aggregate) let:t>
                    {
                        let title = if t.aggregate { format!("{} (all pods)", t.name) } else { t.name.clone() };
                        view! {
                            <div class="logpane">
                                <LogsView url=t.url() title=title target=t />
                            </div>
                        }
                    }
                </For>
            </div>
        </div>
    }
}

#[component]
pub(crate) fn LogsView(
    /// The `/api/logs` URL to stream (single pod or aggregated workload).
    url: String,
    /// Shown at the left of the controls row (the sidebar passes the source name).
    #[prop(optional, into)]
    title: Option<String>,
    /// When set, a close button removes this source from the log sidebar.
    #[prop(optional)]
    target: Option<LogTarget>,
) -> impl IntoView {
    let log_pods = expect_context::<LogPods>().0;
    // (id, line) so the keyed <For> appends only the new line instead of
    // re-rendering the whole buffer on every incoming line.
    let lines = RwSignal::new(Vec::<(u64, String)>::new());
    let counter = StoredValue::new(0u64);
    let follow = RwSignal::new(true);
    let wrap = RwSignal::new(true);
    let show_timestamps = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let level_filter = RwSignal::new(String::new());
    let logs_ref = NodeRef::<leptos::html::Div>::new();

    Effect::new(move |_prev: Option<Option<data::SseHandle>>| {
        lines.set(Vec::new());
        let url = url.clone();
        data::subscribe_lines(&url, move |line| {
            let id = counter
                .try_update_value(|c| {
                    *c += 1;
                    *c
                })
                .unwrap_or_default();
            lines.update(|v| {
                v.push((id, line));
                if v.len() > 1000 {
                    let excess = v.len() - 1000;
                    v.drain(0..excess);
                }
            });
        })
    });

    // Auto-scroll to the bottom on new lines while following.
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        lines.get();
        if follow.get() {
            if let Some(el) = logs_ref.get_untracked() {
                el.set_scroll_top(el.scroll_height());
            }
        }
    });

    // Filter lines based on search text and level
    let filtered_lines = Memo::new(move |_| {
        let f = filter.get().to_lowercase();
        let lvl_f = level_filter.get().to_lowercase();
        lines.with(|v| {
            v.iter()
                .filter(|(_, line)| {
                    let (pod, msg) = match line.split_once(" │ ") {
                        Some((p, m)) => (Some(p), m),
                        None => (None, line.as_str()),
                    };
                    // Level filter
                    if !lvl_f.is_empty() && log_level(msg) != lvl_f.as_str() {
                        return false;
                    }
                    // Text filter (search in pod name and message)
                    if !f.is_empty() {
                        let pod_match = pod.is_none_or(|p| p.to_lowercase().contains(&f));
                        let msg_match = msg.to_lowercase().contains(&f);
                        if !pod_match && !msg_match {
                            return false;
                        }
                    }
                    true
                })
                .cloned()
                .collect::<Vec<_>>()
        })
    });

    view! {
        <div class="logs-ctl">
            {title.map(|t| view! { <span class="logs-title">{t}</span> })}
            <input class="logs-filter" placeholder="Filter..."
                prop:value=move || filter.get()
                on:input=move |e| filter.set(event_target_value(&e)) />
            <select class="logs-level-filter"
                prop:value=move || level_filter.get()
                on:change=move |e| level_filter.set(event_target_value(&e))>
                <option value="">"All"</option>
                <option value="error">"Error"</option>
                <option value="warn">"Warn"</option>
                <option value="info">"Info"</option>
                <option value="debug">"Debug"</option>
            </select>
            <span class="logs-ctl-spacer"></span>
            <button class="logtog" class:on=move || show_timestamps.get()
                on:click=move |_| show_timestamps.update(|t| *t = !*t)>"Timestamps"</button>
            <button class="logtog" class:on=move || follow.get()
                on:click=move |_| follow.update(|f| *f = !*f)>"Follow"</button>
            <button class="logtog" class:on=move || wrap.get()
                on:click=move |_| wrap.update(|w| *w = !*w)>"Wrap"</button>
            {target.map(|t| view! {
                <button class="logpane-close"
                    on:click=move |_| log_pods.update(|v| v.retain(|x| x != &t))>"✕"</button>
            })}
        </div>
        <div class="logs" class:nowrap=move || !wrap.get() node_ref=logs_ref>
            <For each=move || filtered_lines.get() key=|(id, _)| *id let:item>
                {
                    // Aggregated workload lines are "pod │ message" — show the pod as a pill.
                    let (pod, msg) = match item.1.split_once(" │ ") {
                        Some((p, m)) => (Some(p.to_string()), m.to_string()),
                        None => (None, item.1),
                    };
                    let lvl = log_level(&msg);
                    // Extract timestamp if present (ISO 8601 format at start of line)
                    let (timestamp, content) = extract_timestamp(&msg);
                    let msg_html = ansi_to_html(&content);
                    view! {
                        <div class="log-line">
                            {pod.map(|p| {
                                let style = format!("background:hsl({}deg 45% 28%)", hue_of(&p));
                                view! { <span class="log-pod" style=style>{p}</span> }
                            })}
                            {move || show_timestamps.get().then(|| timestamp.clone().map(|ts| view! { <span class="log-ts">{ts}</span> }))}
                            <span class=format!("log-msg log-{lvl}") inner_html=msg_html></span>
                        </div>
                    }
                }
            </For>
        </div>
    }
}

/// Extract an ISO 8601 timestamp from the beginning of a log line.
/// Returns `(Some(timestamp), remaining_content)` or `(None, original_line)`.
fn extract_timestamp(line: &str) -> (Option<String>, String) {
    let trimmed = line.trim_start();
    let b = trimmed.as_bytes();

    // Use byte indexing throughout: ISO 8601 timestamps are pure ASCII so
    // byte offsets == char offsets, and we never risk slicing mid-codepoint.
    // Bail early if the first 19 bytes aren't all ASCII (e.g. line starts with
    // a non-ASCII pod name in a multi-pod stream).
    if b.len() >= 19
        && b[4] == b'-'
        && b[7] == b'-'
        && (b[10] == b'T' || b[10] == b' ')
        && b[13] == b':'
        && b[16] == b':'
        && b[..19].iter().all(|c| c.is_ascii())
    {
        // Fractional seconds
        let end = b[19..]
            .iter()
            .position(|&c| c == b' ' || c == b'Z' || c == b'+' || c == b'-')
            .map(|i| i + 19)
            .unwrap_or(19);

        let mut ts_end = end;
        if b.get(end) == Some(&b'.') {
            ts_end = b[end..]
                .iter()
                .position(|c| !c.is_ascii_digit())
                .map(|i| i + end)
                .unwrap_or(b.len());
        }
        if matches!(b.get(ts_end), Some(&b'Z') | Some(&b'+') | Some(&b'-')) {
            let was_z = b[ts_end] == b'Z';
            ts_end += 1;
            if !was_z {
                // Consume +HH:MM or -HH:MM (up to 5 more bytes)
                ts_end = (ts_end + 5).min(b.len());
            }
        }

        let timestamp = trimmed[..ts_end].to_string();
        let content = trimmed[ts_end..].trim_start().to_string();
        return (Some(timestamp), content);
    }

    (None, line.to_string())
}

#[cfg(test)]
mod tests {
    use super::extract_timestamp;

    #[test]
    fn rfc3339_with_z() {
        let (ts, rest) = extract_timestamp("2024-01-15T10:30:45Z some message");
        assert_eq!(ts.as_deref(), Some("2024-01-15T10:30:45Z"));
        assert_eq!(rest, "some message");
    }

    #[test]
    fn rfc3339_with_fractional_and_z() {
        let (ts, rest) = extract_timestamp("2024-01-15T10:30:45.123456789Z payload");
        assert_eq!(ts.as_deref(), Some("2024-01-15T10:30:45.123456789Z"));
        assert_eq!(rest, "payload");
    }

    #[test]
    fn rfc3339_with_offset() {
        let (ts, rest) = extract_timestamp("2024-01-15T10:30:45+05:30 message");
        assert_eq!(ts.as_deref(), Some("2024-01-15T10:30:45+05:30"));
        assert_eq!(rest, "message");
    }

    #[test]
    fn space_separator() {
        let (ts, rest) = extract_timestamp("2024-01-15 10:30:45Z message");
        assert_eq!(ts.as_deref(), Some("2024-01-15 10:30:45Z"));
        assert_eq!(rest, "message");
    }

    #[test]
    fn no_timestamp_returns_original() {
        let line = "plain log line without timestamp";
        let (ts, rest) = extract_timestamp(line);
        assert!(ts.is_none());
        assert_eq!(rest, line);
    }

    #[test]
    fn empty_line() {
        let (ts, rest) = extract_timestamp("");
        assert!(ts.is_none());
        assert_eq!(rest, "");
    }

    #[test]
    fn leading_whitespace_trimmed_for_match() {
        let (ts, _rest) = extract_timestamp("  2024-01-15T10:30:45Z message");
        assert_eq!(ts.as_deref(), Some("2024-01-15T10:30:45Z"));
    }

    #[test]
    fn non_ascii_start_no_match() {
        let line = "ñoño 2024-01-15T10:30:45Z";
        let (ts, rest) = extract_timestamp(line);
        assert!(ts.is_none());
        assert_eq!(rest, line);
    }
}
