//! Export throughput: layout + PDF write of the sample documents, and the PNG raster.
//! (Font discovery happens once, outside the measurement.)

use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use markise_export::job::JobControl;
use markise_export::model::{ExDoc, ModelOptions};
use markise_export::vars::Vars;
use markise_pdf::samples::{CODE_SAMPLES, SAMPLE};
use markise_pdf::{PdfOptions, PngOptions, export_pdf, export_png, system_engine};

fn bench(c: &mut Criterion) {
    let mut eng = system_engine(Duration::from_secs(30));
    let sample = ExDoc::build(&markise_md::parse(SAMPLE), &ModelOptions { highlight: true });
    let code = ExDoc::build(&markise_md::parse(CODE_SAMPLES), &ModelOptions { highlight: true });
    let big: String = std::iter::repeat_n(SAMPLE.split_once("\n---\n").map(|x| x.1).unwrap_or(SAMPLE), 10).collect();
    let big = ExDoc::build(&markise_md::parse(&big), &ModelOptions { highlight: true });
    let control = JobControl::new();
    let mut g = c.benchmark_group("export");
    g.sample_size(10);
    g.bench_function("pdf_sample", |b| {
        b.iter(|| export_pdf(&mut eng, &sample, &PdfOptions::default(), &Vars::new(), &control).unwrap())
    });
    g.bench_function("pdf_code_samples", |b| {
        b.iter(|| export_pdf(&mut eng, &code, &PdfOptions::default(), &Vars::new(), &control).unwrap())
    });
    g.bench_function("pdf_sample_x10", |b| {
        b.iter(|| export_pdf(&mut eng, &big, &PdfOptions::default(), &Vars::new(), &control).unwrap())
    });
    g.bench_function("png_sample", |b| {
        b.iter(|| export_png(&mut eng, &sample, &PngOptions::default(), &control).unwrap())
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
