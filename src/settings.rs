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
}

impl Divider {
    pub const ALL: [Divider; 5] = [Divider::Dots, Divider::Line, Divider::Stars, Divider::Asterism, Divider::Flourish];

    pub fn key(self) -> &'static str {
        match self {
            Divider::Dots => "dots",
            Divider::Line => "line",
            Divider::Stars => "stars",
            Divider::Asterism => "asterism",
            Divider::Flourish => "flourish",
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
    /// The body's size in px; headings and tables scale with it.
    pub size: u32,
    pub width: Width,
    pub sort: Sort,
    /// How a divider is drawn.
    pub divider: Divider,
    /// The note's first paragraph opens with a dropped letter (`large`
    /// in the file, as it was).
    pub initial: bool,
    /// How many lines that letter is tall.
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
            size: 16,
            width: Width::Medium,
            sort: Sort::Edited,
            divider: Divider::Dots,
            initial: false,
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
                "divider" => s.divider = Divider::from_key(v).unwrap_or(Divider::Dots),
                "initial" => s.initial = v == "large",
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
            "# Carnet settings\ntheme = {theme}\npalette = {}\ndensity = {}\nfont = {}\nheading_font = {}\nsize = {}\nwidth = {width}\nsort = {sort}\ndivider = {}\ninitial = {}\ninitial_lines = {}\ninitial_font = {}\ninitial_color = {}\nlibrary_width = {}\nwindow = {}\ntoolbar = {}\nfocus = {}\ntypewriter = {}\nsidebar = {}\ncolors = {}\ndir = {dir}\n",
            self.palette.key(),
            self.density.key(),
            self.font,
            self.heading_font,
            self.size,
            self.divider.key(),
            if self.initial { "large" } else { "plain" },
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
            size: 18,
            width: Width::Wide,
            sort: Sort::Title,
            divider: Divider::Flourish,
            initial: true,
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
        // A file from before the letter's styles: three lines, the
        // headings' face, the accent; and a height out of range held in.
        let old = Settings::parse("initial = large\n");
        assert_eq!((old.initial, old.initial_lines, old.initial_font.as_str(), old.initial_accent), (true, 3, "", true));
        assert_eq!(Settings::parse("initial_lines = 9\n").initial_lines, 4);
        assert_eq!(Settings::parse("initial_lines = 1\n").initial_lines, 2);
    }
}
