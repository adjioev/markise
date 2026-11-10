//! Why a package was refused.

use std::fmt;

use crate::limits::{MAX_ISSUES, MAX_NAME_BYTES};

/// One problem, with the package file it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub file: String,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

/// A package this build will not use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageError {
    /// The manifest names a format this build does not read (an older or a newer one).
    UnsupportedFormat { found: String },
    /// The package breaks the format: every problem found, up to [`MAX_ISSUES`].
    Invalid(Vec<Issue>),
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageError::UnsupportedFormat { found } => {
                write!(
                    f,
                    "package format \"{found}\" is not supported by this version of Markise (it reads {})",
                    crate::FORMAT
                )
            }
            PackageError::Invalid(issues) => {
                for (i, issue) in issues.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{issue}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for PackageError {}

/// Collects the issues of one validation run, and the notes: what a newer package may say that this
/// build does not read, which refuses nothing (ADR 0019).
#[derive(Default)]
pub(crate) struct Issues {
    issues: Vec<Issue>,
    notes: Vec<Issue>,
}

impl Issues {
    pub(crate) fn push(&mut self, file: &str, message: impl Into<String>) {
        if self.issues.len() < MAX_ISSUES {
            self.issues.push(Issue {
                file: file.to_string(),
                message: message.into(),
            });
        }
    }

    /// A note for every key of `map` not in `known`: kept or ignored as data for later versions, and
    /// said, so that whoever wrote it knows this build does nothing with it.
    pub(crate) fn unknown_keys(
        &mut self,
        file: &str,
        what: &str,
        map: &serde_json::Map<String, serde_json::Value>,
        known: &[&str],
    ) {
        for key in map.keys().filter(|k| !known.contains(&k.as_str())) {
            if self.notes.len() >= MAX_ISSUES {
                return;
            }
            let shown: String = key.chars().take(MAX_NAME_BYTES).collect();
            self.notes.push(Issue {
                file: file.to_string(),
                message: format!("{what} has the key \"{shown}\", which this version of Markise does not read"),
            });
        }
    }

    pub(crate) fn into_result<T>(self, ok: impl FnOnce(Vec<Issue>) -> T) -> Result<T, PackageError> {
        if self.issues.is_empty() {
            Ok(ok(self.notes))
        } else {
            Err(PackageError::Invalid(self.issues))
        }
    }
}
