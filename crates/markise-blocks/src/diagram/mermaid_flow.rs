//! Mermaid `flowchart` / `graph` parser (syntax per the public Mermaid documentation).
//!
//! Supported: direction; nodes with the classic shapes (`[ ]`, `( )`, `([ ])`, `[[ ]]`,
//! `[( )]`, `(( ))`, `((( )))`, `> ]`, `{ }`, `{{ }}`, `[/ /]`, `[\ \]`, `[/ \]`, `[\ /]`) and
//! the `id@{ shape: …, label: "…" }` form for the same shapes; quoted and markdown-string
//! labels; links `-->`, `---`, `-.->`, `-.-`, `==>`, `===`, `~~~`, `--o`, `--x`, `<-->`,
//! `o--o`, `x--x`, longer variants (more ranks), labels as `-- text -->`, `-. text .->`,
//! `== text ==>` and `-->|text|`; chains (`A --> B --> C`) and `&` groups; `subgraph … end`
//! (nested); `classDef`, `class`, `:::class`, `style`, `linkStyle` (colors only).
//! Ignored: `click` (no scripts or links in diagrams), `direction` inside subgraphs,
//! accessibility statements, icons and images.

use std::collections::HashMap;

use super::graph::{Direction, Edge, GraphBuilder, Head, LineKind, NodeStyle, Shape, parse_color, parse_style};
use super::text::{label_lines, unquote};
use super::{DiagramError, Limits, Work, is_mermaid_noise, mermaid_body};

/// Longest class name kept (longer names are ignored).
const MAX_CLASS_NAME: usize = 256;
/// Distinct class names (`classDef` and uses together).
const MAX_CLASSES: usize = 512;
/// Classes one node can carry.
const MAX_CLASSES_PER_NODE: usize = 8;

pub(crate) fn parse(source: &str, limits: &Limits, work: &mut Work) -> Result<super::graph::Graph, DiagramError> {
    let mut p = Parser {
        b: GraphBuilder::new(limits),
        stack: Vec::new(),
        class_defs: HashMap::new(),
        class_ids: HashMap::new(),
        node_classes: Vec::new(),
        header_seen: false,
        line: 0,
        work,
    };
    for (n, line) in mermaid_body(source) {
        p.work.charge(line.len() + 1)?;
        if is_mermaid_noise(line) {
            continue;
        }
        p.line = n;
        let line = strip_trailing_comment(line);
        for stmt in split_statements(line) {
            let stmt = stmt.trim();
            if !stmt.is_empty() {
                p.statement(stmt)?;
            }
        }
    }
    if !p.header_seen {
        return Err(DiagramError::Empty);
    }
    // classes are applied last so `classDef` may follow its use
    for (node, classes) in std::mem::take(&mut p.node_classes).into_iter().enumerate() {
        for c in classes {
            if let Some(st) = p.class_defs.get(&c)
                && node < p.b.graph.nodes.len()
            {
                p.b.graph.nodes[node].style.merge(st);
            }
        }
    }
    if let Some(st) = p.class_ids.get("default").and_then(|c| p.class_defs.get(c)).copied() {
        for n in &mut p.b.graph.nodes {
            let mut s = st;
            s.merge(&n.style);
            n.style = s;
        }
    }
    Ok(p.b.finish())
}

struct Parser<'l, 'w, 'c> {
    b: GraphBuilder<'l>,
    /// Open subgraphs (innermost last).
    stack: Vec<usize>,
    /// `classDef` styles by interned class id.
    class_defs: HashMap<u32, NodeStyle>,
    /// Interned class names (bounded by [`MAX_CLASSES`], names by [`MAX_CLASS_NAME`]).
    class_ids: HashMap<String, u32>,
    /// Class ids per node index (deduplicated, at most [`MAX_CLASSES_PER_NODE`]).
    node_classes: Vec<Vec<u32>>,
    header_seen: bool,
    line: usize,
    work: &'w mut Work<'c>,
}

/// `%%` starts a comment only at the beginning of a line in Mermaid; trailing `%%` comments are
/// tolerated outside quotes.
fn strip_trailing_comment(line: &str) -> &str {
    let mut in_quote = false;
    let b = line.as_bytes();
    for i in 0..b.len() {
        match b[i] {
            b'"' => in_quote = !in_quote,
            b'%' if !in_quote && b.get(i + 1) == Some(&b'%') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Splits on `;` outside quotes and brackets.
fn split_statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut in_quote, mut start) = (0i32, false, 0usize);
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            '[' | '(' | '{' if !in_quote => depth += 1,
            ']' | ')' | '}' if !in_quote => depth = (depth - 1).max(0),
            ';' if !in_quote && depth == 0 => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&line[start..]);
    out
}

