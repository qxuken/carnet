//! The pictures a note shows: each file decoded once, off the UI thread,
//! and handed to kui as RGBA. A view asks for a path every frame and gets
//! what there is so far — loading, ready, or why not.
//!
//! What is kept is kept within a budget. Each frame (`sweep`, before the
//! page is drawn) notes which pictures the last frame drew; a picture no
//! frame has drawn for `IDLE` — its note closed, scrolled past in another
//! — is let go, and so is the least recently drawn one not drawn for
//! `GRACE` while the pictures held come to more than `BUDGET`. Let go, a
//! picture's pixels leave kui (and the GPU) and only its size is kept: asked
//! for again, it is `Loading` at that size, holding its space, and is
//! decoded anew.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kui_native::{Core, ImageId, Waker};

/// The longest side a picture is kept at: past it, a phone photo would
/// cost a screen's worth of memory for pixels no page shows (the page is
/// at most 980px wide, twice that on a 2x screen).
const MAX_SIDE: u32 = 2000;

/// How much decoded pixels the pictures held may come to, in bytes (each
/// is `w × h × 4`, and kui keeps as much again on the GPU): some twenty
/// photos at `MAX_SIDE` — more than any one note shows, a few notes'
/// worth to flick between without a picture being read twice.
pub const BUDGET: u64 = 256 << 20;

/// Over budget, how long a picture goes undrawn before it may be let go:
/// long enough that one scrolled past and back, or a note glanced away
/// from, stays.
const GRACE: Duration = Duration::from_secs(5);

/// How long a picture goes undrawn before it is let go, budget or not: a
/// note closed this long is likely done with, and reading its pictures
/// again takes a moment, their space held meanwhile.
const IDLE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// Being decoded: its size, from the file's header (or from when it
    /// was shown before), when that could be read — so the space it will
    /// take is held from the start.
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

/// A picture and when it was last drawn.
struct Held {
    entry: Entry,
    /// The frame that last drew it.
    drawn: u64,
    /// When it was last known on screen: the start of the frame after
    /// the last one that drew it.
    seen: Instant,
}

impl Held {
    /// What its pixels cost, in bytes; nothing until they are in hand.
    fn bytes(&self) -> u64 {
        match self.entry {
            Entry::Ready { w, h, .. } => u64::from(w) * u64::from(h) * 4,
            _ => 0,
        }
    }
}

pub struct Images {
    entries: HashMap<PathBuf, Held>,
    /// The size of each picture let go, for when it is asked for again.
    sizes: HashMap<PathBuf, (u32, u32)>,
    /// The frame being drawn, and when it began.
    frame: u64,
    at: Instant,
    budget: u64,
    /// Pixels replaced before they were let go, freed at the next pump.
    stale: Vec<ImageId>,
    inbox: Inbox,
    /// Wakes the loop when a decode lands; none in tests, which decode in
    /// place instead.
    waker: Option<Waker>,
}

impl Default for Images {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            sizes: HashMap::new(),
            frame: 0,
            at: Instant::now(),
            budget: BUDGET,
            stale: Vec::new(),
            inbox: Inbox::default(),
            waker: None,
        }
    }
}

