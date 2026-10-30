//! `markise-blocks` — renderers for special blocks (layer L2): image decoding, code
//! highlighting ([`highlight`]), TeX math ([`math`], ADR 0008) and diagrams ([`diagram`],
//! ADR 0009).
//!
//! Images are decoded off the UI thread by a small worker pool, with bounded memory:
//! - only regular files are opened (FIFOs, devices and directories are refused; on Unix the
//!   file is opened non-blocking and the opened handle is checked again, so a FIFO swapped in
//!   between check and open cannot block a worker);
//! - the encoded file is bounded ([`DecodeLimits::max_encoded`], checked on the size and
//!   enforced while reading) and its bytes are charged to the budget before any decoder is
//!   constructed (some decoders buffer the whole input); the header is then read by the same
//!   decoder that later decodes, and images above the side/pixel limits are refused before
//!   any pixel buffer is allocated; the decoder also gets an allocation limit;
//! - concurrent decodes share a byte budget ([`DECODE_BUDGET`]), so workers never hold more
//!   than that in pixel buffers at once (a single image above the budget decodes alone);
//! - jobs carry a generation: [`ImageLoader::set_generation`] cancels everything queued for
//!   an older document; the job and result queues are bounded.

pub mod diagram;
pub mod highlight;
pub mod math;
#[doc(hidden)]
pub mod wake_track;

use std::fs::File;
use std::io::{Cursor, Read};

use image::ImageDecoder;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};

/// A decoded image ready for upload.
#[derive(Clone, Debug)]
pub struct DecodedImage {
    /// Natural size in pixels (used for layout).
    pub size: [u32; 2],
    /// Pixel size of `rgba` (may be smaller than `size` after downscaling).
    pub pixels_size: [usize; 2],
    /// Straight-alpha RGBA8.
    pub rgba: Vec<u8>,
}

/// Result of one decode request.
#[derive(Debug)]
pub struct Decoded {
    pub src: String,
    pub generation: u64,
    pub result: Result<DecodedImage, String>,
}

/// Largest texture side kept after decoding.
pub const MAX_TEXTURE_SIDE: u32 = 2048;

/// Bytes of pixel buffers all workers together may hold while decoding.
pub const DECODE_BUDGET: u64 = 192 << 20;

#[derive(Clone, Copy, Debug)]
pub struct DecodeLimits {
    /// Largest encoded file accepted.
    pub max_encoded: u64,
    pub max_side: u32,
    pub max_pixels: u64,
    /// Passed to the decoder as its allocation limit.
    pub max_alloc: u64,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        DecodeLimits {
            max_encoded: 64 << 20,
            max_side: 16_384,
            max_pixels: 40_000_000,
            max_alloc: 256 << 20,
        }
    }
}

/// Shared byte budget for concurrent decodes.
#[derive(Clone)]
pub struct MemBudget(Arc<(Mutex<u64>, Condvar, u64)>);

impl MemBudget {
    pub fn new(capacity: u64) -> Self {
        MemBudget(Arc::new((Mutex::new(0), Condvar::new(), capacity)))
    }

    /// Blocks until `n` bytes fit (or nothing else is held), then reserves them.
    fn acquire(&self, n: u64) -> BudgetGuard {
        let (m, cv, cap) = &*self.0;
        let mut used = m.lock().unwrap_or_else(|e| e.into_inner());
        while *used > 0 && *used + n > *cap {
            used = cv.wait(used).unwrap_or_else(|e| e.into_inner());
        }
        *used += n;
        BudgetGuard(self.clone(), n)
    }

