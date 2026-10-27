//! js-sequence-diagrams syntax (fence language `sequence`, spec 01 R4.6.1), per its public
//! README: `Title: …`, `participant A` / `participant "Long name" as A`, messages `A->B: text`
//! (solid, filled arrow), `A-->B` (dashed), `A->>B` (open arrow), `A-->>B`, notes
//! `Note left of A: …`, `Note right of A: …`, `Note over A,B: …`; `\n` breaks lines; `#` starts
//! a comment line.

use super::sequence::{ArrowHead, Event, NotePlace, Sequence, SequenceBuilder};
use super::text::{label_lines, unquote};
use super::{DiagramError, Limits, Work};

const ARROWS: &[(&str, bool, ArrowHead)] = &[
    ("-->>", true, ArrowHead::Open),
    ("->>", false, ArrowHead::Open),
    ("-->", true, ArrowHead::Filled),
    ("->", false, ArrowHead::Filled),
];

pub(crate) fn parse(source: &str, limits: &Limits, work: &mut Work) -> Result<Sequence, DiagramError> {
    let mut b = SequenceBuilder::new(limits);
    let max = limits.max_label_chars;
    for (n, raw) in source.lines().enumerate() {
        work.charge(raw.len() + 1)?;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let syn = |m: String| DiagramError::Syntax {
            line: n + 1,
            message: m,
        };
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("title") && line[5..].trim_start().starts_with(':') {
            let t = line[5..].trim_start()[1..].trim();
            b.seq.title = Some(label_lines(t, true, max));
            continue;
        }
        if let Some(rest) = lower
            .strip_prefix("participant ")
            .map(|r| &line[line.len() - r.len()..])
        {
            let rest = rest.trim();
            let (name, alias) = match rest.rfind(" as ") {
                Some(i) => (unquote(&rest[..i]), Some(rest[i + 4..].trim())),
                None => (unquote(rest), None),
            };
            match alias {
                // `participant "Long name" as A`: A is the id, the quoted text the label
                Some(id) => b.participant(id, Some(name), false, true)?,
                None => b.participant(name, None, false, true)?,
            };
            continue;
        }
        if let Some(rest) = lower.strip_prefix("note ").map(|r| &line[line.len() - r.len()..]) {
            let Some((place, text)) = rest.split_once(':') else {
                return Err(syn("note needs `: text`".into()));
            };
            let text = label_lines(text.trim(), true, max);
            let pl = place.trim();
            let pll = pl.to_ascii_lowercase();
            let who = |prefix: &str| pl[prefix.len()..].trim();
            let place = if pll.starts_with("left of") {
                NotePlace::LeftOf(b.participant(unquote(who("left of")), None, false, true)?)
            } else if pll.starts_with("right of") {
                NotePlace::RightOf(b.participant(unquote(who("right of")), None, false, true)?)
            } else if pll.starts_with("over") {
                let mut it = who("over")
                    .split(',')
                    .map(|s| unquote(s.trim()))
                    .filter(|s| !s.is_empty());
                let Some(a) = it.next() else {
                    return Err(syn("note over needs a participant".into()));
                };
                let a = b.participant(a, None, false, true)?;
                let c = match it.next() {
                    Some(x) => b.participant(x, None, false, true)?,
                    None => a,
                };
                NotePlace::Over(a, c)
            } else {
                return Err(syn("note placement must be `left of`, `right of` or `over`".into()));
            };
            b.event(Event::Note { place, text })?;
            continue;
        }
        let (head, text) = match line.split_once(':') {
            Some((h, t)) => (h, t.trim()),
            None => (line, ""),
        };
        let found = head
            .char_indices()
            .find_map(|(i, _)| ARROWS.iter().find(|a| head[i..].starts_with(a.0)).map(|a| (i, a)));
        let Some((i, &(arrow, dotted, ah))) = found else {
            return Err(syn(format!(
                "unexpected \u{201c}{}\u{201d}",
                line.chars().take(30).collect::<String>()
            )));
        };
        let from = unquote(&head[..i]);
        let to = unquote(&head[i + arrow.len()..]);
        if from.is_empty() || to.is_empty() {
            return Err(syn("message needs a sender and a receiver".into()));
        }
        let f = b.participant(from, None, false, true)?;
        let t = b.participant(to, None, false, true)?;
        b.event(Event::Message {
            from: f,
            to: t,
            text: label_lines(text, true, max),
            dotted,
            head: ah,
            both: false,
        })?;
    }
    let seq = b.finish();
    if seq.participants.is_empty() {
        return Err(DiagramError::Empty);
    }
    Ok(seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r4_6_1_js_sequence_sample() {
        let s = parse(
            "Title: Here is a title\nA->B: Normal line\nB-->C: Dashed line\nC->>D: Open arrow\nD-->>A: Dashed open arrow\nNote left of A: Note to the\\n left of A\nNote over A,B: across\nparticipant \"Long name\" as E\nE->A: hi",
            &Limits::default(),
            &mut Work::unbounded(),
        )
        .unwrap();
        assert_eq!(s.title, Some(vec!["Here is a title".to_string()]));
        assert_eq!(
            s.participants.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["A", "B", "C", "D", "E"]
        );
        assert_eq!(s.participants[4].label, ["Long name"]);
        let heads: Vec<_> = s
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Message { dotted, head, .. } => Some((*dotted, *head)),
                _ => None,
            })
            .collect();
        assert_eq!(
            &heads[..4],
            [
                (false, ArrowHead::Filled),
                (true, ArrowHead::Filled),
                (false, ArrowHead::Open),
                (true, ArrowHead::Open)
            ]
        );
        assert!(s.events.contains(&Event::Note {
            place: NotePlace::LeftOf(0),
            text: vec!["Note to the".into(), "left of A".into()]
        }));
        assert!(s.events.contains(&Event::Note {
            place: NotePlace::Over(0, 1),
            text: vec!["across".into()]
        }));
    }

    #[test]
    fn errors() {
        assert!(matches!(
            parse("A->B: ok\nnonsense", &Limits::default(), &mut Work::unbounded()),
            Err(DiagramError::Syntax { line: 2, .. })
        ));
        assert!(matches!(
            parse("->B: x", &Limits::default(), &mut Work::unbounded()),
            Err(DiagramError::Syntax { line: 1, .. })
        ));
        assert_eq!(
            parse("# comment", &Limits::default(), &mut Work::unbounded()),
            Err(DiagramError::Empty)
        );
    }
}
