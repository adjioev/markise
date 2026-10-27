//! Deterministic Markdown corpus generator (original content) for round-trip tests and
//! benchmarks. The same `(size, seed)` always yields the same text.

/// Small xorshift PRNG (no external dependency, deterministic across platforms).
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const WORDS: &[&str] = &[
    "lorem", "harbor", "signal", "copper", "meadow", "lantern", "orbit", "quiet", "river", "ember", "atlas", "velvet",
    "pixel", "cobalt", "summit", "thread", "willow", "canvas", "delta", "forge", "glacier", "hollow", "island",
    "juniper", "kernel", "lumen", "marble", "nectar", "onyx", "prism",
];

const CJK: &[&str] = &["日本語", "中文字", "한국어", "東京", "表格", "数学", "編集", "文書"];

/// Profile of generated content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// Mixed prose, lists, quotes, code, tables, math.
    Mixed,
    /// Mostly tables.
    Tables,
    /// Mostly math.
    Math,
    /// Mostly CJK prose.
    Cjk,
}

fn word(r: &mut Rng, cjk: bool) -> String {
    if cjk && r.chance(60) {
        r.pick(CJK).to_string()
    } else {
        r.pick(WORDS).to_string()
    }
}

fn inline_text(r: &mut Rng, words: usize, cjk: bool) -> String {
    let mut s = String::new();
    for i in 0..words {
        if i > 0 && !(cjk && r.chance(30)) {
            s.push(' ');
        }
        let w = word(r, cjk);
        match r.below(40) {
            0 => s.push_str(&format!("**{w}**")),
            1 => s.push_str(&format!("_{w}_")),
            2 => s.push_str(&format!("`{w}`")),
            3 => s.push_str(&format!("[{w}](https://example.com/{w})")),
            4 => s.push_str(&format!("~~{w}~~")),
            5 => s.push_str(&format!("${w}^2$")),
            6 => s.push_str(&format!("<u>{w}</u>")),
            7 => s.push_str(&format!("{w}[^n{}]", r.below(5))),
            8 => s.push_str(&format!("__{w}__")),
            9 => s.push_str(&format!("*{w}*")),
            10 => s.push_str(&format!("\\*{w}")),
            11 => s.push_str(&format!("{w}&amp;")),
            12 => s.push_str(&format!("![{w}](img/{w}.png)")),
            _ => s.push_str(&w),
        }
    }
    s
}

fn paragraph(r: &mut Rng, out: &mut String, cjk: bool) {
    let lines = 1 + r.below(4);
    for l in 0..lines {
        if l > 0 {
            out.push('\n');
        }
        out.push_str(&{
            let n = 4 + r.below(10);
            inline_text(r, n, cjk)
        });
        if r.chance(5) {
            out.push_str("  ");
        }
    }
    out.push('\n');
}

fn list(r: &mut Rng, out: &mut String, depth: usize, cjk: bool) {
    let ordered = r.chance(35);
    let bullet = *r.pick(&["-", "*", "+"]);
    let fixed = r.chance(20);
    let start = 1 + r.below(3);
    let loose = r.chance(25);
    let indent = "  ".repeat(depth);
    let n = 2 + r.below(4);
    for i in 0..n {
        let marker = if ordered {
            format!("{}.", if fixed { start } else { start + i })
        } else {
            bullet.to_string()
        };
        let task = if !ordered && r.chance(20) {
            if r.chance(50) { "[x] " } else { "[ ] " }
        } else {
            ""
        };
        out.push_str(&format!("{indent}{marker} {task}{}\n", {
            let n = 3 + r.below(8);
            inline_text(r, n, cjk)
        }));
        if depth < 3 && r.chance(15) {
            let sub = " ".repeat(marker.len() + 1);
            let mut nested = String::new();
            list(r, &mut nested, 0, cjk);
            for line in nested.lines() {
                if line.is_empty() {
                    out.push('\n');
                } else {
                    out.push_str(&format!("{indent}{sub}{line}\n"));
                }
            }
        }
        if loose && i + 1 < n {
            out.push('\n');
        }
    }
}

fn quote(r: &mut Rng, out: &mut String, cjk: bool) {
    let kind = if r.chance(20) {
        Some(*r.pick(&["NOTE", "TIP", "WARNING", "caution"]))
    } else {
        None
    };
    if let Some(k) = kind {
        out.push_str(&format!("> [!{k}]\n"));
    }
    let lines = 1 + r.below(4);
    for _ in 0..lines {
        if r.chance(10) {
            out.push_str(">\n");
        }
        let nested = if r.chance(15) { "> " } else { "" };
        out.push_str(&format!("> {nested}{}\n", {
            let n = 4 + r.below(8);
            inline_text(r, n, cjk)
        }));
    }
}

