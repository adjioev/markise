//! Extended callouts: a blockquote whose first line starts with `[!name]` (ADR 0025).
//!
//! ```markdown
//! > [!warning] Optional title
//! > Body.
//!
//! > [!success]- Starts closed
//! > Hidden until opened.
//! ```
//!
//! Everything this syntax means lives here, so it can be read, tested and switched off as a
//! unit ([`crate::ParseOptions::callouts`]): the grammar of the first line ([`parse_tag`]), the
//! table of names ([`CalloutType`]), default titles, and the neutral view layout reads
//! ([`view`]). The parser has one hook, [`recognise`], called when a quote is finished; nothing
//! else in the parser knows about callouts, and the source stays the only store: the tree keeps
//! what the tag declared ([`Callout`]), never its text or spans.
//!
//! The bare forms of the five GitHub alert names (`> [!NOTE]`) are the narrow case of the same
//! recognition ([`Callout::alert`]): same option (`alerts`), same output as before.

use std::ops::Range;
use std::sync::Arc;

use markise_doc::{CalloutAccent, CalloutIcon, CalloutKind, CalloutView};

use crate::block::{AlertKind, Block, BlockKind};
use crate::options::ParseOptions;

/// Longest accepted name, in bytes.
const MAX_NAME: usize = 64;

/// What a callout's name means: its colour and icon. Aliases share a type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CalloutType {
    Note,
    Info,
    Tip,
    Important,
    Success,
    Question,
    Warning,
    Caution,
    Failure,
    Danger,
    Bug,
    Example,
    Quote,
    Abstract,
    Todo,
    /// A valid name outside the table: the note style, with its own name as the title.
    Unknown,
}

/// Known names and aliases (compared without regard to ASCII case).
const NAMES: &[(&str, CalloutType)] = &[
    ("note", CalloutType::Note),
    ("info", CalloutType::Info),
    ("tip", CalloutType::Tip),
    ("hint", CalloutType::Tip),
    ("important", CalloutType::Important),
    ("success", CalloutType::Success),
    ("check", CalloutType::Success),
    ("done", CalloutType::Success),
    ("question", CalloutType::Question),
    ("help", CalloutType::Question),
    ("faq", CalloutType::Question),
    ("warning", CalloutType::Warning),
    ("attention", CalloutType::Warning),
    ("caution", CalloutType::Caution),
    ("failure", CalloutType::Failure),
    ("fail", CalloutType::Failure),
    ("missing", CalloutType::Failure),
    ("danger", CalloutType::Danger),
    ("error", CalloutType::Danger),
    ("bug", CalloutType::Bug),
    ("example", CalloutType::Example),
    ("quote", CalloutType::Quote),
    ("cite", CalloutType::Quote),
    ("abstract", CalloutType::Abstract),
    ("summary", CalloutType::Abstract),
    ("tldr", CalloutType::Abstract),
    ("todo", CalloutType::Todo),
];

impl CalloutType {
    /// The type of a name; [`CalloutType::Unknown`] when it is not in the table.
    pub fn of_name(name: &str) -> CalloutType {
        NAMES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map_or(CalloutType::Unknown, |(_, t)| *t)
    }

