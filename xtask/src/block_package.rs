//! `cargo xtask block-package`: the example package as a document's folder can use it: the archive,
//! a binding that pins it, and the sample document. Nothing of this is committed: the binding's hash
//! always belongs to the files in `examples/block-package/`.

use std::fs;
use std::path::{Path, PathBuf};

use markise_package::{BINDING_FILE, pack_folder};

const HELP: &str = "\
usage: cargo xtask block-package [--out FOLDER]

Packs examples/block-package/package and writes, ready to open in Markise:
  blocks/<library>-<version>.tar.gz, blocks.config.json, and the sample document

  --out   output folder (default: target/block-package-example/); files in it are overwritten,
          nothing else there is touched
";

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut out = root.join("target").join("block-package-example");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => {
                let o = PathBuf::from(it.next().ok_or("--out needs a value")?);
                out = std::env::current_dir()
                    .map_err(|e| format!("current dir: {e}"))?
                    .join(o);
            }
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            other => return Err(format!("unknown argument `{other}`\n\n{HELP}")),
        }
    }
    let binding = build(root, &out)?;
    println!("block-package: wrote {}", binding.display());
    Ok(())
}

/// Returns the binding's path.
fn build(root: &Path, out: &Path) -> Result<PathBuf, String> {
    let example = root.join("examples").join("block-package");
    let packed = pack_folder(&example.join("package")).map_err(|e| format!("the example package: {e}"))?;
    copy_tree(&example.join("document"), out)?;
    let source = format!("blocks/{}-{}.tar.gz", packed.id.library, packed.id.version);
    write(&out.join(&source), &packed.bytes)?;
    let binding = serde_json::json!({
        "library": packed.id.library,
        "version": packed.id.version,
        "source": source,
        "sha256": packed.sha256.as_str(),
    });
    let text = serde_json::to_string_pretty(&binding).map_err(|e| e.to_string())? + "\n";
    let path = out.join(BINDING_FILE);
    write(&path, text.as_bytes())?;
    Ok(path)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let cannot = |e: std::io::Error| format!("{}: {e}", path.display());
    fs::create_dir_all(path.parent().expect("a file in a folder")).map_err(cannot)?;
    fs::write(path, bytes).map_err(cannot)
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let cannot = |e: std::io::Error| format!("{}: {e}", from.display());
    for entry in fs::read_dir(from).map_err(cannot)? {
        let entry = entry.map_err(cannot)?;
        let target = to.join(entry.file_name());
        if entry.file_type().map_err(cannot)?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            write(&target, &fs::read(entry.path()).map_err(cannot)?)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use markise_package::{Resolution, Store, resolve};

    use super::*;

    #[test]
    fn what_it_builds_is_a_package_a_document_can_use() {
        let base = std::env::temp_dir().join(format!("markise-block-package-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let out = base.join("out");
        build(&crate::root(), &out).unwrap();
        let store = Store::in_config(&base.join("config"));
        let found = resolve(&out.join("weeknight-loaf.md"), &store, Some(&base));
        let _ = fs::remove_dir_all(&base);
        let Resolution::Ask(pending) = found else {
            panic!("expected a question, got {found:?}");
        };
        assert_eq!(pending.id.library, "recipe-site");
        assert_eq!(pending.block_count, 3);
    }
}
