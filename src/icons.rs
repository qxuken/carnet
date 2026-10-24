//! Carnet's icons: outlines on a 24-unit grid, drawn as kui paths, so a
//! button looks the same whatever fonts the machine has — no glyph from a
//! symbol font, no letter standing in for a picture. Strokes are round,
//! about 1.6 units wide, in the button's ink.

use std::cell::RefCell;
use std::collections::HashMap;

use kui_native::{Color, NodeSpec, Path, PathOp, Role, Stroke, Ui, Vec2};

use crate::settings::Divider;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    Text,
    H1,
    H2,
    H3,
    Bullets,
    Numbers,
    Todo,
    Bold,
    Italic,
    Strike,
    Underline,
    Code,
    Table,
    Divider,
    Picture,
    AddRow,
    AddCol,
    DelRow,
    DelCol,
    /// A table's header row, there or not: the frame with its top band
    /// filled.
    TableHeader,
    Sort,
    SortUp,
    SortDown,
    Outdent,
    Indent,
    Up,
    Down,
    Cross,
    Star,
    Trash,
    Back,
    Plus,
    Minus,
    Folder,
    FolderPlus,
    Settings,
    Search,
    ChevronDown,
    ChevronUp,
    /// A folder's subfolders folded away; turned a quarter, shown.
    ChevronRight,
    /// The toolbar's overflow: three dots, where a menu of more is meant.
    More,
    Quote,
    /// The words' colour: an A on a bar.
    TextColor,
    /// A highlighter's chisel tip.
    Highlight,
    /// A picture to the left of the words, or to the right.
    FloatLeft,
    FloatRight,
    /// A picture on a line of its own.
    Inline,
    /// The library beside the page: a window with its left pane ruled off.
    Sidebar,
    /// Zen: four corners framing nothing but the page (a moon would read
    /// as the dark theme, a broken circle as reload).
    Zen,
    /// Sync's: done, in step.
    Check,
    /// Back to how it was: an arrow turning back.
    Undo,
    /// Sent up: an arrow out of a tray.
    Upload,
    /// A file or a picture added to the note: a paperclip.
    Attach,
    /// A new version to get: an arrow into a tray.
    Download,
    /// Saves packed into one: three sheets stacked.
    Layers,
    /// Not yet: a clock.
    Clock,
    /// Look here: a triangle with a mark.
    Warning,
    /// Read a code with the camera: a frame's four corners, a line across.
    Scan,
    /// One of the account's devices: a phone.
    Device,
    /// The server, what it keeps: a cloud.
    Cloud,
    /// Not synced, kept off the server: the cloud struck through.
    CloudOff,
    /// The recovery words: a key.
    Key,
    /// Write in it: a pencil.
    Edit,
    /// A link: two links of a chain.
    Link,
    /// Someone: an account on the server, a head and shoulders.
    Person,
    /// A passkey: a fingertip's ridges.
    Passkey,
    /// Show the password: an open eye.
    Eye,
    /// Hide it again: the eye struck through.
    EyeOff,
    /// A dropped first letter: a tall A, lines beside it and under it.
    Initial,
}

// The rounded frames several icons share, corners of radius 1.5.
const FRAME_TABLE: &str = "M5 5.5h14a1.5 1.5 0 0 1 1.5 1.5v10a1.5 1.5 0 0 1-1.5 1.5H5a1.5 1.5 0 0 1-1.5-1.5V7A1.5 1.5 0 0 1 5 5.5z ";
const FRAME_PICTURE: &str = "M5 4.5h14a1.5 1.5 0 0 1 1.5 1.5v12a1.5 1.5 0 0 1-1.5 1.5H5a1.5 1.5 0 0 1-1.5-1.5V6A1.5 1.5 0 0 1 5 4.5z ";
const FRAME_ROW: &str = "M5 4h14a1.5 1.5 0 0 1 1.5 1.5v6.5a1.5 1.5 0 0 1-1.5 1.5H5a1.5 1.5 0 0 1-1.5-1.5V5.5A1.5 1.5 0 0 1 5 4z ";
const CLOUD: &str = "M7.5 18.5h9.5a3 3 0 1 0-.23-5.99a5 5 0 1 0-9.74-.98A3.5 3.5 0 0 0 7.5 18.5z";
/// The cloud, struck through its middle: the stroke the same length past
/// each side, the outline parted where it crosses.
const CLOUD_OFF: &str = "M15.81 18.5H7.5A3.5 3.5 0 0 1 7.03 11.53A5 5 0 0 1 7.14 9.83M8.83 7.14A5 5 0 0 1 16.77 12.51A3 3 0 0 1 19.16 17.46M5.5 6L20 20.5";
const FRAME_COL: &str ="M5 3.5h6a1.5 1.5 0 0 1 1.5 1.5v14a1.5 1.5 0 0 1-1.5 1.5H5a1.5 1.5 0 0 1-1.5-1.5V5A1.5 1.5 0 0 1 5 3.5z ";

