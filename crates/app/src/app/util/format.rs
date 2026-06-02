//! Pure formatting / parsing helpers used across the UI.

/// Parse a resource key ("group/version/kind", group may be empty) into (group, kind).
pub(crate) fn parse_key(key: &str) -> (String, String) {
    let parts: Vec<&str> = key.splitn(3, '/').collect();
    match parts.as_slice() {
        [g, _v, k] => (g.to_string(), k.to_string()),
        _ => (String::new(), key.to_string()),
    }
}

/// "readyReplicas" -> "Ready replicas", "hostIP" -> "Host IP" (acronyms kept intact).
pub(crate) fn camel_label(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for (i, &ch) in chars.iter().enumerate() {
        if i == 0 {
            out.extend(ch.to_uppercase());
            continue;
        }
        if ch.is_uppercase() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            // Word boundary: camelCase (prev lower/digit) or end of an acronym (HTTPServer -> HTTP Server).
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push(' ');
            }
            out.push(ch);
        } else {
            out.push(ch);
        }
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

/// Classify a log line by severity for colouring. Handles klog/glog single-letter
/// prefixes (E0603…, W…, I…), `level=error`/`"level":"warn"`, and plain keywords.
pub(crate) fn log_level(line: &str) -> &'static str {
    // Aggregated workload logs are prefixed "pod │ " — classify the message itself.
    let line = line.split_once(" │ ").map(|(_, rest)| rest).unwrap_or(line);
    let t = line.trim_start();
    let mut chars = t.chars();
    if let (Some(c0), Some(c1)) = (chars.next(), chars.next()) {
        if c1.is_ascii_digit() {
            match c0 {
                'E' | 'F' => return "error",
                'W' => return "warn",
                'I' => return "info",
                'D' => return "debug",
                _ => {}
            }
        }
    }
    let l = t.to_ascii_lowercase();
    if l.contains("error") || l.contains("fatal") || l.contains("panic") || l.contains("level=error") {
        "error"
    } else if l.contains("warn") {
        "warn"
    } else if l.contains("debug") || l.contains("trace") {
        "debug"
    } else if l.contains("info") {
        "info"
    } else {
        "plain"
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
            out.push(ch);
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
                1 => {
                    if !bold {
                        close_if_any(&mut out, bold, fg);
                        bold = true;
                        open_if_any(&mut out, bold, fg);
                    }
                }
                2 => { /* dim — not rendered */ }
                22 => {
                    if bold {
                        close_if_any(&mut out, bold, fg);
                        bold = false;
                    }
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
                40..=47 | 48 | 49 => { /* background colors — skip */ }
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
