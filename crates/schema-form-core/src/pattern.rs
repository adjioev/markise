//! `pattern`: what a string must look like, as a package or any other outsider writes it. So the
//! engine runs in time linear in the text (the `regex` crate has no backtracking, so no catastrophic
//! case), and what can grow is bounded: the pattern, what it compiles to, and the text it reads.

use regex::{Regex, RegexBuilder};

/// The longest pattern, in bytes.
pub const MAX_PATTERN_BYTES: usize = 1024;
/// What a pattern may compile to, in bytes. `(a{1000}){1000}` is short and would be far more.
const MAX_COMPILED_BYTES: usize = 256 << 10;
const MAX_NESTING: u32 = 32;
/// The longest text a pattern reads, in bytes. A longer one does not match, unread: the time a match
/// takes is bounded by this times the compiled size, and the check runs on the UI thread.
pub const MAX_MATCHED_BYTES: usize = 4096;

/// A compiled `pattern`, unanchored as in JSON Schema: `^` and `$` say so where it matters.
#[derive(Clone, Debug)]
pub struct Pattern {
    source: String,
    regex: Regex,
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Pattern) -> bool {
        self.source == other.source
    }
}

impl Pattern {
    /// `Err` says why in one line: too long, too large, or not a pattern this engine has.
    pub fn new(source: &str) -> Result<Pattern, String> {
        if source.len() > MAX_PATTERN_BYTES {
            return Err(format!(
                "is {} bytes long, the limit is {MAX_PATTERN_BYTES}",
                source.len()
            ));
        }
        let regex = RegexBuilder::new(source)
            .size_limit(MAX_COMPILED_BYTES)
            .nest_limit(MAX_NESTING)
            .build()
            .map_err(|e| match e {
                regex::Error::CompiledTooBig(_) => "is too large once compiled".to_string(),
                // several lines with a caret under the place; the last says what is wrong
                e => {
                    let text = e.to_string();
                    let line = text.lines().last().unwrap_or("").trim_start_matches("error: ");
                    format!("cannot be used: {line}")
                }
            })?;
        Ok(Pattern {
            source: source.to_string(),
            regex,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn is_match(&self, text: &str) -> bool {
        text.len() <= MAX_MATCHED_BYTES && self.regex.is_match(text)
    }
}
