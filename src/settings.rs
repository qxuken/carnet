//! What the reader chose: the theme, the faces and size the page is set
//! in, its width, the library's order, and where the notes live. Kept as
//! `key = value` lines in the OS's config folder, read once at start and
//! written on every change.

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
    Stars,
    Asterism,
    Flourish,
    /// A printer's leaf, the hedera (❧).
    Fleuron,
    /// Three small diamonds in a row: a dinkus of lozenges.
    Diamonds,
    /// A short rule each side of a diamond.
    Lozenge,
    /// A swung dash (⁓): one wave.
    Wave,
}

impl Divider {
    pub const ALL: [Divider; 9] = [
        Divider::Dots,
        Divider::Line,
        Divider::Stars,
        Divider::Asterism,
        Divider::Flourish,
        Divider::Fleuron,
        Divider::Diamonds,
        Divider::Lozenge,
        Divider::Wave,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Divider::Dots => "dots",
            Divider::Line => "line",
            Divider::Stars => "stars",
            Divider::Asterism => "asterism",
            Divider::Flourish => "flourish",
            Divider::Fleuron => "fleuron",
            Divider::Diamonds => "diamonds",
            Divider::Lozenge => "lozenge",
            Divider::Wave => "wave",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.key() == s)
    }

    /// What it is, in words: its name to a screen reader and in a tooltip.
    pub fn name(self) -> &'static str {
        match self {
            Divider::Dots => "Three dots",
            Divider::Line => "A short line",
            Divider::Stars => "Three stars",
            Divider::Asterism => "Stars in a triangle",
            Divider::Flourish => "A flourish",
            Divider::Fleuron => "A printer's leaf",
            Divider::Diamonds => "Three diamonds",
            Divider::Lozenge => "A diamond between lines",
            Divider::Wave => "A wave",
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
    /// The library shows beside the page; hidden, the page has the
    /// window.
    pub sidebar: bool,
    /// Colours given lately, newest first (0xRRGGBBAA): the picker's
    /// recent row.
    pub colors: Vec<u32>,
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
            initial_lines: 3,
            initial_font: String::new(),
            initial_accent: true,
            library_w: crate::design::size::SIDEBAR as u32,
            translucent: true,
            fade_tools: true,
            focus: false,
            typewriter: false,
            sidebar: true,
            colors: Vec::new(),
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
                "sidebar" => s.sidebar = v != "hidden",
                "colors" => {
                    s.colors = v.split(',').filter_map(carnet_core::inline::parse_color).take(10).collect();
                }
                "dir" if !v.is_empty() => s.dir = Some(PathBuf::from(v)),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
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
        format!(
            "# Carnet settings\ntheme = {theme}\npalette = {}\ndensity = {}\nfont = {}\nheading_font = {}\ncode_font = {}\nsize = {}\nwidth = {width}\nsort = {sort}\ndates = {}\ndivider = {}\ninitial_lines = {}\ninitial_font = {}\ninitial_color = {}\nlibrary_width = {}\nwindow = {}\ntoolbar = {}\nfocus = {}\ntypewriter = {}\nsidebar = {}\ncolors = {}\ndir = {dir}\n",
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
            if self.sidebar { "shown" } else { "hidden" },
            self.colors.iter().map(|&c| carnet_core::inline::css_color(c)).collect::<Vec<_>>().join(","),
        )
    }

    /// How the page looks and behaves back as they first were: the
    /// colours, faces, size, width, the letter and dividers, the window
    /// (solid or translucent, taken at the next start), and the writing
    /// aids. Not the notes folder, the library (its order, dates, width,
    /// whether it shows), nor the colours given lately.
    pub fn reset_look(&mut self) {
        *self = Settings {
            sort: self.sort,
            dates_created: self.dates_created,
            library_w: self.library_w,
            sidebar: self.sidebar,
            colors: std::mem::take(&mut self.colors),
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
            divider: Divider::Flourish,
            initial_lines: 4,
            initial_font: "Zapfino".into(),
            initial_accent: false,
            library_w: 320,
            translucent: false,
            fade_tools: false,
            focus: true,
            typewriter: true,
            sidebar: false,
            colors: vec![0xc0504dff, 0x12345680],
            dir: Some(PathBuf::from("D:\\My notes")),
            file: None,
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        let odd = Settings::parse("size = 400\nwhat\ntheme = purple\n");
        assert_eq!(odd.size, 24);
        assert_eq!(odd.theme, ThemePref::System);
        let odd = Settings::parse("size = 2\nlibrary_width = 9000\ndivider = wavy\n");
        assert_eq!((odd.size, odd.library_w, odd.divider), (12, 480, Divider::Dots));
        // A file from before the letter's styles, which dropped every
        // note's first letter: read, the old key passed over; three lines,
        // the headings' face, the accent; and a height out of range held in.
        let old = Settings::parse("initial = large\n");
        assert_eq!(old, Settings::default());
        assert!(!old.to_text().contains("initial ="));
        assert_eq!(Settings::parse("initial_lines = 9\n").initial_lines, 4);
        assert_eq!(Settings::parse("initial_lines = 1\n").initial_lines, 2);
    }

    #[test]
    fn a_reset_puts_back_the_look_and_keeps_the_rest() {
        let mut s = Settings::parse(
            "theme = dark\npalette = sage\nfont = Georgia\ncode_font = Menlo\nsize = 20\nwidth = wide\ninitial_lines = 4\nfocus = on\n\
             window = solid\nsort = title\ndates = created\nlibrary_width = 300\nsidebar = hidden\ncolors = #c0504d\ndir = /notes\n",
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
        assert_eq!(s.colors, vec![0xc0504dff]);
        assert_eq!(s.dir.as_deref(), Some(Path::new("/notes")));
        assert_eq!(s.file.as_deref(), Some(Path::new("/c/settings.txt")));
    }

    /// Each divider is kept by its own word, the first five as ever, so a
    /// settings file from before the newer ones reads the same.
    #[test]
    fn every_divider_is_kept_by_its_word() {
        for d in Divider::ALL {
            let s = Settings { divider: d, ..Settings::default() };
            assert_eq!(Settings::parse(&s.to_text()).divider, d);
        }
        let words = ["dots", "line", "stars", "asterism", "flourish"];
        assert_eq!(words.map(|w| Divider::from_key(w).map(Divider::key)), words.map(Some));
        assert_eq!(Settings::parse("divider = fleuron\n").divider, Divider::Fleuron);
    }
}
