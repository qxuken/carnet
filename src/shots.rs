//! `carnet --screenshots <dir>`: the README's pictures of the desktop app
//! (`docs/light.png`, `dark.png`, `settings.png`, `narrow.png`), taken by
//! the app itself. Each is a fresh app over a demo library made in a
//! temporary folder, run in kui's headless runner, turned until nothing
//! is left to draw, and its last frame kept.
//!
//! Nothing of the machine's is read or written: the settings are in
//! memory, the notes and their history in the temporary folder, sync is
//! off and no update is asked for. What the OS would say is pinned (light
//! or dark, the accent, motion, the language), the faces are the ones
//! Carnet ships, and every note's age is set back from now, so a shot
//! taken on Windows is the one a Mac takes. The headless runner tells the
//! app of no window controls the OS draws, so the Mac's traffic lights
//! are told of ([`LIGHTS`]) and drawn on the frame after, with the
//! window's rounded corners.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use kui_native::{
    App, Appearance, Assistive, Color, ContrastPref, Core, HeadlessRunner, Locale, MotionPref,
    Rect, SystemEnv, Ui, UiEvent, Waker,
};

use crate::app::Carnet;
use crate::design::Palette;
use crate::settings::{Settings, ThemePref};
use crate::store::Store;

/// One picture: its file, the window's size (logical px), the note it
/// shows (the newest, with none), dark or not, and Settings open or not.
struct Shot {
    file: &'static str,
    size: (f32, f32),
    note: Option<&'static str>,
    dark: bool,
    settings: bool,
}

const SHOTS: [Shot; 4] = [
    Shot { file: "light.png", size: (1100.0, 740.0), note: None, dark: false, settings: false },
    Shot { file: "dark.png", size: (1100.0, 740.0), note: Some("Books"), dark: true, settings: false },
    Shot { file: "settings.png", size: (1100.0, 740.0), note: None, dark: false, settings: true },
    // Too narrow for the library beside the page: the note, with a way back.
    Shot { file: "narrow.png", size: (560.0, 560.0), note: Some("Q4 plan"), dark: true, settings: false },
];

/// Drawn at a Mac's 2×.
const SCALE: f32 = 2.0;

/// How long a shot may take to settle before it's given up on.
const TIMEOUT: Duration = Duration::from_secs(30);

/// How long nothing must happen before the frame is taken: a picture
/// decoded on a reader thread wakes the app well within it.
const QUIET: Duration = Duration::from_millis(300);

/// Where macOS 27 puts the traffic lights in a full toolbar's titlebar,
/// as `env.window.native_controls` reports them there: 14 pt buttons from
/// x 19, centred 26 pt down, and as much again after them.
const LIGHTS: Rect = Rect { x: 0.0, y: 0.0, w: 98.0, h: 52.0 };

/// The lights' centres (logical px) and their colours, fill and rim.
const LIGHT_DOTS: [(f32, u32, u32); 3] = [(26.0, 0xff5f57, 0xe2463f), (49.0, 0xfebc2e, 0xe1a325), (72.0, 0x28c840, 0x14ae2e)];
const LIGHT_Y: f32 = 26.0;
const LIGHT_R: f32 = 7.0;

/// A Mac window's corner (logical px).
const CORNER: f32 = 20.0;

/// A note of the demo library: its folder, its file, how long ago it was
/// last changed, and its words.
struct Demo {
    folder: &'static str,
    file: &'static str,
    ago: Duration,
    text: &'static str,
}

const MINUTE: u64 = 60;

