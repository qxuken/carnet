//! What the reader chose: the theme, the faces and size the page is set
//! in, its width, the library's order, and where the notes live. Kept as
//! `key = value` lines in the OS's config folder, read once at start and
//! written on every change.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::design::{Density, Palette};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePref {
    System,
    Light,
    Dark,
}

/// The page's measure, padding included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Narrow,
    Medium,
    Wide,
}

impl Width {
    pub fn px(self) -> f32 {
        match self {
            Width::Narrow => 620.0,
            Width::Medium => 720.0,
            Width::Wide => 880.0,
        }
    }
}

/// The library's order, after the pinned notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Edited,
    Title,
    Created,
}

pub const SIZES: std::ops::RangeInclusive<u32> = 12..=24;

/// How many lines a dropped first letter can be tall.
pub const INITIAL_LINES: std::ops::RangeInclusive<u32> = 2..=4;

/// How wide the library can be dragged, in px.
pub const LIBRARY_WIDTHS: std::ops::RangeInclusive<u32> = 200..=480;

/// How a divider is drawn on the page. The file says `---` whichever.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Divider {
    Dots,
    Line,
    /// Three small diamonds in a row: a dinkus of lozenges.
    Diamonds,
    /// A short rule each side of a diamond.
    Lozenge,
    /// A swung dash (⁓): one wave.
    Wave,
    /// The reader's own words (`divider_text`), centred: `~ ✦ ~`, `• • •`.
    Text,
    /// The reader's own picture (`divider_picture`), centred, a line of
    /// text tall or so.
    Picture,
}

/// The longest the reader's own words for a divider may be, in letters.
pub const DIVIDER_TEXT_MOST: usize = 40;

impl Divider {
    pub const ALL: [Divider; 7] = [
        Divider::Dots,
        Divider::Line,
        Divider::Diamonds,
        Divider::Lozenge,
        Divider::Wave,
        Divider::Text,
        Divider::Picture,
    ];

    /// What a divider of the reader's own is drawn as while it has nothing
    /// to show: no words typed, a picture gone or unreadable.
    pub const FALLBACK: Divider = Divider::Dots;

    pub fn key(self) -> &'static str {
        match self {
            Divider::Dots => "dots",
            Divider::Line => "line",
            Divider::Diamonds => "diamonds",
            Divider::Lozenge => "lozenge",
            Divider::Wave => "wave",
            Divider::Text => "text",
            Divider::Picture => "picture",
        }
    }

    /// A divider by its word. The ways no longer offered, from a file an
    /// older Carnet wrote, are read as the nearest still here: three
    /// stars, or stars in a triangle, as three dots; a flourish as a wave;
    /// a printer's leaf as a diamond between lines.
    pub fn from_key(s: &str) -> Option<Self> {
        let s = match s {
            "stars" | "asterism" => "dots",
            "flourish" => "wave",
            "fleuron" => "lozenge",
            s => s,
        };
        Self::ALL.into_iter().find(|d| d.key() == s)
    }

    /// What it is, in words: its name to a screen reader and in a tooltip.
    pub fn name(self) -> &'static str {
        match self {
            Divider::Dots => "Three dots",
            Divider::Line => "A short line",
            Divider::Diamonds => "Three diamonds",
            Divider::Lozenge => "A diamond between lines",
            Divider::Wave => "A wave",
            Divider::Text => "Your own words",
            Divider::Picture => "Your own picture",
        }
    }
}

/// How a mouse wheel's notch scrolls: Carnet's own feel, or one of kui's
/// presets (`kui_native::WheelFeel`), from slow to fast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wheel {
    /// 60 px a notch, glided there over a 0.5 s response with a small
    /// kick: further than kui's and softer.
    Carnet,
    Gentle,
    Standard,
    Brisk,
    Swift,
}

impl Wheel {
    pub const ALL: [Wheel; 5] = [Wheel::Carnet, Wheel::Gentle, Wheel::Standard, Wheel::Brisk, Wheel::Swift];

