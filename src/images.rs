//! The pictures a note shows: each file decoded once, off the UI thread,
//! and handed to kui as RGBA. A view asks for a path every frame and gets
//! what there is so far — loading, ready, or why not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kui_native::{Core, ImageId, Waker};

/// The longest side a picture is kept at: past it, a phone photo would
/// cost a screen's worth of memory for pixels no page shows (the page is
/// at most 980px wide, twice that on a 2x screen).
const MAX_SIDE: u32 = 2000;

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// Being decoded: its size, from the file's header, when that could
    /// be read — so the space it will take is held from the start.
    Loading { w: u32, h: u32 },
    Ready { id: ImageId, w: u32, h: u32 },
    Failed(String),
}

pub struct Decoded {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

type Inbox = Arc<Mutex<Vec<(PathBuf, Result<Decoded, String>)>>>;

#[derive(Default)]
pub struct Images {
    entries: HashMap<PathBuf, Entry>,
    inbox: Inbox,
    /// Wakes the loop when a decode lands; none in tests, which decode in
    /// place instead.
    waker: Option<Waker>,
}

impl Images {
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// What there is of the picture at `path`, starting its decode the
    /// first time it is asked for.
    pub fn get(&mut self, path: &Path) -> Entry {
        if let Some(e) = self.entries.get(path) {
            return e.clone();
        }
        // The header alone: a few bytes, read now.
        let (w, h) = image::image_dimensions(path).unwrap_or((0, 0));
        let loading = Entry::Loading { w, h };
        self.entries.insert(path.to_path_buf(), loading.clone());
        let (inbox, path) = (self.inbox.clone(), path.to_path_buf());
        match self.waker.clone() {
            Some(waker) => {
                std::thread::spawn(move || {
                    let r = decode(&path);
                    if let Ok(mut q) = inbox.lock() {
                        q.push((path, r));
                    }
                    waker.wake();
                });
            }
            None => {
                let r = decode(&path);
                if let Ok(mut q) = inbox.lock() {
                    q.push((path, r));
                }
            }
        }
        loading
    }

    /// A picture whose pixels are in hand already (it was just pasted):
    /// shown on the next frame, never read back from its file.
    pub fn preload(&mut self, path: &Path, d: Decoded) {
        self.entries.insert(path.to_path_buf(), Entry::Loading { w: d.w, h: d.h });
        if let Ok(mut q) = self.inbox.lock() {
            q.push((path.to_path_buf(), Ok(fit(d))));
        }
    }

    /// Hands finished decodes to kui. True when one landed (the page
    /// changes height).
    pub fn pump(&mut self, core: &mut Core) -> bool {
        let done: Vec<_> = match self.inbox.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(_) => return false,
        };
        let landed = !done.is_empty();
        for (path, r) in done {
            let e = match r {
                Ok(d) => Entry::Ready {
                    id: core.resources.add_image(d.w, d.h, d.rgba),
                    w: d.w,
                    h: d.h,
                },
                Err(e) => Entry::Failed(e),
            };
            self.entries.insert(path, e);
        }
        landed
    }


    /// Forgets the pictures that could not be read, so they are read again
    /// when next shown.
    pub fn retry_failed(&mut self) {
        self.entries.retain(|_, e| !matches!(e, Entry::Failed(_)));
    }

    /// How many pictures are being read, shown, and could not be read.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut n = (0, 0, 0);
        for e in self.entries.values() {
            match e {
                Entry::Loading { .. } => n.0 += 1,
                Entry::Ready { .. } => n.1 += 1,
                Entry::Failed(_) => n.2 += 1,
            }
        }
        n
    }

    /// Forgets every picture: the notes folder changed.
    pub fn clear(&mut self, core: &mut Core) {
        for (_, e) in self.entries.drain() {
            if let Entry::Ready { id, .. } = e {
                core.remove_image(id);
            }
        }
    }
}

/// Why a HEIC picture isn't shown, where only macOS reads them.
pub const HEIC_ELSEWHERE: &str = "HEIC pictures show on macOS";

