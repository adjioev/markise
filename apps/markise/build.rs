//! Windows: embeds the application icon (`assets/brand/markise.ico`) into `markise.exe` as
//! the first icon resource (shown by Explorer, the taskbar and Alt+Tab). Needs the Windows
//! resource compiler (`rc.exe` from the Windows SDK with MSVC, `windres` with GNU toolchains);
//! other targets are untouched.
//!
//! All targets: `MARKISE_COMMIT` = the short git commit of the sources (About window), when
//! the build runs inside a git checkout with `git` available; otherwise unset.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest.join("../../assets/brand/markise.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    git_commit(&manifest);
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon = icon.canonicalize().expect("assets/brand/markise.ico exists");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let rc = out.join("markise.rc");
    // absolute path, backslashes escaped for the .rc string literal
    let icon = icon.display().to_string().replace('\\', "\\\\");
    std::fs::write(&rc, format!("1 ICON \"{icon}\"\n")).expect("write markise.rc");
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_required()
        .expect("compile the Windows icon resource");
}

fn git(dir: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Exports the short commit and re-runs when HEAD or the checked-out branch moves.
fn git_commit(manifest: &std::path::Path) {
    let Some(commit) = git(manifest, &["rev-parse", "--short=9", "HEAD"]) else {
        return;
    };
    println!("cargo:rustc-env=MARKISE_COMMIT={commit}");
    let mut watch = vec!["HEAD".to_string(), "packed-refs".to_string()];
    if let Some(r) = git(manifest, &["symbolic-ref", "-q", "HEAD"]) {
        watch.push(r);
    }
    for w in watch {
        if let Some(p) = git(manifest, &["rev-parse", "--path-format=absolute", "--git-path", &w]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
}
