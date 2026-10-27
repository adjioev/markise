//! flowchart.js syntax (fence language `flow`, spec 01 R4.6.1), per its public README:
//!
//! ```text
//! st=>start: Start|past:>http://example.com[blank]
//! op=>operation: Do it
//! cond=>condition: OK?
//! e=>end: End
//! st->op->cond
//! cond(yes)->e
//! cond(no, right)->op
//! ```
//!
//! Node types `start`, `end`, `operation`, `inputoutput`, `subroutine`, `condition`,
//! `parallel`; the `|state` and `:>link[target]` suffixes are accepted and ignored (no links in
//! diagrams). Direction hints (`right`, `left`, `top`, `bottom`) are ignored: the layered layout
//! places nodes; condition branches are labelled `yes` / `no`.

use super::graph::{Edge, Graph, GraphBuilder, Head, LineKind, Shape};
use super::text::label_lines;
use super::{DiagramError, Limits, Work};

pub fn parse(source: &str, limits: &Limits) -> Result<Graph, DiagramError> {
    parse_with(source, limits, &mut Work::unbounded())
}

pub(crate) fn parse_with(source: &str, limits: &Limits, work: &mut Work) -> Result<Graph, DiagramError> {
    let mut b = GraphBuilder::new(limits);
    for (n, raw) in source.lines().enumerate() {
        work.charge(raw.len() + 1)?;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let err = |m: String| DiagramError::Syntax {
            line: n + 1,
            message: m,
        };
        if let Some((id, def)) = line.split_once("=>") {
            let id = id.trim();
            if id.is_empty()
                || !id
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
            {
                return Err(err(format!("invalid node name \u{201c}{}\u{201d}", truncate(id))));
            }
            let (kind, text) = match def.split_once(':') {
                Some((k, t)) => (k.trim(), Some(t)),
                None => (def.trim(), None),
            };
            let shape = match kind {
                "start" | "end" => Shape::Stadium,
                "operation" | "parallel" => Shape::Rect,
                "inputoutput" => Shape::Parallelogram,
                "subroutine" => Shape::Subroutine,
                "condition" => Shape::Diamond,
                other => return Err(err(format!("unknown node type \u{201c}{}\u{201d}", truncate(other)))),
            };
            let label = text.map(strip_suffixes).unwrap_or(kind);
            let i = b.node(id, None)?;
            let node = &mut b.graph.nodes[i];
            node.shape = shape;
            node.label = label_lines(label.trim(), true, limits.max_label_chars);
        } else if line.contains("->") {
            let parts: Vec<&str> = line.split("->").map(str::trim).collect();
            let mut prev: Option<(usize, Option<&str>)> = None;
            for part in parts {
                let (id, args) = match part.find('(') {
                    Some(o) if part.ends_with(')') => (part[..o].trim(), Some(&part[o + 1..part.len() - 1])),
                    _ => (part, None),
                };
                if id.is_empty() {
                    return Err(err("missing node name in connection".into()));
                }
                let i = b.node(id, None)?;
                if let Some((from, from_args)) = prev {
                    let label = from_args
                        .and_then(|a| a.split(',').map(str::trim).find(|w| matches!(*w, "yes" | "no")))
                        .map(|w| vec![w.to_string()])
                        .unwrap_or_default();
                    b.edge(Edge {
                        from,
                        to: i,
                        label,
                        line: LineKind::Solid,
                        head: Head::Arrow,
                        tail: Head::None,
                        minlen: 1,
                        stroke: None,
                    })?;
                }
                prev = Some((i, args));
            }
        } else {
            return Err(err(format!("unexpected \u{201c}{}\u{201d}", truncate(line))));
        }
    }
    if b.graph.nodes.is_empty() {
        return Err(DiagramError::Empty);
    }
    Ok(b.finish())
}

/// Drops `:>link[target]` and `|state` suffixes from a node text.
fn strip_suffixes(t: &str) -> &str {
    let t = match t.find(":>") {
        Some(i) => &t[..i],
        None => t,
    };
    match t.rfind('|') {
        Some(i) if !t[i + 1..].contains(' ') => &t[..i],
        _ => t,
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(30).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "st=>start: Start|past:>http://www.example.com[blank]
e=>end: End:>http://www.example.com
op1=>operation: My Operation|past
sub1=>subroutine: My Subroutine|invalid
cond=>condition: Yes
or No?|approved:>http://www.example.com
io=>inputoutput: catch something...|request
para=>parallel: parallel tasks

st->op1->cond
cond(yes)->io->e
cond(no)->para
para(path1, bottom)->sub1(right)->op1
para(path2, top)->op1
";

    #[test]
    fn r4_6_1_flowchart_js_sample() {
        // the second line of `cond` is not a definition: it is reported, not guessed
        assert!(matches!(
            parse(SAMPLE, &Limits::default()),
            Err(DiagramError::Syntax { line: 6, .. })
        ));
        let src = SAMPLE.replace("Yes\nor No?", "Yes or No?");
        let g = parse(&src, &Limits::default()).unwrap();
        let node = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(node("st").label, ["Start"]);
        assert_eq!(node("st").shape, Shape::Stadium);
        assert_eq!(node("e").label, ["End"]);
        assert_eq!(node("cond").shape, Shape::Diamond);
        assert_eq!(node("cond").label, ["Yes or No?"]);
        assert_eq!(node("io").shape, Shape::Parallelogram);
        assert_eq!(node("sub1").shape, Shape::Subroutine);
        assert_eq!(g.edges.len(), 8);
        let yes = g.edges.iter().find(|e| g.nodes[e.to].id == "io").unwrap();
        assert_eq!(yes.label, ["yes"]);
        let no = g.edges.iter().find(|e| g.nodes[e.to].id == "para").unwrap();
        assert_eq!(no.label, ["no"]);
    }

    #[test]
    fn errors() {
        assert!(matches!(
            parse("x=>bogus: y", &Limits::default()),
            Err(DiagramError::Syntax { line: 1, .. })
        ));
        assert!(matches!(
            parse("a->", &Limits::default()),
            Err(DiagramError::Syntax { .. })
        ));
        assert_eq!(parse("// only comment", &Limits::default()), Err(DiagramError::Empty));
    }
}
