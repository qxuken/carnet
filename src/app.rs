//! The window: a library on the left, the note on the right, and one quiet
//! strip over the note that is the toolbar and the titlebar at once.
//!
//! The note's body is an editor this app owns ([`crate::page`]): one key
//! sink for the whole page, every block drawn as wrapped text with a caret
//! and a selection of its own. kui's own `edit` is still the title and the
//! search — single fields, which is what it is for. Owning the page is what
//! lets the arrows cross blocks and table cells, Backspace join lines and
//! take empty ones, and one undo history cover every change.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use kui_native::widgets;
use kui_native::{
    Align, App, Color, Core, EditOptions, Edges, FloatConfig, Key, KeyCode, KeyPhase, Menu,
    MenuItem, Message, NodeSpec, Rect, Role, Sizing, Stroke, TextStyle, Theme, Ui, UiEvent,
    Value, Vec2, Waker,
};

use carnet_core::doc::{Block, Doc, Kind, ListStyle, Table};
use carnet_core::export;
use crate::history::{History, State, Step};
use crate::look::{Look, Styler};
use crate::page::{
    self, Caret, Pos, Slot, TURN_BULLET, TURN_H1, TURN_H2, TURN_H3, TURN_NUMBER, TURN_TEXT,
    TURN_QUOTE, TURN_TODO, TableOp, Typing,
};
use crate::images::{self, Images};
use crate::settings::{Settings, Sort, ThemePref, Width};
use crate::store::{self, Note, Saved, Store};
use carnet_sync::{Origin, Revision, Version};
use crate::clip;

#[path = "app_chrome.rs"]
mod chrome;
#[path = "app_page.rs"]
mod page_view;
#[path = "app_settings.rs"]
mod settings_view;
#[path = "app_devtools.rs"]
mod devtools_view;
#[path = "app_menu.rs"]
mod menu_view;
#[path = "app_pick.rs"]
mod pick_view;
use pick_view::Pick;
use chrome::ago;

use crate::design::{Type, radius, size};

const STRIP_H: f32 = size::STRIP;
const BODY_LH: f32 = Type::BODY.line;
/// The least the page keeps beside the library: below the library's
/// width and this, the library folds away, to a button.
const PAGE_MIN: f32 = 592.0;
/// How long typing has to pause before the note is written.
const SAVE_AFTER: Duration = Duration::from_millis(600);
const NOTICE_FOR: Duration = Duration::from_secs(8);
/// How far the pointer moves, in px, before typing is taken to be over.
const POINTER_NUDGE: f32 = 6.0;
/// How soon a save that failed is tried again.
const RETRY_AFTER: Duration = Duration::from_millis(350);
/// How often the inspector's "Checked …" is drawn again while it shows.
const INSPECTED_EVERY: Duration = Duration::from_secs(20);
/// The longest pause between two clicks on the Settings page's title
/// that still counts them as one run.
const TITLE_CLICKS: Duration = Duration::from_millis(600);

#[derive(Message, Clone, Debug, PartialEq)]
pub enum Msg {
    /// The root key sink: keys while nothing holds focus, and the chords
    /// the title and the search let go of.
    RootKey,
    /// The page's key sink: keys, typed and pasted text, IME composition.
    Page,
    /// A press or drag on the page: in text, or in the margin.
    PageDrag,
    /// A click on the Settings page's title: five in quick succession
    /// turn the debug information on or off.
    SettingsTitle,
    OpenNote { note: u64 },
    NewNote,
    PinNote,
    DeleteNote,
    Toggle { id: u64 },
    /// A click on a divider, which has no text to put a caret in.
    Select { id: u64 },
    Turn { to: u64 },
    /// Drop the first letter of the paragraphs the caret touches, or not.
    ToggleInitial,
    Indent { by: i64 },
    Move { by: i64 },
    DeleteBlock,
    InsertTable,
    InsertDivider,
    AddRow,
    AddCol,
    DelRow,
    DelCol,
    Sort,
    Undo,
    ClearSearch,
    FocusTitle,
    FocusSearch,
    /// Bold, italic, struck or code (`STYLES[which]`) on the selection.
    Style { which: u64 },
    /// A narrow window's way back from a note to the list of notes.
    ShowList,
    /// Shows one folder's notes (`store.folders[index]`), or all of them
    /// (`ALL`).
    SelectFolder { index: u64 },
    /// Opens the field that names a new top-level folder.
    NewFolder,
    /// Opens the settings, or closes them.
    ShowSettings,
    /// 0 system, 1 light, 2 dark.
    SetTheme { to: u64 },
    /// `design::Palette::ALL[to]`.
    SetPalette { to: u64 },
    /// 0 compact, 1 comfortable, 2 airy.
    SetDensity { to: u64 },
    /// `design::PAIRINGS[to]`: a text face and a heading face together.
    SetPairing { to: u64 },
    /// 0 narrow, 1 medium, 2 wide.
    SetWidth { to: u64 },
    /// 0 last edited, 1 title, 2 created.
    SetSort { to: u64 },
    /// The date a note shows: 0 when last edited, 1 when made.
    SetDates { to: u64 },
    /// How the page looks and behaves, back as it first was: asked
    /// first, then done; or not.
    ResetLook,
    ResetLookCancel,
    /// The window: 0 solid, 1 the desktop showing through the library.
    SetWindow { to: u64 },
    /// While typing, the toolbar: 0 stays, 1 fades.
    SetToolbar { to: u64 },
    /// While typing, the other blocks: 0 as they are, 1 stepped back.
    SetFocus { to: u64 },
    /// The caret's line: 0 wherever it is, 1 held near the middle.
    SetTypewriter { to: u64 },
    /// Shows the library beside the page, or hides it.
    ToggleSidebar,
    /// The library shows the Deleted folder: what is in the trash.
    ShowDeleted,
    /// The library shows the open note's versions, and the page the
    /// newest, read-only.
    ShowHistory,
    /// The page shows the history's `index`th version.
    ShowVersion { index: u64 },
    /// The note becomes the version the page shows.
    RestoreVersion,
    /// The version the page shows becomes a note of its own.
    RestoreVersionCopy,
    /// Back to the note and the notes.
    CloseHistory,
    /// Settings → Sync: which way in (0 a code: an invite or a pairing
    /// code, 1 the recovery words, 2 an email and password).
    SyncHow { to: u64 },
    /// Settings → Sync: the password typed shown as it is, or as dots.
    SyncShowPassword,
    /// Settings → Sync: set (or change) the password, its fields opened;
    /// saved; let be.
    SyncPassword,
    SyncPasswordSave,
    SyncPasswordCancel,
    /// Take the password away (asked first).
    SyncPasswordRemove,
    /// Delete the whole account (asked first).
    SyncDeleteAccount,
    SyncDeleteAccountCancel,
    /// Remove the passkey at this place in Settings → Sync's list (asked
    /// first); or not.
    SyncPasskeyRemove { at: u64 },
    SyncPasskeyCancel,
    /// Remove the device at this place in Settings → Sync's list (asked
    /// twice).
    SyncRemove { at: u64 },
    /// The sync inspector opened or closed; asked again.
    SyncInspector,
    SyncInspectorRefresh,
    /// The note at this place in the inspector: its ways to fix it shown
    /// or hidden.
    SyncInspectMore { at: u64 },
    /// Its packing undone, or this device's copy made the server's: each
    /// asked first, then done by `SyncInspectConfirm`.
    SyncRollBack { at: u64 },
    SyncRepair { at: u64 },
    SyncInspectConfirm,
    SyncInspectCancel,
    /// Settings → Sync: set up, the way chosen.
    SyncStart,
    SyncCancel,
    /// Show a code to let a new device in.
    SyncPair,
    SyncPairCancel,
    /// Change this device's name: the field shown, its name saved, or
    /// left as it was.
    SyncRename,
    SyncRenameSave,
    SyncRenameCancel,
    /// Make an invite, for someone's own account on this server (the
    /// owner's devices only); the code shown put away; one waiting taken
    /// back, by its place in the list.
    SyncInvite,
    SyncInviteDone,
    SyncInviteDrop { at: u64 },
    /// The waiting invite at this place, its code on the clipboard.
    SyncInviteCopy { at: u64 },
    /// Remove the account at this place in the owner's list (asked twice).
    SyncAccountRemove { at: u64 },
    /// Make a recovery code, its words shown once.
    SyncRecovery,
    /// The words written down: put away.
    SyncRecoveryDone,
    /// Sync off on this device.
    SyncOff,
    /// The version the page shows, compared with the note as it is now,
    /// or (again) with the version before it.
    CompareWithNow,
    /// Puts `trash[index]` back where it was.
    RestoreTrashed { index: u64 },
    /// Opens `trash[index]` on the page, to read: what it holds, and the
    /// ways out (put back, or deleted forever).
    OpenDeleted { index: u64 },
    /// The deleted thing on the page, put back.
    RestorePeek,
    /// The deleted thing on the page, deleted forever: asked first, then
    /// done.
    DeleteForever,
    DeleteForeverCancel,
    /// Zen: the library away, the tools out of sight until the pointer
    /// goes up to them, focus and the typewriter on; or back.
    ToggleZen,
    /// Colour on the selection: `ink` 1 for the words' own, 0 for a
    /// highlighter; `color` 1–7, 0 none.
    Tint { ink: u64, color: u64 },
    /// The colour picker for the words' colour (`ink` 1) or a highlighter.
    TintMenu { ink: u64 },
    /// The link editor's buttons: 0 done, 1 open, 2 remove the link.
    LinkAct { what: u64 },
    CloseLink,
    /// The selected picture beside the words (1 left, 2 right) or on its
    /// own line (0).
    Float { to: u64 },
    /// The text a step bigger (1) or smaller (-1).
    SetSize { by: i64 },
    /// How dividers are drawn: `settings::Divider::ALL[to]`.
    SetDivider { to: u64 },
    /// How tall the dropped letter is: `to + 2` lines.
    SetInitialLines { to: u64 },
    /// The dropped letter's ink: 0 the accent's, 1 the text's.
    SetInitialColor { to: u64 },
    /// Opens (or closes) the list of faces for the body (0), the
    /// headings (1) or the dropped letter (2).
    FontMenu { which: u64 },
    /// Chooses a face from that list (`ALL` for the default).
    SetFont { which: u64, index: u64 },
    /// Asks for another folder to keep the notes in.
    PickDir,
    /// Back to `~/Documents/Carnet`.
    ResetDir,
    /// Shows the notes folder in the file manager.
    ShowDir,
    /// Runs `slash::COMMANDS[index]` on the `/word` line.
    Slash { index: u64 },
    /// A picture pressed and maybe dragged to another place.
    PictureDrag { id: u64 },
    /// A picture's handle dragged: its width follows; `side` 1 for one on
    /// its left edge, which widens it going left.
    ResizePicture { id: u64, side: u64 },
    /// A press or drag on a note's card: a click opens it, a drag carries
    /// it to a folder.
    CardDrag { note: u64 },
    /// The library's edge dragged: its width follows; a double-click puts
    /// it back.
    LibraryDrag,
    /// Opens the More menu: the tools a narrow strip has no room for.
    ToolsMenu,
    /// Puts a piece of the window's text on the clipboard: the notes
    /// folder's path (0), the open note as Markdown (1).
    CopyText { what: u64 },
}

/// `Msg::SelectFolder`'s every note, and `Msg::SetFont`'s default face:
/// the largest number a message carries whole (kui keeps it as an
/// `i64`), or a click on it arrives as another.
pub const ALL: u64 = i64::MAX as u64;

/// The inline styles, as `Msg::Style` names them.
pub const STYLES: [carnet_core::inline::Kind; 4] =
    [carnet_core::inline::Kind::Bold, carnet_core::inline::Kind::Italic, carnet_core::inline::Kind::Strike, carnet_core::inline::Kind::Code];

/// What a context menu is about.
#[derive(Clone, Debug, PartialEq)]
enum MenuFor {
    Note(u64),
    Folder(String),
    Page,
    /// The toolbar's overflow.
    Tools,
    /// The notes and folders chosen together in the library.
    Picked,
    /// A row of the Deleted folder: `trash[i]`.
    Deleted(usize),
}

/// The kui editors left: the note's title, the library's search, and a
/// new folder's name while it is typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Search,
    Title,
    Folder,
    /// A face's name typed in the settings.
    Font,
    /// A link's address, in the link editor.
    Link,
    /// Settings → Sync: the server's address.
    SyncServer,
    /// Settings → Sync: an invite code, a pairing code, or the words.
    SyncCode,
    /// Settings → Sync: this device's name, being changed.
    DeviceName,
    /// Settings → Sync: the email and password to come in by.
    SyncEmail,
    SyncPassword,
    /// Settings → Sync: the account's password being set.
    PasswordEmail,
    PasswordNew,
}

pub struct Notice {
    pub text: String,
    pub at: Instant,
    pub undo: Option<Undo>,
}

/// What a notice's Undo puts back.
pub enum Undo {
    /// The last change to the open note: the history's own undo.
    Edit,
    /// A note moved to the trash: where its file went, its old name, and
    /// the folder it was in.
    Note {
        file: std::path::PathBuf,
        name: String,
        folder: String,
    },
    /// A folder moved to the trash whole: where it went, and what it was.
    Folder { from: std::path::PathBuf, folder: String },
    /// A note brought back to an older version: the note, and the version
    /// it was at, which a restore of its own brings back.
    Version { note: u64, version: Version },
    /// Several things moved to the trash together: each put back.
    Many(Vec<Undo>),
}

/// A note's history, shown in place of the library: its versions, and
/// the one the page shows, read-only (docs/SYNC.md §9).
pub struct Versions {
    /// The note's id: the history is of this note only.
    pub note: u64,
    /// Newest first; the first is the note as it is.
    pub list: Vec<Revision>,
    /// Which one the page shows.
    pub shown: usize,
    /// What the page draws for it: its words, with what changed marked
    /// (carnet_core::compare).
    pub title: String,
    pub doc: Doc,
    /// Marked against the note as it is now (what a restore would bring
    /// back and take away), not against the version before.
    pub against_now: bool,
    /// The versions' texts, read as a row or the page needs one.
    texts: HashMap<usize, String>,
}

/// Something in the trash, open on the page to read (a folder: its notes'
/// titles), with the ways out: put back, or deleted forever.
pub struct Peek {
    /// Where it is in the trash: what finds it again as the trash changes.
    pub path: std::path::PathBuf,
    pub title: String,
    pub doc: Doc,
    /// The folder it came from: what its pictures' paths are from.
    pub folder: String,
    /// "Deleted 3 days ago, from Work".
    pub what: String,
    pub is_folder: bool,
    /// Delete forever pressed: the line asks to be sure.
    pub armed: bool,
}

/// A press on the page, until it is let go.
#[derive(Clone, Copy, Debug)]
enum Drag {
    /// In text: the selection follows the pointer — a letter at a time,
    /// or, begun by a double or triple click, by the words (or lines) it
    /// chose, which stay selected whichever way it goes.
    Text { unit: Option<(Caret, bool)> },
    /// In the margin: a drag selects whole blocks from the one pressed
    /// beside (`from`), a click places the caret.
    Margin { y0: f32, moved: bool, from: Option<usize> },
}

pub struct Carnet {
    pub store: Store,
    /// Index into `store.notes`; there is always one.
    pub current: usize,
    /// The library's order: indices into `store.notes`. Recomputed only when
    /// the set changes (new, delete, pin, search) — never while typing and
    /// never on a click, so a row is not pulled out from under the pointer.
    pub order: Vec<usize>,
    query: String,
    /// This frame's kui editors. Rebuilt every view.
    pub fields: HashMap<Key, Field>,
    focus_req: Option<Field>,

    // -- the page editor --
    /// Where the caret is in the open note, and what it selects.
    pub caret: Caret,
    /// The page has (or last had) the keyboard: the toolbar acts on the
    /// caret, and the caret is drawn.
    pub editing: bool,
    /// Every note's undo history, by note.
    histories: HashMap<u64, History>,
    /// The page's key sink, as last declared.
    pub page_key: Option<Key>,
    /// Give the page the keyboard on the next frame.
    focus_page: bool,
    /// Each slot's text node, as last drawn: what the caret is measured
    /// against.
    pub slot_keys: HashMap<Slot, Key>,
    /// Lines drawn with their marks hidden: the source's length and the
    /// byte ranges still drawn.
    shown: HashMap<Slot, carnet_core::inline::Visible>,
    /// The colour picker: for the words' own colour (`true`) or a
    /// highlighter, hanging from a point.
    tint_open: Option<(bool, Vec2)>,
    /// Where the toolbar's two colour buttons are: the picker hangs there.
    tint_anchor: [Option<Vec2>; 2],
    /// The toolbar button the picker was opened from, which it keeps
    /// hanging from as the window changes size; `None` for one opened
    /// at a point (the page's menu, More).
    tint_from: Option<usize>,
    /// What typing keeps between keys: a style switched on with nothing
    /// selected, for what is typed next, and spaces carried past a run's
    /// closing marks until the next letter (`page::Typing`).
    pub writing: Typing,
    /// The link editor: what it edits and where it hangs.
    pub link_edit: Option<LinkEdit>,
    /// Where the page's menu last opened.
    menu_at: Vec2,
    /// Each table cell's box as the last frame laid it out: what a click is
    /// matched against.
    cell_rects: HashMap<Slot, Rect>,
    /// Each slot's text as last drawn (a hash): a caret is measured only
    /// against text the last layout had.
    drawn: HashMap<Slot, u64>,
    /// The x the caret keeps to while it goes up and down.
    goal_x: Option<f32>,
    drag: Option<Drag>,
    /// Where the page's drag is: the pointer as last heard, where the
    /// press was, and the frame clock's time of the last edge step. Held
    /// past the page's top or bottom, the page scrolls under it.
    drag_at: Option<(Vec2, Vec2, f64)>,
    /// Each block's row as the last frame laid it out, in document order:
    /// what a margin drag's `y` is read against.
    pub row_rects: Vec<(u64, Rect)>,
    /// An IME composition under way, drawn at the caret.
    preedit: Option<String>,
    /// Scroll the caret into view on the next frame.
    reveal: bool,
    /// A paste asked for and not answered yet.
    paste_pending: bool,
    /// Column the last sort used, and whether it was descending.
    last_sort: Option<(u64, bool)>,

