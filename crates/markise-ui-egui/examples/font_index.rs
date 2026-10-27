//! Prints the system font index lookup for the given family names (debug aid).
//! `cargo run -p markise-ui-egui --release --example font_index -- Menlo Optima`
fn main() {
    let t = std::time::Instant::now();
    let idx = markise_ui_egui::fonts::SystemFonts::global();
    println!(
        "{} faces indexed in {:.0} ms",
        idx.faces.len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    for name in std::env::args().skip(1) {
        let faces = idx.family_faces(&name);
        println!(
            "{name}: {:?}",
            faces
                .iter()
                .map(|f| (&f.family, f.weight, f.italic, f.index))
                .collect::<Vec<_>>()
        );
    }
}
