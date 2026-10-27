//! Mermaid `sequenceDiagram` parser (syntax per the public Mermaid documentation).
//!
//! Supported: `participant` / `actor` (with `as` aliases, `create`), messages with the arrows
//! `->`, `-->`, `->>`, `-->>`, `<<->>`, `<<-->>`, `-x`, `--x`, `-)`, `--)` and `+`/`-`
//! activation shorthands, `activate` / `deactivate`, notes (`left of`, `right of`, `over A,B`),
//! blocks `loop`, `alt`/`else`, `opt`, `par`/`and`, `critical`/`option`, `break`, `rect`,
//! `autonumber`, `title`. `box … end` groups are accepted (not drawn); `destroy`, `link(s)`,
//! `properties` and `details` are ignored.

use super::graph::parse_color;
use super::sequence::{ArrowHead, Event, NotePlace, Sequence, SequenceBuilder};
use super::text::label_lines;
use super::{DiagramError, Limits, Work, is_mermaid_noise, mermaid_body};

/// Arrows, longest first: (text, dotted, head, both ends).
const ARROWS: &[(&str, bool, ArrowHead, bool)] = &[
    ("<<-->>", true, ArrowHead::Filled, true),
    ("<<->>", false, ArrowHead::Filled, true),
    ("-->>", true, ArrowHead::Filled, false),
    ("->>", false, ArrowHead::Filled, false),
    ("--x", true, ArrowHead::Cross, false),
    ("-x", false, ArrowHead::Cross, false),
    ("--)", true, ArrowHead::Open, false),
    ("-)", false, ArrowHead::Open, false),
    ("-->", true, ArrowHead::None, false),
    ("->", false, ArrowHead::None, false),
];

pub(crate) fn parse(source: &str, limits: &Limits, work: &mut Work) -> Result<Sequence, DiagramError> {
    let mut b = SequenceBuilder::new(limits);
    let mut header = false;
    // open `end`-terminated groups: true = drawn block, false = `box`
    let mut groups: Vec<bool> = Vec::new();
    for (n, raw) in mermaid_body(source) {
        work.charge(raw.len() + 1)?;
        if is_mermaid_noise(raw) {
            continue;
        }
        let line = raw.trim().trim_end_matches(';').trim();
        if !header {
            if line.split_whitespace().next() == Some("sequenceDiagram") {
                header = true;
                continue;
            }
            return Err(DiagramError::Syntax {
                line: n,
                message: "expected `sequenceDiagram`".into(),
            });
        }
        statement(&mut b, &mut groups, line, limits).map_err(|e| match e {
            DiagramError::Syntax { line: 0, message } => DiagramError::Syntax { line: n, message },
            e => e,
        })?;
    }
    if !header {
        return Err(DiagramError::Empty);
    }
    let seq = b.finish();
    if seq.participants.is_empty() {
        return Err(DiagramError::Empty);
    }
    Ok(seq)
}

fn syn(message: String) -> DiagramError {
    DiagramError::Syntax { line: 0, message }
}

fn statement(b: &mut SequenceBuilder, groups: &mut Vec<bool>, line: &str, limits: &Limits) -> Result<(), DiagramError> {
    let max = limits.max_label_chars;
    let (word, rest) = match line.find(char::is_whitespace) {
        Some(i) => (&line[..i], line[i..].trim()),
        None => (line, ""),
    };
    let lw = word.to_ascii_lowercase();
    match lw.as_str() {
        "participant" | "actor" => {
            declare(b, rest, lw == "actor")?;
            return Ok(());
        }
        "create" => {
            let (w2, r2) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if w2 == "participant" || w2 == "actor" {
                declare(b, r2.trim(), w2 == "actor")?;
                return Ok(());
            }
            return Err(syn("expected `participant` or `actor` after `create`".into()));
        }
        "destroy" | "link" | "links" | "properties" | "details" => return Ok(()),
        "autonumber" => {
            b.seq.autonumber = rest != "off";
            return Ok(());
        }
        "activate" | "deactivate" => {
            if rest.is_empty() {
                return Err(syn(format!("`{lw}` needs a participant")));
            }
            let p = b.participant(rest, None, false, false)?;
            b.event(if lw == "activate" {
                Event::Activate(p)
            } else {
                Event::Deactivate(p)
            })?;
            return Ok(());
        }
        "note" => {
            note(b, rest)?;
            return Ok(());
        }
        "loop" | "alt" | "opt" | "par" | "critical" | "break" | "par_over" => {
            let kind = if lw == "par_over" {
                "par".to_string()
            } else {
                lw.clone()
            };
            b.event(Event::BlockStart {
                kind,
                label: label_lines(rest, false, max),
                fill: None,
            })?;
            groups.push(true);
            return Ok(());
        }
        "rect" => {
            b.event(Event::BlockStart {
                kind: "rect".into(),
                label: Vec::new(),
                fill: parse_color(rest),
            })?;
            groups.push(true);
            return Ok(());
        }
        "box" => {
            if groups.len() >= limits.max_depth {
                return Err(DiagramError::TooLarge(format!(
                    "groups nested deeper than {}",
                    limits.max_depth
                )));
            }
            groups.push(false);
            return Ok(());
        }
        "else" | "and" | "option" => {
            if groups.last() != Some(&true) {
                return Err(syn(format!("`{lw}` outside a block")));
            }
            b.event(Event::BlockSection {
                label: label_lines(rest, false, max),
            })?;
            return Ok(());
        }
        "end" if rest.is_empty() => {
            match groups.pop() {
                Some(true) => b.event(Event::BlockEnd)?,
                Some(false) => {}
                None => return Err(syn("`end` without a block".into())),
            }
            return Ok(());
        }
        _ => {}
    }
    if lw.starts_with("title") {
        let t = line[5..].trim_start_matches(':').trim();
        b.seq.title = Some(label_lines(t, false, max));
        return Ok(());
    }
    if lw.starts_with("acctitle") || lw.starts_with("accdescr") {
        return Ok(());
    }
    message(b, line)
}

