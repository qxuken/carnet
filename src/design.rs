//! Carnet's design system: every colour, size, space and radius the
//! views draw with, as named tokens.
//!
//! **Colour** is a palette — five seeds per mode (page, sidebar, card,
//! ink, accent) — from which every other colour is derived, the text
//! colours *solved* against the surfaces they sit on, so any palette meets
//! the same contrast: muted text 5:1, faint 3.3:1 (placeholders), a
//! control's edge 3:1, accent text and the primary button's label 4.5:1.
//! Five palettes, each light and dark.
//!
//! **Metrics** are two scales: the interface's (fixed, like any app's) and
//! the page's (the reader's text size scales it), plus spacing, radii and
//! sizes. **Density** stretches the page's rhythm and the library's rows.
//!
//! `cargo run -- --tokens` prints every token as JSON (`docs/tokens.json`),
//! which is what the design reference is built from.

use kui_native::{Appearance, Color, Theme};

// ------------------------------------------------------------ palettes

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Palette {
    /// Warm linen and terracotta; espresso by night. The default.
    Linen,
    /// Pale stone and sage green.
    Sage,
    /// Cool paper and harbour blue.
    Harbor,
    /// Blush and rose.
    Blossom,
    /// Neutral greys, the accent nearly ink.
    Graphite,
}

impl Palette {
    pub const ALL: [Palette; 5] = [Palette::Linen, Palette::Sage, Palette::Harbor, Palette::Blossom, Palette::Graphite];

