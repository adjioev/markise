//! Run-level font choice shared by the two rendering adapters. No font discovery or I/O.
use super::{FontRole, StyledText};
use epaint::text::{FontDefinitions, FontFamily};
use markise_text::fallback::{Profile, covers, required_chars, script};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub fn variant(base: &str, index: usize) -> String {
    format!("{base}:face:{index}")
}
pub fn script_family(base: &str, script: &str) -> String {
    format!("{base}:script:{script}")
}

/// Each choice retains the rest of its role's chain for symbols, emoji and missing glyphs.
/// Called when definitions change, not per frame.
pub fn register_variants(defs: &mut FontDefinitions) {
    let families: Vec<_> = defs
        .families
        .iter()
        .filter_map(|(family, names)| match family {
            FontFamily::Name(n) if n.starts_with("markise-") && !n.contains(":face:") => {
                Some((n.to_string(), names.clone()))
            }
            _ => None,
        })
        .collect();
    for (base, names) in families {
        for (i, name) in names.iter().enumerate() {
            let mut chain = vec![name.clone()];
            chain.extend(names.iter().filter(|n| *n != name).cloned());
            defs.families.insert(FontFamily::Name(variant(&base, i).into()), chain);
        }
    }
}
#[derive(Clone, Debug)]
pub struct Choice {
    pub family: String,
    pub scale: f32,
}
#[derive(Clone, Debug)]
pub struct Run {
    pub range: Range<usize>,
    pub choice: Option<Choice>,
}

fn choice(defs: &FontDefinitions, base: &str, index: usize, scale: f32) -> Option<Choice> {
    let alias = variant(base, index);
    let family = if defs.families.contains_key(&FontFamily::Name(alias.clone().into())) {
        alias
    } else if index == 0 {
        // Consumers may supply role families directly, without our additional aliases.
        base.to_owned()
    } else {
        return None;
    };
    Some(Choice { family, scale })
}

fn choose(defs: &FontDefinitions, role: FontRole, chars: &[char], word: bool) -> Option<Choice> {
    if chars.is_empty() {
        return None;
    }
    let base = role.family_name();
    let chain = defs.families.get(&FontFamily::Name(base.into()))?;
    let primary = defs.font_data.get(chain.first()?)?;
    let profile = Profile::read(primary.font.as_ref().as_ref(), primary.index)?;
    let bold = matches!(role, FontRole::Bold | FontRole::HeadingBold | FontRole::MonoBold);
    let family = base.to_string();
    let names = chain;
    // A script-only user font still applies: try the paragraph first, then fall back to
    // words when it cannot also cover neighbouring scripts. Explicit choices outrank
    // automatic run unification; missing glyphs retain the role's tail.
    if !matches!(role, FontRole::Mono | FontRole::MonoBold) {
        let mut overrides = Vec::new();
        for s in ["cyrillic", "greek", "cjk"] {
            let own: Vec<_> = chars.iter().copied().filter(|&c| script(c) == Some(s)).collect();
            if own.is_empty() {
                continue;
            }
            let family = script_family(base, s);
            if let Some(data) = defs
                .families
                .get(&FontFamily::Name(family.clone().into()))
                .and_then(|names| names.first())
                .and_then(|name| defs.font_data.get(name))
                && covers(data.font.as_ref().as_ref(), data.index, &own)
            {
                overrides.push((family, data));
            }
        }
        if !word && overrides.len() > 1 {
            return None;
        }
        if let Some((family, data)) = overrides.first() {
            if !word && !covers(data.font.as_ref().as_ref(), data.index, chars) {
                return None;
            }
            let p = Profile::read(data.font.as_ref().as_ref(), data.index)?;
            return choice(defs, family, 0, p.scale_to(profile));
        }
    }
    let mut best = None;
    for (i, name) in names.iter().enumerate() {
        let Some(data) = defs.font_data.get(name) else { continue };
        if !covers(data.font.as_ref().as_ref(), data.index, chars) {
            continue;
        }
        let Some(p) = Profile::read(data.font.as_ref().as_ref(), data.index) else {
            continue;
        };
        if i == 0 {
            return choice(defs, &family, i, if family == base { 1.0 } else { p.scale_to(profile) });
        }
        let score = p.distance(profile, bold);
        if best.as_ref().is_none_or(|(old, _, _)| score < *old) {
            best = Some((score, i, p));
        }
    }
    best.and_then(|(_, i, p)| choice(defs, &family, i, p.scale_to(profile)))
}

/// Resolves once per paragraph and role, not once per bold/italic/color span. This keeps
/// heavily formatted mixed-script paragraphs from repeatedly scanning the same text.
pub struct Resolver<'a> {
    defs: &'a FontDefinitions,
    text: &'a StyledText,
    whole: Vec<char>,
    ascii: bool,
    choices: [Option<Option<Choice>>; 6],
}
impl<'a> Resolver<'a> {
    pub fn new(defs: &'a FontDefinitions, text: &'a StyledText) -> Self {
        let ascii = text.text.is_ascii();
        Self {
            defs,
            text,
            ascii,
            whole: if ascii { Vec::new() } else { required_chars(&text.text) },
            choices: Default::default(),
        }
    }
    /// Prefer one face across adjacent words/style sections, then whole Unicode words.
    /// Only an uncovered word delegates to the backend's existing cluster fallback.
    pub fn runs(&mut self, role: FontRole, range: Range<usize>) -> Vec<Run> {
        if self.ascii {
            return vec![Run { range, choice: None }];
        }
        let choice = self.choices[role.index()].get_or_insert_with(|| choose(self.defs, role, &self.whole, false));
        if let Some(choice) = choice {
            return vec![Run {
                range,
                choice: Some(choice.clone()),
            }];
        }
        self.text.text[range.clone()]
            .split_word_bound_indices()
            .map(|(i, word)| Run {
                range: range.start + i..range.start + i + word.len(),
                choice: choose(self.defs, role, &required_chars(word), true),
            })
            .collect()
    }
}
