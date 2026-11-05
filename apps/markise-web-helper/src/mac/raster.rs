//! A snapshot to the bytes the protocol carries.

use markise_web_render::proto::{MAX_PIXELS, MAX_SIDE_PX};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, kCGColorSpaceSRGB,
};

/// RGBA8, premultiplied, sRGB, top row first, with no gap between rows.
pub fn rgba(image: &CGImage) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    let total = w.checked_mul(h)?.checked_mul(4)?;
    if w == 0 || h == 0 || w > MAX_SIDE_PX as usize || h > MAX_SIDE_PX as usize || total as u64 > MAX_PIXELS * 4 {
        return None;
    }
    let mut bytes = vec![0u8; total];
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    let info = CGImageAlphaInfo::PremultipliedLast.0;
    // SAFETY: the buffer outlives the context, which goes before the buffer goes anywhere, and its
    // rows are `w * 4` bytes.
    let cx = unsafe { CGBitmapContextCreate(bytes.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&space), info) }?;
    let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
    CGContext::draw_image(Some(&cx), rect, Some(image));
    drop(cx);
    Some((w as u32, h as u32, bytes))
}
