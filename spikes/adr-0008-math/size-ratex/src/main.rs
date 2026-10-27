fn main() {
    let s = std::env::args().nth(1).unwrap_or_default();
    let ast = ratex_parser::parse(&s).unwrap();
    let b = ratex_layout::layout(&ast, &ratex_layout::LayoutOptions::default());
    let dl = ratex_layout::to_display_list(&b);
    println!("{} {}", dl.items.len(), dl.width);
}
