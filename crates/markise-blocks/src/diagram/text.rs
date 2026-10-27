//! Label text helpers shared by the parsers.

/// Most lines one label may have (more are dropped, the last kept line ends with `…`).
pub(crate) const MAX_LABEL_LINES: usize = 40;

/// Splits a label into display lines: `<br>`, `<br/>`, `<br />` (any case) and, when
/// `backslash_n` is set, the two characters `\n` break lines. Mermaid entity codes (`#quot;`,
/// `#35;`) and a few HTML entities are decoded; other markup is kept as text. The label is
/// truncated to `max_chars` characters.
pub(crate) fn label_lines(raw: &str, backslash_n: bool, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut count = 0usize;
    let mut truncated = false;
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if lines.len() >= MAX_LABEL_LINES {
            truncated = true;
            break;
        }
        let rest = &raw[i..];
        if let Some(n) = br_len(rest) {
            lines.push(std::mem::take(&mut cur).trim().to_string());
            i += n;
            continue;
        }
        if backslash_n && rest.starts_with("\\n") {
            lines.push(std::mem::take(&mut cur).trim().to_string());
            i += 2;
            continue;
        }
        if bytes[i] == b'\n' {
            lines.push(std::mem::take(&mut cur).trim().to_string());
            i += 1;
            continue;
        }
        let (c, n) = decode_entity(rest).unwrap_or_else(|| {
            let c = rest.chars().next().unwrap_or(' ');
            (c, c.len_utf8())
        });
        if count >= max_chars {
            truncated = true;
            break;
        }
        cur.push(if c.is_control() { ' ' } else { c });
        count += 1;
        i += n;
    }
    if lines.len() < MAX_LABEL_LINES {
        lines.push(cur.trim().to_string());
    }
    if truncated && let Some(last) = lines.last_mut() {
        last.push('\u{2026}');
    }
    // drop trailing empty lines but keep at least one line
    while lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn br_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < 4 || b[0] != b'<' || !b[1].eq_ignore_ascii_case(&b'b') || !b[2].eq_ignore_ascii_case(&b'r') {
        return None;
    }
    let mut j = 3;
    while j < b.len() && b[j] == b' ' {
        j += 1;
    }
    if j < b.len() && b[j] == b'/' {
        j += 1;
    }
    (j < b.len() && b[j] == b'>').then_some(j + 1)
}

/// `#quot;`, `#35;`, `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#35;`, `&#x23;` → (char, bytes used).
fn decode_entity(s: &str) -> Option<(char, usize)> {
    let lead = s.as_bytes().first().copied()?;
    if lead != b'#' && lead != b'&' {
        return None;
    }
    let end = s[1..].find(';').filter(|&e| e > 0 && e <= 10)? + 1;
    let name = &s[1..end];
    let c = if let Some(num) = name.strip_prefix('#') {
        if lead == b'#' {
            return None;
        }
        parse_num(num)?
    } else if name.bytes().all(|b| b.is_ascii_digit()) {
        if lead != b'#' {
            return None;
        }
        parse_num(name)?
    } else {
        match name {
            "quot" => '"',
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "apos" => '\'',
            "nbsp" => '\u{a0}',
            _ => return None,
        }
    };
    Some((c, end + 1))
}

fn parse_num(n: &str) -> Option<char> {
    let v = if let Some(h) = n.strip_prefix('x').or_else(|| n.strip_prefix('X')) {
        u32::from_str_radix(h, 16).ok()?
    } else {
        n.parse().ok()?
    };
    char::from_u32(v).filter(|c| !c.is_control())
}

/// Removes one level of surrounding double quotes and Mermaid markdown-string backticks
/// (`"`text`"`).
pub(crate) fn unquote(s: &str) -> &str {
    let t = s.trim();
    let t = if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        &t[1..t.len() - 1]
    } else {
        t
    };
    if t.len() >= 2 && t.starts_with('`') && t.ends_with('`') {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaks_and_entities() {
        assert_eq!(label_lines("a<br>b<BR/>c<br />d", false, 100), ["a", "b", "c", "d"]);
        assert_eq!(label_lines("x\\ny", true, 100), ["x", "y"]);
        assert_eq!(label_lines("x\\ny", false, 100), ["x\\ny"]);
        assert_eq!(
            label_lines("say #quot;hi#quot; &amp; #35;1", false, 100),
            ["say \"hi\" & #1"]
        );
        assert_eq!(
            label_lines("#; &; &#0; &#x110000;", false, 100),
            ["#; &; &#0; &#x110000;"]
        );
    }

    #[test]
    fn truncation() {
        assert_eq!(label_lines("abcdef", false, 3), ["abc\u{2026}"]);
        let many = "a<br>".repeat(100);
        let l = label_lines(&many, false, 10_000);
        assert_eq!(l.len(), MAX_LABEL_LINES);
        assert!(l.last().unwrap().ends_with('\u{2026}'));
    }

    #[test]
    fn unquoting() {
        assert_eq!(unquote(" \"a b\" "), "a b");
        assert_eq!(unquote("\"`md`\""), "md");
        assert_eq!(unquote("\""), "\"");
    }
}
