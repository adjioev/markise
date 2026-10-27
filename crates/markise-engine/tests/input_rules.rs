//! M2c input rules on the headless editing surface: immediate rules (spec 02 §2.1) with
//! the literal-hint history of ADR 0006 §2.6, Enter rules (§2.2), auto-pair (§4), smart
//! punctuation (§13). Notation: `‸` caret, `(v)‸` virtual paragraph.

mod common;
use common::T;
use markise_doc::{EditPrefs, SmartPunctuation};

fn step(t: &mut T, k: &str) {
    if let Some(s) = k.strip_prefix("type:") {
        t.typ(s);
    } else {
        t.run(k);
    }
}

fn script(src: &str, steps: &[(&str, &str, &str)]) {
    script_with(src, EditPrefs::default(), steps)
}

/// Steps: (key, shown text, debug-tree substring expected).
fn script_with(src: &str, prefs: EditPrefs, steps: &[(&str, &str, &str)]) {
    let mut t = T::at(src);
    t.ed.session.set_prefs(prefs);
    for (k, want, tree) in steps {
        step(&mut t, k);
        assert_eq!(t.shown(), *want, "after {k} (from {src:?})");
        let dt = t.ed.session.doc().debug_tree();
        assert!(dt.contains(tree), "after {k}: tree {dt:?} lacks {tree:?}");
        assert!(t.caret_visible(), "caret not drawable after {k}: {want:?}");
        assert!(
            t.selection_on_stops(),
            "caret off the stops after {k}: {want:?} → {}",
            t.shown()
        );
        t.check_model();
    }
}

#[test]
fn s2_2_bullet_rule_and_undo_restores_the_literal_text() {
    script(
        "‸",
        &[
            ("type:-", "-‸", "para"),
            ("type: ", "- ‸", "ul"),
            ("edit.undo", "- ‸", "para \"- \""),
            ("edit.undo", "-‸", "para"),
            ("edit.redo", "- ‸", "para \"- \""),
            ("edit.redo", "- ‸", "ul"),
            ("type:milk", "- milk‸", "li"),
        ],
    );
}

#[test]
fn spec02_2_1_rules_one_per_row() {
    script("‸", &[("type:> ", "> ‸", "quote")]);
    script("‸", &[("type:* ", "* ‸", "ul")]);
    script("‸", &[("type:+ ", "+ ‸", "ul")]);
    // S-2.3: the start number is kept, Enter continues it
    script(
        "‸",
        &[
            ("type:3. ", "3. ‸", "ol"),
            ("type:a", "3. a‸", "ol"),
            ("edit.newline", "3. a\n4. ‸", "ol"),
        ],
    );
    script("‸", &[("type:1) ", "1) ‸", "ol")]);
    // `- [ ] ` / `- [x] ` → task item (the `[ ] ` part converts inside the new item)
    script("‸", &[("type:- [ ] ", "- [ ] ‸", "li[ ]")]);
    script("‸", &[("type:- [x] ", "- [x] ‸", "li[x]")]);
    // `[ ] ` at the start of a bullet item's first paragraph
    script("- ‸text\n", &[("type:[ ] ", "- [ ] ‸text\n", "li[ ]")]);
    // callout: the tag line of a quote, the body is a fresh (virtual) paragraph
    script(
        "> ‸",
        &[
            ("type:[!NOTE]", "> [!NOTE](v)‸", "alert-note"),
            ("type:x", "> [!NOTE]\n>\n> x‸", "alert-note"),
        ],
    );
    // remaining text is kept
    script("‸milk\n", &[("type:- ", "- ‸milk\n", "ul")]);
}

#[test]
fn rules_inside_containers_keep_the_containers() {
    script("> ‸", &[("type:- ", "> - ‸", "quote\n  ul")]);
    script("- ‸a\n", &[("type:> ", "- > ‸a\n", "li\n    quote")]);
    // nested list from an empty item
    script("- a\n- ‸", &[("type:- ", "- a\n- - ‸", "ul")]);
    // a heading typed inside a quote stays a hinted paragraph in the quote until Enter
    script(
        "> ‸",
        &[
            ("type:## Plan", "> ## Plan‸", "quote"),
            ("edit.newline", "> ## Plan(v)‸", "h2"),
        ],
    );
}

#[test]
fn c9_no_rule_on_a_soft_broken_line() {
    // `a⏎-` would be a setext heading in CommonMark: the line stays hinted paragraph text
    script("a\n‸", &[("type:- ", "a\n- ‸", "para \"a\\n- \"")]);
    script("a\n‸", &[("type:===", "a\n===‸", "para")]);
}

#[test]
fn s2_1_heading_stays_a_paragraph_until_enter() {
    script(
        "‸",
        &[
            ("type:## Plan", "## Plan‸", "para"),
            ("edit.newline", "## Plan(v)‸", "h2"),
            ("type:x", "## Plan\n\nx‸", "h2"),
        ],
    );
    script("‸", &[("type:---", "---‸", "para"), ("edit.newline", "---(v)‸", "hr")]);
    // after a materialised virtual paragraph (Enter at the end of a paragraph)
    script(
        "x‸",
        &[
            ("edit.newline", "x(v)‸", "para"),
            ("type:## Plan", "x\n\n## Plan‸", "para \"## Plan\""),
        ],
    );
    script(
        "‸",
        &[("type:[toc]", "[toc]‸", "para"), ("edit.newline", "[toc](v)‸", "toc")],
    );
    script(
        "‸",
        &[
            ("type:[^n]: note", "[^n]: note‸", "para"),
            ("edit.newline", "[^n]: note(v)‸", "fn[n]"),
        ],
    );
    script(
        "p\n\n‸",
        &[
            ("type:[x]: http://a", "p\n\n[x]: http://a‸", "para"),
            ("edit.newline", "p(v)‸\n\n[x]: http://a", "def"),
        ],
    );
    // C43: an empty heading becomes an empty paragraph
    script(
        "a\n\n‸",
        &[("type:## ", "a\n\n## ‸", "para"), ("edit.newline", "a(v)‸", "para")],
    );
}

