//! Renders TeX to PNG and SVG for visual checks (ADR 0008):
//! `cargo run -p markise-blocks --example math_render -- '<tex>' out` writes `out.png`/`out.svg`.
//! With `-` as TeX, each stdin line is one formula, stacked into one image.

use markise_blocks::math::{MathMode, layout, raster::rasterize, svg};

fn main() {
    let mut args = std::env::args().skip(1);
    let tex = args.next().expect("usage: math_render '<tex>' <out-stem>");
    let stem = args.next().unwrap_or_else(|| "math".into());
    let lines: Vec<String> = if tex == "-" {
        std::io::stdin()
            .lines()
            .map_while(Result::ok)
            .filter(|l| !l.trim().is_empty())
            .collect()
    } else {
        vec![tex]
    };
    let mut imgs = Vec::new();
    let mut svgs = String::from("<html><body style='font-size:20px'>\n");
    for l in &lines {
        match layout(l, MathMode::Display) {
            Ok(m) => {
                svgs.push_str(&format!("<p>{}</p>\n", svg::to_svg(&m, &svg::SvgOptions::default())));
                if let Some(img) = rasterize(&m, 40.0, [20, 20, 20, 255], None) {
                    imgs.push(img);
                }
            }
            Err(e) => eprintln!("error: {l}: {e}"),
        }
    }
    svgs.push_str("</body></html>\n");
    std::fs::write(format!("{stem}.html"), svgs).unwrap();
    let w = imgs.iter().map(|i| i.size[0]).max().unwrap_or(1);
    let h: u32 = imgs.iter().map(|i| i.size[1] + 8).sum::<u32>().max(1);
    let mut canvas = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
    let mut y0 = 0;
    for img in &imgs {
        for y in 0..img.size[1] {
            for x in 0..img.size[0] {
                let i = ((y * img.size[0] + x) * 4) as usize;
                let a = img.rgba[i + 3] as u32;
                let px = canvas.get_pixel_mut(x, y0 + y);
                for c in 0..3 {
                    px.0[c] = ((img.rgba[i + c] as u32 * a + px.0[c] as u32 * (255 - a)) / 255) as u8;
                }
            }
        }
        y0 += img.size[1] + 8;
    }
    canvas.save(format!("{stem}.png")).unwrap();
}