    pub fn key(self) -> &'static str {
        match self {
            Wheel::Carnet => "carnet",
            Wheel::Gentle => "gentle",
            Wheel::Standard => "standard",
            Wheel::Brisk => "brisk",
            Wheel::Swift => "swift",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.key() == s)
    }

    /// What it is called in Settings.
    pub fn name(self) -> &'static str {
        match self {
            Wheel::Carnet => "Carnet",
            Wheel::Gentle => "Gentle",
            Wheel::Standard => "Standard",
            Wheel::Brisk => "Brisk",
            Wheel::Swift => "Swift",
        }
    }

    pub fn feel(self) -> kui_native::WheelFeel {
        use kui_native::WheelFeel;
        match self {
            Wheel::Carnet => WheelFeel { line_px: 60.0, response: 0.5, kick: 0.2 },
            Wheel::Gentle => WheelFeel::GENTLE,
            Wheel::Standard => WheelFeel::STANDARD,
            Wheel::Brisk => WheelFeel::BRISK,
            Wheel::Swift => WheelFeel::SWIFT,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Light, dark, or as the OS is.
    pub theme: ThemePref,
    /// The colours: one of the design system's palettes.
    pub palette: Palette,
    pub density: Density,
    /// The body's face by family name; empty for the system's own.
    pub font: String,
    /// The title's and headings' face; empty for the built-in serif.
    pub heading_font: String,
    /// Code's face, a block of it or a word: a monospaced family by name;
    /// empty for the system's own monospace (SF Mono or Menlo, Cascadia
    /// Mono or Consolas, DejaVu Sans Mono…).
    pub code_font: String,
    /// The body's size in px; headings and tables scale with it.
    pub size: u32,
    pub width: Width,
    pub sort: Sort,
    /// A note's date, on the page and in the library: when it was made;
    /// or when last edited.
    pub dates_created: bool,
    /// How a divider is drawn.
    pub divider: Divider,
    /// The words a divider of the reader's own is drawn as; kept when
    /// another way is chosen, for when it is chosen again.
    pub divider_text: String,
    /// The picture a divider of the reader's own is drawn as: a copy in
    /// the settings' folder, by its name there (so the one picked can be
    /// moved or thrown away), or, with no folder to copy it to, where it
    /// was picked. Kept as the words are.
    pub divider_picture: Option<PathBuf>,
    /// How many lines a dropped letter is tall, at most. A letter drops
    /// only where its paragraph asks (the toolbar's toggle); an old file's
    /// `initial = large`, which dropped every note's first, is passed over.
    pub initial_lines: u32,
    /// Its face by family name; empty for the headings'.
    pub initial_font: String,
    /// In the accent's ink; or the text's.
    pub initial_accent: bool,
    /// The library's width in px, as dragged.
    pub library_w: u32,

    /// The library column shows the desktop through it, blurred, where
    /// the system can; read when the window opens.
    pub translucent: bool,
    /// The toolbar fades while the reader types, and comes back when the
    /// pointer moves, on a click or a selection.
    pub fade_tools: bool,
    /// While the reader types, every block but the one being written
    /// steps back.
    pub focus: bool,
    /// The line being written is held near the middle of the window, the
    /// page scrolling under it, as on a typewriter.
    pub typewriter: bool,
    /// How a link in a note opens while it's written in: ⌘-click (Ctrl
    /// elsewhere), a click, or a double-click.
    pub link_open: carnet_core::page::LinkOpen,
    /// How a mouse wheel's notch scrolls, in every window.
    pub wheel: Wheel,
    /// The library shows beside the page; hidden, the page has the
    /// window.
    pub sidebar: bool,
    /// The folders whose subfolders the library hides, by path: this
    /// device's view of them, not the notes'. One `collapsed = path` line
    /// each; "" is the notes folder itself, All notes with every folder
    /// folded away, written `folders = folded`.
    pub collapsed: BTreeSet<String>,
    /// Once a day, GitHub asked whether a newer Carnet is out
    /// (src/update.rs).
    pub check_updates: bool,
    /// The newest version the app has said is out, so it's said once;
    /// empty for none.
    pub told_update: String,
    /// Where the notes live; `None` for `~/Documents/Carnet`.
    pub dir: Option<PathBuf>,
    /// The file they are kept in; `None` keeps them in memory (tests).
    pub file: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePref::System,
            palette: Palette::Linen,
            density: Density::Comfortable,
            font: String::new(),
            heading_font: String::new(),
            code_font: String::new(),
            size: 16,
            width: Width::Medium,
            sort: Sort::Edited,
            dates_created: false,
            divider: Divider::Dots,
            divider_text: String::new(),
            divider_picture: None,
            initial_lines: 3,
            initial_font: String::new(),
            initial_accent: true,
            library_w: crate::design::size::SIDEBAR as u32,
            translucent: true,
            fade_tools: true,
            focus: false,
            typewriter: false,
            link_open: carnet_core::page::LinkOpen::ModClick,
            wheel: Wheel::Carnet,
            sidebar: true,
            collapsed: BTreeSet::new(),
            check_updates: true,
            told_update: String::new(),
            dir: None,
            file: None,
        }
    }
}