    pub fn name(self) -> &'static str {
        match self {
            Palette::Linen => "Linen",
            Palette::Sage => "Sage",
            Palette::Harbor => "Harbor",
            Palette::Blossom => "Blossom",
            Palette::Graphite => "Graphite",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Palette::Linen => "linen",
            Palette::Sage => "sage",
            Palette::Harbor => "harbor",
            Palette::Blossom => "blossom",
            Palette::Graphite => "graphite",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == s)
    }

    /// The five colours everything else is derived from.
    pub fn seeds(self, dark: bool) -> Seeds {
        let s = |page, side, card, ink, accent| Seeds {
            page: rgb(page),
            side: rgb(side),
            card: rgb(card),
            ink: rgb(ink),
            accent: rgb(accent),
        };
        match (self, dark) {
            (Palette::Linen, false) => s(0xfcfaf7, 0xf5f1ec, 0xffffff, 0x2f2a27, 0xc27c68),
            (Palette::Linen, true) => s(0x1f1b18, 0x171412, 0x2c2622, 0xefe6dc, 0xd9a08c),
            (Palette::Sage, false) => s(0xf8f9f5, 0xeff2ea, 0xffffff, 0x27302a, 0x6f9a7a),
            (Palette::Sage, true) => s(0x1a1e1b, 0x141815, 0x252b26, 0xe5ebe3, 0x9cc3a5),
            (Palette::Harbor, false) => s(0xf7f9fb, 0xedf1f5, 0xffffff, 0x232f3d, 0x5b7fae),
            (Palette::Harbor, true) => s(0x171b21, 0x111419, 0x222833, 0xe3e9f1, 0x8eaedb),
            (Palette::Blossom, false) => s(0xfdf8f9, 0xf7eef0, 0xffffff, 0x3a2a30, 0xc9718c),
            (Palette::Blossom, true) => s(0x201a1c, 0x181314, 0x2d2427, 0xf1e4e8, 0xe09ab0),
            (Palette::Graphite, false) => s(0xfafafa, 0xf1f1f0, 0xffffff, 0x1f1f1f, 0x555555),
            (Palette::Graphite, true) => s(0x18181a, 0x121213, 0x242426, 0xececec, 0xb0b0b0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seeds {
    /// The page the note is written on.
    pub page: Color,
    /// The library's column, a shade off the page.
    pub side: Color,
    /// A card: the open note's row, the settings' groups, menus.
    pub card: Color,
    /// Text.
    pub ink: Color,
    /// The one colour.
    pub accent: Color,
}

fn rgb(v: u32) -> Color {
    Color::hex((v << 8) | 0xff)
}

// ------------------------------------------------------------ colours

/// Every colour a view draws with. Named for what it is for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub dark: bool,
    // Surfaces.
    pub page: Color,
    pub side: Color,
    pub card: Color,
    /// A well: code, table headers, the search pill's, controls' tracks.
    pub sunken: Color,
    /// The search pill, on the library.
    pub pill: Color,
    // Ink.
    pub ink: Color,
    /// Words that matter but are not the point: dates, counts, previews.
    pub muted: Color,
    /// The barely there: placeholders, separators, a done item.
    pub faint: Color,
    // Lines.
    pub border: Color,
    pub border_strong: Color,
    /// An unticked checkbox's ring: a control's edge.
    pub ring: Color,
    // The accent.
    pub accent: Color,
    /// The accent as text (links, a card's folder): deepened to read.
    pub accent_ink: Color,
    /// The primary button's fill, and `on_accent` the word on it.
    pub primary: Color,
    pub on_accent: Color,
    /// A wash of the accent: a chosen tool, a hovered row.
    pub accent_soft: Color,
    pub selection: Color,
    /// The keyboard's focus ring.
    pub focus: Color,
    // States.
    pub hover: Color,
    pub pressed: Color,
    pub shadow: Color,
    pub scrollbar: Color,
    pub scrollbar_active: Color,
    // Meanings.
    pub danger: Color,
    pub success: Color,
    pub warning: Color,
}

/// WCAG contrast of two opaque colours.
pub fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (a.luminance(), b.luminance());
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// `from` moved toward `toward` as far as it can go (up to `max`) while
/// still reaching `target` contrast on every one of `on`.
fn farthest(from: Color, toward: Color, on: &[Color], target: f32, max: f32) -> Color {
    let mut t = max;
    while t > 0.0 {
        let c = from.mix(toward, t);
        if on.iter().all(|&b| contrast(c, b) >= target) {
            return c;
        }
        t -= 0.01;
    }
    from
}

/// `from` moved toward `toward` only as far as it must to reach `target`
/// contrast on every one of `on`.
fn nearest(from: Color, toward: Color, on: &[Color], target: f32) -> Color {
    let mut t = 0.0;
    while t <= 1.0 {
        let c = from.mix(toward, t);
        if on.iter().all(|&b| contrast(c, b) >= target) {
            return c;
        }
        t += 0.01;
    }
    toward
}

impl Colors {
    pub fn of(palette: Palette, dark: bool) -> Self {
        let s = palette.seeds(dark);
        let (page, side, card, ink, accent) = (s.page, s.side, s.card, s.ink, s.accent);
        let surfaces = [page, side, card];
        let away = if dark { Color::WHITE } else { Color::BLACK };
        // The greys are the ink leaning a fifth toward the accent, so a
        // warm palette's greys are warm and a cool one's cool.
        let tone = ink.mix(accent, 0.2);
        let muted = farthest(tone, page, &surfaces, 5.0, 0.8);
        let faint = farthest(tone, page, &[page, side], 3.3, 0.85);
        let ring = farthest(tone, page, &[page], 3.0, 0.85);
        let accent_ink = nearest(accent, away, &surfaces, 4.6);
        // Light: white on the deepened accent. Dark: the page's dark on
        // the accent, lightened until it reads.
        let (primary, on_accent) = if dark {
            (nearest(accent, Color::WHITE, &[page], 4.6), page)
        } else {
            (nearest(accent, Color::BLACK, &[Color::WHITE], 4.6), Color::WHITE)
        };
        // A well: by day, a step past the library's shade of the page; by
        // night, a little toward the card.
        let sunken = if dark { page.mix(card, 0.45) } else { page.mix(side, 1.6) };
        let meaning = |light: u32, dark_v: u32| {
            nearest(rgb(if dark { dark_v } else { light }), away, &[page, card], 4.5)
        };
        Colors {
            dark,
            page,
            side,
            card,
            sunken,
            pill: if dark { card } else { page },
            ink,
            muted,
            faint,
            border: page.mix(tone, if dark { 0.11 } else { 0.08 }),
            border_strong: page.mix(tone, if dark { 0.22 } else { 0.17 }),
            ring,
            accent,
            accent_ink,
            primary,
            on_accent,
            accent_soft: accent.with_alpha(if dark { 0.16 } else { 0.12 }),
            selection: accent.with_alpha(if dark { 0.30 } else { 0.22 }),
            focus: accent_ink,
            hover: ink.with_alpha(if dark { 0.05 } else { 0.045 }),
            pressed: ink.with_alpha(if dark { 0.09 } else { 0.08 }),
            shadow: if dark {
                Color::rgba(0.0, 0.0, 0.0, 0.35)
            } else {
                ink.mix(accent, 0.3).with_alpha(0.08)
            },
            scrollbar: ink.with_alpha(if dark { 0.12 } else { 0.14 }),
            scrollbar_active: ink.with_alpha(if dark { 0.26 } else { 0.30 }),
            danger: meaning(0xa8483f, 0xe08a80),
            success: meaning(0x5f7d5a, 0x9fb89a),
            warning: meaning(0x8a6a35, 0xd8b27a),
        }
    }

    /// kui's theme, so the stock pieces — caret, selections, scrollbars,
    /// tooltips, menus — wear the palette too.
    pub fn theme(&self) -> Theme {
        Theme {
            appearance: if self.dark { Appearance::Dark } else { Appearance::Light },
            bg: self.page,
            surface: self.card,
            raised: self.card,
            sunken: self.sunken,
            border: self.border,
            border_strong: self.border_strong,
            fg: self.ink,
            muted: self.muted,
            faint: self.faint,
            accent: self.accent,
            accent_hover: self.accent.mix(if self.dark { Color::WHITE } else { Color::BLACK }, 0.08),
            accent_pressed: self.accent.mix(Color::BLACK, 0.10),
            on_accent: self.on_accent,
            accent_soft: self.accent_soft,
            selection: self.selection,
            focus_ring: self.focus,
            hover: self.hover,
            pressed: self.pressed,
            disabled_opacity: DISABLED_OPACITY,
            success: self.success,
            warning: self.warning,
            danger: self.danger,
            scrollbar: self.scrollbar,
            scrollbar_active: self.scrollbar_active,
        }
    }

    /// Every token by name, for the reference.
    pub fn named(&self) -> Vec<(&'static str, Color)> {
        vec![
            ("page", self.page),
            ("side", self.side),
            ("card", self.card),
            ("sunken", self.sunken),
            ("pill", self.pill),
            ("ink", self.ink),
            ("muted", self.muted),
            ("faint", self.faint),
            ("border", self.border),
            ("border-strong", self.border_strong),
            ("ring", self.ring),
            ("accent", self.accent),
            ("accent-ink", self.accent_ink),
            ("primary", self.primary),
            ("on-accent", self.on_accent),
            ("accent-soft", self.accent_soft),
            ("selection", self.selection),
            ("focus", self.focus),
            ("hover", self.hover),
            ("pressed", self.pressed),
            ("shadow", self.shadow),
            ("danger", self.danger),
            ("success", self.success),
            ("warning", self.warning),
        ]
    }
}

/// How faded a control is when it does nothing.
pub const DISABLED_OPACITY: f32 = 0.45;

/// The colours words can be given, as the picker lays them out — the
/// grid Word and Obsidian's editing toolbar share: ten theme colours with
/// five lighter and darker steps of each under them, then ten standard
/// ones. Values are 0xRRGGBBAA.
pub mod tint {
    use super::rgb;
    use kui_native::Color;

    /// A colour the reader can give words: a token, written in the file
    /// as its light-theme colour so any Markdown viewer shows it, and
    /// drawn here in the colour each theme has for it.
    pub struct Hue {
        pub name: &'static str,
        /// The words' colour on paper (0xRRGGBB): a quiet mid-tone, 3:1 or
        /// better on every light page. What the file says.
        pub ink: u32,
        /// The highlighter's wash on paper (0xRRGGBBAA): a soft pastel, a
        /// little see-through, as Obsidian's highlighters are. What the
        /// file says.
        pub wash: u32,
        /// The same hue in the dark (0xRRGGBB): the words lighter, 7:1 or
        /// better on every dark page; the wash a deep tint of the hue that
        /// the text reads on at 7:1, and every hue's words at 3.5:1.
        pub ink_dark: u32,
        pub wash_dark: u32,
    }

    const fn hue(name: &'static str, ink: u32, wash: u32, ink_dark: u32, wash_dark: u32) -> Hue {
        Hue { name, ink, wash: (wash << 8) | 0xcc, ink_dark, wash_dark }
    }

    /// The colours the picker offers: ten, as calm as the page.
    pub const HUES: [Hue; 10] = [
        hue("Grey", 0x857c76, 0xe6e1dc, 0xb8afa8, 0x3b3633),
        hue("Brown", 0x9a6f4f, 0xefdcc8, 0xd0a27c, 0x4b3828),
        hue("Red", 0xc0553f, 0xf8cfc4, 0xf0907a, 0x5c2d26),
        hue("Orange", 0xc4762f, 0xf9dbb6, 0xf0a866, 0x5c3d20),
        hue("Yellow", 0xa98a1c, 0xf8eba6, 0xe3c45e, 0x57491b),
        hue("Green", 0x5f8f4a, 0xd8eac0, 0x9cc884, 0x30462a),
        hue("Teal", 0x3a8c86, 0xc4e8e1, 0x72c7bd, 0x1f4642),
        hue("Blue", 0x5279b3, 0xcddcf3, 0x94b5ec, 0x263856),
        hue("Purple", 0x8d68ad, 0xe2d5f0, 0xc2a0e2, 0x403256),
        hue("Pink", 0xbb5a85, 0xf5cfe0, 0xec95ba, 0x562c40),
    ];

    /// The hue a colour from a file is (0xRRGGBBAA), when it is one of
    /// ours: its ink or its wash, as written.
    fn hue_of(c: u32) -> Option<&'static Hue> {
        HUES.iter().find(|h| (h.ink << 8) | 0xff == c || h.wash == c)
    }

    /// The name the picker gives colour `c` (0xRRGGBBAA): a hue's, else
    /// its CSS spelling.
    pub fn name(c: u32) -> String {
        hue_of(c).map_or_else(|| carnet_core::inline::css_color(c), |h| h.name.to_string())
    }

    /// Words' colour `c` (0xRRGGBBAA, as the file has it) as the page
    /// shows it: one of the hues in this theme's colour, any other as
    /// written.
    pub fn ink(c: u32, dark: bool) -> Color {
        match hue_of(c) {
            Some(h) if dark => rgb(h.ink_dark),
            _ => Color::hex(c),
        }
    }

    /// The highlighter `==…==` is: a yellow wash, pale on paper and dim
    /// in the dark.
    pub fn highlight(dark: bool) -> Color {
        rgb(if dark { 0x5a4d17 } else { 0xfcefa1 })
    }

    /// A highlighter's wash (0xRRGGBBAA, as the file has it) as the page
    /// shows it: one of the hues in this theme's colour; any other itself
    /// on paper and, in the dark, its hue sunk most of the way into the
    /// page, so a pastel from another app is a glow and not a lit block.
    pub fn wash(c: u32, dark: bool, page: Color) -> Color {
        match hue_of(c) {
            Some(h) if dark => rgb(h.wash_dark),
            _ if dark => Color::hex(c).with_alpha(1.0).mix(page, 0.8),
            _ => Color::hex(c),
        }
    }
}

// ------------------------------------------------------------ metrics

/// A size and its line height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Type {
    pub size: f32,
    pub line: f32,
}

