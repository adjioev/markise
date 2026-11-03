//! Issue #92: measure real shaping with deterministic partial-coverage fonts.
use epaint::Color32;
use epaint::text::{FontData, FontDefinitions, Fonts};
use markise_layout::{
    block::{FontSet, RoleFaces, register_families},
    testing,
    text::{FontRole, ParleyShaper, StyledText, TextShaper, TextStyle},
};
use markise_text::testing::TestFont;
use std::sync::Arc;

fn face(f: TestFont) -> (String, Arc<FontData>) {
    (f.family.clone(), Arc::new(FontData::from_owned(f.build())))
}
fn latin() -> TestFont {
    TestFont::new("Latin only").with_range(' ', '~', 300)
}
fn multilingual(name: &str, advance: u16) -> TestFont {
    TestFont::new(name)
        .with_range(' ', '~', advance)
        .with_chars("РеактПриветΕλληνικά中文", advance)
}
fn definitions(fallback: TestFont) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    register_families(
        &mut defs,
        &FontSet {
            body: RoleFaces {
                regular: vec![face(latin()), face(fallback)],
                bold: vec![],
            },
            ..Default::default()
        },
    );
    defs
}
fn style() -> TextStyle {
    TextStyle::new(20.0, 28.0, FontRole::Body, Color32::BLACK)
}

#[test]
fn mixed_adjacent_words_use_one_face_on_both_backends() {
    let defs = definitions(multilingual("Multilingual", 600));
    let mut fonts = Fonts::new(testing::text_options(), defs.clone());
    let mut shaper = fonts.with_pixels_per_point(1.0);
    let mut parley = ParleyShaper::from_font_definitions(&defs);
    for sh in [&mut shaper as &mut dyn TextShaper, &mut parley] {
        for sample in ["React Реакт", "React Ελληνικά", "React 中文", "ReactРеакт"] {
            let block = sh.shape(&StyledText::single(sample, style()), None);
            let cs = block.clusters();
            let first = cs.iter().find(|c| c.ch == 'R').unwrap();
            let non_latin = cs.iter().find(|c| !c.ch.is_ascii()).unwrap();
            assert!(
                (first.advance - non_latin.advance).abs() < 0.1,
                "{sample}: {} vs {}",
                first.advance,
                non_latin.advance
            );
            assert!((block.first_line_height().unwrap() - 28.0).abs() < 1.0);
        }
    }
}
#[test]
fn fallback_matches_x_height_without_expanding_the_line_box() {
    let mut fallback = multilingual("Tall", 600);
    fallback.x_height = 625;
    let defs = definitions(fallback);
    let mut fonts = Fonts::new(testing::text_options(), defs.clone());
    let mut shaper = fonts.with_pixels_per_point(1.0);
    let mut parley = ParleyShaper::from_font_definitions(&defs);
    for sh in [&mut shaper as &mut dyn TextShaper, &mut parley] {
        let block = sh.shape(&StyledText::single("React Реакт", style()), None);
        // 20 * (500 / 625) * (600 / 1000), allowing pixel snapping in epaint.
        assert!((block.clusters()[0].advance - 9.6).abs() < 0.6);
        assert!((block.first_line_height().unwrap() - 28.0).abs() < 1.0);
    }
}
#[test]
fn fallback_prefers_matching_serif_style_and_weight() {
    let mut primary = latin();
    primary.serif = Some(false);
    let mut wrong = multilingual("Serif", 400);
    wrong.serif = Some(true);
    let mut right = multilingual("Sans", 700);
    right.serif = Some(false);
    let mut defs = FontDefinitions::default();
    register_families(
        &mut defs,
        &FontSet {
            body: RoleFaces {
                regular: vec![face(primary.clone()), face(wrong), face(right)],
                bold: vec![
                    face(primary.style(700, false)),
                    face(multilingual("Wrong weight", 400)),
                    face(multilingual("Bold fallback", 800).style(700, false)),
                ],
            },
            ..Default::default()
        },
    );
    let mut fonts = Fonts::new(testing::text_options(), defs);
    let mut sh = fonts.with_pixels_per_point(1.0);
    let regular = sh.shape(&StyledText::single("React Реакт", style()), None);
    assert!((regular.clusters()[0].advance - 14.0).abs() < 0.1);
    let mut bold = style();
    bold.role = FontRole::Bold;
    bold.italic = true;
    let block = sh.shape(&StyledText::single("React Реакт", bold), None);
    assert!((block.clusters()[0].advance - 16.0).abs() < 0.1);
}
#[test]
fn script_override_applies_to_mixed_prose_and_can_be_removed() {
    let mut defs = definitions(multilingual("Automatic", 600));
    let mut set = FontSet::default();
    set.body.regular = vec![face(latin()), face(multilingual("Automatic", 600))];
    set.scripts.insert(
        "cyrillic".into(),
        RoleFaces {
            regular: vec![face(multilingual("Chosen", 800))],
            bold: vec![],
        },
    );
    register_families(&mut defs, &set);
    let mut fonts = Fonts::new(testing::text_options(), defs);
    let mut sh = fonts.with_pixels_per_point(1.0);
    let text = sh.shape(&StyledText::single("React Реакт", style()), None);
    assert!((text.clusters()[0].advance - 16.0).abs() < 0.1);
    let ascii = sh.shape(&StyledText::single("React", style()), None);
    assert!((ascii.clusters()[0].advance - 6.0).abs() < 0.1);
    let mut fonts = Fonts::new(testing::text_options(), definitions(multilingual("Automatic", 600)));
    let text = fonts
        .with_pixels_per_point(1.0)
        .shape(&StyledText::single("React Реакт", style()), None);
    assert!((text.clusters()[0].advance - 12.0).abs() < 0.1);
}

#[test]
fn script_only_override_is_honoured_when_it_cannot_cover_latin() {
    let mut set = FontSet::default();
    set.body.regular = vec![face(multilingual("Automatic", 600))];
    set.scripts.insert(
        "cyrillic".into(),
        RoleFaces {
            regular: vec![face(TestFont::new("Cyrillic only").with_chars("Реакт", 800))],
            bold: vec![],
        },
    );
    let mut defs = FontDefinitions::default();
    register_families(&mut defs, &set);
    let mut fonts = Fonts::new(testing::text_options(), defs.clone());
    let mut epaint = fonts.with_pixels_per_point(1.0);
    let mut parley = ParleyShaper::from_font_definitions(&defs);
    for sh in [&mut epaint as &mut dyn TextShaper, &mut parley] {
        let block = sh.shape(&StyledText::single("React Реакт", style()), None);
        let cs = block.clusters();
        assert!((cs.iter().find(|c| c.ch == 'R').unwrap().advance - 12.0).abs() < 0.1);
        assert!((cs.iter().find(|c| c.ch == 'Р').unwrap().advance - 16.0).abs() < 0.1);
    }
}