fn code(r: &mut Rng, out: &mut String) {
    let lang = *r.pick(&["rust", "python", "js", "", "sh", "mermaid", "math"]);
    let fence = if r.chance(15) { "~~~" } else { "```" };
    out.push_str(&format!("{fence}{lang}\n"));
    let n = 1 + r.below(12);
    for i in 0..n {
        let ind = "    ".repeat(r.below(3));
        out.push_str(&format!(
            "{ind}let v{i} = call_{}({i}); // {}\n",
            r.pick(WORDS),
            r.pick(WORDS)
        ));
    }
    out.push_str(fence);
    out.push('\n');
}

fn table(r: &mut Rng, out: &mut String, cjk: bool) {
    let cols = 2 + r.below(4);
    let rows = 2 + r.below(8);
    let pretty = r.chance(50);
    let cell = |r: &mut Rng| {
        let mut c = {
            let n = 1 + r.below(3);
            inline_text(r, n, cjk)
        };
        if r.chance(5) {
            c.push_str(" a\\|b");
        }
        c
    };
    let mut hdr = String::from("|");
    let mut delim = String::from("|");
    for _ in 0..cols {
        hdr.push_str(&format!(" {} |", cell(r)));
        delim.push_str(match r.below(4) {
            0 => " :--- |",
            1 => " ---: |",
            2 => ":---:|",
            _ => "---|",
        });
    }
    out.push_str(&hdr);
    out.push('\n');
    out.push_str(&delim);
    out.push('\n');
    for _ in 0..rows {
        let mut row = String::from(if pretty { "| " } else { "|" });
        for _ in 0..cols {
            row.push_str(&cell(r));
            row.push_str(if pretty { " | " } else { "|" });
        }
        out.push_str(row.trim_end());
        out.push('\n');
    }
}

fn math(r: &mut Rng, out: &mut String) {
    if r.chance(20) {
        out.push_str("\\[\n");
        out.push_str(&format!("\\int_0^{} f(x)\\,dx\n", r.below(9)));
        out.push_str("\\]\n");
    } else {
        out.push_str(if r.chance(20) { "$$ {eq}\n" } else { "$$\n" });
        for _ in 0..1 + r.below(3) {
            out.push_str(&format!("\\sum_{{i=0}}^{{{}}} a_i x^i \\\\\n", r.below(20)));
        }
        out.push_str("$$\n");
    }
}

/// Generates roughly `target_bytes` of Markdown. `crlf` converts line endings.
pub fn generate_profile(target_bytes: usize, seed: u64, profile: Profile, crlf: bool) -> String {
    let mut r = Rng::new(seed);
    let mut out = String::with_capacity(target_bytes + 4096);
    let cjk_doc = profile == Profile::Cjk;
    if r.chance(70) {
        out.push_str("---\ntitle: Generated corpus\ntags: [bench, markise]\n---\n\n");
    }
    let mut section = 0;
    let mut footnotes = 0;
    while out.len() < target_bytes {
        let cjk = cjk_doc || r.chance(10);
        let choice = match profile {
            Profile::Tables if r.chance(60) => 5,
            Profile::Math if r.chance(60) => 6,
            _ => r.below(20),
        };
        match choice {
            0 => {
                section += 1;
                let level = 1 + r.below(4);
                if r.chance(15) && level <= 2 {
                    let t = inline_text(&mut r, 3, cjk);
                    let ch = if level == 1 { "=" } else { "-" };
                    out.push_str(&format!("{t}\n{}\n", ch.repeat(3 + r.below(5))));
                } else {
                    out.push_str(&format!(
                        "{} Section {section} {}\n",
                        "#".repeat(level),
                        word(&mut r, cjk)
                    ));
                }
            }
            1 => list(&mut r, &mut out, 0, cjk),
            2 => quote(&mut r, &mut out, cjk),
            3 => code(&mut r, &mut out),
            4 => {
                let hr: &&str = r.pick(&["---\n", "***\n", "- - -\n", "______\n"]);
                out.push_str(hr)
            }
            5 => table(&mut r, &mut out, cjk),
            6 => math(&mut r, &mut out),
            7 => {
                footnotes += 1;
                out.push_str(&format!("[^n{}]: {}\n", footnotes % 5, inline_text(&mut r, 6, cjk)));
            }
            8 => out.push_str(&format!(
                "[ref{}]: https://example.com/{}  \"Title\"\n",
                r.below(50),
                r.pick(WORDS)
            )),
            9 if r.chance(10) => out.push_str("[TOC]\n"),
            10 if r.chance(30) => out.push_str("<div class=\"note\">\n<b>html</b> block\n</div>\n"),
            _ => paragraph(&mut r, &mut out, cjk),
        }
        // blank lines between blocks: usually one, sometimes 0 or 2-3
        let blanks = match r.below(20) {
            0 => 0,
            1 => 2,
            2 => 3,
            _ => 1,
        };
        for _ in 0..blanks {
            out.push('\n');
        }
    }
    if crlf {
        out = out.replace('\n', "\r\n");
    }
    out
}