impl Glyph {
    /// The outline to stroke (a frame and the rest), and the one to fill
    /// (empty for none), in SVG path syntax on the 24 grid.
    fn parts(self) -> (String, &'static str) {
        let (rest, fill) = self.pieces();
        (format!("{}{rest}", self.frame()), fill)
    }

    fn frame(self) -> &'static str {
        match self {
            Glyph::Table | Glyph::TableHeader => FRAME_TABLE,
            Glyph::Picture => FRAME_PICTURE,
            Glyph::AddRow | Glyph::DelRow => FRAME_ROW,
            Glyph::AddCol | Glyph::DelCol => FRAME_COL,
            _ => "",
        }
    }

    fn pieces(self) -> (&'static str, &'static str) {
        use Glyph::*;
        match self {
            Text => ("M6 7.5V6h12v1.5 M12 6v12 M9.5 18h5", ""),
            // The heading icons share one H and one box for the digit:
            // x 15–20, y 10–18, standing on the H's baseline (18), so the
            // three sit alike and only the digit changes.
            H1 => ("M3.5 6v12 M11.5 6v12 M3.5 12h8 M15.5 11.8l2.7-1.8v8", ""),
            H2 => ("M3.5 6v12 M11.5 6v12 M3.5 12h8 M15 12.5a2.5 2.5 0 0 1 5 0c0 2.3-5 3.2-5 5.5h5", ""),
            H3 => (
                "M3.5 6v12 M11.5 6v12 M3.5 12h8 \
                 M15.1 10h4.9l-2.8 3.2c1.6 0 2.9.9 2.9 2.3 0 1.5-1.2 2.5-2.8 2.5-1 0-1.9-.4-2.5-1",
                "",
            ),            Bullets => (
                "M10 6.5h10 M10 12h10 M10 17.5h10",
                "M3.4 6.5a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0z \
                 M3.4 12a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0z \
                 M3.4 17.5a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0z",
            ),
            Numbers => (
                "M10 6.5h10 M10 12h10 M10 17.5h10 M4.3 5.2l1.6-1.1v5.4 \
                 M3.9 15.4c0-.9.7-1.5 1.5-1.5s1.5.6 1.5 1.4c0 1.2-3 2.1-3 4.1h3.1",
                "",
            ),
            Todo => ("M12 3.8a8.2 8.2 0 1 0 0 16.4a8.2 8.2 0 1 0 0-16.4z M8.2 12.3l2.6 2.6 5-5.3", ""),
            Bold => (
                "M7.5 5h5.3a3.4 3.4 0 0 1 0 6.8H7.5z M7.5 11.8h6.2a3.6 3.6 0 0 1 0 7.2H7.5z",
                "",
            ),
            Italic => ("M10.5 5h7 M6.5 19h7 M14.5 5l-4.5 14", ""),
            Strike => (
                "M4.5 12h15 M16.3 7.6c-.5-1.6-2.2-2.6-4.3-2.6-2.5 0-4.3 1.3-4.3 3.2 0 1 .4 1.8 1.4 2.4 \
                 M15.6 14.3c.5.6.8 1.2.8 2 0 1.9-1.9 3.2-4.5 3.2-2.3 0-4-1-4.6-2.6",
                "",
            ),
            // A U standing on its line.
            Underline => ("M7.5 4.5v6.8a4.5 4.5 0 0 0 9 0V4.5 M5.5 20h13", ""),
            Code => ("M8.5 7.5L4 12l4.5 4.5 M15.5 7.5L20 12l-4.5 4.5 M13.2 5.5l-2.4 13", ""),
            Table => ("M3.5 10.5h17 M9.5 10.5v8", ""),
            // A rule with the space it makes: chevrons parting from it.
            // (Dots read as a "more" menu in a toolbar.)
            Divider => ("M3.5 12h17 M9 7.5l3-3 3 3 M9 16.5l3 3 3-3", ""),            Picture => (
                "M3.5 16.5l4.5-4.5 4 4 2.5-2.5 5.6 5.6 M15.5 7.8a1.5 1.5 0 1 0 0 3a1.5 1.5 0 1 0 0-3z",
                "",
            ),
            AddRow => ("M3.5 8.8h17 M12 15.5v5 M9.5 18h5", ""),
            DelRow => ("M3.5 8.8h17 M9.5 18h5", ""),
            AddCol => ("M8 3.5v17 M18 9.5v5 M15.5 12h5", ""),
            DelCol => ("M8 3.5v17 M15.5 12h5", ""),
            TableHeader => ("M3.5 10.5h17 M9.5 10.5v8", "M5 5.5h14a1.5 1.5 0 0 1 1.5 1.5v3.5h-17V7A1.5 1.5 0 0 1 5 5.5z"),
            Sort => ("M8 5v14 M4.5 15.5L8 19l3.5-3.5 M16 19V5 M12.5 8.5L16 5l3.5 3.5", ""),
            SortUp => ("M4 7h5 M4 12h8 M4 17h11 M18.5 19V5 M15.5 8l3-3 3 3", ""),
            SortDown => ("M4 7h11 M4 12h8 M4 17h5 M18.5 5v14 M15.5 16l3 3 3-3", ""),
            Outdent => ("M20 6H10 M20 12h-7 M20 18H10 M7.5 9l-3.5 3 3.5 3", ""),
            Indent => ("M20 6H10 M20 12h-7 M20 18H10 M4 9l3.5 3L4 15", ""),
            Up => ("M12 19V5 M6.5 10.5L12 5l5.5 5.5", ""),
            Down => ("M12 5v14 M6.5 13.5L12 19l5.5-5.5", ""),
            Cross => ("M6.5 6.5l11 11 M17.5 6.5l-11 11", ""),
            Star => ("M12 3.8l2.5 5.2 5.7.8-4.1 4 1 5.6-5.1-2.7-5.1 2.7 1-5.6-4.1-4 5.7-.8z", ""),
            Trash => (
                "M4.5 7h15 M9.5 7V5.2c0-.4.3-.7.7-.7h3.6c.4 0 .7.3.7.7V7 \
                 M6.5 7l.9 11.8c.1.9.8 1.7 1.7 1.7h5.8c.9 0 1.6-.8 1.7-1.7L17.5 7 M10 11v6 M14 11v6",
                "",
            ),
            Back => ("M15 5.5L8.5 12l6.5 6.5", ""),
            Plus => ("M12 5v14 M5 12h14", ""),
            Minus => ("M5 12h14", ""),
            Folder => (
                "M3.5 7.5c0-1.1.9-2 2-2h3.8l2 2.2h7.2c1.1 0 2 .9 2 2v7.8c0 1.1-.9 2-2 2h-13c-1.1 0-2-.9-2-2z",
                "",
            ),
            FolderPlus => (
                "M3.5 7.5c0-1.1.9-2 2-2h3.8l2 2.2h7.2c1.1 0 2 .9 2 2v7.8c0 1.1-.9 2-2 2h-13c-1.1 0-2-.9-2-2z \
                 M12 11v5 M9.5 13.5h5",
                "",
            ),
            Settings => (
                "M4 7h8 M16 7h4 M4 17h3 M11 17h9 \
                 M14 5a2 2 0 1 0 0 4a2 2 0 1 0 0-4z M9 15a2 2 0 1 0 0 4a2 2 0 1 0 0-4z",
                "",
            ),
            Search => ("M10.5 4.5a6 6 0 1 0 0 12a6 6 0 1 0 0-12z M15 15l4.5 4.5", ""),
            ChevronDown => ("M6.5 9.5L12 15l5.5-5.5", ""),
            ChevronUp => ("M6.5 14.5L12 9l5.5 5.5", ""),
            ChevronRight => ("M9.5 6.5L15 12l-5.5 5.5", ""),
            Quote => ("M5 5.5v13 M9.5 7.5h10 M9.5 12h10 M9.5 16.5h6.5", ""),
            TextColor => ("M7 16.5l5-12 5 12 M8.9 12h6.2", "M4.5 18.5h15v2.2h-15z"),
            Highlight => (
                "M14.6 4.4l5 5-7.4 7.4-5-5z M7.2 11.8L5 17.2l1.8 1.8 5.4-2.2 M4.5 20.5h15",
                "",
            ),
            FloatLeft => ("M3.5 5h7.5v7.5H3.5z M14.5 6h6 M14.5 11.5h6 M3.5 16.5h17 M3.5 20h11", ""),
            FloatRight => ("M13 5h7.5v7.5H13z M3.5 6h6 M3.5 11.5h6 M3.5 16.5h17 M3.5 20h11", ""),
            Inline => ("M6.5 4.5h11v8h-11z M3.5 16.5h17 M3.5 20h11", ""),
            Sidebar => (
                "M5.5 5h13c1.1 0 2 .9 2 2v10c0 1.1-.9 2-2 2h-13c-1.1 0-2-.9-2-2V7c0-1.1.9-2 2-2z M9.5 5v14",
                "",
            ),
            Zen => (
                "M4.5 9V6c0-.8.7-1.5 1.5-1.5h3 M15 4.5h3c.8 0 1.5.7 1.5 1.5v3 \
                 M19.5 15v3c0 .8-.7 1.5-1.5 1.5h-3 M9 19.5H6c-.8 0-1.5-.7-1.5-1.5v-3",
                "",
            ),
            Check => ("M5 12.5l4.5 4.5L19 7.5", ""),
            Undo => ("M8.5 5.5l-4 4 4 4 M4.5 9.5h9.5a5 5 0 0 1 0 10H10", ""),
            Upload => (
                "M12 15V4.5 M7.5 9L12 4.5 16.5 9 M4.5 14.5v3.5c0 1.1.9 2 2 2h11c1.1 0 2-.9 2-2v-3.5",
                "",
            ),
            Attach => (
                "M21.44 11.05l-9.19 9.19a6 6 0 0 1 -8.49 -8.49l8.57 -8.57a4 4 0 0 1 5.67 5.66l-8.59 8.57a2 2 0 0 1 -2.83 -2.83l8.49 -8.48",
                "",
            ),
            Download => (
                "M12 4.5V15 M7.5 10.5L12 15 16.5 10.5 M4.5 14.5v3.5c0 1.1.9 2 2 2h11c1.1 0 2-.9 2-2v-3.5",
                "",
            ),
            Layers => ("M12 4l8.5 4.5L12 13 3.5 8.5z M3.5 12.5L12 17l8.5-4.5 M3.5 16.5L12 21l8.5-4.5", ""),
            Clock => ("M12 3.8a8.2 8.2 0 1 0 0 16.4a8.2 8.2 0 1 0 0-16.4z M12 7.5V12l3 2", ""),
            Scan => (
                "M4 8.5V6a2 2 0 0 1 2-2h2.5 M15.5 4H18a2 2 0 0 1 2 2v2.5 M20 15.5V18a2 2 0 0 1-2 2h-2.5 \
                 M8.5 20H6a2 2 0 0 1-2-2v-2.5 M7.5 12h9",
                "",
            ),
            Device => ("M8.5 3.5h7a1.5 1.5 0 0 1 1.5 1.5v14a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 7 19V5a1.5 1.5 0 0 1 1.5-1.5z M11 17.5h2", ""),
            // Three lobes on a flat base: a small one each side, a big one
            // over them.
            Cloud => (CLOUD, ""),
            CloudOff => (CLOUD_OFF, ""),
            Key => ("M7.5 15a3.5 3.5 0 1 0 0-7a3.5 3.5 0 1 0 0 7z M11 11.5h9 M17 11.5v3 M20 11.5v2", ""),
            Edit => ("M4.5 19.5h3.8L19 8.8a2.1 2.1 0 0 0-3-3L5.3 16.5z M14.3 7.5l2.2 2.2", ""),
            Person => ("M12 12a3.5 3.5 0 1 0 0-7a3.5 3.5 0 1 0 0 7z M5 20c0-3.6 3.1-5.5 7-5.5s7 1.9 7 5.5", ""),
            Eye => (
                "M2.5 12C4.7 7.8 8.1 5.5 12 5.5s7.3 2.3 9.5 6.5c-2.2 4.2-5.6 6.5-9.5 6.5s-7.3-2.3-9.5-6.5z \
                 M12 9a3 3 0 1 0 0 6a3 3 0 1 0 0-6z",
                "",
            ),
            EyeOff => (
                "M2.5 12C4.7 7.8 8.1 5.5 12 5.5s7.3 2.3 9.5 6.5c-2.2 4.2-5.6 6.5-9.5 6.5s-7.3-2.3-9.5-6.5z \
                 M12 9a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M4.5 4.5l15 15",
                "",
            ),
            Passkey => ("M5 16.5v-4.5a7 7 0 0 1 14 0v2 M8.5 19.5v-7.5a3.5 3.5 0 0 1 7 0v5 M12 12v8", ""),
            Initial => ("M3.5 14.5L7.25 4.5l3.75 10 M4.9 10.8h4.7 M14 5.5h6.5 M14 10h6.5 M14 14.5h6.5 M3.5 19h17", ""),
            Link => (
                "M10.5 13.5a3.5 3.5 0 0 0 5 0l3-3a3.5 3.5 0 0 0-5-5l-1 1 M13.5 10.5a3.5 3.5 0 0 0-5 0l-3 3a3.5 3.5 0 0 0 5 5l1-1",
                "",
            ),
            Warning => (
                "M10.3 4.6c.8-1.4 2.6-1.4 3.4 0l7 12.2c.8 1.4-.2 3.2-1.7 3.2H5c-1.5 0-2.5-1.8-1.7-3.2z M12 9.5v4.5",
                "M11.1 17a.9.9 0 1 0 1.8 0a.9.9 0 1 0-1.8 0z",
            ),
            More => (
                "",
                "M4.4 12a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0z M10.4 12a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0z \
                 M16.4 12a1.6 1.6 0 1 0 3.2 0a1.6 1.6 0 1 0-3.2 0z",
            ),
        }
    }
}

