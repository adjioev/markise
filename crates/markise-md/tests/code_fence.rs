//! Fenced code blocks behind the format seam (spec 02 §7, issue #76): the fence view (info
//! string, language word, content as stored) and the language edit, which rewrites only the
//! language word of the info string.

use markise_doc::FormatDoc;
use markise_md::{Document, ParseOptions};

fn parse(s: &str) -> Document {
    Document::parse(s, ParseOptions::default())
}

/// `src` with the language of the code block holding `at` changed to `lang`.
fn relang(src: &str, at: usize, lang: &str) -> String {
    let mut d = parse(src);
    match d.code_lang_patch(at, lang) {
        Some(p) => {
            FormatDoc::apply(&mut d, &p);
            d.text().to_string()
        }
        None => src.to_string(),
    }
}

#[test]
fn fence_view_reports_info_language_and_content() {
    let src = "a\n\n```rust {.x}\nfn main() {}\n  x\n```\n";
    let d = parse(src);
    let at = src.find("fn").unwrap();
    let f = d.code_fence(at).unwrap();
    assert_eq!(f.info.slice(src), "rust {.x}");
    assert_eq!(f.lang(src).slice(src), "rust");
    assert_eq!(f.content(src), "fn main() {}\n  x");
    // from the opening fence and the closing fence too
    assert_eq!(d.code_fence(src.find("```").unwrap()), Some(f.clone()));
    assert_eq!(d.code_fence(src.rfind("```").unwrap()), Some(f));
    assert_eq!(d.code_fence(0), None, "a paragraph");
}

#[test]
fn fence_view_without_info_points_after_the_fence() {
    let src = "~~~~\ncode\n~~~~\n";
    let f = parse(src).code_fence(5).unwrap();
    assert_eq!(f.info, markise_doc::Span::new(4, 4));
    assert_eq!(f.lang(src).slice(src), "");
}

#[test]
fn content_keeps_crlf_and_container_prefixes_out() {
    let src = "```sh\r\necho 1\r\n\r\necho 2\r\n```\r\n";
    let f = parse(src).code_fence(7).unwrap();
    assert_eq!(f.content(src), "echo 1\r\n\r\necho 2");
    let q = "> ```py\n> a = 1\n>   b\n> ```\n";
    let f = parse(q).code_fence(q.find("a =").unwrap()).unwrap();
    assert_eq!(f.content(q), "a = 1\n  b");
    let empty = "```js\n```\n";
    assert_eq!(parse(empty).code_fence(0).unwrap().content(empty), "");
}

#[test]
fn language_edit_rewrites_only_the_language_word() {
    assert_eq!(relang("```rust\nx\n```\n", 8, "sh"), "```sh\nx\n```\n");
    // tilde and long fences keep their characters
    assert_eq!(relang("~~~rust\nx\n~~~\n", 8, "sh"), "~~~sh\nx\n~~~\n");
    assert_eq!(relang("````rust\nx\n````\n", 9, "sh"), "````sh\nx\n````\n");
    // trailing attributes and spacing are kept byte for byte
    assert_eq!(
        relang("```  rust   {.x #y}\nx\n```\n", 21, "python"),
        "```  python   {.x #y}\nx\n```\n"
    );
    // CRLF documents
    assert_eq!(relang("```rust\r\nx\r\n```\r\n", 9, "go"), "```go\r\nx\r\n```\r\n");
    // inside a quote
    let q = "> ```rust\n> x\n> ```\n";
    assert_eq!(relang(q, q.find("x\n").unwrap(), "c"), "> ```c\n> x\n> ```\n");
}

#[test]
fn language_edit_adds_and_removes_the_language() {
    assert_eq!(relang("```\nx\n```\n", 4, "js"), "```js\nx\n```\n");
    assert_eq!(relang("~~~~~\nx\n~~~~~\n", 6, "c++"), "~~~~~c++\nx\n~~~~~\n");
    // clearing removes the info string with the space before it
    assert_eq!(relang("``` rust\nx\n```\n", 9, ""), "```\nx\n```\n");
    assert_eq!(relang("```rust\nx\n```\n", 8, "   "), "```\nx\n```\n");
    // …but keeps attributes that follow it
    assert_eq!(relang("```rust {.x}\nx\n```\n", 13, ""), "``` {.x}\nx\n```\n");
}

#[test]
fn language_edit_refuses_what_a_fence_cannot_hold() {
    // no change: no edit
    assert!(parse("```rust\nx\n```\n").code_lang_patch(8, "rust").is_none());
    assert!(parse("para\n").code_lang_patch(0, "rust").is_none());
    // backticks after a backtick fence would end it; line breaks would split the line
    assert_eq!(relang("```rust\nx\n```\n", 8, "s`h\n"), "```sh\nx\n```\n");
    // a tilde fence may hold backticks
    assert_eq!(relang("~~~rust\nx\n~~~\n", 8, "a`b"), "~~~a`b\nx\n~~~\n");
}

#[test]
fn language_edit_reparses_into_a_diagram_language() {
    let mut d = parse("```rust\ngraph TD; A-->B\n```\n");
    let p = d.code_lang_patch(8, "mermaid").unwrap();
    FormatDoc::apply(&mut d, &p);
    let leaf = d.leaf(8).unwrap();
    assert!(matches!(leaf.kind, markise_doc::LeafKind::CodeBlock { ref info } if &**info == "mermaid"));
}