    last_edit: Instant,
    pub error: Option<String>,
    words: (u64, u64, usize),
    /// Checklist progress of the open note, (done, all): kept with `words`.
    todos: Option<(usize, usize)>,
    styler: Styler,
    last_focus: Option<Key>,
    /// The title has the caret: the file is not renamed while it does, so
    /// it is not renamed letter by letter.
    title_focused: bool,
    /// Empty the search box on the next frame.
    clear_search: bool,
    /// One line under the title for what just happened, with the way back.
    notice: Option<Notice>,
    /// Said once by a screen reader on the next frame.
    say: Option<String>,
    /// Move along the Tab ring on the next frame: forward, or back.
    step: Option<bool>,
    /// Drop keyboard focus on the next frame.
    blur: bool,
    /// Text to put on the clipboard on the next frame, and its HTML.
    clip_out: Option<(String, Option<String>)>,
    /// A context menu to open on the next frame: the node it is about,
    /// where, and what.
    pending_menu: Option<(Key, Vec2, MenuFor)>,
    /// On a narrow window: the list of notes is showing, not a note.
    list_view: bool,
    /// The folder the library shows; `None` shows every note.
    pub folder: Option<String>,
    /// Notes and folders chosen together in the library (⇧- or ⌘-click),
    /// to go to the trash or a folder at once; empty when one note is
    /// simply open.
    pub picked: Vec<Pick>,
    /// The row a ⇧-click's range runs from.
    pick_anchor: Option<Pick>,
    /// A new folder being named.
    naming: Option<Naming>,
    /// How many folders have been named: each naming has a field of its
    /// own, so a name typed before never shows in the next one.
    namings: u64,
    /// The folder list's rows as last drawn, in order, by key: what a key
    /// pressed on one of them acts on.
    pub(crate) folder_rows: Vec<(Key, FolderRow)>,
    /// A row to give the keyboard to the frame it is drawn: after an
    /// arrow, a rename, or a delete that took the row it was on.
    focus_row: Option<FolderRow>,
    /// A folder row was the last thing clicked, and the keyboard has gone
    /// nowhere since: ↑ and ↓ walk the folders from the one shown.
    folders_clicked: bool,
    pub settings: Settings,
    /// The settings show in place of the note.
    pub settings_open: bool,
    /// Reset in Settings pressed: its line asks to be sure.
    pub reset_armed: bool,
    /// The font list's search to be emptied on the next frame: it opens
    /// afresh each time.
    font_search_reset: bool,
    /// Notes whose made time was looked up in their history.
    dated: std::collections::HashSet<u64>,
    /// What the font list is narrowed to.
    font_query: String,
    /// The font list open in the settings: for the body (0), the headings
    /// (1), the dropped letter (2).
    font_pick: Option<u64>,
    /// The font list's rows as last drawn: what `Msg::SetFont` indexes.
    font_rows: Vec<String>,
    /// Every installed family, read once when the list first opens.
    families: Option<Vec<String>>,
    /// Faces by name, for drawing each font's name in itself.
    font_faces: HashMap<String, kui_native::FontFamily>,
    /// Ask for a folder on the next frame.
    pick_dir: bool,
    /// Move to another notes folder on the next frame (`None`: the
    /// default one).
    dir_change: Option<Option<std::path::PathBuf>>,
    /// The pictures the notes show.
    images: Images,
    /// The page's text width, as last laid out: what a picture fits.
    text_w: f32,
    /// The paragraphs whose first letters are dropped, this frame.
    initial: std::collections::HashSet<Slot>,
    /// How far the leftmost strip's content keeps from the window's left
    /// edge: past the macOS traffic lights under custom chrome, else 0.
    pub lights: f32,
    /// A context menu is up: the window's drag regions stand down, so a
    /// press on them dismisses it (the OS would take it for a drag).
    menu_open: bool,
    /// The page had the keyboard when the window lost it: it gets it back
    /// when the window does.
    refocus: bool,
    /// The `/` menu's highlighted row, as a position in what it shows.
    slash_sel: usize,
    /// The word the `/` menu last showed for: a new word starts it over.
    slash_word: String,
    /// The line whose `/` menu Esc closed, until it stops being one.
    slash_off: Option<u64>,
    /// Ask for picture files on the next frame.
    pick_pictures: bool,
    /// Files being dragged over the window, until they drop or leave.
    file_drag: Option<FileDrag>,
    /// Where dropped pictures would go: before this block (the page's
    /// length for after the last). Drawn as a line.
    pub drop_index: Option<usize>,
    /// The library's cards as last laid out: what a dragged file hovers.
    card_rects: Vec<(usize, Rect)>,
    /// The page's scroller as last laid out: its edges scroll a drag.
    page_rect: Option<Rect>,
    /// The page's scroller, as last declared (the body's key sink is
    /// `page_key`, inside it).
    scroll_key: Option<Key>,
    /// A picture's width being dragged: which, from what, where the drag
    /// began, and the width now.
    resizing: Option<Resize>,
    /// Each picture's width as last drawn, by block id, and the widths it
    /// would have with none given and at the most.
    pic_w: HashMap<u64, (f32, f32, f32)>,
    /// While a row of floated pictures is drawn: how many share it, and
    /// the row's width.
    float_count: usize,
    float_room: f32,
    /// Each floated picture's row as last drawn: where the blocks beside
    /// it end, and how many frames running that changed.
    float_ends: HashMap<u64, (usize, u8)>,
    /// A note being carried from its card to a folder.
    note_drag: Option<NoteDrag>,
    /// The library's edge being dragged: where the drag began and the
    /// width then.
    library_from: Option<(f32, u32)>,
    /// Since when (the frame clock's seconds) the pointer has rested on
    /// the library's edge, for its hint; infinite after a drag, until the
    /// pointer leaves it.
    edge_hover: Option<f64>,
    /// A picture being carried on the page.
    pub pic_drag: Option<PicDrag>,
    /// The folder rows as last laid out, by folder ("" for All notes,
    /// which takes a note to the top level).
    folder_rects: Vec<(String, Rect)>,
    /// The tools the strip had no room for, in the More menu.
    overflow: Vec<chrome::ToolDef>,
    /// The More button, and where its menu opens.
    more: Option<(Key, Vec2)>,
    /// Debug information: kui's frame-time graph (Shift+F12, or five
    /// quick clicks on the Settings page's title, so a release build has
    /// it too); and, while it shows, F12's devtools in a release build.
    /// For this run only: never saved.
    pub hud: bool,
    /// The clicks on the Settings page's title so far, and when the last
    /// was: five in quick succession turn the debug information on or off.
    title_clicks: (u32, Instant),
    /// Words are going down: the last thing the reader did was type on
    /// the page. The toolbar fades and the other blocks step back while
    /// it holds (as the settings ask); the pointer, a click or anything
    /// that is not typing ends it.
    pub typing: bool,
    /// When the last of that typing was, on the frame clock.
    typed_at: f64,
    /// Where the pointer was as the typing began (`Some(None)`: outside
    /// the window): moved on from there, the reader is reaching for
    /// something, and the typing is over.
    typing_cursor: Option<Option<Vec2>>,
    /// The caret's line was measured by the last layout, as drawn now:
    /// where it is can be read to scroll by.
    caret_fresh: bool,
    /// Frames the caret has been waited for, to scroll to it.
    reveal_wait: u8,
    /// Zen, for as long as the reader keeps it: nothing on the screen
    /// but the note.
    pub zen: bool,
    /// The library shows the Deleted folder in place of the notes.
    pub deleted: bool,
    /// Where the title's caret lands as it takes the keyboard from the
    /// body: `None` at its end (← from the note's start), a column for ↑
    /// from the note's first line, under the caret it came from.
    title_to: Option<Option<f32>>,
    /// The search had the keyboard on the last frame: its field shows
    /// whole.
    search_active: bool,
    /// Something for the window to do on the next frame (the menu bar's
    /// Quit, Minimize, Zoom).
    window_cmd: Option<kui_native::WindowCommand>,
    /// What is in the trash, as last read: the Deleted folder's rows.
    pub trash: Vec<store::Trashed>,
    /// The open note's history, while it shows.
    pub versions: Option<Versions>,
    /// Something in the trash, open to read.
    pub peek: Option<Peek>,
    /// Sync, off until Settings → Sync sets it up (docs/SYNC.md §5).
    pub sync: crate::sync::Sync,
    /// What Settings → Sync's fields hold.
    pub sync_server: String,
    pub sync_code: String,
    pub sync_how: u64,
    /// The way in by email and password: what's typed.
    pub sync_email: String,
    pub sync_password: String,
    /// The password fields show their text, not dots.
    pub sync_show_password: bool,
    /// A password field's dots stand for new text: it is to be drawn as
    /// dots again.
    sync_mask_reset: bool,
    /// The account's password being set: its email and password fields.
    pub pw_edit: Option<(String, String)>,
    /// Taking the password away was asked once.
    pub pw_remove_armed: bool,
    /// Deleting the account was asked once.
    pub delete_account_armed: bool,
    /// Turn off asked once: the second asks for real.
    pub sync_off_armed: bool,
    /// The device whose Remove was asked once.
    pub sync_remove_armed: Option<Vec<u8>>,
    /// The passkey whose Remove was asked once.
    pub sync_passkey_armed: Option<[u8; 32]>,
    /// The account whose Remove was asked once.
    pub sync_account_armed: Option<Vec<u8>>,
    /// This device's name as typed, while it's being changed.
    pub sync_rename: Option<String>,
    /// The pairing code shown as a QR code: the link it carries, and its
    /// picture, made once per code.
    pub pair_qr: Option<(String, kui_native::ImageId)>,
    /// Settings → Sync's fields are to show `sync_server` and `sync_code`
    /// when next drawn: a field keeps its own text, so changing ours is not
    /// enough.
    sync_reset_fields: bool,
    /// The sync inspector is open in Settings → Sync.
    pub sync_inspector: bool,
    /// A rollback (false) or repair (true) of this room, awaiting yes.
    pub sync_inspect_armed: Option<(String, bool)>,
    /// The room whose ways to fix it are shown.
    pub sync_inspect_open: Option<String>,
    /// The empty note put in when joining took the welcome note away:
    /// gone again, untouched, once the account's notes come.
    sync_stand_in: Option<u64>,

}

/// A folder being named — a new one, or one renamed: the folder it goes
/// in, the name so far, and its field — the `namings`th, declared under its
/// own key so it opens with this naming's text whatever kui kept of an
/// earlier one.
#[derive(Clone, Debug)]
pub struct Naming {
    pub parent: String,
    pub name: String,
    pub nth: u64,
    pub key: Option<Key>,
    /// The folder renamed, its field in its row's place; `None` for a new
    /// one.
    pub rename: Option<String>,
}

/// A row of the folder list the keyboard can be on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderRow {
    All,
    Folder(String),
    Deleted,
}

/// The link being edited: its address so far, whether the caret was in a
/// link already (else the selection becomes one), and where the editor
/// hangs.
#[derive(Clone, Debug)]
pub struct LinkEdit {
    /// The selection it was opened on: the field takes the keyboard.
    pub caret: Caret,
    pub url: String,
    pub existing: bool,
    /// Under the caret, found when it is first drawn.
    pub at: Option<Vec2>,
}

/// A picture being carried on the page: from where, where the pointer is,
/// whether it has travelled, and where it would land.
#[derive(Clone, Debug)]
pub struct PicDrag {
    pub id: u64,
    pub from: Vec2,
    pub at: Vec2,
    pub moved: bool,
    pub target: Option<page::PicDrop>,
}

/// A note's card being dragged.
#[derive(Clone, Debug)]
struct NoteDrag {
    id: u64,
    from: Vec2,
    at: Vec2,
    /// Past a few pixels: a drag, not a click.
    moved: bool,
    /// The folder row under the pointer.
    target: Option<String>,
}

/// Files over the window.
struct FileDrag {
    /// The pointer, as the OS (or kui) last reported it.
    at: Option<Vec2>,
    /// The card it has been resting on, and since when: held long
    /// enough, that note opens under it.
    card: Option<(usize, Instant)>,
}

/// How long a dragged file rests on a note's card before the note opens.
const SPRING: Duration = Duration::from_millis(650);

#[derive(Clone, Copy, Debug)]
struct Resize {
    id: u64,
    w0: f32,
    x0: f32,
    w: f32,
    /// A handle on the left edge: going left widens.
    left: bool,
}

/// A drag region of the custom titlebar, except while a menu is up.
trait DragRegion {
    fn window_drag_unless(self, menu_open: bool) -> Self;
}

impl DragRegion for NodeSpec {
    fn window_drag_unless(self, menu_open: bool) -> Self {
        if menu_open { self } else { self.window_drag() }
    }
}

impl Carnet {
    /// With the default settings, kept nowhere: what the tests run.
    #[cfg(test)]
    pub fn new(store: Store) -> Self {
        Self::with_settings(store, Settings::default())
    }

    pub fn with_settings(store: Store, settings: Settings) -> Self {
        let mut app = Self {
            store,
            current: 0,
            order: Vec::new(),
            query: String::new(),
            fields: HashMap::new(),
            focus_req: None,
            caret: Caret::at(page::start(Slot::block(0))),
            editing: false,
            histories: HashMap::new(),
            page_key: None,
            focus_page: false,
            slot_keys: HashMap::new(),
            shown: HashMap::new(),
            tint_open: None,
            tint_anchor: [None, None],
            tint_from: None,
            writing: Typing::default(),
            link_edit: None,
            menu_at: Vec2::new(0.0, 0.0),
            cell_rects: HashMap::new(),
            drawn: HashMap::new(),
            goal_x: None,
            drag: None,
            drag_at: None,
            row_rects: Vec::new(),
            preedit: None,
            reveal: false,
            paste_pending: false,
            last_sort: None,
            last_edit: Instant::now(),
            error: None,
            words: (u64::MAX, 0, 0),
            todos: None,
            styler: Styler::default(),
            last_focus: None,
            title_focused: false,
            clear_search: false,
            notice: None,
            say: None,
            step: None,
            blur: false,
            clip_out: None,
            pending_menu: None,
            list_view: false,
            folder: None,
            picked: Vec::new(),
            pick_anchor: None,
            naming: None,
            namings: 0,
            folder_rows: Vec::new(),
            focus_row: None,
            folders_clicked: false,
            settings,
            settings_open: false,
            reset_armed: false,
            font_search_reset: false,
            dated: Default::default(),
            font_query: String::new(),
            font_pick: None,
            font_rows: Vec::new(),
            families: None,
            font_faces: HashMap::new(),
            pick_dir: false,
            dir_change: None,
            images: Images::default(),
            text_w: 600.0,
            initial: Default::default(),
            menu_open: false,
            lights: 0.0,
            refocus: false,
            slash_sel: 0,
            slash_word: String::new(),
            slash_off: None,
            pick_pictures: false,
            file_drag: None,
            drop_index: None,
            card_rects: Vec::new(),
            page_rect: None,
            scroll_key: None,
            resizing: None,
            pic_w: HashMap::new(),
            float_count: 0,
            float_room: 0.0,
            float_ends: HashMap::new(),
            note_drag: None,
            library_from: None,
            edge_hover: None,
            pic_drag: None,
            folder_rects: Vec::new(),
            overflow: Vec::new(),
            more: None,
            hud: false,
            title_clicks: (0, Instant::now()),
            typing: false,
            typed_at: 0.0,
            typing_cursor: None,
            caret_fresh: false,
            reveal_wait: 0,
            zen: false,
            deleted: false,
            title_to: None,
            window_cmd: None,
            search_active: false,
            trash: Vec::new(),
            versions: None,
            peek: None,
            sync: crate::sync::Sync::off(),
            sync_server: String::new(),
            sync_code: String::new(),
            sync_how: 0,
            sync_email: String::new(),
            sync_password: String::new(),
            sync_show_password: false,
            sync_mask_reset: false,
            pw_edit: None,
            pw_remove_armed: false,
            delete_account_armed: false,
            sync_off_armed: false,
            sync_remove_armed: None,
            sync_passkey_armed: None,
            sync_account_armed: None,
            sync_rename: None,
            pair_qr: None,
            sync_reset_fields: false,
            sync_inspector: false,
            sync_inspect_armed: None,
            sync_inspect_open: None,
            sync_stand_in: None,
        };
        if app.store.notes.is_empty() {
            app.store.notes.push(blank_note());
        }
        if !app.store.skipped.is_empty() {
            app.notify(
                format!(
                    "Left {} file(s) alone that aren't UTF-8: {}",
                    app.store.skipped.len(),
                    app.store.skipped.join(", ")
                ),
                None,
            );
        }
        app.trash = app.store.trashed();
        app.reorder();
        app.current = app.order.first().copied().unwrap_or(0);
        app.caret = Caret::at(page::first(app.doc()));
        app.gather_pictures();
        // The notes open all the same; their history waits for an update.
        if app.error.is_none() {
            app.error = app.store.replica_refused.clone();
        }
        app
    }