impl Parser<'_, '_, '_> {
    fn err(&self, message: impl Into<String>) -> DiagramError {
        DiagramError::Syntax {
            line: self.line,
            message: message.into(),
        }
    }

    fn statement(&mut self, s: &str) -> Result<(), DiagramError> {
        let (word, rest) = split_word(s);
        if !self.header_seen {
            let lw = word.to_ascii_lowercase();
            if lw == "flowchart" || lw == "graph" || lw == "flowchart-elk" {
                self.header_seen = true;
                let (dir, after) = split_word(rest);
                if let Some(d) = Direction::parse(dir) {
                    self.b.graph.direction = d;
                    if !after.trim().is_empty() {
                        return self.statement(after.trim());
                    }
                } else if !rest.trim().is_empty() {
                    return self.statement(rest.trim());
                }
                return Ok(());
            }
            return Err(self.err("expected `flowchart` or `graph`"));
        }
        match word {
            "subgraph" => return self.subgraph(rest),
            "end" if rest.trim().is_empty() => {
                return if self.stack.pop().is_some() {
                    Ok(())
                } else {
                    Err(self.err("`end` without `subgraph`"))
                };
            }
            "direction" => return Ok(()),
            "classDef" => {
                let (names, style) = split_word(rest);
                let st = parse_style(style);
                for n in names.split(',').filter(|n| !n.is_empty()) {
                    self.work.charge(n.len() + 1)?;
                    if let Some(c) = self.class_id(n) {
                        self.class_defs.insert(c, st);
                    }
                }
                return Ok(());
            }
            "class" => {
                let (ids, class) = split_word(rest);
                let Some(c) = self.class_id(class.trim()) else {
                    return Ok(());
                };
                for id in ids.split(',').filter(|n| !n.is_empty()) {
                    self.work.charge(id.len() + 1)?;
                    if let Some(&i) = self.b.index.get(id) {
                        self.add_class(i, c);
                    }
                }
                return Ok(());
            }
            "style" => {
                let (id, style) = split_word(rest);
                if let Some(&i) = self.b.index.get(id) {
                    let st = parse_style(style);
                    self.b.graph.nodes[i].style.merge(&st);
                }
                return Ok(());
            }
            "linkStyle" => {
                let (which, style) = split_word(rest);
                let stroke = style.split(',').find_map(|d| {
                    let (k, v) = d.split_once(':')?;
                    (k.trim() == "stroke").then(|| parse_color(v)).flatten()
                });
                let n = self.b.graph.edges.len();
                let mut default_done = false;
                for w in which.split(',') {
                    self.work.charge(w.len() + 1)?;
                    if w == "default" {
                        if !default_done {
                            default_done = true;
                            self.work.charge(n)?;
                            self.b
                                .graph
                                .edges
                                .iter_mut()
                                .for_each(|e| e.stroke = e.stroke.or(stroke));
                        }
                    } else if let Ok(i) = w.parse::<usize>()
                        && i < n
                    {
                        self.b.graph.edges[i].stroke = stroke;
                    }
                }
                return Ok(());
            }
            "click" | "accTitle" | "accTitle:" | "accDescr" | "accDescr:" => return Ok(()),
            "title" => {
                self.b.graph.title = Some(rest.trim().chars().take(self.b.limits.max_label_chars).collect());
                return Ok(());
            }
            _ => {}
        }
        if word.starts_with("accTitle") || word.starts_with("accDescr") {
            return Ok(());
        }
        self.chain(s)
    }

    /// The interned id of class `name`; `None` for over-long names or past [`MAX_CLASSES`].
    fn class_id(&mut self, name: &str) -> Option<u32> {
        if name.is_empty() || name.len() > MAX_CLASS_NAME {
            return None;
        }
        if let Some(&c) = self.class_ids.get(name) {
            return Some(c);
        }
        if self.class_ids.len() >= MAX_CLASSES {
            return None;
        }
        let c = self.class_ids.len() as u32;
        self.class_ids.insert(name.to_string(), c);
        Some(c)
    }

    /// Records that node `i` has class `c` (deduplicated, bounded per node).
    fn add_class(&mut self, i: usize, c: u32) {
        if self.node_classes.len() <= i {
            self.node_classes.resize_with(i + 1, Vec::new);
        }
        let v = &mut self.node_classes[i];
        if !v.contains(&c) && v.len() < MAX_CLASSES_PER_NODE {
            v.push(c);
        }
    }

    /// Whether cluster `a` is a (strict) ancestor of cluster `c`.
    fn is_ancestor(&self, a: usize, c: usize) -> bool {
        let mut p = self.b.graph.clusters[c].parent;
        let mut steps = 0;
        while let Some(x) = p {
            if x == a {
                return true;
            }
            steps += 1;
            if steps > self.b.limits.max_depth + 1 {
                return false;
            }
            p = self.b.graph.clusters[x].parent;
        }
        false
    }

    fn subgraph(&mut self, rest: &str) -> Result<(), DiagramError> {
        if self.stack.len() >= self.b.limits.max_depth {
            return Err(DiagramError::TooLarge(format!(
                "subgraphs nested deeper than {}",
                self.b.limits.max_depth
            )));
        }
        let rest = rest.trim();
        let max = self.b.limits.max_label_chars;
        let (id, title) = if rest.starts_with('"') {
            let t = unquote(rest);
            (t.to_string(), label_lines(t, false, max))
        } else if let Some(open) = rest.find('[').filter(|_| rest.ends_with(']')) {
            let id = rest[..open].trim();
            let t = unquote(&rest[open + 1..rest.len() - 1]);
            (id.to_string(), label_lines(t, false, max))
        } else {
            (rest.to_string(), label_lines(rest, false, max))
        };
        let id = if id.is_empty() {
            format!("subgraph{}", self.b.graph.clusters.len())
        } else {
            id
        };
        let parent = self.stack.last().copied();
        let c = self.b.cluster(id, title, parent)?;
        self.stack.push(c);
        Ok(())
    }

    fn chain(&mut self, s: &str) -> Result<(), DiagramError> {
        let mut cur = Cursor { s, pos: 0 };
        let mut left = self.node_group(&mut cur)?;
        loop {
            cur.skip_ws();
            if cur.done() {
                return Ok(());
            }
            let Some(mut link) = parse_link(&mut cur) else {
                return Err(self.err(format!(
                    "unexpected \u{201c}{}\u{201d}",
                    cur.rest().chars().take(20).collect::<String>()
                )));
            };
            cur.skip_ws();
            if cur.peek() == Some('|') {
                cur.bump();
                let Some(end) = cur.rest().find('|') else {
                    return Err(self.err("unterminated `|label|`"));
                };
                link.label = Some(cur.rest()[..end].to_string());
                cur.pos += end + 1;
            }
            cur.skip_ws();
            let right = self.node_group(&mut cur)?;
            let label = link
                .label
                .as_deref()
                .map(|l| label_lines(unquote(l), false, self.b.limits.max_label_chars))
                .unwrap_or_default();
            self.work.charge(left.len().saturating_mul(right.len()))?;
            for &a in &left {
                for &b in &right {
                    self.b.edge(Edge {
                        from: a,
                        to: b,
                        label: label.clone(),
                        line: link.line,
                        head: link.head,
                        tail: link.tail,
                        minlen: link.minlen,
                        stroke: None,
                    })?;
                }
            }
            left = right;
        }
    }

    fn node_group(&mut self, cur: &mut Cursor) -> Result<Vec<usize>, DiagramError> {
        let mut out = vec![self.node(cur)?];
        loop {
            let save = cur.pos;
            cur.skip_ws();
            if cur.peek() == Some('&') {
                cur.bump();
                cur.skip_ws();
                out.push(self.node(cur)?);
                if out.len() > self.b.limits.max_nodes {
                    return Err(DiagramError::TooLarge("node group".into()));
                }
            } else {
                cur.pos = save;
                return Ok(out);
            }
        }
    }

    fn node(&mut self, cur: &mut Cursor) -> Result<usize, DiagramError> {
        let id = cur.take_id();
        if id.is_empty() {
            let near: String = cur.rest().chars().take(20).collect();
            return Err(self.err(if near.is_empty() {
                "expected a node".to_string()
            } else {
                format!("expected a node at \u{201c}{near}\u{201d}")
            }));
        }
        let cluster = self.stack.last().copied();
        let i = self.b.node(id, cluster)?;
        // a node mentioned inside a subgraph belongs to it (innermost wins over its ancestors)
        if let Some(c) = cluster {
            let cur = self.b.graph.nodes[i].cluster;
            if cur.is_none_or(|old| self.is_ancestor(old, c)) {
                self.b.graph.nodes[i].cluster = Some(c);
            }
        }
        let max = self.b.limits.max_label_chars;
        if cur.rest().starts_with("@{") {
            let Some(end) = cur.rest().find('}') else {
                return Err(self.err("unterminated `@{`"));
            };
            let body = &cur.rest()[2..end];
            cur.pos += end + 1;
            for (k, v) in split_props(body) {
                match k {
                    "shape" => self.b.graph.nodes[i].shape = named_shape(v),
                    "label" => self.b.graph.nodes[i].label = label_lines(unquote(v), false, max),
                    _ => {}
                }
            }
        } else if let Some((shape, open, close)) = shape_open(cur.rest()) {
            cur.pos += open;
            let text = if cur.rest().trim_start().starts_with('"') {
                // quoted: up to the closing quote, then the closer
                let lead = cur.rest().len() - cur.rest().trim_start().len();
                let r = &cur.rest()[lead + 1..];
                let Some(q) = r.find('"') else {
                    return Err(self.err("unterminated string"));
                };
                let t = r[..q].to_string();
                cur.pos += lead + 1 + q + 1;
                cur.skip_ws();
                if !cur.rest().starts_with(close) {
                    return Err(self.err(format!("expected `{close}`")));
                }
                cur.pos += close.len();
                t
            } else {
                let Some(end) = find_closer(cur.rest(), close) else {
                    return Err(self.err(format!("expected `{close}`")));
                };
                let t = cur.rest()[..end].to_string();
                cur.pos += end + close.len();
                t
            };
            let node = &mut self.b.graph.nodes[i];
            node.shape = shape;
            node.label = label_lines(unquote(&text), false, max);
        }
        if cur.rest().starts_with(":::") {
            cur.pos += 3;
            let class = cur.take_id();
            if let Some(c) = self.class_id(class) {
                self.add_class(i, c);
            }
        }
        Ok(i)
    }
}

