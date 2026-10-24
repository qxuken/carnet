//! Every shortcut, in one table, written as kui spells them (`mod` is Cmd
//! on a Mac and Ctrl elsewhere) — and shown as the platform writes them
//! (`⌥⌘1`, `Ctrl+Alt+1`), so a tooltip never says Ctrl on a Mac.

use kui_native::{Accel, KeyCode, KeyPress};

// The block under the caret becomes…
pub const TEXT: &str = "mod+alt+0";
pub const H1: &str = "mod+alt+1";
pub const H2: &str = "mod+alt+2";
pub const H3: &str = "mod+alt+3";
pub const TODO: &str = "mod+alt+4";
pub const BULLETS: &str = "mod+alt+5";
pub const NUMBERS: &str = "mod+alt+6";
pub const QUOTE: &str = "mod+alt+7";
/// A paragraph's first letter dropped, or not: beside the block kinds.
pub const INITIAL: &str = "mod+alt+8";
// Inserting.
pub const TABLE: &str = "mod+alt+t";
pub const DIVIDER: &str = "mod+alt+-";
// Inline styles.
pub const BOLD: &str = "mod+b";
pub const ITALIC: &str = "mod+i";
pub const STRIKE: &str = "mod+shift+x";
pub const UNDERLINE: &str = "mod+u";
pub const CODE: &str = "mod+e";
pub const LINK: &str = "mod+k";
pub const HIGHLIGHT: &str = "mod+shift+h";
/// lowercase, UPPERCASE, Capitalize Each Word, round again: Word's key.
pub const CHANGE_CASE: &str = "shift+f3";
// Pictures.
pub const FLOAT_LEFT: &str = "mod+alt+l";
pub const FLOAT_RIGHT: &str = "mod+alt+r";
// Blocks.
pub const MOVE_UP: &str = "alt+up";
pub const MOVE_DOWN: &str = "alt+down";
pub const INDENT: &str = "tab";
pub const OUTDENT: &str = "shift+tab";
pub const DUPLICATE: &str = "mod+d";
pub const REMOVE: &str = "mod+shift+k";
pub const TICK: &str = "mod+enter";
// In a table's cell, the block's move with Shift: its row; with the chord
// too, its column — Alt+Shift+←→ is selecting by word, Option+Shift on a
// Mac, and stays that in a cell.
pub const ROW_UP: &str = "alt+shift+up";
pub const ROW_DOWN: &str = "alt+shift+down";
pub const COL_LEFT: &str = "mod+alt+shift+left";
pub const COL_RIGHT: &str = "mod+alt+shift+right";
// Moving around.
pub const TOP: &str = "mod+up";
pub const BOTTOM: &str = "mod+down";
pub const PREVIOUS_NOTE: &str = "mod+alt+up";
pub const NEXT_NOTE: &str = "mod+alt+down";
// Notes.
pub const NEW_NOTE: &str = "mod+n";
pub const NEW_FOLDER: &str = "mod+shift+n";
/// On a folder in the list, or the one shown after a click on it.
pub const RENAME_FOLDER: &str = "f2";
pub const SEARCH: &str = "mod+f";
pub const PIN: &str = "mod+shift+p";
pub const TRASH: &str = "mod+shift+backspace";
pub const SETTINGS: &str = "mod+,";
pub const SIDEBAR: &str = "mod+\\";
pub const ZEN: &str = "mod+.";
pub const UNDO: &str = "mod+z";
pub const REDO: &str = "mod+shift+z";
pub const SAVE: &str = "mod+s";
pub const PASTE: &str = "mod+v";

/// Every shortcut with what it does, in the order the settings list them.
pub const ALL: [(&str, &str); 49] = [
    ("New note", NEW_NOTE),
    ("New folder", NEW_FOLDER),
    ("Rename folder", RENAME_FOLDER),
    ("Search", SEARCH),
    ("Previous note", PREVIOUS_NOTE),
    ("Next note", NEXT_NOTE),
    ("Pin or unpin", PIN),
    ("Move note to Deleted", TRASH),
    ("Settings", SETTINGS),
    ("Show or hide the notes", SIDEBAR),
    ("Zen mode", ZEN),
    ("Save now", SAVE),
    ("Undo", UNDO),
    ("Redo", REDO),
    ("Top of the note", TOP),
    ("Bottom of the note", BOTTOM),
    ("Text", TEXT),
    ("Heading 1", H1),
    ("Heading 2", H2),
    ("Heading 3", H3),
    ("Checklist", TODO),
    ("Bulleted list", BULLETS),
    ("Numbered list", NUMBERS),
    ("Quote", QUOTE),
    ("Drop the first letter", INITIAL),
    ("Tick or untick", TICK),
    ("Indent, or a tab in text", INDENT),
    ("Outdent, or a tab taken back", OUTDENT),
    ("Move block up", MOVE_UP),
    ("Move block down", MOVE_DOWN),
    ("Duplicate block", DUPLICATE),
    ("Remove block", REMOVE),
    ("Table", TABLE),
    ("Move table row up", ROW_UP),
    ("Move table row down", ROW_DOWN),
    ("Move table column left", COL_LEFT),
    ("Move table column right", COL_RIGHT),
    ("Divider", DIVIDER),
    ("Bold", BOLD),
    ("Italic", ITALIC),
    ("Strikethrough", STRIKE),
    ("Underline", UNDERLINE),
    ("Code", CODE),
    ("Link", LINK),
    ("Highlight", HIGHLIGHT),
    ("Change case", CHANGE_CASE),
    ("Paste a picture", PASTE),
    ("Picture left of the text", FLOAT_LEFT),
    ("Picture right of the text", FLOAT_RIGHT),
];

/// A shortcut as this platform writes it.
pub fn label(spec: &str) -> String {
    Accel::parse(spec).map_or_else(|| spec.to_string(), |a| a.display())
}

/// "Bold  ·  ⌘B": a tooltip naming a thing and its shortcut.
pub fn hint(what: &str, spec: &str) -> String {
    format!("{what}  \u{b7}  {}", label(spec))
}

/// The key a chord means: its character when that is a Latin one, else
/// the key where it sits — so Ctrl+С on a Cyrillic layout is Ctrl+C, and
/// Option+1 on a Mac (which types "¡") is still Option+1.
pub fn chord_code(k: &KeyPress) -> KeyCode {
    match k.code {
        KeyCode::Char(c) if c.is_ascii() => KeyCode::Char(c.to_ascii_lowercase()),
        KeyCode::Char(_) => match k.physical {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_parses() {
        for (what, spec) in ALL {
            assert!(Accel::parse(spec).is_some(), "{what}: {spec}");
        }
        // No two actions share a key.
        for (i, (a, x)) in ALL.iter().enumerate() {
            for (b, y) in &ALL[i + 1..] {
                assert_ne!(x, y, "{a} and {b}");
            }
        }
        assert!(hint("Bold", BOLD).ends_with('B'));
    }
}