const fn ty(size: f32, line: f32) -> Type {
    Type { size, line }
}

impl Type {
    // The interface's type: fixed sizes, as in any app.
    /// Dates and counts.
    pub const CAPTION: Type = ty(11.0, 15.0);
    /// Previews, hints, notes under a setting, the meta line.
    pub const SMALL: Type = ty(12.5, 17.0);
    /// Controls and rows: buttons, menus, folders, search, settings.
    pub const UI: Type = ty(13.5, 19.0);
    /// A card's title, a setting's name.
    pub const STRONG: Type = ty(14.0, 19.0);
    /// The library's heading.
    pub const HEADING: Type = ty(20.0, 26.0);
    /// A page's title: the settings'.
    pub const DISPLAY: Type = ty(36.0, 46.0);

    // The page's type at 16px; the reader's size scales it (`Look::px`),
    // a step of about 1.2 from the body up.
    pub const BODY: Type = ty(16.0, 27.0);
    pub const H1: Type = ty(30.0, 40.0);
    pub const H2: Type = ty(24.0, 32.0);
    pub const H3: Type = ty(19.0, 28.0);
    /// A note's title.
    pub const TITLE: Type = ty(36.0, 46.0);
    pub const CODE: Type = ty(13.5, 21.0);
    /// A table's header row.
    pub const LABEL: Type = ty(12.5, 18.0);
}
/// Space, in one ladder: every gap and padding is one of these.
pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
    pub const XXXL: f32 = 48.0;
}

