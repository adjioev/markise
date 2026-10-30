//! `markise-cli` — command line front end.
//!
//! ```text
//! markise-cli roundtrip <file>          parse + serialize; exit 1 and print a diff on change
//! markise-cli html <file> [-o <out>] [--max-size <size>]   export HTML (spec 03 DOM contract) to stdout or <out>
//! markise-cli theme-import <theme.css> [--json]   import a CSS theme (spec 03 §2), print summary + report
//! ```

mod theme;

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use markise_export::{CodeHighlight, HtmlOptions, LARGE_HTML_BYTES, estimate_html_size, human_bytes, render_document};
use markise_md::{Document, ParseOptions};

const USAGE: &str = "usage:
  markise-cli roundtrip <file>
  markise-cli html <file> [-o <out.html>] [--trusted] [--no-highlight | --inline-colors] [--no-diagrams]
                    [--max-size <size>]
  markise-cli theme-import <theme.css> [--json]

  --trusted         keep raw HTML and all URL schemes (default: sanitized, safe schemes only)
  --no-highlight    plain code blocks (default: token spans with cm-* classes)
  --inline-colors   token spans also carry inline colors (default Markise light scheme)
  --no-diagrams     keep mermaid/sequence/flow fences as code (default: rendered as inline SVG)
  --max-size <size> refuse (exit 1, nothing written) when the estimated HTML is larger than
                    <size>: bytes, or with a K/M/G suffix (decimal), e.g. 500M. Without it an
                    estimate above 200M only prints a warning to stderr (every [TOC] lists
                    all headings, so many [TOC] blocks make very large files)";

fn read_markdown(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    String::from_utf8(bytes)
        .map_err(|e| format!("{path}: not valid UTF-8 ({e}); other encodings are not supported yet"))
}

fn roundtrip(path: &str) -> Result<ExitCode, String> {
    let original = read_markdown(path)?;
    let start = std::time::Instant::now();
    let doc = Document::parse(original.as_str(), ParseOptions::default());
    let parsed = start.elapsed();
    let out = doc.serialize();
    if out == original {
        println!(
            "OK {path}: {} bytes, {} top-level blocks, line ending {:?}, final newline: {}, parse {:.2?}",
            original.len(),
            doc.blocks().len(),
            doc.line_ending(),
            doc.has_final_newline(),
            parsed
        );
        return Ok(ExitCode::SUCCESS);
    }
    let diff = similar::TextDiff::from_lines(&original, &out);
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "MISMATCH {path}: serialized output differs from input");
    for op in diff.ops() {
        for change in diff.iter_changes(op) {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => "-",
                similar::ChangeTag::Insert => "+",
                similar::ChangeTag::Equal => continue,
            };
            // escape so that line-ending and whitespace differences are visible
            let _ = writeln!(stdout, "{sign}{}", change.value().escape_debug());
        }
    }
    Ok(ExitCode::from(1))
}

/// `--max-size` value: bytes, or a decimal K/M/G suffix ("500M").
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mul) = match s.char_indices().last()? {
        (i, 'k' | 'K') => (&s[..i], 1_000),
        (i, 'm' | 'M') => (&s[..i], 1_000_000),
        (i, 'g' | 'G') => (&s[..i], 1_000_000_000),
        _ => (s, 1),
    };
    num.trim().parse::<u64>().ok()?.checked_mul(mul)
}

fn html(
    path: &str,
    out: Option<&str>,
    trusted: bool,
    highlight: CodeHighlight,
    diagrams: bool,
    max_size: Option<u64>,
) -> Result<ExitCode, String> {
    let text = read_markdown(path)?;
    let doc = Document::parse(
        text,
        ParseOptions {
            diagrams,
            ..ParseOptions::default()
        },
    );
    let opts = HtmlOptions {
        fallback_title: Path::new(path).file_stem().and_then(|s| s.to_str()).map(str::to_string),
        trusted,
        highlight,
        ..Default::default()
    };
    // issue #29: every [TOC] lists all headings; warn (or refuse) before writing gigabytes
    let est = estimate_html_size(&doc, &opts);
    let why = match est.tocs {
        0 => format!("about {}", human_bytes(est.bytes)),
        1 => format!(
            "about {} (the document has 1 table of contents)",
            human_bytes(est.bytes)
        ),
        n => format!(
            "about {} because the document has {n} tables of contents",
            human_bytes(est.bytes)
        ),
    };
    match max_size {
        Some(max) if est.bytes > max => {
            eprintln!(
                "markise-cli: refusing to export {path}: the HTML would be {why} (--max-size {})",
                human_bytes(max)
            );
            return Ok(ExitCode::from(1));
        }
        _ if est.bytes > LARGE_HTML_BYTES => {
            eprintln!("markise-cli: warning: the HTML export of {path} will be {why}");
        }
        _ => {}
    }
    let html = render_document(&doc, &opts);
    match out {
        Some(o) => std::fs::write(o, html).map_err(|e| format!("{o}: {e}"))?,
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(html.as_bytes()).map_err(|e| e.to_string())?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    match args {
        [cmd, file] if cmd == "roundtrip" => roundtrip(file),
        [cmd, rest @ ..] if cmd == "html" => {
            let mut file = None;
            let mut out = None;
            let mut trusted = false;
            let mut highlight = CodeHighlight::Classes;
            let mut diagrams = true;
            let mut max_size = None;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "-o" | "--output" => out = Some(it.next().ok_or(USAGE)?.as_str()),
                    "--trusted" => trusted = true,
                    "--no-diagrams" => diagrams = false,
                    "--max-size" => {
                        max_size = Some(parse_size(it.next().ok_or(USAGE)?).ok_or(USAGE)?);
                    }
                    "--no-highlight" => highlight = CodeHighlight::Off,
                    "--inline-colors" => highlight = CodeHighlight::inline_from(&markise_theme::Theme::light()),
                    f if file.is_none() && !f.starts_with('-') => file = Some(f),
                    _ => return Err(USAGE.to_string()),
                }
            }
            html(file.ok_or(USAGE)?, out, trusted, highlight, diagrams, max_size)
        }
        [cmd, rest @ ..] if cmd == "theme-import" => {
            let mut file = None;
            let mut json = false;
            for a in rest {
                match a.as_str() {
                    "--json" => json = true,
                    f if file.is_none() && !f.starts_with('-') => file = Some(f),
                    _ => return Err(USAGE.to_string()),
                }
            }
            theme::import(file.ok_or(USAGE)?, json)
        }
        [flag] if flag == "-h" || flag == "--help" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        _ => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}
