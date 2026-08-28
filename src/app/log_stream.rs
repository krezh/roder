use leptos::prelude::*;

use crate::app::util::format::log_level;
use crate::data;

#[derive(Clone, Copy)]
pub(crate) struct LogStream {
    pub(crate) filtered_lines: Memo<Vec<(u64, String)>>,
    pub(crate) follow: RwSignal<bool>,
    pub(crate) wrap: RwSignal<bool>,
    pub(crate) show_timestamps: RwSignal<bool>,
    pub(crate) filter: RwSignal<String>,
    pub(crate) level_filter: RwSignal<String>,
}

pub(crate) fn use_log_stream(url: String) -> LogStream {
    let lines = RwSignal::new(Vec::<(u64, String)>::new());
    let counter = StoredValue::new(0u64);
    let follow = RwSignal::new(true);
    let wrap = RwSignal::new(true);
    let show_timestamps = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let level_filter = RwSignal::new(String::new());

    Effect::new(move |_prev: Option<Option<data::SseHandle>>| {
        lines.set(Vec::new());
        let url = url.clone();
        data::subscribe_lines(&url, move |line| {
            let id = counter
                .try_update_value(|counter| {
                    *counter += 1;
                    *counter
                })
                .unwrap_or_default();
            lines.update(|lines| {
                lines.push((id, line));
                if lines.len() > 1000 {
                    let excess = lines.len() - 1000;
                    lines.drain(0..excess);
                }
            });
        })
    });

    let filtered_lines = Memo::new(move |_| {
        let text = filter.get().to_lowercase();
        let level = level_filter.get().to_lowercase();
        lines.with(|lines| {
            lines
                .iter()
                .filter(|(_, line)| log_line_matches(line, &text, &level))
                .cloned()
                .collect()
        })
    });

    LogStream {
        filtered_lines,
        follow,
        wrap,
        show_timestamps,
        filter,
        level_filter,
    }
}

fn log_line_matches(line: &str, text_filter_lower: &str, level_filter_lower: &str) -> bool {
    let (pod, message) = match line.split_once(" │ ") {
        Some((pod, message)) => (Some(pod), message),
        None => (None, line),
    };
    if !level_filter_lower.is_empty() && log_level(message) != level_filter_lower {
        return false;
    }
    text_filter_lower.is_empty()
        || pod.is_some_and(|pod| pod.to_lowercase().contains(text_filter_lower))
        || message.to_lowercase().contains(text_filter_lower)
}

pub(crate) fn extract_timestamp(line: &str) -> (Option<String>, String) {
    let trimmed = line.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 19
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && matches!(bytes[10], b'T' | b' ')
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[..19].iter().all(u8::is_ascii)
    {
        let end = bytes[19..]
            .iter()
            .position(|byte| matches!(byte, b' ' | b'Z' | b'+' | b'-'))
            .map(|index| index + 19)
            .unwrap_or(19);
        let mut timestamp_end = end;
        if bytes.get(end) == Some(&b'.') {
            timestamp_end = bytes[end..]
                .iter()
                .position(|byte| !byte.is_ascii_digit())
                .map(|index| index + end)
                .unwrap_or(bytes.len());
        }
        if matches!(
            bytes.get(timestamp_end),
            Some(&b'Z') | Some(&b'+') | Some(&b'-')
        ) {
            let is_z = bytes[timestamp_end] == b'Z';
            timestamp_end += 1;
            if !is_z {
                timestamp_end = (timestamp_end + 5).min(bytes.len());
            }
        }
        return (
            Some(trimmed[..timestamp_end].to_string()),
            trimmed[timestamp_end..].trim_start().to_string(),
        );
    }
    (None, line.to_string())
}

#[cfg(test)]
mod tests {
    use super::{extract_timestamp, log_line_matches};

    #[test]
    fn aggregate_filter_matches_source_message_and_level() {
        let line = "api-7c9 │ ERROR request failed";
        assert!(log_line_matches(line, "api-7c9", "error"));
        assert!(log_line_matches(line, "request", "error"));
        assert!(!log_line_matches(line, "api-7c9", "info"));
    }

    #[test]
    fn extracts_rfc3339_timestamp() {
        let (timestamp, message) = extract_timestamp("2024-01-15T10:30:45.123Z payload");
        assert_eq!(timestamp.as_deref(), Some("2024-01-15T10:30:45.123Z"));
        assert_eq!(message, "payload");
    }
}
