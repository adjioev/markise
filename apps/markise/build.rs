//! Windows: embeds the application icon (`assets/brand/markise.ico`) into `markise.exe` as
//! the first icon resource (shown by Explorer, the taskbar and Alt+Tab), and the application
//! manifest binding it to common controls 6
//! (`crates/markise-ui-egui/windows/common-controls-v6.manifest`, issue #40): the native
//! alerts (rfd `common-controls-v6`) import `TaskDialogIndirect`, so without the manifest the
//! app does not start. Both go into every executable of this package (the app, its test
//! harness, examples). Needs the Windows resource compiler (`rc.exe` from the Windows SDK with
//! MSVC, `windres` with GNU toolchains); other targets are untouched.
//!
//! All targets: `MARKISE_COMMIT` = the short git commit of the sources (About window), when
//! the build runs inside a git checkout with `git` available; otherwise unset.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest.join("../../assets/brand/markise.ico");
    let app_manifest = manifest.join("../../crates/markise-ui-egui/windows/common-controls-v6.manifest");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", app_manifest.display());
    git_commit(&manifest);
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon = rc_path(&icon, "assets/brand/markise.ico");
    let app_manifest = rc_path(&app_manifest, "the common-controls manifest");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let rc = out.join("markise.rc");
    // 24 = RT_MANIFEST, id 1 = the manifest the loader reads at process creation
    std::fs::write(&rc, format!("1 ICON \"{icon}\"\n1 24 \"{app_manifest}\"\n")).expect("write markise.rc");
    embed_resource::compile_for_everything(&rc, embed_resource::NONE)
        .manifest_required()
        .expect("compile the Windows icon and manifest resources");
}

/// Absolute path of `p`, backslashes escaped for an .rc string literal.
fn rc_path(p: &std::path::Path, what: &str) -> String {
    let p = p.canonicalize().unwrap_or_else(|e| panic!("{what} exists: {e}"));
    p.display().to_string().replace('\\', "\\\\")
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