    /// Pictures an export brought in an `Attachments` folder of its own go
    /// to the attachments folder, where Carnet keeps them (and syncs them
    /// from), the notes' links with them; and it says so.
    fn gather_pictures(&mut self) {
        let g = self.store.gather_pictures();
        if g.files == 0 {
            return;
        }
        // A note reread with its new links is not the one its history
        // describes.
        self.histories.retain(|id, _| !g.notes.contains(id));
        self.caret = page::fix(self.doc(), self.caret);
        let others = g.files - g.pictures;
        let n = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
        let what = match (g.pictures, others) {
            (p, 0) => n(p, "picture", "pictures"),
            (0, f) => n(f, "file", "files"),
            (p, f) => format!("{} and {}", n(p, "picture", "pictures"), n(f, "other file", "other files")),
        };
        self.notify(format!("Moved {what} to the attachments folder, links and all"), None);
    }

    /// What the page shows: the open note, or the version of it its
    /// history shows.
    pub fn doc(&self) -> &Doc {
        if let Some(p) = &self.peek {
            return &p.doc;
        }
        match &self.versions {
            Some(v) if v.note == self.store.notes[self.current].id => &v.doc,
            _ => &self.store.notes[self.current].doc,
        }
    }

    fn note(&mut self) -> &mut Note {
        &mut self.store.notes[self.current]
    }

    fn notify(&mut self, text: impl Into<String>, undo: Option<Undo>) {
        let text = text.into();
        self.say = Some(text.clone());
        self.notice = Some(Notice {
            text,
            at: Instant::now(),
            undo,
        });
    }

    /// Pinned first, then newest; the folder's notes — or, while there is
    /// a search, every note (each card names its folder), so a note is
    /// never "not found" for being in another folder.
    fn reorder(&mut self) {
        let q = self.query.trim().to_lowercase();
        let folder = if q.is_empty() { self.folder.as_deref() } else { None };
        let mut order: Vec<usize> = (0..self.store.notes.len())
            .filter(|&i| {
                let n = &self.store.notes[i];
                folder.is_none_or(|f| n.folder == f)
                    && (q.is_empty() || n.title.to_lowercase().contains(&q) || n.doc.contains(&q))
            })
            .collect();
        let notes = &self.store.notes;
        let sort = self.settings.sort;
        order.sort_by(|&a, &b| {
            let then = match sort {
                Sort::Title => {
                    let key = |i: usize| notes[i].display_title().to_lowercase();
                    key(a).cmp(&key(b))
                }
                Sort::Created => notes[b].created.cmp(&notes[a].created),
                Sort::Edited => notes[b].modified.cmp(&notes[a].modified),
            };
            notes[b].pinned.cmp(&notes[a].pinned).then(then)
        });
        self.order = order;
    }

    /// When the open note was made: its file's birth time, or its
    /// history's first version where that is older (a file saved before
    /// births were kept was born at its last save). Looked up once a note.
    pub(crate) fn date_created(&mut self) -> SystemTime {
        let i = self.current;
        let id = self.store.notes[i].id;
        if self.dated.insert(id)
            && let Some(first) = self.store.first_version(i)
        {
            let note = &mut self.store.notes[i];
            note.created = note.created.min(first);
        }
        self.store.notes[i].created
    }

    fn edited(&mut self) {
        self.note().touch();
        self.last_edit = Instant::now();
    }

    // ------------------------------------------------------------ editing

    /// The pointer moving on — a hand gone to the mouse — ends the typing,
    /// and the toolbar is back. A nudge of a few pixels does not.
    fn follow_pointer(&mut self, ui: &mut Ui<'_>) {
        // Asked of the OS where it says: kui hears no move while the
        // system hides the pointer for typing.
        let at = crate::oscursor::on_screen().or_else(|| ui.core().cursor());
        if !self.typing {
            self.typing_cursor = None;
            return;
        }
        let moved = match (self.typing_cursor, at) {
            (None, _) => {
                self.typing_cursor = Some(at);
                false
            }
            (Some(Some(from)), Some(p)) => (p.x - from.x).hypot(p.y - from.y) > POINTER_NUDGE,
            // Come into the window, or gone out of it.
            (Some(from), p) => from.is_some() != p.is_some(),
        };
        if moved {
            self.typing = false;
            self.typing_cursor = None;
        }
    }

    /// How much of the window's width the library takes beside the page.
    pub(super) fn side_w(&self, narrow: bool) -> f32 {
        if narrow || !self.side_shown() { 0.0 } else { self.library_w() }
    }

    /// The library's width, as last dragged.
    pub(super) fn library_w(&self) -> f32 {
        self.settings.library_w as f32
    }

    /// The library beside the page: as the settings say, and never in zen.
    pub(super) fn side_shown(&self) -> bool {
        self.settings.sidebar && !self.zen
    }

    /// Focus — the blocks not being written step back — as the settings
    /// say, or for zen.
    pub(super) fn focus_on(&self) -> bool {
        self.settings.focus || self.zen
    }

    /// The line being written held near the middle: as the settings say,
    /// or for zen.
    pub(super) fn typewriter_on(&self) -> bool {
        self.settings.typewriter || self.zen
    }

    /// Writing undisturbed: the last thing done was typing on the page,
    /// with nothing selected and nothing open over it.
    pub(super) fn in_flow(&self) -> bool {
        self.typing
            && self.editing
            && self.caret.is_empty()
            && !self.menu_open
            && !self.settings_open
            && self.tint_open.is_none()
            && self.link_edit.is_none()
            && self.drag.is_none()
            && self.pic_drag.is_none()
    }

    fn history(&mut self) -> &mut History {
        let id = self.store.notes[self.current].id;
        self.histories.entry(id).or_default()
    }

    /// Runs an edit on the open note: the state before it goes into the
    /// history when it changed anything, and the caret moves either way.
    fn edit(&mut self, step: Step, f: impl FnOnce(&mut Doc, &mut Caret) -> bool) -> bool {
        // A version in the history, or a note in the trash, is read, not
        // written.
        if self.reading() {
            return false;
        }
        let id = self.store.notes[self.current].id;
        let before = State::of(self.doc(), self.caret);
        let block = self.caret.head.slot.block;
        let mut caret = self.caret;
        let changed = f(&mut self.store.notes[self.current].doc, &mut caret);
        self.caret = caret;
        if step != Step::Typing {
            self.writing.moved();
        }
        if changed {
            // A notice's Undo of the last edit means that edit, no other.
            if self.notice.as_ref().is_some_and(|n| matches!(n.undo, Some(Undo::Edit))) {
                self.notice = None;
            }
            self.histories.entry(id).or_default().record(step, block, before);
            self.edited();
        } else {
            self.history().seal();
        }
        self.goal_x = None;
        self.reveal = true;
        changed
    }

    /// The caret goes somewhere without an edit: the typing run under way
    /// ends there.
    fn move_to(&mut self, c: Caret) {
        self.caret = c;
        self.writing.moved();
        self.history().seal();
        self.reveal = true;
    }

    fn undo_edit(&mut self) {
        if self.reading() {
            return;
        }
        let now = State::of(self.doc(), self.caret);
        if let Some(s) = self.history().undo(now) {
            self.restore(s);
        }
    }

    fn redo_edit(&mut self) {
        if self.reading() {
            return;
        }
        let now = State::of(self.doc(), self.caret);
        if let Some(s) = self.history().redo(now) {
            self.restore(s);
        }
    }

    fn restore(&mut self, s: State) {
        self.store.notes[self.current].doc.restore(s.blocks);
        self.caret = page::fix(self.doc(), s.caret);
        self.goal_x = None;
        self.reveal = true;
        self.focus_page = true;
        self.edited();
    }

    fn focus(&mut self, field: Field) {
        // The search and a new folder's name are the library's: it shows.
        if matches!(field, Field::Search | Field::Folder) && !self.side_shown() {
            self.zen = false;
            if !self.settings.sidebar {
                self.on_settings(Msg::ToggleSidebar);
            }
        }
        self.focus_req = Some(field);
    }

    /// The caret into the page, at `p`.
    fn edit_at(&mut self, p: Pos) {
        self.caret = Caret::at(p);
        self.focus_page = true;
        self.editing = true;
        self.reveal = true;
    }

    // ------------------------------------------------------------ notes

    fn save_current(&mut self) {
        let i = self.current;
        let note = &mut self.store.notes[i];
        if note.is_blank() && note.path.is_none() {
            // Nothing to keep yet; it is not "saving" either.
            note.dirty = false;
            return;
        }
        // Not renamed while the title is being typed.
        let rename = !self.title_focused;
        match self.store.save(i, rename) {
            Ok(Saved::Merged) => {
                self.error = None;
                self.notify("Changed in another app too — the two are merged", None);
                // The page is the merged note now: its undo history
                // describes one that is gone.
                let id = self.store.notes[self.current].id;
                self.histories.remove(&id);
                self.caret = page::fix(self.doc(), self.caret);
                self.reorder();
            }
            Ok(Saved::Conflict(copy)) => {
                self.error = None;
                let name = self.store.notes[copy].display_title().to_string();
                self.notify(
                    format!("Changed in another app too — your version is kept as \u{201c}{name}\u{201d}"),
                    None,
                );
                let id = self.store.notes[self.current].id;
                self.histories.remove(&id);
                self.caret = page::fix(self.doc(), self.caret);
                self.reorder();
            }
            Ok(_) => self.error = None,
            Err(e) => self.error = Some(format!("Couldn't save: {e}")),
        }
    }

    /// Catches up with edits made to the folder in other apps. The open
    /// note is followed by id; if it went, the first in the list opens.
    fn rescan(&mut self) {
        let open = self.store.notes.get(self.current).map(|n| n.id);
        let changed = self.store.rescan();
        self.gather_pictures();
        // A picture put beside a note meanwhile goes to the vault soon.
        self.sync.poke();
        if !changed {
            return;
        }
        // The folder shown went in another app: show them all.
        if self
            .folder
            .as_ref()
            .is_some_and(|f| !self.store.folders.contains(f))
        {
            self.folder = None;
        }
        // A note reread from disk is not the one its history describes:
        // only a note still being edited here keeps its history.
        let editing: Vec<u64> = self
            .store
            .notes
            .iter()
            .filter(|n| n.dirty)
            .map(|n| n.id)
            .collect();
        self.histories.retain(|id, _| editing.contains(id));
        self.reorder();
        match open.and_then(|id| self.store.notes.iter().position(|n| n.id == id)) {
            Some(i) => self.current = i,
            None => {
                if self.store.notes.is_empty() {
                    self.store.notes.push(blank_note());
                    self.reorder();
                }
                self.current = self.order.first().copied().unwrap_or(0);
            }
        }
        self.caret = page::fix(self.doc(), self.caret);
    }

    /// Leaves the current note: saves it, or forgets it when it was a new
    /// note nobody wrote anything in.
    fn leave_current(&mut self) {
        let i = self.current;
        let n = &self.store.notes[i];
        if n.is_blank() && n.path.is_none() && self.store.notes.len() > 1 {
            self.store.notes.remove(i);
            self.order.retain(|&j| j != i);
            for j in &mut self.order {
                if *j > i {
                    *j -= 1;
                }
            }
            if self.current > i {
                self.current -= 1;
            }
            return;
        }
        self.save_current();
    }

    fn open(&mut self, note: usize) {
        self.folders_clicked = false;
        // Another note (⌥⌘↓ comes from the keyboard): not being written yet.
        self.typing = false;
        // On a narrow window, choosing the open note goes back to it too;
        // from the settings, it goes back to the note.
        self.list_view = false;
        self.settings_open = false;
        if note == self.current || note >= self.store.notes.len() {
            return;
        }
        // "Removed · Undo" was about the note being left.
        if self.notice.as_ref().is_some_and(|n| matches!(n.undo, Some(Undo::Edit))) {
            self.notice = None;
        }
        let id = self.store.notes[note].id;
        self.close_history();
        self.leave_current();
        self.current = self
            .store
            .notes
            .iter()
            .position(|n| n.id == id)
            .unwrap_or(0);
        // Edited in another app since it was read, and not here: show
        // what is on disk now.
        if self.store.refresh(self.current) {
            self.histories.remove(&id);
        }
        self.caret = Caret::at(page::first(self.doc()));
        self.editing = false;
        self.focus_req = None;
        self.last_sort = None;
        self.preedit = None;
    }

    /// A new note, in the folder the library shows.
    fn new_note(&mut self) {
        self.close_history();
        self.settings_open = false;
        self.deleted = false;
        self.close_peek();
        self.leave_current();
        let mut note = blank_note();
        note.folder = self.folder.clone().unwrap_or_default();
        self.store.notes.push(note);
        self.current = self.store.notes.len() - 1;
        self.caret = Caret::at(page::first(self.doc()));
        self.editing = false;
        self.list_view = false;
        if !self.query.is_empty() {
            self.query.clear();
            self.clear_search = true;
        }
        self.reorder();
        self.focus(Field::Title);
    }

    /// Moves note `i` to the trash. Deleting the open note opens the one
    /// that took its place in the library; deleting another leaves the
    /// open one open.
    fn delete_note_at(&mut self, i: usize) {
        let was_current = i == self.current;
        let open_id = self.store.notes[self.current].id;
        let pos = self.order.iter().position(|&j| j == i).unwrap_or(0);
        // What was typed since the last save goes to the trash with it,
        // so Undo brings back the note as it was, not as it was saved.
        let note = &self.store.notes[i];
        let name = note.display_title().to_string();
        if note.dirty
            && !(note.path.is_none() && note.is_blank())
            && let Err(e) = self.store.save(i, false)
        {
            // What was typed is not on disk: trashing the file now would
            // throw it away. The note stays, and says why.
            self.error = Some(format!("Couldn't save \u{201c}{name}\u{201d}, so it stays: {e}"));
            return;
        }
        let file_name = self.store.notes[i]
            .path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let folder = self.store.notes[i].folder.clone();
        match self.store.trash(i) {
            Ok(Some(file)) => self.notify(
                format!("Moved \u{201c}{name}\u{201d} to the trash"),
                file_name.map(|name| Undo::Note { file, name, folder }),
            ),
            Ok(None) => {}
            // Nothing was forgotten; the list and the open note are put
            // back in step all the same.
            Err(e) => self.error = Some(format!("Couldn't move to trash: {e}")),
        }
        if self.store.notes.is_empty() {
            self.store.notes.push(blank_note());
        }
        self.reorder();
        let still_there = self.store.notes.iter().position(|n| n.id == open_id);
        self.current = match still_there {
            Some(j) => j,
            None => self
                .order
                .get(pos.min(self.order.len().saturating_sub(1)))
                .copied()
                .unwrap_or(0),
        };
        if was_current {
            self.caret = Caret::at(page::first(self.doc()));
            self.editing = false;
        }
        self.refresh_trash();
    }

    /// What the notice's Undo names, put back.
    fn notice_undo(&mut self) {
        let Some(undo) = self.notice.take().and_then(|n| n.undo) else {
            return;
        };
        self.undo(undo);
        self.refresh_trash();
    }

    fn undo(&mut self, undo: Undo) {
        match undo {
            // The last first, as they went.
            Undo::Many(all) => {
                for u in all.into_iter().rev() {
                    self.undo(u);
                }
            }
            Undo::Edit => {
                self.undo_edit();
                self.say = Some("Restored".into());
            }
            Undo::Note { file, name, folder } => match self.store.restore(&file, &name, &folder) {
                Ok(i) => {
                    let id = self.store.notes[i].id;
                    self.reorder();
                    let i = self.store.notes.iter().position(|n| n.id == id).unwrap_or(i);
                    self.open(i);
                    self.say = Some("Restored".into());
                }
                Err(e) => self.error = Some(format!("Couldn't restore: {e}")),
            },
            Undo::Version { note, version } => {
                if let Some(i) = self.store.notes.iter().position(|n| n.id == note) {
                    match self.store.restore_version(i, &version) {
                        Ok(()) => {
                            self.histories.remove(&note);
                            if i == self.current {
                                self.caret = Caret::at(page::first(self.doc()));
                            }
                            self.reorder();
                            self.say = Some("Restored".into());
                        }
                        Err(e) => self.error = Some(format!("Couldn't restore: {e}")),
                    }
                }
            }
            Undo::Folder { from, folder } => match self.store.restore_folder(&from, &folder) {
                Ok(()) => {
                    self.reorder();
                    self.say = Some("Restored".into());
                }
                Err(e) => self.error = Some(format!("Couldn't restore the folder: {e}")),
            },
        }
    }