    /// The canonical name: the class exports use, and the debug tree. An unknown type has the
    /// note style, so its class is `note`; the name as written stays in the source.
    pub fn as_str(self) -> &'static str {
        match self {
            CalloutType::Note | CalloutType::Unknown => "note",
            CalloutType::Info => "info",
            CalloutType::Tip => "tip",
            CalloutType::Important => "important",
            CalloutType::Success => "success",
            CalloutType::Question => "question",
            CalloutType::Warning => "warning",
            CalloutType::Caution => "caution",
            CalloutType::Failure => "failure",
            CalloutType::Danger => "danger",
            CalloutType::Bug => "bug",
            CalloutType::Example => "example",
            CalloutType::Quote => "quote",
            CalloutType::Abstract => "abstract",
            CalloutType::Todo => "todo",
        }
    }

    /// The theme colour of this type.
    pub fn accent(self) -> CalloutAccent {
        match self {
            CalloutType::Note
            | CalloutType::Info
            | CalloutType::Abstract
            | CalloutType::Todo
            | CalloutType::Unknown => CalloutAccent::Note,
            CalloutType::Tip | CalloutType::Success => CalloutAccent::Tip,
            CalloutType::Important | CalloutType::Example => CalloutAccent::Important,
            CalloutType::Warning | CalloutType::Question => CalloutAccent::Warning,
            CalloutType::Caution | CalloutType::Failure | CalloutType::Danger | CalloutType::Bug => {
                CalloutAccent::Caution
            }
            CalloutType::Quote => CalloutAccent::Quote,
        }
    }

    fn icon(self) -> CalloutIcon {
        match self {
            CalloutType::Note | CalloutType::Unknown => CalloutIcon::Pencil,
            CalloutType::Info => CalloutIcon::Info,
            CalloutType::Tip => CalloutIcon::Bulb,
            CalloutType::Important => CalloutIcon::Star,
            CalloutType::Success => CalloutIcon::Check,
            CalloutType::Question => CalloutIcon::Question,
            CalloutType::Warning | CalloutType::Caution => CalloutIcon::Warning,
            CalloutType::Failure => CalloutIcon::Cross,
            CalloutType::Danger => CalloutIcon::Bolt,
            CalloutType::Bug => CalloutIcon::Bug,
            CalloutType::Example => CalloutIcon::List,
            CalloutType::Quote => CalloutIcon::Quote,
            CalloutType::Abstract => CalloutIcon::Lines,
            CalloutType::Todo => CalloutIcon::Task,
        }
    }
}

/// The folding a tag declares: `-` starts closed, `+` starts open, nothing is not foldable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fold {
    None,
    Open,
    Closed,
}

/// What a quote's first line declared. Holds no spans or text: the source keeps those.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Callout {
    pub ty: CalloutType,
    pub fold: Fold,
    /// Set for the bare form of a GitHub alert name (`[!NOTE]`), the narrow case.
    pub alert: Option<AlertKind>,
}

impl Callout {
    /// The format-neutral kind of this callout.
    pub(crate) fn kind(self) -> CalloutKind {
        match self.ty {
            CalloutType::Note => CalloutKind::Note,
            CalloutType::Tip => CalloutKind::Tip,
            CalloutType::Important => CalloutKind::Important,
            CalloutType::Warning => CalloutKind::Warning,
            CalloutType::Caution => CalloutKind::Caution,
            t => CalloutKind::Other(t.as_str().into()),
        }
    }
}

/// A callout tag on a line. Offsets are bytes into the line handed to [`parse_tag`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag<'a> {
    pub name: &'a str,
    pub fold: Fold,
    /// End of `[!name]` and its fold mark.
    pub end: usize,
    /// The title's text, without the white space around it.
    pub title: Option<Range<usize>>,
}

impl Tag<'_> {
    /// The alert kind of a bare tag: a GitHub alert name, no fold mark, no title.
    fn alert(&self) -> Option<AlertKind> {
        if self.fold != Fold::None || self.title.is_some() {
            return None;
        }
        Some(match CalloutType::of_name(self.name) {
            CalloutType::Note if self.name.eq_ignore_ascii_case("note") => AlertKind::Note,
            CalloutType::Tip if self.name.eq_ignore_ascii_case("tip") => AlertKind::Tip,
            CalloutType::Important => AlertKind::Important,
            CalloutType::Warning if self.name.eq_ignore_ascii_case("warning") => AlertKind::Warning,
            CalloutType::Caution => AlertKind::Caution,
            _ => return None,
        })
    }
}

fn valid_name(name: &str) -> bool {
    name.len() <= MAX_NAME
        && name.chars().next().is_some_and(char::is_alphanumeric)
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
}