/// Finds `close` in `s`, skipping it inside double quotes.
fn find_closer(s: &str, close: &str) -> Option<usize> {
    let mut in_quote = false;
    for (i, c) in s.char_indices() {
        if c == '"' {
            in_quote = !in_quote;
        } else if !in_quote && s[i..].starts_with(close) {
            return Some(i);
        }
    }
    None
}

/// Shape opener at the start of `s`: (shape, opener length, closer).
fn shape_open(s: &str) -> Option<(Shape, usize, &'static str)> {
    const OPENERS: &[(&str, Shape, &str)] = &[
        ("(((", Shape::DoubleCircle, ")))"),
        ("((", Shape::Circle, "))"),
        ("([", Shape::Stadium, "])"),
        ("[(", Shape::Cylinder, ")]"),
        ("[[", Shape::Subroutine, "]]"),
        ("{{", Shape::Hexagon, "}}"),
        ("(", Shape::Round, ")"),
        ("{", Shape::Diamond, "}"),
        (">", Shape::Asymmetric, "]"),
    ];
    for &(o, sh, c) in OPENERS {
        if s.starts_with(o) {
            return Some((sh, o.len(), c));
        }
    }
    if let Some(r) = s.strip_prefix("[/").or_else(|| s.strip_prefix("[\\")) {
        let slash = s.as_bytes()[1] == b'/';
        // closer decides between parallelogram and trapezoid
        let fwd = r.find("/]");
        let back = r.find("\\]");
        let (shape, close) = match (fwd, back) {
            (Some(f), Some(b)) if f < b => (
                if slash {
                    Shape::Parallelogram
                } else {
                    Shape::TrapezoidAlt
                },
                "/]",
            ),
            (Some(_), None) => (
                if slash {
                    Shape::Parallelogram
                } else {
                    Shape::TrapezoidAlt
                },
                "/]",
            ),
            (_, Some(_)) => (
                if slash {
                    Shape::Trapezoid
                } else {
                    Shape::ParallelogramAlt
                },
                "\\]",
            ),
            (None, None) => (Shape::Rect, "]"),
        };
        if shape == Shape::Rect {
            return Some((Shape::Rect, 1, "]"));
        }
        return Some((shape, 2, close));
    }
    if s.starts_with('[') {
        return Some((Shape::Rect, 1, "]"));
    }
    None
}