/// The demo library, newest first. Each age is a little past the one it
/// reads as ("1m", "45m"), so a slow run still reads it.
const NOTES: [Demo; 6] = [
    Demo {
        folder: "Home",
        file: "Weekend.md",
        ago: Duration::from_secs(MINUTE + 20),
        text: "# Weekend\n\n\
               <span class=\"initial\">T</span>he light on the hill at seven is ==worth getting up for==: the valley \
               still in shadow, the ridge already gold, and the hut's chimney smoking by the time we reach the last bend.\n\n\
               ![The ridge at seven|right|300](../attachments/ridge.png)\n\n\
               - [x] Book the cabin\n\
               - [ ] Pack the warm jacket\n\
               - [ ] Charge the camera\n  \
               - [ ] And the spare battery\n",
    },
    Demo {
        folder: "Work",
        file: "Q4 plan.md",
        ago: Duration::from_secs(45 * MINUTE + 20),
        text: "# Q4 plan\n\n\
               Ship **folders** and *inline styles*, then ~~nothing else~~ rest.\n\n\
               - [x] Folders\n\
               - [ ] Styles in tables\n\
               - [ ] Pictures beside the text\n",
    },
    Demo { folder: "Home", file: "Groceries.md", ago: Duration::from_secs(52 * MINUTE + 20), text: "# Groceries\n\n- [ ] milk\n" },
    Demo {
        folder: "Reading",
        file: "Books.md",
        ago: Duration::from_secs(62 * MINUTE),
        text: "# Books\n\n\
               | Title | Author | Status |\n\
               | --- | --- | --- |\n\
               | The Remains of the Day | Kazuo Ishiguro | Read |\n\
               | A Gentleman in Moscow | Amor Towles | Reading |\n\
               | Piranesi | Susanna Clarke | Next |\n\
               | Stoner | John Williams | Next |\n",
    },
    Demo { folder: "Work/Clients", file: "Acme.md", ago: Duration::from_secs(122 * MINUTE), text: "# Acme\n\nCall on Thursday.\n" },
    Demo {
        folder: "",
        file: "Ideas.md",
        ago: Duration::from_secs(16 * 24 * 60 * MINUTE),
        text: "# Ideas\n\nA calm notes app: lists and tables that stay put, and every note a Markdown file you can open anywhere.\n",
    },
];

/// The Weekend note's picture: a warm sky, a pale sun, three rolling
/// hills and two small trees.
const RIDGE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="524" viewBox="0 0 600 262">
<defs>
<linearGradient id="sky" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#f4d6b8"/><stop offset="1" stop-color="#eebd96"/></linearGradient>
<radialGradient id="glow"><stop offset="0" stop-color="#fdf1de" stop-opacity="0.9"/><stop offset="1" stop-color="#fdf1de" stop-opacity="0"/></radialGradient>
</defs>
<rect width="600" height="262" fill="url(#sky)"/>
<circle cx="405" cy="104" r="78" fill="url(#glow)"/>
<circle cx="405" cy="104" r="22" fill="#fbead2"/>
<path d="M0 150 C90 120 180 125 270 140 S450 115 600 135 V262 H0Z" fill="#c99a82"/>
<path d="M0 185 C100 160 220 170 320 185 S500 165 600 178 V262 H0Z" fill="#a87468"/>
<path d="M105 174 l7 -22 l7 22z M115 175 l4.5 -14 l4.5 14z M466 185 l7 -22 l7 22z M476 186 l4.5 -14 l4.5 14z" fill="#5f3e3a"/>
<path d="M0 225 C120 205 260 210 380 216 S540 205 600 212 V262 H0Z" fill="#7f5550"/>
</svg>"##;

/// Every shot, written into `dir`. Fails, saying which and why, rather
/// than waiting for ever.
pub fn run(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for shot in &SHOTS {
        take(shot, &dir.join(shot.file)).map_err(|e| format!("{}: {e}", shot.file))?;
    }
    Ok(())
}

fn take(shot: &Shot, to: &Path) -> Result<(), String> {
    let lib = Library::new()?;
    let store = Store::open_with(lib.notes(), Some(lib.0.join("replica"))).map_err(|e| e.to_string())?;
    let settings = Settings {
        theme: if shot.dark { ThemePref::Dark } else { ThemePref::Light },
        palette: Palette::Linen,
        translucent: false,
        check_updates: false,
        file: None,
        ..Settings::default()
    };
    let mut app = Carnet::with_settings(store, settings);
    app.updates = crate::update::Updates::with(None);
    if let Some(title) = shot.note {
        let i = app.store.notes.iter().position(|n| n.title == title).ok_or(format!("no note {title:?}"))?;
        app.open(i);
    }
    app.settings_open = shot.settings;
    // The caret in the note's first block, as someone writing would have
    // it: the toolbar shows that block's tools. (The caret itself is not
    // drawn: `Framed`.)
    app.editing = true;
    let (w, h) = shot.size;
    let mut runner = launcher(shot.dark).open_headless(Framed(app), w, h, SCALE);
    settle(&mut runner)?;
    let frame = kui_native::capture(runner.core_mut())?;
    write(frame, to)
}

