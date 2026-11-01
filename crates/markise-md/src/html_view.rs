//! Conservative display-only interpretation of common HTML (issue #103).
//! Never changes the CommonMark block tree or source bytes. Unsupported combinations stay raw.
use markise_doc::RawPresentation;

struct Tag<'a> {
    name: &'a str,
    closing: bool,
    open: bool,
    end: usize,
}
fn tag(s: &str) -> Option<Tag<'_>> {
    let b = s.as_bytes();
    if b.first() != Some(&b'<') {
        return None;
    }
    let closing = b.get(1) == Some(&b'/');
    let start = if closing { 2 } else { 1 };
    let mut i = start;
    while b.get(i).is_some_and(u8::is_ascii_alphanumeric) {
        i += 1;
    }
    if i == start {
        return None;
    }
    let name = &s[start..i];
    let mut open = false;
    loop {
        let ws = i;
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if b.get(i) == Some(&b'>') {
            return Some(Tag {
                name,
                closing,
                open,
                end: i + 1,
            });
        }
        if closing || i == ws {
            return None;
        }
        let a = i;
        while b
            .get(i)
            .is_some_and(|c| !c.is_ascii_whitespace() && !b"=/>\"'".contains(c))
        {
            i += 1;
        }
        if a == i {
            return None;
        }
        open |= s[a..i].eq_ignore_ascii_case("open");
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if b.get(i) == Some(&b'=') {
            i += 1;
            while b.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            if let Some(&q @ (b'\'' | b'"')) = b.get(i) {
                i += 1;
                while b.get(i).is_some_and(|c| *c != q) {
                    i += 1;
                }
                if b.get(i) != Some(&q) {
                    return None;
                }
                i += 1;
            } else {
                let a = i;
                while b.get(i).is_some_and(|c| !c.is_ascii_whitespace() && *c != b'>') {
                    i += 1;
                }
                if a == i {
                    return None;
                }
            }
        }
        // Attribute parsing already consumed the separator after a boolean attribute.
        if b.get(i) == Some(&b'>') {
            continue;
        }
        if b.get(i).is_some_and(|c| !c.is_ascii_whitespace()) {
            // Rewind over consumed whitespace so the next attribute sees its separator.
            if i > 0 && b[i - 1].is_ascii_whitespace() {
                i -= 1;
            }
        }
    }
}

pub(crate) fn keyboard_tag(s: &str) -> Option<bool> {
    let t = tag(s)?;
    (t.end == s.len() && t.name.eq_ignore_ascii_case("kbd")).then_some(t.closing)
}

fn summary(s: &str) -> Option<(String, usize)> {
    let t = tag(s)?;
    if t.closing || !t.name.eq_ignore_ascii_case("summary") {
        return None;
    }
    let mut text = String::new();
    let mut i = t.end;
    while i < s.len() {
        if s.as_bytes()[i] == b'<' {
            let t = tag(&s[i..])?;
            i += t.end;
            if t.closing && t.name.eq_ignore_ascii_case("summary") {
                let text = html_escape::decode_html_entities(&text);
                return Some((text.split_whitespace().collect::<Vec<_>>().join(" "), i));
            }
            if t.name.eq_ignore_ascii_case("summary") || t.name.eq_ignore_ascii_case("details") {
                return None;
            }
        } else {
            let c = s[i..].chars().next()?;
            text.push(c);
            i += c.len_utf8();
        }
    }
    None
}

pub(crate) fn presentation(source: &str) -> Option<RawPresentation> {
    // Display headers are bounded; unusually large raw blocks retain their segmented fallback.
    if source.len() > 32 * 1024 {
        return None;
    }
    let s = source.trim();
    if s.starts_with("<!--") {
        let mut rest = s;
        while let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.get(comment.find("-->")? + 3..)?.trim();
        }
        return rest.is_empty().then_some(RawPresentation::Comment);
    }
    let t = tag(s)?;
    if t.name.eq_ignore_ascii_case("details") && !t.closing {
        let rest = s[t.end..].trim();
        let title = if rest.is_empty() {
            String::new()
        } else {
            let (title, end) = summary(rest)?;
            if !rest[end..].trim().is_empty() {
                return None;
            }
            title
        };
        return Some(RawPresentation::DisclosureStart { title, open: t.open });
    }
    if t.name.eq_ignore_ascii_case("summary") && !t.closing {
        let (title, end) = summary(s)?;
        return s[end..]
            .trim()
            .is_empty()
            .then_some(RawPresentation::DisclosureSummary { title });
    }
    let mut rest = s;
    let mut count = 0;
    while !rest.is_empty() {
        let t = tag(rest)?;
        if !t.closing || !t.name.eq_ignore_ascii_case("details") {
            return None;
        }
        count += 1;
        rest = rest[t.end..].trim();
    }
    (count > 0).then_some(RawPresentation::DisclosureEnd { count })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_html_headers_are_conservative_and_attributes_are_boolean() {
        for src in [
            "<DETAILS OPEN>",
            "<details open='false'>",
            "<details title='>' open class=x>",
        ] {
            assert!(
                matches!(
                    presentation(src),
                    Some(RawPresentation::DisclosureStart { open: true, .. })
                ),
                "{src}"
            );
        }
        assert!(matches!(
            presentation("<details data-open='yes'>"),
            Some(RawPresentation::DisclosureStart { open: false, .. })
        ));
        for src in [
            "<details-extra>",
            "<details/><summary>x</summary>",
            "<details>lost content",
            "<!--x--><div>keep</div><!--y-->",
            "<summary>broken",
        ] {
            assert!(presentation(src).is_none(), "{src}");
        }
        assert_eq!(presentation("<!--a-->\n<!--b-->"), Some(RawPresentation::Comment));
        assert_eq!(
            presentation("</details>\n</details>"),
            Some(RawPresentation::DisclosureEnd { count: 2 })
        );
        assert_eq!(keyboard_tag("<KBD class='key'>"), Some(false));
    }
}