    pub fn used(&self) -> u64 {
        *self.0.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct BudgetGuard(MemBudget, u64);

impl Drop for BudgetGuard {
    fn drop(&mut self) {
        let (m, cv, _) = &*self.0.0;
        let mut used = m.lock().unwrap_or_else(|e| e.into_inner());
        *used -= self.1;
        cv.notify_all();
    }
}

/// Opens `path` only if it is a regular file, without ever blocking on a FIFO.
fn open_regular(path: &Path) -> Result<File, String> {
    // the shared hardened opener: regular files only, never blocking
    markise_doc::resource::open_regular(path)
        .map(|(f, _)| f)
        .map_err(|e| format!("cannot open: {e}"))
}

/// Decodes the image at `src` (a local path). URLs are not fetched (M1).
pub fn decode(src: &str, max_side: u32) -> Result<DecodedImage, String> {
    decode_with(src, max_side, DecodeLimits::default(), &MemBudget::new(DECODE_BUDGET))
}

/// [`decode`] with explicit limits and a shared budget.
pub fn decode_with(src: &str, max_side: u32, limits: DecodeLimits, budget: &MemBudget) -> Result<DecodedImage, String> {
    if src.contains("://") || src.starts_with("data:") {
        return Err("remote/inline images are not loaded yet".into());
    }
    let file = open_regular(Path::new(src))?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len > limits.max_encoded {
        return Err(format!("image file too large ({} MB)", len >> 20));
    }
    // the encoded bytes (read here, and possibly copied once more by the decoder) count
    // against the budget before any decoder exists
    let input_guard = budget.acquire(len.saturating_mul(2));
    let mut bytes = Vec::with_capacity(len as usize);
    // the bound is enforced while reading too (the file may grow after the size check)
    file.take(limits.max_encoded + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read: {e}"))?;
    if bytes.len() as u64 > limits.max_encoded {
        return Err("image file too large".into());
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut l = image::Limits::default();
    l.max_image_width = Some(limits.max_side);
    l.max_image_height = Some(limits.max_side);
    l.max_alloc = Some(limits.max_alloc);
    reader.limits(l);
    // one decoder: header first, then (if acceptable) the pixels
    let decoder = reader.into_decoder().map_err(|e| format!("cannot decode: {e}"))?;
    let (w, h) = decoder.dimensions();
    if w > limits.max_side || h > limits.max_side || w as u64 * h as u64 > limits.max_pixels {
        return Err(format!("image too large to display ({w}×{h})"));
    }
    // decoded buffer + RGBA8 conversion + encoded input. Re-acquired as one reservation
    // (releasing first) so two workers can never wait on each other's partial holdings.
    let need = len.saturating_mul(2) + decoder.total_bytes() + w as u64 * h as u64 * 4;
    drop(input_guard);
    let _guard = budget.acquire(need);
    let img = image::DynamicImage::from_decoder(decoder).map_err(|e| format!("cannot decode: {e}"))?;
    let size = [img.width(), img.height()];
    let img = if img.width() > max_side || img.height() > max_side {
        img.thumbnail(max_side, max_side)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(DecodedImage {
        size,
        pixels_size: [rgba.width() as usize, rgba.height() as usize],
        rgba: rgba.into_raw(),
    })
}

/// Background image decoder: `request` queues a path, results arrive on `try_recv`.
pub struct ImageLoader {
    jobs: SyncSender<(String, u64)>,
    results: Receiver<Decoded>,
    generation: Arc<AtomicU64>,
    pub budget: MemBudget,
}

/// Queue lengths.
pub const MAX_QUEUED_JOBS: usize = 64;
pub const MAX_QUEUED_RESULTS: usize = 8;

impl ImageLoader {
    /// Starts `threads` workers; `wake` is called after each finished decode (e.g. to request
    /// a repaint). Returns an error if no worker thread could be started.
    pub fn new(threads: usize, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let (jobs, job_rx) = sync_channel::<(String, u64)>(MAX_QUEUED_JOBS);
        let (res_tx, results) = sync_channel(MAX_QUEUED_RESULTS);
        let job_rx = Arc::new(Mutex::new(job_rx));
        let generation = Arc::new(AtomicU64::new(0));
        let budget = MemBudget::new(DECODE_BUDGET);
        let mut started = 0;
        for i in 0..threads.max(1) {
            let job_rx = job_rx.clone();
            let res_tx = res_tx.clone();
            let wake = wake.clone();
            let generation = generation.clone();
            let budget = budget.clone();
            let r = std::thread::Builder::new()
                .name(format!("markise-image-{i}"))
                .spawn(move || {
                    loop {
                        let job = { job_rx.lock().ok().and_then(|r| r.recv().ok()) };
                        let Some((src, gen_)) = job else { break };
                        if gen_ < generation.load(Ordering::Acquire) {
                            continue; // cancelled: belongs to a closed document
                        }
                        let result = decode_with(&src, MAX_TEXTURE_SIDE, DecodeLimits::default(), &budget);
                        let published = crate::wake_track::publishing();
                        if res_tx
                            .send(Decoded {
                                src,
                                generation: gen_,
                                result,
                            })
                            .is_err()
                        {
                            drop(published);
                            break;
                        }
                        wake();
                        drop(published);
                    }
                });
            if r.is_ok() {
                started += 1;
            }
        }
        if started == 0 {
            return Err("cannot start image worker threads".into());
        }
        Ok(ImageLoader {
            jobs,
            results,
            generation,
            budget,
        })
    }

    /// Queues `src` for generation `generation`; `false` when the queue is full (retry later).
    pub fn request(&self, src: &str, generation: u64) -> bool {
        match self.jobs.try_send((src.to_string(), generation)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Cancels queued jobs of older generations.
    pub fn set_generation(&self, generation: u64) {
        self.generation.store(generation, Ordering::Release);
    }

    pub fn try_recv(&self) -> Option<Decoded> {
        self.results.try_recv().ok()
    }
}

/// Upper bound for tests waiting on a worker's result: a hang guard, not a latency
/// expectation (issue #42: fixed deadlines failed on loaded machines).
#[cfg(test)]
pub(crate) const TEST_HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(180);

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("markise-blocks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn recv(loader: &ImageLoader, n: usize) -> Vec<Decoded> {
        let t = Instant::now();
        let mut v = Vec::new();
        while v.len() < n && t.elapsed() < crate::TEST_HANG_GUARD {
            match loader.try_recv() {
                Some(d) => v.push(d),
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        v
    }

    #[test]
    fn decodes_and_downscales_off_thread() {
        let d = dir("decode");
        let p = d.join("big.png");
        image::RgbaImage::from_pixel(3000, 1000, image::Rgba([10, 20, 30, 255]))
            .save(&p)
            .unwrap();
        let loader = ImageLoader::new(1, Arc::new(|| {})).unwrap();
        assert!(loader.request(p.to_str().unwrap(), 0));
        assert!(loader.request(d.join("missing.png").to_str().unwrap(), 0));
        let v = recv(&loader, 2);
        let (ok, err): (Vec<_>, Vec<_>) = v.into_iter().partition(|x| x.result.is_ok());
        let img = ok[0].result.as_ref().unwrap();
        assert_eq!(img.size, [3000, 1000]);
        assert_eq!(img.pixels_size, [2048, 683]);
        assert_eq!(img.rgba.len(), 2048 * 683 * 4);
        assert!(err[0].result.is_err());
        assert!(decode("https://example.com/x.png", 64).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn oversized_images_are_refused_from_the_header() {
        let d = dir("limits");
        let p = d.join("wide.png");
        image::RgbaImage::from_pixel(3000, 1000, image::Rgba([1, 2, 3, 255]))
            .save(&p)
            .unwrap();
        let budget = MemBudget::new(DECODE_BUDGET);
        let small = DecodeLimits {
            max_pixels: 1_000_000,
            ..Default::default()
        };
        let e = decode_with(p.to_str().unwrap(), 2048, small, &budget).unwrap_err();
        assert!(e.contains("too large"), "{e}");
        let side = DecodeLimits {
            max_side: 2000,
            ..Default::default()
        };
        assert!(decode_with(p.to_str().unwrap(), 2048, side, &budget).is_err());
        assert_eq!(budget.used(), 0, "nothing reserved for refused images");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn oversized_encoded_input_is_refused_before_any_decoder() {
        // second review, item 6: a small image with huge trailing data
        let d = dir("encoded");
        let p = d.join("padded.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
            .save(&p)
            .unwrap();
        let mut bytes = std::fs::read(&p).unwrap();
        bytes.extend(std::iter::repeat_n(0u8, 200_000));
        std::fs::write(&p, &bytes).unwrap();
        let budget = MemBudget::new(DECODE_BUDGET);
        let small = DecodeLimits {
            max_encoded: 100_000,
            ..Default::default()
        };
        let e = decode_with(p.to_str().unwrap(), 2048, small, &budget).unwrap_err();
        assert!(e.contains("file too large"), "{e}");
        assert_eq!(budget.used(), 0);
        // within the bound it decodes (trailing bytes are ignored by the PNG decoder)
        let img = decode_with(p.to_str().unwrap(), 2048, DecodeLimits::default(), &budget).unwrap();
        assert_eq!(img.size, [8, 8]);
        assert_eq!(budget.used(), 0, "every reservation released");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn budget_serialises_large_decodes() {
        let b = MemBudget::new(100);
        let g1 = b.acquire(80);
        let b2 = b.clone();
        let t = std::thread::spawn(move || {
            let _g = b2.acquire(50); // must wait for g1
            b2.used()
        });
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(b.used(), 80);
        drop(g1);
        assert_eq!(t.join().unwrap(), 50);
        assert_eq!(b.used(), 0);
        // a single request above the capacity proceeds alone
        let g = b.acquire(500);
        assert_eq!(b.used(), 500);
        drop(g);
    }

    #[cfg(unix)]
    #[test]
    fn special_files_never_block_the_workers() {
        let d = dir("fifo");
        let fifo = d.join("pipe.png");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        let link = d.join("link.png");
        std::os::unix::fs::symlink(&fifo, &link).unwrap();
        let t = Instant::now();
        for p in [&fifo, &link, &d] {
            let e = decode(p.to_str().unwrap(), 64).unwrap_err();
            assert!(e.contains("regular file"), "{e}");
        }
        assert!(t.elapsed() < Duration::from_secs(1));
        // the pool keeps working after FIFO requests
        let png = d.join("ok.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
            .save(&png)
            .unwrap();
        let loader = ImageLoader::new(2, Arc::new(|| {})).unwrap();
        loader.request(fifo.to_str().unwrap(), 0);
        loader.request(link.to_str().unwrap(), 0);
        loader.request(png.to_str().unwrap(), 0);
        let v = recv(&loader, 3);
        assert_eq!(v.len(), 3);
        assert!(v.iter().any(|x| x.result.is_ok()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn older_generations_are_cancelled() {
        let d = dir("gen");
        let png = d.join("ok.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
            .save(&png)
            .unwrap();
        let loader = ImageLoader::new(1, Arc::new(|| {})).unwrap();
        loader.set_generation(5);
        loader.request(png.to_str().unwrap(), 4); // stale: skipped
        loader.request(png.to_str().unwrap(), 5);
        let v = recv(&loader, 1);
        assert_eq!(v[0].generation, 5);
        std::thread::sleep(Duration::from_millis(200));
        assert!(loader.try_recv().is_none(), "the stale job produced no result");
        let _ = std::fs::remove_dir_all(&d);
    }
}
