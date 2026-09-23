//! Log-line parsing: JSON (Zap/logrus/slog/tracing-subscriber), logfmt, Python
//! `logging`, and syslog RFC 3164/5424, plus severity classification. Raw / klog
//! / ANSI-coloured output is passed through as-is.

use serde_json::Value as JsonValue;

use super::ansi::strip_ansi;

/// Structured fields extracted from a log line.
pub(crate) struct ParsedLog {
    /// The human-readable message (extracted `msg`/`message`/`event` field, or the raw line).
    pub display: String,
    /// Shortened caller (`file.go:line`), if present in structured fields.
    pub caller: Option<String>,
    /// Timestamp from a structured field (`ts`, `time`, `timestamp`).
    pub timestamp: Option<String>,
    /// Normalized severity used by badges and filters.
    pub level: &'static str,
    /// Full structured payload shown on demand when `display` is a summary.
    pub details: Option<JsonValue>,
    /// True when a structured format (JSON or logfmt) was recognised.
    #[cfg_attr(not(test), allow(dead_code))]
    pub is_structured: bool,
}

/// Parse a log line into structured fields. Handles:
/// - JSON (Zap, logrus, slog, Bunyan-ish)
/// - logfmt (logrus text, many Go loggers)
/// - Raw / klog / ANSI-coloured output (returned as-is)
///
/// Lines containing ANSI escape codes are returned raw so colours are preserved.
pub(crate) fn parse_log_line(line: &str) -> ParsedLog {
    let t = line.trim_start();
    let (outer_timestamp, payload) = split_timestamp_prefix(t)
        .map(|(timestamp, rest)| (Some(timestamp.to_string()), rest))
        .unwrap_or((None, t));
    let plain_payload = payload.contains('\x1b').then(|| strip_ansi(payload));
    let level = classify_level(plain_payload.as_deref().unwrap_or(payload));
    let raw_display = if outer_timestamp.is_some() {
        strip_level_prefix(payload).unwrap_or(payload)
    } else {
        payload
    };
    let raw = ParsedLog {
        display: raw_display.to_string(),
        caller: None,
        timestamp: outer_timestamp.clone(),
        level,
        details: None,
        is_structured: outer_timestamp.is_some(),
    };

    // Parsing ANSI text would discard styling from extracted fields.
    if payload.contains('\x1b') {
        return raw;
    }

    let t = payload;

    if t.starts_with('{') {
        if let Ok(JsonValue::Object(obj)) = serde_json::from_str::<JsonValue>(t) {
            if let Some(parsed) = parse_json_rpc(&obj, outer_timestamp.clone(), level) {
                return parsed;
            }
            // Top-level message field (Zap, logrus, slog JSON, …), or tracing-subscriber's
            // nested `fields.message` / `fields.msg`.
            let msg = ["msg", "message", "event", "body", "MESSAGE"]
                .iter()
                .find_map(|&k| obj.get(k).map(json_message_display))
                .or_else(|| {
                    let fields = obj.get("fields")?.as_object()?;
                    ["message", "msg", "event"]
                        .iter()
                        .find_map(|&k| fields.get(k).map(json_message_display))
                });

            if let Some(msg_text) = msg {
                // `target` is the tracing-subscriber equivalent of `caller`.
                let caller = ["caller", "source", "logger", "target", "name"]
                    .iter()
                    .find_map(|&k| obj.get(k)?.as_str().map(shorten_caller));

                let timestamp = outer_timestamp.clone().or_else(|| {
                    ["ts", "time", "timestamp", "@timestamp"]
                        .iter()
                        .find_map(|&k| {
                            let v = obj.get(k)?;
                            v.as_str()
                                .map(str::to_string)
                                .or_else(|| v.as_f64().map(|n| format!("{n:.3}s")))
                        })
                });

                const JSON_META: &[&str] = &[
                    "msg",
                    "message",
                    "event",
                    "body",
                    "MESSAGE",
                    "caller",
                    "source",
                    "logger",
                    "target",
                    "name",
                    "ts",
                    "time",
                    "timestamp",
                    "@timestamp",
                    "level",
                    "severity",
                    "lvl",
                    "severity_text",
                    "severityText",
                    "level_name",
                    "v",
                    "pid",
                    "hostname",
                    "fields",
                ];
                let mut extras: Vec<String> = obj
                    .iter()
                    .filter(|(k, _)| !JSON_META.contains(&k.as_str()))
                    .map(|(k, v)| format!("{k}={}", json_val_display(v)))
                    .collect();
                if let Some(fields) = obj.get("fields").and_then(|f| f.as_object()) {
                    for (k, v) in fields {
                        if k != "message" && k != "msg" {
                            extras.push(format!("{k}={}", json_val_display(v)));
                        }
                    }
                }
                let display = if extras.is_empty() {
                    msg_text
                } else {
                    format!("{msg_text} {}", extras.join(" "))
                };

                return ParsedLog {
                    display,
                    caller,
                    timestamp,
                    level,
                    details: None,
                    is_structured: true,
                };
            }
        }
        return raw;
    }

    // logfmt
    let pairs = logfmt_tokenize(t);
    if let Some(msg_text) = pairs
        .iter()
        .find(|(k, _)| k == "msg" || k == "message" || k == "event")
        .map(|(_, v)| v.clone())
    {
        const LOGFMT_META: &[&str] = &[
            "msg",
            "message",
            "event",
            "caller",
            "source",
            "ts",
            "time",
            "timestamp",
            "level",
            "lvl",
            "severity",
        ];
        let extras: Vec<String> = pairs
            .iter()
            .filter(|(k, _)| !LOGFMT_META.contains(&k.as_str()))
            .map(|(k, v)| {
                if v.contains(' ') || v.is_empty() {
                    format!("{k}=\"{v}\"")
                } else {
                    format!("{k}={v}")
                }
            })
            .collect();
        let display = if extras.is_empty() {
            msg_text
        } else {
            format!("{msg_text} {}", extras.join(" "))
        };
        let caller = pairs
            .iter()
            .find(|(k, _)| k == "caller" || k == "source")
            .map(|(_, v)| shorten_caller(v));
        let timestamp = outer_timestamp.clone().or_else(|| {
            pairs
                .iter()
                .find(|(k, _)| k == "ts" || k == "time" || k == "timestamp")
                .map(|(_, v)| v.clone())
        });
        return ParsedLog {
            display,
            caller,
            timestamp,
            level,
            details: None,
            is_structured: true,
        };
    }

    // Zap's console encoder uses tab-separated timestamp, level, logger, message, and fields.
    if outer_timestamp.is_some() && t.contains('\t') {
        let mut fields = t.split('\t');
        if fields.next().and_then(level_word).is_some() {
            let fields = fields.collect::<Vec<_>>();
            let (caller, message) = match fields.as_slice() {
                [] => (None, String::new()),
                [message] => (None, (*message).to_string()),
                [message, extras] if extras.trim_start().starts_with('{') => {
                    (None, format!("{message}\t{extras}"))
                }
                [caller, rest @ ..] => (Some(shorten_caller(caller)), rest.join("\t")),
            };
            return ParsedLog {
                display: message,
                caller,
                timestamp: outer_timestamp,
                level,
                details: None,
                is_structured: true,
            };
        }
    }

    // Syslog RFC 5424: "<N>1 timestamp hostname app pid msgid [data] message"
    if t.starts_with('<') {
        if let Some((_, rest)) = syslog_priority(t) {
            let rest = rest.trim_start();
            // RFC 5424: version field is a digit
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                let mut fields = rest.splitn(7, ' ');
                let _version = fields.next();
                let timestamp = fields.next().filter(|s| *s != "-").map(str::to_string);
                let _hostname = fields.next();
                let caller = fields.next().filter(|s| *s != "-").map(str::to_string);
                let _procid = fields.next();
                let _msgid = fields.next();
                let sd_and_msg = fields.next().unwrap_or("");
                let msg = rfc5424_message(sd_and_msg);
                if !msg.is_empty() {
                    return ParsedLog {
                        display: msg.to_string(),
                        caller,
                        timestamp: outer_timestamp.clone().or(timestamp),
                        level,
                        details: None,
                        is_structured: true,
                    };
                }
            }
            // RFC 3164 with priority: "<N>Mon DD HH:MM:SS hostname app: message"
            if let Some(mut parsed) = parse_syslog_3164_body(rest) {
                parsed.level = level;
                parsed.timestamp = outer_timestamp.clone().or(parsed.timestamp);
                return parsed;
            }
        }
    }

    // Syslog RFC 3164 without priority: "Mon DD HH:MM:SS hostname app: message"
    if let Some(mut parsed) = parse_syslog_3164_body(t) {
        parsed.level = level;
        parsed.timestamp = outer_timestamp.clone().or(parsed.timestamp);
        return parsed;
    }

    raw
}

