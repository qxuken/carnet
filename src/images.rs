//! The pictures a note shows: each file decoded once, off the UI thread,
//! and handed to kui as RGBA. A view asks for a path every frame and gets
//! what there is so far — loading, ready, or why not.
//!
//! A picture is read by one of a few threads ([`READERS`]), not a thread
//! each: a note of a hundred photos reads them a few at a time. Each is
//! read within limits ([`MAX_PIXELS_SIDE`], [`MAX_ALLOC`]): a file that
//! says it is bigger, as one made to be could, is a card saying it's too
//! big, not the app's memory gone. Only HEIC, TIFF and AVIF, by their
//! names, go to macOS's ImageIO; anything else is read as its name says
//! or not at all, never guessed at from its bytes.
//!
//! What is kept is kept within a budget. Each frame (`sweep`, before the
//! page is drawn) notes which pictures the last frame drew; a picture no
//! frame has drawn for `IDLE` — its note closed, scrolled past in another
//! — is let go, and so is the least recently drawn one not drawn for
//! `GRACE` while the pictures held come to more than `BUDGET`. Let go, a
//! picture's pixels leave kui (and the GPU) and only its size is kept: asked
//! for again, it is `Loading` at that size, holding its space, and is
//! decoded anew.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use kui_native::{Core, ImageId, Waker};

/// The longest side a picture is kept at: past it, a phone photo would
/// cost a screen's worth of memory for pixels no page shows (the page is
/// at most 980px wide, twice that on a 2x screen).
const MAX_SIDE: u32 = 2000;

/// The widest or tallest a picture file may say it is, to be read at all.
const MAX_PIXELS_SIDE: u32 = 16384;

/// The most memory reading one picture may take, in bytes.
const MAX_ALLOC: u64 = 256 << 20;

/// How many pictures are read at once, each on a thread of its own.
const READERS: usize = 3;

/// Why a picture isn't shown: it's past what Carnet reads.
pub const TOO_BIG: &str = "too big to show";

/// Why a picture isn't shown: no reader for its kind.
pub const UNSUPPORTED: &str = "this kind of picture is not supported";

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

/// A picture not found is looked for again this long after, with nothing
/// else to say it may be there now (a file come by sync or put back by
/// hand: the store says so at once); then twice as long each time, every
/// 10 minutes at most, while any is not found.
pub const MISSING_FIRST: Duration = Duration::from_secs(30);
pub const MISSING_MOST: Duration = Duration::from_secs(600);

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
    /// The size it is laid out at, when it has more pixels than that: an
    /// SVG drawn sharper than its own size, to stay sharp on a dense
    /// screen and widened. `None` for a picture's own pixels.
    pub shown: Option<(u32, u32)>,
}

/// What a reader found of a picture: its size, from the file's header,
/// then its pixels, or why not, and how long reading it took in ms (none
/// for pixels already in hand): a recording's `decode picture` track.
enum Landed {
    Size(u32, u32),
    Done(Result<Decoded, String>, Option<f64>),
}

type Inbox = Arc<Mutex<Vec<(PathBuf, Landed)>>>;

/// Pictures waiting for a reader.
type Queue = Arc<(Mutex<VecDeque<PathBuf>>, Condvar)>;

/// A picture and when it was last drawn.
struct Held {
    entry: Entry,
    /// The frame that last drew it.
    drawn: u64,
    /// When it was last known on screen: the start of the frame after
    /// the last one that drew it.
    seen: Instant,
    /// What its pixels cost, in bytes, once in hand: a drawing has more
    /// of them than the size it's laid out at says.
    pixels: u64,
}

