//! Renders the sample documents to PDF and PNG for visual checks.
//!
//! `cargo run -p markise-pdf --release --example export_samples [-- <out-dir>]`
//! Default output: `<repo>/.export-samples/` (gitignored).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use markise_export::job::JobControl;
use markise_export::model::{ExDoc, ModelOptions};
use markise_export::vars::Vars;
use markise_pdf::samples::{CODE_SAMPLES, SAMPLE};
use markise_pdf::{PdfOptions, PngOptions, export_pdf, export_png, system_engine};
use markise_theme::Theme;

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.export-samples"));
    std::fs::create_dir_all(&out).expect("output folder");
    // a generated image for the sample's `![A lens](lens.png)`
    let img = image::RgbaImage::from_fn(320, 160, |x, y| {
        let d = ((x as f32 - 160.0).powi(2) + (y as f32 - 80.0).powi(2)).sqrt();
        let v = (255.0 * (1.0 - (d / 180.0).min(1.0))) as u8;
        image::Rgba([v, (v as u16 * 3 / 4) as u8, 40, 255])
    });
    img.save(out.join("lens.png")).expect("lens.png");
    let t0 = Instant::now();
    let mut eng = system_engine(Duration::from_secs(20));
    println!("fonts: {:?}", t0.elapsed());
    let control = JobControl::new();
    for (name, text) in [("sample", SAMPLE), ("code-samples", CODE_SAMPLES)] {
        std::fs::write(out.join(format!("{name}.md")), text).unwrap();
        let doc = ExDoc::build(
            &markise_md::parse(text),
            &ModelOptions {
                highlight: true,
                ..Default::default()
            },
        );
        for theme in [Theme::light(), Theme::native("markise-dark").unwrap()] {
            let suffix = if theme.dark { "-dark" } else { "" };
            let mut opts = PdfOptions {
                theme: theme.clone(),
                base_dir: Some(out.clone()),
                fallback_title: name.into(),
                ..PdfOptions::default()
            };
            opts.apply_front_matter(&doc);
            let vars = Vars::for_export(
                None,
                &out.join(format!("{name}.pdf")),
                None,
                &markise_export::vars::today(),
            )
            .with_front_matter(&doc.meta_yaml);
            let t = Instant::now();
            let r = export_pdf(&mut eng, &doc, &opts, &vars, &control).expect("pdf");
            let path = out.join(format!("{name}{suffix}.pdf"));
            std::fs::write(&path, &r.files[0]).unwrap();
            println!(
                "{} : {} pages, {} KB, {:?} {:?}",
                path.display(),
                r.pages,
                r.files[0].len() / 1024,
                t.elapsed(),
                r.warnings
            );
            let t = Instant::now();
            let png = export_png(
                &mut eng,
                &doc,
                &PngOptions {
                    theme,
                    base_dir: Some(out.clone()),
                    ..PngOptions::default()
                },
                &control,
            )
            .expect("png");
            for (i, f) in png.files.iter().enumerate() {
                let p = out.join(format!("{name}{suffix}-{i}.png"));
                std::fs::write(&p, f).unwrap();
                println!("{} : {} KB, {:?}", p.display(), f.len() / 1024, t.elapsed());
            }
        }
    }
}
