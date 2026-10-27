//! Workspace tooling: `cargo xtask <command>` (alias in `.cargo/config.toml`).
//!
//! Pure Rust (std, serde_json, toml). External tools (`cargo`, `git`) are invoked through
//! `std::process::Command`. The command structure follows WordCraft's xtask (see NOTICE).

mod assets;
mod dist;
mod layers;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  layers   enforce the crate layering (layers.toml, which must match the AGENTS.md table)
  assets   check that every committed asset has an attribution record (SOURCE.md or NOTICE)
  ci       fmt --check, clippy -D warnings, test, assets, layers (stops at the first failure)
  dist     build and package release artifacts: `cargo xtask dist --help`
  version  print the workspace version (the release tag is `v` + this)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("layers") => cmd_layers(),
        Some("assets") => assets::run(&root()),
        Some("ci") => cmd_ci(),
        Some("dist") => dist::run(&root(), &args[1..]),
        Some("version") => dist::workspace_version(&root()).map(|v| println!("{v}")),
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Workspace root (parent of the xtask crate).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent dir")
        .to_path_buf()
}

pub(crate) fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn cmd_layers() -> Result<(), String> {
    let out = cargo()
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let meta = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata output: {e}"))?;
    layers::run(&root(), &meta)
}

fn run_step(name: &str, args: &[&str]) -> Result<(), String> {
    println!("==> cargo {}", args.join(" "));
    let status = cargo().args(args).status().map_err(|e| format!("{name}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{name} failed ({status})"))
    }
}

fn cmd_ci() -> Result<(), String> {
    run_step("fmt", &["fmt", "--all", "--check"])?;
    run_step(
        "clippy",
        &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
    )?;
    run_step("test", &["test", "--workspace"])?;
    println!("==> assets");
    assets::run(&root())?;
    println!("==> layers");
    cmd_layers()
}