fn json_message_display(value: &JsonValue) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn parse_json_rpc(
    object: &serde_json::Map<String, JsonValue>,
    timestamp: Option<String>,
    level: &'static str,
) -> Option<ParsedLog> {
    if object.get("jsonrpc").and_then(JsonValue::as_str) != Some("2.0") {
        return None;
    }

    let (display, level) = if let Some(method) = object.get("method").and_then(JsonValue::as_str) {
        let subject = object
            .get("params")
            .and_then(JsonValue::as_object)
            .and_then(|params| params.get("name"))
            .and_then(JsonValue::as_str)
            .map(|name| format!(" | {name}"))
            .unwrap_or_default();
        (format!("request | {method}{subject}"), level)
    } else if let Some(error) = object.get("error") {
        let message = error
            .get("message")
            .and_then(JsonValue::as_str)
            .unwrap_or("request failed");
        (format!("error | {message}"), "error")
    } else if let Some(result) = object.get("result") {
        (json_rpc_result_summary(result), level)
    } else {
        ("response".to_string(), level)
    };

    Some(ParsedLog {
        display,
        caller: Some("jsonrpc".to_string()),
        timestamp,
        level,
        details: Some(JsonValue::Object(object.clone())),
        is_structured: true,
    })
}

fn json_rpc_result_summary(result: &JsonValue) -> String {
    let Some(object) = result.as_object() else {
        return match result {
            JsonValue::Array(items) => format!("response | {} items", items.len()),
            value => format!("response | {}", json_val_display(value)),
        };
    };

    let mut parts = vec!["response".to_string()];
    if let Some(server) = object
        .get("_meta")
        .and_then(JsonValue::as_object)
        .and_then(|meta| meta.get("io.modelcontextprotocol/serverInfo"))
        .and_then(JsonValue::as_object)
    {
        let name = server.get("name").and_then(JsonValue::as_str);
        let version = server.get("version").and_then(JsonValue::as_str);
        if let Some(label) = match (name, version) {
            (Some(name), Some(version)) => Some(format!("{name} {version}")),
            (Some(name), None) => Some(name.to_string()),
            _ => None,
        } {
            parts.push(label);
        }
    }
    for (key, singular, plural) in [
        ("tools", "tool", "tools"),
        ("resources", "resource", "resources"),
        (
            "resourceTemplates",
            "resource template",
            "resource templates",
        ),
        ("prompts", "prompt", "prompts"),
        ("content", "content item", "content items"),
    ] {
        if let Some(items) = object.get(key).and_then(JsonValue::as_array) {
            let suffix = if items.len() == 1 { singular } else { plural };
            parts.push(format!("{} {suffix}", items.len()));
        }
    }
    if let Some(result_type) = object.get("resultType").and_then(JsonValue::as_str) {
        parts.push(result_type.to_string());
    }
    if parts.len() == 1 {
        parts.push(format!("{} fields", object.len()));
    }
    parts.join(" | ")
}

