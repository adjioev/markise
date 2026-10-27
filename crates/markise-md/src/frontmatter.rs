//! Front-matter keys interpreted by the app (spec 01 §8): shallow YAML reading,
//! `typora-root-url` image resolution and `typora-copy-images-to` targets.

use std::path::{Path, PathBuf};

use crate::Document;
use crate::block::BlockKind;

/// Shallow YAML (R8.1): `key: value` lines, quoted keys, `#` comments. Inline lists/maps are
/// returned as their raw text. Nested structures are ignored.
pub fn parse_shallow_yaml(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim_start_matches('\u{200B}');
        if line.starts_with([' ', '\t']) || line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let (key, rest) = if let Some(q) = line.strip_prefix('"').or_else(|| line.strip_prefix('\'')) {
            let qc = line.as_bytes()[0] as char;
            let Some(end) = q.find(qc) else { continue };
            let after = q[end + 1..].trim_start();
            let Some(rest) = after.strip_prefix(':') else { continue };
            (q[..end].to_string(), rest)
        } else {
            let Some(colon) = line.find(':') else { continue };
            (line[..colon].trim().to_string(), &line[colon + 1..])
        };
        let mut value = rest.trim();
        // strip trailing comment (only when preceded by whitespace and not quoted)
        if !value.starts_with(['"', '\''])
            && let Some(i) = value.find(" #")
        {
            value = value[..i].trim_end();
        }
        let value = unquote(value);
        out.push((key, value));
    }
    out
}

fn unquote(v: &str) -> String {
    if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

/// Front-matter key/value pairs of a document (empty when there is no front matter).
pub fn front_matter(doc: &Document) -> Vec<(String, String)> {
    match doc.top(0) {
        Some(t) if matches!(t.block.kind, BlockKind::FrontMatter { .. }) => {
            parse_shallow_yaml(&t.to_abs().content(doc.text()))
        }
        _ => Vec::new(),
    }
}

/// The `typora-root-url` value of the document's front matter (resource root, R8.1).
pub fn resource_root(doc: &Document) -> Option<String> {
    get(&front_matter(doc), "typora-root-url").map(str::to_string)
}

/// Looks up a key (with or without the `typora-` prefix).
pub fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Resolves an image `src` for display (R8.1 `typora-root-url`, R9.1).
/// URLs are returned unchanged; root-relative paths resolve against the root URL
/// (itself relative to the document folder); other relative paths against the document folder.
/// (The format-neutral implementation lives in `markise_doc::resource`.)
pub fn resolve_image_src(doc_dir: &Path, root_url: Option<&str>, src: &str) -> PathBuf {
    markise_doc::resource::resolve(doc_dir, root_url, src)
}

/// Where inserted images go (R8.1 `typora-copy-images-to`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyImagesTarget {
    /// Copy into `folder`; reference the image as `<src_prefix>/<name>`.
    Folder { folder: PathBuf, src_prefix: String },
    /// Upload with the configured uploader (`upload` / `ipic`).
    Upload,
}

/// Interprets a `typora-copy-images-to` value for a document at `doc_path`.
pub fn copy_images_target(doc_path: &Path, value: &str) -> CopyImagesTarget {
    let v = value.trim();
    if v.eq_ignore_ascii_case("upload") || v.eq_ignore_ascii_case("ipic") {
        return CopyImagesTarget::Upload;
    }
    let stem = doc_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let expanded = v.replace("${filename}", stem);
    let doc_dir = doc_path.parent().unwrap_or(Path::new(""));
    if markise_doc::resource::is_absolute_like(&expanded) {
        let folder = PathBuf::from(&expanded);
        return CopyImagesTarget::Folder {
            src_prefix: expanded.trim_end_matches(['/', '\\']).to_string(),
            folder,
        };
    }
    let rel = expanded
        .trim_start_matches("./")
        .trim_end_matches(['/', '\\'])
        .to_string();
    CopyImagesTarget::Folder {
        folder: doc_dir.join(&rel),
        src_prefix: rel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shallow_yaml() {
        let y = "title: \"Hello\"\n'typora-root-url': ./site # c\ntags: [a, b]\n  nested: x\n# comment\n";
        let p = parse_shallow_yaml(y);
        assert_eq!(get(&p, "title"), Some("Hello"));
        assert_eq!(get(&p, "typora-root-url"), Some("./site"));
        assert_eq!(get(&p, "tags"), Some("[a, b]"));
        assert_eq!(get(&p, "nested"), None);
    }

    #[test]
    fn t42_root_url() {
        let doc = crate::parse("---\ntypora-root-url: ./site\n---\n![a](/img/x.png)\n");
        let fm = front_matter(&doc);
        let root = get(&fm, "typora-root-url");
        let p = resolve_image_src(Path::new("/docs"), root, "/img/x.png");
        assert_eq!(p, PathBuf::from("/docs/site/img/x.png"));
        assert_eq!(
            resolve_image_src(Path::new("/docs"), root, "a.png"),
            PathBuf::from("/docs/a.png")
        );
        assert_eq!(
            resolve_image_src(Path::new("/docs"), root, "https://x/y.png"),
            PathBuf::from("https://x/y.png")
        );
    }

    #[test]
    fn t43_copy_images_to() {
        let t = copy_images_target(Path::new("/notes/post.md"), "./${filename}.assets");
        assert_eq!(
            t,
            CopyImagesTarget::Folder {
                folder: PathBuf::from("/notes/post.assets"),
                src_prefix: "post.assets".into()
            }
        );
        assert_eq!(
            copy_images_target(Path::new("/a/b.md"), "IPIC"),
            CopyImagesTarget::Upload
        );
    }
}