fn declare(b: &mut SequenceBuilder, rest: &str, actor: bool) -> Result<(), DiagramError> {
    let rest = match rest.find("@{") {
        Some(i) => rest[..i].trim(),
        None => rest,
    };
    let (id, alias) = match rest.find(" as ") {
        Some(i) => (rest[..i].trim(), Some(rest[i + 4..].trim())),
        None => (rest.trim(), None),
    };
    if id.is_empty() {
        return Err(DiagramError::Syntax {
            line: 0,
            message: "participant needs a name".into(),
        });
    }
    b.participant(id, alias, actor, false)?;
    Ok(())
}

/// `left of A: text`, `right of A: text`, `over A[,B]: text` (after `Note`).
fn note(b: &mut SequenceBuilder, rest: &str) -> Result<(), DiagramError> {
    let Some((place, text)) = rest.split_once(':') else {
        return Err(syn("note needs `: text`".into()));
    };
    let place = place.trim();
    let lower = place.to_ascii_lowercase();
    let text = label_lines(text.trim(), false, b.limits.max_label_chars);
    let p = if let Some(who) = lower.strip_prefix("left of") {
        NotePlace::LeftOf(b.participant(place[place.len() - who.len()..].trim(), None, false, false)?)
    } else if let Some(who) = lower.strip_prefix("right of") {
        NotePlace::RightOf(b.participant(place[place.len() - who.len()..].trim(), None, false, false)?)
    } else if let Some(who) = lower.strip_prefix("over") {
        let who = &place[place.len() - who.len()..];
        let mut it = who.split(',').map(str::trim).filter(|s| !s.is_empty());
        let a = it.next().ok_or_else(|| DiagramError::Syntax {
            line: 0,
            message: "note over needs a participant".into(),
        })?;
        let a = b.participant(a, None, false, false)?;
        let c = match it.next() {
            Some(x) => b.participant(x, None, false, false)?,
            None => a,
        };
        NotePlace::Over(a, c)
    } else {
        return Err(DiagramError::Syntax {
            line: 0,
            message: "note placement must be `left of`, `right of` or `over`".into(),
        });
    };
    b.event(Event::Note { place: p, text })
}