    // ---------------------------------------------------------- history

    /// The open note's history in place of the library, the newest
    /// version on the page. Its edits are saved first: the history ends
    /// with the note as it is.
    pub(super) fn show_history(&mut self) {
        self.save_current();
        if self.store.notes[self.current].dirty {
            return;
        }
        let list = self.store.history(self.current);
        if list.is_empty() {
            self.notify("This note has no history yet", None);
            return;
        }
        self.settings_open = false;
        self.deleted = false;
        self.close_peek();
        self.list_view = true;
        self.versions = Some(Versions {
            note: self.store.notes[self.current].id,
            list,
            shown: 0,
            title: String::new(),
            doc: Doc::default(),
            against_now: false,
            texts: HashMap::new(),
        });
        self.show_version(0);
    }

    /// The `i`th version's text, read once.
    pub(super) fn version_text(&mut self, i: usize) -> Option<String> {
        let v = self.versions.as_mut()?;
        if let Some(t) = v.texts.get(&i) {
            return Some(t.clone());
        }
        let version = v.list.get(i)?.version.clone();
        let text = self.store.text_at(self.current, &version)?;
        self.versions.as_mut()?.texts.insert(i, text.clone());
        Some(text)
    }

    /// The page shows the `i`th version, what changed in it marked: since
    /// the version before it, or, compared with now, what a restore would
    /// bring back (added) and take away (removed). The first version has
    /// nothing before it, and is shown as it is.
    fn show_version(&mut self, i: usize) {
        let Some(text) = self.version_text(i) else { return };
        let (against_now, count) = self.versions.as_ref().map_or((false, 0), |v| (v.against_now, v.list.len()));
        let base = if against_now && i != 0 {
            self.version_text(0)
        } else if i + 1 < count {
            self.version_text(i + 1)
        } else {
            None
        };
        let (title, doc) = match base {
            Some(old) => carnet_core::compare::marked(&old, &text),
            None => carnet_core::doc::from_markdown(&text),
        };
        let fallback = self.store.notes[self.current].display_title().to_string();
        let Some(v) = self.versions.as_mut() else { return };
        v.shown = i;
        v.title = title.unwrap_or(fallback);
        v.doc = doc;
        self.caret = Caret::at(page::first(self.doc()));
        self.editing = false;
        self.focus_page = false;
        self.reveal = true;
    }

    /// Back to the note as it is, and the notes.
    pub(super) fn close_history(&mut self) {
        if self.versions.take().is_some() {
            self.caret = Caret::at(page::first(self.doc()));
            self.editing = false;
        }
        self.close_peek();
    }

    /// The page shows a version or a deleted note: read, not written.
    pub(super) fn reading(&self) -> bool {
        self.versions.is_some() || self.peek.is_some()
    }

    /// The folder the page's pictures' paths are from.
    pub(super) fn page_folder(&self) -> String {
        match &self.peek {
            Some(p) => p.folder.clone(),
            None => self.store.notes[self.current].folder.clone(),
        }
    }

    /// `trash[i]` on the page, to read.
    fn open_deleted(&mut self, i: usize) {
        let Some(t) = self.trash.get(i).cloned() else { return };
        let from = if t.folder.is_empty() { "All notes".to_string() } else { folder_name(&t.folder).to_string() };
        let ago = ago(std::time::SystemTime::now(), t.when);
        let (title, doc) = if t.is_folder {
            let titles = self.store.trashed_titles(&t);
            let mut blocks: Vec<Block> =
                titles.iter().map(|n| Block::new(Kind::Item { style: ListStyle::Bullet, indent: 0, done: false, num: 0, text: n.clone() })).collect();
            if blocks.is_empty() {
                blocks.push(Block::text("No notes in it"));
            }
            (t.title.clone(), Doc::new(blocks))
        } else {
            match Note::load_file(&t.path) {
                Ok(n) => (n.title.clone(), n.doc),
                Err(e) => {
                    self.error = Some(format!("Couldn't read it: {e}"));
                    return;
                }
            }
        };
        self.close_history();
        self.peek = Some(Peek {
            path: t.path.clone(),
            title,
            doc,
            folder: t.folder.clone(),
            what: format!("Deleted {ago}, from {from}"),
            is_folder: t.is_folder,
            armed: false,
        });
        self.caret = Caret::at(page::first(self.doc()));
        self.editing = false;
    }

    fn close_peek(&mut self) {
        if self.peek.take().is_some() {
            self.caret = Caret::at(page::first(self.doc()));
            self.editing = false;
        }
    }

    /// Where the open deleted thing is in the trash now.
    fn peek_index(&self) -> Option<usize> {
        let p = self.peek.as_ref()?;
        self.trash.iter().position(|t| t.path == p.path)
    }

    /// The deleted thing on the page, deleted forever: its file, its
    /// history, here and on every device; the pictures only it showed too.
    fn delete_forever(&mut self) {
        let Some(t) = self.peek_index().and_then(|i| self.trash.get(i).cloned()) else { return };
        self.close_peek();
        match self.store.delete_forever(&t) {
            Ok(()) => self.notify(format!("Deleted \u{201c}{}\u{201d} forever", t.title), None),
            Err(e) => self.error = Some(format!("Couldn't delete it: {e}")),
        }
        self.refresh_trash();
    }

    /// The note becomes the version the page shows. The restore is a
    /// version too: the notice's Undo restores the one before it.
    fn restore_version(&mut self) {
        let Some(v) = &self.versions else { return };
        let (version, before) = (v.list[v.shown].version.clone(), v.list[0].version.clone());
        let when = revision_ago(&v.list[v.shown]);
        let id = v.note;
        self.close_history();
        match self.store.restore_version(self.current, &version) {
            Ok(()) => {
                self.histories.remove(&id);
                self.caret = Caret::at(page::first(self.doc()));
                self.reorder();
                self.notify(
                    format!("Restored the version from {when}"),
                    Some(Undo::Version { note: id, version: before }),
                );
            }
            Err(e) => self.error = Some(format!("Couldn't restore: {e}")),
        }
    }

    /// The version the page shows becomes a new note beside this one,
    /// titled for where it came from; the note itself is left as it is.
    fn restore_version_copy(&mut self) {
        let Some(shown) = self.versions.as_ref().map(|v| v.shown) else { return };
        let Some(text) = self.version_text(shown) else { return };
        let (title, doc) = carnet_core::doc::from_markdown(&text);
        let note = &self.store.notes[self.current];
        let title = format!("{} (restored)", title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(note.display_title()));
        let folder = note.folder.clone();
        self.close_history();
        let mut copy = Note::new(&title, doc);
        copy.folder = folder;
        let id = copy.id;
        self.store.notes.push(copy);
        let i = self.store.notes.len() - 1;
        if let Err(e) = self.store.save(i, true) {
            self.error = Some(format!("Couldn't save the copy: {e}"));
        }
        self.reorder();
        let i = self.store.notes.iter().position(|n| n.id == id).unwrap_or(i);
        self.open(i);
        self.notify(format!("Restored as \u{201c}{title}\u{201d}"), None);
    }

    /// Sync's turn: what came in taken, notes changed on another device
    /// written, notes saved here sent. The open note, when another device
    /// changed it, shows it at once.
    fn sync_tick(&mut self) {
        // The list of devices is asked for while Settings shows it, and the
        // server's rooms while the inspector does.
        // The devices name who made each version, too.
        self.sync.want_devices = self.settings_open || self.versions.is_some();
        // Invites are used elsewhere: asked afresh each time Settings opens.
        if self.settings_open && !self.sync.want_invites {
            self.sync.invites = None;
        }
        self.sync.want_invites = self.settings_open;
        self.sync.want_inspection = self.settings_open && self.sync_inspector;
        let open = self.store.notes[self.current].id;
        let remote = self.sync.tick(&mut self.store);
        if std::mem::take(&mut self.sync.password_set) {
            let said = match self.pw_edit.take() {
                Some((email, _)) => format!("Password set: a new device can come in with {}", carnet_sync::password::id(&email)),
                None => "Password taken away".to_string(),
            };
            self.notify(said, None);
        }
        if self.sync.paired.take().is_some() {
            self.notify("A new device is in", None);
        }
        if let Some(origin) = self.sync.last_origin.take()
            && self.sync_server.is_empty()
        {
            self.sync_server = origin;
            self.sync_reset_fields = true;
        }
        // In: the code, or the twelve words, need not stay on screen.
        if self.sync.is_on() && !self.sync_code.is_empty() {
            self.sync_code.clear();
            self.sync_reset_fields = true;
        }
        // Joining an account can take a note away, the open one or one
        // before it: this device's untouched welcome note, the account's
        // own on its way.
        let moved = self.store.notes.get(self.current).is_none_or(|n| n.id != open);
        if remote.is_empty() && !moved {
            return;
        }
        for r in &remote {
            match *r {
                // (An index may be stale: a note trashed by the same tick's
                // reconcile moves those after it.)
                store::Remote::Updated(i) | store::Remote::Merged(i) if self.store.notes.get(i).is_some_and(|n| n.id == open) => {
                    self.histories.remove(&open);
                    self.caret = page::fix(self.doc(), self.caret);
                    if matches!(r, store::Remote::Merged(_)) {
                        self.notify("Changed on another device too — the two are merged", None);
                    }
                }
                _ => {}
            }
        }
        if remote.iter().any(|r| matches!(r, store::Remote::Pictures)) {
            self.store.index_files();
            self.images.retry_failed();
        }
        // Trashed or put back elsewhere: the Deleted list follows.
        if remote.iter().any(|r| matches!(r, store::Remote::Layout)) {
            self.refresh_trash();
        }
        // The account's notes are here: the empty stand-in goes, if
        // nobody wrote in it.
        if !remote.is_empty()
            && let Some(id) = self.sync_stand_in.take()
            && let Some(i) = self.store.notes.iter().position(|n| n.id == id)
        {
            let n = &self.store.notes[i];
            if self.store.notes.len() > 1 && n.is_blank() && n.path.is_none() {
                self.store.notes.remove(i);
            } else {
                self.sync_stand_in = Some(id);
            }
        }
        if self.store.notes.is_empty() {
            let blank = blank_note();
            self.sync_stand_in = Some(blank.id);
            self.store.notes.push(blank);
        }
        self.reorder();
        match self.store.notes.iter().position(|n| n.id == open) {
            Some(i) => self.current = i,
            None => {
                self.current = self.order.first().copied().unwrap_or(0);
                self.caret = page::fix(self.doc(), self.caret);
            }
        }
    }

    /// Reads the trash again: after something went in or came out.
    fn refresh_trash(&mut self) {
        self.trash = self.store.trashed();
        // Gone from the trash meanwhile (put back or deleted on another
        // device): nothing left to read.
        if self.peek.is_some() && self.peek_index().is_none() {
            self.close_peek();
        }
        if self.trash.is_empty() {
            self.deleted = false;
            self.close_peek();
        }
    }

    /// Puts `trash[i]` back where it was: a note opens, a folder shows.
    fn restore_trashed(&mut self, i: usize) {
        let Some(t) = self.trash.get(i).cloned() else { return };
        match self.store.restore_trashed(&t) {
            Ok(Some(n)) => {
                let id = self.store.notes[n].id;
                self.reorder();
                let n = self.store.notes.iter().position(|x| x.id == id).unwrap_or(n);
                self.open(n);
                self.notify(format!("Put \u{201c}{}\u{201d} back", t.title), None);
            }
            Ok(None) => {
                self.reorder();
                self.notify(format!("Put the folder \u{201c}{}\u{201d} back", t.title), None);
            }
            Err(e) => self.error = Some(format!("Couldn't put it back: {e}")),
        }
        self.refresh_trash();
    }

    /// A Deleted row's menu.
    fn deleted_menu(&self, i: usize) -> Vec<MenuItem> {
        let item = |label: &str, what: &str| {
            MenuItem::new(label).id(Value::map([("trash", Value::Int(i as i64)), ("do", Value::str(what))]))
        };
        vec![item("Restore", "restore"), item("Delete Forever\u{2026}", "forever"), item(reveal_label(), "reveal")]
    }

    /// What a library row's context menu asked for.
    fn on_menu(&mut self, ev: &UiEvent) {
        let Some(item) = ev.payload.get("item") else { return };
        // A tool from the More menu.
        if let Some(i) = item.get_int("tool") {
            if let Some(msg) = self.overflow.get(i as usize).map(|d| d.msg.clone()) {
                self.on_plain(msg);
            }
            return;
        }
        if let Some(i) = item.get_int("trash") {
            match item.get_str("do") {
                Some("restore") => self.restore_trashed(i as usize),
                // Opened, and asked to be sure.
                Some("forever") => {
                    self.open_deleted(i as usize);
                    if let Some(p) = self.peek.as_mut() {
                        p.armed = true;
                    }
                }
                Some("reveal") => {
                    if let Some(t) = self.trash.get(i as usize)
                        && let Err(e) = store::reveal(&t.path)
                    {
                        self.error = Some(format!("Couldn't open the folder: {e}"));
                    }
                }
                _ => {}
            }
            return;
        }
        match item.get_str("picked") {
            Some("trash") => return self.trash_picked(),
            Some("move") => return self.move_picked(item.get_str("folder").unwrap_or_default()),
            _ => {}
        }
        if let Some(action) = item.get_str("page") {
            let action = action.to_string();
            self.page_action(&action);
            return;
        }
        if item.get_int("note").is_none()
            && let Some(folder) = item.get_str("folder")
        {
            let (folder, what) = (folder.to_string(), item.get_str("do").unwrap_or_default().to_string());
            self.folder_action(&folder, &what);
            return;
        }
        let Some(id) = item.get_int("note").map(|v| v as u64) else { return };
        let Some(i) = self.store.notes.iter().position(|n| n.id == id) else { return };
        match item.get_str("do") {
            Some("open") => self.open(i),
            Some("history") => {
                self.open(i);
                self.show_history();
            }
            Some("move") => {
                let to = item.get_str("folder").unwrap_or_default().to_string();
                self.move_note(i, &to);
            }
            Some("pin") => self.pin(i),
            Some("reveal") => {
                // A note never saved has no file yet: write it first.
                if self.store.notes[i].path.is_none() {
                    self.store.notes[i].dirty = true;
                    if let Err(e) = self.store.save(i, false) {
                        self.error = Some(format!("Couldn't save: {e}"));
                    }
                }
                let target = self.store.notes[i].path.clone();
                let result = match &target {
                    Some(p) => store::reveal(p),
                    None => store::reveal(&self.store.dir),
                };
                if let Err(e) = result {
                    self.error = Some(format!("Couldn't open the folder: {e}"));
                }
            }
            Some("copy-path") => {
                if let Some(p) = &self.store.notes[i].path {
                    self.clip_out = Some((p.display().to_string(), None));
                }
            }
            Some("copy-md") => {
                let md = self.store.notes[i].markdown();
                let folder = self.store.notes[i].folder.clone();
                self.copy_markdown(md, &folder);
                self.say = Some("Copied".into());
            }
            Some("delete") => self.delete_note_at(i),
            _ => {}
        }
    }

    /// A note's card pressed and maybe dragged: a press that does not
    /// travel opens the note; one that does carries it, and a folder row
    /// it is let go over takes it ("All notes": the top level).
    fn card_drag(&mut self, id: u64, ev: &UiEvent) {
        let p = &ev.payload;
        let num = |k: &str| p.get_float(k).or_else(|| p.get_int(k).map(|v| v as f64)).unwrap_or(0.0) as f32;
        let at = Vec2::new(num("x"), num("y"));
        match p.get_str("phase") {
            Some("start") => {
                self.note_drag = Some(NoteDrag { id, from: at, at, moved: false, target: None });
            }
            Some("move") => {
                let target = self
                    .folder_rects
                    .iter()
                    .find(|(_, r)| at.x >= r.x && at.x <= r.x + r.w && at.y >= r.y && at.y <= r.y + r.h)
                    .map(|(f, _)| f.clone());
                if let Some(d) = self.note_drag.as_mut() {
                    d.at = at;
                    d.moved = d.moved || (at.x - d.from.x).abs() + (at.y - d.from.y).abs() > 6.0;
                    d.target = if d.moved { target } else { None };
                }
            }
            _ => {
                let Some(d) = self.note_drag.take() else { return };
                let Some(i) = self.store.notes.iter().position(|n| n.id == d.id) else { return };
                match (d.moved, d.target) {
                    // A click: the card's own (`Msg::OpenNote`).
                    (false, _) => {}
                    // One of several chosen: all of them go.
                    (true, Some(folder)) if self.is_picked(&Pick::Note(d.id)) => self.move_picked(&folder),
                    (true, Some(folder)) if self.store.notes[i].folder != folder => self.move_note(i, &folder),
                    _ => {}
                }
            }
        }
    }

