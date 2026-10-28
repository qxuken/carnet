//! Pictures the `image` crate can't read, read by macOS: an iPhone's HEIC
//! above all (ImageIO decodes it natively), and TIFF or AVIF on the way.
//! Read the right way up, no side longer than asked, as straight RGBA.

use std::ffi::c_void;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::images::Decoded;

type Ref = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(alloc: Ref, buf: *const u8, len: isize, is_dir: u8) -> Ref;
    fn CFNumberCreate(alloc: Ref, kind: isize, value: *const c_void) -> Ref;
    fn CFDictionaryCreate(alloc: Ref, keys: *const Ref, values: *const Ref, n: isize, key_cb: *const c_void, value_cb: *const c_void) -> Ref;
    fn CFRelease(cf: Ref);
    static kCFBooleanTrue: Ref;
    // Structs of six and five pointers; only their addresses are used.
    static kCFTypeDictionaryKeyCallBacks: [usize; 6];
    static kCFTypeDictionaryValueCallBacks: [usize; 5];
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithURL(url: Ref, options: Ref) -> Ref;
    fn CGImageSourceCreateThumbnailAtIndex(source: Ref, index: usize, options: Ref) -> Ref;
    static kCGImageSourceCreateThumbnailFromImageAlways: Ref;
    static kCGImageSourceCreateThumbnailWithTransform: Ref;
    static kCGImageSourceThumbnailMaxPixelSize: Ref;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetWidth(image: Ref) -> usize;
    fn CGImageGetHeight(image: Ref) -> usize;
    fn CGImageRelease(image: Ref);
    fn CGColorSpaceCreateWithName(name: Ref) -> Ref;
    fn CGColorSpaceRelease(space: Ref);
    fn CGBitmapContextCreate(data: *mut c_void, w: usize, h: usize, bits: usize, row: usize, space: Ref, info: u32) -> *mut c_void;
    fn CGContextDrawImage(ctx: *mut c_void, rect: Rect, image: Ref);
    fn CGContextRelease(ctx: *mut c_void);
    static kCGColorSpaceSRGB: Ref;
}

/// `kCFNumberSInt64Type`.
const SINT64: isize = 4;
/// RGBA bytes in that order, alpha premultiplied: `kCGImageAlphaPremultipliedLast
/// | kCGBitmapByteOrder32Big`.
const RGBA: u32 = 1 | (4 << 12);

/// The picture at `path`, the right way up, no side longer than `max`.
pub fn decode(path: &Path, max: u32) -> Result<Decoded, String> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: Core Foundation and Core Graphics calls on objects made here,
    // each checked for null before use and released once; the pixels are
    // written into a buffer of exactly the size the context is told.
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 0);
        if url.is_null() {
            return Err("not found".into());
        }
        let source = CGImageSourceCreateWithURL(url, std::ptr::null());
        CFRelease(url);
        if source.is_null() {
            return Err(if path.exists() { "not a picture macOS can read".into() } else { "not found".into() });
        }
        let side = i64::from(max);
        let side = CFNumberCreate(std::ptr::null(), SINT64, (&raw const side).cast());
        let keys = [kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceCreateThumbnailWithTransform, kCGImageSourceThumbnailMaxPixelSize];
        let values = [kCFBooleanTrue, kCFBooleanTrue, side];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            (&raw const kCFTypeDictionaryKeyCallBacks).cast(),
            (&raw const kCFTypeDictionaryValueCallBacks).cast(),
        );
        CFRelease(side);
        let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options);
        CFRelease(options);
        CFRelease(source);
        if image.is_null() {
            return Err("not a picture macOS can read".into());
        }
        let (w, h) = (CGImageGetWidth(image), CGImageGetHeight(image));
        if w == 0 || h == 0 {
            CGImageRelease(image);
            return Err("an empty picture".into());
        }
        let mut rgba = vec![0u8; w * h * 4];
        let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        let ctx = CGBitmapContextCreate(rgba.as_mut_ptr().cast(), w, h, 8, w * 4, space, RGBA);
        CGColorSpaceRelease(space);
        if ctx.is_null() {
            CGImageRelease(image);
            return Err("couldn't draw the picture".into());
        }
        CGContextDrawImage(ctx, Rect { x: 0.0, y: 0.0, w: w as f64, h: h as f64 }, image);
        CGContextRelease(ctx);
        CGImageRelease(image);
        // Straight alpha, as kui takes it.
        for px in rgba.as_chunks_mut::<4>().0 {
            let a = u32::from(px[3]);
            if a != 0 && a != 255 {
                for c in &mut px[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Ok(Decoded { w: w as u32, h: h as u32, rgba, shown: None })
    }
}
