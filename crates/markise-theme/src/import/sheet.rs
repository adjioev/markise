//! Stylesheet loading: tolerant CSS parsing (cssparser), `@import` inlining, `@media`
//! evaluation, `@font-face` collection and CSS nesting (spec 03 §1.5, §2.4 steps 1–2).

use std::path::{Path, PathBuf};

use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserState, QualifiedRuleParser,
    RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
};

use super::ImportOptions;
use super::report::{Location, ReportBuilder};
use super::selector::{SelError, Selector, parse_selector_list, split_top_level_commas};
use crate::{FontFace, FontSource};

/// When a rule applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cond {
    Always,
    Light,
    Dark,
    /// Only in `@media print` (kept as text for export).
    Print,
    Never,
}

impl Cond {
    fn and(self, other: Cond) -> Cond {
        use Cond::*;
        match (self, other) {
            (Never, _) | (_, Never) => Never,
            (Print, _) | (_, Print) => Print,
            (Always, x) | (x, Always) => x,
            (Light, Light) => Light,
            (Dark, Dark) => Dark,
            _ => Never,
        }
    }
}

pub(crate) struct Decl {
    /// Lowercased property name (custom properties keep their case).
    pub name: String,
    pub value: String,
    pub important: bool,
    pub loc: Location,
}

pub(crate) struct Rule {
    pub selectors: Vec<Selector>,
    pub decls: Vec<Decl>,
    pub cond: Cond,
}

#[derive(Default)]
pub(crate) struct Sheet {
    pub rules: Vec<Rule>,
    pub faces: Vec<FontFace>,
    pub include_when_export: Vec<String>,
    pub print_css: Vec<String>,
    pub scheme_aware: bool,
    pub declarations: usize,
    pub selectors: usize,
}

pub(crate) struct Loader<'a> {
    pub opts: &'a ImportOptions,
    pub report: &'a mut ReportBuilder,
    pub sheet: Sheet,
    visiting: Vec<PathBuf>,
    total_bytes: usize,
    limit_warned: bool,
    /// Rules of the built-in base layer (not counted against `max_rules`).
    pub base_rules: usize,
    /// Canonical theme folder (the folder of the root theme CSS); `@import` targets must
    /// resolve inside it (TI-9). `None` disables local imports.
    pub root_dir: Option<PathBuf>,
}

type PErr = ParseError<()>;

impl<'a> Loader<'a> {
    pub fn new(opts: &'a ImportOptions, report: &'a mut ReportBuilder) -> Self {
        Loader {
            opts,
            report,
            sheet: Sheet::default(),
            visiting: Vec::new(),
            total_bytes: 0,
            limit_warned: false,
            base_rules: 0,
            root_dir: None,
        }
    }

    /// Register a file name for reporting; returns its index.
    pub fn add_file(&mut self, path: PathBuf) -> usize {
        self.report.files.push(path);
        self.report.files.len() - 1
    }

