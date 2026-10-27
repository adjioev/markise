//! Fence info string → syntax definition (spec 01 R3.5.2 and Appendix A).
//!
//! The language is the first word of the info string, compared case-insensitively. The table
//! below lists every Appendix A canonical name and alias; each maps to the name of a syntax in
//! the bundled syntax set, or to `None` when the set has no definition for it (shown plain).
//! Words that are not in the table fall back to the syntax set's own lookup by file extension
//! or name (`rs`, `py`, `tsx`, `zig`, …). The special render languages (`mermaid`, `sequence`,
//! `flow`, `math`) are never highlighted.

/// `(info word, syntax name)`; `None` = known language without a bundled definition.
/// Keys are lowercase. Appendix A order, then common extra spellings.
pub(crate) const ALIASES: &[(&str, Option<&str>)] = &[
    ("asp", Some("ASP")),
    ("aspx", Some("ASP")),
    ("asp.net", Some("ASP")),
    ("javascript", Some("JavaScript")),
    ("js", Some("JavaScript")),
    ("text/javascript", Some("JavaScript")),
    ("json", Some("JSON")),
    ("typescript", Some("TypeScript")),
    ("ts", Some("TypeScript")),
    ("clojure", Some("Clojure")),
    ("coffeescript", Some("CoffeeScript")),
    ("coffee", Some("CoffeeScript")),
    ("css", Some("CSS")),
    ("less", Some("Less")),
    ("scss", Some("SCSS")),
    ("gfm", Some("Markdown")),
    ("markdown", Some("Markdown")),
    ("xml", Some("XML")),
    ("haskell", Some("Haskell")),
    ("html", Some("HTML")),
    ("htmlmixed", Some("HTML")),
    ("lua", Some("Lua")),
    ("commonlisp", Some("Lisp")),
    ("lisp", Some("Lisp")),
    ("pascal", Some("Pascal")),
    ("perl", Some("Perl")),
    ("php", Some("PHP")),
    ("php+html", Some("PHP")),
    ("cython", Some("Python")),
    ("python", Some("Python")),
    ("ruby", Some("Ruby")),
    ("shell", Some("Bourne Again Shell (bash)")),
    ("sh", Some("Bourne Again Shell (bash)")),
    ("bash", Some("Bourne Again Shell (bash)")),
    ("sql", Some("SQL")),
    ("sqlite", Some("SQL")),
    ("mssql", Some("SQL")),
    ("mysql", Some("SQL")),
    ("mariadb", Some("SQL")),
    ("cql", Some("SQL")),
    ("cassandra", Some("SQL")),
    ("plsql", Some("SQL")),
    ("tex", Some("LaTeX")),
    ("stex", Some("LaTeX")),
    ("latex", Some("LaTeX")),
    ("tiddlywiki", None),
    ("wiki", None),
    ("vb", None),
    ("basic", None),
    ("vbscript", None),
    ("velocity", None),
    ("verilog", Some("Verilog")),
    ("xquery", None),
    ("yaml", Some("YAML")),
    ("go", Some("Go")),
    ("groovy", Some("Groovy")),
    ("nginx", Some("nginx")),
    ("octave", Some("MATLAB")),
    ("matlab", Some("MATLAB")),
    ("oz", None),
    ("c", Some("C")),
    ("clike", Some("C")),
    ("c++", Some("C++")),
    ("cpp", Some("C++")),
    ("cc", Some("C++")),
    ("objective-c", Some("Objective-C")),
    ("obj-c", Some("Objective-C")),
    ("objc", Some("Objective-C")),
    ("scala", Some("Scala")),
    ("c#", Some("C#")),
    ("csharp", Some("C#")),
    ("java", Some("Java")),
    ("squirrel", None),
    ("ceylon", None),
    ("kotlin", Some("Kotlin")),
    ("swift", Some("Swift")),
    ("r", Some("R")),
    ("rlang", Some("R")),
    ("r-lang", Some("R")),
    ("d", Some("D")),
    ("diff", Some("Diff")),
    ("erlang", Some("Erlang")),
    ("http", Some("HTTP Request and Response")),
    ("jade", None),
    ("rst", Some("reStructuredText")),
    ("restructuredtext", Some("reStructuredText")),
    ("rust", Some("Rust")),
    ("jinja2", Some("Jinja2")),
    ("jsp", Some("Java Server Page (JSP)")),
    ("erb", Some("HTML (Rails)")),
    ("embeddedjs", None),
    ("ejs", None),
    // the pure-Rust regex backend cannot compile the bundled PowerShell grammar
    ("powershell", None),
    ("dockerfile", Some("Dockerfile")),
    ("jsx", Some("JavaScript")),
    ("react", Some("JavaScript")),
    ("vue", Some("Vue Component")),
    ("vue.js", Some("Vue Component")),
    ("vue-template", Some("Vue Component")),
    ("nsis", Some("NSIS")),
    ("tiki", None),
    ("tikiwiki", None),
    ("tiki-wiki", None),
    ("properties", Some("Java Properties")),
    ("ini", Some("INI")),
    ("livescript", Some("LiveScript")),
    ("assembly", Some("x86_64 Assembly")),
    ("asm", Some("x86_64 Assembly")),
    ("nasm", Some("x86_64 Assembly")),
    ("gas", Some("x86_64 Assembly")),
    ("toml", Some("TOML")),
    ("ocaml", Some("OCaml")),
    ("f#", Some("F#")),
    ("elm", Some("Elm")),
    ("elixir", Some("Elixir")),
    ("spreadsheet", None),
    ("pgp", None),
    ("asciiarmor", None),
    ("pgp-keys", None),
    ("cmake", Some("CMake")),
    ("cypher", None),
    ("dart", Some("Dart")),
    ("django", Some("HTML (Jinja2)")),
    ("dtd", None),
    ("xml-dtd", None),
    ("dylan", None),
    ("handlebars", None),
    ("idl", None),
    ("web-idl", None),
    ("yacas", None),
    ("mbox", Some("Email")),
    ("bhdl", None),
    ("julia", Some("Julia")),
    ("haxe", None),
    ("hxml", None),
    ("pseudocode", None),
    ("sas", None),
    ("twig", Some("HTML (Twig)")),
    ("scheme", Some("Lisp")),
    ("tcl", Some("Tcl")),
    ("makefile", Some("Makefile")),
    ("protobuf", Some("Protocol Buffer")),
    ("fortran", Some("Fortran (Modern)")),
    ("cobol", None),
    ("sparql", None),
    ("crystal", Some("Crystal")),
    ("asn.1", None),
    ("gherkin", None),
    ("smalltalk", None),
    ("turtle", None),
    ("glsl", Some("GLSL")),
    ("apl", None),
    ("abap", None),
    ("q", None),
    ("v", None),
    ("forth", None),
    ("nim", Some("Nim")),
    ("solidity", Some("Solidity")),
    ("modelica", None),
    ("structured", None),
    ("url", None),
    // common spellings beyond Appendix A
    ("rs", Some("Rust")),
    ("py", Some("Python")),
    ("python3", Some("Python")),
    ("rb", Some("Ruby")),
    ("zsh", Some("Bourne Again Shell (bash)")),
    ("console", Some("Bourne Again Shell (bash)")),
    ("shell-session", Some("Bourne Again Shell (bash)")),
    ("yml", Some("YAML")),
    ("md", Some("Markdown")),
    ("tsx", Some("TypeScriptReact")),
    ("cs", Some("C#")),
    ("fsharp", Some("F#")),
    ("kt", Some("Kotlin")),
    ("golang", Some("Go")),
    ("make", Some("Makefile")),
    ("proto", Some("Protocol Buffer")),
    ("patch", Some("Diff")),
    ("docker", Some("Dockerfile")),
    ("jsonc", Some("JSON")),
    ("json5", Some("JSON")),
    ("hcl", Some("Terraform")),
    ("terraform", Some("Terraform")),
];