impl Settings {
    /// The folder the settings file is in: where sync's file and, without
    /// a keychain, its keys go too.
    pub fn dir(&self) -> Option<PathBuf> {
        Some(self.file.as_ref()?.parent()?.to_path_buf())
    }

    /// The settings in the OS's config folder (`CARNET_CONFIG` names
    /// another file), or the defaults when there are none yet.
    pub fn load() -> Self {
        let file = std::env::var_os("CARNET_CONFIG")
            .map(PathBuf::from)
            .or_else(|| config_dir().map(|d| d.join("settings.txt")));
        let mut s = match &file {
            Some(f) => fs::read_to_string(f).map(|t| Self::parse(&t)).unwrap_or_default(),
            None => Self::default(),
        };
        s.file = file;
        s
    }

    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "theme" => {
                    s.theme = match v {
                        "light" => ThemePref::Light,
                        "dark" => ThemePref::Dark,
                        _ => ThemePref::System,
                    }
                }
                "palette" => s.palette = Palette::from_key(v).unwrap_or(Palette::Linen),
                "density" => s.density = Density::from_key(v).unwrap_or(Density::Comfortable),
                "font" => s.font = v.to_string(),
                "heading_font" => s.heading_font = v.to_string(),
                "code_font" => s.code_font = v.to_string(),
                "size" => {
                    if let Ok(n) = v.parse::<u32>() {
                        s.size = n.clamp(*SIZES.start(), *SIZES.end());
                    }
                }
                "width" => {
                    s.width = match v {
                        "narrow" => Width::Narrow,
                        "wide" => Width::Wide,
                        _ => Width::Medium,
                    }
                }
                "sort" => {
                    s.sort = match v {
                        "title" => Sort::Title,
                        "created" => Sort::Created,
                        _ => Sort::Edited,
                    }
                }
                "dates" => s.dates_created = v == "created",
                "divider" => s.divider = Divider::from_key(v).unwrap_or(Divider::Dots),
                "divider_text" => s.divider_text = v.chars().take(DIVIDER_TEXT_MOST).collect(),
                "divider_picture" if !v.is_empty() => s.divider_picture = Some(PathBuf::from(v)),
                "initial_lines" => {
                    if let Ok(n) = v.parse::<u32>() {
                        s.initial_lines = n.clamp(*INITIAL_LINES.start(), *INITIAL_LINES.end());
                    }
                }
                "initial_font" => s.initial_font = v.to_string(),
                "initial_color" => s.initial_accent = v != "text",
                "library_width" => {
                    if let Ok(n) = v.parse::<u32>() {
                        s.library_w = n.clamp(*LIBRARY_WIDTHS.start(), *LIBRARY_WIDTHS.end());
                    }
                }
                "window" => s.translucent = v != "solid",
                "toolbar" => s.fade_tools = v != "stays",
                "focus" => s.focus = v == "on",
                "typewriter" => s.typewriter = v == "on",
                "links" => s.link_open = carnet_core::page::LinkOpen::from_key(v).unwrap_or_default(),
                "wheel" => s.wheel = Wheel::from_key(v).unwrap_or(Wheel::Carnet),
                "sidebar" => s.sidebar = v != "hidden",
                "collapsed" if !v.is_empty() => {
                    s.collapsed.insert(v.to_string());
                }
                "folders" if v == "folded" => {
                    s.collapsed.insert(String::new());
                }
                "check_updates" => s.check_updates = v != "off",
                "told_update" => s.told_update = v.to_string(),
                "dir" if !v.is_empty() => s.dir = Some(PathBuf::from(v)),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        // The reader's own divider after the rest: its words, and its
        // picture's name in this folder (or where it was picked); then the
        // wheel.
        let picture = self.divider_picture.as_deref().map(Path::display).map(|p| p.to_string()).unwrap_or_default();
        format!(
            "{}divider_text = {}\ndivider_picture = {picture}\nwheel = {}\n",
            self.keys_text(),
            self.divider_text,
            self.wheel.key()
        )
    }

    /// Where the picture a divider of the reader's own is drawn as is:
    /// a name is in the settings' folder.
    pub fn divider_picture_file(&self) -> Option<PathBuf> {
        let p = self.divider_picture.as_ref()?;
        if p.is_absolute() { Some(p.clone()) } else { Some(self.dir()?.join(p)) }
    }

    fn keys_text(&self) -> String {
        let theme = match self.theme {
            ThemePref::System => "system",
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
        };
        let width = match self.width {
            Width::Narrow => "narrow",
            Width::Medium => "medium",
            Width::Wide => "wide",
        };
        let sort = match self.sort {
            Sort::Edited => "edited",
            Sort::Title => "title",
            Sort::Created => "created",
        };
        let dir = self.dir.as_deref().map(Path::display).map(|d| d.to_string()).unwrap_or_default();
        let collapsed: String = self
            .collapsed
            .iter()
            .map(|f| if f.is_empty() { "folders = folded\n".to_string() } else { format!("collapsed = {f}\n") })
            .collect();
        format!(
            "# Carnet settings\ntheme = {theme}\npalette = {}\ndensity = {}\nfont = {}\nheading_font = {}\ncode_font = {}\nsize = {}\nwidth = {width}\nsort = {sort}\ndates = {}\ndivider = {}\ninitial_lines = {}\ninitial_font = {}\ninitial_color = {}\nlibrary_width = {}\nwindow = {}\ntoolbar = {}\nfocus = {}\ntypewriter = {}\nlinks = {}\nsidebar = {}\n{collapsed}check_updates = {}\ntold_update = {}\ndir = {dir}\n",
            self.palette.key(),
            self.density.key(),
            self.font,
            self.heading_font,
            self.code_font,
            self.size,
            if self.dates_created { "created" } else { "edited" },
            self.divider.key(),
            self.initial_lines,
            self.initial_font,
            if self.initial_accent { "accent" } else { "text" },
            self.library_w,
            if self.translucent { "translucent" } else { "solid" },
            if self.fade_tools { "fades" } else { "stays" },
            if self.focus { "on" } else { "off" },
            if self.typewriter { "on" } else { "off" },
            self.link_open.key(),
            if self.sidebar { "shown" } else { "hidden" },
            if self.check_updates { "on" } else { "off" },
            self.told_update,
        )
    }

    /// How the page looks and behaves back as they first were: the
    /// colours, faces, size, width, the letter and dividers, the window
    /// (solid or translucent, taken at the next start), and the writing
    /// aids. Not the notes folder, the library (its order, dates, width,
    /// whether it shows), the wheel (an advanced setting, not a look), nor
    /// the check for updates; and a divider of the reader's own, its words
    /// or picture, is no longer used but kept, for when it is chosen again.
    pub fn reset_look(&mut self) {
        *self = Settings {
            wheel: self.wheel,
            divider_text: std::mem::take(&mut self.divider_text),
            divider_picture: self.divider_picture.take(),
            sort: self.sort,
            dates_created: self.dates_created,
            library_w: self.library_w,
            sidebar: self.sidebar,
            collapsed: std::mem::take(&mut self.collapsed),
            check_updates: self.check_updates,
            told_update: std::mem::take(&mut self.told_update),
            dir: self.dir.take(),
            file: self.file.take(),
            ..Settings::default()
        };
    }

    /// Whether [`reset_look`](Self::reset_look) would change anything.
    pub fn look_is_default(&self) -> bool {
        let mut d = self.clone();
        d.reset_look();
        d == *self
    }

    /// Writes them back, when they have a file.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(file) = &self.file else { return Ok(()) };
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        // Whole or not at all: a half-written file would forget where the
        // notes live.
        let tmp = file.with_extension("tmp");
        fs::write(&tmp, self.to_text())?;
        fs::rename(&tmp, file)
    }
}

