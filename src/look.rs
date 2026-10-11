//! How Carnet looks, this frame: the design system's tokens
//! (`crate::design`) for the palette and mode the settings chose, pinned
//! as the kui theme so the stock pieces — the caret, selections,
//! scrollbars, tooltips — wear them too, and the faces the page is set in.

use std::collections::HashMap;

use kui_native::{Appearance, Color, FontFamily, Theme, Ui};

use crate::design::{self, Colors};
use crate::settings::{Settings, ThemePref};

/// Everything a view paints with: the palette's colours (and kui's roles
/// over them), the faces, and the reader's scale and density.
#[derive(Clone, Copy)]
pub struct Look {
    pub t: Theme,
    /// Every colour token, by what it is for.
    pub c: Colors,
    /// The page's rhythm and the library's rows: 1 is comfortable.
    pub d: f32,
    /// The library's column, a shade off the page.
    pub side: Color,
    /// A card resting on the library: the open note's row.
    pub card: Color,
    pub shadow: Color,
    /// The search pill: a step off the library column it sits on.
    pub pill: Color,
    /// An unticked checkbox's ring: 3:1 on the page, the least a control's
    /// edge may be.
    pub ring: Color,
    /// Titles and headings.
    pub serif: FontFamily,
    /// The body's face.
    pub body: FontFamily,
    /// A dropped first letter's face: the headings' unless one is chosen.
    pub initial: FontFamily,
    /// The body's size over the 16px everything was drawn at: headings,
    /// tables and the title scale by it.
    pub k: f32,
    /// The page's width, padding included: the setting's, in px.
    pub page_w: f32,
}

impl Look {
    /// `px` at the reader's size.
    pub fn px(&self, px: f32) -> f32 {
        (px * self.k).round()
    }

    /// A gap in the page's rhythm, at the density chosen and the reader's
    /// size: smaller text, closer blocks.
    pub fn air(&self, px: f32) -> f32 {
        (px * self.d * self.k).round()
    }

    /// A page type token at the reader's size.
    pub fn page_type(&self, t: design::Type) -> kui_native::TextStyle {
        kui_native::TextStyle::new(self.px(t.size)).line_height(self.px(t.line))
    }

}

/// Resolves each face once, by name; the palette follows the settings, or
/// the OS, each frame.
#[derive(Default)]
pub struct Styler {
    /// The colours pinned as kui's theme: re-pinned only when they change.
    pinned: Option<Colors>,
    faces: HashMap<String, FontFamily>,
    /// The shipped faces are registered with kui.
    shipped: bool,
}

/// The faces Carnet ships (`assets/fonts`, under the SIL Open Font
/// License): the same page on every machine. Source Serif 4 for titles
/// and headings — lining figures, so a "4" in a heading sits on the line
/// as Georgia's old-style one does not — and Inter for the text. Four
/// styles each, so bold and italic are the face's own, not faked.
const SHIPPED: [&[u8]; 8] = [
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Italic.ttf"),
    include_bytes!("../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../assets/fonts/Inter-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/SourceSerif4-Regular.ttf"),
    include_bytes!("../assets/fonts/SourceSerif4-It.ttf"),
    include_bytes!("../assets/fonts/SourceSerif4-Semibold.ttf"),
    include_bytes!("../assets/fonts/SourceSerif4-SemiboldIt.ttf"),
];
pub const BODY_FACE: &str = "Inter";
pub const HEADING_FACE: &str = "Source Serif 4";

/// The heading face if the shipped one can't be loaded: the first of
/// these installed.
const SERIFS: [&str; 4] = ["Iowan Old Style", "Charter", "Georgia", "Cambria"];

impl Styler {
    /// A face by family name; `fallback` when it is not installed.
    fn face(&mut self, ui: &mut Ui<'_>, name: &str, fallback: FontFamily) -> FontFamily {
        if name.is_empty() {
            return fallback;
        }
        *self
            .faces
            .entry(name.to_string())
            .or_insert_with(|| ui.system_font(name).map_or(fallback, FontFamily::Custom))
    }

    pub fn look(&mut self, ui: &mut Ui<'_>, s: &Settings) -> Look {
        // `CARNET_THEME=light|dark` pins one; then the settings; then the
        // OS decides.
        let dark = match std::env::var("CARNET_THEME").as_deref() {
            Ok("light") => false,
            Ok("dark") => true,
            _ => match s.theme {
                ThemePref::Light => false,
                ThemePref::Dark => true,
                ThemePref::System => ui.env().system.appearance == Appearance::Dark,
            },
        };
        let c = Colors::of(s.palette, dark);
        if self.pinned != Some(c) {
            ui.core().set_theme(c.theme());
            self.pinned = Some(c);
        }
        if !std::mem::replace(&mut self.shipped, true) {
            for data in SHIPPED {
                // A pointer file instead of a font is skipped: the
                // fallbacks below take over.
                let _ = ui.core().add_font_data(data.to_vec());
            }
        }
        let default_serif = match self.faces.get("") {
            Some(f) => *f,
            None => {
                let f = std::iter::once(&HEADING_FACE)
                    .chain(SERIFS.iter())
                    .find_map(|name| ui.system_font(name))
                    .map_or(FontFamily::Serif, FontFamily::Custom);
                self.faces.insert(String::new(), f);
                f
            }
        };
        let default_body = match self.faces.get(" body") {
            Some(f) => *f,
            None => {
                let f = ui.system_font(BODY_FACE).map_or(FontFamily::Sans, FontFamily::Custom);
                self.faces.insert(" body".to_string(), f);
                f
            }
        };
        let serif = self.face(ui, &s.heading_font, default_serif);
        let body = self.face(ui, &s.font, default_body);
        let initial = self.face(ui, &s.initial_font, serif);
        let t = ui.theme();
        Look {
            t,
            c,
            d: s.density.factor(),
            side: c.side,
            card: c.card,
            pill: c.pill,
            ring: c.ring,
            shadow: c.shadow,
            serif,
            body,
            initial,
            k: s.size as f32 / 16.0,
            // As wide as chosen, whatever the text's size: smaller text
            // doesn't narrow the page, nor bigger text widen it.
            page_w: s.width.px(),
        }
    }
}

/// Faces worth offering first, when installed: book faces, then the
/// clean sans ones, then a monospace or two.
pub const SUGGESTED_FONTS: [&str; 18] = [
    "Iowan Old Style",
    "Charter",
    "Georgia",
    "Cambria",
    "Palatino Linotype",
    "Book Antiqua",
    "Literata",
    "Inter",
    "Segoe UI",
    "SF Pro Text",
    "Helvetica Neue",
    "Avenir Next",
    "Calibri",
    "Aptos",
    "Verdana",
    "IBM Plex Sans",
    "JetBrains Mono",
    "Consolas",
];
