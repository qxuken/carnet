//! How Carnet looks, this frame: the design system's tokens
//! (`crate::design`) for the palette and mode the settings chose, pinned
//! as the kui theme so the stock pieces — the caret, selections,
//! scrollbars, tooltips — wear them too, and the faces the page is set in.

use std::collections::HashSet;

use kui_native::{Appearance, Color, Core, FontFamily, TextStyle, Theme, Tokens, Ui};

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
    /// Code's, a block or a word of it: the system's monospace (kui's
    /// `Mono`: SF Mono or Menlo on a Mac, Cascadia Mono or Consolas on
    /// Windows, DejaVu Sans Mono or Liberation Mono elsewhere) unless one
    /// is chosen.
    pub code: FontFamily,
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

/// The faces by kui's family tokens, declared again only when the chosen
/// ones change; the palette follows the settings, or the OS, each frame.
#[derive(Default)]
pub struct Styler {
    /// The colours pinned as kui's theme: re-pinned only when they change.
    pinned: Option<Colors>,
    /// The faces chosen when the tokens were last declared: the text's,
    /// the headings', a dropped letter's and code's.
    declared: Option<[String; 4]>,
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

/// The installed families code can be set in, for code's font list: those
/// whose every face says it is fixed-pitch and that set letters so, ten
/// i's as wide as ten M's. A face can say it is fixed-pitch and have no
/// letters at all — Noto Color Emoji on Linux — and code chosen in it
/// was drawn in the proportional face the letters fell back to. Asks
/// each fixed-pitch family once (its file loaded, a line shaped), so the
/// list keeps the answer. The hidden families, `.`- or `@`-named, never
/// show in a list and are not asked.
pub fn code_families(core: &mut Core) -> HashSet<String> {
    let style = TextStyle::new(16.0);
    core.system_fonts()
        .into_iter()
        .filter(|f| f.monospaced && !f.family.starts_with('.') && !f.family.starts_with('@'))
        .filter(|f| {
            core.add_system_font(&f.family).is_some_and(|id| {
                let style = style.family(FontFamily::Custom(id));
                let i = core.measure_text("iiiiiiiiii", &style, None).width;
                let m = core.measure_text("MMMMMMMMMM", &style, None).width;
                i > 0.0 && (i - m).abs() < 0.5
            })
        })
        .map(|f| f.family)
        .collect()
}

/// The page's faces as kui's family tokens (ADR 0049): each the face
/// chosen in Settings, then Carnet's own, then a stock family. kui takes
/// the first that is installed or loaded, and takes again when the fonts
/// change, so a face installed while Carnet runs is used as it comes. A
/// dropped letter's falls back to the headings', whichever won there.
fn faces(s: &Settings) -> Tokens {
    let chosen = |name: &str| (!name.is_empty()).then(|| name.to_string());
    let heading: Vec<String> = chosen(&s.heading_font)
        .into_iter()
        .chain(std::iter::once(HEADING_FACE).chain(SERIFS).map(String::from))
        .chain(["serif".to_string()])
        .collect();
    let initial: Vec<String> = chosen(&s.initial_font).into_iter().chain(heading.iter().cloned()).collect();
    Tokens::new()
        .family("body", chosen(&s.font).into_iter().chain([BODY_FACE.to_string(), "sans".to_string()]))
        .family("heading", heading)
        .family("initial", initial)
        .family("code", chosen(&s.code_font).into_iter().chain(["mono".to_string()]))
}

impl Styler {
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
        let chosen = [s.font.clone(), s.heading_font.clone(), s.initial_font.clone(), s.code_font.clone()];
        if self.declared.as_ref() != Some(&chosen) {
            ui.set_tokens(faces(s));
            self.declared = Some(chosen);
        }
        let mut face = |name: &str, stock: FontFamily| ui.token_family(name).unwrap_or(stock);
        let serif = face("heading", FontFamily::Serif);
        let body = face("body", FontFamily::Sans);
        let initial = face("initial", serif);
        let code = face("code", FontFamily::Mono);
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
            code,
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

#[cfg(test)]
mod tests {
    use super::*;
    use kui_native::Size;

    /// The faces are kui's family tokens: Carnet's own unless one is
    /// chosen, a chosen one this machine hasn't got falling back to
    /// Carnet's, a dropped letter in the headings' face unless it has its
    /// own, code in the system's monospace; declared again as the choices
    /// change, and as they were when they change back.
    #[test]
    fn the_faces_are_kuis_family_tokens() {
        let mut core = Core::new();
        let mut styler = Styler::default();
        let mut faces = |core: &mut Core, s: &Settings| {
            let mut ui = core.frame(Size::new(400.0, 300.0), 1.0);
            let l = styler.look(&mut ui, s);
            ui.finish();
            (l.body, l.serif, l.initial, l.code)
        };
        let mut s = Settings::default();
        let first = faces(&mut core, &s);
        let inter = FontFamily::Custom(core.add_system_font(BODY_FACE).expect("Inter, shipped"));
        let source = FontFamily::Custom(core.add_system_font(HEADING_FACE).expect("Source Serif 4, shipped"));
        assert_eq!(first, (inter, source, source, FontFamily::Mono));
        let tokens: Vec<_> = core.tokens().expect("declared").families().iter().map(|(n, t)| (n.as_str(), t.picked())).collect();
        assert_eq!(tokens, [("body", Some(BODY_FACE)), ("heading", Some(HEADING_FACE)), ("initial", Some(HEADING_FACE)), ("code", Some("mono"))]);

        // The headings in the text's face: a dropped letter follows them.
        s.heading_font = BODY_FACE.into();
        s.code_font = HEADING_FACE.into();
        assert_eq!(faces(&mut core, &s), (inter, inter, inter, source));
        // A dropped letter's own; a face not on this machine is Carnet's.
        s.initial_font = HEADING_FACE.into();
        s.font = "No Such Face 2026".into();
        assert_eq!(faces(&mut core, &s), (inter, inter, source, source));
        assert_eq!(faces(&mut core, &Settings::default()), first);
    }
}