    /// The library's edge dragged: the width follows the pointer, within
    /// `LIBRARY_WIDTHS`, and is saved when it is let go. A double-click
    /// puts it back to the design's.
    fn library_drag(&mut self, ev: &UiEvent) {
        let p = &ev.payload;
        let x = p.get_float("x").or_else(|| p.get_int("x").map(|v| v as f64)).unwrap_or(0.0) as f32;
        let (lo, hi) = (*crate::settings::LIBRARY_WIDTHS.start(), *crate::settings::LIBRARY_WIDTHS.end());
        match p.get_str("phase") {
            Some("start") if p.get_int("clicks").unwrap_or(1) >= 2 => {
                self.library_from = None;
                self.settings.library_w = size::SIDEBAR as u32;
                self.save_library_w();
            }
            Some("start") => self.library_from = Some((x, self.settings.library_w)),
            Some("move") => {
                if let Some((x0, w0)) = self.library_from {
                    self.settings.library_w = (w0 as f32 + x - x0).round().clamp(lo as f32, hi as f32) as u32;
                }
            }
            _ => {
                if self.library_from.take().is_some() {
                    self.save_library_w();
                }
            }
        }
    }

    fn save_library_w(&mut self) {
        if let Err(e) = self.settings.save() {
            self.error = Some(format!("Couldn't save the settings: {e}"));
        }
    }

    /// The note being carried, under the pointer: its title on a card.
    fn note_ghost(&self, ui: &mut Ui<'_>, l: &Look) {
        let Some(d) = self.note_drag.as_ref().filter(|d| d.moved) else { return };
        let Some(note) = self.store.notes.iter().find(|n| n.id == d.id) else { return };
        let t = l.t;
        ui.with(
            NodeSpec::row()
                .float(FloatConfig::viewport().offset(d.at.x + 14.0, d.at.y + 10.0))
                .pad_xy(12.0, 7.0)
                .gap(8.0)
                .radius(radius::CONTROL)
                .bg(l.card)
                .border(1.0, t.border)
                .shadow_color(l.shadow)
                .shadow_blur(14.0)
                .shadow_y(3.0)
                .cross_align(Align::Center)
                .role(Role::None),
            |ui| {
                crate::icons::icon(ui, crate::icons::Glyph::Folder, if d.target.is_some() { t.accent } else { t.muted }, 14.0, false);
                ui.text(note.display_title(), TextStyle::new(Type::UI.size).color(t.fg).nowrap());
            },
        );
    }

    /// Typed text, at the caret: inside the styles switched on for it,
    /// when there are any, whichever way the letters came (a key's text,
    /// or the input method's), and never a space where Markdown's marks
    /// can't have one (`page::Typing`, the web editor's too).
    pub(super) fn type_text(&mut self, s: &str) {
        let mut writing = std::mem::take(&mut self.writing);
        self.edit(Step::Typing, |d, c| writing.type_text(d, c, s));
        self.writing = writing;
    }

    /// A colour for what is typed next, with nothing selected
    /// (`Typing::pend_tint`).
    fn pend_tint(&mut self, ink: bool, color: u32) {
        let mut writing = std::mem::take(&mut self.writing);
        writing.pend_tint(self.doc(), self.caret, ink, color);
        self.writing = writing;
    }

    /// Opens the note above (`up`) or below the open one in the library.
    fn step_note(&mut self, up: bool) {
        let Some(pos) = self.order.iter().position(|&i| i == self.current) else {
            if let Some(&first) = self.order.first() {
                self.open(first);
            }
            return;
        };
        let next = if up { pos.checked_sub(1) } else { Some(pos + 1) };
        if let Some(&i) = next.and_then(|p| self.order.get(p)) {
            self.open(i);
            self.focus_page = true;
            self.editing = true;
            self.say = Some(self.store.notes[self.current].display_title().to_string());
        }
    }

    /// Moves note `i` into folder `to` ("" for the top level). A note the
    /// library no longer shows stays open where it is.
    fn move_note(&mut self, i: usize, to: &str) {
        let name = self.store.notes[i].display_title().to_string();
        if self.store.notes[i].dirty
            && let Err(e) = self.store.save(i, false)
        {
            self.error = Some(format!("Couldn't save \u{201c}{name}\u{201d}, so it stays where it is: {e}"));
            return;
        }
        match self.store.move_note(i, to) {
            Ok(()) => {
                // Its picture links were rewritten for the new folder: an
                // undo from before the move would put back ones that no
                // longer lead anywhere.
                let id = self.store.notes[i].id;
                self.histories.remove(&id);
                let place = if to.is_empty() {
                    "out of its folder".to_string()
                } else {
                    format!("to \u{201c}{}\u{201d}", folder_name(to))
                };
                self.notify(format!("Moved \u{201c}{name}\u{201d} {place}"), None);
            }
            Err(e) => self.error = Some(format!("Couldn't move the note: {e}")),
        }
        self.reorder();
    }

    /// Shows a folder's notes, or every note.
    fn show_folder(&mut self, folder: Option<String>) {
        if self.folder != folder {
            self.folder = folder;
            self.reorder();
        }
    }

    /// Starts naming a new folder inside `parent`, in a new, empty field.
    /// One being named already is made first, if it has a name: a second
    /// New folder is as good as a click away from the first.
    fn name_folder(&mut self, parent: &str) {
        self.finish_naming(true, false);
        self.namings += 1;
        self.naming = Some(Naming { parent: parent.to_string(), name: String::new(), nth: self.namings, key: None, rename: None });
        self.focus(Field::Folder);
    }

    /// Starts renaming `folder`: its row becomes a field holding its name.
    fn start_rename(&mut self, folder: &str) {
        if folder.is_empty() {
            return;
        }
        self.finish_naming(true, false);
        self.namings += 1;
        let (parent, leaf) = folder.rsplit_once('/').unwrap_or(("", folder));
        self.naming = Some(Naming {
            parent: parent.to_string(),
            name: leaf.to_string(),
            nth: self.namings,
            key: None,
            rename: Some(folder.to_string()),
        });
        self.focus(Field::Folder);
    }

    /// Renames `folder` to `name`: its notes' edits saved first, the
    /// library kept on it if it was showing it or one inside it. Returns
    /// the new path.
    fn rename_folder(&mut self, folder: &str, name: &str) -> Option<String> {
        let under = format!("{folder}/");
        let inside = |f: &str| f == folder || f.starts_with(&under);
        for i in 0..self.store.notes.len() {
            let n = &self.store.notes[i];
            if inside(&n.folder)
                && n.dirty
                && n.path.is_some()
                && let Err(e) = self.store.save(i, false)
            {
                self.error = Some(format!("Couldn't save \u{201c}{}\u{201d}, so the folder keeps its name: {e}", self.store.notes[i].display_title()));
                return None;
            }
        }
        match self.store.rename_folder(folder, name) {
            Ok(to) => {
                if let Some(shown) = self.folder.as_deref().filter(|f| inside(f)) {
                    self.folder = Some(format!("{to}{}", &shown[folder.len()..]));
                }
                self.reorder();
                Some(to)
            }
            Err(e) => {
                self.error = Some(format!("Couldn't rename the folder: {e}"));
                None
            }
        }
    }

    /// A key pressed on a row of the folder list: F2 renames it, Delete
    /// or Backspace deletes it (to the trash with its notes, Undo away),
    /// ↑ and ↓ go to the row above and below and show it, as a Mac's
    /// sidebars do — so where the keyboard is shows after a click too,
    /// which hides the focus ring. True when the key was one of those.
    fn folder_row_key(&mut self, row: &FolderRow, k: &kui_native::KeyPress) -> bool {
        let m = k.mods;
        if m.shift || m.alt || (m.ctrl && cfg!(target_os = "macos")) {
            return false;
        }
        let at = self.folder_rows.iter().position(|(_, r)| r == row);
        match k.code {
            KeyCode::F(2) if !m.ctrl && !m.super_key => {
                if let FolderRow::Folder(f) = row {
                    self.start_rename(&f.clone());
                }
            }
            KeyCode::Backspace | KeyCode::Delete if matches!(row, FolderRow::Folder(f) if self.is_picked(&Pick::Folder(f.clone()))) => {
                self.trash_picked();
            }
            KeyCode::Backspace | KeyCode::Delete => {
                let FolderRow::Folder(f) = row else { return true };
                // The keyboard stays in the list: on the row that takes
                // this one's place, or the one above the last.
                self.focus_row = at.and_then(|i| {
                    self.folder_rows
                        .get(i + 1)
                        .filter(|(_, r)| !matches!(r, FolderRow::Folder(n) if n.starts_with(&format!("{f}/"))))
                        .or_else(|| i.checked_sub(1).and_then(|i| self.folder_rows.get(i)))
                        .map(|(_, r)| r.clone())
                });
                self.folder_action(&f.clone(), "delete");
            }
            KeyCode::Up | KeyCode::Down if !m.ctrl && !m.super_key => {
                let next = at.and_then(|i| if k.code == KeyCode::Up { i.checked_sub(1) } else { Some(i + 1) });
                if let Some((_, r)) = next.and_then(|i| self.folder_rows.get(i)).cloned() {
                    match &r {
                        FolderRow::All => self.on_plain(Msg::SelectFolder { index: ALL }),
                        FolderRow::Folder(f) => {
                            if let Some(i) = self.store.folders.iter().position(|x| x == f) {
                                self.on_plain(Msg::SelectFolder { index: i as u64 });
                            }
                        }
                        FolderRow::Deleted => self.on_plain(Msg::ShowDeleted),
                    }
                    self.focus_row = Some(r);
                }
            }
            _ => return false,
        }
        true
    }

    /// Ends naming a folder: made or renamed from its name when `make` and
    /// there is one (a new one shown, when `show`), dropped otherwise. The
    /// folder made or renamed, if one was.
    fn finish_naming(&mut self, make: bool, show: bool) -> Option<String> {
        let n = self.naming.take()?;
        if let Some(k) = n.key {
            self.fields.remove(&k);
        }
        let name = n.name.trim();
        // A rename let go of with its name as it was, or emptied: the
        // folder keeps its name.
        if let Some(folder) = &n.rename {
            let leaf = folder.rsplit('/').next().unwrap_or(folder);
            if !make || name.is_empty() || name == leaf {
                return None;
            }
            return self.rename_folder(folder, name);
        }
        if !make || name.is_empty() {
            return None;
        }
        match self.store.create_folder(&n.parent, name) {
            Ok(rel) => {
                if show {
                    self.show_folder(Some(rel.clone()));
                }
                Some(rel)
            }
            Err(e) => {
                self.error = Some(format!("Couldn't make the folder: {e}"));
                None
            }
        }
    }

    /// What a folder's context menu asked for.
    fn folder_action(&mut self, folder: &str, what: &str) {
        match what {
            "new" => self.name_folder(folder),
            "rename" => self.start_rename(folder),
            "new-note" => {
                self.show_folder((!folder.is_empty()).then(|| folder.to_string()));
                self.new_note();
            }
            "copy-path" => self.clip_out = Some((self.store.folder_dir(folder).display().to_string(), None)),
            "sort-edited" => self.on_plain(Msg::SetSort { to: 0 }),
            "sort-title" => self.on_plain(Msg::SetSort { to: 1 }),
            "sort-created" => self.on_plain(Msg::SetSort { to: 2 }),
            "reveal" => {
                if let Err(e) = store::reveal(&self.store.folder_dir(folder)) {
                    self.error = Some(format!("Couldn't open the folder: {e}"));
                }
            }
            // A folder with notes, or any file, goes to the trash whole,
            // Undo away; only one with nothing in it is just removed.
            "delete" if !self.folder_is_bare(folder) => self.trash_folder(folder),
            "delete" => match self.store.delete_folder(folder) {
                Ok(()) => {
                    let under = format!("{folder}/");
                    if self
                        .folder
                        .as_ref()
                        .is_some_and(|f| f == folder || f.starts_with(&under))
                    {
                        self.folder = None;
                    }
                    self.notify(format!("Deleted the folder \u{201c}{}\u{201d}", folder_name(folder)), None);
                    self.reorder();
                }
                Err(e) => self.error = Some(format!("Couldn't delete the folder: {}", store::plain_error(&e))),
            },
            _ => {}
        }
    }

    /// Whether a folder has nothing in it at all, on disk or here: no note
    /// (a new one nobody wrote in aside), no file of any kind.
    fn folder_is_bare(&self, folder: &str) -> bool {
        self.store.folder_is_empty(folder) && !self.store.folder_has_files(folder)
    }

    fn folder_menu(&self, folder: &str) -> Vec<MenuItem> {
        let item = |label: &str, what: &str| {
            MenuItem::new(label).id(Value::map([
                ("do", Value::str(what)),
                ("folder", Value::str(folder)),
            ]))
        };
        // "" is the notes folder itself: "All notes" in the list.
        if folder.is_empty() {
            let sort = self.settings.sort;
            return vec![
                item("New note", "new-note").accel(crate::keys::NEW_NOTE),
                item("New folder", "new").accel(crate::keys::NEW_FOLDER),
                MenuItem::separator(),
                item(reveal_label(), "reveal"),
                item("Copy folder path", "copy-path"),
                MenuItem::separator(),
                MenuItem::submenu(
                    "Sort by",
                    vec![
                        item("Last edited", "sort-edited").checked(sort == Sort::Edited),
                        item("Date created", "sort-created").checked(sort == Sort::Created),
                        item("Title", "sort-title").checked(sort == Sort::Title),
                    ],
                ),
            ];
        }
        vec![
            item("New note here", "new-note"),
            item("New folder inside", "new"),
            MenuItem::separator(),
            item(reveal_label(), "reveal"),
            item("Copy folder path", "copy-path"),
            MenuItem::separator(),
            item("Rename\u{2026}", "rename").accel(crate::keys::RENAME_FOLDER),
            item(if self.folder_is_bare(folder) { "Delete folder" } else { "Move folder to trash" }, "delete"),
        ]
    }

    /// Moves a folder with notes in it to the trash, whole: their edits
    /// saved first, the open note left for the next one shown.
    fn trash_folder(&mut self, folder: &str) {
        let under = format!("{folder}/");
        let inside = |f: &str| f == folder || f.starts_with(&under);
        for i in 0..self.store.notes.len() {
            let n = &self.store.notes[i];
            if inside(&n.folder)
                && n.dirty
                && !(n.path.is_none() && n.is_blank())
                && let Err(e) = self.store.save(i, false)
            {
                self.error = Some(format!("Couldn't save \u{201c}{}\u{201d}, so the folder stays: {e}", self.store.notes[i].display_title()));
                return;
            }
        }
        let open_id = self.store.notes[self.current].id;
        let count = self.store.notes.iter().filter(|n| inside(&n.folder) && n.path.is_some()).count();
        match self.store.trash_folder(folder) {
            Ok(from) => {
                let what = match count {
                    0 => String::new(),
                    1 => " and its note".into(),
                    n => format!(" and its {n} notes"),
                };
                self.notify(
                    format!("Moved \u{201c}{}\u{201d}{what} to the trash", folder_name(folder)),
                    Some(Undo::Folder { from, folder: folder.to_string() }),
                );
            }
            Err(e) => {
                self.error = Some(format!("Couldn't move the folder to the trash: {}", store::plain_error(&e)));
                return;
            }
        }
        if self.folder.as_deref().is_some_and(inside) {
            self.folder = None;
        }
        if self.store.notes.is_empty() {
            self.store.notes.push(blank_note());
        }
        self.reorder();
        match self.store.notes.iter().position(|n| n.id == open_id) {
            Some(j) => self.current = j,
            None => {
                self.current = self.order.first().copied().unwrap_or(0);
                self.caret = Caret::at(page::first(self.doc()));
                self.editing = false;
            }
        }
        self.refresh_trash();
    }

    fn pin(&mut self, i: usize) {
        let n = &mut self.store.notes[i];
        n.pinned = !n.pinned;
        let pinned = n.pinned;
        if n.path.is_none() {
            n.dirty = true;
            let _ = self.store.save(i, false);
        }
        if let Err(e) = self.store.save_pins() {
            self.error = Some(format!("Couldn't save pins: {e}"));
        }
        self.say = Some(if pinned { "Pinned" } else { "Unpinned" }.into());
        self.reorder();
    }

