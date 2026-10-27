//! PDF and PNG export structure tests (spec 04 §8.1): page count, extractable text, outline,
//! link annotations, tagging, header/footer variables (A17), cancellation.
//!
//! Text needs at least one system font; on a machine without any fonts the text assertions
//! are skipped (reported on stderr).

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use lopdf::{Document as Pdf, Object};
use markise_export::job::JobControl;
use markise_export::model::{ExDoc, ModelOptions};
use markise_export::vars::Vars;
use markise_pdf::samples::{CODE_SAMPLES, SAMPLE};
use markise_pdf::{PdfOptions, PngOptions, export_pdf, export_png, system_engine};
use markise_text::TextEngine;

fn engine() -> std::sync::MutexGuard<'static, TextEngine> {
    static E: OnceLock<Mutex<TextEngine>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(system_engine(Duration::from_secs(30))))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn model(md: &str) -> ExDoc {
    ExDoc::build(&markise_md::parse(md), &ModelOptions { highlight: true })
}

fn pdf_of(md: &str, opts: PdfOptions) -> (Vec<u8>, usize) {
    let doc = model(md);
    let mut o = opts;
    o.apply_front_matter(&doc);
    let vars = Vars::new().with_front_matter(&doc.meta_yaml);
    let mut v = vars;
    v.set("today", "2026-10-27");
    let out = export_pdf(&mut engine(), &doc, &o, &v, &JobControl::new()).expect("pdf export");
    (out.files[0].clone(), out.pages)
}

fn text_of(pdf: &Pdf) -> String {
    let pages: Vec<u32> = pdf.get_pages().keys().copied().collect();
    pdf.extract_text(&pages).unwrap_or_default()
}

fn has_fonts() -> bool {
    let mut e = engine();
    let st = markise_text::StyledText::plain("abc", markise_text::TextStyle::default());
    let l = e.layout(&st, None, 1.0);
    l.glyph_runs().iter().any(|r| r.glyphs.iter().any(|g| g.id != 0))
}