/// `%APPDATA%\Carnet` on Windows, `~/Library/Application Support/Carnet`
/// on a Mac, `$XDG_CONFIG_HOME/carnet` (or `~/.config/carnet`) elsewhere.
fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Carnet"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Carnet"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|d| d.join("carnet"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip_and_bad_lines() {
        let s = Settings {
            theme: ThemePref::Dark,
            palette: Palette::Sage,
            density: Density::Airy,
            font: "Georgia".into(),
            heading_font: "Inter".into(),
            code_font: "JetBrains Mono".into(),
            size: 18,
            width: Width::Wide,
            sort: Sort::Title,
            dates_created: true,
            divider: Divider::Text,
            divider_text: "~ \u{2726} = ~".into(),
            divider_picture: Some(PathBuf::from("divider-1.png")),
            initial_lines: 4,
            initial_font: "Zapfino".into(),
            initial_accent: false,
            library_w: 320,
            translucent: false,
            fade_tools: false,
            focus: true,
            typewriter: true,
            link_open: carnet_core::page::LinkOpen::DoubleClick,
            wheel: Wheel::Brisk,
            sidebar: false,
            collapsed: [String::new(), "Work".to_string(), "Work/Old, kept = 2".to_string()].into(),
            check_updates: false,
            told_update: "0.1.2".into(),
            dir: Some(PathBuf::from("D:\\My notes")),
            file: None,
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        // All notes folded is a word of its own, not an empty path.
        assert!(s.to_text().contains("\nfolders = folded\n") && !s.to_text().contains("collapsed = \n"));
        assert!(Settings::parse("collapsed = \nfolders = open\n").collapsed.is_empty());
        let odd = Settings::parse("size = 400\nwhat\ntheme = purple\n");
        assert_eq!(odd.size, 24);
        assert_eq!(odd.theme, ThemePref::System);
        let odd = Settings::parse("size = 2\nlibrary_width = 9000\ndivider = wavy\nwheel = jerky\n");
        assert_eq!((odd.size, odd.library_w, odd.divider, odd.wheel), (12, 480, Divider::Dots, Wheel::Carnet));
        // A file from before the letter's styles, which dropped every
        // note's first letter: read, the old key passed over; three lines,
        // the headings' face, the accent; and a height out of range held in.
        let old = Settings::parse("initial = large\n");
        assert_eq!(old, Settings::default());
        assert!(!old.to_text().contains("initial ="));
        // And one from when the picker kept the colours given lately: the
        // line passed over, and not written back.
        let old = Settings::parse("colors = #c0504d,#12345680\n");
        assert_eq!(old, Settings::default());
        assert!(!old.to_text().contains("colors ="));
        assert_eq!(Settings::parse("initial_lines = 9\n").initial_lines, 4);
        assert_eq!(Settings::parse("initial_lines = 1\n").initial_lines, 2);
        // How links open: by its word; one not known, ⌘-click.
        assert_eq!(Settings::parse("links = click\n").link_open, carnet_core::page::LinkOpen::Click);
        assert_eq!(Settings::parse("links = triple\n").link_open, carnet_core::page::LinkOpen::ModClick);
    }

    /// The check for updates is on unless the file says off: a file from
    /// before it checks; one an older app wrote back (dropping the key)
    /// checks again.
    #[test]
    fn the_check_for_updates_is_on_unless_turned_off() {
        assert!(Settings::default().check_updates);
        assert!(Settings::parse("theme = dark\n").check_updates, "a file from before it");
        let off = Settings::parse("check_updates = off\ntold_update = 0.1.2\n");
        assert!(!off.check_updates);
        assert_eq!(off.told_update, "0.1.2");
        assert!(off.to_text().contains("check_updates = off\n"));
        assert!(Settings::parse(&Settings::default().to_text()).check_updates);
        let mut s = Settings { check_updates: false, size: 20, ..Settings::default() };
        s.reset_look();
        assert!(!s.check_updates, "not part of the look");
    }

    #[test]
    fn a_reset_puts_back_the_look_and_keeps_the_rest() {
        let mut s = Settings::parse(
            "theme = dark\npalette = sage\nfont = Georgia\ncode_font = Menlo\nsize = 20\nwidth = wide\ninitial_lines = 4\nfocus = on\n\
             window = solid\nsort = title\ndates = created\nlibrary_width = 300\nsidebar = hidden\ncollapsed = Work\ndir = /notes\n",
        );
        s.file = Some(PathBuf::from("/c/settings.txt"));
        assert!(!s.look_is_default());
        s.reset_look();
        assert!(s.look_is_default());
        let d = Settings::default();
        assert_eq!((s.theme, s.palette, s.font.as_str(), s.size, s.width), (d.theme, d.palette, "", 16, Width::Medium));
        assert_eq!((s.initial_lines, s.focus), (3, false));
        assert_eq!(s.code_font, "", "code in the system's monospace again");
        assert_eq!(s.translucent, d.translucent, "the window as at first");
        assert_eq!((s.sort, s.dates_created, s.library_w, s.sidebar), (Sort::Title, true, 300, false));
        assert!(s.collapsed.contains("Work"), "the folders folded stay folded");
        assert_eq!(s.dir.as_deref(), Some(Path::new("/notes")));
        assert_eq!(s.file.as_deref(), Some(Path::new("/c/settings.txt")));
    }

    /// Each divider is kept by its own word, the ornaments' as ever, so a
    /// settings file from before the reader's own reads the same.
    #[test]
    fn every_divider_is_kept_by_its_word() {
        for d in Divider::ALL {
            let s = Settings { divider: d, ..Settings::default() };
            assert_eq!(Settings::parse(&s.to_text()).divider, d);
        }
        let words = ["dots", "line", "diamonds", "lozenge", "wave"];
        assert_eq!(words.map(|w| Divider::from_key(w).map(Divider::key)), words.map(Some));
    }

    /// A file from when there were nine ornaments: the four gone read as
    /// the nearest still offered, never as the default by mistake, and
    /// are written back by those words.
    #[test]
    fn a_divider_no_longer_offered_reads_as_the_nearest_one() {
        for (old, now) in [("stars", Divider::Dots), ("asterism", Divider::Dots), ("flourish", Divider::Wave), ("fleuron", Divider::Lozenge)] {
            let s = Settings::parse(&format!("theme = dark\ndivider = {old}\nsize = 18\n"));
            assert_eq!((s.divider, s.theme, s.size), (now, ThemePref::Dark, 18), "{old}");
            assert!(s.to_text().contains(&format!("\ndivider = {}\n", now.key())), "{old}");
        }
    }

    /// The reader's own words for a divider come back as typed, but for
    /// the spaces at their ends: a line's own. Too long, they are cut. A
    /// file from before them has none, and a reset keeps them.
    #[test]
    fn a_dividers_own_words_survive_the_file() {
        let s = Settings { divider: Divider::Text, divider_text: "\u{2022} \u{2022} \u{2022}".into(), ..Settings::default() };
        let back = Settings::parse(&s.to_text());
        assert_eq!((back.divider, back.divider_text.as_str()), (Divider::Text, "\u{2022} \u{2022} \u{2022}"));
        assert_eq!(back, s);
        let long = Settings::parse(&format!("divider_text = {}\n", "x".repeat(100)));
        assert_eq!(long.divider_text.chars().count(), DIVIDER_TEXT_MOST);
        let old = Settings::parse("divider = dots\n");
        assert_eq!((old.divider_text.as_str(), old.divider_picture.as_deref()), ("", None));
        // A reset draws the dots again, the words kept for another time.
        let mut s = s;
        s.reset_look();
        assert_eq!((s.divider, s.divider_text.as_str()), (Divider::Dots, "\u{2022} \u{2022} \u{2022}"));
        assert!(s.look_is_default());
    }

    /// The picture is found by its name in the settings' folder, or where
    /// it was picked when it was kept by its whole path; with no folder, a
    /// name alone is nowhere.
    #[test]
    fn a_dividers_picture_is_found_beside_the_settings() {
        let folder = std::env::temp_dir().join("carnet-settings-divider");
        let mut s = Settings { divider_picture: Some(PathBuf::from("divider-7.png")), file: Some(folder.join("settings.txt")), ..Settings::default() };
        assert_eq!(s.divider_picture_file(), Some(folder.join("divider-7.png")));
        let whole = folder.join("elsewhere").join("leaf.png");
        s.divider_picture = Some(whole.clone());
        assert_eq!(s.divider_picture_file(), Some(whole.clone()));
        assert_eq!(Settings::parse(&s.to_text()).divider_picture, Some(whole));
        let loose = Settings { divider_picture: Some(PathBuf::from("divider-7.png")), ..Settings::default() };
        assert_eq!(loose.divider_picture_file(), None);
    }
}