    fn note_menu(&self, id: u64) -> Vec<MenuItem> {
        let Some(note) = self.store.notes.iter().find(|n| n.id == id) else {
            return Vec::new();
        };
        let item = |label: &str, what: &str| {
            MenuItem::new(label).id(Value::map([
                ("do", Value::str(what)),
                ("note", Value::Int(id as i64)),
            ]))
        };
        let mut items = vec![
            item("Open", "open"),
            // Every version it has been, kept by the replica.
            item("History", "history").enabled(note.key.is_some()),
            item(if note.pinned { "Unpin" } else { "Pin to top" }, "pin").accel(crate::keys::PIN),
            MenuItem::separator(),
            item("Copy as Markdown", "copy-md"),
            item("Copy file path", "copy-path").enabled(note.path.is_some()),
            item(reveal_label(), "reveal"),
        ];
        // Where it can go: every folder but its own, or back out — in a
        // submenu of its own.
        let move_to = |label: String, to: &str| {
            MenuItem::new(label).id(Value::map([
                ("do", Value::str("move")),
                ("note", Value::Int(id as i64)),
                ("folder", Value::str(to)),
            ]))
        };
        let mut targets: Vec<MenuItem> = Vec::new();
        if !note.folder.is_empty() {
            targets.push(move_to("Out of its folder".into(), ""));
        }
        for f in self.store.folders.iter().filter(|f| **f != note.folder) {
            targets.push(move_to(f.clone(), f));
        }
        if !targets.is_empty() {
            items.push(MenuItem::separator());
            items.push(MenuItem::submenu("Move to", targets));
        }
        items.push(MenuItem::separator());
        items.push(item("Move to trash", "delete").accel(crate::keys::TRASH));
        items
    }

    // ------------------------------------------------------------ events

    fn on_msg(&mut self, msg: Msg, ev: &UiEvent, core: &mut Core) {
        // Typing goes on through the page's keys; anything else ends it.
        // The colour picker stays for its own swatches and tabs (and the
        // toolbar buttons that switch them); anything else puts it away.
        if msg != Msg::Page {
            self.typing = false;
            if !matches!(msg, Msg::Tint { .. } | Msg::TintMenu { .. }) {
                self.tint_open = None;
            }
        }
        match msg {
            Msg::RootKey => self.root_key(ev, core),
            Msg::Page => {
                if let Some((KeyPhase::Down, k)) = ev.key_press()
                    && self.debug_key(&k, core)
                {
                    return;
                }
                self.page_event(ev, core)
            }
            Msg::PageDrag => self.page_drag(ev, core),
            Msg::SettingsTitle => self.title_click(Instant::now(), core),
            Msg::ResizePicture { id, side } => self.resize_drag(id, side == 1, ev),
            Msg::PictureDrag { id } => self.picture_drag(id, ev),
            Msg::CardDrag { note } => self.card_drag(note, ev),
            // A click on a card or a folder with ⇧ or ⌘ held chooses it.
            Msg::OpenNote { note } => {
                let id = self.store.notes.get(note as usize).map(|n| n.id);
                if !id.is_some_and(|id| self.pick_click(Pick::Note(id), core.modifiers())) {
                    self.on_plain(msg);
                }
            }
            Msg::SelectFolder { index } if index != ALL && self.store.folders.get(index as usize).is_some() => {
                let f = self.store.folders[index as usize].clone();
                if !self.pick_click(Pick::Folder(f), core.modifiers()) {
                    self.on_plain(msg);
                }
            }
            Msg::LibraryDrag => self.library_drag(ev),
            other => self.on_plain(other),
        }
    }

    /// A message that needs nothing but itself: a click's, or one the
    /// page's menu and shortcuts send on.
    fn on_plain(&mut self, msg: Msg) {
        // Turn off, and Remove, ask again after anything else is done.
        if !matches!(msg, Msg::SyncOff) {
            self.sync_off_armed = false;
        }
        if !matches!(msg, Msg::SyncRemove { .. }) {
            self.sync_remove_armed = None;
        }
        if !matches!(msg, Msg::SyncAccountRemove { .. }) {
            self.sync_account_armed = None;
        }
        if !matches!(msg, Msg::SyncPasskeyRemove { .. }) {
            self.sync_passkey_armed = None;
        }
        if !matches!(msg, Msg::ResetLook) {
            self.reset_armed = false;
        }
        if !matches!(msg, Msg::SyncRollBack { .. } | Msg::SyncRepair { .. } | Msg::SyncInspectConfirm) {
            self.sync_inspect_armed = None;
        }
        // A toolbar click took the keyboard from the page: give it back.
        let toolbar = matches!(
            msg,
            Msg::Turn { .. }
                | Msg::ToggleInitial
                | Msg::Style { .. }
                | Msg::Indent { .. }
                | Msg::Move { .. }
                | Msg::DeleteBlock
                | Msg::InsertTable
                | Msg::InsertDivider
                | Msg::AddRow
                | Msg::AddCol
                | Msg::DelRow
                | Msg::DelCol
                | Msg::Sort
                | Msg::Tint { .. }
                | Msg::TintMenu { .. }
                | Msg::Float { .. }
                | Msg::LinkAct { .. }
                | Msg::CloseLink
        );
        if toolbar {
            self.focus_page = true;
            self.editing = true;
        }
        match msg {
            Msg::RootKey | Msg::Page | Msg::PageDrag | Msg::SettingsTitle => {}
            Msg::OpenNote { note } => self.open(note as usize),
            Msg::NewNote => self.new_note(),
            Msg::ToggleZen => {
                self.zen = !self.zen;
                self.focus_page = true;
                self.editing = true;
                self.say = Some(if self.zen { "Zen mode" } else { "Zen mode off" }.into());
            }
            // Not the note behind a deleted one on the page.
            Msg::PinNote | Msg::DeleteNote if self.peek.is_some() => {}
            Msg::PinNote => self.pin(self.current),
            Msg::DeleteNote => self.delete_note_at(self.current),
            Msg::Toggle { id } => {
                self.edit(Step::Other, |d, _| page::toggle(d, id));
                self.reveal = false;
            }
            Msg::Select { id } => {
                if let Some(i) = self.doc().index_of(id) {
                    let c = page::select_blocks(self.doc(), i, i);
                    self.move_to(c);
                    self.focus_page = true;
                    self.editing = true;
                }
            }
            Msg::Turn { to } => {
                self.edit(Step::Other, |d, c| page::toggle_turn(d, c, to));
            }
            Msg::ToggleInitial => {
                self.edit(Step::Other, page::toggle_initial);
            }
            Msg::Indent { by } => {
                self.edit(Step::Other, |d, c| page::indent(d, *c, by));
            }
            Msg::Move { by } => {
                self.edit(Step::Other, |d, c| page::move_blocks(d, *c, by));
            }
            Msg::DeleteBlock => {
                if self.edit(Step::Other, page::remove_block) {
                    self.notify("Removed", Some(Undo::Edit));
                }
            }
            Msg::InsertTable => {
                self.edit(Step::Other, |d, c| {
                    page::insert_block(d, c, Kind::Table(Table::new(3, 3)))
                });
            }
            Msg::InsertDivider => {
                self.edit(Step::Other, |d, c| page::insert_block(d, c, Kind::Divider));
            }
            Msg::AddRow => {
                self.edit(Step::Other, |d, c| page::table_op(d, c, TableOp::AddRow, false));
            }
            Msg::AddCol => {
                self.edit(Step::Other, |d, c| page::table_op(d, c, TableOp::AddCol, false));
            }
            Msg::DelRow => {
                self.edit(Step::Other, |d, c| page::table_op(d, c, TableOp::DelRow, false));
            }
            Msg::DelCol => {
                self.edit(Step::Other, |d, c| page::table_op(d, c, TableOp::DelCol, false));
            }
            Msg::Sort => {
                let Some((_, col)) = self.caret.head.slot.cell else { return };
                // The same column again flips the direction.
                let desc = self.last_sort.is_some_and(|(k, d)| k == col && !d);
                self.edit(Step::Other, |d, c| page::table_op(d, c, TableOp::Sort, desc));
                self.last_sort = Some((col, desc));
            }
            Msg::Undo => self.notice_undo(),
            Msg::ClearSearch => {
                self.query.clear();
                self.clear_search = true;
                self.reorder();
                self.focus(Field::Search);
            }
            Msg::FocusTitle => self.focus(Field::Title),
            Msg::FocusSearch => self.focus(Field::Search),
            Msg::Style { which } => {
                let kind = STYLES[(which as usize).min(STYLES.len() - 1)];
                if self.caret.is_empty() {
                    // Nothing selected: on (or off) for what is typed next.
                    let (pre, post) = self.writing.switch(self.doc(), self.caret, kind);
                    self.writing.pend(self.caret, &format!("style{which}"), pre, post);
                } else {
                    self.edit(Step::Other, |d, c| page::style(d, c, kind));
                }
            }
            Msg::ShowList => self.list_view = true,
            Msg::SelectFolder { index } => {
                self.close_history();
                self.folders_clicked = true;
                self.deleted = false;
                self.close_peek();
                let folder = self.store.folders.get(index as usize).cloned();
                self.show_folder(if index == ALL { None } else { folder });
            }
            Msg::ShowHistory => self.show_history(),
            Msg::ShowVersion { index } => self.show_version(index as usize),
            Msg::RestoreVersion => self.restore_version(),
            Msg::RestoreVersionCopy => self.restore_version_copy(),
            Msg::CloseHistory => self.close_history(),
            Msg::SyncHow { to } => {
                self.sync_how = to.min(2);
                self.sync.error = None;
            }
            Msg::SyncShowPassword => {
                self.sync_show_password = !self.sync_show_password;
                self.sync_mask_reset = true;
            }
            Msg::SyncStart => {
                let code = self.sync_code.clone();
                let how = match self.sync_how {
                    0 => crate::sync::How::Code(code),
                    1 => crate::sync::How::Recover(code),
                    _ => crate::sync::How::Password { email: self.sync_email.clone(), password: self.sync_password.clone() },
                };
                self.sync.start(&self.sync_server, how);
            }
            Msg::SyncPassword => {
                // Held to the account's email (a hosted service): that one,
                // and the password field to type in.
                let only = self.sync.password_email.clone();
                let email = only.clone().or_else(|| self.sync.password.as_ref().and_then(|(_, id)| id.clone())).unwrap_or_default();
                self.pw_edit = Some((email, String::new()));
                self.pw_remove_armed = false;
                self.sync.error = None;
                self.focus_req = Some(if only.is_some() { Field::PasswordNew } else { Field::PasswordEmail });
            }
            Msg::SyncPasswordSave => {
                if let Some((email, password)) = self.pw_edit.clone() {
                    let email = self.sync.password_email.clone().unwrap_or(email);
                    self.sync.set_password(&email, &password);
                }
            }
            Msg::SyncPasswordCancel => {
                self.pw_edit = None;
                self.pw_remove_armed = false;
            }
            Msg::SyncPasswordRemove if !self.pw_remove_armed => self.pw_remove_armed = true,
            Msg::SyncPasswordRemove => {
                self.pw_remove_armed = false;
                self.sync.drop_password();
            }
            // Asked first: Cancel, or Yes, remove.
            Msg::SyncPasskeyRemove { at } => {
                let Some(id) = self.sync.passkeys.get(at as usize).map(|p| p.id) else { return };
                if self.sync_passkey_armed == Some(id) {
                    self.sync_passkey_armed = None;
                    self.sync.remove_passkey(&id);
                } else {
                    self.sync_passkey_armed = Some(id);
                }
            }
            Msg::SyncPasskeyCancel => self.sync_passkey_armed = None,
            Msg::SyncCancel => self.sync.cancel(),
            Msg::SyncPair => self.sync.pair_open(),
            Msg::SyncPairCancel => self.sync.pair_cancel(),
            Msg::SyncRename => {
                let mine = self.sync.devices.as_ref().and_then(|l| l.iter().find(|d| d.you)).and_then(|d| d.name.clone());
                self.sync_rename = Some(mine.unwrap_or_default());
                // The keyboard to the field, its name to type over.
                self.focus_req = Some(Field::DeviceName);
            }
            Msg::SyncRenameSave => {
                if let Some(name) = self.sync_rename.take() {
                    self.sync.rename(&name);
                }
            }
            Msg::SyncRenameCancel => self.sync_rename = None,
            Msg::SyncInvite => self.sync.make_invite(),
            Msg::SyncInviteDone => self.sync.invite = None,
            Msg::SyncInviteDrop { at } => {
                if let Some(hash) = self.sync.invites.as_ref().and_then(|i| i.waiting.get(at as usize)).map(|w| w.hash) {
                    self.sync.drop_invite(hash);
                }
            }
            Msg::SyncInviteCopy { at } => {
                let code = self.sync.invites.as_ref().and_then(|i| i.waiting.get(at as usize)).and_then(|w| self.sync.invite_code(&w.hash));
                if let Some(code) = code {
                    self.clip_out = Some((code, None));
                    self.say = Some("Copied".into());
                }
            }
            // Asked twice: everything the account keeps on the server goes.
            Msg::SyncAccountRemove { at } => {
                let Some(id) = self.sync.invites.as_ref().and_then(|i| i.accounts.get(at as usize)).map(|a| a.id.clone()) else { return };
                if self.sync_account_armed.as_deref() == Some(id.as_slice()) {
                    self.sync_account_armed = None;
                    self.sync.remove_account(&id);
                } else {
                    self.sync_account_armed = Some(id);
                }
            }
            Msg::SyncRecovery => self.sync.make_recovery(),
            Msg::SyncRecoveryDone => self.sync.words = None,
            // Asked twice: off, this device needs a code or the words to
            // come back, and the last device off leaves only the words.
            Msg::SyncRemove { at } => {
                let Some(id) = self.sync.devices.as_ref().and_then(|l| l.get(at as usize)).map(|d| d.id.clone()) else { return };
                if self.sync_remove_armed.as_deref() == Some(id.as_slice()) {
                    self.sync_remove_armed = None;
                    self.sync.revoke(&id);
                } else {
                    self.sync_remove_armed = Some(id);
                }
            }
            Msg::SyncInspector => {
                self.sync_inspector = !self.sync_inspector;
                self.sync.refresh_inspection();
            }
            Msg::SyncInspectorRefresh => self.sync.refresh_inspection(),
            Msg::SyncInspectMore { at } => {
                let room = self.sync.inspection.as_ref().and_then(|(rooms, _)| rooms.get(at as usize)).map(|r| r.room.clone());
                self.sync_inspect_open = if self.sync_inspect_open == room { None } else { room };
            }
            Msg::SyncRollBack { at } | Msg::SyncRepair { at } => {
                let repair = matches!(msg, Msg::SyncRepair { .. });
                if let Some(room) = self.sync.inspection.as_ref().and_then(|(rooms, _)| rooms.get(at as usize)).map(|r| r.room.clone()) {
                    self.sync_inspect_armed = Some((room, repair));
                }
            }
            Msg::SyncInspectConfirm => {
                if let Some((room, repair)) = self.sync_inspect_armed.take() {
                    self.sync_inspect_open = None;
                    if repair {
                        self.sync.repair(&room);
                    } else {
                        self.sync.roll_back(&room);
                    }
                }
            }
            Msg::SyncInspectCancel => self.sync_inspect_armed = None,
            Msg::SyncDeleteAccount if !self.delete_account_armed => self.delete_account_armed = true,
            Msg::SyncDeleteAccount => {
                self.delete_account_armed = false;
                self.sync.delete_account();
            }
            Msg::SyncDeleteAccountCancel => self.delete_account_armed = false,
            Msg::SyncOff if !self.sync_off_armed => self.sync_off_armed = true,
            Msg::SyncOff => {
                self.sync_off_armed = false;
                self.sync.turn_off();
            }
            Msg::CompareWithNow => {
                if let Some(v) = self.versions.as_mut() {
                    v.against_now = !v.against_now;
                    let shown = v.shown;
                    self.show_version(shown);
                }
            }
            Msg::ShowDeleted => {
                self.close_history();
                self.folders_clicked = true;
                self.refresh_trash();
                self.deleted = !self.trash.is_empty();
                self.list_view = true;
            }
            Msg::RestoreTrashed { index } => self.restore_trashed(index as usize),
            Msg::OpenDeleted { index } => self.open_deleted(index as usize),
            Msg::RestorePeek => {
                if let Some(i) = self.peek_index() {
                    self.close_peek();
                    self.restore_trashed(i);
                }
            }
            Msg::DeleteForever => match self.peek.as_mut() {
                Some(p) if !p.armed => p.armed = true,
                Some(_) => self.delete_forever(),
                None => {}
            },
            Msg::DeleteForeverCancel => {
                if let Some(p) = self.peek.as_mut() {
                    p.armed = false;
                }
            }
            Msg::NewFolder => self.name_folder(""),
            Msg::Tint { ink, color } => {
                // The picker stays open: a colour and a highlighter are
                // often given together. Esc or a press outside puts it away.
                let (ink, color) = (ink == 1, color as u32);
                if self.caret.is_empty() {
                    self.pend_tint(ink, color);
                } else {
                    self.edit(Step::Other, |d, c| page::tint(d, c, ink, color));
                }
                // The picker's "recent" row: the newest first, ten at most.
                if color > carnet_core::inline::HIGHLIGHT {
                    let recent = &mut self.settings.colors;
                    recent.retain(|&c| c != color);
                    recent.insert(0, color);
                    recent.truncate(10);
                    if let Err(e) = self.settings.save() {
                        self.error = Some(format!("Couldn't save the settings: {e}"));
                    }
                }
            }
            Msg::TintMenu { ink } => {
                let ink = ink == 1;
                self.tint_open = match self.tint_open {
                    Some((was, _)) if was == ink => None,
                    // From one tab to the other, the picker stays put.
                    Some((_, at)) => Some((ink, at)),
                    None => {
                        self.tint_from = self.tint_anchor[ink as usize].is_some().then_some(ink as usize);
                        self.tint_anchor[ink as usize].or(self.more.map(|m| m.1)).map(|at| (ink, at))
                    }
                };
            }
            Msg::LinkAct { what } => self.link_act(what),
            Msg::CloseLink => {
                self.link_edit = None;
                self.focus_page = true;
            }
            Msg::Float { to } => {
                if let Some((id, _, _)) = self.picture_here() {
                    let to = match to {
                        1 => carnet_core::doc::Float::Left,
                        2 => carnet_core::doc::Float::Right,
                        _ => carnet_core::doc::Float::None,
                    };
                    self.edit(Step::Other, |d, _| page::set_image_float(d, id, to));
                }
            }
            Msg::Slash { index } => self.run_slash(index as usize),
            Msg::ResizePicture { .. } | Msg::CardDrag { .. } | Msg::PictureDrag { .. } | Msg::LibraryDrag => {}
            Msg::ToolsMenu => {
                if let Some((k, at)) = self.more {
                    self.pending_menu = Some((k, at, MenuFor::Tools));
                }
            }
            Msg::CopyText { what } => match what {
                0 => self.clip_out = Some((self.store.dir.display().to_string(), None)),
                _ => {
                    let md = self.store.notes[self.current].markdown();
                    let folder = self.store.notes[self.current].folder.clone();
                    self.copy_markdown(md, &folder);
                    self.say = Some("Copied".into());
                }
            },
            Msg::ShowSettings
            | Msg::SetTheme { .. }
            | Msg::SetPalette { .. }
            | Msg::SetDensity { .. }
            | Msg::SetPairing { .. }
            | Msg::SetWidth { .. }
            | Msg::SetSort { .. }
            | Msg::SetDates { .. }
            | Msg::ResetLook
            | Msg::ResetLookCancel
            | Msg::SetWindow { .. }
            | Msg::SetToolbar { .. }
            | Msg::SetFocus { .. }
            | Msg::SetTypewriter { .. }
            | Msg::ToggleSidebar
            | Msg::SetSize { .. }
            | Msg::SetDivider { .. }
            | Msg::SetInitialLines { .. }
            | Msg::SetInitialColor { .. }
            | Msg::FontMenu { .. }
            | Msg::SetFont { .. }
            | Msg::PickDir
            | Msg::ResetDir
            | Msg::ShowDir => self.on_settings(msg),
        }
    }