/// A glyph's outlines at one size: what it strokes, what it fills.
type Outlines = (Option<Path>, Option<Path>);

impl Glyph {
    pub const ALL: [Glyph; 69] = [
        Glyph::Text, Glyph::H1, Glyph::H2, Glyph::H3, Glyph::Bullets, Glyph::Numbers, Glyph::Todo, Glyph::Bold,
        Glyph::Italic, Glyph::Strike, Glyph::Underline, Glyph::Code, Glyph::Table, Glyph::Divider, Glyph::Picture, Glyph::AddRow,
        Glyph::AddCol, Glyph::DelRow, Glyph::DelCol, Glyph::Sort, Glyph::SortUp, Glyph::SortDown, Glyph::Outdent,
        Glyph::Indent, Glyph::Up, Glyph::Down, Glyph::Cross, Glyph::Star, Glyph::Trash, Glyph::Back, Glyph::Plus,
        Glyph::Minus, Glyph::Folder, Glyph::FolderPlus, Glyph::Settings, Glyph::Search, Glyph::ChevronDown,
        Glyph::ChevronUp, Glyph::More, Glyph::Quote, Glyph::TextColor, Glyph::Highlight, Glyph::FloatLeft,
        Glyph::FloatRight, Glyph::Inline, Glyph::Sidebar, Glyph::Zen, Glyph::Check, Glyph::Undo, Glyph::Upload,
        Glyph::Download, Glyph::Layers, Glyph::Clock, Glyph::Warning, Glyph::Scan, Glyph::Device, Glyph::Key, Glyph::Edit, Glyph::Link,
        Glyph::Person, Glyph::Passkey, Glyph::Eye, Glyph::EyeOff, Glyph::Initial, Glyph::Cloud, Glyph::CloudOff,
        Glyph::TableHeader, Glyph::ChevronRight, Glyph::Attach,
    ];

