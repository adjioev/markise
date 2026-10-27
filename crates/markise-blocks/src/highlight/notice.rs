//! The license notice of the bundled syntax definitions, shipped in release archives as
//! `THIRD_PARTY/syntax-licenses.md` (generated from two-face's embedded acknowledgements; a test
//! keeps the committed file in sync — regenerate with `MARKISE_BLESS=1 cargo test -p
//! markise-blocks syntax_license_notice`).

/// Sublime HQ Packages license (syntaxes/01_Packages/LICENSE), applies to the files of that
/// repository that carry no license of their own.
const SUBLIME_LICENSE: &str = "\
Permission to copy, use, modify, sell and distribute this
software is granted. This software is provided \"as is\" without
express or implied warranty, and with no claim as to its
suitability for any purpose.";

/// The full notice text.
pub fn syntax_license_notice() -> String {
    let mut md = String::from(
        "# Syntax definitions bundled with Markise\n\n\
         Markise highlights code with syntect and the syntax definitions of the two-face crate \
         (v0.5.2+bat-0.26.1, MIT OR Apache-2.0, <https://codeberg.org/CosmicHarper/two-face>), \
         curated by the bat project (<https://github.com/sharkdp/bat>); only the definitions \
         that work with syntect's pure-Rust regex engine are included.\n\n\
         ## Sublime HQ Packages (syntaxes/01_Packages)\n\n\
         Files of <https://github.com/sublimehq/Packages> without their own license information \
         fall under:\n\n````text\n",
    );
    md.push_str(SUBLIME_LICENSE);
    md.push_str(
        "\n````\n\n\
         ## Public-domain style licenses (no notice required; listed for completeness)\n\n\
         - GLSL (syntaxes/02_Extra/GLSL): public-domain dedication.\n\
         - PureScript (syntaxes/02_Extra/PureScript/LICENSE): The Unlicense \
           (its NOTICE file, MIT, is reproduced below).\n\
         - GraphQL (syntaxes/02_Extra/GraphQL): WTFPL.\n\n\
         ## Licenses requiring attribution (full texts)\n\n",
    );
    for l in two_face::acknowledgement::listing().for_syntaxes() {
        l.write_md(&mut md);
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_license_notice_is_committed_and_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../THIRD_PARTY/syntax-licenses.md");
        let want = syntax_license_notice();
        if std::env::var_os("MARKISE_BLESS").is_some() {
            std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
            std::fs::write(path, &want).unwrap();
        }
        let have = std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n");
        assert!(
            have == want,
            "THIRD_PARTY/syntax-licenses.md is stale: regenerate with MARKISE_BLESS=1"
        );
        assert!(want.contains("syntaxes/02_Extra/TypeScript/LICENSE"));
        assert!(want.contains("Permission to copy, use, modify, sell"));
    }
}