impl Held {
    /// What its pixels cost, in bytes; nothing until they are in hand.
    fn bytes(&self) -> u64 {
        match self.entry {
            Entry::Ready { .. } => self.pixels,
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
    /// The readers' queue, once they are started.
    queue: Option<Queue>,
    /// When the pictures not found are looked for again, and the wait
    /// after that ([`MISSING_FIRST`]); none while none is missing.
    missing: Option<(Instant, Duration)>,
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
            queue: None,
            missing: None,
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
        // Its size as last shown; or, from the file's header, as soon as a
        // reader has it (in tests, now).
        let known = self.sizes.remove(path);
        let Some(waker) = self.waker.clone() else {
            let (w, h) = known.unwrap_or_else(|| size(path));
            let loading = Entry::Loading { w, h };
            self.hold(path, loading.clone());
            let began = Instant::now();
            let r = decode(path);
            let took = began.elapsed().as_secs_f64() * 1000.0;
            if let Ok(mut q) = self.inbox.lock() {
                q.push((path.to_path_buf(), Landed::Done(r, Some(took))));
            }
            return loading;
        };
        let (w, h) = known.unwrap_or((0, 0));
        let loading = Entry::Loading { w, h };
        self.hold(path, loading.clone());
        let inbox = self.inbox.clone();
        let queue = self.queue.get_or_insert_with(|| readers(inbox, waker));
        let (jobs, ready) = &**queue;
        if let Ok(mut q) = jobs.lock() {
            q.push_back(path.to_path_buf());
            ready.notify_one();
        }
        loading
    }

    /// Whether the picture at `path` was decoded, and is shown: a
    /// picture, whatever its file is named.
    pub fn is_ready(&self, path: &Path) -> bool {
        self.entries.get(path).is_some_and(|h| matches!(h.entry, Entry::Ready { .. }))
    }

    fn hold(&mut self, path: &Path, entry: Entry) {
        self.hold_with(path, entry, 0);
    }

    fn hold_with(&mut self, path: &Path, entry: Entry, pixels: u64) {
        let held = Held { entry, drawn: self.frame, seen: self.at, pixels };
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
            q.push((path.to_path_buf(), Landed::Done(Ok(fit(d)), None)));
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
        for (path, landed) in done {
            let r = match landed {
                // The size, before the pixels: the space held.
                Landed::Size(w, h) => {
                    if let Some(held) = self.entries.get_mut(&path)
                        && matches!(held.entry, Entry::Loading { w: 0, h: 0 })
                    {
                        held.entry = Entry::Loading { w, h };
                    }
                    continue;
                }
                Landed::Done(r, took) => {
                    if let Some(ms) = took {
                        core.perf_measure("decode picture", ms);
                    }
                    r
                }
            };
            let (e, pixels) = match r {
                Ok(d) => {
                    let (w, h) = d.shown.unwrap_or((d.w, d.h));
                    let pixels = u64::from(d.w) * u64::from(d.h) * 4;
                    (Entry::Ready { id: core.resources.add_image(d.w, d.h, d.rgba), w, h }, pixels)
                }
                Err(e) => (Entry::Failed(e), 0),
            };
            match self.entries.get_mut(&path) {
                Some(held) => {
                    if let Entry::Ready { id, .. } = held.entry {
                        core.remove_image(id);
                    }
                    held.entry = e;
                    held.pixels = pixels;
                }
                None => self.hold_with(&path, e, pixels),
            }
        }
        if landed {
            core.perf_gauge("pictures held (bytes)", self.bytes() as f64);
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
        let go = self
            .entries
            .values()
            .filter(|h| h.bytes() > 0)
            .map(|h| (h.seen + wait).saturating_duration_since(now).max(Duration::from_millis(100)))
            .min();
        let again = self.look_again(now);
        go.into_iter().chain(again).min()
    }

    /// Pictures not found, looked for again once their wait is up (they
    /// are read anew when next drawn); how long until the next time, while
    /// any is missing.
    fn look_again(&mut self, now: Instant) -> Option<Duration> {
        let missing = |h: &Held| matches!(&h.entry, Entry::Failed(e) if e == "not found");
        if !self.entries.values().any(missing) {
            self.missing = None;
            return None;
        }
        let (at, wait) = *self.missing.get_or_insert((now + MISSING_FIRST, MISSING_FIRST));
        if now < at {
            return Some(at - now);
        }
        self.entries.retain(|_, h| !missing(h));
        let wait = (wait * 2).min(MISSING_MOST);
        self.missing = Some((now + wait, wait));
        Some(wait)
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

/// The readers: a few threads taking pictures from the queue, each
/// saying the picture's size as soon as its header is read, then its
/// pixels.
fn readers(inbox: Inbox, waker: Waker) -> Queue {
    let queue: Queue = Arc::default();
    for i in 0..READERS {
        let (queue, inbox, waker) = (queue.clone(), inbox.clone(), waker.clone());
        let read = move || {
            loop {
                let path = {
                    let (jobs, ready) = &*queue;
                    let Ok(mut q) = jobs.lock() else { return };
                    loop {
                        if let Some(p) = q.pop_front() {
                            break p;
                        }
                        let Ok(next) = ready.wait(q) else { return };
                        q = next;
                    }
                };
                let land = |landed: Landed| {
                    if let Ok(mut q) = inbox.lock() {
                        q.push((path.clone(), landed));
                    }
                    waker.wake();
                };
                let (w, h) = size(&path);
                if w > 0 && h > 0 {
                    land(Landed::Size(w, h));
                }
                let began = Instant::now();
                let r = decode(&path);
                land(Landed::Done(r, Some(began.elapsed().as_secs_f64() * 1000.0)));
            }
        };
        let _ = std::thread::Builder::new().name(format!("carnet-picture-{i}")).spawn(read);
    }
    queue
}

/// A picture's size from its file's header alone, or nothing; past what
/// is read at all, nothing (the picture then says why).
fn size(path: &Path) -> (u32, u32) {
    if for_imageio(path) {
        return (0, 0);
    }
    if is_svg(path) {
        return svg_tree(path, false).ok().map_or((0, 0), |t| natural(&t));
    }
    let fits = |(w, h): (u32, u32)| w <= MAX_PIXELS_SIDE && h <= MAX_PIXELS_SIDE && u64::from(w) * u64::from(h) * 4 <= MAX_ALLOC;
    image::image_dimensions(path).ok().filter(|&wh| fits(wh)).unwrap_or((0, 0))
}

/// Why a HEIC picture isn't shown, where only macOS reads them.
pub const HEIC_ELSEWHERE: &str = "HEIC pictures show on macOS";

/// Whether a picture is read by macOS's ImageIO, not the `image` crate:
/// by its name, HEIC (an iPhone's photos), TIFF or AVIF. Nothing else is
/// handed to it, so a file is never read as a kind its name doesn't say.
fn for_imageio(path: &Path) -> bool {
    is_heic(path) || path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "tif" | "tiff" | "avif"))
}

/// A picture file as RGBA, no side longer than `MAX_SIDE`, read within
/// limits: one past them is [`TOO_BIG`]. HEIC, TIFF and AVIF are macOS's
/// to read.
pub fn decode(path: &Path) -> Result<Decoded, String> {
    // A drawing: drawn twice its own size, so a dense screen and a wider
    // picture stay sharp, and no smaller than `SVG_SIDE_LEAST` (an icon
    // 24px across is widened as much as a photo), within `MAX_SIDE`;
    // laid out at its own size.
    if is_svg(path) {
        let tree = svg_tree(path, true)?;
        let (w, h) = natural(&tree);
        let long = w.max(h) as f32;
        let side = (2.0 * long).max(SVG_SIDE_LEAST as f32).min(MAX_SIDE as f32);
        let mut d = draw(&tree, side / long)?;
        d.shown = Some((w, h));
        return Ok(d);
    }
    if for_imageio(path) {
        #[cfg(target_os = "macos")]
        return crate::imageio::decode(path, MAX_SIDE);
        #[cfg(not(target_os = "macos"))]
        return Err(if is_heic(path) { HEIC_ELSEWHERE } else { UNSUPPORTED }.to_string());
    }
    let mut reader = match image::ImageReader::open(path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("not found".to_string()),
        Err(e) => return Err(e.to_string()),
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PIXELS_SIDE);
    limits.max_image_height = Some(MAX_PIXELS_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let img = match reader.decode() {
        Ok(img) => img,
        Err(image::ImageError::Limits(_)) => return Err(TOO_BIG.to_string()),
        Err(image::ImageError::Unsupported(_)) => return Err(UNSUPPORTED.to_string()),
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
        shown: None,
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
                shown: d.shown,
            }
        }
        None => Decoded { w: 0, h: 0, rgba: Vec::new(), shown: None },
    }
}

/// A picture written again at `to` as a PNG no wider than `most_w` and no
/// taller than `most_h`, its shape and its transparency kept: a divider's
/// picture is drawn a few lines tall, and a camera's photo kept whole for
/// it took a second to decode and a frame to upload. Its size, as written.
pub fn shrink_to_png(from: &Path, to: &Path, most_w: u32, most_h: u32) -> Result<(u32, u32), String> {
    let d = if is_svg(from) { draw_svg(from, most_w, most_h)? } else { decode(from)? };
    let img = image::RgbaImage::from_raw(d.w, d.h, d.rgba).ok_or_else(|| "no pixels".to_string())?;
    let img = image::DynamicImage::ImageRgba8(img);
    let img = if img.width() > most_w || img.height() > most_h { img.thumbnail(most_w, most_h) } else { img };
    let rgba = img.to_rgba8();
    let png = encode_png(rgba.width(), rgba.height(), rgba.as_raw())?;
    std::fs::write(to, png).map_err(|e| e.to_string())?;
    Ok((rgba.width(), rgba.height()))
}

/// Whether a file is an SVG drawing (or a gzipped one), by its extension:
/// a note's picture may be (`.svg`, as carnet-core counts one), and a
/// divider's, either.
pub fn is_svg(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "svg" | "svgz"))
}

