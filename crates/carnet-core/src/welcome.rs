//! The welcome note: what a first open shows, and what Settings' "Add the
//! getting-started note" puts back, on the desktop and the web alike. Its
//! words are the same everywhere; only its shortcuts are written as the
//! platform it is made on writes them (`⌘B` on a Mac, `Ctrl+B` elsewhere),
//! by whoever makes it ([`doc`]'s `key`): the desktop by kui's own names,
//! the web by [`label`].

use crate::doc::{Block, Doc, Kind, ListStyle, Table};

/// Its title, and the name its file has.
pub const TITLE: &str = "Welcome to Carnet";

/// The shortcuts the welcome note names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Bold,
    Italic,
    Highlight,
    Link,
    ChangeCase,
    Top,
    Bottom,
    MoveUp,
    MoveDown,
    RowUp,
    RowDown,
    ColLeft,
    ColRight,
    Copy,
    Undo,
    Paste,
    NewNote,
    NewFolder,
    Search,
    Settings,
}

impl Shortcut {
    /// Every one, in the order the note names them.
    pub const ALL: [Shortcut; 20] = [
        Shortcut::Bold,
        Shortcut::Italic,
        Shortcut::Highlight,
        Shortcut::Link,
        Shortcut::ChangeCase,
        Shortcut::Top,
        Shortcut::Bottom,
        Shortcut::MoveUp,
        Shortcut::MoveDown,
        Shortcut::RowUp,
        Shortcut::RowDown,
        Shortcut::ColLeft,
        Shortcut::ColRight,
        Shortcut::Copy,
        Shortcut::Undo,
        Shortcut::Paste,
        Shortcut::NewNote,
        Shortcut::NewFolder,
        Shortcut::Search,
        Shortcut::Settings,
    ];

    /// The keys, as kui spells them (`mod` is ⌘ on a Mac and Ctrl
    /// elsewhere): the desktop's own (src/keys.rs, which its tests hold
    /// these to).
    pub const fn spec(self) -> &'static str {
        match self {
            Shortcut::Bold => "mod+b",
            Shortcut::Italic => "mod+i",
            Shortcut::Highlight => "mod+shift+h",
            Shortcut::Link => "mod+k",
            Shortcut::ChangeCase => "shift+f3",
            Shortcut::Top => "mod+up",
            Shortcut::Bottom => "mod+down",
            Shortcut::MoveUp => "alt+up",
            Shortcut::MoveDown => "alt+down",
            Shortcut::RowUp => "alt+shift+up",
            Shortcut::RowDown => "alt+shift+down",
            Shortcut::ColLeft => "mod+alt+shift+left",
            Shortcut::ColRight => "mod+alt+shift+right",
            Shortcut::Copy => "mod+c",
            Shortcut::Undo => "mod+z",
            Shortcut::Paste => "mod+v",
            Shortcut::NewNote => "mod+n",
            Shortcut::NewFolder => "mod+shift+n",
            Shortcut::Search => "mod+f",
            Shortcut::Settings => "mod+,",
        }
    }
}