fn named_shape(v: &str) -> Shape {
    match unquote(v).to_ascii_lowercase().as_str() {
        "rounded" | "event" | "round" => Shape::Round,
        "stadium" | "pill" | "terminal" => Shape::Stadium,
        "subroutine" | "subproc" | "fr-rect" | "framed-rectangle" | "subprocess" => Shape::Subroutine,
        "cyl" | "cylinder" | "database" | "db" => Shape::Cylinder,
        "circle" | "circ" | "sm-circ" | "small-circle" | "start" => Shape::Circle,
        "dbl-circ" | "double-circle" | "fr-circ" | "framed-circle" | "stop" => Shape::DoubleCircle,
        "diam" | "diamond" | "decision" | "question" => Shape::Diamond,
        "hex" | "hexagon" | "prepare" => Shape::Hexagon,
        "lean-r" | "lean-right" | "in-out" => Shape::Parallelogram,
        "lean-l" | "lean-left" | "out-in" => Shape::ParallelogramAlt,
        "trap-b" | "trapezoid-bottom" | "priority" | "trapezoid" => Shape::Trapezoid,
        "trap-t" | "trapezoid-top" | "manual" | "inv-trapezoid" => Shape::TrapezoidAlt,
        "odd" | "asymmetric" => Shape::Asymmetric,
        _ => Shape::Rect,
    }
}

