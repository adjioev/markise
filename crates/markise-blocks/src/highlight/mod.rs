//! Code highlighting for fenced code blocks (ADR 0006 §5.4, Q13: `syntect` with the pure-Rust
//! `fancy-regex` backend; syntax definitions from `two-face`, licenses in `NOTICE`).
//!
//! - [`lang`]: fence info string → syntax (spec 01 Appendix A aliases).
//! - [`scope`]: syntect scopes → [`TokenKind`]; colors are never taken from syntect themes but
//!   resolved from the active color scheme by the consumer (`Theme::token_look`).
//! - [`Highlighter`]: synchronous, incremental (carries syntect's line state across calls).
//! - [`HighlightService`]: a bounded background worker with a content-keyed result cache.
//!
//! The syntax set is loaded lazily on first use (by the worker thread, or by export), so it
//! never costs startup time.

pub mod lang;
mod notice;
pub mod scope;
mod service;

use std::sync::OnceLock;

use markise_theme::{TokenKind, TokenRun};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

pub use lang::{Language, info_language, resolve};
pub use notice::syntax_license_notice;
pub use scope::ScopeMapper;
pub use service::{BlockKey, HighlightService, Lookup, ServiceStats, block_key};

/// Lines longer than this are not parsed, so one pathological line cannot stall the worker.
/// The parser state after such a line is unknown (it may have opened or closed a comment or
/// string), so the highlighter switches to *degraded* mode: the rest of the block is plain.
pub const MAX_LINE_BYTES: usize = 16 * 1024;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();

/// The bundled syntax set (loaded on first call; syntax contexts load lazily on first use).
pub fn syntax_set() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

/// Whether the syntax set has been loaded in this process (startup measurement, tests).
pub fn syntax_set_loaded() -> bool {
    SYNTAXES.get().is_some()
}

fn syntax_by_name(name: &str) -> Option<&'static SyntaxReference> {
    syntax_set().find_syntax_by_name(name)
}

/// Incremental highlighter for one code block: feed lines in order with [`Highlighter::line`];
/// the syntect parse state carries over between calls (and can be kept between jobs).
pub struct Highlighter {
    state: ParseState,
    stack: ScopeStack,
    mapper: ScopeMapper,
    /// Bytes fed so far (offset of the next line in the block content).
    pos: usize,
    /// An over-long line was skipped: the parse state is unknown, everything after is plain.
    degraded: bool,
}

impl Highlighter {
    /// `None` for a plain language.
    pub fn new(lang: Language) -> Option<Highlighter> {
        let Language::Syntax(name) = lang else { return None };
        let syntax = syntax_by_name(name)?;
        Some(Highlighter {
            state: ParseState::new(syntax),
            stack: ScopeStack::new(),
            mapper: ScopeMapper::new(),
            pos: 0,
            degraded: false,
        })
    }

    /// Byte offset of the next line in the block content.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Whether an over-long line put the highlighter into degraded (plain) mode.
    pub fn degraded(&self) -> bool {
        self.degraded
    }

    /// Highlights one line (`line` includes its trailing `\n`, if any) and appends its runs,
    /// with offsets relative to the block content, to `out` (adjacent equal runs merged).
    pub fn line(&mut self, line: &str, out: &mut Vec<TokenRun>) {
        let base = self.pos;
        self.pos += line.len();
        // the trailing newline is never part of a run
        let end = line.strip_suffix('\n').map_or(line.len(), str::len);
        if self.degraded {
            return;
        }
        if line.len() > MAX_LINE_BYTES {
            self.degraded = true;
            return;
        }
        let Ok(ops) = self.state.parse_line(line, syntax_set()) else {
            return;
        };
        let mut at = 0usize;
        let mut kind = self.mapper.classify(self.stack.as_slice());
        for (off, op) in ops {
            let off = off.min(end);
            if off > at {
                push_run(out, base + at, base + off, kind);
                at = off;
            }
            if self.stack.apply(&op).is_err() {
                // inconsistent stack (should not happen with bundled grammars): restart it
                self.stack = ScopeStack::new();
            }
            kind = self.mapper.classify(self.stack.as_slice());
        }
        if end > at {
            push_run(out, base + at, base + end, kind);
        }
    }
}

fn push_run(out: &mut Vec<TokenRun>, start: usize, end: usize, kind: Option<TokenKind>) {
    let Some(kind) = kind else { return };
    let (start, end) = (start as u32, end as u32);
    if let Some(last) = out.last_mut()
        && last.kind == kind
        && last.end == start
    {
        last.end = end;
        return;
    }
    out.push(TokenRun { start, end, kind });
}

/// Iterates the lines of `text` with their trailing `\n`.
pub fn lines_with_newline(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n')
}