/// A picture file as RGBA, no side longer than `MAX_SIDE`. What the
/// `image` crate can't read, macOS may: an iPhone's HEIC.
pub fn decode(path: &Path) -> Result<Decoded, String> {
    let img = match image::open(path) {
        Ok(img) => img,
        Err(image::ImageError::IoError(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("not found".to_string());
        }
        #[cfg(target_os = "macos")]
        Err(image::ImageError::Unsupported(_)) => return crate::imageio::decode(path, MAX_SIDE),
        #[cfg(not(target_os = "macos"))]
        Err(image::ImageError::Unsupported(_)) if is_heic(path) => return Err(HEIC_ELSEWHERE.to_string()),
        Err(e) => return Err(e.to_string()),
    };
    // `thumbnail` is the fast way down: a box filter in integer steps,
    // many times quicker than a resampling filter on a camera's photo.
    let img = if img.width().max(img.height()) > MAX_SIDE {
        img.thumbnail(MAX_SIDE, MAX_SIDE)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(Decoded {
        w: rgba.width(),
        h: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// Pixels in hand, no side longer than `MAX_SIDE`.
fn fit(d: Decoded) -> Decoded {
    if d.w.max(d.h) <= MAX_SIDE {
        return d;
    }
    match image::RgbaImage::from_raw(d.w, d.h, d.rgba) {
        Some(img) => {
            let small = image::DynamicImage::ImageRgba8(img).thumbnail(MAX_SIDE, MAX_SIDE).to_rgba8();
            Decoded {
                w: small.width(),
                h: small.height(),
                rgba: small.into_raw(),
            }
        }
        None => Decoded { w: 0, h: 0, rgba: Vec::new() },
    }
}

/// RGBA as a PNG file's bytes.
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut out,
        rgba,
        w,
        h,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// Whether a file is a picture Carnet keeps in a note, by its extension.
pub fn is_image(path: &Path) -> bool {
    is_heic(path)
        || path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
                )
            })
}

/// Whether a file is an iPhone's kind of photo, HEIC, by its extension.
pub fn is_heic(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("heic") || e.eq_ignore_ascii_case("heif"))
}

/// A photo's long side at most, in pixels, as the web app takes one:
/// sharp across a wide window on a high-density screen.
#[cfg(target_os = "macos")]
pub const PHOTO: u32 = 2560;

/// A HEIC photo as a JPEG every device and browser shows: the right way
/// up, at most `PHOTO` pixels on its long side, and with nothing the
/// camera wrote in it, such as where it was taken (only the pixels are
/// kept). What the web app does with a photo, done here as it comes in.
#[cfg(target_os = "macos")]
pub fn heic_to_jpeg(path: &Path) -> Result<Vec<u8>, String> {
    let d = crate::imageio::decode(path, PHOTO)?;
    let rgb: Vec<u8> = d.rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88)
        .encode(&rgb, d.w, d.h, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small HEIC photo, as an iPhone writes them: its left half red, its
    /// right half blue.
    fn heic() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fixtures/halves.heic")
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reads_a_heic_photo_and_makes_it_a_plain_jpeg() {
        let d = decode(&heic()).unwrap();
        assert_eq!((d.w, d.h), (48, 32));
        let px = |x: u32, y: u32| &d.rgba[((y * d.w + x) * 4) as usize..][..4];
        let (left, right) = (px(4, 16), px(43, 16));
        assert!(left[0] > 180 && left[2] < 90 && left[3] == 255, "red on the left, the right way round: {left:?}");
        assert!(right[2] > 180 && right[0] < 90, "blue on the right: {right:?}");
        let jpeg = heic_to_jpeg(&heic()).unwrap();
        let back = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (48, 32));
        assert!(!jpeg.windows(4).any(|w| w == b"Exif"), "nothing the camera wrote in it");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn elsewhere_a_heic_photo_says_where_it_shows() {
        assert_eq!(decode(&heic()).err().as_deref(), Some(HEIC_ELSEWHERE));
    }

    #[test]
    fn a_heic_photo_is_a_picture_to_keep() {
        assert!(is_image(Path::new("IMG_0001.HEIC")) && is_heic(Path::new("a.heif")) && !is_heic(Path::new("a.png")));
    }
}