    /// The icon's file name: `h1`, `folder-plus`.
    pub fn slug(self) -> String {
        let name = format!("{self:?}");
        let mut out = String::new();
        for (i, ch) in name.chars().enumerate() {
            if ch.is_uppercase() && i > 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        }
        out
    }

    /// The icon as an SVG file on the 24 grid, in `ink`: the same outlines
    /// the app draws.
    pub fn svg(self, ink: &str) -> String {
        let (stroke, fill) = self.parts();
        let mut body = String::new();
        if !fill.is_empty() {
            body.push_str(&format!("<path d=\"{fill}\" fill=\"{ink}\"/>"));
        }
        if !stroke.trim().is_empty() {
            body.push_str(&format!(
                "<path d=\"{}\" fill=\"none\" stroke=\"{ink}\" stroke-width=\"1.65\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>",
                stroke.split_whitespace().collect::<Vec<_>>().join(" ")
            ));
        }
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\">{body}</svg>\n")
    }
}

thread_local! {
    /// Parsed and scaled outlines, by glyph and size: parsing is cheap,
    /// but a toolbar asks for thirty of them every frame.
    static CACHE: RefCell<HashMap<(Glyph, u32), Outlines>> = RefCell::new(HashMap::new());
}

/// An outline parsed and scaled by `k` from the 24 grid.
fn scaled(d: &str, k: f32) -> Option<Path> {
    if d.is_empty() {
        return None;
    }
    let p = Path::parse(d).ok()?;
    let s = |v: Vec2| Vec2::new(v.x * k, v.y * k);
    let ops = p
        .ops()
        .iter()
        .map(|op| match *op {
            PathOp::MoveTo(a) => PathOp::MoveTo(s(a)),
            PathOp::LineTo(a) => PathOp::LineTo(s(a)),
            PathOp::QuadTo(a, b) => PathOp::QuadTo(s(a), s(b)),
            PathOp::CubicTo(a, b, c) => PathOp::CubicTo(s(a), s(b), s(c)),
            PathOp::ArcTo {
                rx,
                ry,
                rotation,
                large,
                sweep,
                to,
            } => PathOp::ArcTo {
                rx: rx * k,
                ry: ry * k,
                rotation,
                large,
                sweep,
                to: s(to),
            },
            PathOp::Close => PathOp::Close,
        })
        .collect();
    Some(Path::from_ops(ops))
}

