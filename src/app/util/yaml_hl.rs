//! Minimal YAML syntax highlighter for the detail viewer.
//! Returns an HTML string with `<span class="y-*">` tokens safe for `inner_html`.

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn sp(cls: &str, content: &str) -> String {
    format!("<span class=\"y-{cls}\">{}</span>", esc(content))
}

pub(crate) fn highlight_yaml(yaml: &str) -> String {
    let mut out = String::with_capacity(yaml.len() * 2);
    for (i, line) in yaml.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&highlight_line(line));
    }
    out
}

fn highlight_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();
    let indent = &line[..indent_len];
    let mut out = esc(indent);

    if trimmed.is_empty() {
        return out;
    }

    if trimmed.starts_with('#') {
        out.push_str(&sp("comment", trimmed));
        return out;
    }

    if trimmed == "---" || trimmed == "..." {
        out.push_str(&sp("doc", trimmed));
        return out;
    }

    // Strip list-item prefix "- "
    let rest = if let Some(r) = trimmed.strip_prefix("- ") {
        out.push_str(&sp("list", "-"));
        out.push(' ');
        r
    } else if trimmed == "-" {
        out.push_str(&sp("list", "-"));
        return out;
    } else {
        trimmed
    };

    // Key: value
    if let Some(colon_pos) = find_key_colon(rest) {
        let key = &rest[..colon_pos];
        let after = &rest[colon_pos + 1..];

        out.push_str(&sp("key", key));
        out.push_str(&sp("colon", ":"));

        if let Some(val) = after.strip_prefix(' ') {
            out.push(' ');
            if !val.is_empty() {
                out.push_str(&highlight_value(val));
            }
        }
        // after is empty → nested mapping, nothing to append
    } else {
        // Bare scalar (list continuation, block literal body, etc.)
        out.push_str(&highlight_value(rest));
    }

    out
}

fn highlight_value(v: &str) -> String {
    if v.is_empty() {
        return String::new();
    }

    // Split off trailing inline comment (` #...` not inside quotes)
    if let Some(comment_at) = find_inline_comment(v) {
        let val_part = v[..comment_at].trim_end();
        let comment_part = &v[comment_at..];
        let mut result = highlight_scalar(val_part);
        result.push(' ');
        result.push_str(&sp("comment", comment_part));
        return result;
    }

    highlight_scalar(v)
}

fn highlight_scalar(v: &str) -> String {
    if v.is_empty() {
        return String::new();
    }

    // Block scalars
    if matches!(v, "|" | ">" | "|-" | ">-" | "|+" | ">+") {
        return sp("block", v);
    }

    // Anchors & aliases
    if v.starts_with('&') || v.starts_with('*') {
        return sp("anchor", v);
    }

    // Quoted strings
    let len = v.len();
    if len >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        return sp("str", v);
    }

    // Integers and floats
    if v.parse::<i64>().is_ok() || v.parse::<f64>().is_ok() {
        return sp("num", v);
    }

    // Booleans
    match v {
        "true" | "false" | "True" | "False" | "TRUE" | "FALSE" | "yes" | "no" | "Yes" | "No"
        | "YES" | "NO" | "on" | "off" | "On" | "Off" | "ON" | "OFF" => return sp("bool", v),
        "null" | "Null" | "NULL" | "~" => return sp("null", v),
        _ => {}
    }

    sp("plain", v)
}

// First `:` not inside quotes that is followed by a space or end-of-string.
fn find_key_colon(s: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    for (i, c) in s.char_indices() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ':' if !in_single && !in_double => {
                let next = s[i + 1..].chars().next();
                if next.is_none() || next == Some(' ') {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

// Returns the byte position of a ` #` inline comment not inside quotes.
fn find_inline_comment(s: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b' ' if !in_single && !in_double && i + 1 < b.len() && b[i + 1] == b'#' => {
                return Some(i + 1);
            }
            _ => {}
        }
        i += 1;
    }
    None
}