/// The fewest pixels a note's drawing is drawn at along its longer side,
/// however small it says it is.
const SVG_SIDE_LEAST: u32 = 1200;

/// An SVG read, with nothing run (an SVG's scripts are never read): its
/// text in the system's fonts, when it's to be drawn; just its size, not.
fn svg_tree(path: &Path, fonts: bool) -> Result<resvg::usvg::Tree, String> {
    use resvg::usvg;
    let data = std::fs::read(path).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "not found".to_string() } else { e.to_string() })?;
    let mut opt = usvg::Options::default();
    if fonts {
        opt.fontdb_mut().load_system_fonts();
    }
    let tree = usvg::Tree::from_data(&data, &opt).map_err(|_| "it isn't a drawing Carnet can read".to_string())?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return Err("it draws nothing".to_string());
    }
    Ok(tree)
}

/// A drawing's own size, in whole pixels.
fn natural(tree: &resvg::usvg::Tree) -> (u32, u32) {
    let s = tree.size();
    ((s.width().round() as u32).max(1), (s.height().round() as u32).max(1))
}

/// An SVG drawn as RGBA as large as fits `most_w` × `most_h`, its shape
/// kept: a drawing has no size of its own to be shrunk from, so it's drawn
/// at the size it will be kept at, never blurred up from a small one. Its
/// text in the system's fonts.
fn draw_svg(path: &Path, most_w: u32, most_h: u32) -> Result<Decoded, String> {
    let tree = svg_tree(path, true)?;
    let size = tree.size();
    draw(&tree, (most_w as f32 / size.width()).min(most_h as f32 / size.height()))
}