/// `g` in a `size`-square box of its own, in `ink`. `filled` fills the
/// outline too (a pinned star).
pub fn icon(ui: &mut Ui<'_>, g: Glyph, ink: Color, size: f32, filled: bool) {
    let (stroke, fill) = CACHE.with(|c| {
        c.borrow_mut()
            .entry((g, size.to_bits()))
            .or_insert_with(|| {
                let (s, f) = g.parts();
                let k = size / 24.0;
                (scaled(&s, k), scaled(f, k))
            })
            .clone()
    });
    // A hairline at small sizes still reads; at large ones it is not heavy.
    let width = (1.65 * size / 24.0).max(1.25);
    let width = if matches!(g, Glyph::Bold) { width * 1.25 } else { width };
    ui.with(NodeSpec::row().size(size, size).role(Role::None), |ui| {
        if let Some(p) = fill {
            ui.path(&p, NodeSpec::row().bg(ink));
        }
        if let Some(p) = stroke {
            let spec = if filled { NodeSpec::row().bg(ink) } else { NodeSpec::row() };
            ui.path(&p.stroked(Stroke::new(width, ink)), spec);
        }
    });
}

/// A divider's ornament as path data on a box in px at 16px text: its
/// width and height, what is stroked and what is filled, and the stroke's
/// width. Drawn, not typed: no font needs to have ⁂ or a fleuron.
fn ornament_parts(d: Divider) -> (f32, f32, String, String, f32) {
    let dot = |cx: f32, cy: f32, r: f32| format!("M{} {cy} A{r} {r} 0 1 0 {} {cy} A{r} {r} 0 1 0 {} {cy} Z ", cx - r, cx + r, cx - r);
    // An asterisk: three strokes through its middle, six points.
    let star = |cx: f32, cy: f32, r: f32| {
        let (dx, dy) = (r * 0.866, r * 0.5);
        format!(
            "M{cx} {} L{cx} {} M{} {} L{} {} M{} {} L{} {} ",
            cy - r,
            cy + r,
            cx - dx,
            cy - dy,
            cx + dx,
            cy + dy,
            cx - dx,
            cy + dy,
            cx + dx,
            cy - dy
        )
    };
    match d {
        Divider::Dots => (33.0, 3.0, String::new(), [1.5, 16.5, 31.5].map(|x| dot(x, 1.5, 1.5)).concat(), 0.0),
        Divider::Line => (96.0, 2.0, "M0 1 L96 1".into(), String::new(), 1.0),
        Divider::Stars => (44.0, 8.0, [4.0, 22.0, 40.0].map(|x| star(x, 4.0, 3.6)).concat(), String::new(), 1.3),
        Divider::Asterism => (
            19.0,
            16.0,
            [(9.5, 4.0), (4.0, 12.0), (15.0, 12.0)].map(|(x, y)| star(x, y, 3.4)).concat(),
            String::new(),
            1.2,
        ),
        // A swash each way from a small diamond, each ending in a curl.
        Divider::Flourish => (
            120.0,
            20.0,
            "M66 10 C74 4 82 4 88 10 C94 16 102 16 106 10 C108 7 105 4 102 6 \
             M54 10 C46 4 38 4 32 10 C26 16 18 16 14 10 C12 7 15 4 18 6"
                .into(),
            "M60 6 L64 10 L60 14 L56 10 Z".into(),
            1.3,
        ),
        // The hedera: a heart-shaped leaf pointing right, notched where
        // its stem meets it, the stem curling back over it.
        Divider::Fleuron => (
            36.0,
            22.0,
            "M15 11 C10 11 6.5 10 5 7.2 C3.6 4.6 5.6 2 8.2 2.8 C10.2 3.4 10 6 7.9 6.1".into(),
            "M15 11 C11.5 7 13 3 17.5 3.5 C23 4 28 7.5 33 11 C28 14.5 23 18 17.5 18.5 \
             C13 19 11.5 15 15 11 Z"
                .into(),
            1.3,
        ),
        // Lozenges, a little taller than wide, spaced as the dots are.
        Divider::Diamonds => (
            36.0,
            9.0,
            String::new(),
            [3.0, 18.0, 33.0].map(|x| format!("M{x} 0 L{} 4.5 L{x} 9 L{} 4.5 Z ", x + 3.0, x - 3.0)).concat(),
            0.0,
        ),
        Divider::Lozenge => (
            96.0,
            10.0,
            "M0 5 L39 5 M57 5 L96 5".into(),
            "M48 0.5 L52.5 5 L48 9.5 L43.5 5 Z".into(),
            1.0,
        ),
        // One even wave, its ends turned in.
        Divider::Wave => (
            44.0,
            10.0,
            "M2 6.5 C7 1 14 1 22 5 C30 9 37 9 42 3.5".into(),
            String::new(),
            1.6,
        ),
    }
}

