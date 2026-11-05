use markise_web_render::proto::{Area, FailKind};

use crate::support::{Answer, Helper, pixel};

const FIXTURE: &str = r#"<section data-block="fixture"><h2 data-field="title">A title</h2><p>Text</p></section>"#;
const CSS: &str = "[data-block]{background:rgb(10,200,30);height:100px}h2{margin:0;color:rgb(10,200,30)}";
const JS: &str = "export default function init(root){const d=document.createElement('div');d.style.cssText='height:50px;background:rgb(200,10,30)';root.after(d);d.dataset.done=root.dataset.block;return new Promise((r)=>setTimeout(r,5));}";

fn image(a: Answer) -> (markise_web_render::proto::Rendered, Vec<u8>) {
    match a {
        Answer::Image(r, b) => (r, b),
        Answer::Failed(k, why) => panic!("{k:?}: {why}"),
    }
}

#[test]
fn a_block_draws_with_its_styles_and_its_module() {
    let mut h = Helper::start();
    h.file(Area::Package, "styles/a.css", CSS);
    h.file(Area::Package, "components/x/x.js", JS);
    let r = h.request(FIXTURE, Some("components/x/x.js"), &["styles/a.css"]);
    let (m, b) = image(h.render(r));
    assert_eq!(m.px_width, 400);
    assert_eq!(m.css_height, 150.0, "the block and what its module added");
    assert_eq!(m.px_height, 150);
    assert_eq!(pixel(&m, &b, 200, 50), [10, 200, 30, 255], "the style");
    assert_eq!(pixel(&m, &b, 200, 125), [200, 10, 30, 255], "the module");
    assert!(h.asked.contains(&(Area::Package, "components/x/x.js".to_string())));
}

#[test]
fn the_scale_is_what_was_asked_for_and_outside_is_transparent() {
    let mut h = Helper::start();
    let mut r = h.request(
        r#"<div style="width:100px;height:10px;background:rgb(0,0,255)"></div>"#,
        None,
        &[],
    );
    r.scale = 3.0;
    let (m, b) = image(h.render(r));
    assert_eq!((m.px_width, m.px_height), (1200, 30));
    assert_eq!(pixel(&m, &b, 10, 10), [0, 0, 255, 255]);
    assert_eq!(pixel(&m, &b, 1000, 10)[3], 0, "nothing paints beyond it");
}

