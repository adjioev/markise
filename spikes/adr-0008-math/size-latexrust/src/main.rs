fn main() {
    let s = std::env::args().nth(1).unwrap_or_default();
    let ast = latex_rust::parse(&s).unwrap();
    let font = latex_rust::MathFont::stix_two_math().unwrap();
    let b = latex_rust::layout(&ast, &font, latex_rust::MathStyle::Display).unwrap();
    println!("{:?}", b.width);
}