impl Images {
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// What there is of the picture at `path`, starting its decode the
    /// first time it is asked for (or the first time since it was let
    /// go). Asking is drawing: a picture asked for this frame is kept.
    pub fn get(&mut self, path: &Path) -> Entry {
        if let Some(held) = self.entries.get_mut(path) {
            held.drawn = self.frame;
            return held.entry.clone();
        }
        // On another computer: never looked at (`Store::resolve` gives
        // none such; this makes sure).
        if crate::store::on_network(path) {
            let failed = Entry::Failed("not found".into());
            self.hold(path, failed.clone());
            return failed;
        }
        // Its size as last shown, or the header alone: a few bytes, read now.
        let (w, h) = self
            .sizes
            .remove(path)
            .unwrap_or_else(|| image::image_dimensions(path).unwrap_or((0, 0)));
        let loading = Entry::Loading { w, h };
        self.hold(path, loading.clone());
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

    /// Whether the picture at `path` was decoded, and is shown: a
    /// picture, whatever its file is named.
    pub fn is_ready(&self, path: &Path) -> bool {
        self.entries.get(path).is_some_and(|h| matches!(h.entry, Entry::Ready { .. }))
    }

    fn hold(&mut self, path: &Path, entry: Entry) {
        let held = Held { entry, drawn: self.frame, seen: self.at };
        self.entries.insert(path.to_path_buf(), held);
    }

    /// A picture whose pixels are in hand already (it was just pasted):
    /// shown on the next frame, never read back from its file.
    pub fn preload(&mut self, path: &Path, d: Decoded) {
        self.sizes.remove(path);
        // Pixels shown under that name before: they go when these land.
        if let Some(Held { entry: Entry::Ready { id, .. }, .. }) = self.entries.remove(path) {
            self.stale.push(id);
        }
        self.hold(path, Entry::Loading { w: d.w, h: d.h });
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
        for id in self.stale.drain(..) {
            core.remove_image(id);
        }
        for (path, r) in done {
            let e = match r {
                Ok(d) => Entry::Ready {
                    id: core.resources.add_image(d.w, d.h, d.rgba),
                    w: d.w,
                    h: d.h,
                },
                Err(e) => Entry::Failed(e),
            };
            match self.entries.get_mut(&path) {
                Some(held) => {
                    if let Entry::Ready { id, .. } = held.entry {
                        core.remove_image(id);
                    }
                    held.entry = e;
                }
                None => self.hold(&path, e),
            }
        }
        landed
    }

    /// Called as each frame begins, before anything is drawn: notes what
    /// the last frame drew, and lets go of what has gone undrawn long
    /// enough (see the module's words). Says how long until a picture
    /// could next be let go, for a frame then: with nothing else to draw
    /// the loop sleeps, and a closed note's pictures are let go all the
    /// same.
    pub fn sweep(&mut self, core: &mut Core, now: Instant) -> Option<Duration> {
        let last = self.frame;
        for held in self.entries.values_mut() {
            if held.drawn == last {
                held.seen = now;
            }
        }
        self.frame += 1;
        self.at = now;
        let undrawn = |held: &Held, wait: Duration| {
            held.bytes() > 0 && held.drawn < last && now.saturating_duration_since(held.seen) >= wait
        };
        // Long undrawn: let go.
        let idle: Vec<PathBuf> = self
            .entries
            .iter()
            .filter(|(_, h)| undrawn(h, IDLE))
            .map(|(p, _)| p.clone())
            .collect();
        for path in idle {
            self.let_go(core, &path);
        }
        // Over budget: the least recently drawn first, of those undrawn a
        // while.
        let mut total = self.bytes();
        if total > self.budget {
            let mut old: Vec<(u64, Instant, PathBuf)> = self
                .entries
                .iter()
                .filter(|(_, h)| undrawn(h, GRACE))
                .map(|(p, h)| (h.drawn, h.seen, p.clone()))
                .collect();
            old.sort();
            for (_, _, path) in old {
                if total <= self.budget {
                    break;
                }
                total -= self.entries.get(&path).map_or(0, Held::bytes);
                self.let_go(core, &path);
            }
        }
        // When the next could go: one on screen now at `IDLE` from now,
        // should this frame be the last to draw it.
        let wait = if total > self.budget { GRACE } else { IDLE };
        self.entries
            .values()
            .filter(|h| h.bytes() > 0)
            .map(|h| (h.seen + wait).saturating_duration_since(now).max(Duration::from_millis(100)))
            .min()
    }

    /// Forgets a picture's pixels, keeping its size for when it is next
    /// asked for.
    fn let_go(&mut self, core: &mut Core, path: &Path) {
        if let Some(held) = self.entries.remove(path)
            && let Entry::Ready { id, w, h } = held.entry
        {
            core.remove_image(id);
            self.sizes.insert(path.to_path_buf(), (w, h));
        }
    }

    /// What the pictures held come to, in bytes of pixels.
    pub fn bytes(&self) -> u64 {
        self.entries.values().map(Held::bytes).sum()
    }

    /// Forgets the pictures that could not be read, so they are read again
    /// when next shown.
    pub fn retry_failed(&mut self) {
        self.entries.retain(|_, h| !matches!(h.entry, Entry::Failed(_)));
    }

    /// How many pictures are being read, shown, could not be read, and
    /// were let go.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut n = (0, 0, 0, self.sizes.len());
        for h in self.entries.values() {
            match h.entry {
                Entry::Loading { .. } => n.0 += 1,
                Entry::Ready { .. } => n.1 += 1,
                Entry::Failed(_) => n.2 += 1,
            }
        }
        n
    }