#[test]
fn the_theme_and_the_language_reach_the_page_as_data() {
    let mut h = Helper::start();
    h.file(
        Area::Package,
        "t.css",
        ":root:lang(ru)[data-theme=dark] .x{background:var(--markise-text);height:10px}",
    );
    let mut r = h.request(r#"<div class="x"></div>"#, None, &["t.css"]);
    r.dark = true;
    r.lang = "ru".to_string();
    let (m, b) = image(h.render(r));
    assert_eq!(pixel(&m, &b, 5, 5), [0x11, 0x22, 0x33, 255]);
}

#[test]
fn document_text_is_data_and_runs_nothing() {
    let mut h = Helper::start();
    // an inline handler and an inline script in the markup, and a quote to break out of a script
    let html = r#"<div id="v" style="height:10px;background:rgb(0,255,0)"></div>
<img src="data:," onerror="document.getElementById('v').style.background='rgb(255,0,0)'">
<script>document.getElementById('v').style.background='rgb(255,0,0)'</script>"`;'}"#;
    let r = h.request(html, None, &[]);
    let (m, b) = image(h.render(r));
    assert_eq!(pixel(&m, &b, 5, 5), [0, 255, 0, 255]);
}

#[test]
fn a_document_image_is_asked_for_by_its_relative_path() {
    let mut h = Helper::start();
    // one blue pixel as a PNG
    h.file(Area::Document, "images/a b.png", one_pixel_png());
    let html = r#"<img src="images/a%20b.png" style="display:block;width:20px;height:20px">"#;
    let r = h.request(html, None, &[]);
    let (m, b) = image(h.render(r));
    assert_eq!(pixel(&m, &b, 5, 5), [0, 0, 255, 255]);
    assert_eq!(h.asked, [(Area::Document, "images/a b.png".to_string())]);
}

/// A hidden page's timers would otherwise wait a second or more: a block with a short wait in its
/// code would use up its render, and on a slow machine more than that.
#[test]
fn a_page_nobody_sees_runs_its_timers_on_time() {
    let mut h = Helper::start();
    h.file(
        Area::Package,
        "m.js",
        "export default async function(root){ const t=performance.now(); for(let i=0;i<8;i++){ await new Promise((r)=>setTimeout(r,25)); } root.style.height=Math.ceil(performance.now()-t)+'px'; }",
    );
    // the page has been hidden for a while by the second render
    for _ in 0..2 {
        let r = h.request("<section></section>", Some("m.js"), &[]);
        let (m, _) = image(h.render(r));
        assert!(m.css_height >= 190.0, "they did wait: {}", m.css_height);
        assert!(m.css_height < 900.0, "8 waits of 25 ms took {} ms", m.css_height);
    }
}

fn failed(a: Answer) -> (FailKind, String) {
    match a {
        Answer::Image(..) => panic!("an image"),
        Answer::Failed(k, why) => (k, why),
    }
}

#[test]
fn a_picture_that_is_not_there_is_no_picture_of_the_block() {
    let mut h = Helper::start();
    let r = h.request(
        r#"<div style="height:10px"><img src="images/none.png"></div>"#,
        None,
        &[],
    );
    assert_eq!(failed(h.render(r)).0, FailKind::Images);
    assert!(h.asked.contains(&(Area::Document, "images/none.png".to_string())));
}

#[test]
fn nor_is_one_the_blocks_code_adds() {
    let mut h = Helper::start();
    h.file(
        Area::Package,
        "m.js",
        "export default function(root){ const i=new Image(); i.src='images/none.png'; root.append(i); }",
    );
    let r = h.request("<section></section>", Some("m.js"), &[]);
    assert_eq!(failed(h.render(r)).0, FailKind::Images);
}

#[test]
fn nor_is_one_that_never_comes() {
    let mut h = Helper::start();
    h.silent.push("images/slow.png".to_string());
    let r = h.request(
        r#"<div style="height:10px"><img src="images/slow.png"></div>"#,
        None,
        &[],
    );
    assert_eq!(failed(h.render(r)).0, FailKind::Images);
}

#[test]
fn a_block_hiding_a_picture_it_loaded_still_draws() {
    let mut h = Helper::start();
    h.file(Area::Document, "images/p.png", one_pixel_png());
    let r = h.request(
        r#"<div style="height:10px;background:rgb(0,255,0)"><img src="images/p.png" style="display:none"></div>"#,
        None,
        &[],
    );
    let (m, b) = image(h.render(r));
    assert_eq!(pixel(&m, &b, 5, 5), [0, 255, 0, 255]);
}

/// A row across the middle of a large picture: what the snapshot shows of it. Empty, the frame's
/// background has one colour there.
fn colours_across_the_middle(m: &markise_web_render::proto::Rendered, b: &[u8]) -> usize {
    let row = m.px_height / 2;
    let colours: std::collections::HashSet<[u8; 4]> = (m.px_width / 8..m.px_width * 7 / 8)
        .step_by(3)
        .map(|x| pixel(m, b, x, row))
        .collect();
    colours.len()
}

/// A fresh helper's first block, with a picture large enough for the engine to decode it apart.
fn cold_open(png: &[u8], html: &str, module: Option<&str>) -> usize {
    let mut h = Helper::start();
    h.file(Area::Document, "images/large.png", png.to_vec());
    h.file(
        Area::Package,
        "m.js",
        "export default function(root){ const i=new Image(); i.src='images/large.png'; root.append(i); return new Promise((d)=>{i.onload=d;i.onerror=d;}); }",
    );
    h.file(
        Area::Package,
        "s.css",
        "section{background:rgb(30,40,60);padding:20px}img{display:block;width:100%;background:rgb(40,50,70);border-radius:8px}",
    );
    let mut r = h.request(html, module, &["s.css"]);
    r.width = 780;
    r.scale = 2.0;
    let (m, b) = image(h.render(r));
    colours_across_the_middle(&m, &b)
}

const LARGE: &str = r#"<section><img src="images/large.png"></section>"#;

#[test]
fn the_first_block_of_a_fresh_helper_has_its_large_picture() {
    let png = large_png();
    assert!(cold_open(&png, LARGE, None) >= 8, "the frame is empty");
    assert!(
        cold_open(&png, "<section></section>", Some("m.js")) >= 8,
        "the frame is empty"
    );
}

#[test]
fn nor_does_a_css_background_leave_a_hole() {
    let mut h = Helper::start();
    h.file(Area::Document, "images/large.png", large_png());
    h.file(
        Area::Package,
        "s.css",
        "section{height:555px;background:rgb(40,50,70) url(markise-block://document/images/large.png) 0 0/100% 100% no-repeat}",
    );
    let mut r = h.request("<section></section>", None, &["s.css"]);
    r.width = 780;
    r.scale = 2.0;
    let (m, b) = image(h.render(r));
    assert!(colours_across_the_middle(&m, &b) >= 8, "the frame is empty");
}

/// About 2% of these came out empty, and 5 to 10% on a busy machine, before a picture was asked to
/// be decoded for its paint. `MARKISE_COLD_OPENS` for a longer run.
#[test]
#[ignore = "heavy: starts a helper 300 times, a few minutes"]
fn heavy_suite_no_cold_open_draws_an_empty_frame() {
    let opens = std::env::var("MARKISE_COLD_OPENS").map_or(300, |n| n.parse().expect("a number"));
    let png = large_png();
    let empty = (0..opens).filter(|_| cold_open(&png, LARGE, None) < 8).count();
    assert_eq!(empty, 0, "{empty} of {opens} cold opens drew an empty frame");
}

/// A PNG the size of a phone's photo scaled down, with no two rows alike: never one colour anywhere.
fn large_png() -> Vec<u8> {
    let (w, h) = (1448u32, 1086u32);
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    let mut noise = 12345u32;
    for y in 0..h {
        for x in 0..w {
            noise = noise.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = (noise >> 16) & 15;
            pixels.extend([
                (x * 240 / w + n) as u8,
                (y * 240 / h + n) as u8,
                ((x ^ y) % 240 + n) as u8,
            ]);
        }
    }
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&pixels).unwrap();
    out
}

/// A PNG of one opaque blue pixel.
fn one_pixel_png() -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, 1, 1);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&[0, 0, 255]).unwrap();
    out
}