/// Corners: a row or control is `CONTROL`, a container `CARD`; a dot or a
/// pill is round.
pub mod radius {
    /// A selection's wash, a code span.
    pub const TIGHT: f32 = 4.0;
    /// Rows and controls up to 34px tall: tools, menu rows, segments.
    pub const CONTROL: f32 = 8.0;
    /// Containers: cards, menus, pictures, tables, code.
    pub const CARD: f32 = 12.0;
    pub const ROUND: f32 = 999.0;
}

/// Fixed sizes.
pub mod size {
    pub const ICON_S: f32 = 14.0;
    pub const ICON: f32 = 16.0;
    pub const ICON_L: f32 = 18.0;
    /// A tool's square.
    pub const CONTROL: f32 = 30.0;
    /// A row: a folder, a menu item, a footer button.
    pub const ROW: f32 = 34.0;
    /// The titlebar strip over the page and the library: a full macOS
    /// toolbar's height (kui's `Titlebar::Tall`), the traffic lights
    /// centred in it.
    pub const STRIP: f32 = 52.0;
    pub const SIDEBAR: f32 = 268.0;
    /// Where the library's text starts, from the window's edge.
    pub const GUTTER: f32 = 26.0;
    /// A note's card in the library, at the default density.
    pub const LIBRARY_ROW: f32 = 68.0;
    /// The page's padding, wide and narrow.
    pub const PAGE_PAD: f32 = 48.0;
    pub const PAGE_PAD_NARROW: f32 = 18.0;
}