    /// Forgets every picture: the notes folder changed.
    pub fn clear(&mut self, core: &mut Core) {
        self.sizes.clear();
        for id in self.stale.drain(..) {
            core.remove_image(id);
        }
        for (_, h) in self.entries.drain() {
            if let Entry::Ready { id, .. } = h.entry {
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

    /// `n` pictures of 100 × 50 in a folder of their own.
    fn pictures(name: &str, n: usize) -> Vec<PathBuf> {
        let dir = std::env::temp_dir().join(format!("carnet-images-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (0..n)
            .map(|i| {
                let p = dir.join(format!("{i}.png"));
                let rgba = vec![i as u8; 100 * 50 * 4];
                std::fs::write(&p, encode_png(100, 50, &rgba).unwrap()).unwrap();
                p
            })
            .collect()
    }

    const ONE: u64 = 100 * 50 * 4;

    /// One frame: the sweep, the decodes handed over, then `drawn` drawn.
    fn frame(images: &mut Images, core: &mut Core, now: Instant, drawn: &[PathBuf]) -> Vec<Entry> {
        images.sweep(core, now);
        images.pump(core);
        drawn.iter().map(|p| images.get(p)).collect()
    }

    fn live(core: &Core, e: &Entry) -> bool {
        matches!(e, Entry::Ready { id, .. } if core.resources.image_size(*id).is_some())
    }

    #[test]
    fn drawing_many_pictures_over_budget_lets_old_ones_go_and_they_come_back_at_their_size() {
        let paths = pictures("budget", 6);
        let mut core = Core::new();
        let mut images = Images { budget: 3 * ONE, ..Images::default() };
        let t0 = Instant::now();
        // A note with the first four: read, then shown — over budget, but
        // all on screen, so all kept.
        frame(&mut images, &mut core, t0, &paths[..4]);
        let shown = frame(&mut images, &mut core, t0, &paths[..4]);
        assert!(shown.iter().all(|e| live(&core, e)), "{shown:?}");
        assert_eq!(images.bytes(), 4 * ONE);
        // Another with the last two; the first four undrawn, but only
        // just: kept a while yet.
        let at = |s: u64| t0 + Duration::from_secs(s);
        frame(&mut images, &mut core, at(1), &paths[4..]);
        frame(&mut images, &mut core, at(2), &paths[4..]);
        assert_eq!(images.bytes(), 6 * ONE);
        // Past the grace, the least recently drawn go until it fits: the
        // second note's two stay, and one of the first's.
        let wait = images.sweep(&mut core, at(1) + GRACE);
        images.get(&paths[4]);
        images.get(&paths[5]);
        assert_eq!(images.bytes(), 3 * ONE);
        assert!(shown.iter().filter(|e| live(&core, e)).count() == 1, "kui let go of the pixels too");
        assert_eq!(images.counts(), (0, 3, 0, 3));
        assert!(wait.is_some_and(|w| w <= IDLE), "a frame asked for when the next could go: {wait:?}");
        // The first asked for again: its space held at once, then shown.
        let gone = paths[..4].iter().find(|p| !images.entries.contains_key(*p)).unwrap();
        assert_eq!(images.get(gone), Entry::Loading { w: 100, h: 50 });
        images.pump(&mut core);
        assert!(live(&core, &images.get(gone)));
    }

    #[test]
    fn a_closed_notes_pictures_are_let_go_after_a_while() {
        let paths = pictures("idle", 2);
        let mut core = Core::new();
        let mut images = Images::default();
        let t0 = Instant::now();
        frame(&mut images, &mut core, t0, &paths);
        let first = frame(&mut images, &mut core, t0, &paths);
        // The note with both stays open, untouched: no frame for a long
        // while, then one — what was on screen stays.
        let later = t0 + IDLE * 3;
        frame(&mut images, &mut core, later, &paths[1..]);
        assert!(live(&core, &first[0]) && live(&core, &first[1]));
        // The first undrawn since: let go once `IDLE` has passed, under
        // budget as it is.
        frame(&mut images, &mut core, later + IDLE - Duration::from_secs(1), &paths[1..]);
        assert!(live(&core, &first[0]));
        frame(&mut images, &mut core, later + IDLE, &paths[1..]);
        assert!(!live(&core, &first[0]) && live(&core, &first[1]));
        assert_eq!(images.get(&paths[0]), Entry::Loading { w: 100, h: 50 });
    }

    #[test]
    fn a_heic_photo_is_a_picture_to_keep() {
        assert!(is_image(Path::new("IMG_0001.HEIC")) && is_heic(Path::new("a.heif")) && !is_heic(Path::new("a.png")));
    }
}
