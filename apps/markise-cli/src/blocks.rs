//! `markise-cli blocks`: check a document's blocks and print a package's reference, for a coding
//! agent or a person who writes blocks without opening the app. The rules are the app's: the engine
//! and the package crate hold them, and nothing here asks a question or writes to the store.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use markise_engine::block_check::{BlockChecker, DocumentCheck};
use markise_engine::block_page::is_language_tag;
use markise_package::{
    BINDING_FILE, Binding, PackageIndex, ReferenceHeader, Sha256Hex, find_binding, pack_folder, read_archive,
    read_bound_package,
};
use serde_json::{Value, json};

/// The name of the shape of `blocks check --json`, which changes only with that name.
const CHECK_FORMAT: &str = "markise-blocks-check/1";

/// One `blocks.config.json` as far as it was read: a package once, however many files use it.
type Bound = Result<(Binding, BlockChecker), String>;

pub fn run(args: &[String]) -> Result<ExitCode, String> {
    match args {
        [cmd, rest @ ..] if cmd == "check" && !wants_help(rest) => check(rest),
        [cmd, rest @ ..] if cmd == "docs" && !wants_help(rest) => docs(rest),
        [flag] if flag == "-h" || flag == "--help" => help(),
        [cmd, rest @ ..] if (cmd == "check" || cmd == "docs") && wants_help(rest) => help(),
        _ => Err(crate::USAGE.to_string()),
    }
}

fn wants_help(args: &[String]) -> bool {
    args.iter().any(|a| a == "-h" || a == "--help")
}

fn help() -> Result<ExitCode, String> {
    println!("{}", crate::USAGE);
    Ok(ExitCode::SUCCESS)
}

fn print(text: &str) -> Result<(), String> {
    let mut stdout = std::io::stdout().lock();
    (stdout.write_all(text.as_bytes())).map_err(|e| format!("cannot write the output: {e}"))
}

/// What became of one file.
enum Outcome {
    /// No binding applies.
    Unbound,
    /// The file cannot be read, or its package cannot.
    Failed {
        binding: Option<Value>,
        error: String,
    },
    Checked {
        binding: Value,
        check: DocumentCheck,
    },
}

fn binding_json(binding: &Binding) -> Value {
    json!({
        "file": binding.file.display().to_string(),
        "library": binding.library,
        "version": binding.version,
        "sha256": binding.sha256.as_str(),
    })
}

/// The path the app would have for this file, so that the same binding is found.
fn document_path(file: &str) -> Result<PathBuf, String> {
    let path = std::path::absolute(file).map_err(|e| format!("{file}: {e}"))?;
    let folder = path.parent().ok_or_else(|| format!("{file}: not a file"))?;
    let folder = std::fs::canonicalize(folder).map_err(|e| format!("{file}: {e}"))?;
    let name = path.file_name().ok_or_else(|| format!("{file}: not a file"))?;
    Ok(folder.join(name))
}

fn check_file(file: &str, known: &mut HashMap<PathBuf, Bound>) -> Outcome {
    let failed = |error: String| Outcome::Failed { binding: None, error };
    let text = match crate::read_markdown(file) {
        Ok(text) => text,
        Err(e) => return failed(e),
    };
    let path = match document_path(file) {
        Ok(path) => path,
        Err(e) => return failed(e),
    };
    let home = std::env::home_dir();
    let Some(binding_file) = find_binding(&path, home.as_deref()) else {
        return Outcome::Unbound;
    };
    let bound = known.entry(binding_file.clone()).or_insert_with(|| {
        (read_bound_package(&binding_file))
            .map(|p| (p.binding, BlockChecker::new(Arc::new(p.index))))
            .map_err(|problem| problem.to_string())
    });
    match bound {
        Ok((binding, checker)) => Outcome::Checked {
            binding: binding_json(binding),
            check: checker.check(&text),
        },
        Err(error) => Outcome::Failed {
            binding: Some(json!({ "file": binding_file.display().to_string() })),
            error: error.clone(),
        },
    }
}

const NO_BINDING: &str = "no blocks.config.json applies to this file, so its blocks were not checked";

fn check(args: &[String]) -> Result<ExitCode, String> {
    let mut json = false;
    let mut files = Vec::new();
    for a in args {
        match a.as_str() {
            "--json" => json = true,
            f if !f.starts_with('-') => files.push(f),
            _ => return Err(crate::USAGE.to_string()),
        }
    }
    if files.is_empty() {
        return Err(crate::USAGE.to_string());
    }
    let mut known = HashMap::new();
    let results: Vec<(&str, Outcome)> = files.iter().map(|&f| (f, check_file(f, &mut known))).collect();
    let errors = results
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::Failed { .. }))
        .count();
    let problems: usize = (results.iter())
        .map(|(_, o)| match o {
            Outcome::Checked { check, .. } => check.problems.len(),
            _ => 0,
        })
        .sum();
    if json {
        print(&check_json(&results))?;
    } else {
        print(&check_text(&results))?;
    }
    Ok(ExitCode::from(if errors > 0 {
        2
    } else if problems > 0 {
        1
    } else {
        0
    }))
}

