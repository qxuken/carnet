//! The `/` menu: a line that is nothing but `/` and a word offers the
//! blocks that word could mean — `/ta` offers a table, `/h` the headings —
//! and Enter (or a click) makes the line one. The commands, which lines
//! offer them and what each makes are carnet-core's
//! ([`carnet_core::slash`]), shared with the web editor; here, each one's
//! glyph.

pub use carnet_core::slash::{apply, matches, query, Action, COMMANDS};
use crate::icons::Glyph;

/// Each command's glyph, in [`COMMANDS`]' order.
pub const GLYPHS: [Glyph; COMMANDS.len()] = [
    Glyph::H1,
    Glyph::H2,
    Glyph::H3,
    Glyph::Bullets,
    Glyph::Numbers,
    Glyph::Todo,
    Glyph::Quote,
    Glyph::Table,
    Glyph::Divider,
    Glyph::Code,
    Glyph::Picture,
    Glyph::Attach,
];
