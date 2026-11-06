//! A new section's id from its title (ADR 0026): Latin only, by a fixed table, so that the same title gives
//! the same id on any machine and in any language setting. An id is a letter, then up to 63 letters, digits,
//! `_` and `-`, as sites read it.

use std::collections::BTreeSet;

const MAX_ID: usize = 64;

/// The Cyrillic letters of the table, lower case, with what each is written as. Anything else that is
/// no ASCII letter or digit is no part of an id, whatever script it is in.
pub const CYRILLIC: &[(char, &str)] = &[
    ('а', "a"),
    ('б', "b"),
    ('в', "v"),
    ('г', "g"),
    ('ґ', "g"),
    ('д', "d"),
    ('е', "e"),
    ('ё', "e"),
    ('є', "ye"),
    ('ж', "zh"),
    ('з', "z"),
    ('и', "i"),
    ('і', "i"),
    ('ї', "yi"),
    ('й', "y"),
    ('к', "k"),
    ('л', "l"),
    ('м', "m"),
    ('н', "n"),
    ('о', "o"),
    ('п', "p"),
    ('р', "r"),
    ('с', "s"),
    ('т', "t"),
    ('у', "u"),
    ('ў', "u"),
    ('ф', "f"),
    ('х', "kh"),
    ('ц', "ts"),
    ('ч', "ch"),
    ('ш', "sh"),
    ('щ', "shch"),
    ('ъ', ""),
    ('ы', "y"),
    ('ь', ""),
    ('э', "e"),
    ('ю', "yu"),
    ('я', "ya"),
];

/// A section's id can be this: a letter, then up to 63 letters, digits, `_` and `-`.
pub fn is_section_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= MAX_ID
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// `title` as an id, before it is made unique: empty when it has no letter to start with.
fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in title.chars().flat_map(char::to_lowercase) {
        let latin = CYRILLIC.iter().find(|(k, _)| *k == c).map(|(_, v)| *v);
        let piece = match (latin, c) {
            (Some(v), _) => v.to_string(),
            (None, 'a'..='z' | '0'..='9' | '_') => c.to_string(),
            // inside a word, these join it
            (None, '\'' | '’' | 'ʼ') => continue,
            _ => {
                gap = true;
                continue;
            }
        };
        if piece.is_empty() {
            continue;
        }
        if out.is_empty() {
            // an id starts with a letter
            let from_letter = piece.trim_start_matches(|c: char| !c.is_ascii_lowercase());
            out.push_str(from_letter);
        } else {
            if gap {
                out.push('-');
            }
            out.push_str(&piece);
        }
        gap = false;
    }
    cut_to(&out, MAX_ID)
}

/// At most `max` bytes of an id made here, with no separator left at its end.
fn cut_to(id: &str, max: usize) -> String {
    id[..id.len().min(max)].trim_end_matches(['-', '_']).to_string()
}

/// The id for a section with this title, not in `taken`: its letters, or with no letters `section-1`, the
/// first free number. A taken one gets the first free number after a hyphen, `-2` on, within the length.
pub fn section_id(title: &str, taken: &BTreeSet<String>) -> String {
    let base = slug(title);
    if base.is_empty() {
        return (1u64..)
            .map(|n| format!("section-{n}"))
            .find(|id| !taken.contains(id))
            .expect("finitely many ids are taken");
    }
    if !taken.contains(&base) {
        return base;
    }
    (2u64..)
        .map(|n| {
            let tail = format!("-{n}");
            format!("{}{tail}", cut_to(&base, MAX_ID - tail.len()))
        })
        .find(|id| !taken.contains(id))
        .expect("finitely many ids are taken")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free(title: &str) -> String {
        section_id(title, &BTreeSet::new())
    }

    #[test]
    fn a_title_becomes_latin_lower_case_with_hyphens() {
        for (title, want) in [
            ("Hello, World!", "hello-world"),
            ("  Two   words ", "two-words"),
            ("snake_case stays", "snake_case-stays"),
            ("Don't stop", "dont-stop"),
            ("Увидели маячок. Как действовать?", "uvideli-mayachok-kak-deystvovat"),
            ("Щука и Ёж", "shchuka-i-ezh"),
            ("Объём съезда", "obem-sezda"),
            ("Їжак і ґанок", "yizhak-i-ganok"),
            ("Café crème", "caf-cr-me"),
            ("3 шага", "shaga"),
            ("1. Первый", "pervyy"),
        ] {
            assert_eq!(free(title), want, "{title:?}");
            assert!(is_section_id(&free(title)), "{title:?}");
        }
    }

    #[test]
    fn a_title_with_no_usable_letters_gets_a_number() {
        for title in ["", "123", "!!! ...", "日本語", "ъь"] {
            assert_eq!(free(title), "section-1", "{title:?}");
        }
        let taken: BTreeSet<String> = ["section-1".to_string(), "section-2".to_string()].into();
        assert_eq!(section_id("123", &taken), "section-3");
    }

    #[test]
    fn a_taken_id_gets_a_number_within_the_length() {
        let taken: BTreeSet<String> = ["intro".to_string(), "intro-2".to_string()].into();
        assert_eq!(section_id("Intro", &taken), "intro-3");
        let long = "a".repeat(200);
        let id = free(&long);
        assert_eq!(id.len(), 64);
        let id = section_id(&long, &[id].into());
        assert_eq!(id.len(), 64);
        assert!(id.ends_with("-2") && is_section_id(&id), "{id}");
    }

    #[test]
    fn the_cut_leaves_no_separator_at_the_end() {
        let title = format!("{} b", "a".repeat(63));
        assert_eq!(free(&title), "a".repeat(63));
    }

    #[test]
    fn the_grammar_is_a_letter_then_up_to_63_of_these() {
        for good in ["a", "A1", "a_b-c", &"x".repeat(64)] {
            assert!(is_section_id(good), "{good}");
        }
        for bad in ["", "1a", "-a", "_a", "a b", "a{", "я", &"x".repeat(65)] {
            assert!(!is_section_id(bad), "{bad}");
        }
    }
}