/// `key: value, key: "value, with comma"` pairs of an `@{ … }` body.
fn split_props(body: &str) -> Vec<(&str, &str)> {
    let mut parts = Vec::new();
    let (mut in_quote, mut start) = (false, 0usize);
    for (i, c) in body.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            ',' if !in_quote => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[start..]);
    parts
        .into_iter()
        .filter_map(|p| p.split_once(':').map(|(k, v)| (k.trim(), v.trim())))
        .collect()
}

fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

struct Cursor<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }
    fn done(&self) -> bool {
        self.pos >= self.s.len()
    }
    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }
    fn bump(&mut self) {
        if let Some(c) = self.peek() {
            self.pos += c.len_utf8();
        }
    }
    fn skip_ws(&mut self) {
        let r = self.rest();
        self.pos += r.len() - r.trim_start().len();
    }
    /// A node id: letters, digits, `_`, and `-`/`.` between such characters.
    fn take_id(&mut self) -> &'a str {
        let r = self.rest();
        let mut end = 0;
        let mut it = r.char_indices().peekable();
        while let Some((i, c)) = it.next() {
            let next = it.peek().map(|&(_, n)| n);
            let ok = c.is_alphanumeric()
                || c == '_'
                || ((c == '-' || c == '.') && end > 0 && next.is_some_and(|n| n.is_alphanumeric() || n == '_'));
            if !ok {
                break;
            }
            end = i + c.len_utf8();
        }
        self.pos += end;
        &r[..end]
    }
}

#[derive(Debug)]
struct Link {
    line: LineKind,
    head: Head,
    tail: Head,
    minlen: usize,
    label: Option<String>,
}

fn head_of(c: Option<char>) -> Option<Head> {
    match c {
        Some('>') => Some(Head::Arrow),
        Some('o') => Some(Head::Circle),
        Some('x') => Some(Head::Cross),
        _ => None,
    }
}

