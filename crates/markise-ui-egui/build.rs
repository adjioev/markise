//! Windows: embeds the common-controls 6 manifest (`windows/common-controls-v6.manifest`,
//! issue #40) into every executable this package links — unit and integration tests,
//! examples, benches. rfd's `common-controls-v6` alerts import `TaskDialogIndirect`, which
//! only version 6 of comctl32 exports; an executable without the manifest fails to start.
//! (Link arguments do not reach dependent packages: `apps/markise/build.rs` embeds the same
//! manifest into `markise.exe`.) Needs the Windows resource compiler, like the app's icon.

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let manifest = dir.join("windows/common-controls-v6.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let rc = out.join("markise-ui-egui.rc");
    std::fs::write(&rc, manifest_resource(&manifest)).expect("write markise-ui-egui.rc");
    embed_resource::compile_for_everything(&rc, embed_resource::NONE)
        .manifest_required()
        .expect("compile the Windows manifest resource");
}

/// `1 24 "<path>"`: the application manifest (`RT_MANIFEST`, `CREATEPROCESS_MANIFEST_RESOURCE_ID`).
fn manifest_resource(manifest: &std::path::Path) -> String {
    let path = manifest.canonicalize().expect("the manifest exists");
    // absolute path, backslashes escaped for the .rc string literal
    format!("1 24 \"{}\"\n", path.display().to_string().replace('\\', "\\\\"))
}