    /// Read a theme layer from disk (size-limited, UTF-8 lossy). `None` on error (reported).
    pub fn read_file(&mut self, path: &Path) -> Option<String> {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                self.report.warn(format!("{}: {e}", path.display()));
                return None;
            }
        };
        let len = meta.len() as usize;
        if len > self.opts.max_file_bytes {
            self.report.warn(format!(
                "{}: {len} bytes exceeds the {} byte file limit; skipped",
                path.display(),
                self.opts.max_file_bytes
            ));
            return None;
        }
        if self.total_bytes + len > self.opts.max_total_bytes {
            self.report
                .warn(format!("{}: total CSS size limit reached; skipped", path.display()));
            return None;
        }
        // regular files only (a FIFO would block), bounded while reading
        match markise_doc::resource::read_regular(path, self.opts.max_file_bytes as u64) {
            Ok(b) => {
                self.total_bytes += b.len();
                Some(String::from_utf8_lossy(&b).into_owned())
            }
            Err(e) => {
                self.report.warn(format!("{}: {e}", path.display()));
                None
            }
        }
    }

    /// Account for bytes read outside [`Loader::read_file`] (top-level layers).
    pub fn note_bytes(&mut self, n: usize) {
        self.total_bytes += n;
    }

    /// Parse `css` (one file) and append its rules.
    pub fn load_text(
        &mut self,
        css: &str,
        base_dir: Option<PathBuf>,
        path: Option<&Path>,
        file: usize,
        depth: u32,
        cond: Cond,
    ) {
        let css = if css.len() > self.opts.max_file_bytes {
            self.report.warn(format!(
                "input truncated to the {} byte file limit",
                self.opts.max_file_bytes
            ));
            let mut end = self.opts.max_file_bytes;
            while !css.is_char_boundary(end) {
                end -= 1;
            }
            &css[..end]
        } else {
            css
        };
        let mut pushed = false;
        if let Some(p) = path
            && let Ok(c) = p.canonicalize()
        {
            self.visiting.push(c);
            pushed = true;
        }
        let mut input = Parser::new(css);
        let mut top = Top {
            l: self,
            file,
            base_dir,
            depth,
            cond,
        };
        let sp = StyleSheetParser::new(&mut input, &mut top);
        for r in sp {
            if let Err((_, _, loc)) = r {
                let _ = loc; // invalid rules are reported where they are recognised
            }
        }
        if pushed {
            self.visiting.pop();
        }
    }

    fn rules_full(&mut self) -> bool {
        if self.sheet.rules.len() >= self.opts.max_rules.saturating_add(self.base_rules) {
            if !self.limit_warned {
                self.limit_warned = true;
                let m = format!("rule limit ({}) reached; remaining rules ignored", self.opts.max_rules);
                self.report.warn(m);
            }
            return true;
        }
        false
    }

    fn import(&mut self, url: &str, media: &str, from: Location, base_dir: Option<&Path>, depth: u32, cond: Cond) {
        let mcond = if media.trim().is_empty() {
            Cond::Always
        } else {
            self.eval_media(media, from)
        };
        let cond = cond.and(mcond);
        if matches!(cond, Cond::Never | Cond::Print) {
            return;
        }
        let lower = url.to_ascii_lowercase();
        if !lower.starts_with("file:")
            && (lower.contains("://") || lower.starts_with("//") || lower.starts_with("data:"))
        {
            self.report.at_rules.add("@import (remote, not fetched)", from);
            return;
        }
        if depth + 1 > self.opts.max_import_depth {
            self.report.warn(format!(
                "@import {url}: depth limit ({}) reached",
                self.opts.max_import_depth
            ));
            return;
        }
        let Some(base) = base_dir else {
            self.report
                .warn(format!("@import {url}: no base directory to resolve against"));
            return;
        };
        let clean = percent_decode(url.split(['?', '#']).next().unwrap_or(""));
        if is_absolute_ref(&clean) {
            self.report.at_rules.add("@import (absolute path, rejected)", from);
            self.report
                .warn(format!("@import {url}: absolute paths are not allowed; skipped"));
            return;
        }
        let target = base.join(&clean);
        if self.report.attempted.len() < 256 && !self.report.attempted.contains(&target) {
            self.report.attempted.push(target.clone());
        }
        let canon = match target.canonicalize() {
            Ok(c) => c,
            Err(e) => {
                self.report.warn(format!("@import {url}: {e}"));
                return;
            }
        };
        // TI-9: confine imports to the theme folder (catches `..` escapes and symlinks that
        // resolve outside it, since `canon` is fully resolved)
        match &self.root_dir {
            Some(root) if canon.starts_with(root) => {}
            _ => {
                self.report
                    .at_rules
                    .add("@import (outside the theme folder, rejected)", from);
                self.report
                    .warn(format!("@import {url}: resolves outside the theme folder; skipped"));
                return;
            }
        }
        if self.visiting.contains(&canon) {
            self.report.warn(format!("@import {url}: import cycle; skipped"));
            return;
        }
        if self.report.files.len() >= self.opts.max_files {
            self.report
                .warn(format!("@import {url}: file limit ({}) reached", self.opts.max_files));
            return;
        }
        let Some(text) = self.read_file(&canon) else { return };
        let idx = self.add_file(canon.clone());
        let dir = canon.parent().map(Path::to_path_buf);
        self.load_text(&text, dir, Some(&canon), idx, depth + 1, cond);
    }

    /// Evaluate a media query list for the screen (spec 03 §1.4/§1.5).
    pub(crate) fn eval_media(&mut self, text: &str, loc: Location) -> Cond {
        let mut any_always = false;
        let (mut light, mut dark, mut print) = (false, false, false);
        for q in split_top_level_commas(text) {
            match self.eval_query(q, loc) {
                Cond::Always => any_always = true,
                Cond::Light => light = true,
                Cond::Dark => dark = true,
                Cond::Print => print = true,
                Cond::Never => {}
            }
        }
        if any_always || (light && dark) {
            Cond::Always
        } else if light {
            Cond::Light
        } else if dark {
            Cond::Dark
        } else if print {
            Cond::Print
        } else {
            Cond::Never
        }
    }

    fn eval_query(&mut self, q: &str, loc: Location) -> Cond {
        let q = q.trim().to_ascii_lowercase();
        if q.is_empty() {
            return Cond::Never;
        }
        let (negate, rest) = match q.strip_prefix("not ") {
            Some(r) => (true, r.trim()),
            None => (false, q.strip_prefix("only ").map(str::trim).unwrap_or(&q)),
        };
        let mut result = Cond::Always;
        let mut rest = rest.to_string();
        // media type
        if !rest.starts_with('(') {
            let end = rest.find(|c: char| c.is_whitespace() || c == '(').unwrap_or(rest.len());
            let ty = rest[..end].to_string();
            rest = rest[end..].trim().trim_start_matches("and").trim().to_string();
            result = match ty.as_str() {
                "all" | "screen" => Cond::Always,
                "print" => Cond::Print,
                _ => Cond::Never,
            };
        }
        // features: `(a) and (b)`
        let mut s = rest.as_str();
        while let Some(open) = s.find('(') {
            let Some(close_rel) = s[open..].find(')') else {
                self.report.at_rules.add("@media (malformed query)", loc);
                return Cond::Never;
            };
            let feat = &s[open + 1..open + close_rel];
            if s[..open].contains(" or ") {
                self.report.at_rules.add("@media (`or` in query)", loc);
                return Cond::Never;
            }
            let c = self.eval_feature(feat.trim(), loc);
            result = result.and(c);
            s = &s[open + close_rel + 1..];
        }
        if negate {
            result = match result {
                Cond::Always => Cond::Never,
                Cond::Never => Cond::Always,
                Cond::Light => Cond::Dark,
                Cond::Dark => Cond::Light,
                Cond::Print => Cond::Always,
            };
        }
        result
    }

    fn eval_feature(&mut self, feat: &str, loc: Location) -> Cond {
        let (name, value) = match feat.split_once(':') {
            Some((n, v)) => (n.trim(), Some(v.trim())),
            None => (feat, None),
        };
        let px = |v: Option<&str>| -> Option<f32> {
            let v = v?;
            let num_end = v
                .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
                .unwrap_or(v.len());
            let n: f32 = v[..num_end].parse().ok()?;
            match v[num_end..].trim() {
                "px" | "" => Some(n),
                "em" | "rem" => Some(n * 16.0),
                "pt" => Some(n * 4.0 / 3.0),
                _ => None,
            }
        };
        let w = self.opts.viewport_width;
        let h = self.opts.viewport_height;
        let bool_of = |b: bool| if b { Cond::Always } else { Cond::Never };
        match name {
            "prefers-color-scheme" => {
                self.sheet.scheme_aware = true;
                match value {
                    Some("dark") => Cond::Dark,
                    Some("light") => Cond::Light,
                    _ => Cond::Never,
                }
            }
            "prefers-dark-interface" => {
                self.sheet.scheme_aware = true;
                Cond::Dark
            }
            "min-width" | "max-width" | "width" | "min-height" | "max-height" | "height" | "min-device-width"
            | "max-device-width" => match px(value) {
                Some(v) => {
                    let dim = if name.contains("height") { h } else { w };
                    bool_of(if name.starts_with("min") {
                        dim >= v
                    } else if name.starts_with("max") {
                        dim <= v
                    } else {
                        (dim - v).abs() < 0.5
                    })
                }
                None => {
                    self.report
                        .at_rules
                        .add(&format!("@media ({name}: unsupported value)"), loc);
                    Cond::Never
                }
            },
            "orientation" => bool_of(value == Some(if w >= h { "landscape" } else { "portrait" })),
            "hover" | "any-hover" => bool_of(value.is_none() || value == Some("hover")),
            "pointer" | "any-pointer" => bool_of(value.is_none() || value == Some("fine")),
            "prefers-reduced-motion" | "prefers-reduced-transparency" => bool_of(value == Some("no-preference")),
            "prefers-contrast" | "forced-colors" | "inverted-colors" => {
                bool_of(value == Some("no-preference") || value == Some("none"))
            }
            "color" => Cond::Always,
            "-webkit-min-device-pixel-ratio"
            | "min-resolution"
            | "-webkit-device-pixel-ratio"
            | "-webkit-max-device-pixel-ratio"
            | "max-resolution"
            | "resolution" => {
                let ratio = value
                    .and_then(|v| {
                        let v = v.trim_end_matches("dppx").trim_end_matches('x');
                        if let Some(d) = v.strip_suffix("dpi") {
                            d.parse::<f32>().ok().map(|d| d / 96.0)
                        } else {
                            v.parse::<f32>().ok()
                        }
                    })
                    .unwrap_or(1.0);
                let dpr = self.opts.device_pixel_ratio;
                bool_of(if name.contains("min") {
                    dpr >= ratio
                } else if name.contains("max") {
                    dpr <= ratio
                } else {
                    (dpr - ratio).abs() < 0.01
                })
            }
            _ => {
                self.report.at_rules.add(&format!("@media ({name})"), loc);
                Cond::Never
            }
        }
    }
}