/// Parses a link at the cursor (after whitespace); `None` if there is none.
fn parse_link(cur: &mut Cursor) -> Option<Link> {
    let start = cur.pos;
    let r = cur.rest();
    let b = r.as_bytes();
    let mut tail = Head::None;
    let mut i = 0usize;
    if b.len() > 1 && matches!(b[1], b'-' | b'=') {
        match b[0] {
            b'<' => {
                tail = Head::Arrow;
                i = 1;
            }
            b'o' => {
                tail = Head::Circle;
                i = 1;
            }
            b'x' => {
                tail = Head::Cross;
                i = 1;
            }
            _ => {}
        }
    }
    let rest = &r[i..];
    if rest.starts_with("~~~") {
        let n = rest.bytes().take_while(|&c| c == b'~').count();
        cur.pos += i + n;
        return Some(Link {
            line: LineKind::Invisible,
            head: Head::None,
            tail: Head::None,
            minlen: (n - 2).max(1),
            label: None,
        });
    }
    let first = rest.as_bytes().first().copied()?;
    if first != b'-' && first != b'=' {
        cur.pos = start;
        return None;
    }
    let ch = first as char;
    let thick = first == b'=';
    // dotted: `-.` `.`* `-`?
    if !thick && rest.starts_with("-.") {
        let dots = rest[1..].bytes().take_while(|&c| c == b'.').count();
        let mut j = 1 + dots;
        let after = &rest[j..];
        if after.starts_with('-') {
            j += 1;
            let head = head_of(rest[j..].chars().next());
            if head.is_some() {
                j += 1;
            }
            cur.pos += i + j;
            return Some(Link {
                line: LineKind::Dotted,
                head: head.unwrap_or_default(),
                tail,
                minlen: dots.max(1),
                label: None,
            });
        }
        if dots == 1 && after.starts_with(char::is_whitespace) {
            // `-. text .->` / `-. text .-`
            let close = after.find(".-")?;
            let label = after[..close].trim().to_string();
            let mut j2 = j + close + 2;
            let head = head_of(rest[j2..].chars().next());
            if head.is_some() {
                j2 += 1;
            }
            cur.pos += i + j2;
            return Some(Link {
                line: LineKind::Dotted,
                head: head.unwrap_or_default(),
                tail,
                minlen: 1,
                label: Some(label),
            });
        }
        cur.pos = start;
        return None;
    }
    let n = rest.bytes().take_while(|&c| c == first).count();
    if n < 2 {
        cur.pos = start;
        return None;
    }
    let line = if thick { LineKind::Thick } else { LineKind::Solid };
    let after = &rest[n..];
    if let Some(head) = head_of(after.chars().next()) {
        // `o`/`x` heads must not swallow a following id (`--oB` is still circle + B)
        cur.pos += i + n + 1;
        return Some(Link {
            line,
            head,
            tail,
            minlen: (n - 1).max(1),
            label: None,
        });
    }
    if n == 2 && after.starts_with(char::is_whitespace) && tail == Head::None {
        // `-- text -->` / `== text ==>`: text up to the next run of the same character
        let pat: String = std::iter::repeat_n(ch, 2).collect();
        let close = after.find(&pat)?;
        let label = after[..close].trim().to_string();
        if label.is_empty() {
            cur.pos += i + n;
            return Some(Link {
                line,
                head: Head::None,
                tail,
                minlen: 1,
                label: None,
            });
        }
        let tail_run = after[close..].bytes().take_while(|&c| c == first).count();
        let mut j = n + close + tail_run;
        let head = head_of(rest[j..].chars().next());
        if head.is_some() {
            j += 1;
        }
        cur.pos += i + j;
        let extra = if head.is_some() {
            tail_run.saturating_sub(1)
        } else {
            tail_run.saturating_sub(2)
        };
        return Some(Link {
            line,
            head: head.unwrap_or_default(),
            tail,
            minlen: extra.max(1),
            label: Some(label),
        });
    }
    if n >= 3 {
        cur.pos += i + n;
        return Some(Link {
            line,
            head: Head::None,
            tail,
            minlen: (n - 2).max(1),
            label: None,
        });
    }
    cur.pos = start;
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::graph::Graph;

    fn p(s: &str) -> Graph {
        parse(s, &Limits::default(), &mut Work::unbounded()).unwrap_or_else(|e| panic!("{e}: {s}"))
    }

    fn edge_ids(g: &Graph) -> Vec<(String, String)> {
        g.edges
            .iter()
            .map(|e| (g.nodes[e.from].id.clone(), g.nodes[e.to].id.clone()))
            .collect()
    }

    #[test]
    fn header_and_direction() {
        assert_eq!(p("graph LR\nA-->B").direction, Direction::LeftRight);
        assert_eq!(p("flowchart TD;A-->B;B-->C").edges.len(), 2);
        assert_eq!(p("flowchart\nA").direction, Direction::TopDown);
        assert_eq!(p("graph RL; A --> B").direction, Direction::RightLeft);
    }

    #[test]
    fn shapes_and_labels() {
        let g = p(
            "flowchart TD\nA[Box] --> B(Round) --> C{Decide?}\nD((Circle)) --- E([Stadium])\nF[[Sub]]\nG[(DB)]\nH>Flag]\nI{{Hex}}\nJ[/In/]\nK[\\Out\\]\nL[/Trap\\]\nM[\\Inv/]\nN(((Dbl)))\nO[\"Quoted [x]\"]",
        );
        let shape = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap().shape;
        let label = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap().label.join("\n");
        assert_eq!(shape("A"), Shape::Rect);
        assert_eq!(shape("B"), Shape::Round);
        assert_eq!(shape("C"), Shape::Diamond);
        assert_eq!(label("C"), "Decide?");
        assert_eq!(shape("D"), Shape::Circle);
        assert_eq!(shape("E"), Shape::Stadium);
        assert_eq!(shape("F"), Shape::Subroutine);
        assert_eq!(shape("G"), Shape::Cylinder);
        assert_eq!(shape("H"), Shape::Asymmetric);
        assert_eq!(shape("I"), Shape::Hexagon);
        assert_eq!(shape("J"), Shape::Parallelogram);
        assert_eq!(shape("K"), Shape::ParallelogramAlt);
        assert_eq!(shape("L"), Shape::Trapezoid);
        assert_eq!(shape("M"), Shape::TrapezoidAlt);
        assert_eq!(shape("N"), Shape::DoubleCircle);
        assert_eq!(label("O"), "Quoted [x]");
    }

    #[test]
    fn new_shape_syntax() {
        let g = p("flowchart LR\nA@{ shape: diam, label: \"Is it, really?\" } --> B@{shape: cyl}");
        assert_eq!(g.nodes[0].shape, Shape::Diamond);
        assert_eq!(g.nodes[0].label, ["Is it, really?"]);
        assert_eq!(g.nodes[1].shape, Shape::Cylinder);
    }

    #[test]
    fn link_kinds() {
        let g = p(
            "graph TD\nA-->B\nA---C\nA-.->D\nA-.-E\nA==>F\nA===G\nA~~~H\nA--oI\nA--xJ\nA<-->K\nA o--o L\nA x--x M\nA---->N",
        );
        let e = |to: &str| g.edges.iter().find(|e| g.nodes[e.to].id == to).unwrap().clone();
        assert_eq!((e("B").line, e("B").head), (LineKind::Solid, Head::Arrow));
        assert_eq!((e("C").line, e("C").head), (LineKind::Solid, Head::None));
        assert_eq!((e("D").line, e("D").head), (LineKind::Dotted, Head::Arrow));
        assert_eq!((e("E").line, e("E").head), (LineKind::Dotted, Head::None));
        assert_eq!((e("F").line, e("F").head), (LineKind::Thick, Head::Arrow));
        assert_eq!((e("G").line, e("G").head), (LineKind::Thick, Head::None));
        assert_eq!(e("H").line, LineKind::Invisible);
        assert_eq!(e("I").head, Head::Circle);
        assert_eq!(e("J").head, Head::Cross);
        assert_eq!((e("K").tail, e("K").head), (Head::Arrow, Head::Arrow));
        assert_eq!((e("L").tail, e("L").head), (Head::Circle, Head::Circle));
        assert_eq!((e("M").tail, e("M").head), (Head::Cross, Head::Cross));
        assert_eq!(e("N").minlen, 3);
        assert_eq!(e("B").minlen, 1);
    }

    #[test]
    fn link_labels() {
        let g = p("graph TD\nA-- yes -->B\nA-->|no|C\nA-. maybe .->D\nA== sure ==>E\nA -- plain --- F");
        let l = |to: &str| g.edges.iter().find(|e| g.nodes[e.to].id == to).unwrap().label.join(" ");
        assert_eq!(l("B"), "yes");
        assert_eq!(l("C"), "no");
        assert_eq!(l("D"), "maybe");
        assert_eq!(l("E"), "sure");
        assert_eq!(l("F"), "plain");
        assert_eq!(g.edges[4].head, Head::None);
    }

    #[test]
    fn chains_and_groups() {
        let g = p("graph TD\nA --> B --> C\nD & E --> F & G");
        assert_eq!(
            edge_ids(&g),
            [("A", "B"), ("B", "C"), ("D", "F"), ("D", "G"), ("E", "F"), ("E", "G")]
                .map(|(a, b)| (a.to_string(), b.to_string()))
        );
    }

    #[test]
    fn subgraphs() {
        let g = p(
            "flowchart TB\nc1-->a2\nsubgraph one\na1-->a2\nend\nsubgraph two [Second one]\n  direction LR\n  b1-->b2\n  subgraph inner\n  x\n  end\nend",
        );
        assert_eq!(g.clusters.len(), 3);
        assert_eq!(g.clusters[1].id, "two");
        assert_eq!(g.clusters[1].title, ["Second one"]);
        assert_eq!(g.clusters[2].parent, Some(1));
        let node = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap();
        // a2 was first used outside, then inside `one`: it belongs to `one`
        assert_eq!(node("a2").cluster, Some(0));
        assert_eq!(node("c1").cluster, None);
        assert_eq!(node("a1").cluster, Some(0));
        assert_eq!(node("x").cluster, Some(2));
    }

    #[test]
    fn styles_and_classes() {
        let g = p(
            "graph TD\nA:::hot --> B\nclassDef hot fill:#f00,color:#fff\nstyle B stroke:#00f\nclass B hot\nlinkStyle 0 stroke:#0f0",
        );
        assert_eq!(g.nodes[0].style.fill, Some(markise_theme::Rgba::hex(0xff0000)));
        assert_eq!(g.nodes[1].style.stroke, Some(markise_theme::Rgba::hex(0x0000ff)));
        assert_eq!(g.nodes[1].style.fill, Some(markise_theme::Rgba::hex(0xff0000)));
        assert_eq!(g.edges[0].stroke, Some(markise_theme::Rgba::hex(0x00ff00)));
    }

    #[test]
    fn comments_click_and_br() {
        let g = p("%% c\ngraph TD\nA[one<br>two] --> B %% trailing\nclick A \"https://example.com\"\n");
        assert_eq!(g.nodes[0].label, ["one", "two"]);
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn errors_have_lines() {
        let e = parse("graph TD\nA-->B\nA-->", &Limits::default(), &mut Work::unbounded()).unwrap_err();
        assert!(matches!(e, DiagramError::Syntax { line: 3, .. }), "{e:?}");
        let e = parse("graph TD\nA[unclosed", &Limits::default(), &mut Work::unbounded()).unwrap_err();
        assert!(matches!(e, DiagramError::Syntax { line: 2, .. }), "{e:?}");
        let e = parse("graph TD\nend", &Limits::default(), &mut Work::unbounded()).unwrap_err();
        assert!(matches!(e, DiagramError::Syntax { .. }));
    }

    /// Codex P1: `class` with tens of thousands of (duplicate) ids and a huge class name must
    /// not allocate per id (the name is interned, too-long names are ignored, work is bounded).
    #[test]
    fn class_assignments_do_not_amplify_memory() {
        let ids = vec!["A"; 65_000].join(",");
        let src = format!("graph TD\nA\nclass {ids} {}\n", "c".repeat(130_000));
        let t = std::time::Instant::now();
        let g = parse(&src, &Limits::default(), &mut Work::unbounded()).unwrap();
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(g.nodes.len(), 1);
        // a short class used 65 000 times is stored once per node
        let src = format!("graph TD\nA\nclass {ids} hot\nclassDef hot fill:#f00\n");
        let g = parse(&src, &Limits::default(), &mut Work::unbounded()).unwrap();
        assert_eq!(g.nodes[0].style.fill, Some(markise_theme::Rgba::hex(0xff0000)));
        // linkStyle default repeated is applied once
        let src = format!(
            "graph TD\nA-->B\nlinkStyle {} stroke:#00f\n",
            vec!["default"; 40_000].join(",")
        );
        let g = parse(&src, &Limits::default(), &mut Work::unbounded()).unwrap();
        assert_eq!(g.edges[0].stroke, Some(markise_theme::Rgba::hex(0x0000ff)));
    }

    #[test]
    fn parse_work_is_capped() {
        // every byte is charged: a source over the hard cap is refused even without a deadline
        let mut w = Work::unbounded();
        assert!(w.charge(crate::diagram::MAX_PARSE_WORK).is_ok());
        assert!(matches!(w.charge(1), Err(DiagramError::TooLarge(_))));
    }

    #[test]
    fn limits_are_enforced() {
        let l = Limits {
            max_nodes: 10,
            ..Limits::default()
        };
        let src = format!("graph TD\n{}", (0..20).map(|i| format!("n{i}\n")).collect::<String>());
        assert!(matches!(
            parse(&src, &l, &mut Work::unbounded()),
            Err(DiagramError::TooLarge(_))
        ));
        let l = Limits {
            max_depth: 3,
            ..Limits::default()
        };
        let src = format!("graph TD\n{}", "subgraph s\n".repeat(10));
        assert!(matches!(
            parse(&src, &l, &mut Work::unbounded()),
            Err(DiagramError::TooLarge(_))
        ));
    }
}
