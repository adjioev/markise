//! Prints the export model of a Markdown file (debugging aid).
fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_model <file.md>");
    let t = std::fs::read_to_string(path).expect("readable file");
    let d = markise_export::model::ExDoc::build(&markise_md::parse(&t), &Default::default());
    println!("{:#?}", d.blocks);
}