#[test]
fn sample_pdf_structure() {
    let (bytes, pages) = pdf_of(SAMPLE, PdfOptions::default());
    assert!(bytes.starts_with(b"%PDF-"));
    let pdf = Pdf::load_mem(&bytes).expect("lopdf parses the PDF");
    assert_eq!(pdf.get_pages().len(), pages);
    assert!(pages >= 3, "the sample spans several pages, got {pages}");

    let catalog = pdf.catalog().unwrap();
    // tagged PDF
    assert!(catalog.get(b"StructTreeRoot").is_ok(), "struct tree present");
    assert!(catalog.get(b"MarkInfo").is_ok());
    // outline: one entry per heading (7 in the sample), nested
    let outlines = pdf
        .get_dictionary(catalog.get(b"Outlines").unwrap().as_reference().unwrap())
        .unwrap();
    let mut titles = Vec::new();
    fn walk(pdf: &Pdf, first: Option<&Object>, out: &mut Vec<String>) {
        let mut cur = first.and_then(|o| o.as_reference().ok());
        while let Some(r) = cur {
            let d = pdf.get_dictionary(r).unwrap();
            if let Ok(Object::String(s, _)) = d.get(b"Title") {
                out.push(
                    lopdf::decode_text_string(&Object::String(s.clone(), lopdf::StringFormat::Literal))
                        .unwrap_or_default(),
                );
            }
            walk(pdf, d.get(b"First").ok(), out);
            cur = d.get(b"Next").ok().and_then(|o| o.as_reference().ok());
        }
    }
    walk(&pdf, outlines.get(b"First").ok(), &mut titles);
    assert_eq!(titles.len(), 7, "{titles:?}");
    assert_eq!(titles[0], "Field Notes on Lighthouses");
    assert!(titles.contains(&"Lamp types".to_string()));

    // link annotations: external URI and internal GoTo/Dest
    let mut uris = Vec::new();
    let mut internal = 0;
    for (_, page_id) in pdf.get_pages() {
        let page = pdf.get_dictionary(page_id).unwrap();
        let Ok(annots) = page.get(b"Annots") else { continue };
        let annots = match annots {
            Object::Array(a) => a.clone(),
            Object::Reference(r) => pdf.get_object(*r).unwrap().as_array().unwrap().clone(),
            _ => continue,
        };
        for a in annots {
            let d = pdf.get_dictionary(a.as_reference().unwrap()).unwrap();
            if let Ok(action) = d.get(b"A") {
                let action = match action {
                    Object::Reference(r) => pdf.get_dictionary(*r).unwrap().clone(),
                    Object::Dictionary(d) => d.clone(),
                    _ => continue,
                };
                if let Ok(Object::String(u, _)) = action.get(b"URI") {
                    uris.push(String::from_utf8_lossy(u).into_owned());
                }
                if action.get(b"D").is_ok() {
                    internal += 1;
                }
            } else if d.get(b"Dest").is_ok() {
                internal += 1;
            }
        }
    }
    assert!(uris.iter().any(|u| u == "https://example.org/harbour"), "{uris:?}");
    // TOC entries (7), "the lamp table", the footnote reference
    assert!(internal >= 9, "internal links: {internal}");

    let info = pdf.trailer.get(b"Info").and_then(|i| i.as_reference()).ok();
    if let Some(r) = info {
        let d = pdf.get_dictionary(r).unwrap();
        assert!(d.get(b"Title").is_ok());
    }

    if !has_fonts() {
        eprintln!("no system fonts: skipping text extraction checks");
        return;
    }
    // lopdf breaks lines between text objects: compare without whitespace
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let text = squash(&text_of(&pdf));
    for needle in [
        "Field Notes on Lighthouses",
        "harbour office",
        "Never look straight into a lit lens",
        "Pressure",
        "fn beam(angle: f32)",
        "Fog signals were sounded",
        // header and footer variables (A17)
        "Ada Keeper",
        "Page 1 of",
        "2026-10-27",
    ] {
        assert!(
            text.contains(&squash(needle)),
            "missing {needle:?} in extracted text:\n{text}"
        );
    }
    assert!(!text.contains("${"), "unexpanded variable in {text}");
}

#[test]
fn page_size_margins_and_breaks() {
    let md = "---\npapersize: letter\nmargin: 1in\n---\n# One\n\ntext\n\n# Two\n\ntext\n";
    let opts = PdfOptions {
        break_before_h1: true,
        ..PdfOptions::default()
    };
    let (bytes, pages) = pdf_of(md, opts);
    assert_eq!(pages, 2, "page break before the second h1");
    let pdf = Pdf::load_mem(&bytes).unwrap();
    let (_, first) = pdf.get_pages().into_iter().next().unwrap();
    let mb = pdf
        .get_dictionary(first)
        .unwrap()
        .get(b"MediaBox")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    let w = mb[2].as_float().unwrap();
    let h = mb[3].as_float().unwrap();
    assert!((w - 612.0).abs() < 0.5 && (h - 792.0).abs() < 0.5, "{w}×{h}");
}

#[test]
fn code_samples_pdf_and_png() {
    let (bytes, pages) = pdf_of(CODE_SAMPLES, PdfOptions::default());
    assert_eq!(pages, 1);
    assert!(Pdf::load_mem(&bytes).is_ok());
    let doc = model(CODE_SAMPLES);
    let out = export_png(&mut engine(), &doc, &PngOptions::default(), &JobControl::new()).unwrap();
    assert_eq!(out.files.len(), 1);
    let img = image::load_from_memory(&out.files[0]).unwrap().to_rgba8();
    assert_eq!(img.width(), 1280, "640 CSS px at scale 2");
    assert!(img.height() > 600);
    // not a blank image: the code boxes and text change the pixels
    let first = *img.get_pixel(0, 0);
    let distinct = img.pixels().filter(|p| **p != first).count();
    assert!(distinct > 10_000, "{distinct}");
}