/// Reads `[!name]`, an optional fold mark and an optional title from a line of text. White
/// space must separate the title from the tag, so `[!x](url)` stays a link.
pub fn parse_tag(line: &str) -> Option<Tag<'_>> {
    let start = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = line[start..].strip_prefix("[!")?;
    let close = rest.find(']')?;
    let name = &rest[..close];
    if !valid_name(name) {
        return None;
    }
    let mut end = start + 2 + close + 1;
    let fold = match line.as_bytes().get(end) {
        Some(b'-') => Fold::Closed,
        Some(b'+') => Fold::Open,
        _ => Fold::None,
    };
    if fold != Fold::None {
        end += 1;
    }
    let after = &line[end..];
    let trimmed = after.trim_start_matches([' ', '\t']);
    if !trimmed.is_empty() && trimmed.len() == after.len() {
        return None; // text right after the tag
    }
    let title = trimmed.trim_end_matches([' ', '\t']);
    let at = line.len() - trimmed.len();
    Some(Tag {
        name,
        fold,
        end,
        title: (!title.is_empty()).then(|| at..at + title.len()),
    })
}

/// The parser's hook: the callout a quote's first line declares, under the options. `alerts`
/// governs the bare GitHub forms, `callouts` everything else.
pub(crate) fn recognise(first_line: &str, opts: &ParseOptions) -> Option<Callout> {
    if !opts.alerts && !opts.callouts {
        return None;
    }
    let tag = parse_tag(first_line)?;
    let alert = tag.alert();
    if !(if alert.is_some() { opts.alerts } else { opts.callouts }) {
        return None;
    }
    Some(Callout {
        ty: CalloutType::of_name(tag.name),
        fold: tag.fold,
        alert,
    })
}

/// Whether `line`, exactly, is a tag alone that the options turn into a callout: the immediate
/// input rule of a quote's first line.
pub(crate) fn is_bare_tag(line: &str, opts: &ParseOptions) -> bool {
    !line.starts_with([' ', '\t'])
        && recognise(line, opts).is_some()
        && parse_tag(line).is_some_and(|t| t.title.is_none() && t.fold == Fold::None && t.end == line.len())
}

/// The title of a callout with none written: its name, lower case, the first letter a capital.
pub fn default_title(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What an export writes for a callout that is not an alert: its class, the name as written,
/// the title and the folding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportHead<'a> {
    pub ty: CalloutType,
    pub fold: Fold,
    /// The name as written.
    pub name: &'a str,
    /// The written title, or `None` for [`ExportHead::label`].
    pub title: Option<&'a str>,
    /// The default title.
    pub label: String,
}

/// The export head of an extended callout whose first line (the quote's first paragraph's) is
/// `first_line`. `None` for a plain quote and for an alert, which keeps its own output.
pub fn export_head<'a>(callout: Option<Callout>, first_line: &'a str) -> Option<ExportHead<'a>> {
    let c = callout.filter(|c| c.alert.is_none())?;
    let tag = parse_tag(first_line)?;
    Some(ExportHead {
        ty: c.ty,
        fold: c.fold,
        name: tag.name,
        title: tag.title.map(|r| &first_line[r]),
        label: default_title(tag.name),
    })
}