fn strip_level_prefix(t: &str) -> Option<&str> {
    if let Some(inner) = t.strip_prefix('[') {
        let end = inner.find(']')?;
        level_word(&inner[..end])?;
        return Some(
            inner[end + 1..]
                .trim_start()
                .trim_start_matches([':', '-'])
                .trim_start(),
        );
    }
    let end = t
        .bytes()
        .position(|byte| !byte.is_ascii_alphabetic())
        .unwrap_or(t.len());
    level_word(&t[..end])?;
    Some(
        t[end..]
            .trim_start()
            .trim_start_matches([':', '-', '|'])
            .trim_start(),
    )
}

fn split_timestamp_prefix(t: &str) -> Option<(&str, &str)> {
    let bracketed = t.starts_with('[');
    let start = usize::from(bracketed);
    let b = t.as_bytes();
    if b.len() < start + 19
        || b.get(start + 4) != Some(&b'-')
        || b.get(start + 7) != Some(&b'-')
        || !matches!(b.get(start + 10), Some(b'T' | b' '))
        || b.get(start + 13) != Some(&b':')
        || b.get(start + 16) != Some(&b':')
        || !b[start..start + 19].iter().all(u8::is_ascii)
    {
        return None;
    }

    let mut end = start + 19;
    if matches!(b.get(end), Some(b'.' | b',')) {
        end += 1;
        while b.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
    }
    if b.get(end) == Some(&b'Z') {
        end += 1;
    } else if matches!(b.get(end), Some(b'+' | b'-')) {
        end += 1;
        while b
            .get(end)
            .is_some_and(|byte| byte.is_ascii_digit() || *byte == b':')
        {
            end += 1;
        }
    }

    let timestamp_end = end;
    if bracketed {
        if b.get(end) != Some(&b']') {
            return None;
        }
        end += 1;
    }
    if !b.get(end).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    let mut rest = t[end..].trim_start();
    if let Some(after_stream) = rest
        .strip_prefix("stdout ")
        .or_else(|| rest.strip_prefix("stderr "))
    {
        rest = after_stream
            .strip_prefix("F ")
            .or_else(|| after_stream.strip_prefix("P "))
            .unwrap_or(after_stream);
    }
    Some((&t[start..timestamp_end], rest))
}

fn shorten_caller(s: &str) -> String {
    s.rfind('/')
        .map_or_else(|| s.to_string(), |i| s[i + 1..].to_string())
}

/// Extract syslog priority from a `<NNN>` prefix. Returns `(severity 0–7, rest after '>')`.
fn syslog_priority(t: &str) -> Option<(u8, &str)> {
    let inner = t.strip_prefix('<')?;
    let gt = inner.find('>')?;
    let priority: u32 = inner[..gt].parse().ok()?;
    Some(((priority % 8) as u8, &inner[gt + 1..]))
}

fn syslog_severity_to_level(sev: u8) -> &'static str {
    match sev {
        0..=3 => "error",
        4 => "warn",
        5 | 6 => "info",
        7 => "debug",
        _ => "plain",
    }
}

