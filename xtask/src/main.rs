//! Workspace tooling: `cargo xtask <command>` (alias in `.cargo/config.toml`).
//!
//! Pure Rust (std, serde_json, toml; `site` renders Markdown with markise-md and markise-export). External tools (`cargo`, `git`) are invoked through
//! `std::process::Command`. The command structure follows WordCraft's xtask (see NOTICE).

mod assets;
mod dist;
mod layers;
mod site;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  layers   enforce the crate layering (layers.toml, which must match the AGENTS.md table)
  assets   check that every committed asset has an attribution record (SOURCE.md or NOTICE)
  ci       fmt --check, clippy -D warnings, test, assets, layers, site (stops at the first failure)
  heavy    heavy tests (release) and 3000-case incremental properties; excludes known bugs
  dist     build and package release artifacts: `cargo xtask dist --help`
  site     build the website and user guide into target/site/ and check its links
  version  print the workspace version (the release tag is `v` + this)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("layers") => cmd_layers(),
        Some("assets") => assets::run(&root()),
        Some("ci") => cmd_ci(),
        Some("heavy") => cmd_heavy(),
        Some("dist") => dist::run(&root(), &args[1..]),
        Some("site") => site::run(&root(), &args[1..]),
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
    cmd_layers()?;
    println!("==> site");
    site::run(&root(), &[])
}

/// The name filter is mandatory: bare `--ignored` also runs known-bug tests.
fn cmd_heavy() -> Result<(), String> {
    let args = [
        "test",
        "--release",
        "--workspace",
        "--locked",
        "heavy_suite_",
        "--",
        "--ignored",
    ];
    println!("==> cargo {}", args.join(" "));
    // glibc's mmap threshold is fixed from process start (thread-safe, unlike `mallopt`), so
    // the scaling checks page-fault large blocks alike at both input sizes (issue #109; see
    // `markise_corpus::assert_scales_linearly`). Non-glibc targets ignore the variable.
    let tunable = "glibc.malloc.mmap_threshold=131072";
    let tunables = match std::env::var("GLIBC_TUNABLES") {
        Ok(t) if !t.is_empty() => format!("{t}:{tunable}"),
        _ => tunable.to_string(),
    };
    let status = cargo()
        .env("GLIBC_TUNABLES", tunables)
        .args(args)
        .status()
        .map_err(|e| format!("heavy: {e}"))?;
    if !status.success() {
        return Err(format!("heavy failed ({status})"));
    }
    let status = cargo()
        .env("PROPTEST_CASES", "3000")
        .args([
            "test",
            "--release",
            "--locked",
            "-p",
            "markise-md",
            "-p",
            "markise-layout",
            // one integration-test binary per crate (issue #116): the property modules are
            // selected by name (`nested`, `fastpath`, `hints` in markise-md, `incremental` in
            // markise-layout)
            "--test",
            "it",
            "--",
            "nested::",
            "fastpath::",
            "hints::",
            "incremental::",
        ])
        .status()
        .map_err(|e| format!("stress properties: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("stress properties failed ({status})"))
    }
}
