//! `cargo xtask dist <platform> [--label <label>] [--out <dir>]`: release artifacts.
//!
//! Builds `markise` and `markise-cli` with the `dist` profile (release + thin LTO + strip) for
//! the platform's Rust target(s), joins the two macOS architectures with `lipo` for
//! `macos-universal`, then packages them with `scripts/package.sh` (archive layout, Info.plist,
//! .dmg, desktop entry, licences, reproducibility rules: see that script). The release workflow
//! (`.github/workflows/release.yml`) runs exactly this command, so a local run produces the same
//! artifact layout as CI.

use std::path::{Path, PathBuf};
use std::process::Command;

pub const USAGE: &str = "\
usage: cargo xtask dist <platform> [--label <label>] [--out <dir>]

platforms (the Rust target must be installed: `rustup target add <target>`):
  macos-universal   aarch64-apple-darwin + x86_64-apple-darwin (lipo) -> .dmg   (macOS host)
  macos-aarch64     aarch64-apple-darwin                         -> .dmg   (macOS host)
  macos-x86_64      x86_64-apple-darwin                          -> .dmg   (macOS host)
  windows-x86_64    x86_64-pc-windows-msvc                       -> .zip   (Windows host)
  linux-x86_64      x86_64-unknown-linux-gnu                     -> .tar.gz (Linux host)

  --label   archive label (default: the workspace version, e.g. 0.1.0-beta.1)
  --out     output folder (default: dist/)
Environment: SOURCE_DATE_EPOCH, PACKAGE_STABLE_NAME, MACOS_FORMAT, BUILD_NUMBER (see
scripts/package.sh); XTASK_BASH = bash executable for scripts/package.sh (default `bash`).
";

/// Oldest macOS the bundle supports; matches `LSMinimumSystemVersion` in scripts/package.sh.
const MACOS_DEPLOYMENT_TARGET: &str = "11.0";
const PROFILE: &str = "dist";
const BINS: &[&str] = &["markise", "markise-cli"];

fn targets(platform: &str) -> Option<&'static [&'static str]> {
    Some(match platform {
        "macos-universal" => &["aarch64-apple-darwin", "x86_64-apple-darwin"],
        "macos-aarch64" => &["aarch64-apple-darwin"],
        "macos-x86_64" => &["x86_64-apple-darwin"],
        "windows-x86_64" => &["x86_64-pc-windows-msvc"],
        "linux-x86_64" => &["x86_64-unknown-linux-gnu"],
        _ => return None,
    })
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut platform = None;
    let mut label = None;
    let mut out = root.join("dist");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--label" => label = Some(it.next().ok_or("--label needs a value")?.clone()),
            "--out" => {
                let o = PathBuf::from(it.next().ok_or("--out needs a value")?);
                out = std::env::current_dir()
                    .map_err(|e| format!("current dir: {e}"))?
                    .join(o);
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            p if platform.is_none() && !p.starts_with('-') => platform = Some(p.to_string()),
            other => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
        }
    }
    let platform = platform.ok_or_else(|| format!("missing <platform>\n\n{USAGE}"))?;
    let targets = targets(&platform).ok_or_else(|| format!("unknown platform `{platform}`\n\n{USAGE}"))?;
    let label = match label {
        Some(l) => l,
        None => workspace_version(root)?,
    };

    let target_dir = target_dir(root)?;
    for target in targets {
        println!("==> cargo build --profile {PROFILE} --target {target}");
        let mut cmd = crate::cargo();
        cmd.args(["build", "--locked", "--profile", PROFILE, "--target", target]);
        for b in BINS {
            cmd.args(["-p", b]);
        }
        if target.ends_with("-apple-darwin") {
            cmd.env("MACOSX_DEPLOYMENT_TARGET", MACOS_DEPLOYMENT_TARGET);
        }
        status(&mut cmd, "cargo build")?;
    }

    let exe = if platform.starts_with("windows-") { ".exe" } else { "" };
    let bin_dir = if targets.len() > 1 {
        // universal binaries via lipo
        let dir = target_dir.join("universal-apple-darwin").join(PROFILE);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for b in BINS {
            let mut cmd = Command::new("lipo");
            cmd.arg("-create").arg("-output").arg(dir.join(b));
            for t in targets {
                cmd.arg(target_dir.join(t).join(PROFILE).join(b));
            }
            status(&mut cmd, "lipo -create")?;
            status(Command::new("lipo").arg("-info").arg(dir.join(b)), "lipo -info")?;
        }
        dir
    } else {
        target_dir.join(targets[0]).join(PROFILE)
    };
    for b in BINS {
        let f = bin_dir.join(format!("{b}{exe}"));
        let len = std::fs::metadata(&f)
            .map_err(|e| format!("{}: {e}", f.display()))?
            .len();
        println!("    {} ({:.1} MB)", f.display(), len as f64 / 1e6);
    }

    println!("==> scripts/package.sh {platform} {label}");
    // XTASK_BASH: explicit bash (Windows runners: Git Bash; a bare `bash` there may resolve to
    // the WSL launcher in System32, which Windows searches before PATH)
    let mut cmd = Command::new(std::env::var_os("XTASK_BASH").unwrap_or_else(|| "bash".into()));
    cmd.current_dir(root)
        .arg("scripts/package.sh")
        .arg(&platform)
        .arg(&label)
        .arg(shell_path(root, &bin_dir))
        .arg(shell_path(root, &out));
    status(&mut cmd, "scripts/package.sh")
}

/// Path for the bash script: relative to the workspace root with `/` separators when possible
/// (Git Bash on Windows mangles some absolute `C:\…` paths), else as is.
fn shell_path(root: &Path, p: &Path) -> String {
    let rel = p.strip_prefix(root).unwrap_or(p);
    let s = rel.to_string_lossy();
    if rel.is_relative() {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    }
}

fn status(cmd: &mut Command, what: &str) -> Result<(), String> {
    let s = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    if s.success() {
        Ok(())
    } else {
        Err(format!("{what} failed ({s})"))
    }
}

fn metadata(root: &Path) -> Result<serde_json::Value, String> {
    let out = crate::cargo()
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata output: {e}"))
}

fn target_dir(root: &Path) -> Result<PathBuf, String> {
    let meta = metadata(root)?;
    meta["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| "cargo metadata: no target_directory".into())
}

/// Version of the `markise` package (the workspace version).
pub fn workspace_version(root: &Path) -> Result<String, String> {
    let meta = metadata(root)?;
    meta["packages"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["name"] == "markise"))
        .and_then(|p| p["version"].as_str())
        .map(str::to_string)
        .ok_or_else(|| "cargo metadata: package `markise` not found".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platforms_map_to_targets() {
        assert_eq!(targets("macos-universal").map(<[_]>::len), Some(2));
        assert_eq!(targets("windows-x86_64"), Some(&["x86_64-pc-windows-msvc"][..]));
        assert_eq!(targets("linux-x86_64"), Some(&["x86_64-unknown-linux-gnu"][..]));
        assert!(targets("freebsd-x86_64").is_none());
    }

    #[test]
    fn shell_paths_are_root_relative() {
        let root = Path::new("/w/markise");
        assert_eq!(
            shell_path(root, &root.join("target").join("x").join("dist")),
            "target/x/dist"
        );
        assert_eq!(shell_path(root, Path::new("/elsewhere/out")), "/elsewhere/out");
    }
}