/// Parse the body of a syslog RFC 3164 line: "Mon DD HH:MM:SS hostname app[pid]: message".
/// `t` must start at the month abbreviation (priority prefix already stripped).
fn parse_syslog_3164_body(t: &str) -> Option<ParsedLog> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if !MONTHS.iter().any(|&m| t.starts_with(m)) {
        return None;
    }
    // "Mon DD HH:MM:SS hostname rest..."
    let mut iter = t.splitn(5, ' ').filter(|s| !s.is_empty());
    let month = iter.next()?;
    let day = iter.next()?;
    let time = iter.next()?;
    let _hostname = iter.next()?;
    let rest = iter.next().unwrap_or("").trim();

    let timestamp = format!("{month} {day} {time}");

    let (caller, msg) = if let Some(pos) = rest.find(": ") {
        let app_part = &rest[..pos];
        let app = app_part
            .find('[')
            .map(|i| app_part[..i].to_string())
            .unwrap_or_else(|| app_part.to_string());
        (Some(app), rest[pos + 2..].to_string())
    } else {
        (None, rest.to_string())
    };

    if msg.is_empty() && caller.is_none() {
        return None;
    }
    Some(ParsedLog {
        display: if msg.is_empty() {
            rest.to_string()
        } else {
            msg
        },
        caller,
        timestamp: Some(timestamp),
        level: "plain",
        details: None,
        is_structured: true,
    })
}

/// Locate the syslog RFC 5424 message: everything after the structured-data block `[…]` or
/// the `-` nil value. `s` starts at the structured-data or `-` field.
fn rfc5424_message(s: &str) -> &str {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix("- ").or_else(|| (s == "-").then_some("")) {
        return rest.trim_start();
    }
    if !s.starts_with('[') {
        return s;
    }
    // Walk through structured-data element(s), respecting quoted values.
    let b = s.as_bytes();
    let mut i = 0;
    let mut in_quotes = false;
    while i < b.len() {
        match b[i] {
            b'\\' if in_quotes => i += 2,
            b'"' => {
                in_quotes = !in_quotes;
                i += 1;
            }
            b']' if !in_quotes => {
                i += 1;
                // Another SD element may follow immediately.
                if b.get(i) == Some(&b'[') {
                    continue;
                }
                break;
            }
            _ => i += 1,
        }
    }
    s[i..].trim_start()
}

/// Classify a log line by severity. Tries structured formats in order:
/// klog/glog prefix, JSON `"level":`, logfmt `level=`, bracket prefix `[LEVEL]`,
/// then level word at line start. Returns "plain" when nothing matches.
///
/// Deliberately avoids substring search across the full message to prevent false
/// positives on messages that happen to contain words like "error" or "info".
#[cfg(test)]
pub(crate) fn log_level(line: &str) -> &'static str {
    let line = line.split_once(" │ ").map(|(_, r)| r).unwrap_or(line);
    let plain = line.contains('\x1b').then(|| strip_ansi(line));
    let t = plain.as_deref().unwrap_or(line).trim_start();
    let t = split_timestamp_prefix(t).map(|(_, rest)| rest).unwrap_or(t);
    classify_level(t)
}

fn classify_level(t: &str) -> &'static str {
    // klog/glog: E0603, W0603, I0603, D0603, F0603
    {
        let b = t.as_bytes();
        if b.len() >= 2 && b[1].is_ascii_digit() {
            match b[0] {
                b'E' | b'F' => return "error",
                b'W' => return "warn",
                b'I' => return "info",
                b'D' => return "debug",
                _ => {}
            }
        }
    }

    // Parse JSON rather than matching its surface formatting: valid JSON allows
    // whitespace around separators and escaped content can contain level-like text.
    if t.starts_with('{') {
        if let Ok(JsonValue::Object(object)) = serde_json::from_str::<JsonValue>(t) {
            for key in [
                "level",
                "severity",
                "lvl",
                "severity_text",
                "severityText",
                "level_name",
            ] {
                if let Some(value) = object.get(key) {
                    if let Some(lvl) = value.as_str().and_then(level_word) {
                        return lvl;
                    }
                    if let Some(lvl) = value.as_u64().and_then(numeric_level) {
                        return lvl;
                    }
                }
            }
            if let Some(lvl) = object
                .get("fields")
                .and_then(JsonValue::as_object)
                .and_then(|fields| fields.get("level").or_else(|| fields.get("severity")))
                .and_then(JsonValue::as_str)
                .and_then(level_word)
            {
                return lvl;
            }
        }
        return "plain";
    }

    // logfmt: level=error, lvl=warn, severity=info (value optionally quoted).
    // Require the key to be at start-of-line or preceded by whitespace so we
    // don't match `level=` embedded inside a quoted value later in the line.
    for key in ["level=", "lvl=", "severity="] {
        if let Some(lvl) = logfmt_level(t, key) {
            return lvl;
        }
    }

    // Syslog with priority prefix: "<N>..." → severity = N % 8
    if t.starts_with('<') {
        if let Some((severity, _)) = syslog_priority(t) {
            return syslog_severity_to_level(severity);
        }
    }

    // Bracket prefix: [ERROR], [WARN], [INFO], [DEBUG]
    if let Some(inner) = t.strip_prefix('[') {
        if let Some(end) = inner.find(']') {
            if let Some(lvl) = level_word(&inner[..end]) {
                return lvl;
            }
        }
    }

    // Level word at the very start of the line: "ERROR: ...", "WARN message", etc.
    let word_end = t
        .bytes()
        .position(|b| matches!(b, b':' | b' ' | b'\t' | b'-' | b'|'))
        .unwrap_or(t.len());
    if word_end <= 8 {
        if let Some(lvl) = level_word(&t[..word_end]) {
            return lvl;
        }
    }

    "plain"
}