/// The launcher as main.rs builds it, and what the OS would say pinned.
fn launcher(dark: bool) -> kui_native::Launcher {
    kui_native::app("Carnet")
        .chrome(kui_native::Chrome::Custom)
        .titlebar(kui_native::Titlebar::Tall)
        .backdrop(kui_native::Backdrop::Opaque)
        .system(SystemEnv {
            appearance: if dark { Appearance::Dark } else { Appearance::Light },
            // A Mac's own blue.
            accent: Some(Color::hex(0x007affff)),
            motion: MotionPref::Full,
            locale: Locale::new("en-GB"),
            assistive: Assistive::None,
            contrast: ContrastPref::Normal,
        })
}

/// Turns until the app has nothing left to do: its fonts in, its pictures
/// read and handed over, its floats laid out, nothing animating.
fn settle(runner: &mut HeadlessRunner<Framed>) -> Result<(), String> {
    let deadline = Instant::now() + TIMEOUT;
    let mut quiet_from: Option<Instant> = None;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(format!("still drawing after {}s", TIMEOUT.as_secs()));
        }
        if !runner.turn_until(deadline.min(now + QUIET / 4)) {
            return Err("the app closed".into());
        }
        if runner.idle() && !runner.app_mut().0.pictures_loading() && runner.frames() > 1 {
            let from = *quiet_from.get_or_insert(now);
            if from.elapsed() >= QUIET {
                return Ok(());
            }
        } else {
            quiet_from = None;
        }
    }
}

