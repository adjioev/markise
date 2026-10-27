//! Image loading for page layout: local files only (remote images are never fetched during
//! export); dimensions read up front, pixels decoded lazily by the back ends.

use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Encoded image format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    /// Other formats `image` can decode (BMP); embedded as decoded RGBA.
    Other,
}

/// A loaded image: encoded bytes plus size; RGBA pixels on demand.
#[derive(Debug)]
pub struct LoadedImage {
    pub bytes: Arc<Vec<u8>>,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    rgba: OnceLock<Option<Arc<Vec<u8>>>>,
}

/// Largest accepted image file (bytes) and pixel count, to bound memory on hostile input.
const MAX_FILE: u64 = 64 << 20;
const MAX_PIXELS: u64 = 80_000_000;

impl LoadedImage {
    /// Decodes `bytes` (format sniffed from the content).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<LoadedImage, String> {
        let fmt = image::guess_format(&bytes).map_err(|e| e.to_string())?;
        let format = match fmt {
            image::ImageFormat::Png => ImageFormat::Png,
            image::ImageFormat::Jpeg => ImageFormat::Jpeg,
            image::ImageFormat::Gif => ImageFormat::Gif,
            image::ImageFormat::WebP => ImageFormat::Webp,
            image::ImageFormat::Bmp => ImageFormat::Other,
            other => return Err(format!("unsupported image format {other:?}")),
        };
        let reader = image::ImageReader::with_format(std::io::Cursor::new(&bytes), fmt);
        let (width, height) = reader.into_dimensions().map_err(|e| e.to_string())?;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(format!("image size {width}×{height} not supported"));
        }
        Ok(LoadedImage {
            bytes: Arc::new(bytes),
            format,
            width,
            height,
            rgba: OnceLock::new(),
        })
    }

    /// Straight-alpha RGBA8 pixels (decoded once).
    pub fn rgba(&self) -> Option<Arc<Vec<u8>>> {
        self.rgba
            .get_or_init(|| {
                image::load_from_memory(&self.bytes)
                    .ok()
                    .map(|i| Arc::new(i.to_rgba8().into_raw()))
            })
            .clone()
    }
}

/// Loads the image `src` (document-relative path, absolute path or `file://` URL).
pub fn load_image(base: Option<&Path>, root: Option<&str>, src: &str) -> Result<LoadedImage, String> {
    load_image_capped(base, root, src, MAX_FILE).map_err(|e| e.to_string())
}

/// Why [`load_image_capped`] did not load an image.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadError {
    /// The file is larger than the caller's remaining allowance (checked before reading).
    OverAllowance(u64),
    Other(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::OverAllowance(n) => write!(f, "the image ({} KB) exceeds the remaining image memory", n / 1024),
            LoadError::Other(e) => f.write_str(e),
        }
    }
}

/// [`load_image`] reading at most `cap` bytes: the size is checked on the opened handle before
/// anything is read (nothing over the cap is ever loaded), and the read through that handle is
/// bounded too.
pub fn load_image_capped(
    base: Option<&Path>,
    root: Option<&str>,
    src: &str,
    cap: u64,
) -> Result<LoadedImage, LoadError> {
    use std::io::Read;
    // the shared resolver of the viewer and the HTML export (document folder, resource root,
    // `file:` URLs, query/fragment); remote URLs and `data:` are never loaded
    let Some(local) = markise_doc::resource::local_file(base.unwrap_or(Path::new("")), root, src) else {
        let lower = src.trim().to_ascii_lowercase();
        return Err(LoadError::Other(if lower.starts_with("data:") {
            "data: URLs are not supported yet".into()
        } else {
            "remote images are not downloaded during export".into()
        }));
    };
    let path = local.path;
    // regular files only, never blocking (a FIFO named `.png`)
    let (file, len) =
        markise_doc::resource::open_regular(&path).map_err(|e| LoadError::Other(format!("{}: {e}", path.display())))?;
    if len > MAX_FILE {
        return Err(LoadError::Other(format!("{} is too large", path.display())));
    }
    let limit = MAX_FILE.min(cap);
    if len > limit {
        return Err(LoadError::OverAllowance(len));
    }
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| LoadError::Other(format!("{}: {e}", path.display())))?;
    if bytes.len() as u64 > limit {
        return Err(LoadError::Other(format!(
            "{} changed while it was read",
            path.display()
        )));
    }
    LoadedImage::from_bytes(bytes).map_err(LoadError::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Codex round 9: the PDF/PNG path resolves with the same function and table as the viewer
    // and the HTML export.
    #[test]
    fn shared_resolver_table() {
        for &(src, root, want, _) in markise_doc::resource::RESOLVER_CASES {
            let got = markise_doc::resource::local_file(Path::new("/docs"), root, src).map(|l| l.path);
            assert_eq!(got.is_some(), want.is_some(), "{src}");
            if want.is_none() {
                assert!(load_image(Some(Path::new("/docs")), root, src).is_err());
            }
        }
        // `/photo.png` with a resource root is not read from the filesystem root
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("media")).unwrap();
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(d.path().join("media/photo.png"), png).unwrap();
        let img = load_image(Some(d.path()), Some("./media"), "/photo.png").unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert!(load_image(Some(d.path()), None, "/photo.png").is_err());
    }

    // Codex round 10: the export-wide image allowance is checked on the file size before reading.
    #[test]
    fn allowance_is_checked_before_reading() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.png"), vec![0u8; 5000]).unwrap();
        assert_eq!(
            load_image_capped(Some(d.path()), None, "a.png", 4999).unwrap_err(),
            LoadError::OverAllowance(5000)
        );
    }

    // Codex round 8: the PDF/PNG image path never blocks on a FIFO and rejects oversized files.
    #[test]
    fn fifo_and_oversized_images_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            let fifo = d.path().join("pipe.png");
            let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
            // SAFETY: plain syscall with a valid NUL-terminated path.
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            let t = std::time::Instant::now();
            assert!(load_image(Some(d.path()), None, "pipe.png").is_err());
            assert!(t.elapsed() < std::time::Duration::from_secs(2));
        }
        let big = d.path().join("big.png");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_FILE + 1).unwrap();
        let e = load_image(Some(d.path()), None, "big.png").unwrap_err();
        assert!(e.contains("too large"), "{e}");
        assert!(load_image(Some(d.path()), None, ".").is_err(), "a directory");
    }
}
