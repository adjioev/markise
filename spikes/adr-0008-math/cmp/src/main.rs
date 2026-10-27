//! Coverage and timing: RaTeX (parse + layout + display list) vs latex-rust (parse + layout).
use std::time::Instant;

fn ratex(s: &str) -> Result<usize, String> {
    let ast = ratex_parser::parse(s).map_err(|e| e.to_string())?;
    let b = ratex_layout::layout(&ast, &ratex_layout::LayoutOptions::default());
    ratex_layout::validate_layout(&b)?;
    Ok(ratex_layout::to_display_list(&b).items.len())
}

fn latexrust(s: &str, font: &latex_rust::MathFont) -> Result<(), String> {
    let ast = latex_rust::parse(s).map_err(|e| format!("{e:?}"))?;
    latex_rust::layout(&ast, font, latex_rust::MathStyle::Display).map_err(|e| format!("{e:?}"))?;
    Ok(())
}

fn median_us(mut f: impl FnMut()) -> f64 {
    let mut v = Vec::new();
    for _ in 0..7 {
        let t = Instant::now();
        for _ in 0..50 {
            f();
        }
        v.push(t.elapsed().as_secs_f64() * 1e6 / 50.0);
    }
    v.sort_by(f64::total_cmp);
    v[3]
}

fn main() {
    let corpus = include_str!("corpus.txt");
    let t = Instant::now();
    let font = latex_rust::MathFont::stix_two_math().unwrap();
    println!("latex-rust font load: {:.2} ms", t.elapsed().as_secs_f64() * 1e3);
    let (mut ok_r, mut ok_l, mut n) = (0, 0, 0);
    let (mut tr, mut tl) = (0.0, 0.0);
    for line in corpus.lines().filter(|l| !l.trim().is_empty()) {
        n += 1;
        let r = ratex(line);
        let l = latexrust(line, &font);
        let rt = median_us(|| {
            let _ = ratex(line);
        });
        let lt = median_us(|| {
            let _ = latexrust(line, &font);
        });
        tr += rt;
        tl += lt;
        ok_r += r.is_ok() as usize;
        ok_l += l.is_ok() as usize;
        let short = |e: &String| e.chars().take(70).collect::<String>();
        println!(
            "{:>7.1}us {:<4} | {:>7.1}us {:<4} | {}{}{}",
            rt,
            if r.is_ok() { "ok" } else { "ERR" },
            lt,
            if l.is_ok() { "ok" } else { "ERR" },
            line.chars().take(60).collect::<String>(),
            r.as_ref().err().map(|e| format!("\n            ratex: {}", short(e))).unwrap_or_default(),
            l.as_ref().err().map(|e| format!("\n            latex-rust: {}", short(e))).unwrap_or_default(),
        );
    }
    println!("RaTeX ok {ok_r}/{n}, total {tr:.0} us; latex-rust ok {ok_l}/{n}, total {tl:.0} us");
}
