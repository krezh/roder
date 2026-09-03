//! ANSI SGR escape codes → safe HTML `<span>`s, for rendering colored log output.

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

pub(crate) fn strip_ansi(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' || chars.peek() != Some(&'[') {
            out.push(ch);
            continue;
        }
        chars.next();
        for ch in chars.by_ref() {
            if ('\x40'..='\x7e').contains(&ch) {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn strips_sgr_for_text_parsing() {
        assert_eq!(strip_ansi("\x1b[32mINFO\x1b[0m ready"), "INFO ready");
    }
}