fn check_text(results: &[(&str, Outcome)]) -> String {
    let mut out = String::new();
    let (mut blocks, mut problems) = (0, 0);
    for (file, outcome) in results {
        match outcome {
            Outcome::Unbound => out.push_str(&format!("{file}: note: {NO_BINDING}\n")),
            Outcome::Failed { error, .. } => out.push_str(&format!("{file}: package or file error: {error}\n")),
            Outcome::Checked { check, .. } => {
                blocks += check.blocks;
                problems += check.problems.len();
                for p in &check.problems {
                    let block = p.block.as_ref().map_or(String::new(), |b| format!("{b}: "));
                    out.push_str(&format!(
                        "{file}:{}: {block}{}: {}\n",
                        p.line,
                        p.kind.headline(),
                        p.reason
                    ));
                }
                if check.problems.is_empty() {
                    out.push_str(&format!("{file}: ok, {} blocks\n", check.blocks));
                }
            }
        }
    }
    out.push_str(&format!(
        "files: {}, blocks: {blocks}, problems: {problems}\n",
        results.len()
    ));
    out
}

fn check_json(results: &[(&str, Outcome)]) -> String {
    let (mut blocks, mut problems, mut errors, mut unbound) = (0, 0, 0, 0);
    let files: Vec<Value> = (results.iter())
        .map(|(file, outcome)| match outcome {
            Outcome::Unbound => {
                unbound += 1;
                json!({"file": file, "status": "unbound", "note": NO_BINDING, "binding": null, "error": null,
                       "language": null, "blocks": 0, "problems": []})
            }
            Outcome::Failed { binding, error } => {
                errors += 1;
                json!({"file": file, "status": "error", "note": null, "binding": binding, "error": error,
                       "language": null, "blocks": 0, "problems": []})
            }
            Outcome::Checked { binding, check } => {
                blocks += check.blocks;
                problems += check.problems.len();
                let found: Vec<Value> = (check.problems.iter())
                    .map(|p| {
                        json!({"line": p.line, "block_line": p.block_line, "block": p.block, "kind": p.kind.name(),
                               "headline": p.kind.headline(), "reason": p.reason})
                    })
                    .collect();
                let status = if found.is_empty() { "clean" } else { "problems" };
                json!({"file": file, "status": status, "note": null, "binding": binding, "error": null,
                       "language": check.language, "blocks": check.blocks, "problems": found})
            }
        })
        .collect();
    let report = json!({
        "format": CHECK_FORMAT,
        "files": files,
        "summary": {"files": results.len(), "blocks": blocks, "problems": problems, "errors": errors, "unbound": unbound},
    });
    let mut text = serde_json::to_string_pretty(&report).unwrap_or_default();
    text.push('\n');
    text
}

/// A word of the regenerate command, quoted when a shell would read it otherwise.
fn shell_word(word: &str) -> String {
    let plain = |c: char| c.is_alphanumeric() || "-_./=:@+,".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// An archive read in memory, or a folder packed in memory as `pack_folder` does.
fn read_package(path: &str) -> Result<(PackageIndex, Sha256Hex), String> {
    let at = Path::new(path);
    let bytes = if at.is_dir() {
        pack_folder(at).map_err(|e| format!("{path}: {e}"))?.bytes
    } else {
        markise_doc::resource::read_regular(at, markise_package::limits::MAX_ARCHIVE_BYTES)
            .map_err(|e| format!("{path}: {e}"))?
    };
    let sha256 = Sha256Hex::of(&bytes);
    let files = read_archive(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let index = PackageIndex::from_files(&files).map_err(|e| format!("{path}: {e}"))?;
    Ok((index, sha256))
}

fn docs(args: &[String]) -> Result<ExitCode, String> {
    let (mut package, mut binding, mut lang) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let slot = match a.as_str() {
            "--package" => &mut package,
            "--binding" => &mut binding,
            "--lang" => &mut lang,
            _ => return Err(crate::USAGE.to_string()),
        };
        *slot = Some(it.next().ok_or(crate::USAGE)?.as_str());
    }
    if package.is_some() && binding.is_some() {
        return Err(crate::USAGE.to_string());
    }
    if lang.is_some_and(|l| !is_language_tag(l)) {
        return Err(crate::USAGE.to_string());
    }
    let (index, sha256) = match (package, binding) {
        (Some(path), _) => read_package(path)?,
        (None, Some(file)) => bound_package(Path::new(file))?,
        (None, None) => {
            let here = std::env::current_dir().map_err(|e| e.to_string())?.join(BINDING_FILE);
            let home = std::env::home_dir();
            let file = find_binding(&here, home.as_deref())
                .ok_or_else(|| format!("no {BINDING_FILE} here or above: name one with --binding or --package"))?;
            bound_package(&file)?
        }
    };
    let mut command = vec!["markise-cli blocks docs".to_string()];
    for (flag, value) in [("--package", package), ("--binding", binding), ("--lang", lang)] {
        if let Some(value) = value {
            command.push(format!("{flag} {}", shell_word(value)));
        }
    }
    let header = ReferenceHeader {
        sha256: sha256.as_str(),
        command: &command.join(" "),
    };
    print(&index.reference(header, lang.unwrap_or("en")))?;
    Ok(ExitCode::SUCCESS)
}

fn bound_package(file: &Path) -> Result<(PackageIndex, Sha256Hex), String> {
    // its `source` is relative to its folder, which a bare file name would not have
    let file = std::path::absolute(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let bound = read_bound_package(&file).map_err(|problem| format!("{}: {problem}", file.display()))?;
    Ok((bound.index, bound.binding.sha256))
}