/// A drawing drawn at `k` times its own size, as RGBA.
fn draw(tree: &resvg::usvg::Tree, k: f32) -> Result<Decoded, String> {
    use resvg::tiny_skia;
    let size = tree.size();
    let (w, h) = (((size.width() * k).round() as u32).max(1), ((size.height() * k).round() as u32).max(1));
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| "too big to draw".to_string())?;
    resvg::render(tree, tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    // tiny-skia's pixels are premultiplied; a PNG's aren't.
    let mut rgba = pixmap.take();
    for p in rgba.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Ok(Decoded { w, h, rgba, shown: None })
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
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"
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
pub(crate) mod tests {
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

    /// A drawing in a note: its size from the file before any pixels,
    /// drawn sharper than that (twice, and 1200 at least along its long
    /// side), laid out at its own size, and its pixels counted as they are
    /// against the budget, not as its size says.
    #[test]
    fn a_drawing_is_laid_out_at_its_size_and_drawn_sharper() {
        let dir = std::env::temp_dir().join(format!("carnet-images-svg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("logo.svg");
        std::fs::write(
            &path,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="100"><rect width="200" height="100" fill="#336699"/><script>alert(1)</script></svg>"##,
        )
        .unwrap();
        assert!(is_image(&path) && !is_image(&dir.join("logo.svgz")), "a note's picture; a gzipped one is a divider's only");
        assert_eq!(size(&path), (400, 100));
        let d = decode(&path).unwrap();
        assert_eq!(((d.w, d.h), d.shown), ((1200, 300), Some((400, 100))));
        let px = |x: u32, y: u32| &d.rgba[((y * d.w + x) * 4) as usize..][..4];
        assert_eq!(px(300, 150), [0x33, 0x66, 0x99, 255], "the drawn half");
        assert_eq!(px(900, 150)[3], 0, "the rest see-through");
        // Big ones within MAX_SIDE; laid out as they say.
        std::fs::write(&path, r#"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="1000"/>"#).unwrap();
        let d = decode(&path).unwrap();
        assert_eq!(((d.w, d.h), d.shown), ((2000, 400), Some((5000, 1000))));
        assert_eq!(decode(&dir.join("gone.svg")).err().as_deref(), Some("not found"));
        std::fs::write(&path, "not a drawing").unwrap();
        assert_eq!(decode(&path).err().as_deref(), Some("it isn't a drawing Carnet can read"));
        // In the cache: laid out at 400 × 100, counted at 1200 × 300.
        std::fs::write(&path, r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="100"><rect width="400" height="100" fill="#000"/></svg>"##).unwrap();
        let mut core = Core::new();
        let mut images = Images::default();
        let t0 = Instant::now();
        frame(&mut images, &mut core, t0, std::slice::from_ref(&path));
        let e = frame(&mut images, &mut core, t0, std::slice::from_ref(&path));
        assert!(matches!(e[0], Entry::Ready { w: 400, h: 100, .. }), "{e:?}");
        assert_eq!(images.bytes(), 1200 * 300 * 4);
        let _ = std::fs::remove_dir_all(dir);
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

    /// A PNG's signature and header, and no pixels, saying it is `w × h`:
    /// what a file made to exhaust a reader looks like.
    /// The finding: a picture not found was looked for again only when
    /// the notes changed. Now it's looked for again a while on, the wait
    /// doubling to 10 minutes, and shown once its file is there.
    #[test]
    fn a_picture_not_found_is_looked_for_again_now_and_then() {
        let dir = std::env::temp_dir().join(format!("carnet-images-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("late.png");
        let mut images = Images::default();
        let mut core = Core::new();
        let t0 = Instant::now();
        let look = |images: &mut Images, core: &mut Core, at: Instant| {
            let wait = images.sweep(core, at);
            let e = images.get(&path);
            images.pump(core);
            (wait, if matches!(e, Entry::Loading { .. }) { images.get(&path) } else { e })
        };
        let (_, e) = look(&mut images, &mut core, t0);
        assert_eq!(e, Entry::Failed("not found".into()));
        // The next frame asks for one when it's to be looked for.
        let (wait, _) = look(&mut images, &mut core, t0);
        assert_eq!(wait, Some(MISSING_FIRST));
        std::fs::write(&path, encode_png(3, 2, &[90u8; 24]).unwrap()).unwrap();
        // Not before.
        let (_, e) = look(&mut images, &mut core, t0 + MISSING_FIRST / 2);
        assert_eq!(e, Entry::Failed("not found".into()));
        // Then: read anew, and shown.
        let (wait, e) = look(&mut images, &mut core, t0 + MISSING_FIRST);
        assert_eq!(wait, Some(MISSING_FIRST * 2), "twice as long, the next time");
        assert!(matches!(e, Entry::Ready { w: 3, h: 2, .. }), "{e:?}");
        // None missing: no more looking.
        images.sweep(&mut core, t0 + MISSING_FIRST * 3);
        assert_eq!(images.missing, None);
        // Gone again: the wait starts over, and never passes the most.
        std::fs::remove_file(&path).unwrap();
        let mut images = Images::default();
        let mut at = t0;
        let _ = look(&mut images, &mut core, at);
        let mut waits = Vec::new();
        for _ in 0..8 {
            let (wait, _) = look(&mut images, &mut core, at);
            let w = wait.unwrap();
            waits.push(w);
            at += w;
        }
        assert_eq!(waits[0], MISSING_FIRST);
        assert!(waits.iter().all(|w| *w <= MISSING_MOST) && waits.contains(&MISSING_MOST), "{waits:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    pub(crate) fn png_saying(w: u32, h: u32) -> Vec<u8> {
        fn crc(bytes: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in bytes {
                c ^= u32::from(b);
                for _ in 0..8 {
                    c = if c & 1 == 1 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
                }
            }
            !c
        }
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend(w.to_be_bytes());
        ihdr.extend(h.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(13u32.to_be_bytes());
        png.extend(&ihdr);
        png.extend(crc(&ihdr).to_be_bytes());
        // Its pixels begun, so a reader goes on to them.
        png.extend(0u32.to_be_bytes());
        png.extend(b"IDAT");
        png.extend(crc(b"IDAT").to_be_bytes());
        png
    }

    /// A picture saying it is past the side or the memory a picture may
    /// take is too big to show, said so, and nothing is allocated for it;
    /// one at the edge is read.
    #[test]
    fn a_picture_past_the_limits_is_too_big_to_show() {
        let dir = std::env::temp_dir().join(format!("carnet-images-limits-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, w, h) in [("wide.png", 20_000, 10), ("huge.png", 16_000, 16_000), ("vast.png", u32::MAX / 2, u32::MAX / 2)] {
            let p = dir.join(name);
            std::fs::write(&p, png_saying(w, h)).unwrap();
            assert_eq!(decode(&p).err().as_deref(), Some(TOO_BIG), "{name}");
            assert_eq!(size(&p), (0, 0), "{name}: no space held for it");
        }
        let p = dir.join("long.png");
        std::fs::write(&p, encode_png(MAX_PIXELS_SIDE, 1, &vec![9; MAX_PIXELS_SIDE as usize * 4]).unwrap()).unwrap();
        assert_eq!(size(&p), (MAX_PIXELS_SIDE, 1));
        assert_eq!(decode(&p).map(|d| (d.w, d.h)), Ok((MAX_SIDE, 1)));
        // Named as no picture it reads: not read, whatever is in it.
        let p = dir.join("photo.xyz");
        std::fs::write(&p, encode_png(2, 2, &[0; 16]).unwrap()).unwrap();
        assert_eq!(decode(&p).err().as_deref(), Some(UNSUPPORTED));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_heic_photo_is_a_picture_to_keep() {
        assert!(is_image(Path::new("IMG_0001.HEIC")) && is_heic(Path::new("a.heif")) && !is_heic(Path::new("a.png")));
        assert!(for_imageio(Path::new("a.HEIC")) && for_imageio(Path::new("scan.tiff")) && for_imageio(Path::new("x.avif")));
        assert!(!for_imageio(Path::new("a.png")) && !for_imageio(Path::new("a.svg")) && !for_imageio(Path::new("noext")));
    }
}