/// The frame as a Mac's window looks: the traffic lights, and the corners
/// rounded off, outside them clear. Written as a PNG, packed hard.
fn write(mut f: kui_native::Capture, to: &Path) -> Result<(), String> {
    use image::ImageEncoder;
    let (w, h, k) = (f.width as usize, f.height as usize, f.scale);
    let px = |x: usize, y: usize| (y * w + x) * 4;
    for (cx, fill, rim) in LIGHT_DOTS {
        let (cx, cy, r) = (cx * k, LIGHT_Y * k, LIGHT_R * k);
        for y in (cy - r - 1.0) as usize..=(cy + r + 1.0) as usize {
            for x in (cx - r - 1.0) as usize..=(cx + r + 1.0) as usize {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let i = px(x, y);
                blend(&mut f.rgba[i..i + 4], rim, (r - d + 0.5).clamp(0.0, 1.0));
                blend(&mut f.rgba[i..i + 4], fill, (r - 0.5 * k - d + 0.5).clamp(0.0, 1.0));
            }
        }
    }
    let r = CORNER * k;
    for y in 0..r.ceil() as usize {
        for x in 0..r.ceil() as usize {
            let d = ((r - x as f32 - 0.5).powi(2) + (r - y as f32 - 0.5).powi(2)).sqrt();
            let keep = (r - d + 0.5).clamp(0.0, 1.0);
            for (cx, cy) in [(x, y), (w - 1 - x, y), (x, h - 1 - y), (w - 1 - x, h - 1 - y)] {
                let a = &mut f.rgba[px(cx, cy) + 3];
                *a = (*a as f32 * keep).round() as u8;
            }
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(&mut png, image::codecs::png::CompressionType::Best, image::codecs::png::FilterType::Adaptive)
        .write_image(&f.rgba, f.width, f.height, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    std::fs::write(to, png).map_err(|e| format!("{}: {e}", to.display()))
}

/// `rgb` laid over an opaque pixel, `a` of it.
fn blend(p: &mut [u8], rgb: u32, a: f32) {
    for (c, v) in p[..3].iter_mut().zip([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]) {
        *c = (*c as f32 + (v as f32 - *c as f32) * a).round() as u8;
    }
}

/// The app, told of the traffic lights a Mac draws over its corner: the
/// headless runner tells it of none, and kui's own window buttons would
/// be drawn in their place. Its caret is in a blink's off phase, which a
/// headless core never comes to by itself.
struct Framed(Carnet);

impl App for Framed {
    fn setup(&mut self, waker: Waker) {
        self.0.setup(waker);
    }

    fn view(&mut self, ui: &mut Ui<'_>) {
        ui.core().env.window.native_controls = Some(LIGHTS);
        ui.core().set_caret_visible(false);
        self.0.view(ui);
    }

    fn on_event_with(&mut self, ev: UiEvent, core: &mut Core) {
        self.0.on_event_with(ev, core);
    }

    fn on_event(&mut self, ev: UiEvent) {
        self.0.on_event(ev);
    }

    fn on_wake(&mut self) {
        self.0.on_wake();
    }

    fn view_state(&self) -> Option<u64> {
        self.0.view_state()
    }

    fn teardown(&mut self) {
        self.0.teardown();
    }
}

/// The demo library in a temporary folder of its own: the notes, their
/// folders and the picture under `notes/`, their history beside it. Gone
/// when dropped.
struct Library(PathBuf);

impl Library {
    fn new() -> Result<Self, String> {
        let root = std::env::temp_dir().join(format!("carnet-shots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let lib = Library(root);
        lib.write().map_err(|e| format!("the demo library: {e}"))?;
        Ok(lib)
    }

    fn notes(&self) -> PathBuf {
        self.0.join("notes")
    }

    fn write(&self) -> std::io::Result<()> {
        let notes = self.notes();
        let pictures = notes.join(crate::store::ATTACHMENTS);
        std::fs::create_dir_all(&pictures)?;
        let svg = self.0.join("ridge.svg");
        std::fs::write(&svg, RIDGE)?;
        crate::images::shrink_to_png(&svg, &pictures.join("ridge.png"), 1200, 1200).map_err(std::io::Error::other)?;
        let now = SystemTime::now();
        for n in &NOTES {
            let folder = notes.join(n.folder);
            std::fs::create_dir_all(&folder)?;
            let path = folder.join(n.file);
            std::fs::write(&path, n.text)?;
            // The age the store reads: the file's modified time.
            std::fs::File::options().write(true).open(&path)?.set_modified(now - n.ago)?;
        }
        Ok(())
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carnet_core::doc::{Float, Kind};

    /// The demo library reads back as the shots show it: every note in its
    /// folder, newest first, the Weekend note's dropped letter, highlight,
    /// picture at the right and indented item, and the picture there.
    #[test]
    fn the_demo_library_is_what_the_shots_show() {
        let lib = Library::new().unwrap();
        let store = Store::open_with(lib.notes(), None).unwrap();
        let mut notes: Vec<_> = store.notes.iter().collect();
        notes.sort_by_key(|n| std::cmp::Reverse(n.modified));
        let seen: Vec<_> = notes.iter().map(|n| (n.folder.as_str(), n.title.as_str())).collect();
        assert_eq!(
            seen,
            [("Home", "Weekend"), ("Work", "Q4 plan"), ("Home", "Groceries"), ("Reading", "Books"), ("Work/Clients", "Acme"), ("", "Ideas")]
        );
        assert_eq!(store.folders, ["Home", "Reading", "Work", "Work/Clients"]);
        let weekend = &notes[0].doc;
        let blocks = weekend.blocks();
        assert!(blocks[0].initial, "the dropped letter");
        assert!(matches!(&blocks[0].kind, Kind::Text(t) if t.contains("==worth getting up for==")), "{:?}", blocks[0].kind);
        let Kind::Image { src, float: Float::Right, width: Some(300), .. } = &blocks[1].kind else { panic!("{:?}", blocks[1].kind) };
        assert!(store.resolve("Home", src).is_some_and(|p| p.is_file()), "{src}");
        let last = &blocks.iter().rfind(|b| matches!(b.kind, Kind::Item { .. })).unwrap().kind;
        assert!(matches!(last, Kind::Item { indent: 1, done: false, text, .. } if text == "And the spare battery"), "{last:?}");
        let ago = SystemTime::now().duration_since(notes[0].modified).unwrap();
        assert!(ago >= Duration::from_secs(60) && ago < Duration::from_secs(120), "{ago:?}");
    }
}