#[test]
fn auto_pair_brackets_and_quotes() {
    script(
        "‸",
        &[
            ("type:(", "(‸)", "para"),
            ("type:a", "(a‸)", ""),
            ("type:)", "(a)‸", ""),
        ],
    );
    script("x ‸", &[("type:[", "x [‸]", ""), ("edit.deleteBackward", "x ‸", "")]);
    // no pairing before a word, after a letter (apostrophe) or after a backslash
    script("‸word", &[("type:(", "(‸word", "")]);
    script("don‸", &[("type:'", "don'‸", "")]);
    script("\\‸", &[("type:(", "\\(‸", "")]);
    script(
        "say ‸",
        &[("type:\"", "say \"‸\"", ""), ("type:hi\"", "say \"hi\"‸", "")],
    );
    // a selection is wrapped
    let mut t = T::new("a word b");
    t.ed.set_selection(markise_doc::Selection::range(2, 6));
    t.typ("(");
    assert_eq!(t.shown(), "a (⟦word⟧) b");
}

#[test]
fn auto_pair_markdown_is_off_by_default_and_wraps_when_on() {
    script("‸", &[("type:*", "*‸", "")]);
    let prefs = EditPrefs {
        auto_pair_markdown: true,
        ..Default::default()
    };
    script_with(
        "‸",
        prefs,
        &[
            ("type:*", "*‸*", ""),
            ("type:b", "*b‸*", ""),
            ("type:*", "*b*‸", "para"),
        ],
    );
    // `* ` with Markdown pairing: the stray pair character goes, a list is created
    script_with("‸", prefs, &[("type:* ", "* ‸", "ul")]);
    script_with("snake‸", prefs, &[("type:_", "snake_‸", "")]);
}

#[test]
fn smart_punctuation_converts_on_input() {
    let prefs = EditPrefs {
        smart_punctuation: SmartPunctuation::ConvertOnInput,
        ..Default::default()
    };
    script_with(
        "say ‸",
        prefs,
        &[
            ("type:\"", "say \u{201C}‸\u{201D}", ""),
            ("type:hi\"", "say \u{201C}hi\u{201D}‸", ""),
        ],
    );
    script_with("don‸", prefs, &[("type:'", "don\u{2019}‸", "")]);
    script_with(
        "a‸",
        prefs,
        &[
            ("type:-", "a-‸", ""),
            ("type:-", "a\u{2013}‸", ""),
            ("type:-", "a\u{2014}‸", ""),
            ("edit.undo", "a\u{2013}-‸", ""),
        ],
    );
    // never at a line start (list markers / rules)
    script_with("‸", prefs, &[("type:--", "--‸", "")]);
}

#[test]
fn adr_2_6_undone_rule_materialises_an_escape_when_left() {
    let mut t = T::at("‸\n\nother\n");
    t.typ("- ");
    t.run("edit.undo");
    assert_eq!(t.shown(), "- ‸\n\nother\n");
    t.run("caret.down");
    assert_eq!(t.text(), "\\- \n\nother\n", "escaped so it saves as what was shown");
    assert!(t.ed.session.doc().debug_tree().starts_with("para"));
    t.run("edit.undo");
    assert_eq!(t.text(), "- \n\nother\n");
}

#[test]
fn c42_shift_enter_on_a_blank_soft_line_acts_as_enter() {
    script("foo\n‸bar", &[("edit.softBreak", "foo\n\n‸bar", "para")]);
    script("foo\n‸", &[("edit.softBreak", "foo(v)‸", "para")]);
    script("foo  \n‸", &[("edit.softBreak", "foo(v)‸", "para")]);
    script("foo\nba‸r", &[("edit.softBreak", "foo\nba\n‸r", "para")]);
    script("> a‸b", &[("edit.softBreak", "> a\n> ‸b", "quote")]);
    script("- a‸b", &[("edit.softBreak", "- a\n  ‸b", "li")]);
}

#[test]
fn c44_shift_enter_in_headings() {
    script("# Long‸ title", &[("edit.softBreak", "# Long<br/>‸ title", "h1")]);
    script("Long‸ title\n===", &[("edit.softBreak", "Long\n ‸title\n===", "h1")]);
    script("Title‸\n===", &[("edit.softBreak", "Title<br/>‸\n===", "h1")]);
    script("‸Title\n===", &[("edit.softBreak", "<br/>‸Title\n===", "h1")]);
    script("a‸ - b\n===", &[("edit.softBreak", "a<br/>‸ - b\n===", "h1")]);
    script("a‸ ===\n---", &[("edit.softBreak", "a<br/>‸ ===\n---", "h2")]);
}