fn message(b: &mut SequenceBuilder, line: &str) -> Result<(), DiagramError> {
    let (head_part, text) = match line.split_once(':') {
        Some((h, t)) => (h, t.trim()),
        None => (line, ""),
    };
    // first position where an arrow starts
    let found = head_part
        .char_indices()
        .find_map(|(i, _)| ARROWS.iter().find(|a| head_part[i..].starts_with(a.0)).map(|a| (i, a)));
    let Some((i, &(arrow, dotted, head, both))) = found else {
        return Err(syn(format!(
            "unexpected \u{201c}{}\u{201d}",
            line.chars().take(30).collect::<String>()
        )));
    };
    let from = head_part[..i].trim();
    let mut to = head_part[i + arrow.len()..].trim();
    let mut act = None;
    if let Some(r) = to.strip_prefix('+') {
        act = Some(true);
        to = r.trim();
    } else if let Some(r) = to.strip_prefix('-') {
        act = Some(false);
        to = r.trim();
    }
    if from.is_empty() || to.is_empty() {
        return Err(syn("message needs a sender and a receiver".into()));
    }
    let text = label_lines(text, false, b.limits.max_label_chars);
    let f = b.participant(from, None, false, false)?;
    let t = b.participant(to, None, false, false)?;
    b.event(Event::Message {
        from: f,
        to: t,
        text,
        dotted,
        head,
        both,
    })?;
    match act {
        Some(true) => b.event(Event::Activate(t))?,
        Some(false) => b.event(Event::Deactivate(f))?,
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Sequence {
        parse(s, &Limits::default(), &mut Work::unbounded()).unwrap_or_else(|e| panic!("{e}: {s}"))
    }

    #[test]
    fn participants_and_messages() {
        let s = p(
            "sequenceDiagram\n  participant A as Alice\n  actor B as Bob\n  A->>B: Hello\n  B-->>A: Hi\n  A-xC: lost\n  C-)A: async\n  A<<->>B: both",
        );
        assert_eq!(s.participants.len(), 3);
        assert_eq!(s.participants[0].label, ["Alice"]);
        assert!(s.participants[1].actor);
        assert_eq!(s.participants[2].id, "C");
        let msgs: Vec<_> = s
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Message {
                    from,
                    to,
                    text,
                    dotted,
                    head,
                    both,
                } => Some((*from, *to, text.join(""), *dotted, *head, *both)),
                _ => None,
            })
            .collect();
        assert_eq!(msgs[0], (0, 1, "Hello".into(), false, ArrowHead::Filled, false));
        assert_eq!(msgs[1], (1, 0, "Hi".into(), true, ArrowHead::Filled, false));
        assert_eq!(msgs[2].4, ArrowHead::Cross);
        assert_eq!(msgs[3].4, ArrowHead::Open);
        assert!(msgs[4].5);
    }

    #[test]
    fn activation_notes_blocks() {
        let s = p(
            "sequenceDiagram\nA->>+B: req\nB-->>-A: resp\nactivate A\ndeactivate A\nNote right of A: thinks\nNote over A,B: shared\nloop every minute\nA->B: ping\nend\nalt ok\nA->B: x\nelse fail\nA->B: y\nend\nrect rgb(200, 220, 255)\nA->B: z\nend\nautonumber\ntitle: Demo",
        );
        assert!(s.autonumber);
        assert_eq!(s.title.as_deref(), Some(&["Demo".to_string()][..]));
        assert!(s.events.contains(&Event::Activate(1)));
        assert!(s.events.contains(&Event::Deactivate(1)));
        assert!(s.events.contains(&Event::Note {
            place: NotePlace::RightOf(0),
            text: vec!["thinks".into()]
        }));
        assert!(s.events.contains(&Event::Note {
            place: NotePlace::Over(0, 1),
            text: vec!["shared".into()]
        }));
        let starts = s
            .events
            .iter()
            .filter(|e| matches!(e, Event::BlockStart { .. }))
            .count();
        let ends = s.events.iter().filter(|e| matches!(e, Event::BlockEnd)).count();
        assert_eq!((starts, ends), (3, 3));
        assert!(
            s.events
                .iter()
                .any(|e| matches!(e, Event::BlockStart { kind, fill: Some(_), .. } if kind == "rect"))
        );
    }

    #[test]
    fn boxes_are_accepted() {
        let s = p("sequenceDiagram\nbox Aqua Group\nparticipant A\nparticipant B\nend\nA->>B: hi");
        assert_eq!(s.participants.len(), 2);
        assert!(!s.events.iter().any(|e| matches!(e, Event::BlockEnd)));
    }

    #[test]
    fn errors() {
        let e = parse(
            "sequenceDiagram\nA->>B: ok\nwhat is this",
            &Limits::default(),
            &mut Work::unbounded(),
        )
        .unwrap_err();
        assert!(matches!(e, DiagramError::Syntax { line: 3, .. }), "{e:?}");
        let e = parse("sequenceDiagram\nend", &Limits::default(), &mut Work::unbounded()).unwrap_err();
        assert!(matches!(e, DiagramError::Syntax { line: 2, .. }), "{e:?}");
        let e = parse("sequenceDiagram\nelse x", &Limits::default(), &mut Work::unbounded()).unwrap_err();
        assert!(matches!(e, DiagramError::Syntax { .. }));
        assert_eq!(
            parse("sequenceDiagram\n", &Limits::default(), &mut Work::unbounded()),
            Err(DiagramError::Empty)
        );
    }

    #[test]
    fn unterminated_blocks_are_closed() {
        let s = p("sequenceDiagram\nloop x\nA->>B: hi");
        assert_eq!(s.events.last(), Some(&Event::BlockEnd));
    }
}