/// Languages rendered instead of highlighted (spec 01 R3.5.6).
const SPECIAL: &[&str] = &["mermaid", "sequence", "flow", "math"];

/// What a fence language resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    /// A bundled syntax definition (its name in the syntax set).
    Syntax(&'static str),
    /// Plain code: no language, an unknown one, a known one without a bundled definition, or a
    /// special render language.
    Plain,
}

/// The first word of a fence info string, lowercased (spec 01 R3.5.2).
pub fn info_language(info: &str) -> String {
    info.split_whitespace().next().unwrap_or("").to_lowercase()
}

/// Resolves a fence language (first info word, any case) through the Appendix A table only
/// (no syntax-set fallback, no loading). `None` = not in the table.
pub fn table_lookup(lang: &str) -> Option<Option<&'static str>> {
    let l = lang.to_lowercase();
    ALIASES.iter().find(|(k, _)| *k == l).map(|(_, v)| *v)
}

/// Resolves a fence language (first info word, any case).
pub fn resolve(lang: &str) -> Language {
    let l = lang.trim().to_lowercase();
    if l.is_empty() || SPECIAL.contains(&l.as_str()) {
        return Language::Plain;
    }
    if let Some(v) = table_lookup(&l) {
        return v.map_or(Language::Plain, Language::Syntax);
    }
    // syntax-set fallback: extensions and names (case-insensitive)
    let set = super::syntax_set();
    match set.find_syntax_by_token(&l) {
        Some(s) if s.name != "Plain Text" => Language::Syntax(static_name(&s.name)),
        _ => Language::Plain,
    }
}