/// A shortcut spelled as kui spells it (`mod+alt+shift+left`), written
/// as kui writes it on a Mac (`mac`: the glyphs in AppKit's order, `⌥⇧⌘←`)
/// or elsewhere (`Ctrl+Alt+Shift+Left`): what a page that can't ask kui
/// shows. One it can't read is as spelled.
pub fn label(spec: &str, mac: bool) -> String {
    let parts: Vec<&str> = spec.split('+').collect();
    let Some((&key, mods)) = parts.split_last() else { return spec.to_string() };
    // `mod+,` and the like: the key is the last part, even a `+`'s neighbour.
    let (mut ctrl, mut alt, mut shift, mut cmd) = (false, false, false, false);
    for m in mods {
        match *m {
            "mod" if mac => cmd = true,
            "mod" | "ctrl" => ctrl = true,
            "cmd" | "super" | "meta" => cmd = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            _ => return spec.to_string(),
        }
    }
    let named = |mac_glyph: &'static str, word: &'static str| if mac { mac_glyph } else { word };
    let key = match key {
        "up" => named("\u{2191}", "Up").to_string(),
        "down" => named("\u{2193}", "Down").to_string(),
        "left" => named("\u{2190}", "Left").to_string(),
        "right" => named("\u{2192}", "Right").to_string(),
        "tab" => named("\u{21e5}", "Tab").to_string(),
        "enter" => named("\u{21a9}", "Enter").to_string(),
        "backspace" => named("\u{232b}", "Backspace").to_string(),
        "delete" => named("\u{2326}", "Delete").to_string(),
        "escape" => named("\u{238b}", "Esc").to_string(),
        "space" => named("\u{2423}", "Space").to_string(),
        k if k.len() > 1 && k.starts_with('f') && k[1..].chars().all(|c| c.is_ascii_digit()) => k.to_ascii_uppercase(),
        k if k.chars().count() == 1 => k.to_ascii_uppercase(),
        _ => return spec.to_string(),
    };
    if mac {
        let mut out = String::new();
        for (on, glyph) in [(ctrl, '\u{2303}'), (alt, '\u{2325}'), (shift, '\u{21e7}'), (cmd, '\u{2318}')] {
            if on {
                out.push(glyph);
            }
        }
        out + &key
    } else {
        let mut out: Vec<&str> = [(ctrl, "Ctrl"), (cmd, "Super"), (alt, "Alt"), (shift, "Shift")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect();
        out.push(&key);
        out.join("+")
    }
}

/// The welcome note's body (its title is [`TITLE`]), each shortcut
/// written by `key`.
pub fn doc(key: impl Fn(Shortcut) -> String) -> Doc {
    let k = key;
    let item = |style, indent, done, text: &str| Block::new(Kind::Item { style, indent, done, num: 0, text: text.into() });
    let heading = |text: &str| Block::new(Kind::Heading { level: 2, text: text.into() });
    let cells = |rows: &[&[&str]]| Table::from_cells(rows.iter().map(|r| r.iter().map(|c| c.to_string()).collect()).collect());
    // Two keys at once, as the platform writes them: "Alt+Shift+↑ / ↓".
    let pair = |a: Shortcut, b: Shortcut| format!("{} / {}", k(a), k(b));
    let (rows, cols, blocks) = (pair(Shortcut::RowUp, Shortcut::RowDown), pair(Shortcut::ColLeft, Shortcut::ColRight), pair(Shortcut::MoveUp, Shortcut::MoveDown));
    Doc::new(vec![
        Block::text(
            "Carnet is a plain notes app: every note is a Markdown file in your Documents folder. \
             It does two things properly: lists and tables.",
        ),
        heading("Type to make blocks"),
        item(ListStyle::Todo, 0, true, "Start a line with \"- \" for a bullet, \"1. \" for numbers, \"[] \" for a checklist"),
        item(ListStyle::Todo, 0, false, "\"# \" makes a heading, \"## \" a smaller one, \"> \" a quote, \"---\" a divider, \"```\" a code block"),
        item(ListStyle::Todo, 0, false, "Enter continues a list; Enter on an empty item steps out of it"),
        item(ListStyle::Todo, 1, false, "Tab indents an item, Shift+Tab brings it back"),
        item(ListStyle::Todo, 0, false, "Enter starts a new paragraph; Shift+Enter a new line in the same one, or in a quote"),
        heading("The / menu"),
        Block::text(
            "On a line of its own — a new paragraph, or a new line after Shift+Enter — type / and a word, \
             and the kinds of block it could mean are offered. ↑ and ↓ choose, Enter, Tab or a click \
             makes the line one, Esc puts the menu away.",
        ),
        Block::new(Kind::Table(cells(&[
            &["Type", "For"],
            &["/h1, /h2, /h3", "a heading"],
            &["/bullet, /number, /todo", "a bulleted, numbered or checklist item"],
            &["/quote", "a quote"],
            &["/table", "a table"],
            &["/divider", "a divider"],
            &["/code", "a code block"],
            &["/picture", "pictures from your files"],
            &["/file", "any file, linked among the words"],
        ]))),
        Block::text("A start is enough: /ta is a table, /h the headings. Beside most, the menu shows what to type instead next time: #, -, ---."),
        heading("Tables that stay put"),
        Block::new(Kind::Table(cells(&[
            &["Key", "In a cell"],
            &["Tab / Shift+Tab", "next / previous cell"],
            &["Enter", "cell below; a new row at the end"],
            &["Enter in an empty last row", "leave the table"],
            &[rows.as_str(), "move the row up or down; the header stays on top"],
            &[cols.as_str(), "move the column left or right"],
            &["Toolbar, or a right-click", "add, remove and move rows and columns, sort by the column you're in"],
        ]))),
        Block::text("Columns keep their width while you type, so the grid never jumps."),
        Block::new(Kind::Divider),
        heading("Words"),
        Block::text(format!(
            "Select some words and press {} for bold, {} for italic, {} to highlight, {} for a \
             link; the toolbar colours them, and {} goes round lowercase, UPPERCASE and Capitalize Each Word. \
             The page shows them styled, and the file keeps them as Markdown, so it reads the same anywhere.",
            k(Shortcut::Bold),
            k(Shortcut::Italic),
            k(Shortcut::Highlight),
            k(Shortcut::Link),
            k(Shortcut::ChangeCase)
        )),
        Block::text(format!(
            "The arrows walk from block to block and through tables; {} and {} go to the \
             top and the bottom, and {} moves the block you are in. Shift+arrows, or a drag, \
             select across blocks; Esc selects the block you are in. {} copies Markdown, and {} \
             undoes anything.",
            k(Shortcut::Top),
            k(Shortcut::Bottom),
            blocks,
            k(Shortcut::Copy),
            k(Shortcut::Undo)
        )),
        heading("Pictures, files and other notes"),
        Block::text(format!(
            "Paste a screenshot with {}, drop a picture on the page, or use the paperclip in the \
             toolbar: pictures show in the note, and a PDF or a document is linked among the words. \
             Either is kept in the attachments folder beside your notes.",
            k(Shortcut::Paste)
        )),
        Block::text(
            "Drop Markdown files on the window to bring them in as notes: on a folder in the list, into \
             that folder. The files stay where they were; Carnet keeps copies, with the pictures they show.",
        ),
        Block::text(format!(
            "Drag a note onto a folder to move it there, or right-click it. {} makes a note, \
             {} a folder, and {} searches. {} opens the settings: colours, fonts, text size, \
             density, how a divider looks (your own words or picture too), where your notes \
             live, and every shortcut.",
            k(Shortcut::NewNote),
            k(Shortcut::NewFolder),
            k(Shortcut::Search),
            k(Shortcut::Settings)
        )),
    ])
}

/// The welcome note as Markdown, its title over it, each shortcut written
/// by `key`: what a note made from it is, untouched.
pub fn markdown(key: impl Fn(Shortcut) -> String) -> String {
    crate::doc::to_markdown(Some(TITLE), &doc(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written as kui writes them: the spelled form with `+` between, in
    /// Ctrl, Super, Alt, Shift order; on a Mac AppKit's glyphs, ⌃⌥⇧⌘, run
    /// together. One it can't read stays as spelled.
    #[test]
    fn shortcuts_are_written_as_the_platform_writes_them() {
        assert_eq!(label("mod+b", false), "Ctrl+B");
        assert_eq!(label("mod+b", true), "\u{2318}B");
        assert_eq!(label("alt+shift+up", false), "Alt+Shift+Up");
        assert_eq!(label("alt+shift+up", true), "\u{2325}\u{21e7}\u{2191}");
        assert_eq!(label("mod+alt+shift+left", false), "Ctrl+Alt+Shift+Left");
        assert_eq!(label("mod+alt+shift+left", true), "\u{2325}\u{21e7}\u{2318}\u{2190}");
        assert_eq!(label("ctrl+shift+tab", true), "\u{2303}\u{21e7}\u{21e5}");
        assert_eq!(label("shift+f3", false), "Shift+F3");
        assert_eq!(label("mod+,", false), "Ctrl+,");
        assert_eq!(label("mod+,", true), "\u{2318},");
        assert_eq!(label("hyper+q", false), "hyper+q");
    }

    /// The note names every shortcut as it is written, and reads back as
    /// the same note: its title, its two tables, its divider.
    #[test]
    fn the_welcome_reads_back_as_itself() {
        let md = markdown(|s| label(s.spec(), false));
        assert!(md.starts_with("# Welcome to Carnet\n\nCarnet is a plain notes app"), "{md}");
        assert!(md.contains("press Ctrl+B for bold, Ctrl+I for italic, Ctrl+Shift+H to highlight, Ctrl+K for a link"), "{md}");
        assert!(md.contains("| Alt+Shift+Up / Alt+Shift+Down | move the row up or down"), "{md}");
        assert!(md.contains("Ctrl+, opens the settings"), "{md}");
        let (title, d) = crate::doc::from_markdown(&md);
        assert_eq!(title.as_deref(), Some(TITLE));
        assert_eq!(crate::doc::to_markdown(title.as_deref(), &d), md);
        let mac = markdown(|s| label(s.spec(), true));
        assert!(mac.contains("press \u{2318}B for bold"), "{mac}");
    }
}