/// Highlights a whole code block synchronously (export). `content` is the block's lines, each
/// followed by `\n`. `None` when the language is plain.
pub fn highlight_block(lang: &str, content: &str) -> Option<Vec<TokenRun>> {
    let mut h = Highlighter::new(resolve(lang))?;
    let mut out = Vec::new();
    for l in lines_with_newline(content) {
        h.line(l, &mut out);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: &str, src: &str) -> Vec<(String, TokenKind)> {
        highlight_block(lang, src)
            .unwrap()
            .into_iter()
            .map(|r| (src[r.start as usize..r.end as usize].to_string(), r.kind))
            .collect()
    }

    fn has(v: &[(String, TokenKind)], text: &str, k: TokenKind) -> bool {
        v.iter().any(|(t, kk)| t == text && *kk == k)
    }

    #[test]
    fn rust_tokens() {
        let v = kinds("rust", "// hi\nfn main() { let x = 42; println!(\"a\\n\"); }\n");
        assert!(has(&v, "// hi", TokenKind::Comment), "{v:?}");
        assert!(has(&v, "fn", TokenKind::Keyword), "{v:?}");
        assert!(has(&v, "main", TokenKind::Function), "{v:?}");
        assert!(has(&v, "42", TokenKind::Number), "{v:?}");
        assert!(
            v.iter().any(|(t, k)| t.contains('"') && *k == TokenKind::String),
            "{v:?}"
        );
        assert!(has(&v, "\\n", TokenKind::StringSpecial), "{v:?}");
    }

    #[test]
    fn python_json_yaml_bash_ts() {
        let v = kinds("python", "def f(x):\n    return None  # c\n");
        assert!(has(&v, "def", TokenKind::Keyword), "{v:?}");
        assert!(has(&v, "f", TokenKind::Function), "{v:?}");
        assert!(has(&v, "None", TokenKind::Constant), "{v:?}");
        let v = kinds("json", "{\"k\": \"v\", \"n\": 1.5, \"b\": true}\n");
        assert!(
            v.iter().any(|(t, k)| t.contains('k') && *k == TokenKind::Property),
            "{v:?}"
        );
        assert!(
            v.iter().any(|(t, k)| t.contains('v') && *k == TokenKind::String),
            "{v:?}"
        );
        assert!(has(&v, "1.5", TokenKind::Number), "{v:?}");
        let v = kinds("yaml", "key: value # c\nn: 3\n");
        assert!(has(&v, "key", TokenKind::Tag), "{v:?}");
        assert!(has(&v, "3", TokenKind::Number), "{v:?}");
        let v = kinds("bash", "if [ -f x ]; then echo \"$HOME\"; fi\n");
        assert!(has(&v, "if", TokenKind::Keyword), "{v:?}");
        let v = kinds("ts", "interface A { b: string }\nconst x: number = 1;\n");
        assert!(has(&v, "interface", TokenKind::Keyword), "{v:?}");
        assert!(has(&v, "string", TokenKind::Type), "{v:?}");
    }

    #[test]
    fn runs_are_sorted_disjoint_and_within_lines() {
        let src = "fn a() {}\n\n/* x\n y */ let s = \"q\";\n";
        let runs = highlight_block("rust", src).unwrap();
        let mut last = 0;
        for r in &runs {
            assert!(r.start >= last && r.end > r.start, "{runs:?}");
            assert!(!src[r.start as usize..r.end as usize].contains('\n') || r.kind == TokenKind::Comment);
            last = r.end;
        }
    }

    /// Review P2: a skipped over-long line may contain multiline delimiters; the code after it
    /// must not be colored with a guessed state (degraded mode: plain).
    #[test]
    fn skipped_long_line_degrades_to_plain() {
        let long = format!("{} */\n", "x".repeat(MAX_LINE_BYTES + 10));
        let src = format!("fn a() {{}}\n/* open\n{long}fn b() {{}}\nlet s = \"x\";\n");
        let runs = highlight_block("rust", &src).unwrap();
        let after = src.find("fn b").unwrap() as u32;
        assert!(
            runs.iter().all(|r| r.end <= after),
            "plain after the skipped line: {runs:?}"
        );
        assert!(
            runs.iter().any(|r| r.kind == TokenKind::Keyword),
            "highlighted before it"
        );
        // a skipped line opening a string: same
        let long = format!("let s = \"{}\n", "y".repeat(MAX_LINE_BYTES + 10));
        let src = format!("{long}\" ; fn c() {{}}\n");
        assert!(highlight_block("rust", &src).unwrap().is_empty());
    }

    #[test]
    fn plain_language_is_none() {
        assert!(highlight_block("", "x\n").is_none());
        assert!(highlight_block("mermaid", "graph TD\n").is_none());
        assert!(highlight_block("nope", "x\n").is_none());
    }

    #[test]
    fn incremental_equals_whole() {
        let src = "/* a\nb */ fn x() {}\nlet s = \"multi\nline\";\n";
        let whole = highlight_block("rust", src).unwrap();
        // two highlighters cannot share state, but one fed line by line in two batches must
        // produce the same runs as one call
        let mut h = Highlighter::new(resolve("rust")).unwrap();
        let mut out = Vec::new();
        let lines: Vec<&str> = lines_with_newline(src).collect();
        for l in &lines[..2] {
            h.line(l, &mut out);
        }
        for l in &lines[2..] {
            h.line(l, &mut out);
        }
        assert_eq!(whole, out);
    }
}