#[test]
fn tall_png_is_tiled() {
    let md: String = (0..1500)
        .map(|i| format!("Paragraph {i} with some text.\n\n"))
        .collect();
    let doc = model(&md);
    let out = export_png(
        &mut engine(),
        &doc,
        &PngOptions {
            scale: 1.0,
            ..PngOptions::default()
        },
        &JobControl::new(),
    )
    .unwrap();
    assert!(out.files.len() >= 2, "{} tiles", out.files.len());
    for f in &out.files {
        let img = image::load_from_memory(f).unwrap();
        assert!(img.height() <= markise_pdf::png::MAX_TILE);
    }
}

#[test]
fn cancellation_stops_export() {
    let doc = model(SAMPLE);
    let c = JobControl::new();
    c.cancel();
    let r = export_pdf(&mut engine(), &doc, &PdfOptions::default(), &Vars::new(), &c);
    assert!(matches!(r, Err(markise_pdf::ExportError::Cancelled)));
    let r = export_png(&mut engine(), &doc, &PngOptions::default(), &c);
    assert!(matches!(r, Err(markise_pdf::ExportError::Cancelled)));
}

#[test]
fn dark_theme_falls_back_to_light_for_print() {
    let doc = model("# Dark\n");
    let opts = PdfOptions {
        theme: markise_theme::Theme::dark(),
        ..PdfOptions::default()
    };
    let out = export_pdf(&mut engine(), &doc, &opts, &Vars::new(), &JobControl::new()).unwrap();
    assert!(out.warnings.iter().any(|w| w.contains("dark theme")));
}

#[test]
fn missing_and_remote_images_get_placeholders() {
    let doc = model("![alt one](missing.png)\n\n![remote](https://example.org/x.png)\n");
    let out = export_pdf(
        &mut engine(),
        &doc,
        &PdfOptions::default(),
        &Vars::new(),
        &JobControl::new(),
    )
    .unwrap();
    assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
    assert!(out.warnings[1].contains("remote"));
}

fn annotations(pdf: &Pdf) -> Vec<(u32, lopdf::Dictionary)> {
    let mut out = Vec::new();
    for (no, page_id) in pdf.get_pages() {
        let page = pdf.get_dictionary(page_id).unwrap();
        let Ok(annots) = page.get(b"Annots") else { continue };
        let annots = match annots {
            Object::Array(a) => a.clone(),
            Object::Reference(r) => pdf.get_object(*r).unwrap().as_array().unwrap().clone(),
            _ => continue,
        };
        for a in annots {
            out.push((no, pdf.get_dictionary(a.as_reference().unwrap()).unwrap().clone()));
        }
    }
    out
}

// Codex review P2: a table row taller than a page continues on the next page.
#[test]
fn tall_table_rows_split_across_pages() {
    let cell: String = (0..120).map(|i| format!("line{i}<br>")).collect();
    let md = format!("| head A | head B |\n|---|---|\n| {cell}END | short |\n| after | row |\n");
    let (bytes, pages) = pdf_of(&md, PdfOptions::default());
    assert!(pages >= 3, "120 lines need several pages, got {pages}");
    if !has_fonts() {
        return;
    }
    let pdf = Pdf::load_mem(&bytes).unwrap();
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let all = squash(&text_of(&pdf));
    for needle in ["line0", "line60", "line119", "END", "after"] {
        assert!(all.contains(needle), "missing {needle}");
    }
    // every page shows the (repeated) header
    for no in pdf.get_pages().keys() {
        let t = squash(&pdf.extract_text(&[*no]).unwrap_or_default());
        assert!(t.contains("headA"), "page {no} without header: {t}");
    }
}