/// The `'static` name of a syntax of the global set.
fn static_name(name: &str) -> &'static str {
    super::syntax_set()
        .syntaxes()
        .iter()
        .find(|s| s.name == name)
        .map_or("Plain Text", |s| s.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 01 Appendix A: every table entry with a syntax names a syntax of the bundled set.
    #[test]
    fn appendix_a_targets_exist() {
        let set = crate::highlight::syntax_set();
        for (k, v) in ALIASES {
            if let Some(name) = v {
                assert!(set.find_syntax_by_name(name).is_some(), "{k} → {name} missing");
            }
            assert_eq!(*k, k.to_lowercase(), "keys are lowercase");
        }
        let mut keys: Vec<_> = ALIASES.iter().map(|(k, _)| *k).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(n, keys.len(), "duplicate alias");
    }

    /// R3.5.2: case-insensitive; Appendix A aliases.
    #[test]
    fn aliases_case_insensitive() {
        assert_eq!(resolve("Rust"), Language::Syntax("Rust"));
        assert_eq!(resolve("JS"), Language::Syntax("JavaScript"));
        assert_eq!(resolve("text/javascript"), Language::Syntax("JavaScript"));
        assert_eq!(resolve("ts"), Language::Syntax("TypeScript"));
        assert_eq!(resolve("C++"), Language::Syntax("C++"));
        assert_eq!(resolve("cpp"), Language::Syntax("C++"));
        assert_eq!(resolve("c#"), Language::Syntax("C#"));
        assert_eq!(resolve("obj-c"), Language::Syntax("Objective-C"));
        assert_eq!(resolve("sh"), Language::Syntax("Bourne Again Shell (bash)"));
        assert_eq!(resolve("BASH"), Language::Syntax("Bourne Again Shell (bash)"));
        assert_eq!(resolve("yaml"), Language::Syntax("YAML"));
        assert_eq!(resolve("htmlmixed"), Language::Syntax("HTML"));
        assert_eq!(resolve("r-lang"), Language::Syntax("R"));
        assert_eq!(resolve("ini"), Language::Syntax("INI"));
        assert_eq!(resolve("latex"), Language::Syntax("LaTeX"));
    }

    #[test]
    fn plain_languages() {
        for l in [
            "",
            "mermaid",
            "Math",
            "flow",
            "sequence",
            "vbscript",
            "no-such-lang",
            "cobol",
            "txt",
        ] {
            assert_eq!(resolve(l), Language::Plain, "{l}");
        }
    }

    #[test]
    fn fallback_by_extension() {
        assert_eq!(resolve("zig"), Language::Syntax("Zig"));
        assert_eq!(resolve("nix"), Language::Syntax("Nix"));
    }

    #[test]
    fn info_first_word() {
        assert_eq!(info_language("  Rust  {.numberLines}"), "rust");
        assert_eq!(info_language(""), "");
    }
}