    /// While files are dragged over the window: where the pointer is (the
    /// OS is asked where kui cannot say), the drop line under it, the
    /// page scrolling when it nears an edge, and a note opening under it
    /// when it rests on that note's card.
    fn follow_file_drag(&mut self, ui: &mut Ui<'_>) {
        // A picture carried on the page has the drop line.
        if self.pic_drag.is_some() {
            return;
        }
        let Some(drag) = self.file_drag.as_mut() else {
            self.drop_index = None;
            return;
        };
        if let Some(p) = crate::oscursor::over_window(ui.core().scale()) {
            drag.at = Some(p);
        }
        ui.request_frame();
        let Some(at) = drag.at else { return };
        let inside = |r: &Rect| at.x >= r.x && at.x <= r.x + r.w && at.y >= r.y && at.y <= r.y + r.h;
        // Resting on a card.
        let card = self.card_rects.iter().find(|(_, r)| inside(r)).map(|(n, _)| *n);
        let open_now = match (card, drag.card) {
            (Some(n), Some((m, since))) if n == m => since.elapsed() >= SPRING && n != self.current,
            (Some(n), _) => {
                drag.card = Some((n, Instant::now()));
                false
            }
            (None, _) => {
                drag.card = None;
                false
            }
        };
        if open_now && let Some(n) = card {
            self.open(n);
        }
        // Over the page: the gap between blocks nearest the pointer.
        self.drop_index = match self.page_rect {
            Some(r) if inside(&r) && !self.settings_open => {
                let i = self.gap_at(at.y);
                // Near the scroller's edges, the page scrolls.
                if let Some(k) = self.scroll_key {
                    let edge = 48.0;
                    let dy = if at.y < r.y + edge {
                        -14.0
                    } else if at.y > r.y + r.h - edge {
                        14.0
                    } else {
                        0.0
                    };
                    if dy != 0.0 {
                        let o = ui.scroll_offset(k);
                        ui.set_scroll(k, Vec2::new(o.x, (o.y + dy).max(0.0)));
                    }
                }
                Some(i)
            }
            _ => None,
        };
    }

    /// Markdown on the clipboard, with its HTML beside it for apps that
    /// take formatting. Images are linked from where they are on disk.
    fn copy_markdown(&mut self, md: String, folder: &str) {
        let store = &self.store;
        let html = export::html(&md, |src, wiki| match store.picture_file(folder, src, wiki) {
            Some(p) => format!("file:///{}", p.display().to_string().replace('\\', "/").trim_start_matches('/')),
            None => src.to_string(),
        });
        self.clip_out = Some((md, Some(html)));
    }

    /// Keys at the root, with nothing focused: the app's shortcuts, and Tab
    /// passed on to the ring (the root being a sink, it would keep it).
    /// F12: kui's devtools, in a debug build. Shift+F12: the frame-time
    /// HUD. True when the key was one of them.
    fn debug_key(&mut self, k: &kui_native::KeyPress, core: &mut Core) -> bool {
        if k.code != KeyCode::F(12) || k.mods.ctrl || k.mods.alt || k.mods.super_key {
            return false;
        }
        if k.mods.shift {
            self.set_debug(!self.hud, core);
        } else if cfg!(debug_assertions) || self.hud {
            let on = !core.devtools();
            core.set_devtools(on);
        } else {
            return false;
        }
        true
    }

    /// A click on the Settings page's title at `at`: the fifth of a run,
    /// each within [`TITLE_CLICKS`] of the one before, turns the debug
    /// information on or off and says so.
    pub(super) fn title_click(&mut self, at: Instant, core: &mut Core) {
        let (n, last) = self.title_clicks;
        let n = if n > 0 && at.saturating_duration_since(last) <= TITLE_CLICKS { n + 1 } else { 1 };
        self.title_clicks = (if n >= 5 { 0 } else { n }, at);
        if n >= 5 {
            self.set_debug(!self.hud, core);
            self.notify(if self.hud { "Debug information on" } else { "Debug information off" }, None);
        }
    }

    /// The debug information on or off: the frame-time graph; turned off
    /// in a release build, F12's devtools close with it.
    fn set_debug(&mut self, on: bool, core: &mut Core) {
        self.hud = on;
        if !on && !cfg!(debug_assertions) && core.devtools() {
            core.set_devtools(false);
        }
    }

    fn root_key(&mut self, ev: &UiEvent, core: &mut Core) {
        let Some((KeyPhase::Down, k)) = ev.key_press() else {
            return;
        };
        self.root_press(k, core);
    }

    /// A key no sink took: the window's Tab ring, and the app's chords.
    /// Also what a menu bar row plays back while the page lacks the
    /// keyboard.
    pub(super) fn root_press(&mut self, k: kui_native::KeyPress, core: &mut Core) {
        if self.debug_key(&k, core) {
            return;
        }
        let m = k.mods;
        if let Some(row) = core
            .focus()
            .and_then(|f| self.folder_rows.iter().find(|(key, _)| *key == f))
            .map(|(_, r)| r.clone())
            && self.folder_row_key(&row, &k)
        {
            return;
        }
        // ↑ and ↓ after a click on a folder, the keyboard on nothing in
        // particular: from the row shown, as if the keyboard were on it.
        if matches!(k.code, KeyCode::Up | KeyCode::Down)
            && self.folders_clicked
            && core.focus().is_none_or(|f| f == Key::ROOT)
        {
            let row = if self.deleted {
                FolderRow::Deleted
            } else {
                self.folder.clone().map_or(FolderRow::All, FolderRow::Folder)
            };
            if self.folder_rows.iter().any(|(_, r)| *r == row) && self.folder_row_key(&row, &k) {
                return;
            }
        }
        // F2 with the keyboard on nothing in particular — after a click on
        // a folder, which shows it and leaves the keyboard on the window:
        // the folder shown is renamed, as Finder's selected one is.
        if k.code == KeyCode::F(2)
            && !(m.ctrl || m.super_key || m.alt || m.shift)
            && core.focus().is_none_or(|f| f == Key::ROOT)
            && let Some(f) = self.folder.clone()
        {
            self.start_rename(&f);
            return;
        }
        if k.code == KeyCode::Tab && !(m.ctrl || m.super_key || m.alt) {
            self.step = Some(!m.shift);
            return;
        }
        if k.code == KeyCode::Escape && !self.picked.is_empty() {
            self.picked.clear();
            return;
        }
        if k.code == KeyCode::Escape && self.settings_open {
            self.settings_open = false;
            return;
        }
        if k.code == KeyCode::Escape && self.reading() {
            self.close_history();
            return;
        }
        if (m.ctrl || m.super_key) && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            // Text selected outside the page — the meta line, a notice,
            // the settings — copies as it reads.
            if let Some(text) = core.selection_text().filter(|t| !t.is_empty()) {
                self.clip_out = Some((text, core.selection_html()));
            }
            return;
        }
        if m.ctrl || m.super_key {
            self.chord(&k);
        }
    }

    /// The shortcuts that work anywhere a sink hears them. True when the
    /// key was one.
    fn chord(&mut self, k: &kui_native::KeyPress) -> bool {
        let (shift, alt) = (k.mods.shift, k.mods.alt);
        match crate::keys::chord_code(k) {
            KeyCode::Up | KeyCode::Down if alt => self.step_note(crate::keys::chord_code(k) == KeyCode::Up),
            KeyCode::Char('n') if shift => self.on_plain(Msg::NewFolder),
            KeyCode::Char('p') if shift => self.pin(self.current),
            // What is chosen in the library, or the open note.
            KeyCode::Backspace | KeyCode::Delete if shift && self.picked.len() > 1 => self.trash_picked(),
            KeyCode::Backspace | KeyCode::Delete if shift => self.delete_note_at(self.current),
            KeyCode::Char('z') | KeyCode::Char('Z') if shift => self.redo_edit(),
            // A note just moved to the trash: Ctrl+Z brings it back, as its
            // notice's Undo says — not an edit in the note that took its
            // place.
            KeyCode::Char('z') | KeyCode::Char('Z')
                if self.notice.as_ref().is_some_and(|n| matches!(n.undo, Some(Undo::Note { .. } | Undo::Many(_)))) =>
            {
                self.notice_undo()
            }
            KeyCode::Char('z') | KeyCode::Char('Z') => self.undo_edit(),
            KeyCode::Char('y') | KeyCode::Char('Y') => self.redo_edit(),
            KeyCode::Char('n') | KeyCode::Char('N') => self.new_note(),
            KeyCode::Char(',') => self.on_settings(Msg::ShowSettings),
            KeyCode::Char('\\') => self.on_settings(Msg::ToggleSidebar),
            KeyCode::Char('.') => self.on_plain(Msg::ToggleZen),
            KeyCode::Char('f') | KeyCode::Char('F') => {
                // On a narrow window the search is on the list.
                self.list_view = true;
                self.settings_open = false;
                self.focus(Field::Search);
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.save_current();
                if self.error.is_none() {
                    self.say = Some("Saved".into());
                }
            }
            _ => return false,
        }
        true
    }

    fn on_field_changed(&mut self, field: Field, text: String) {
        match field {
            Field::Link => {
                if let Some(le) = self.link_edit.as_mut() {
                    le.url = text;
                }
            }
            Field::Search => {
                if self.query != text {
                    // A search is of the notes, not of the trash or a
                    // note's history.
                    self.deleted = false;
                    self.close_history();
                    self.query = text;
                    self.reorder();
                }
            }
            Field::Title if self.reading() => {}
            Field::Title => {
                let note = self.note();
                note.title = text;
                // Retitled here: the file may follow, once typing stops.
                note.renamable = true;
                self.edited();
            }
            Field::Folder => {
                if let Some(n) = &mut self.naming {
                    n.name = text;
                }
            }
            Field::Font => self.font_query = text,
            Field::SyncServer => {
                if text != self.sync_server {
                    self.sync.error = None;
                }
                self.sync_server = text;
            }
            // An edit clears what was said of the last try; the same text
            // reported again does not.
            Field::SyncCode => {
                if text != self.sync_code {
                    self.sync.error = None;
                }
                self.sync_code = text;
            }
            Field::DeviceName => {
                if self.sync_rename.is_some() {
                    self.sync_rename = Some(text);
                }
            }
            Field::SyncEmail => {
                if text != self.sync_email {
                    self.sync.error = None;
                }
                self.sync_email = text;
            }
            Field::SyncPassword => {
                let real = if self.sync_show_password { text } else { unmask(&self.sync_password, &text) };
                if real != self.sync_password {
                    self.sync.error = None;
                    self.sync_mask_reset |= !self.sync_show_password;
                }
                self.sync_password = real;
            }
            Field::PasswordEmail => {
                if let Some((email, _)) = self.pw_edit.as_mut() {
                    *email = text;
                }
            }
            Field::PasswordNew => {
                let show = self.sync_show_password;
                if let Some((_, password)) = self.pw_edit.as_mut() {
                    let real = if show { text } else { unmask(password, &text) };
                    if real != *password {
                        self.sync_mask_reset |= !show;
                    }
                    *password = real;
                }
            }
        }
    }

    fn on_field_submit(&mut self, field: Field) {
        match field {
            Field::Link => self.link_act(0),
            Field::Search => {
                if let Some(&first) = self.order.first() {
                    self.open(first);
                    self.edit_at(page::first(self.doc()));
                } else if !self.query.trim().is_empty() {
                    // Nothing matches: Enter makes the note searched for.
                    let title = self.query.trim().to_string();
                    self.new_note();
                    let note = self.note();
                    // A new note's title editor is new too: it opens on
                    // this text.
                    note.title = title;
                    note.renamable = true;
                    self.edited();
                    self.focus_req = None;
                    self.edit_at(page::first(self.doc()));
                }
            }
            // Enter in the title: into the body.
            Field::SyncServer => {}
            Field::SyncCode | Field::SyncEmail | Field::SyncPassword => self.on_plain(Msg::SyncStart),
            Field::PasswordEmail | Field::PasswordNew => self.on_plain(Msg::SyncPasswordSave),
            Field::DeviceName => self.on_plain(Msg::SyncRenameSave),
            Field::Title => self.edit_at(page::first(self.doc())),
            // Enter on a name: the folder is made, and shown. No name, no
            // folder.
            Field::Folder => {
                if let Some(old) = self.naming.as_ref().and_then(|n| n.rename.clone()) {
                    // A rename by Enter: the keyboard back on its row,
                    // under the name it has now.
                    let now = self.finish_naming(true, false).unwrap_or(old);
                    self.focus_row = Some(FolderRow::Folder(now));
                } else {
                    self.blur = true;
                    self.finish_naming(true, true);
                }
            }
            Field::Font => self.apply_typed_font(),
        }
    }

    /// A key that met the edge of a field's text. ↓ from the title's last
    /// line: into the note's first line, under the title's caret, as one
    /// text would; → at its end: to the note's start. ↑ and ← from there
    /// come back up.
    fn on_field_boundary(&mut self, field: Field, key: Key, p: &Value, core: &mut Core) {
        if field != Field::Title || p.get_str("edge") != Some("end") {
            return;
        }
        match p.get_str("key") {
            Some("down") => {
                let x = core.edit_caret_rect(key).map(|r| r.x);
                let at = x.and_then(|x| self.first_line_at(core, x));
                self.edit_at(at.unwrap_or_else(|| page::first(self.doc())));
                self.goal_x = x;
            }
            Some("right") => self.edit_at(page::first(self.doc())),
            _ => {}
        }
    }

    /// Hands focus to the field a model change asked for, the frame it is
    /// declared in.
    fn apply_focus(&mut self, ui: &mut Ui<'_>, k: Key, field: Field) {
        if self.focus_req != Some(field) {
            return;
        }
        ui.focus(k);
        ui.reveal(k);
        self.editing = false;
        self.focus_req = None;
    }
}


/// A folder's own name: the last part of its path.
pub fn folder_name(folder: &str) -> &str {
    folder.rsplit('/').next().unwrap_or(folder)
}

fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(windows) {
        "Show in Explorer"
    } else {
        "Open containing folder"
    }
}

/// When a version was made, as the history says it: "5 minutes ago".
pub(super) fn revision_ago(r: &Revision) -> String {
    chrome::ago(SystemTime::now(), revision_time(r))
}

pub(super) fn revision_time(r: &Revision) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(r.time.max(0) as u64)
}