/// Mixed-profile corpus with LF endings.
pub fn generate(target_bytes: usize, seed: u64) -> String {
    generate_profile(target_bytes, seed, Profile::Mixed, false)
}

/// Backtick runs of strictly increasing length separated by `sep`, about `bytes` long.
pub fn distinct_ticks(bytes: usize, sep: &str) -> String {
    let mut s = String::with_capacity(bytes + 64);
    let mut i = 1;
    while s.len() < bytes {
        s.push_str(&"`".repeat(i));
        s.push_str(sep);
        i += 1;
    }
    s
}

/// Generator of an adversarial input from a repetition count.
pub type AdversarialGen = fn(usize) -> String;

/// Pathological inputs (unclosed delimiters, deep nesting) that must parse in linear time.
pub fn adversarial() -> Vec<(&'static str, AdversarialGen)> {
    vec![
        ("unclosed-latex-blocks", |n| "\\[\n".repeat(n)),
        ("unclosed-latex-in-quote", |n| "> \\[\n".repeat(n)),
        ("unclosed-dollars-in-list", |n| {
            format!("- a\n{}", "  $$ {x}\n  a\n\n".repeat(n))
        }),
        ("inline-dollars", |n| "$a ".repeat(n)),
        ("inline-double-dollars", |n| "$$a ".repeat(n)),
        ("inline-latex-paren", |n| "\\(a ".repeat(n)),
        ("backtick-runs", |n| "`a ``b ```c ".repeat(n)),
        ("html-comments", |n| "<!-- a ".repeat(n)),
        ("html-attr-quotes", |n| "<a href=\"x ".repeat(n)),
        ("footnote-brackets", |n| "[^a ".repeat(n)),
        ("link-dest-no-space", |n| "[a](b".repeat(n)),
        ("link-titles", |n| "[a](b \"c ".repeat(n)),
        ("nested-brackets", |n| format!("{}{}", "[".repeat(n), "]".repeat(n))),
        ("bare-url-parens", |n| format!("www.a.com/{}", ")".repeat(n))),
        ("emphasis-mix", |n| "*a _b **c __d ".repeat(n)),
        ("table-row", |n| {
            format!("| a | b |\n|---|---|\n|{}|\n", " `a $b".repeat(n))
        }),
        ("dest-balanced-parens", |n| "[a](b()".repeat(n)),
        ("angle-then-raw-dests", |n| "[x](<y[a](b)".repeat(n)),
        ("distinct-backtick-runs", |n| distinct_ticks(n * 6, " ")),
        ("table-distinct-backticks", |n| {
            format!("| a |\n|---|\n| {} |\n", distinct_ticks(n * 6, " "))
        }),
    ]
}

/// Asserts that `run` scales (near) linearly on inputs built by `generate`.
///
/// The small input size starts at 4 000 repetitions and doubles until the best-of-5 time is at
/// least [`MIN_SMALL`] (so a fast quadratic step cannot hide below the timer floor; capped at
/// 64 000 repetitions), then the large input is 16× that size. Linear ≈ 16×, n^1.5 ≈ 64×,
/// quadratic ≈ 256×: the ratio must stay below 40×. A busy machine can disturb one measurement,
/// so up to three attempts are made; a super-linear algorithm fails every attempt. A large
/// run slower than the wall-clock cap fails at once.
pub fn assert_scales_linearly(name: &str, generate: impl Fn(usize) -> String, run: &dyn Fn(&str)) {
    use std::time::{Duration, Instant};
    // Debug builds on shared CI runners are slow; release gets the tighter cap.
    let cap = Duration::from_secs(if cfg!(debug_assertions) { 15 } else { 5 });
    let once = |s: &str| {
        let t = Instant::now();
        run(s);
        t.elapsed()
    };
    let best = |s: &str| {
        let first = once(s);
        assert!(
            first < cap,
            "{name}: one run of {} bytes took {first:?} (cap {cap:?})",
            s.len()
        );
        (1..5).map(|_| once(s)).fold(first, Duration::min)
    };
    let mut last = String::new();
    for _ in 0..3 {
        let mut n = 4_000;
        let ts = loop {
            let t = best(&generate(n));
            if t >= MIN_SMALL || n >= 64_000 {
                break t;
            }
            n *= 2;
        };
        let tl = best(&generate(16 * n));
        let ratio = tl.as_secs_f64() / ts.max(Duration::from_millis(1)).as_secs_f64();
        if ratio < 40.0 {
            return;
        }
        last = format!("{name}: 16× input (n={n}) took {ratio:.1}× longer ({ts:?} → {tl:?})");
    }
    panic!("{last}");
}

/// Minimum best-of-5 time of the small input in [`assert_scales_linearly`].
pub const MIN_SMALL: std::time::Duration = std::time::Duration::from_millis(5);

#[cfg(test)]
mod tests {
    #[test]
    fn deterministic() {
        assert_eq!(super::generate(5000, 7), super::generate(5000, 7));
        assert!(super::generate(5000, 7).len() >= 5000);
    }
}
