//! Why a package was refused.

use std::fmt;

use crate::limits::MAX_ISSUES;

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

/// Collects the issues of one validation run.
#[derive(Default)]
pub(crate) struct Issues(Vec<Issue>);

impl Issues {
    pub(crate) fn push(&mut self, file: &str, message: impl Into<String>) {
        if self.0.len() < MAX_ISSUES {
            self.0.push(Issue {
                file: file.to_string(),
                message: message.into(),
            });
        }
    }

    pub(crate) fn into_result<T>(self, ok: impl FnOnce() -> T) -> Result<T, PackageError> {
        if self.0.is_empty() {
            Ok(ok())
        } else {
            Err(PackageError::Invalid(self.0))
        }
    }
}