/// A divider's ornament's width at 16px text.
pub fn ornament_width(d: Divider) -> f32 {
    ornament_parts(d).0
}

/// A divider's ornament in `ink`, at `k` times its size at 16px text.
pub fn ornament(ui: &mut Ui<'_>, d: Divider, ink: Color, k: f32) {
    let (stroke, fill, w, h, sw) = ORNAMENTS.with(|c| {
        c.borrow_mut()
            .entry((d, k.to_bits()))
            .or_insert_with(|| {
                let (w, h, s, f, sw) = ornament_parts(d);
                (scaled(&s, k), scaled(&f, k), (w * k).round(), (h * k).round(), (sw * k).max(1.0))
            })
            .clone()
    });
    ui.with(NodeSpec::row().size(w, h).role(Role::None), |ui| {
        if let Some(p) = fill {
            ui.path(&p, NodeSpec::row().bg(ink));
        }
        if let Some(p) = stroke {
            ui.path(&p.stroked(Stroke::new(sw, ink)), NodeSpec::row());
        }
    });
}

type Ornament = (Option<Path>, Option<Path>, f32, f32, f32);

thread_local! {
    static ORNAMENTS: RefCell<HashMap<(Divider, u32), Ornament>> = RefCell::new(HashMap::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ornament_parses() {
        for d in Divider::ALL {
            let (_, _, s, f, _) = ornament_parts(d);
            for part in [&s, &f] {
                assert!(part.is_empty() || Path::parse(part).is_ok(), "{d:?}: {:?}", Path::parse(part).err());
                // A line ended with `\\` inside the string, not `\`: the
                // rest of the outline read as nothing.
                assert!(!part.contains('\\'), "{d:?}: a stray backslash");
            }
            assert!(scaled(&s, 0.75).is_some() || scaled(&f, 0.75).is_some(), "{d:?} draws something");
        }
    }

    #[test]
    fn every_glyph_parses() {
        for g in Glyph::ALL {
            let (s, f) = g.parts();
            assert!(s.is_empty() || Path::parse(&s).is_ok(), "{g:?}: {:?}", Path::parse(&s).err());
            if !f.is_empty() {
                assert!(Path::parse(f).is_ok(), "{g:?} fill");
            }
            assert!(scaled(&s, 0.75).is_some() || scaled(f, 0.75).is_some(), "{g:?} draws something");
        }
    }
}