impl Carnet {
    /// What made a version, and where, as its row in the history names
    /// it: "Edited here", "Edited on Laptop", "Changed in another app".
    pub(super) fn version_label(&self, r: &Revision) -> String {
        let base = match r.origin {
            Some(Origin::Import) => return "Where its history starts".into(),
            Some(Origin::Save) => "Edited",
            Some(Origin::OtherApp) => "Changed in another app",
            Some(Origin::Restore) => "Restored",
            None => "Changed",
        };
        let me = self.sync.device();
        match &r.device {
            Some(d) if Some(d.as_slice()) == me => format!("{base} here"),
            Some(d) => {
                let name = self.sync.devices.as_ref().and_then(|l| l.iter().find(|x| &x.id == d)).and_then(|x| x.name.clone());
                format!("{base} on {}", name.as_deref().unwrap_or("another device"))
            }
            // Written before devices were named, or with sync off: with it
            // off, written here.
            None if me.is_none() && r.origin == Some(Origin::Save) => "Edited here".into(),
            None => base.into(),
        }
    }
}

/// `s` with its first letter down: a label in the middle of a line.
pub(super) fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

fn blank_note() -> Note {
    Note::new("", Doc::new(vec![Block::text("")]))
}

impl App for Carnet {
    fn setup(&mut self, waker: Waker) {
        let wake = waker.clone();
        self.sync.set_wake(move || wake.wake());
        self.images.set_waker(waker);
    }

    fn view(&mut self, ui: &mut Ui<'_>) {
        // Autosave once typing has paused.
        if self.store.notes[self.current].dirty && self.last_edit.elapsed() >= SAVE_AFTER {
            self.save_current();
        }
        self.sync_tick();
        // A notice lasts a while, then goes; Undo lasts as long as it shows.
        if self.notice.as_ref().is_some_and(|n| n.at.elapsed() >= NOTICE_FOR) {
            self.notice = None;
        }
        // What is still to happen without input, asked of the frame clock:
        // the save once typing has paused, the notice going. (A save that
        // failed tries again a little later, not on every frame.)
        let note = &self.store.notes[self.current];
        let mut due = Vec::new();
        if note.dirty && !(note.is_blank() && note.path.is_none()) {
            due.push(SAVE_AFTER.saturating_sub(self.last_edit.elapsed()));
        }
        if let Some(n) = &self.notice {
            due.push(NOTICE_FOR.saturating_sub(n.at.elapsed()));
        }
        // Sync's turn after what happened: the notes' places and pictures
        // to the vault (a first login, a picture beside a note), the notes
        // on the server asked again; and the inspector's "Checked …" kept
        // true while it shows.
        due.extend(self.sync.due());
        if self.settings_open && self.sync_inspector && self.sync.inspected_at.is_some() {
            due.push(INSPECTED_EVERY);
        }
        if let Some(wait) = due.into_iter().min() {
            let wait = if wait.is_zero() { RETRY_AFTER } else { wait };
            ui.request_frame_at(ui.now() + wait.as_secs_f64());
        }
        if let Some(text) = self.say.take() {
            ui.announce(&text, kui_native::Live::Polite);
        }
        if let Some(forward) = self.step.take() {
            if forward {
                ui.focus_next();
            } else {
                ui.focus_prev();
            }
        }
        if std::mem::take(&mut self.blur) {
            ui.blur();
        }
        if let Some((text, html)) = self.clip_out.take() {
            ui.set_clipboard(text, html);
        }
        // Pictures the last frame didn't draw may go; a frame is asked
        // for when the next could, should nothing else draw one.
        if let Some(wait) = self.images.sweep(ui.core(), std::time::Instant::now()) {
            ui.request_frame_at(ui.now() + wait.as_secs_f64());
        }
        if self.images.pump(ui.core()) {
            self.reveal = self.reveal || self.editing;
        }
        if let Some(dir) = self.dir_change.take() {
            self.change_dir(dir, ui.core());
        }
        if std::mem::take(&mut self.pick_dir) {
            let start = self.store.dir.display().to_string();
            ui.request_files(
                kui_native::FileDialog::folder()
                    .title("Choose where your notes live")
                    .directory(start)
                    .tag("dir"),
            );
        }
        if std::mem::take(&mut self.pick_pictures) {
            ui.request_files(
                kui_native::FileDialog::open()
                    .multiple()
                    .title("Choose pictures")
                    .filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
                    .tag("pictures"),
            );
        }
        // Asked on every frame the answer is outstanding: the core queues
        // one ask at a time.
        if self.paste_pending {
            ui.request_paste();
        }
        if let Some((key, at, what)) = self.pending_menu.take() {
            let items = match what {
                MenuFor::Note(id) => self.note_menu(id),
                MenuFor::Folder(f) => self.folder_menu(&f),
                MenuFor::Tools => self.tools_menu(),
            MenuFor::Deleted(i) => self.deleted_menu(i),
                MenuFor::Picked => self.picked_menu(),
                MenuFor::Page => self.page_menu(),
            };
            ui.open_menu(Menu::new(key, at, items));
        }
        self.menu_open = ui.core().menu().is_some();
        if let Some(cmd) = self.window_cmd.take() {
            ui.window_command(cmd);
        }

        // Where the keyboard is: the page, a field, or neither.
        let focused = ui.focused();
        let field = focused.and_then(|k| self.fields.get(&k)).copied();
        self.search_active = field == Some(Field::Search);
        if focused.is_some() && focused == self.page_key {
            self.editing = true;
        } else if field.is_some() {
            self.editing = false;
        }
        if focused != self.last_focus {
            // The keyboard went somewhere: no longer the folders' arrows.
            if focused.is_some_and(|f| f != Key::ROOT) {
                self.folders_clicked = false;
            }
            // A folder's name let go of — a click elsewhere, Tab: the
            // folder is made, if it has a name, and where the click went
            // stays shown. Escape has already dropped it (`cancel`).
            if let Some(prev) = self.last_focus
                && self.naming.as_ref().is_some_and(|n| n.key == Some(prev))
            {
                self.finish_naming(true, false);
            }
            // One selection in the window: a field that lost the keyboard
            // stops drawing its own (kui keeps it for when it is back), and
            // the page's goes here.
            if field.is_some() {
                self.caret = Caret::at(self.caret.head);
            }
            self.last_focus = focused;
            // The title let go: now the file may take its new name.
            let title_now = field == Some(Field::Title);
            if self.title_focused && !title_now {
                self.title_focused = false;
                self.save_current();
            }
            self.title_focused = title_now;
        }
        // Where the platform has a menu bar (a Mac), it is the app's —
        // read against the last frame's fields, before they are declared
        // again, so a row knows whether the title or the search has the
        // keyboard.
        if ui.core().native_menu_bar() {
            let bar = self.menu_bar(ui.core());
            ui.core().declare_menu_bar(bar);
        }
        self.fields.clear();
        self.caret = page::fix(self.doc(), self.caret);
        self.follow_pointer(ui);
        let mut l = self.styler.look(ui, &self.settings);
        // Over the backdrop the window got, the library column is a wash
        // of its colour, the desktop showing through; the page stays paper.
        let see_through = match ui.env().window.backdrop {
            kui_native::Backdrop::Opaque => 1.0,
            // Over a blur or a material the system tints already: enough
            // wash that the library's quiet words (dates, previews, the
            // Deleted row) keep their contrast over a grey or busy desktop,
            // which still shows through as colour. Over the bare desktop,
            // nearly opaque.
            kui_native::Backdrop::Transparent => 0.94,
            kui_native::Backdrop::Blur => if l.c.dark { 0.66 } else { 0.84 },
            kui_native::Backdrop::Tinted => if l.c.dark { 0.7 } else { 0.86 },
        };
        if self.settings.translucent {
            l.side = l.side.with_alpha(see_through);
        }

        self.lights = ui.env().window.native_controls.map_or(0.0, |r| r.x + r.w + 12.0);
        let vw = ui.viewport().w;
        // Wide: the library beside the note. Narrow: one or the other —
        // the list, until a note is chosen; the note, with a way back.
        let narrow = vw < self.library_w() + PAGE_MIN;
        ui.configure_root(
            NodeSpec::row()
                .fill()
                // Clear over a backdrop: the library's own wash is the one
                // layer between the desktop and its words.
                .bg(if l.side.a < 1.0 { Color::TRANSPARENT } else { l.side })
                .on_key(Msg::RootKey)
                .on_drop(Value::Null),
        );
        self.follow_file_drag(ui);
        if narrow && self.list_view && !self.settings_open {
            self.sidebar(ui, &l, true);
            self.note_ghost(ui, &l);
            if self.hud {
                widgets::latency_hud(ui);
            }
            if cfg!(debug_assertions) || self.hud {
                self.devtools_tab(ui, &l);
            }
            return;
        }
        if !narrow {
            self.library_pane(ui, &l);
        }
        let main_w = vw - self.side_w(narrow);
        ui.with(
            NodeSpec::column().grow_width().grow_height().bg(l.t.bg),
            |ui| {
                if self.settings_open {
                    self.settings_page(ui, &l, narrow, main_w);
                } else {
                    self.strip(ui, &l, narrow);
                    self.page(ui, &l, main_w);
                    // The words scrolling up under the strip fade out
                    // rather than being cut at its edge. Paint only: a
                    // press here is the page's.
                    ui.leaf(
                        NodeSpec::row()
                            .float(FloatConfig::parent().offset(0.0, STRIP_H))
                            .size(main_w - 12.0, 22.0)
                            .gradient(kui_native::Gradient::to(kui_native::Side::Bottom, [l.t.bg, l.t.bg.with_alpha(0.0)]))
                            .role(Role::None),
                    );
                }
            },
        );
        if !self.settings_open {
            self.tint_picker(ui, &l);
            self.link_editor(ui, &l);
        }
        self.note_ghost(ui, &l);
        if self.hud {
            widgets::latency_hud(ui);
        }
        if cfg!(debug_assertions) || self.hud {
            self.devtools_tab(ui, &l);
        }
    }

    fn on_event_with(&mut self, ev: UiEvent, core: &mut Core) {

        // A modal put away (Esc, or a press outside it): the link editor.
        if ev.kind() == Some("dismiss") {
            if self.link_edit.take().is_some() {
                self.focus_page = true;
            }
            return;
        }
        if let Some(msg) = ev.message::<Msg>() {
            self.on_msg(msg, &ev, core);
            return;
        }
        match ev.kind() {
            Some("changed") => {
                if let Some(field) = self.fields.get(&ev.key).copied() {
                    let text = core.edit_text(ev.key).unwrap_or_default();
                    self.on_field_changed(field, text);
                }
            }
            Some("submit") => {
                if let Some(field) = self.fields.get(&ev.key).copied() {
                    self.on_field_submit(field);
                }
            }
            // Escape let go of a field: a folder's name is dropped.
            Some("cancel") => {
                if self.fields.get(&ev.key) == Some(&Field::Folder) {
                    // A rename dropped: the keyboard back on its row.
                    if let Some(f) = self.naming.as_ref().and_then(|n| n.rename.clone()) {
                        self.focus_row = Some(FolderRow::Folder(f));
                    }
                    self.finish_naming(false, false);
                }
            }
            // An arrow, Backspace or Delete that met the end of a field's
            // text.
            Some("boundary") => {
                if let Some(field) = self.fields.get(&ev.key).copied() {
                    self.on_field_boundary(field, ev.key, &ev.payload, core);
                }
            }
            Some("contextmenu") => {
                let p = &ev.payload;
                let num = |k: &str| {
                    p.get_float(k)
                        .or_else(|| p.get_int(k).map(|v| v as f64))
                        .unwrap_or(0.0) as f32
                };
                let at = Vec2::new(num("x"), num("y"));
                let folder = p.get_str("tag").and_then(|t| t.strip_prefix("folder:")).and_then(|i| i.parse::<usize>().ok()).and_then(|i| self.store.folders.get(i));
                if p.get_int("tag").is_some_and(|id| self.is_picked(&Pick::Note(id as u64)))
                    || folder.is_some_and(|f| self.is_picked(&Pick::Folder(f.clone())))
                {
                    self.pending_menu = Some((ev.key, at, MenuFor::Picked));
                } else if let Some(id) = p.get_int("tag") {
                    self.pending_menu = Some((ev.key, at, MenuFor::Note(id as u64)));
                } else if let Some(i) = p.get_str("tag").and_then(|t| t.strip_prefix("deleted:")).and_then(|i| i.parse().ok()) {
                    self.pending_menu = Some((ev.key, at, MenuFor::Deleted(i)));
                } else if p.get_str("tag") == Some("folder:all") {
                    self.pending_menu = Some((ev.key, at, MenuFor::Folder(String::new())));
                } else if let Some(i) = p.get_str("tag").and_then(|t| t.strip_prefix("folder:"))
                    && let Some(f) = i.parse::<usize>().ok().and_then(|i| self.store.folders.get(i))
                {
                    self.pending_menu = Some((ev.key, at, MenuFor::Folder(f.clone())));
                }
            }
            Some("menu") => {
                if let Some(item) = ev.payload.get("item")
                    && self.on_bar(item)
                {
                    return;
                }
                self.on_menu(&ev);
            }
            // The folder dialog's answer.
            Some("files") => {
                let paths: Vec<String> = ev
                    .payload
                    .get("paths")
                    .and_then(Value::as_list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                match ev.payload.get_str("tag") {
                    Some("pictures") => {
                        let files: Vec<_> = paths.iter().map(std::path::PathBuf::from).collect();
                        self.insert_files(&files);
                    }
                    _ => {
                        if let Some(dir) = paths.first() {
                            self.dir_change = Some(Some(std::path::PathBuf::from(dir)));
                        }
                    }
                }
            }
            // Files dragged over the window (the root is the one drop
            // zone; where they are is the page's question): tracked while
            // they hover, put where the drop line is when they land.
            Some("drop") => {
                let p = &ev.payload;
                let at = match (p.get_float("x"), p.get_float("y")) {
                    (Some(x), Some(y)) => Some(Vec2::new(x as f32, y as f32)),
                    _ => None,
                };
                match p.get_str("phase") {
                    Some("leave") => {
                        self.file_drag = None;
                        self.drop_index = None;
                    }
                    Some("drop") => {
                        let paths: Vec<std::path::PathBuf> = p
                            .get("paths")
                            .and_then(Value::as_list)
                            .unwrap_or(&[])
                            .iter()
                            .filter_map(|v| v.as_str().map(std::path::PathBuf::from))
                            .collect();
                        // On a card: into that note, at its end, whether
                        // it opened while the files rested there or not.
                        let card = self.file_drag.as_ref().and_then(|d| d.card).map(|(n, _)| n);
                        if let Some(n) = card {
                            self.open(n);
                            self.drop_index = Some(self.doc().len());
                        }
                        let at = self.drop_index.take();
                        self.file_drag = None;
                        self.insert_files_at(&paths, at);
                    }
                    _ => {
                        let drag = self.file_drag.get_or_insert(FileDrag { at: None, card: None });
                        if at.is_some() {
                            drag.at = at;
                        }
                    }
                }
            }
            // Back from another app: catch up with what it did to the
            // folder.
            Some("window") if ev.payload.get_str("phase") == Some("focused") => {
                self.typing = false;
                self.rescan();
                self.refresh_trash();
                // A picture that could not be read (locked, half-synced)
                // gets another try.
                self.images.retry_failed();
                // Back from another app: the page takes the keyboard back
                // if it had it.
                if std::mem::take(&mut self.refocus) && core.focus() != self.page_key {
                    self.focus_page = true;
                }
            }
            Some("window") if ev.payload.get_str("phase") == Some("blurred") => {
                // A menu does not outlive the window's attention.
                core.close_menu();
                self.refocus = self.editing && core.focus().is_some() && core.focus() == self.page_key;
            }
            _ => {}
        }
    }

    fn teardown(&mut self) {
        self.title_focused = false;
        self.save_current();
        if self.store.save_all().is_err() {
            // The last chance: whatever would not save goes to a file of
            // its own, so nothing typed is lost with the window.
            for i in 0..self.store.notes.len() {
                if self.store.notes[i].dirty && !self.store.notes[i].is_blank() {
                    let _ = self.store.rescue(i);
                }
            }
        }
    }
}

fn gap(ui: &mut Ui<'_>, w: f32) {
    ui.leaf(NodeSpec::row().width(w));
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

/// What a password field is shown as: a dot a letter.
pub const DOT: char = '\u{2022}';

/// The password a field of dots now stands for: `real` was the text, and
/// `shown` is what the field holds after an edit. Dots at either end are
/// the letters kept; what isn't a dot was typed or pasted between them.
/// A letter taken out of the middle can't be told from one at the end
/// (dots are all alike): Show reveals the text for that.
pub fn unmask(real: &str, shown: &str) -> String {
    let old: Vec<char> = real.chars().collect();
    let new: Vec<char> = shown.chars().collect();
    let head = new.iter().take_while(|&&c| c == DOT).count().min(old.len());
    let tail = new[head..].iter().rev().take_while(|&&c| c == DOT).count().min(old.len() - head);
    let typed: String = new[head..new.len() - tail].iter().filter(|&&c| c != DOT).collect();
    old[..head].iter().chain(typed.chars().collect::<Vec<_>>().iter()).chain(old[old.len() - tail..].iter()).collect()
}