// ------------------------------------------------------------ alternatives

/// How much air: the page's rhythm between blocks and the library's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Compact,
    Comfortable,
    Airy,
}

impl Density {
    /// A note's card in the library, at a density factor: its text (a
    /// title over a preview, 39px, never squeezed) with the air around
    /// it stretched or shrunk — Comfortable is `size::LIBRARY_ROW`.
    pub fn library_row(d: f32) -> f32 {
        const TEXT: f32 = 19.0 + 3.0 + 17.0;
        // 3px between cards, 4px inside a card above and below.
        const FIXED: f32 = TEXT + 2.0 * 3.0 + 2.0 * 4.0;
        let air = (size::LIBRARY_ROW - FIXED) / 2.0;
        (FIXED + 2.0 * (air * (1.0 + (d - 1.0) * 2.5)).max(2.0)).round()
    }

    pub fn factor(self) -> f32 {
        match self {
            Density::Compact => 0.8,
            Density::Comfortable => 1.0,
            Density::Airy => 1.25,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Density::Compact => "compact",
            Density::Comfortable => "comfortable",
            Density::Airy => "airy",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        [Density::Compact, Density::Comfortable, Density::Airy]
            .into_iter()
            .find(|d| d.key() == s)
    }
}

/// The faces' pairings offered first: (name, text face, heading face);
/// an empty face is the shipped default.
pub const PAIRINGS: [(&str, &str, &str); 4] = [
    ("Book", "", ""),
    ("Modern", "", "Inter"),
    ("Classic", "Source Serif 4", ""),
    ("System", "Segoe UI", "Georgia"),
];

// ------------------------------------------------------------ export

fn hex(c: Color) -> String {
    let v = c.to_hex();
    if v & 0xff == 0xff {
        format!("#{:06x}", v >> 8)
    } else {
        format!("#{v:08x}")
    }
}

/// Every token as JSON: palettes in both modes, the type scales, space,
/// radii, sizes, densities and pairings.
pub fn tokens_json() -> String {
    let mut out = String::from("{\n  \"palettes\": {\n");
    for (pi, p) in Palette::ALL.iter().enumerate() {
        out.push_str(&format!("    \"{}\": {{\n", p.key()));
        for (mi, dark) in [false, true].iter().enumerate() {
            let c = Colors::of(*p, *dark);
            out.push_str(&format!("      \"{}\": {{", if *dark { "dark" } else { "light" }));
            let fields: Vec<String> = c.named().iter().map(|(n, v)| format!("\"{n}\": \"{}\"", hex(*v))).collect();
            out.push_str(&fields.join(", "));
            out.push('}');
            out.push_str(if mi == 0 { ",\n" } else { "\n" });
        }
        out.push_str(if pi + 1 < Palette::ALL.len() { "    },\n" } else { "    }\n" });
    }
    out.push_str("  },\n");
    let t = |name: &str, v: Type| format!("\"{name}\": [{}, {}]", v.size, v.line);
    out.push_str(&format!(
        "  \"type\": {{\"ui\": {{{}, {}, {}, {}, {}, {}}}, \"page\": {{{}, {}, {}, {}, {}, {}, {}}}}},\n",
        t("caption", Type::CAPTION),
        t("small", Type::SMALL),
        t("ui", Type::UI),
        t("strong", Type::STRONG),
        t("heading", Type::HEADING),
        t("display", Type::DISPLAY),
        t("body", Type::BODY),
        t("h1", Type::H1),
        t("h2", Type::H2),
        t("h3", Type::H3),
        t("title", Type::TITLE),
        t("code", Type::CODE),
        t("label", Type::LABEL),
    ));
    out.push_str(&format!(
        "  \"space\": {{\"xxs\": {}, \"xs\": {}, \"s\": {}, \"m\": {}, \"l\": {}, \"xl\": {}, \"xxl\": {}, \"xxxl\": {}}},\n",
        space::XXS, space::XS, space::S, space::M, space::L, space::XL, space::XXL, space::XXXL
    ));
    out.push_str(&format!(
        "  \"radius\": {{\"tight\": {}, \"control\": {}, \"card\": {}, \"round\": {}}},\n",
        radius::TIGHT, radius::CONTROL, radius::CARD, radius::ROUND
    ));
    out.push_str(&format!(
        "  \"size\": {{\"icon-s\": {}, \"icon\": {}, \"icon-l\": {}, \"control\": {}, \"row\": {}, \"strip\": {}, \"sidebar\": {}, \"gutter\": {}, \"library-row\": {}, \"page-pad\": {}, \"page-pad-narrow\": {}}},\n",
        size::ICON_S, size::ICON, size::ICON_L, size::CONTROL, size::ROW, size::STRIP, size::SIDEBAR, size::GUTTER, size::LIBRARY_ROW, size::PAGE_PAD, size::PAGE_PAD_NARROW
    ));
    out.push_str(&format!(
        "  \"density\": {{\"compact\": {}, \"comfortable\": {}, \"airy\": {}}},\n",
        Density::Compact.factor(),
        Density::Comfortable.factor(),
        Density::Airy.factor()
    ));
    out.push_str(&format!("  \"disabled-opacity\": {DISABLED_OPACITY},\n"));
    let pairs: Vec<String> = PAIRINGS
        .iter()
        .map(|(n, body, head)| {
            let body = if body.is_empty() { "Inter" } else { body };
            let head = if head.is_empty() { "Source Serif 4" } else { head };
            format!("\"{n}\": {{\"text\": \"{body}\", \"headings\": \"{head}\"}}")
        })
        .collect();
    out.push_str(&format!("  \"pairings\": {{{}}}\n}}\n", pairs.join(", ")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A history's marks are three of the hues: carnet-core names them as
    /// a file would, and they must stay the Green and Yellow washes and
    /// the Red ink, so each theme draws them in its own colours.
    #[test]
    fn the_history_marks_are_hues() {
        let hue = |name: &str| tint::HUES.iter().find(|h| h.name == name).unwrap();
        assert_eq!(carnet_core::compare::ADDED, hue("Green").wash);
        assert_eq!(carnet_core::compare::CHANGED, hue("Yellow").wash);
        assert_eq!(carnet_core::compare::REMOVED, (hue("Red").ink << 8) | 0xff);
    }

    /// Every palette, in both modes, keeps the contrast the tokens promise.
    #[test]
    fn every_palette_reads() {
        for p in Palette::ALL {
            for dark in [false, true] {
                let c = Colors::of(p, dark);
                let at = |what: &str, fg: Color, bg: Color, min: f32| {
                    let r = contrast(fg, bg);
                    assert!(r >= min, "{} {}: {what} {r:.2} < {min}", p.name(), if dark { "dark" } else { "light" });
                };
                for bg in [c.page, c.side, c.card] {
                    at("ink", c.ink, bg, 10.0);
                    at("muted", c.muted, bg, 5.0);
                    at("accent text", c.accent_ink, bg, 4.5);
                }
                at("faint on page", c.faint, c.page, 3.3);
                at("faint on side", c.faint, c.side, 3.3);
                at("checkbox ring", c.ring, c.page, 3.0);
                at("primary label", c.on_accent, c.primary, 4.5);
                at("danger", c.danger, c.page, 4.5);
                // Faint stays fainter than muted: two steps, not one.
                assert!(contrast(c.faint, c.page) < contrast(c.muted, c.page));
            }
        }
    }

    #[test]
    fn a_library_row_always_holds_its_text() {
        let comfortable = Density::library_row(Density::Comfortable.factor());
        assert_eq!(comfortable, size::LIBRARY_ROW);
        let compact = Density::library_row(Density::Compact.factor());
        let airy = Density::library_row(Density::Airy.factor());
        assert!(compact < comfortable && comfortable < airy, "{compact} {comfortable} {airy}");
        // Title, gap and preview (39) plus the 3px and 4px edges fit.
        assert!(compact >= 39.0 + 6.0 + 8.0, "{compact}");
    }

    #[test]
    fn tokens_export_names_every_palette() {
        let j = tokens_json();
        for p in Palette::ALL {
            assert!(j.contains(&format!("\"{}\"", p.key())));
        }
        assert!(j.contains("\"accent-ink\""));
        assert!(j.trim_end().ends_with('}'));
    }
}