fn numeric_level(level: u64) -> Option<&'static str> {
    match level {
        50.. => Some("error"),
        40..=49 => Some("warn"),
        30..=39 => Some("info"),
        0..=29 => Some("debug"),
    }
}

fn logfmt_level(t: &str, key: &str) -> Option<&'static str> {
    let pos = logfmt_key_pos(t, key)?;
    let val = t[pos + key.len()..].trim_start_matches('"');
    level_word(val)
}

/// Position of a logfmt key in `t`, requiring it to be at line-start or
/// preceded by whitespace and not inside a quoted value.
fn logfmt_key_pos(t: &str, key: &str) -> Option<usize> {
    let mut search = t;
    while let Some(i) = search.find(key) {
        let abs = t.len() - search.len() + i;
        let preceded_by_ws =
            abs == 0 || matches!(t.as_bytes().get(abs - 1), Some(&b' ') | Some(&b'\t'));
        let inside_quotes = t[..abs].bytes().filter(|&b| b == b'"').count() % 2 == 1;
        if preceded_by_ws && !inside_quotes {
            return Some(abs);
        }
        search = &search[i + 1..];
    }
    None
}

fn json_val_display(v: &JsonValue) -> String {
    match v {
        JsonValue::String(s) => {
            if s.contains(' ') {
                format!("\"{s}\"")
            } else {
                s.clone()
            }
        }
        other => other.to_string(),
    }
}

/// Parse all key=value pairs from a logfmt string.
fn logfmt_tokenize(t: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let b = t.as_bytes();
    let mut i = 0;

    while i < b.len() {
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }

        let key_start = i;
        while i < b.len() && b[i] != b'=' && b[i] != b' ' && b[i] != b'\t' {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            continue;
        }
        let key = t[key_start..i].to_string();
        i += 1; // skip '='

        let value = if i < b.len() && b[i] == b'"' {
            i += 1;
            let mut val = String::new();
            let mut esc = false;
            loop {
                if i >= b.len() {
                    break;
                }
                let c = t[i..].chars().next().unwrap();
                let clen = c.len_utf8();
                if esc {
                    match c {
                        '"' => val.push('"'),
                        'n' => val.push('\n'),
                        't' => val.push('\t'),
                        '\\' => val.push('\\'),
                        _ => {
                            val.push('\\');
                            val.push(c);
                        }
                    }
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    i += clen;
                    break;
                } else {
                    val.push(c);
                }
                i += clen;
            }
            val
        } else {
            let val_start = i;
            while i < b.len() && b[i] != b' ' && b[i] != b'\t' {
                i += 1;
            }
            t[val_start..i].to_string()
        };

        if !key.is_empty() {
            result.push((key, value));
        }
    }

    result
}