/// Absolute file references: leading `/` or backslash, a drive letter (`C:`), `file:` URLs.
fn is_absolute_ref(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('/')
        || s.starts_with('\\')
        || s.to_ascii_lowercase().starts_with("file:")
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
        || Path::new(s).is_absolute()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn loc_of(file: usize, line0: u32) -> Location {
    Location { file, line: line0 + 1 }
}

// ------------------------------------------------------------------------------------------
// cssparser glue: top level / @media blocks

struct Top<'l, 'a> {
    l: &'l mut Loader<'a>,
    file: usize,
    base_dir: Option<PathBuf>,
    depth: u32,
    cond: Cond,
}

enum AtPrelude {
    Import(String, String),
    Media(String),
    Supports,
    FontFace,
    IncludeWhenExport(String),
    Skip,
}

impl<'i> AtRuleParser<'i> for Top<'_, '_> {
    type Prelude = (AtPrelude, Location);
    type AtRule = ();
    type Error = ();

    fn parse_prelude(&mut self, name: CowRcStr<'i>, input: &mut Parser<'i>) -> Result<Self::Prelude, PErr> {
        let loc = loc_of(self.file, input.current_source_location().line);
        let lname = name.to_ascii_lowercase();
        let start = input.position();
        let prelude = match lname.as_str() {
            "import" => {
                let url = input.expect_url_or_string()?.to_string();
                let s = input.position();
                while input.next().is_ok() {}
                AtPrelude::Import(url, input.slice_from(s).to_string())
            }
            "media" => {
                while input.next().is_ok() {}
                AtPrelude::Media(input.slice_from(start).to_string())
            }
            "supports" => {
                while input.next().is_ok() {}
                AtPrelude::Supports
            }
            "font-face" => AtPrelude::FontFace,
            "include-when-export" => {
                let url = input.expect_url_or_string().map(|u| u.to_string()).unwrap_or_default();
                while input.next().is_ok() {}
                AtPrelude::IncludeWhenExport(url)
            }
            "charset" => {
                while input.next().is_ok() {}
                AtPrelude::Skip
            }
            _ => {
                while input.next().is_ok() {}
                self.l.report.at_rules.add(&format!("@{lname}"), loc);
                AtPrelude::Skip
            }
        };
        Ok((prelude, loc))
    }

    fn rule_without_block(&mut self, prelude: Self::Prelude, _start: &ParserState) -> Result<(), ()> {
        let (p, loc) = prelude;
        match p {
            AtPrelude::Import(url, media) => {
                let base = self.base_dir.clone();
                self.l.import(&url, &media, loc, base.as_deref(), self.depth, self.cond);
            }
            AtPrelude::IncludeWhenExport(url) if !url.is_empty() && self.l.sheet.include_when_export.len() < 64 => {
                self.l.sheet.include_when_export.push(url);
            }
            _ => {}
        }
        Ok(())
    }

    fn parse_block(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<(), PErr> {
        let (p, loc) = prelude;
        match p {
            AtPrelude::Media(text) => {
                let c = self.l.eval_media(&text, loc);
                let cond = self.cond.and(c);
                match cond {
                    Cond::Never => skip_all(input),
                    Cond::Print => {
                        let s = input.position();
                        skip_all(input);
                        if self.l.sheet.print_css.len() < 1024 {
                            let body = input.slice_from(s);
                            self.l
                                .sheet
                                .print_css
                                .push(format!("@media {} {{{body}}}", text.trim()));
                        }
                    }
                    _ => {
                        let mut inner = Top {
                            l: &mut *self.l,
                            file: self.file,
                            base_dir: self.base_dir.clone(),
                            depth: self.depth,
                            cond,
                        };
                        let mut sp = StyleSheetParser::new(input, &mut inner);
                        while sp.next().is_some() {}
                    }
                }
            }
            AtPrelude::Supports => {
                // feature queries are assumed true (themes use them for progressive enhancement)
                let mut inner = Top {
                    l: &mut *self.l,
                    file: self.file,
                    base_dir: self.base_dir.clone(),
                    depth: self.depth,
                    cond: self.cond,
                };
                let mut sp = StyleSheetParser::new(input, &mut inner);
                while sp.next().is_some() {}
            }
            AtPrelude::FontFace => {
                let face = parse_font_face(input, self.base_dir.as_deref(), self.l, loc);
                if let Some(f) = face
                    && self.l.sheet.faces.len() < 512
                {
                    self.l.sheet.faces.push(f);
                }
            }
            _ => skip_all(input),
        }
        Ok(())
    }
}

fn skip_all(input: &mut Parser) {
    while input.next_including_whitespace_and_comments().is_ok() {}
}

impl<'i> QualifiedRuleParser<'i> for Top<'_, '_> {
    type Prelude = (String, Location);
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude(&mut self, input: &mut Parser<'i>) -> Result<Self::Prelude, PErr> {
        let loc = loc_of(self.file, input.current_source_location().line);
        let start = input.position();
        while input.next().is_ok() {}
        Ok((input.slice_from(start).to_string(), loc))
    }

    fn parse_block(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<(), PErr> {
        let (text, loc) = prelude;
        let parents = vec![text];
        style_rule(self.l, &parents, loc, self.file, self.cond, input, 0);
        Ok(())
    }
}

/// Parse a style rule body (declarations + nested rules) for the given selector texts.
fn style_rule<'i>(
    l: &mut Loader,
    selector_texts: &[String],
    loc: Location,
    file: usize,
    cond: Cond,
    input: &mut Parser<'i>,
    nest: u32,
) {
    // parse selectors
    let mut selectors = Vec::new();
    let mut texts = Vec::new();
    let mut invalid = false;
    for t in selector_texts {
        let (ok, bad) = parse_selector_list(t);
        for (s, e) in bad {
            let short: String = s
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(120)
                .collect();
            match e {
                SelError::Invalid(why) => {
                    invalid = true;
                    l.report.selectors.add(&format!("invalid: {short} ({why})"), loc);
                }
                SelError::Unsupported(why) => {
                    l.report.selectors.add(&format!("unsupported {why}: {short}"), loc);
                }
            }
        }
        for piece in split_top_level_commas(t) {
            texts.push(piece.trim().to_string());
        }
        selectors.extend(ok);
    }
    let mut full = l.rules_full();
    if l.sheet.selectors + selectors.len() > l.opts.max_selectors {
        full = true;
        let m = format!(
            "selector limit ({}) reached; rules beyond it are ignored",
            l.opts.max_selectors
        );
        l.report.warn(m);
    }
    let idx = if !invalid && !full && !selectors.is_empty() {
        l.sheet.selectors += selectors.len();
        l.sheet.rules.push(Rule {
            selectors,
            decls: Vec::new(),
            cond,
        });
        Some(l.sheet.rules.len() - 1)
    } else {
        None
    };
    let mut body = Body {
        l,
        file,
        parents: texts,
        cond,
        nest,
        decls: Vec::new(),
    };
    let mut rp = RuleBodyParser::new(input, &mut body);
    while rp.next().is_some() {}
    let decls = std::mem::take(&mut body.decls);
    if invalid {
        return;
    }
    if let Some(i) = idx {
        body.l.sheet.declarations += decls.len();
        body.l.sheet.rules[i].decls = decls;
    }
}

struct Body<'l, 'a> {
    l: &'l mut Loader<'a>,
    file: usize,
    parents: Vec<String>,
    cond: Cond,
    nest: u32,
    decls: Vec<Decl>,
}

impl<'i> DeclarationParser<'i> for Body<'_, '_> {
    type Declaration = ();
    type Error = ();

    fn parse_value(&mut self, name: CowRcStr<'i>, input: &mut Parser<'i>, start: &ParserState) -> Result<(), PErr> {
        let loc = loc_of(self.file, start.source_location().line);
        let s = input.position();
        skip_all(input);
        let raw = input.slice_from(s);
        let (value, important) = strip_important(raw);
        let name = if name.starts_with("--") {
            name.to_string()
        } else {
            name.to_ascii_lowercase()
        };
        if self.decls.len() < 4096 && self.l.sheet.declarations + self.decls.len() < self.l.opts.max_declarations {
            self.decls.push(Decl {
                name,
                value: value.to_string(),
                important,
                loc,
            });
        }
        Ok(())
    }
}

fn strip_important(raw: &str) -> (&str, bool) {
    let t = raw.trim_end();
    let lower_tail = t.len().saturating_sub(9);
    if t.is_char_boundary(lower_tail) && t[lower_tail..].eq_ignore_ascii_case("important") {
        let before = t[..lower_tail].trim_end();
        if let Some(b) = before.strip_suffix('!') {
            return (b.trim(), true);
        }
    }
    (raw.trim(), false)
}

impl<'i> AtRuleParser<'i> for Body<'_, '_> {
    type Prelude = (String, String, Location);
    type AtRule = ();
    type Error = ();

    fn parse_prelude(&mut self, name: CowRcStr<'i>, input: &mut Parser<'i>) -> Result<Self::Prelude, PErr> {
        let loc = loc_of(self.file, input.current_source_location().line);
        let s = input.position();
        skip_all(input);
        Ok((name.to_ascii_lowercase(), input.slice_from(s).to_string(), loc))
    }

    fn rule_without_block(&mut self, prelude: Self::Prelude, _start: &ParserState) -> Result<(), ()> {
        self.l
            .report
            .at_rules
            .add(&format!("@{} (nested)", prelude.0), prelude.2);
        Ok(())
    }

    fn parse_block(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<(), PErr> {
        let (name, text, loc) = prelude;
        if name == "media" && self.nest < 8 {
            let c = self.l.eval_media(&text, loc);
            let cond = self.cond.and(c);
            if matches!(cond, Cond::Never | Cond::Print) {
                skip_all(input);
            } else {
                let parents = vec![self.parents.join(", ")];
                style_rule(self.l, &parents, loc, self.file, cond, input, self.nest + 1);
            }
        } else {
            self.l.report.at_rules.add(&format!("@{name} (nested)"), loc);
            skip_all(input);
        }
        Ok(())
    }
}

impl<'i> QualifiedRuleParser<'i> for Body<'_, '_> {
    type Prelude = (String, Location);
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude(&mut self, input: &mut Parser<'i>) -> Result<Self::Prelude, PErr> {
        let loc = loc_of(self.file, input.current_source_location().line);
        let start = input.position();
        while input.next().is_ok() {}
        Ok((input.slice_from(start).to_string(), loc))
    }

    fn parse_block(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<(), PErr> {
        let (child, loc) = prelude;
        if self.nest >= 8 {
            self.l.report.warn("CSS nesting deeper than 8 levels ignored".into());
            skip_all(input);
            return Ok(());
        }
        // CSS nesting: `&` stands for the parent selector, otherwise a descendant is implied.
        let mut composed = Vec::new();
        for c in split_top_level_commas(&child) {
            let c = c.trim();
            for p in &self.parents {
                if composed.len() >= 256 {
                    break;
                }
                if c.contains('&') {
                    composed.push(c.replace('&', &format!(":is({p})")));
                } else {
                    composed.push(format!("{p} {c}"));
                }
            }
        }
        let text = vec![composed.join(", ")];
        style_rule(self.l, &text, loc, self.file, self.cond, input, self.nest + 1);
        Ok(())
    }
}

impl<'i> RuleBodyItemParser<'i, (), ()> for Body<'_, '_> {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        true
    }
}

// ------------------------------------------------------------------------------------------
// @font-face

struct Descriptors(Vec<(String, String)>);

impl<'i> DeclarationParser<'i> for Descriptors {
    type Declaration = ();
    type Error = ();
    fn parse_value(&mut self, name: CowRcStr<'i>, input: &mut Parser<'i>, _s: &ParserState) -> Result<(), PErr> {
        let s = input.position();
        skip_all(input);
        if self.0.len() < 64 {
            self.0
                .push((name.to_ascii_lowercase(), input.slice_from(s).trim().to_string()));
        }
        Ok(())
    }
}
impl<'i> AtRuleParser<'i> for Descriptors {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
}
impl<'i> QualifiedRuleParser<'i> for Descriptors {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
}
impl<'i> RuleBodyItemParser<'i, (), ()> for Descriptors {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        false
    }
}

fn parse_font_face(input: &mut Parser, base_dir: Option<&Path>, l: &mut Loader, loc: Location) -> Option<FontFace> {
    let mut d = Descriptors(Vec::new());
    {
        let mut rp = RuleBodyParser::new(input, &mut d);
        while rp.next().is_some() {}
    }
    let get = |n: &str| d.0.iter().rev().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
    let family = get("font-family")
        .and_then(super::value::parse_families)
        .and_then(|v| v.into_iter().next());
    let Some(family) = family else {
        l.report.at_rules.add("@font-face (no font-family)", loc);
        return None;
    };
    let mut sources = Vec::new();
    if let Some(src) = get("src") {
        for piece in split_top_level_commas(src) {
            if sources.len() >= 16 {
                break;
            }
            if let Some(s) = parse_src(piece, base_dir) {
                sources.push(s);
            }
        }
    }
    let weight = get("font-weight")
        .map(|w| {
            let parts: Vec<u16> = w
                .split_ascii_whitespace()
                .filter_map(|t| match t.to_ascii_lowercase().as_str() {
                    "normal" => Some(400),
                    "bold" => Some(700),
                    n => n.parse::<f32>().ok().map(|f| f.clamp(1.0, 1000.0) as u16),
                })
                .collect();
            match parts.as_slice() {
                [a] => (*a, *a),
                [a, b, ..] => (*a, *b),
                [] => (400, 400),
            }
        })
        .unwrap_or((400, 400));
    let italic = get("font-style")
        .map(|s| {
            let s = s.to_ascii_lowercase();
            s.starts_with("italic") || s.starts_with("oblique")
        })
        .unwrap_or(false);
    let mut unicode_range = Vec::new();
    if let Some(ur) = get("unicode-range") {
        for piece in split_top_level_commas(ur) {
            let mut p = Parser::new(piece.trim());
            match cssparser::UnicodeRange::parse(&mut p) {
                Ok(r) if unicode_range.len() < 256 => unicode_range.push((r.start, r.end)),
                Ok(_) => {}
                Err(_) => l.report.values.add("unicode-range", loc),
            }
        }
    }
    Some(FontFace {
        family,
        sources,
        weight,
        italic,
        unicode_range,
        display: get("font-display").map(str::to_string),
    })
}

fn parse_src(piece: &str, base_dir: Option<&Path>) -> Option<FontSource> {
    let mut p = Parser::new(piece.trim());
    let mut url: Option<String> = None;
    let mut local: Option<String> = None;
    let mut format: Option<String> = None;
    while let Ok(tok) = p.next().cloned() {
        match tok {
            Token::UnquotedUrl(u) => url = Some(u.to_string()),
            Token::Function(f) => {
                let f = f.to_ascii_lowercase();
                let arg: Result<String, PErr> = p.parse_nested_block(|p| {
                    let t = p.next()?.clone();
                    Ok(match t {
                        Token::QuotedString(s) | Token::Ident(s) => s.to_string(),
                        _ => String::new(),
                    })
                });
                let arg = arg.unwrap_or_default();
                match f.as_str() {
                    "url" => url = Some(arg),
                    "local" => local = Some(arg),
                    "format" => format = Some(arg),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if let Some(name) = local {
        return Some(FontSource::Local(name));
    }
    let u = url?;
    let lower = u.to_ascii_lowercase();
    if lower.starts_with("data:") {
        let fmt = format.or_else(|| {
            lower
                .split([';', ','])
                .next()
                .map(|m| m.trim_start_matches("data:").to_string())
        });
        return Some(FontSource::Embedded {
            format: fmt,
            bytes: u.len(),
        });
    }
    if lower.contains("://") || lower.starts_with("//") {
        return Some(FontSource::Remote(u));
    }
    let clean = u.split(['?', '#']).next().unwrap_or("");
    let path = match base_dir {
        Some(b) => b.join(percent_decode(clean)),
        None => PathBuf::from(percent_decode(clean)),
    };
    let exists = path.is_file();
    Some(FontSource::File { path, format, exists })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn important_is_stripped() {
        assert_eq!(strip_important("red !important"), ("red", true));
        assert_eq!(strip_important("red ! IMPORTANT "), ("red", true));
        assert_eq!(strip_important("\"!important\""), ("\"!important\"", false));
        assert_eq!(strip_important("blue"), ("blue", false));
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b.css"), "a b.css");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