// Codex review P2: an image inside a link is drawn and clickable.
#[test]
fn linked_image_keeps_image_and_link() {
    let dir = tempfile::tempdir().unwrap();
    image::RgbaImage::from_pixel(40, 20, image::Rgba([200, 30, 30, 255]))
        .save(dir.path().join("logo.png"))
        .unwrap();
    let opts = PdfOptions {
        base_dir: Some(dir.path().to_path_buf()),
        ..PdfOptions::default()
    };
    let (bytes, _) = pdf_of(
        "Before\n\n[![logo](logo.png)](https://example.org/logo)\n\n![logo](logo.png)\n",
        opts,
    );
    let pdf = Pdf::load_mem(&bytes).unwrap();
    let images = pdf
        .objects
        .values()
        .filter(|o| match o {
            Object::Stream(st) => st.dict.get(b"Subtype").and_then(|v| v.as_name()).ok() == Some(b"Image".as_slice()),
            _ => false,
        })
        .count();
    assert!(images >= 1, "image drawn");
    let uris: Vec<String> = annotations(&pdf)
        .iter()
        .filter_map(|(_, d)| {
            let a = match d.get(b"A").ok()? {
                Object::Reference(r) => pdf.get_dictionary(*r).ok()?.clone(),
                Object::Dictionary(d) => d.clone(),
                _ => return None,
            };
            match a.get(b"URI").ok()? {
                Object::String(s, _) => Some(String::from_utf8_lossy(s).into_owned()),
                _ => None,
            }
        })
        .collect();
    assert_eq!(uris, ["https://example.org/logo"]);
}

// Codex re-review P1: a header taller than half a page is not repeated (no endless pages).
#[test]
fn tall_table_header_terminates() {
    let head: String = (0..80).map(|i| format!("h{i}<br>")).collect();
    let body: String = (0..60).map(|i| format!("| r{i} | x |\n")).collect();
    let md = format!("| {head}H | B |\n|---|---|\n{body}");
    let t = std::time::Instant::now();
    let (_, pages) = pdf_of(&md, PdfOptions::default());
    assert!(pages < 20, "{pages} pages");
    assert!(t.elapsed() < Duration::from_secs(60));
}

// Codex round 3: `**before ![a](a.png) mid ![b](b.png) after**` keeps its order in the layout.
#[test]
fn images_inside_formatting_keep_document_order() {
    use markise_pdf::layout::{Item, LayoutOptions, layout};
    let dir = tempfile::tempdir().unwrap();
    for n in ["a", "b"] {
        image::RgbaImage::from_pixel(30, 10, image::Rgba([0, 90, 200, 255]))
            .save(dir.path().join(format!("{n}.png")))
            .unwrap();
    }
    let doc = model("**before ![a](a.png) mid ![b](b.png) after**\n");
    let o = LayoutOptions {
        page_width: 600.0,
        page_height: f32::INFINITY,
        margins: [10.0; 4],
        unit: 1.0,
        theme: markise_theme::Theme::light(),
        base_dir: Some(dir.path().to_path_buf()),
        break_before_h1: false,
        header: None,
        footer: None,
        vars: Vars::new(),
    };
    let laid = layout(&mut engine(), &doc, &o, &JobControl::new()).unwrap();
    let mut order: Vec<(f32, String)> = Vec::new();
    for it in &laid.pages[0].items {
        match it {
            Item::Text(t) => {
                let y = t.glyphs.first().map(|g| g.y).unwrap_or(0.0);
                order.push((y, t.text.trim().to_string()));
            }
            Item::Image { rect, .. } => order.push((rect.y0, "[img]".into())),
            _ => {}
        }
    }
    order.sort_by(|a, b| a.0.total_cmp(&b.0));
    order.dedup_by(|a, b| a.1 == b.1);
    let seq: Vec<&str> = order.iter().map(|(_, s)| s.as_str()).collect();
    assert_eq!(seq, ["before", "[img]", "mid", "[img]", "after"], "{order:?}");
}
