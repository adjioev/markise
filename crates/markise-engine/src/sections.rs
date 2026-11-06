//! Whether a document has sections, and who says so (ADR 0026).

use markise_package::{PackageId, PackageIndex};

/// The one authoring dialect in which `---` divides a document. Front matter or a package name it;
/// what it means to their site is not known here.
pub const SECTIONS_DIALECT: &str = "blocks-v3";

/// Who named a document's dialect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialectSource {
    FrontMatter,
    Package(PackageId),
}

/// The dialect a document is in, when anything names one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialect {
    pub name: String,
    pub source: DialectSource,
}

impl Dialect {
    /// Sections are on.
    pub fn has_sections(&self) -> bool {
        self.name == SECTIONS_DIALECT
    }

    /// Where the dialect came from, in words.
    pub fn origin(&self) -> String {
        match &self.source {
            DialectSource::FrontMatter => "the document's front matter".to_string(),
            DialectSource::Package(id) => format!("package {} {}", id.library, id.version),
        }
    }
}

/// The dialect of a document with this front matter value and this accepted package. Front matter
/// wins, whatever it names.
pub fn section_dialect(front_matter: Option<&str>, package: Option<&PackageIndex>) -> Option<Dialect> {
    if let Some(name) = front_matter {
        return Some(Dialect {
            name: name.to_string(),
            source: DialectSource::FrontMatter,
        });
    }
    let package = package?;
    Some(Dialect {
        name: package.authoring()?.to_string(),
        source: DialectSource::Package(package.id().clone()),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn package(extra: &str) -> PackageIndex {
        let manifest = format!(
            r#"{{"format":"markise-block-package/2","library":"sample","version":"1.2.0","blocks":["b"]{extra}}}"#
        );
        let files: BTreeMap<String, Vec<u8>> = [
            ("manifest.json".to_string(), manifest.into_bytes()),
            (
                "components/b/b.json".to_string(),
                br#"{"fields":[{"name":"t","type":"text"}]}"#.to_vec(),
            ),
            (
                "editor/palette.json".to_string(),
                br#"{"groups":[{"id":"g","blocks":["b"]}]}"#.to_vec(),
            ),
        ]
        .into_iter()
        .collect();
        PackageIndex::from_files(&files).unwrap()
    }

    #[test]
    fn nothing_named_is_no_dialect() {
        assert_eq!(section_dialect(None, None), None);
        assert_eq!(section_dialect(None, Some(&package(""))), None);
    }

    #[test]
    fn a_package_names_it_and_says_which_version() {
        let p = package(r#","authoring":"blocks-v3""#);
        let d = section_dialect(None, Some(&p)).unwrap();
        assert!(d.has_sections());
        assert_eq!(d.origin(), "package sample 1.2.0");
    }

    #[test]
    fn front_matter_is_enough_and_wins_both_ways() {
        let d = section_dialect(Some(SECTIONS_DIALECT), None).unwrap();
        assert!(d.has_sections());
        assert_eq!(d.source, DialectSource::FrontMatter);
        let on = package(r#","authoring":"blocks-v3""#);
        assert!(!section_dialect(Some("blocks-v2"), Some(&on)).unwrap().has_sections());
        let off = package(r#","authoring":"blocks-v2""#);
        assert!(!section_dialect(None, Some(&off)).unwrap().has_sections());
        assert!(
            section_dialect(Some(SECTIONS_DIALECT), Some(&off))
                .unwrap()
                .has_sections()
        );
    }

    #[test]
    fn a_manifest_value_that_is_no_text_is_ignored_and_refuses_nothing() {
        assert_eq!(package(r#","authoring":3"#).authoring(), None);
        assert_eq!(package(r#","authoring":"""#).authoring(), None);
    }
}