/// Match the first alphabetic token of `s` against known level names.
fn level_word(s: &str) -> Option<&'static str> {
    let word = s
        .split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or("");
    if word.is_empty() {
        return None;
    }
    match word.to_ascii_lowercase().as_str() {
        "error" | "fatal" | "panic" | "crit" | "critical" => Some("error"),
        "warn" | "warning" => Some("warn"),
        "info" => Some("info"),
        "debug" | "trace" | "dbg" => Some("debug"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- log_level ---

    #[test]
    fn log_level_klog_prefix_error() {
        assert_eq!(log_level("E0603 some error message"), "error");
    }

    #[test]
    fn log_level_klog_prefix_warn() {
        assert_eq!(log_level("W0603 some warning"), "warn");
    }

    #[test]
    fn log_level_klog_prefix_info() {
        assert_eq!(log_level("I0603 informational"), "info");
    }

    #[test]
    fn parse_log_line_json_standard() {
        let p = parse_log_line(
            r#"{"level":"info","ts":1234.0,"caller":"pkg/main.go:42","msg":"starting"}"#,
        );
        assert!(p.is_structured);
        assert_eq!(p.display, "starting");
        assert_eq!(p.caller.as_deref(), Some("main.go:42"));
        assert_eq!(p.timestamp.as_deref(), Some("1234.000s"));
    }

    #[test]
    fn parses_timestamp_wrapped_json_consistently() {
        let p = parse_log_line(
            r#"2024-01-15T10:30:45.123Z {"severity":"ERROR","message":"request failed"}"#,
        );
        assert_eq!(p.timestamp.as_deref(), Some("2024-01-15T10:30:45.123Z"));
        assert_eq!(p.level, "error");
        assert_eq!(p.display, "request failed");
    }

    #[test]
    fn parses_cri_envelope_before_structured_payload() {
        let p = parse_log_line(
            r#"2024-01-15T10:30:45.123456789Z stderr F {"level":"warn","msg":"retrying"}"#,
        );
        assert_eq!(
            p.timestamp.as_deref(),
            Some("2024-01-15T10:30:45.123456789Z")
        );
        assert_eq!(p.level, "warn");
        assert_eq!(p.display, "retrying");
    }

    #[test]
    fn parses_bunyan_numeric_level() {
        let p = parse_log_line(
            r#"{"name":"api","hostname":"node-1","pid":12,"level":50,"msg":"failed","time":"2024-01-15T10:30:45Z","v":0}"#,
        );
        assert_eq!(p.level, "error");
        assert_eq!(p.caller.as_deref(), Some("api"));
        assert_eq!(p.display, "failed");
    }

    #[test]
    fn parses_otel_body_and_severity_text() {
        let p = parse_log_line(
            r#"{"severityText":"INFO","body":{"operation":"sync","count":2},"attributes":{"cluster":"prod"}}"#,
        );
        assert_eq!(p.level, "info");
        assert!(p.display.starts_with(r#"{"count":2,"operation":"sync"}"#));
        assert!(p.display.contains(r#"attributes={"cluster":"prod"}"#));
    }

    #[test]
    fn parses_json_rpc_response_without_message_field() {
        let p = parse_log_line(
            r#"{"jsonrpc":"2.0","id":"request-1|int64:42","result":{"cacheScope":"private","resources":[]}}"#,
        );
        assert!(p.is_structured);
        assert_eq!(p.caller.as_deref(), Some("jsonrpc"));
        assert_eq!(p.display, "response | 0 resources");
        assert_eq!(
            p.details
                .as_ref()
                .and_then(|details| details.get("id"))
                .and_then(JsonValue::as_str),
            Some("request-1|int64:42")
        );
    }

    #[test]
    fn summarizes_json_rpc_server_info() {
        let p = parse_log_line(
            r#"{"jsonrpc":"2.0","id":1,"result":{"_meta":{"io.modelcontextprotocol/serverInfo":{"name":"mcp-nixos","version":"3.1.0"}},"resultType":"complete"}}"#,
        );
        assert_eq!(p.display, "response | mcp-nixos 3.1.0 | complete");
    }

    #[test]
    fn parses_bracketed_timestamp_and_level() {
        let p = parse_log_line("[2024-01-15 10:30:45.123] ERROR database unavailable");
        assert_eq!(p.timestamp.as_deref(), Some("2024-01-15 10:30:45.123"));
        assert_eq!(p.level, "error");
        assert_eq!(p.display, "database unavailable");
    }

    #[test]
    fn parses_zap_console_line_with_offset_and_tabs() {
        let p = parse_log_line(
            "2026-09-17T22:10:28+02:00\tINFO\tsetup\twebhooks enabled\t{\"enabled\":true}",
        );
        assert_eq!(p.timestamp.as_deref(), Some("2026-09-17T22:10:28+02:00"));
        assert_eq!(p.level, "info");
        assert_eq!(p.caller.as_deref(), Some("setup"));
        assert_eq!(p.display, "webhooks enabled\t{\"enabled\":true}");
    }

    #[test]
    fn parses_zap_console_line_without_logger() {
        let p = parse_log_line(
            "2026-09-17T22:10:28+02:00\tINFO\tStarting EventSource\t{\"kind\":\"Pod\"}",
        );
        assert!(p.caller.is_none());
        assert_eq!(p.display, "Starting EventSource\t{\"kind\":\"Pod\"}");
    }

    #[test]
    fn parse_log_line_tracing_subscriber() {
        let line = r#"{"timestamp":"2026-06-17T18:26:15.370166Z","level":"WARN","fields":{"message":"stream error"},"target":"towonel_agent::tunnel"}"#;
        let p = parse_log_line(line);
        assert!(p.is_structured);
        assert_eq!(p.display, "stream error");
        assert_eq!(p.caller.as_deref(), Some("towonel_agent::tunnel"));
        assert_eq!(p.timestamp.as_deref(), Some("2026-06-17T18:26:15.370166Z"));
    }

    #[test]
    fn parse_log_line_logfmt() {
        let p = parse_log_line(
            r#"time=2024-01-15T10:30:45Z level=info caller=main.go:42 msg="server started""#,
        );
        assert!(p.is_structured);
        assert_eq!(p.display, "server started");
        assert_eq!(p.caller.as_deref(), Some("main.go:42"));
    }

    #[test]
    fn parse_log_line_ansi_passthrough() {
        let line = "\x1b[31mERROR\x1b[0m something failed";
        let p = parse_log_line(line);
        assert!(!p.is_structured);
        assert_eq!(p.display, line);
    }

    #[test]
    fn parse_log_line_plain() {
        let p = parse_log_line("I0603 12:00:00.000000 main.go:42] starting");
        assert!(!p.is_structured);
        assert_eq!(p.display, "I0603 12:00:00.000000 main.go:42] starting");
    }

    #[test]
    fn parse_log_line_json_extra_fields() {
        let p = parse_log_line(
            r#"{"time":"2026-06-22T20:38:29Z","level":"INFO","msg":"HTTP request","method":"POST","path":"/webhook/token-review","status":200}"#,
        );
        assert!(p.is_structured);
        assert!(p.display.starts_with("HTTP request"), "got: {}", p.display);
        assert!(p.display.contains("method=POST"), "got: {}", p.display);
        assert!(p.display.contains("status=200"), "got: {}", p.display);
        // path contains no spaces so should be unquoted
        assert!(
            p.display.contains("path=/webhook/token-review"),
            "got: {}",
            p.display
        );
    }

    #[test]
    fn parse_log_line_json_extra_field_with_spaces() {
        let p = parse_log_line(
            r#"{"time":"2026-06-22T20:38:29Z","level":"WARN","msg":"token review denied","reason":"invalid webhook token"}"#,
        );
        assert!(p.is_structured);
        assert!(
            p.display.starts_with("token review denied"),
            "got: {}",
            p.display
        );
        assert!(
            p.display.contains("reason=\"invalid webhook token\""),
            "got: {}",
            p.display
        );
    }

    #[test]
    fn parse_log_line_logfmt_extra_fields() {
        let p = parse_log_line(
            r#"time=2024-01-15T10:30:45Z level=info caller=main.go:42 msg="request handled" method=GET status=200"#,
        );
        assert!(p.is_structured);
        assert!(
            p.display.starts_with("request handled"),
            "got: {}",
            p.display
        );
        assert!(p.display.contains("method=GET"), "got: {}", p.display);
        assert!(p.display.contains("status=200"), "got: {}", p.display);
    }

    #[test]
    fn log_level_json() {
        assert_eq!(log_level(r#"{"level":"error","msg":"oops"}"#), "error");
        assert_eq!(
            log_level(r#"{"level":"info","ts":1234,"msg":"ok"}"#),
            "info"
        );
        assert_eq!(
            log_level(r#"{"severity":"WARNING","message":"degraded"}"#),
            "warn"
        );
        assert_eq!(log_level(r#"{"lvl":"debug","msg":"connecting"}"#), "debug");
        // JSON with "error" only in message body must not be classified as error
        assert_eq!(
            log_level(r#"{"level":"info","msg":"network error occurred"}"#),
            "info"
        );
        // JSON without a level key → plain
        assert_eq!(log_level(r#"{"msg":"hello"}"#), "plain");
    }

    #[test]
    fn log_level_json_accepts_valid_whitespace() {
        assert_eq!(
            log_level(r#"{ "message" : "degraded", "level" : "warning" }"#),
            "warn"
        );
    }

    #[test]
    fn log_level_logfmt() {
        assert_eq!(log_level("time=2024 level=warn msg=degraded"), "warn");
        assert_eq!(log_level(r#"time=2024 level="info" msg=ok"#), "info");
        assert_eq!(
            log_level("ts=2024 lvl=error caller=main.go msg=fail"),
            "error"
        );
        // "level=" inside a value must not trigger (preceded by non-space)
        assert_eq!(log_level(r#"msg="set level=debug" ts=2024"#), "plain");
    }

    #[test]
    fn log_level_bracket_prefix() {
        assert_eq!(log_level("[ERROR] something failed"), "error");
        assert_eq!(log_level("[WARN] disk nearly full"), "warn");
        assert_eq!(log_level("[info] starting up"), "info");
    }

    #[test]
    fn log_level_word_prefix() {
        assert_eq!(log_level("panic: runtime error"), "error");
        assert_eq!(log_level("FATAL: out of memory"), "error");
        assert_eq!(log_level("DEBUG: connecting..."), "debug");
        assert_eq!(log_level("ERROR something"), "error");
        assert_eq!(log_level("warn - disk nearly full"), "warn");
    }

    #[test]
    fn log_level_no_false_positives() {
        // Words appearing mid-message must NOT trigger classification
        assert_eq!(log_level("network error occurred"), "plain");
        assert_eq!(log_level("found information in registry"), "plain");
        assert_eq!(
            log_level("connection to debug-service established"),
            "plain"
        );
        assert_eq!(log_level("http://error-reporting.example.com"), "plain");
        assert_eq!(log_level("Reconciling HelmRelease"), "plain");
    }

    #[test]
    fn log_level_aggregated_prefix_stripped() {
        assert_eq!(log_level("my-pod-xyz │ E0101 something bad"), "error");
        // message part containing "error" mid-sentence must not mis-classify
        assert_eq!(log_level("my-pod-xyz │ network error occurred"), "plain");
    }

    #[test]
    fn log_level_plain() {
        assert_eq!(log_level("hello world"), "plain");
        assert_eq!(log_level("Starting server on :8080"), "plain");
    }

    #[test]
    fn log_level_ignores_ansi_formatting() {
        assert_eq!(log_level("\x1b[32mINFO\x1b[0m ready"), "info");
        assert_eq!(log_level("sidecar │ \x1b[31mERROR\x1b[0m failed"), "error");
    }

    // --- Python logging ---

    #[test]
    fn parse_log_line_python_basic() {
        let p = parse_log_line("2024-01-15 10:30:45,123 INFO server started on port 8080");
        assert!(p.is_structured, "expected structured");
        assert_eq!(p.display, "server started on port 8080");
        assert_eq!(p.timestamp.as_deref(), Some("2024-01-15 10:30:45,123"));
        assert!(p.caller.is_none());
    }

    #[test]
    fn parse_log_line_python_error() {
        let p = parse_log_line("2024-01-15 10:30:45,123 ERROR Connection refused to database");
        assert!(p.is_structured);
        assert_eq!(p.display, "Connection refused to database");
        assert_eq!(p.timestamp.as_deref(), Some("2024-01-15 10:30:45,123"));
    }

    #[test]
    fn parse_log_line_python_with_logger() {
        let p = parse_log_line("2024-01-15 10:30:45,123 WARNING django.request POST /api/ 400");
        assert!(p.is_structured);
        assert_eq!(p.display, "django.request POST /api/ 400");
    }

    #[test]
    fn log_level_python_info() {
        assert_eq!(log_level("2024-01-15 10:30:45,123 INFO hello"), "info");
    }

    #[test]
    fn log_level_python_error() {
        assert_eq!(log_level("2024-01-15 10:30:45,123 ERROR boom"), "error");
    }

    #[test]
    fn log_level_python_warning() {
        assert_eq!(
            log_level("2024-01-15 10:30:45,123 WARNING disk almost full"),
            "warn"
        );
    }

    #[test]
    fn log_level_python_debug() {
        assert_eq!(
            log_level("2024-01-15 10:30:45,123 DEBUG connecting"),
            "debug"
        );
    }

    // --- Syslog RFC 5424 ---

    #[test]
    fn parse_log_line_syslog_rfc5424_basic() {
        let p = parse_log_line(
            "<165>1 2024-01-15T10:30:45.000000+00:00 mymachine myapp 1234 ID47 - An application event",
        );
        assert!(p.is_structured, "expected structured");
        assert_eq!(p.display, "An application event");
        assert_eq!(
            p.timestamp.as_deref(),
            Some("2024-01-15T10:30:45.000000+00:00")
        );
        assert_eq!(p.caller.as_deref(), Some("myapp"));
    }

    #[test]
    fn parse_log_line_syslog_rfc5424_with_sd() {
        let p = parse_log_line(
            r#"<34>1 2024-01-15T10:30:45Z host myapp 100 - [exampleSDID@32473 key="val"] the message"#,
        );
        assert!(p.is_structured);
        assert_eq!(p.display, "the message");
        assert_eq!(p.caller.as_deref(), Some("myapp"));
    }

    #[test]
    fn log_level_syslog_rfc5424_error() {
        // priority 34 = facility 4, severity 2 (critical) → error
        assert_eq!(
            log_level("<34>1 2024-01-15T10:30:45Z h app - - - msg"),
            "error"
        );
    }

    #[test]
    fn log_level_syslog_rfc5424_warn() {
        // priority 164 = facility 20, severity 4 (warning) → warn
        assert_eq!(
            log_level("<164>1 2024-01-15T10:30:45Z h app - - - msg"),
            "warn"
        );
    }

    #[test]
    fn log_level_syslog_rfc5424_info() {
        // priority 165 = facility 20, severity 5 (notice) → info
        assert_eq!(
            log_level("<165>1 2024-01-15T10:30:45Z h app - - - msg"),
            "info"
        );
    }

    #[test]
    fn log_level_syslog_rfc5424_debug() {
        // priority 167 = facility 20, severity 7 (debug) → debug
        assert_eq!(
            log_level("<167>1 2024-01-15T10:30:45Z h app - - - msg"),
            "debug"
        );
    }

    // --- Syslog RFC 3164 ---

    #[test]
    fn parse_log_line_syslog_rfc3164_no_priority() {
        let p = parse_log_line("Jan 15 10:30:45 myhost sshd[1234]: Accepted publickey for user");
        assert!(p.is_structured, "expected structured");
        assert_eq!(p.display, "Accepted publickey for user");
        assert_eq!(p.timestamp.as_deref(), Some("Jan 15 10:30:45"));
        assert_eq!(p.caller.as_deref(), Some("sshd"));
    }

    #[test]
    fn parse_log_line_syslog_rfc3164_with_priority() {
        let p = parse_log_line("<34>Jan 15 10:30:45 myhost su: 'su root' failed");
        assert!(p.is_structured);
        assert_eq!(p.display, "'su root' failed");
        assert_eq!(p.caller.as_deref(), Some("su"));
    }

    #[test]
    fn log_level_syslog_rfc3164_priority() {
        // <34> → severity 2 (critical) → error
        assert_eq!(log_level("<34>Jan 15 10:30:45 host su: msg"), "error");
        // <30> → severity 6 (info) → info
        assert_eq!(log_level("<30>Jan 15 10:30:45 host su: msg"), "info");
    }
}