/// The neutral view of quote `b` (spans relative to `origin`), for layout.
pub(crate) fn view(text: &str, b: &Block, origin: usize) -> Option<Arc<CalloutView>> {
    let BlockKind::BlockQuote { callout: Some(c) } = &b.kind else {
        return None;
    };
    let first = b.children.first()?.lines.first()?;
    let line = &text[first.start + origin..first.end + origin];
    let tag = parse_tag(line)?;
    Some(Arc::new(CalloutView {
        accent: c.ty.accent(),
        icon: c.alert.is_none().then(|| c.ty.icon()),
        open: match c.fold {
            Fold::None => None,
            Fold::Open => Some(true),
            Fold::Closed => Some(false),
        },
        title: tag.title.as_ref().map(|t| t.start as u32..t.end as u32),
        tag_end: tag.end as u32,
        label: default_title(tag.name).into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(s: &str) -> Option<(String, Fold, Option<String>)> {
        parse_tag(s).map(|t| {
            let title = t.title.clone().map(|r| s[r].to_string());
            (t.name.to_string(), t.fold, title)
        })
    }

    #[test]
    fn the_first_line_grammar() {
        assert_eq!(tag("[!warning]"), Some(("warning".into(), Fold::None, None)));
        assert_eq!(tag(" [!WARNING]  "), Some(("WARNING".into(), Fold::None, None)));
        assert_eq!(tag("[!a]- T"), Some(("a".into(), Fold::Closed, Some("T".into()))));
        assert_eq!(tag("[!a]+"), Some(("a".into(), Fold::Open, None)));
        assert_eq!(
            tag("[!my-type_2] **T** x  "),
            Some(("my-type_2".into(), Fold::None, Some("**T** x".into())))
        );
        assert_eq!(
            tag("[!заметка] Т"),
            Some(("заметка".into(), Fold::None, Some("Т".into())))
        );
        for no in [
            "[!]", "[! a]", "[!a b]", "[!-a]", "[!a", "[a]", "![!a]", "[!a]x", "[!a](u)", "[!a]-x", "[!a]+-", "x [!a]",
            "[!a.b]",
        ] {
            assert_eq!(tag(no), None, "{no}");
        }
        assert_eq!(tag(&format!("[!{}]", "n".repeat(MAX_NAME + 1))), None);
    }

    #[test]
    fn every_name_of_the_table_maps_to_a_style() {
        for (name, t) in NAMES {
            assert_eq!(CalloutType::of_name(name), *t);
            assert_eq!(CalloutType::of_name(&name.to_ascii_uppercase()), *t);
            assert_ne!(*t, CalloutType::Unknown);
        }
        assert_eq!(CalloutType::of_name("mine"), CalloutType::Unknown);
        assert_eq!(CalloutType::Unknown.accent(), CalloutType::Note.accent());
    }

    #[test]
    fn default_titles_are_the_capitalised_name() {
        assert_eq!(default_title("WARNING"), "Warning");
        assert_eq!(default_title("faq"), "Faq");
        assert_eq!(default_title("my-type"), "My-type");
        assert_eq!(default_title("ЗАМЕТКА"), "Заметка");
    }

    #[test]
    fn bare_alert_names_are_the_narrow_case_and_only_they() {
        let o = ParseOptions::default();
        let alert = |s: &str| recognise(s, &o).and_then(|c| c.alert);
        assert_eq!(alert("[!note]"), Some(AlertKind::Note));
        assert_eq!(alert("[!CAUTION]"), Some(AlertKind::Caution));
        assert_eq!(alert("[!hint]"), None, "an alias is not an alert name");
        assert_eq!(alert("[!attention]"), None);
        assert_eq!(alert("[!note] T"), None);
        assert_eq!(alert("[!note]-"), None);
        assert_eq!(alert("[!info]"), None);
    }

    #[test]
    fn the_two_options_govern_their_own_forms() {
        let off = ParseOptions {
            callouts: false,
            ..Default::default()
        };
        assert!(recognise("[!note]", &off).is_some(), "alerts stay on");
        assert!(recognise("[!note] T", &off).is_none());
        assert!(recognise("[!info]", &off).is_none());
        let no_alerts = ParseOptions {
            alerts: false,
            ..Default::default()
        };
        assert!(recognise("[!note]", &no_alerts).is_none());
        assert!(recognise("[!note] T", &no_alerts).is_some());
        let none = ParseOptions {
            alerts: false,
            callouts: false,
            ..Default::default()
        };
        assert!(recognise("[!note]", &none).is_none());
    }
}
