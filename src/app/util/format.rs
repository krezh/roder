//! Pure formatting / parsing helpers used across the UI.

use serde_json::Value as JsonValue;

/// Parse a resource key ("group/version/kind", group may be empty) into (group, kind).
pub(crate) fn parse_key(key: &str) -> (String, String) {
    let mut parts = key.splitn(3, '/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(g), Some(_v), Some(k)) => (g.to_string(), k.to_string()),
        _ => (String::new(), key.to_string()),
    }
}

/// "readyReplicas" -> "Ready replicas", "hostIP" -> "Host IP" (acronyms kept intact).
pub(crate) fn camel_label(s: &str) -> String {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if prev.is_none() {
            out.extend(ch.to_uppercase());
            prev = Some(ch);
            continue;
        }
        if ch.is_uppercase() {
            let p = prev.unwrap();
            let next_lower = chars.peek().is_some_and(|n| n.is_lowercase());
            // Word boundary: camelCase (prev lower/digit) or end of an acronym (HTTPServer -> HTTP Server).
            if p.is_lowercase() || p.is_ascii_digit() || (p.is_uppercase() && next_lower) {
                out.push(' ');
            }
        }
        out.push(ch);
        prev = Some(ch);
    }
    out
}

/// Extract a Talos version from a node's `osImage` ("Talos (v1.7.6)" → "v1.7.6").
pub(crate) fn talos_version(os_image: &str) -> Option<String> {
    if let (Some(a), Some(b)) = (os_image.find('('), os_image.find(')')) {
        if b > a + 1 {
            return Some(os_image[a + 1..b].to_string());
        }
    }
    os_image
        .strip_prefix("Talos")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Structured fields extracted from a log line.
pub(crate) struct ParsedLog {
    /// The human-readable message (extracted `msg`/`message`/`event` field, or the raw line).
    pub display: String,
    /// Shortened caller (`file.go:line`), if present in structured fields.
    pub caller: Option<String>,
    /// Timestamp from a structured field (`ts`, `time`, `timestamp`).
    pub timestamp: Option<String>,
    /// True when a structured format (JSON or logfmt) was recognised.
    pub is_structured: bool,
}

/// Parse a log line into structured fields. Handles:
/// - JSON (Zap, logrus, slog, Bunyan-ish)
/// - logfmt (logrus text, many Go loggers)
/// - Raw / klog / ANSI-coloured output (returned as-is)
///
/// Lines containing ANSI escape codes are returned raw so colours are preserved.
pub(crate) fn parse_log_line(line: &str) -> ParsedLog {
    let raw = ParsedLog {
        display: line.to_string(),
        caller: None,
        timestamp: None,
        is_structured: false,
    };

    // Don't touch ANSI-coloured output — pass it straight through.
    if line.contains('\x1b') {
        return raw;
    }

    let t = line.trim_start();

    if t.starts_with('{') {
        if let Ok(JsonValue::Object(obj)) = serde_json::from_str::<JsonValue>(t) {
            // Top-level message field (Zap, logrus, slog JSON, …), or tracing-subscriber's
            // nested `fields.message` / `fields.msg`.
            let msg = ["msg", "message", "event"]
                .iter()
                .find_map(|&k| obj.get(k)?.as_str().map(str::to_string))
                .or_else(|| {
                    let fields = obj.get("fields")?.as_object()?;
                    ["message", "msg"]
                        .iter()
                        .find_map(|&k| fields.get(k)?.as_str().map(str::to_string))
                });

            if let Some(msg_text) = msg {
                // `target` is the tracing-subscriber equivalent of `caller`.
                let caller = ["caller", "source", "logger", "target"]
                    .iter()
                    .find_map(|&k| obj.get(k)?.as_str().map(shorten_caller));

                let timestamp = ["ts", "time", "timestamp"].iter().find_map(|&k| {
                    let v = obj.get(k)?;
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| v.as_f64().map(|n| format!("{n:.3}s")))
                });

                const JSON_META: &[&str] = &[
                    "msg", "message", "event",
                    "caller", "source", "logger", "target",
                    "ts", "time", "timestamp",
                    "level", "severity", "lvl",
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
        .find(|(k, _)| k == "msg" || k == "message")
        .map(|(_, v)| v.clone())
    {
        const LOGFMT_META: &[&str] = &[
            "msg", "message", "caller", "source",
            "ts", "time", "timestamp", "level", "lvl", "severity",
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
        let timestamp = pairs
            .iter()
            .find(|(k, _)| k == "ts" || k == "time" || k == "timestamp")
            .map(|(_, v)| v.clone());
        return ParsedLog {
            display,
            caller,
            timestamp,
            is_structured: true,
        };
    }

    raw
}

fn shorten_caller(s: &str) -> String {
    s.rfind('/')
        .map_or_else(|| s.to_string(), |i| s[i + 1..].to_string())
}

/// Classify a log line by severity. Tries structured formats in order:
/// klog/glog prefix, JSON `"level":`, logfmt `level=`, bracket prefix `[LEVEL]`,
/// then level word at line start. Returns "plain" when nothing matches.
///
/// Deliberately avoids substring search across the full message to prevent false
/// positives on messages that happen to contain words like "error" or "info".
pub(crate) fn log_level(line: &str) -> &'static str {
    let line = line.split_once(" │ ").map(|(_, r)| r).unwrap_or(line);
    let t = line.trim_start();

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

    // JSON: only use JSON-path for lines that look like JSON objects to avoid
    // accidentally matching `level=` inside a stringified JSON value.
    if t.starts_with('{') {
        for key in [r#""level":""#, r#""severity":""#, r#""lvl":""#] {
            if let Some(pos) = t.find(key) {
                if let Some(lvl) = level_word(&t[pos + key.len()..]) {
                    return lvl;
                }
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
    match word {
        "error" | "ERROR" | "Error" | "fatal" | "FATAL" | "Fatal" | "panic" | "PANIC" | "Panic"
        | "crit" | "CRIT" | "Crit" | "critical" | "CRITICAL" | "Critical" => Some("error"),

        "warn" | "WARN" | "Warn" | "warning" | "WARNING" | "Warning" => Some("warn"),

        "info" | "INFO" | "Info" => Some("info"),

        "debug" | "DEBUG" | "Debug" | "trace" | "TRACE" | "Trace" | "dbg" | "DBG" => Some("debug"),

        _ if !word.is_empty() => match word.to_ascii_lowercase().as_str() {
            "error" | "fatal" | "panic" | "crit" | "critical" => Some("error"),
            "warn" | "warning" => Some("warn"),
            "info" => Some("info"),
            "debug" | "trace" | "dbg" => Some("debug"),
            _ => None,
        },

        _ => None,
    }
}

// ANSI SGR color index → CSS class name.
const ANSI_COLORS: [&str; 16] = [
    "ansi-0",  // black
    "ansi-1",  // red
    "ansi-2",  // green
    "ansi-3",  // yellow
    "ansi-4",  // blue
    "ansi-5",  // magenta
    "ansi-6",  // cyan
    "ansi-7",  // white (light gray)
    "ansi-8",  // bright black
    "ansi-9",  // bright red
    "ansi-10", // bright green
    "ansi-11", // bright yellow
    "ansi-12", // bright blue
    "ansi-13", // bright magenta
    "ansi-14", // bright cyan
    "ansi-15", // bright white
];

/// Parse ANSI escape sequences out of `raw`, producing safe HTML with `<span
/// class="ansi-N">` for any foreground color codes (SGR 30–37, 90–97, 38;5;N,
/// 38;2;R;G;B). Bold (SGR 1) is mapped to `ansi-bold`. All other SGR attributes
/// (dim, underline, italic, etc.) are silently consumed. Unknown or malformed
/// sequences are passed through as-is (they're harmless in a `<span>` text node).
pub(crate) fn ansi_to_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut bold = false;
    let mut fg: Option<&str> = None;

    let close_if_any = |out: &mut String, bold: bool, fg: Option<&str>| {
        if fg.is_some() {
            out.push_str("</span>");
        }
        if bold {
            out.push_str("</span>");
        }
    };
    let open_if_any = |out: &mut String, bold: bool, fg: Option<&str>| {
        if bold {
            out.push_str("<span class=\"ansi-bold\">");
        }
        if let Some(c) = fg {
            out.push_str("<span class=\"");
            out.push_str(c);
            out.push_str("\">");
        }
    };

    let mut chars = raw.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch != '\x1b' {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                _ => out.push(ch),
            }
            continue;
        }
        // Look for '[' after ESC
        if chars.peek().map(|&(_, c)| c) != Some('[') {
            out.push(ch);
            continue;
        }
        chars.next(); // consume '['

        // Collect the parameter bytes and final byte of the CSI sequence.
        let seq_start = i;
        let mut params = String::new();
        let final_byte: char = loop {
            match chars.next() {
                Some((_, c)) if ('\x40'..='\x7E').contains(&c) => break c,
                Some((_, c)) => params.push(c),
                None => {
                    // Incomplete sequence at EOF — emit what we had literally.
                    out.push_str(&raw[seq_start..]);
                    close_if_any(&mut out, bold, fg);
                    return out;
                }
            }
        };

        if final_byte != 'm' {
            // Not an SGR sequence — skip it (don't emit).
            continue;
        }

        // Parse SGR parameters. Default (empty or "0") = reset.
        let param_str = params.trim_end_matches(';');
        if param_str.is_empty() || param_str == "0" {
            close_if_any(&mut out, bold, fg);
            bold = false;
            fg = None;
            continue;
        }

        let codes: Vec<u32> = param_str
            .split(';')
            .filter_map(|s| s.parse().ok())
            .collect();

        let mut ci = 0;
        while ci < codes.len() {
            match codes[ci] {
                0 => {
                    close_if_any(&mut out, bold, fg);
                    bold = false;
                    fg = None;
                }
                1 if !bold => {
                    close_if_any(&mut out, bold, fg);
                    bold = true;
                    open_if_any(&mut out, bold, fg);
                }
                2 => { /* dim — not rendered */ }
                22 if bold => {
                    close_if_any(&mut out, bold, fg);
                    bold = false;
                }
                30..=37 => {
                    close_if_any(&mut out, bold, fg);
                    fg = Some(ANSI_COLORS[(codes[ci] - 30) as usize]);
                    open_if_any(&mut out, bold, fg);
                }
                38 => {
                    // 256-color or 24-bit color
                    if ci + 1 < codes.len() && codes[ci + 1] == 5 && ci + 2 < codes.len() {
                        close_if_any(&mut out, bold, fg);
                        let idx = codes[ci + 2] as usize;
                        fg = Some(if idx < 16 {
                            ANSI_COLORS[idx]
                        } else {
                            "ansi-ext"
                        });
                        open_if_any(&mut out, bold, fg);
                        ci += 2;
                    } else if ci + 1 < codes.len() && codes[ci + 1] == 2 && ci + 4 < codes.len() {
                        close_if_any(&mut out, bold, fg);
                        fg = Some("ansi-ext");
                        open_if_any(&mut out, bold, fg);
                        ci += 4;
                    }
                }
                39 => {
                    // Default foreground — close current color span
                    close_if_any(&mut out, bold, fg);
                    fg = None;
                    open_if_any(&mut out, bold, fg);
                }
                40..=49 => { /* background colors — skip */ }
                90..=97 => {
                    close_if_any(&mut out, bold, fg);
                    fg = Some(ANSI_COLORS[(codes[ci] - 90 + 8) as usize]);
                    open_if_any(&mut out, bold, fg);
                }
                _ => { /* ignore other SGR codes */ }
            }
            ci += 1;
        }
    }

    close_if_any(&mut out, bold, fg);
    out
}

pub(crate) fn pct(used: Option<f64>, total: Option<f64>) -> f64 {
    match (used, total) {
        (Some(u), Some(t)) if t > 0.0 => (u / t * 100.0).clamp(0.0, 100.0),
        _ => 0.0,
    }
}

pub(crate) fn fmt_cores(used: Option<f64>, total: Option<f64>) -> String {
    match (used, total) {
        (Some(u), Some(t)) => format!("{u:.1} / {t:.0}"),
        (None, Some(t)) => format!("{t:.0} cores"),
        _ => "—".to_string(),
    }
}

pub(crate) fn fmt_mem(used: Option<f64>, total: Option<f64>) -> String {
    let g = |b: f64| b / (1024.0 * 1024.0 * 1024.0);
    match (used, total) {
        (Some(u), Some(t)) => format!("{:.1} / {:.1} GiB", g(u), g(t)),
        (None, Some(t)) => format!("{:.1} GiB", g(t)),
        _ => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ansi_to_html XSS escaping ---

    #[test]
    fn ansi_plain_text_passthrough() {
        assert_eq!(ansi_to_html("hello world"), "hello world");
    }

    #[test]
    fn ansi_escapes_html_special_chars() {
        assert_eq!(
            ansi_to_html("<script>alert(1)</script>"),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
        assert_eq!(ansi_to_html("a & b"), "a &amp; b");
        assert_eq!(ansi_to_html("\"quoted\""), "&quot;quoted&quot;");
    }

    #[test]
    fn ansi_color_wraps_in_span() {
        let out = ansi_to_html("\x1b[31mred\x1b[0m");
        assert!(
            out.contains("<span class=\"ansi-1\">red</span>"),
            "got: {out}"
        );
    }

    #[test]
    fn ansi_color_with_html_in_content() {
        let out = ansi_to_html("\x1b[31m<b>\x1b[0m");
        assert!(out.contains("&lt;b&gt;"), "got: {out}");
        assert!(!out.contains("<b>"), "got: {out}");
    }

    #[test]
    fn ansi_bold_wraps_in_span() {
        let out = ansi_to_html("\x1b[1mbold\x1b[0m");
        assert!(
            out.contains("<span class=\"ansi-bold\">bold</span>"),
            "got: {out}"
        );
    }

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
        assert!(p.display.contains("path=/webhook/token-review"), "got: {}", p.display);
    }

    #[test]
    fn parse_log_line_json_extra_field_with_spaces() {
        let p = parse_log_line(
            r#"{"time":"2026-06-22T20:38:29Z","level":"WARN","msg":"token review denied","reason":"invalid webhook token"}"#,
        );
        assert!(p.is_structured);
        assert!(p.display.starts_with("token review denied"), "got: {}", p.display);
        assert!(p.display.contains("reason=\"invalid webhook token\""), "got: {}", p.display);
    }

    #[test]
    fn parse_log_line_logfmt_extra_fields() {
        let p = parse_log_line(
            r#"time=2024-01-15T10:30:45Z level=info caller=main.go:42 msg="request handled" method=GET status=200"#,
        );
        assert!(p.is_structured);
        assert!(p.display.starts_with("request handled"), "got: {}", p.display);
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
}
