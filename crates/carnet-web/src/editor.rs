//! The web editor's model: the desktop's own page ([`carnet_core::page`])
//! over a note's [`Doc`], and the text CodeMirror shows for it — the words
//! alone, a line of CodeMirror for each line of the note, the title first.
//! No mark is ever in that text, so none can be seen, stood in or typed
//! half of: what an edit does is the desktop's doing, and the note is
//! written back by the desktop's own Markdown.
//!
//! CodeMirror edits the words; each edit comes here as a place in them
//! (a line and a column, in UTF-16 as JavaScript counts) and is made with
//! the page's functions, whose rules are the desktop's: typing carries the
//! style of the letter before it, Backspace undoes a block's structure
//! first, a style switched on with nothing selected styles what is typed
//! next. Then the page's words go back to CodeMirror, and what it draws
//! (a heading's size, a box, a bullet, a style's colour) is read from
//! [`Editor::lines`].

use std::time::Duration;

use carnet_core::compare;
use carnet_core::doc::{self, Doc, Kind, ListStyle, Table};
use carnet_core::history::{History, State, Step};
use carnet_core::inline::{self, Style};
use carnet_core::page::{self, Caret, PicDrop, Pos, Slot, TableOp, Typing};

/// What stands for a divider, a picture or a table in CodeMirror's text:
/// one letter, drawn as the thing itself.
pub const OBJECT: char = '\u{FFFC}';

/// One line of what CodeMirror shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Its block's index; `None` for the title.
    pub block: Option<usize>,
    /// Its slot (a table's first cell for a table).
    pub slot: Option<Slot>,
    /// Where it starts in its slot's words, in bytes.
    pub start: usize,
    /// Its words.
    pub text: String,
    /// The first line of its block (where a block's space above goes).
    pub first: bool,
}

/// Where the caret is: in the title (its bytes), or in the note.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Focus {
    Title { anchor: usize, head: usize },
    Body,
}

pub struct Editor {
    pub title: String,
    pub doc: Doc,
    pub caret: Caret,
    focus: Focus,
    history: History,
    /// Styles switched on for what is typed next, and spaces carried past
    /// a run's marks: the desktop's, as `carnet_core::page` keeps them.
    typing: Typing,
    /// The column last sorted by, and whether downwards: the same one
    /// again sorts the other way, as on the desktop.
    last_sort: Option<(u64, bool)>,
    lines: Vec<Line>,
}

fn now() -> Duration {
    #[cfg(target_arch = "wasm32")]
    {
        Duration::from_secs_f64(js_sys::Date::now() / 1000.0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default()
    }
}

/// UTF-16 column `col` of `s` as a byte offset.
fn u16_to_byte(s: &str, col: usize) -> usize {
    let mut n = 0;
    for (i, ch) in s.char_indices() {
        if n >= col {
            return i;
        }
        n += ch.len_utf16();
    }
    s.len()
}

/// Byte offset `b` of `s` as a UTF-16 column.
fn byte_to_u16(s: &str, b: usize) -> usize {
    let mut b = b.min(s.len());
    while !s.is_char_boundary(b) {
        b -= 1;
    }
    s[..b].encode_utf16().count()
}

impl Editor {
    /// The note's Markdown, to edit.
    pub fn open(md: &str) -> Editor {
        let (title, doc) = doc::from_markdown(md);
        let mut doc = doc;
        if doc.is_empty() {
            doc.push(doc::Block::text(""));
        }
        doc.ensure_tail();
        let first = page::first(&doc);
        let mut e = Editor {
            title: title.unwrap_or_default(),
            doc,
            caret: Caret::at(first),
            focus: Focus::Body,
            history: History::default(),
            typing: Typing::default(),
            last_sort: None,
            lines: Vec::new(),
        };
        e.relay();
        e
    }

    /// The note as Markdown, as the desktop writes it.
    pub fn markdown(&self) -> String {
        let title = self.title.trim();
        doc::to_markdown((!title.is_empty()).then_some(title), &self.doc)
    }

    /// The note became `md` (an edit from elsewhere merged in): the caret
    /// and the selection kept where they were in the words, carried
    /// through the change (`compare::carry`) — in the same block after the
    /// same letters, wherever the change moved them; in the title, the
    /// same.
    pub fn reload(&mut self, md: &str) {
        let (title, mut doc) = doc::from_markdown(md);
        let title = title.unwrap_or_default();
        if doc.is_empty() {
            doc.push(doc::Block::text(""));
        }
        doc.ensure_tail();
        self.caret = page::fix(&doc, compare::carry(&self.doc, &doc, self.caret));
        if let Focus::Title { anchor, head } = self.focus {
            let carry = |b: usize| compare::carry_byte(&self.title, &title, b);
            self.focus = Focus::Title { anchor: carry(anchor), head: carry(head) };
        }
        self.title = title;
        self.doc = doc;
        self.history = History::default();
        self.typing = Typing::default();
        self.relay();
    }

    // ------------------------------------------------------------ lines

    /// The lines CodeMirror shows, as the note is now.
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// CodeMirror's whole text.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    fn relay(&mut self) {
        let mut out = vec![Line { block: None, slot: None, start: 0, text: self.title.clone(), first: true }];
        for (i, b) in self.doc.blocks().iter().enumerate() {
            let slots = page::block_slots(b);
            if matches!(b.kind, Kind::Table(_)) || b.kind.is_atomic() {
                out.push(Line { block: Some(i), slot: Some(slots[0]), start: 0, text: OBJECT.to_string(), first: true });
                continue;
            }
            let slot = slots[0];
            let t = page::text(&self.doc, slot);
            let words = page::visible(&self.doc, slot).text(t);
            // Code's lines between its fences: the fences are Markdown,
            // never shown, nor stood on (`page::writable`).
            let (lo, hi) = if matches!(b.kind, Kind::Raw(_)) { page::writable(&self.doc, slot) } else { (0, words.len()) };
            let mut start = lo;
            for (n, line) in words[lo..hi].split('\n').enumerate() {
                out.push(Line { block: Some(i), slot: Some(slot), start, text: line.to_string(), first: n == 0 });
                start += line.len() + 1;
            }
        }
        self.lines = out;
    }

    /// A place in CodeMirror's text (line, UTF-16 column) as a place in the
    /// note; `None` in the title. `end` for a selection's end, which in a
    /// table goes to its last cell.
    fn pos(&self, line: usize, col: usize) -> Option<Pos> {
        let l = self.lines.get(line.min(self.lines.len() - 1))?;
        let slot = l.slot?;
        let i = l.block?;
        let b = &self.doc.blocks()[i];
        if let Kind::Table(_) = b.kind {
            let slots = page::block_slots(b);
            return Some(if col == 0 {
                Pos { slot: slots[0], byte: 0 }
            } else {
                let last = *slots.last().unwrap();
                Pos { slot: last, byte: page::len(&self.doc, last) }
            });
        }
        if b.kind.is_atomic() {
            return Some(Pos { slot, byte: col.min(1) });
        }
        let v = page::visible(&self.doc, slot);
        let d = l.start + u16_to_byte(&l.text, col);
        Some(Pos { slot, byte: v.at(d) })
    }

    /// A place in the note as CodeMirror's line and column.
    fn lc(&self, p: Pos) -> (usize, usize) {
        let block = p.slot.block;
        let Some(i) = self.doc.index_of(block) else { return (0, 0) };
        let b = &self.doc.blocks()[i];
        let mine: Vec<usize> = (0..self.lines.len()).filter(|&n| self.lines[n].block == Some(i)).collect();
        let Some(&first) = mine.first() else { return (0, 0) };
        if let Kind::Table(_) = b.kind {
            let slots = page::block_slots(b);
            return (first, usize::from(slots.first() != Some(&p.slot) || p.byte > 0));
        }
        if b.kind.is_atomic() {
            return (first, p.byte.min(1));
        }
        // In code, a place on a fence is shown at the nearer end of the
        // lines between them.
        let byte = if matches!(b.kind, Kind::Raw(_)) {
            let (lo, hi) = page::writable(&self.doc, p.slot);
            p.byte.clamp(lo, hi)
        } else {
            p.byte
        };
        let d = page::visible(&self.doc, p.slot).drawn(byte);
        let n = mine.iter().rev().copied().find(|&n| self.lines[n].start <= d).unwrap_or(first);
        let l = &self.lines[n];
        (n, byte_to_u16(&l.text, d - l.start))
    }

    /// The selection as CodeMirror's places: anchor line, column, head line, column.
    pub fn caret_lc(&self) -> (usize, usize, usize, usize) {
        match self.focus {
            Focus::Title { anchor, head } => (0, byte_to_u16(&self.title, anchor), 0, byte_to_u16(&self.title, head)),
            Focus::Body => {
                let a = self.lc(self.caret.anchor);
                let h = self.lc(self.caret.head);
                (a.0, a.1, h.0, h.1)
            }
        }
    }

    /// The selection CodeMirror has. One across the title and the note is
    /// taken as the note's part. Where the caret is at that place in the
    /// words already, it stays as the page put it: past the marks that
    /// just closed a style, say, which the place read afresh would put it
    /// back inside ([`inline::Visible::at`]), so the words after them
    /// would carry the style on.
    pub fn select(&mut self, a_line: usize, a_col: usize, h_line: usize, h_col: usize) {
        let in_text = self.caret.anchor.slot.cell.is_none() && self.caret.head.slot.cell.is_none();
        if matches!(self.focus, Focus::Body) && in_text && a_line > 0 && h_line > 0 && self.caret_lc() == (a_line, a_col, h_line, h_col) {
            return;
        }
        if a_line == 0 && h_line == 0 {
            self.focus = Focus::Title {
                anchor: u16_to_byte(&self.title, a_col),
                head: u16_to_byte(&self.title, h_col),
            };
            return;
        }
        let first = page::first(&self.doc);
        let a = self.pos(a_line, a_col).unwrap_or(first);
        let h = self.pos(h_line, h_col).unwrap_or(first);
        let c = page::fix(&self.doc, Caret { anchor: a, head: h });
        // Down from the title by a bare caret (CodeMirror's own move, ↓ or
        // → at its end): onto a picture or a divider there, it's chosen
        // whole, as from a line above it, so what is typed next takes its
        // place ([`page::from_title`]).
        let c = if matches!(self.focus, Focus::Title { .. }) && c.is_empty() { page::from_title(&self.doc, c.head) } else { c };
        if c != self.caret {
            self.history.seal();
            self.typing.moved();
        }
        self.caret = c;
        self.focus = Focus::Body;
    }

    // ------------------------------------------------------------ edits

    /// `f` on the note, as one step of the history.
    fn edit(&mut self, step: Step, f: impl FnOnce(&mut Doc, &mut Caret) -> bool) -> bool {
        if step != Step::Typing {
            self.typing.moved();
        }
        let before = State::of(&self.doc, self.caret);
        let block = self.caret.head.slot.block;
        let mut caret = self.caret;
        let changed = f(&mut self.doc, &mut caret);
        // Some only move the caret (Enter down a table's column): that is
        // something done too, for the view to follow.
        let moved = caret != self.caret;
        self.caret = caret;
        if changed {
            self.history.record_at(step, block, before, now());
            self.doc.ensure_tail();
        } else {
            self.history.seal();
        }
        self.relay();
        changed || moved
    }

    /// The title's selection taken out; where the caret is then.
    fn title_cut(&mut self) -> Option<usize> {
        let Focus::Title { anchor, head } = self.focus else { return None };
        let (a, b) = (anchor.min(head), anchor.max(head));
        self.title.replace_range(a..b, "");
        self.focus = Focus::Title { anchor: a, head: a };
        Some(a)
    }

    /// Text typed (or put in by the keyboard's input method) over the
    /// selection: inside the styles switched on for it, if any, and never
    /// a space where Markdown's marks can't have one ([`Typing`]).
    pub fn input(&mut self, s: &str) -> bool {
        if let Some(at) = self.title_cut() {
            let s = s.replace('\n', " ");
            self.title.insert_str(at, &s);
            self.focus = Focus::Title { anchor: at + s.len(), head: at + s.len() };
            self.relay();
            return true;
        }
        let mut typing = std::mem::take(&mut self.typing);
        let changed = self.edit(Step::Typing, |d, c| typing.type_text(d, c, s));
        self.typing = typing;
        changed
    }

    /// A chosen picture or divider made an empty paragraph for an input
    /// method about to compose over it ([`Typing::make_room`]): its words
    /// then composed in its place, not at the start of what comes after
    /// it, where the browser's own caret stands. The same step of the
    /// history as the words, so one Undo brings the picture back.
    pub fn make_room(&mut self) -> bool {
        if !matches!(self.focus, Focus::Body) || page::chosen_atom(&self.doc, self.caret).is_none() {
            return false;
        }
        let before = State::of(&self.doc, self.caret);
        let mut typing = std::mem::take(&mut self.typing);
        let changed = typing.make_room(&mut self.doc, &mut self.caret);
        self.typing = typing;
        if changed {
            self.history.seal();
            self.history.record_at(Step::Typing, self.caret.head.slot.block, before, now());
            self.doc.ensure_tail();
        }
        self.relay();
        changed
    }

    /// Enter (Shift+Enter: a line break in the paragraph). In the title,
    /// to the note's start.
    pub fn enter(&mut self, soft: bool) -> bool {
        if matches!(self.focus, Focus::Title { .. }) {
            self.focus = Focus::Body;
            self.caret = Caret::at(page::first(&self.doc));
            return true;
        }
        self.edit(Step::Other, |d, c| page::enter(d, c, soft))
    }

    pub fn backspace(&mut self, word: bool) -> bool {
        if let Focus::Title { anchor, head } = self.focus {
            if anchor != head {
                return self.title_cut().is_some() && self.relaid();
            }
            if head == 0 {
                return false;
            }
            let from = if word { prev_word(&self.title, head) } else { prev_char(&self.title, head) };
            self.title.replace_range(from..head, "");
            self.focus = Focus::Title { anchor: from, head: from };
            return self.relaid();
        }
        // At the very start of the note: up into the title.
        if self.caret.is_empty() && self.caret.head == page::first(&self.doc) && matches!(self.doc.blocks()[0].kind, Kind::Text(_)) {
            let end = self.title.len();
            self.focus = Focus::Title { anchor: end, head: end };
            return true;
        }
        self.edit(Step::Deleting, |d, c| page::backspace(d, c, word))
    }

    pub fn delete(&mut self, word: bool) -> bool {
        if let Focus::Title { anchor, head } = self.focus {
            if anchor != head {
                return self.title_cut().is_some() && self.relaid();
            }
            if head >= self.title.len() {
                return false;
            }
            let to = if word { next_word(&self.title, head) } else { next_char(&self.title, head) };
            self.title.replace_range(head..to, "");
            return self.relaid();
        }
        self.edit(Step::Deleting, |d, c| page::delete(d, c, word))
    }

    fn relaid(&mut self) -> bool {
        self.relay();
        true
    }

    /// Tab and Shift+Tab: an item in or out, the next cell, or a tab
    /// character (`page::tab`).
    pub fn tab(&mut self, back: bool) -> bool {
        if !matches!(self.focus, Focus::Body) {
            return false;
        }
        if page::tab_types(&self.doc, self.caret, back) {
            return self.input("\t");
        }
        self.edit(Step::Other, |d, c| page::tab(d, c, back).0)
    }

    /// The caret's blocks turned into `to` (`page::TURN_*`); back into
    /// text when they're all that already.
    pub fn turn(&mut self, to: u64) -> bool {
        if !matches!(self.focus, Focus::Body) {
            return false;
        }
        let to = if self.turned_to(to) { page::TURN_TEXT } else { to };
        self.edit(Step::Other, |d, c| page::turn(d, c, to))
    }

    /// Whether every block the caret touches is `to` already.
    pub fn turned_to(&self, to: u64) -> bool {
        if !matches!(self.focus, Focus::Body) {
            return false;
        }
        page::touched(&self.doc, self.caret).all(|i| {
            matches!(
                (&self.doc.blocks()[i].kind, to),
                (Kind::Heading { level: 1, .. }, page::TURN_H1)
                    | (Kind::Heading { level: 2, .. }, page::TURN_H2)
                    | (Kind::Heading { level: 3.., .. }, page::TURN_H3)
                    | (Kind::Item { style: ListStyle::Bullet, .. }, page::TURN_BULLET)
                    | (Kind::Item { style: ListStyle::Number, .. }, page::TURN_NUMBER)
                    | (Kind::Item { style: ListStyle::Todo, .. }, page::TURN_TODO)
                    | (Kind::Quote(_), page::TURN_QUOTE)
            )
        })
    }

    /// Bold, italic, struck, underlined or code (`inline::Kind`) on the
    /// selection; with
    /// nothing selected, or a picture or divider chosen, on (or off) for
    /// what is typed next.
    pub fn style(&mut self, kind: inline::Kind) -> bool {
        if !matches!(self.focus, Focus::Body) {
            return false;
        }
        if page::styles_typing(&self.doc, self.caret) {
            let (pre, post) = self.typing.switch(&self.doc, self.caret, kind);
            self.typing.pend(self.caret, &format!("{kind:?}"), pre, post);
            return true;
        }
        self.edit(Step::Other, |d, c| page::style(d, c, kind))
    }

    /// A colour (`ink`) or a highlighter on the selection, 0 taking it
    /// off; with nothing selected, for what is typed next.
    pub fn tint(&mut self, ink: bool, color: u32) -> bool {
        if !matches!(self.focus, Focus::Body) {
            return false;
        }
        if self.caret.is_empty() {
            self.typing.pend_tint(&self.doc, self.caret, ink, color);
            return true;
        }
        self.edit(Step::Other, |d, c| page::tint(d, c, ink, color))
    }

    /// The style typing would have at the caret: the letter before it's,
    /// with what's switched on for what's typed next.
    pub fn style_here(&self) -> Style {
        if !matches!(self.focus, Focus::Body) {
            return Style::default();
        }
        let mut s = self.typing.style_at(&self.doc, self.caret);
        if let Some(p) = self.typing.pending_at(self.caret) {
            for (key, ..) in &p.layers {
                match key.as_str() {
                    "Bold" => s.bold = !s.bold,
                    "Italic" => s.italic = !s.italic,
                    "Strike" => s.strike = !s.strike,
                    "Underline" => s.underline = !s.underline,
                    "Code" => s.code = !s.code,
                    "mark" => s.mark = self.typing.pending_tint(self.caret, false).unwrap_or(s.mark),
                    _ => {}
                }
            }
        }
        s
    }

    /// A picture (`src` as the note links it) on its own line below the
    /// caret's block (in place of an empty paragraph), the caret past it,
    /// no empty line made before the block after (`page::insert_block`);
    /// in a table's cell, among its words at the caret.
    pub fn insert_picture(&mut self, src: &str) -> bool {
        if !matches!(self.focus, Focus::Body) {
            self.focus = Focus::Body;
            self.caret = Caret::at(page::first(&self.doc));
        }
        // In a table's cell: among its words.
        if self.caret.head.slot.cell.is_some() {
            return self.edit(Step::Other, |d, c| page::insert_cell_picture(d, c, src));
        }
        let kind = Kind::Image { src: src.to_string(), alt: String::new(), width: None, float: doc::Float::None, wiki: false };
        self.edit(Step::Other, |d, c| page::insert_block(d, c, kind))
    }

    // ------------------------------------------------------------ pictures

    /// The one picture the caret is on, or selects: its line, where it
    /// sits and the width the note gives it.
    pub fn picture_here(&self) -> Option<(usize, doc::Float, Option<u32>)> {
        if !matches!(self.focus, Focus::Body) {
            return None;
        }
        let i = page::picture_at(&self.doc, self.caret)?;
        let line = self.lines.iter().position(|l| l.block == Some(i))?;
        match self.doc.blocks()[i].kind {
            Kind::Image { float, width, .. } => Some((line, float, width)),
            _ => None,
        }
    }

    /// The caret's picture given `width` (`None`: its own), as one step
    /// of the history; it stays selected.
    pub fn set_picture_width(&mut self, width: Option<u32>) -> bool {
        let Some(i) = page::picture_at(&self.doc, self.caret).filter(|_| matches!(self.focus, Focus::Body)) else { return false };
        let id = self.doc.blocks()[i].id;
        self.edit(Step::Other, |d, _| page::set_image_width(d, id, width))
    }

    /// The caret's picture put on its own line or at an edge; `toggle`, as
    /// a shortcut does, back on its own line when it is there already.
    pub fn set_picture_float(&mut self, to: doc::Float, toggle: bool) -> bool {
        let Some((_, now, _)) = self.picture_here() else { return false };
        let Some(i) = page::picture_at(&self.doc, self.caret) else { return false };
        let id = self.doc.blocks()[i].id;
        let to = if toggle { page::float_toggled(now, to) } else { to };
        self.edit(Step::Other, |d, _| page::set_image_float(d, id, to))
    }

    /// The caret's picture taken out of the note.
    pub fn remove_picture(&mut self) -> bool {
        if self.picture_here().is_none() {
            return false;
        }
        self.edit(Step::Other, page::remove_block)
    }

    /// Where a picture carried in the editor is let go, as a [`PicDrop`]:
    /// `None` for a place that isn't one.
    fn drop_at(&self, to: PicTo) -> Option<PicDrop> {
        match to {
            // Before the block CodeMirror's line `n` is the first line of;
            // a later line of one, after it; past the last, at the end.
            PicTo::Line(n) => Some(PicDrop::Before(match self.lines.get(n) {
                Some(Line { block: Some(i), first, .. }) => i + usize::from(!first),
                Some(_) => 0,
                None => self.doc.len(),
            })),
            PicTo::Beside(n, after) => {
                let b = &self.doc.blocks()[self.lines.get(n)?.block?];
                matches!(b.kind, Kind::Image { .. }).then_some(PicDrop::Beside { id: b.id, after })
            }
            PicTo::Cell(n, r, c, col) => {
                let slot = self.cell_slot(n, r, c)?;
                let at = match col {
                    Some(col) => {
                        let v = page::visible(&self.doc, slot);
                        v.at(u16_to_byte(&v.text(page::text(&self.doc, slot)), col))
                    }
                    None => usize::MAX,
                };
                Some(PicDrop::Cell { slot, at })
            }
        }
    }

    /// The picture on CodeMirror's line `from` carried to `to`: between
    /// blocks, beside another picture, or among a table cell's words (its
    /// line gone, its width kept, its side dropped). One step of the
    /// history; it is selected where it lands.
    pub fn move_picture(&mut self, from: usize, to: PicTo) -> bool {
        let Some(i) = self.lines.get(from).and_then(|l| l.block) else { return false };
        let id = self.doc.blocks()[i].id;
        if !matches!(self.doc.blocks()[i].kind, Kind::Image { .. }) {
            return false;
        }
        let Some(to) = self.drop_at(to) else { return false };
        self.focus = Focus::Body;
        self.edit(Step::Other, |d, c| page::move_picture(d, c, id, to))
    }

    /// The picture among the words of a table's cell (CodeMirror's line
    /// `line`, row `r`, column `c`) whose letter is at UTF-16 column `at`
    /// carried to `to`: to another cell, or another place in its own; out
    /// of the table, a picture on a line of its own (or beside another),
    /// its width kept. One step of the history; it is selected where it
    /// lands.
    pub fn move_cell_picture(&mut self, line: usize, r: usize, c: usize, at: usize, to: PicTo) -> bool {
        let Some(slot) = self.cell_slot(line, r, c) else { return false };
        let t = page::text(&self.doc, slot);
        let v = page::visible(&self.doc, slot);
        let words = v.text(t);
        let Some(p) = inline::cell_pictures(t).into_iter().find(|p| byte_to_u16(&words, v.drawn(p.at.0)) == at) else { return false };
        let Some(to) = self.drop_at(to) else { return false };
        self.focus = Focus::Body;
        self.edit(Step::Other, |d, c| page::move_cell_picture(d, c, slot, p.at.0, to))
    }

    // ------------------------------------------------------------ tables

    /// The slot of a table's cell: CodeMirror's line, the row, the column.
    fn cell_slot(&self, line: usize, r: usize, c: usize) -> Option<Slot> {
        let (_, block, t) = self.table_on(line)?;
        Some(Slot { block, cell: Some((t.rows.get(r)?.id, *t.cols.get(c)?)) })
    }

    /// The table on CodeMirror's line `line`: its block's index and the table.
    fn table_on(&self, line: usize) -> Option<(usize, u64, &Table)> {
        let i = self.lines.get(line)?.block?;
        let b = &self.doc.blocks()[i];
        match &b.kind {
            Kind::Table(t) => Some((i, b.id, t)),
            _ => None,
        }
    }

    /// The caret put in a table's cell (row `r`, column `c`), between
    /// UTF-16 columns `a` and `h` of its words.
    pub fn cell_select(&mut self, line: usize, r: usize, c: usize, a: usize, h: usize) -> bool {
        let Some((_, block, t)) = self.table_on(line) else { return false };
        let (Some(row), Some(&col)) = (t.rows.get(r).map(|row| row.id), t.cols.get(c)) else { return false };
        let slot = Slot { block, cell: Some((row, col)) };
        let v = page::visible(&self.doc, slot);
        let words = v.text(page::text(&self.doc, slot));
        let at = |n: usize| Pos { slot, byte: v.at(u16_to_byte(&words, n)) };
        let caret = page::fix(&self.doc, Caret { anchor: at(a), head: at(h) });
        if caret != self.caret {
            self.history.seal();
            self.typing.moved();
        }
        self.caret = caret;
        self.focus = Focus::Body;
        true
    }

    /// ← (`right` false) or → in a table's cell, where a picture is
    /// beside the caret or chosen: [`page::left`] or [`page::right`], which
    /// choose the picture first and pass it after, the caret staying in
    /// the cell. False, the caret as it was, for any other move: the
    /// browser's, as a cell's words are its.
    pub fn cell_arrow(&mut self, right: bool) -> bool {
        let c = self.caret;
        if !matches!(self.focus, Focus::Body) || c.head.slot.cell.is_none() {
            return false;
        }
        let next = if right { page::right(&self.doc, c, false, false) } else { page::left(&self.doc, c, false, false) };
        let picture = page::chosen_cell_picture(&self.doc, c).is_some() || page::chosen_cell_picture(&self.doc, next).is_some();
        if !picture || next.head.slot != c.head.slot {
            return false;
        }
        self.history.seal();
        self.typing.moved();
        self.caret = next;
        true
    }

    /// Where the caret is when it is in a table's cell: CodeMirror's line,
    /// the row, the column, and the selection in the cell's words (UTF-16).
    pub fn cell_caret(&self) -> Option<(usize, usize, usize, usize, usize)> {
        if !matches!(self.focus, Focus::Body) {
            return None;
        }
        let p = self.caret.head;
        let (row, col) = p.slot.cell?;
        let i = self.doc.index_of(p.slot.block)?;
        let line = self.lines.iter().position(|l| l.block == Some(i))?;
        let Kind::Table(t) = &self.doc.blocks()[i].kind else { return None };
        let v = page::visible(&self.doc, p.slot);
        let words = v.text(page::text(&self.doc, p.slot));
        let col16 = |byte: usize| byte_to_u16(&words, v.drawn(byte));
        let a = if self.caret.anchor.slot == p.slot { col16(self.caret.anchor.byte) } else { col16(p.byte) };
        Some((line, t.row_index(row)?, t.col_index(col)?, a, col16(p.byte)))
    }

    /// A row or a column added after the caret's, or the caret's taken
    /// out: `0` row, `1` column, `2` take the row out, `3` the column.
    pub fn table_op(&mut self, op: u8) -> bool {
        let op = match op {
            0 => TableOp::AddRow,
            1 => TableOp::AddCol,
            2 => TableOp::DelRow,
            3 => TableOp::DelCol,
            _ => TableOp::Header,
        };
        self.edit(Step::Other, |d, c| page::table_op(d, c, op, false))
    }

    /// The table sorted by the caret's column, A to Z; again, Z to A.
    pub fn sort_table(&mut self) -> bool {
        let Some((_, col)) = self.caret.head.slot.cell else { return false };
        let desc = self.last_sort.is_some_and(|(k, d)| k == col && !d);
        let done = self.edit(Step::Other, |d, c| page::table_op(d, c, TableOp::Sort, desc));
        self.last_sort = Some((col, desc));
        done
    }

    /// How the caret's column was last sorted: `Some(false)` A to Z,
    /// `Some(true)` Z to A, `None` not.
    pub fn sorted(&self) -> Option<bool> {
        let (_, col) = self.caret.head.slot.cell?;
        self.last_sort.filter(|(k, _)| *k == col).map(|(_, d)| d)
    }

    /// Whether the table the caret is in has a header row.
    pub fn table_header(&self) -> bool {
        let Some(i) = self.caret.head.slot.cell.and_then(|_| self.doc.index_of(self.caret.head.slot.block)) else { return false };
        matches!(&self.doc.blocks()[i].kind, Kind::Table(t) if t.header)
    }

    /// The table the caret is in, taken out whole.
    pub fn remove_table(&mut self) -> bool {
        if self.caret.head.slot.cell.is_none() {
            return false;
        }
        self.edit(Step::Other, page::remove_block)
    }

    /// A new table, three by three, below the caret's block (in place of
    /// an empty paragraph), the caret in its first cell.
    pub fn insert_table(&mut self) -> bool {
        if !matches!(self.focus, Focus::Body) {
            return false;
        }
        self.edit(Step::Other, |d, c| page::insert_block(d, c, Kind::Table(Table::new(3, 3))))
    }

    /// A divider below the caret's block (in place of an empty paragraph),
    /// the caret carried on into what follows, as the desktop's toolbar
    /// puts one in.
    pub fn insert_divider(&mut self) -> bool {
        if !matches!(self.focus, Focus::Body) {
            return false;
        }
        self.edit(Step::Other, |d, c| page::insert_block(d, c, Kind::Divider))
    }

    /// The address of the link the caret is in, as the link sheet shows
    /// it: no bidi control in it to turn it round.
    pub fn link_here(&self) -> Option<String> {
        if !matches!(self.focus, Focus::Body) {
            return None;
        }
        page::link_at(&self.doc, self.caret).map(|(.., url)| inline::without_bidi(&url))
    }

    /// The address the link the caret is in opens, if it's one a page
    /// may follow ([`carnet_core::export::safe_href`]): a `javascript:`
    /// link is edited and taken off like any, but not opened.
    pub fn link_to_open(&self) -> Option<String> {
        self.link_here().as_deref().and_then(carnet_core::export::safe_href)
    }

    /// The address a press at CodeMirror's `line` and `col` opens, links
    /// opening `how` (`page::LinkOpen::key`; ⌘-click if it's none of
    /// them): the link whose words are there, if a page may follow it
    /// ([`page::link_under`]). What the pointer is a hand over, too.
    pub fn link_under(&self, line: usize, col: usize, how: &str) -> Option<String> {
        if line == 0 {
            return None;
        }
        let pos = self.pos(line, col)?;
        let url = page::link_under(&self.doc, pos, page::LinkOpen::from_key(how).unwrap_or_default())?;
        carnet_core::export::safe_href(&inline::without_bidi(&url))
    }

    /// The link the caret is in given address `url`, or taken off its
    /// words (`None`); else the selection made a link to `url` (with
    /// nothing selected, the address typed in as its own link).
    pub fn set_link(&mut self, url: Option<&str>) -> bool {
        if !matches!(self.focus, Focus::Body) {
            return false;
        }
        if let Some((slot, at, _)) = page::link_at(&self.doc, self.caret) {
            return self.edit(Step::Other, |d, _| page::set_link(d, slot, at, url));
        }
        match url.map(str::trim).filter(|u| !u.is_empty()) {
            Some(u) => self.edit(Step::Other, |d, c| page::make_link(d, c, u)),
            None => false,
        }
    }

    /// The style at the caret, colours and all (the pending ones aside).
    pub fn style_here_full(&self) -> Style {
        if !matches!(self.focus, Focus::Body) {
            return Style::default();
        }
        page::style_at(&self.doc, self.caret)
    }

    /// A checklist item's box ticked or unticked (another item made one).
    pub fn toggle(&mut self, line: usize) -> bool {
        let Some(i) = self.lines.get(line).and_then(|l| l.block) else { return false };
        let id = self.doc.blocks()[i].id;
        self.edit(Step::Other, |d, _| page::toggle(d, id))
    }

    /// The selection as Markdown, for the clipboard.
    pub fn copy(&self) -> Option<String> {
        match self.focus {
            Focus::Title { anchor, head } => (anchor != head).then(|| self.title[anchor.min(head)..anchor.max(head)].to_string()),
            Focus::Body => page::copy(&self.doc, self.caret),
        }
    }

    /// The Markdown a copy's HTML is made from: [`Editor::copy`]'s, but
    /// lines from inside one code block in its fences, so the HTML is
    /// code ([`page::copy_for_html`]).
    pub fn copy_for_html(&self) -> Option<String> {
        match self.focus {
            Focus::Title { .. } => self.copy(),
            Focus::Body => page::copy_for_html(&self.doc, self.caret),
        }
    }

    /// Text from the clipboard: one line typed in, several made blocks.
    pub fn paste(&mut self, s: &str) -> bool {
        if matches!(self.focus, Focus::Title { .. }) {
            return self.input(s.lines().next().unwrap_or(""));
        }
        self.edit(Step::Other, |d, c| page::paste(d, c, s))
    }

    pub fn undo(&mut self) -> bool {
        let Some(back) = self.history.undo(State::of(&self.doc, self.caret)) else { return false };
        self.doc.restore(back.blocks);
        self.caret = page::fix(&self.doc, back.caret);
        self.focus = Focus::Body;
        self.relay();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(ahead) = self.history.redo(State::of(&self.doc, self.caret)) else { return false };
        self.doc.restore(ahead.blocks);
        self.caret = page::fix(&self.doc, ahead.caret);
        self.focus = Focus::Body;
        self.relay();
        true
    }

    // ------------------------------------------------------------ drawing

    /// What a line is, for CodeMirror to draw: its kind and what goes with it.
    pub fn look(&self, n: usize) -> Look {
        let l = &self.lines[n];
        let Some(i) = l.block else { return Look { kind: "title", ..Look::default() } };
        let b = &self.doc.blocks()[i];
        let mut look = Look { first: l.first, ..Look::default() };
        match &b.kind {
            Kind::Text(t) => {
                look.kind = "text";
                // A dropped first letter, on the paragraph's first line, as
                // the desktop drops it; its own colour, if it has one, kept.
                if b.initial && l.start == 0
                    && let Some((f, to)) = inline::initial_letter(t)
                {
                    let st = inline::styles(t);
                    look.initial = if (f..to).any(|i| !st[i].marker && st[i].ink != 0) { 2 } else { 1 };
                }
            }
            Kind::Heading { level, .. } => look.kind = ["h1", "h2", "h3"][(*level as usize).clamp(1, 3) - 1],
            Kind::Item { style, indent, done, .. } => {
                look.kind = match style {
                    ListStyle::Bullet => "bullet",
                    ListStyle::Number => "number",
                    ListStyle::Todo => "todo",
                };
                look.indent = *indent;
                look.done = *done;
                look.number = self.number(i);
            }
            Kind::Quote(_) => look.kind = "quote",
            Kind::Raw(t) => look.kind = if t.starts_with("---\n") { "front" } else { "code" },
            Kind::Divider => look.kind = "divider",
            Kind::Image { src, alt, width, float, wiki } => {
                look.kind = "image";
                look.src = src.clone();
                look.wiki = *wiki;
                look.alt = alt.clone();
                look.width = *width;
                look.float = *float;
            }
            Kind::Table(t) => {
                look.kind = "table";
                look.head = t.header;
                look.cells = t
                    .rows
                    .iter()
                    .map(|r| {
                        t.cols
                            .iter()
                            .map(|&col| {
                                let slot = Slot { block: b.id, cell: Some((r.id, col)) };
                                let v = page::visible(&self.doc, slot);
                                let t = page::text(&self.doc, slot);
                                let words = v.text(t);
                                let line = Line { block: Some(i), slot: Some(slot), start: 0, text: words.clone(), first: true };
                                // Each picture where its one letter is in the words.
                                let pictures = inline::cell_pictures(t)
                                    .into_iter()
                                    .map(|p| CellPicture { at: byte_to_u16(&words, v.drawn(p.at.0)), src: p.src, alt: p.alt, width: p.width, wiki: p.wiki })
                                    .collect();
                                Cell { words, runs: self.runs(slot, &line), pictures }
                            })
                            .collect()
                    })
                    .collect();
            }
        }
        if let (Some(slot), true) = (l.slot, matches!(b.kind, Kind::Text(_) | Kind::Heading { .. } | Kind::Item { .. } | Kind::Quote(_))) {
            look.runs = self.runs(slot, l);
        }
        look
    }

    /// A numbered item's number, as the desktop counts: a run starts at
    /// the number it was written with, then counts on.
    fn number(&self, at: usize) -> u32 {
        let mut numbers: Vec<u32> = Vec::new();
        for (i, b) in self.doc.blocks().iter().enumerate() {
            match &b.kind {
                Kind::Item { style, indent, num, .. } => {
                    let depth = *indent as usize;
                    numbers.truncate(depth + 1);
                    numbers.resize(depth + 1, 0);
                    numbers[depth] = if *style == ListStyle::Number {
                        if numbers[depth] == 0 && *num > 0 { *num } else { numbers[depth] + 1 }
                    } else {
                        0
                    };
                }
                _ => numbers.clear(),
            }
            if i == at {
                return numbers.last().copied().unwrap_or(1);
            }
        }
        1
    }

    /// A line's words in runs of one style: `(from, to)` in UTF-16 with
    /// the style, plain runs left out.
    fn runs(&self, slot: Slot, l: &Line) -> Runs {
        let _r = page::reading(slot);
        let t = page::text(&self.doc, slot);
        let v = page::visible(&self.doc, slot);
        let styles = inline::styles(t);
        let end = l.start + l.text.len();
        let mut out: Vec<(usize, usize, Style)> = Vec::new();
        let mut d = 0;
        for &(f, to) in &v.runs {
            for (k, s) in styles[f..to].iter().enumerate() {
                let at = d + k;
                if at < l.start || at >= end || l.text.as_bytes().get(at - l.start).is_none() {
                    continue;
                }
                let s = Style { marker: false, ..*s };
                if s == Style::default() {
                    continue;
                }
                let (a, b) = (at - l.start, at - l.start + 1);
                match out.last_mut() {
                    Some(last) if last.2 == s && last.1 == a => last.1 = b,
                    _ => out.push((a, b, s)),
                }
            }
            d += to - f;
        }
        // Bytes to UTF-16 columns, each run on whole letters.
        out.into_iter()
            .map(|(a, b, s)| (byte_to_u16(&l.text, a), byte_to_u16(&l.text, b.max(a)).max(byte_to_u16(&l.text, a)), s))
            .filter(|(a, b, _)| b > a)
            .collect()
    }
}

/// Where a picture carried in the editor is let go, in CodeMirror's
/// terms: before the block on a line (past the last line, at the end);
/// beside the picture on a line, after it or before; in a table's cell
/// (its line, row and column) at a UTF-16 column of its words, or at
/// their end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicTo {
    Line(usize),
    Beside(usize, bool),
    Cell(usize, usize, usize, Option<usize>),
}

/// Words in runs of one style: `(from, to)` in UTF-16 with the style.
pub type Runs = Vec<(usize, usize, Style)>;

/// A table's cell, as it is drawn: its words, their styles, and its
/// pictures, each one letter of the words (a run with `picture`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    pub words: String,
    pub runs: Runs,
    pub pictures: Vec<CellPicture>,
}

/// A picture among a cell's words: where its letter is (UTF-16), its
/// `src` as the note has it (an Obsidian embed's, with `wiki`), its alt
/// text and the width the note gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellPicture {
    pub at: usize,
    pub src: String,
    pub alt: String,
    pub width: Option<u32>,
    pub wiki: bool,
}

/// How a line is drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub kind: &'static str,
    pub first: bool,
    pub indent: u8,
    pub done: bool,
    pub number: u32,
    pub runs: Runs,
    pub src: String,
    /// The `src` is an Obsidian embed's: a name to look for.
    pub wiki: bool,
    pub alt: String,
    pub width: Option<u32>,
    /// A picture's place: on its own line, or at an edge.
    pub float: doc::Float,
    /// A table's cells, row by row.
    pub cells: Vec<Vec<Cell>>,
    /// The table's first row is its header.
    pub head: bool,
    /// A paragraph's first line whose first letter drops: 1, or 2 when
    /// the letter has a colour of its own.
    pub initial: u8,
}

fn prev_char(s: &str, b: usize) -> usize {
    s[..b].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn next_char(s: &str, b: usize) -> usize {
    s[b..].chars().next().map_or(b, |c| b + c.len_utf8())
}

fn prev_word(s: &str, b: usize) -> usize {
    let t = s[..b].trim_end();
    t.rfind(char::is_whitespace).map_or(0, |i| i + 1)
}

fn next_word(s: &str, b: usize) -> usize {
    let rest = &s[b..];
    let skip = rest.len() - rest.trim_start().len();
    rest[skip..].find(char::is_whitespace).map_or(s.len(), |i| b + skip + i)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Types `s` at line `l`, column `c`.
    fn type_at(e: &mut Editor, l: usize, c: usize, s: &str) {
        e.select(l, c, l, c);
        e.input(s);
    }

    /// An edit from elsewhere leaves the caret where it was in the words:
    /// after the same letters of the same line, wherever the change moved
    /// them, so the next letter typed lands there.
    #[test]
    fn a_reload_keeps_the_caret_in_its_words() {
        let mut e = Editor::open("# Shop\n\n- milk1\n- bread1\n");
        e.select(2, 6, 2, 6);
        // In another line.
        e.reload("# Shop\n\n- milk1 WEB\n- bread1\n");
        assert_eq!(e.caret_lc(), (2, 6, 2, 6));
        e.input("x");
        assert!(e.markdown().contains("- milk1 WEB\n- bread1x\n"), "{}", e.markdown());
        // Before it, in its own line: it moves with its words.
        e.reload("# Shop\n\n- milk1 WEB\n- fresh bread1x\n");
        assert_eq!(e.caret_lc(), (2, 13, 2, 13));
        // A line put in above: the same line, one down.
        e.reload("# Shop\n\nAt noon\n\n- milk1 WEB\n- fresh bread1x\n");
        assert_eq!(e.caret_lc(), (3, 13, 3, 13));
        e.input("y");
        assert!(e.markdown().contains("- fresh bread1xy\n"), "{}", e.markdown());
        // A selection, end by end; in the title, too.
        e.select(3, 6, 3, 11);
        e.reload("# Shop\n\nAt noon\n\n- milk1 WEB\n- so fresh bread1xy\n");
        assert_eq!(e.caret_lc(), (3, 9, 3, 14));
        e.select(0, 4, 0, 4);
        e.reload("# My Shop\n\nAt noon\n\n- milk1 WEB\n- so fresh bread1xy\n");
        assert_eq!(e.caret_lc(), (0, 7, 0, 7));
    }

    #[test]
    fn the_words_alone_a_line_each() {
        let e = Editor::open("# Shop\n\n- [ ] **milk** and ==eggs==\n- [x] bread\n\nA <span style=\"color: #c03030\">red</span> word.\n\n---\n");
        let lines: Vec<&str> = e.lines().iter().map(|l| l.text.as_str()).collect();
        assert_eq!(lines, ["Shop", "milk and eggs", "bread", "A red word.", "\u{fffc}", ""]);
        assert_eq!(e.look(1).kind, "todo");
        assert!(e.look(2).done);
        let bold: Vec<(usize, usize)> = e.look(1).runs.iter().filter(|r| r.2.bold).map(|r| (r.0, r.1)).collect();
        assert_eq!(bold, [(0, 4)]);
        assert!(e.look(1).runs.iter().any(|r| r.2.mark != 0 && (r.0, r.1) == (9, 13)));
        assert!(e.look(3).runs.iter().any(|r| r.2.ink != 0 && (r.0, r.1) == (2, 5)));
        assert_eq!(e.look(4).kind, "divider");
    }

    /// A paragraph's dropped first letter is its first line's, as the
    /// desktop draws it; a letter with a colour of its own says so.
    #[test]
    fn a_dropped_first_letter_is_its_first_lines() {
        let e = Editor::open("# T\n\n<span class=\"initial\">T</span>he coast road\nruns on.\n\n<span class=\"initial\" style=\"color: #C0553F\">O</span>nce.\n\nPlain.\n");
        let lines: Vec<(&str, u8)> = e.lines().iter().enumerate().map(|(i, l)| (l.text.as_str(), e.look(i).initial)).collect();
        assert_eq!(lines[1..5], [("The coast road", 1), ("runs on.", 0), ("Once.", 2), ("Plain.", 0)]);
    }

    /// A picture selected whole; its width and side are set as
    /// the desktop sets them, written in its alt text, each one step to
    /// undo; a shortcut for the side it is at puts it back on its own line.
    #[test]
    fn a_picture_resized_and_put_at_an_edge() {
        let mut e = Editor::open("# Trip\n\nBefore.\n\n![the view](view.png)\n\nAfter.\n");
        assert_eq!(e.picture_here(), None);
        e.select(2, 0, 2, 1);
        assert_eq!(e.picture_here(), Some((2, doc::Float::None, None)));
        // The caret beside it is on it too, as on the desktop.
        e.select(2, 1, 2, 1);
        assert_eq!(e.picture_here().map(|p| p.0), Some(2));
        assert!(e.set_picture_width(Some(320)));
        assert!(e.set_picture_float(doc::Float::Left, false));
        assert_eq!(e.picture_here(), Some((2, doc::Float::Left, Some(320))));
        assert!(e.markdown().contains("![the view|left|320](view.png)"), "{}", e.markdown());
        let look = e.look(2);
        assert_eq!((look.width, look.float), (Some(320), doc::Float::Left));
        // A shortcut: to the other side, then back on its own line.
        assert!(e.set_picture_float(doc::Float::Right, true));
        assert!(e.markdown().contains("![the view|right|320](view.png)"));
        assert!(e.set_picture_float(doc::Float::Right, true));
        assert!(e.markdown().contains("![the view|320](view.png)"));
        // Its own width again; each change one step back.
        assert!(e.set_picture_width(None));
        assert!(e.markdown().contains("![the view](view.png)"));
        assert!(e.undo());
        assert!(e.markdown().contains("![the view|320](view.png)"));
        // Nothing to do with words in the selection, or in the title.
        e.select(1, 0, 2, 1);
        assert!(!e.set_picture_width(Some(100)) && !e.set_picture_float(doc::Float::Left, false));
        e.select(0, 1, 0, 1);
        assert_eq!(e.picture_here(), None);
        // Taken out.
        e.select(2, 0, 2, 1);
        assert!(e.remove_picture());
        assert!(!e.markdown().contains("view.png"));
        assert_eq!(e.lines().iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Trip", "Before.", "After."]);
    }

    /// An Obsidian embed is a picture on the page, its `src` a name to look
    /// for, and is written back as it was when words around it change.
    #[test]
    fn an_obsidian_embed_is_a_picture_and_stays_one() {
        let mut e = Editor::open("# Trip\n\n![[photo.png|200]]\n\nwords\n");
        let look = e.look(1);
        assert_eq!((look.kind, look.src.as_str(), look.wiki, look.width), ("image", "photo.png", true, Some(200)));
        type_at(&mut e, 2, 5, " more");
        assert_eq!(e.markdown(), "# Trip\n\n![[photo.png|200]]\n\nwords more\n");
    }

    #[test]
    fn typing_carries_the_style_of_the_letter_before() {
        let mut e = Editor::open("# T\n\nbuy **milk** now\n");
        // After "milk": bold goes on.
        type_at(&mut e, 1, 8, "s");
        assert!(e.markdown().contains("buy **milks** now"), "{}", e.markdown());
        // Before "milk": plain.
        type_at(&mut e, 1, 4, "x");
        assert!(e.markdown().contains("buy x**milks** now"), "{}", e.markdown());
        assert_eq!(e.lines()[1].text, "buy xmilks now");
    }

    #[test]
    fn a_style_switched_on_styles_what_is_typed_next() {
        let mut e = Editor::open("# T\n\nsay \n");
        e.select(1, 4, 1, 4);
        assert!(e.style(inline::Kind::Bold));
        assert!(e.style_here().bold);
        e.input("hi");
        assert!(e.markdown().contains("say **hi**"), "{}", e.markdown());
        // Off again, mid-word.
        e.select(1, 6, 1, 6);
        e.style(inline::Kind::Bold);
        e.input("!");
        assert!(e.markdown().contains("say **hi**!"), "{}", e.markdown());
    }

    #[test]
    fn a_selection_styled_as_the_desktop_styles_it() {
        let mut e = Editor::open("# T\n\none two three\n");
        e.select(1, 4, 1, 7);
        e.style(inline::Kind::Italic);
        assert!(e.markdown().contains("one *two* three"));
        e.select(1, 4, 1, 7);
        e.tint(false, inline::HIGHLIGHT);
        assert!(e.markdown().contains("*==two==*"), "as the desktop nests it: {}", e.markdown());
        // The words CodeMirror has are the same throughout.
        assert_eq!(e.lines()[1].text, "one two three");
    }

    /// Underline as the desktop has it: `<u>` in the note, never among
    /// the words CodeMirror has; on the selection, and for what is typed
    /// with none; off again the same way; a copy's HTML `<u>`.
    #[test]
    fn underline_on_the_selection_and_for_what_is_typed() {
        let mut e = Editor::open("# T\n\none two three\n");
        e.select(1, 4, 1, 7);
        assert!(e.style(inline::Kind::Underline));
        assert!(e.markdown().contains("one <u>two</u> three"), "{}", e.markdown());
        assert_eq!(e.lines()[1].text, "one two three");
        assert!(e.look(1).runs.iter().any(|(a, b, s)| (*a, *b) == (4, 7) && s.underline), "drawn underlined");
        assert!(e.style_here().underline);
        let html = e.copy_for_html().map(|md| carnet_core::export::html(&md, |s, _| s.to_string()));
        assert_eq!(html.as_deref(), Some("<p><u>two</u></p>"));
        e.select(1, 4, 1, 7);
        e.style(inline::Kind::Underline);
        assert!(e.markdown().contains("one two three"), "{}", e.markdown());
        // Nothing selected: on for what is typed, the toolbar's state too.
        e.select(1, 13, 1, 13);
        assert!(e.style(inline::Kind::Underline));
        assert!(e.style_here().underline);
        e.input(" four");
        assert!(e.markdown().contains("one two three <u>four</u>"), "{}", e.markdown());
        assert_eq!(e.lines()[1].text, "one two three four");
    }

    #[test]
    fn backspace_undoes_structure_first_then_joins() {
        let mut e = Editor::open("# T\n\nfirst\n\n- [ ] milk\n");
        e.select(2, 0, 2, 0);
        e.backspace(false);
        assert!(e.markdown().contains("first\n\nmilk"), "the box goes: {}", e.markdown());
        e.select(2, 0, 2, 0);
        e.backspace(false);
        assert_eq!(e.lines()[1].text, "firstmilk");
        assert_eq!(e.caret_lc(), (1, 5, 1, 5));
    }

    #[test]
    fn enter_continues_a_list_and_steps_out_of_an_empty_item() {
        let mut e = Editor::open("# T\n\n- [ ] milk\n");
        e.select(1, 4, 1, 4);
        e.enter(false);
        e.input("eggs");
        assert!(e.markdown().contains("- [ ] milk\n- [ ] eggs"), "{}", e.markdown());
        e.enter(false);
        e.enter(false);
        e.input("done");
        assert!(e.markdown().ends_with("- [ ] eggs\n\ndone\n"), "{}", e.markdown());
    }

    #[test]
    fn the_title_is_its_own_line() {
        let mut e = Editor::open("# Plan\n\nwords\n");
        type_at(&mut e, 0, 4, "s");
        assert!(e.markdown().starts_with("# Plans\n"));
        e.select(0, 5, 0, 5);
        e.enter(false);
        assert_eq!(e.caret_lc(), (1, 0, 1, 0));
        e.backspace(false);
        assert_eq!(e.caret_lc(), (0, 5, 0, 5), "up into the title");
    }

    /// Types `s` a letter at a time, as a keyboard does, the caret where
    /// the page last put it.
    fn keys(e: &mut Editor, s: &str) {
        for ch in s.chars() {
            let (a, b, c, d) = e.caret_lc();
            e.select(a, b, c, d);
            e.input(&ch.to_string());
        }
    }

    #[test]
    fn spaces_never_stand_where_marks_cant_have_them() {
        // Bold on, then a space first: plain, the bold still on after it.
        let mut e = Editor::open("# T\n\nword.\n");
        e.select(1, 5, 1, 5);
        e.style(inline::Kind::Bold);
        keys(&mut e, " Bold here");
        assert_eq!(e.markdown().lines().nth(2), Some("word. **Bold here**"));
        assert_eq!(e.lines()[1].text, "word. Bold here");
        // A space at the end of a run, then the caret elsewhere: plain,
        // past the marks; no mark ever among the words.
        let mut e = Editor::open("# T\n\nsay \n");
        e.select(1, 4, 1, 4);
        e.style(inline::Kind::Bold);
        keys(&mut e, "hi ");
        assert_eq!(e.lines()[1].text, "say hi ");
        assert!(e.style_here().bold, "the carried space is the run's");
        e.select(1, 0, 1, 0);
        assert_eq!(e.markdown().lines().nth(2), Some("say **hi** "));
        // At a bold word's start, a space goes before its marks.
        let mut e = Editor::open("# T\n\n**bold**\n");
        keys(&mut e, " ");
        assert_eq!(e.markdown().lines().nth(2), Some(" **bold**"));
        // A word and its space at once (an input method, autocorrect): the
        // same.
        let mut e = Editor::open("# T\n\n**go**\n");
        e.select(1, 2, 1, 2);
        e.input("ing on");
        assert_eq!(e.markdown().lines().nth(2), Some("**going on**"));
    }

    /// Marks typed by hand that close a style leave the caret past them,
    /// though CodeMirror tells the same place back after each key: the
    /// words after are plain.
    #[test]
    fn a_style_closed_by_typed_marks_ends_there() {
        for m in ["**", "*", "~~", "==", "`"] {
            let mut e = Editor::open("# T\n\n\n");
            e.select(1, 0, 1, 0);
            for ch in format!("{m}tb{m} after").chars() {
                let (a, b, c, d) = e.caret_lc();
                e.select(a, b, c, d);
                e.input(&ch.to_string());
            }
            assert_eq!(e.markdown().lines().nth(2), Some(format!("{m}tb{m} after").as_str()));
            assert_eq!(e.style_here(), Style::default(), "{m}");
        }
    }

    /// Typed at the end of a link's words, words go past the link: made a
    /// link of their own, the first keeps its address.
    #[test]
    fn typing_at_a_links_end_goes_past_it() {
        let mut e = Editor::open("# T\n\nsee [docs](http://a)\n");
        e.select(1, 8, 1, 8);
        keys(&mut e, " now");
        assert_eq!(e.markdown().lines().nth(2), Some("see [docs](http://a) now"));
        assert_eq!(e.caret_lc(), (1, 12, 1, 12));
        e.select(1, 9, 1, 12);
        assert!(e.set_link(Some("http://b")));
        assert_eq!(e.markdown().lines().nth(2), Some("see [docs](http://a) [now](http://b)"));
        e.select(1, 6, 1, 6);
        assert_eq!(e.link_here().as_deref(), Some("http://a"));
    }

    /// `` ``` `` and Enter, as CodeMirror sends them, open a code block
    /// with the caret on its first line; a language typed after the marks
    /// stays on the fence.
    #[test]
    fn a_code_block_opens_on_enter_at_its_first_line() {
        for (typed, fence) in [("```", "```"), ("```rust", "```rust")] {
            let mut e = Editor::open("# T\n\n\n");
            e.select(1, 0, 1, 0);
            keys(&mut e, typed);
            assert!(e.enter(false));
            keys(&mut e, "let x = 1;");
            assert_eq!(e.markdown(), format!("# T\n\n{fence}\nlet x = 1;\n```\n"));
            assert_eq!(e.lines()[1].text, "let x = 1;", "the fences not shown");
        }
    }

    /// The editor shows words only: a code block's fences are not among
    /// CodeMirror's lines, so no arrow or click can put the caret on one.
    /// ↓ from its last line is the next block, ↑ from its first the one
    /// before; Backspace and Delete at its ends leave the fences whole.
    #[test]
    fn a_code_blocks_fences_are_not_shown_nor_stood_on() {
        let md = "# T\n\nAbove\n\n```rust\nlet x;\nlet y;\n```\n\nBelow\n";
        let mut e = Editor::open(md);
        let texts: Vec<&str> = e.lines().iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["T", "Above", "let x;", "let y;", "Below"]);
        assert!(!e.text().contains('`'));
        // A place in the code's lines is the same place in the note.
        e.select(3, 4, 3, 4);
        assert!(e.input("!"));
        assert_eq!(e.markdown(), "# T\n\nAbove\n\n```rust\nlet x;\nlet !y;\n```\n\nBelow\n");
        assert_eq!(e.caret_lc(), (3, 5, 3, 5));
        // ↓ from the last line, ↑ from the first: CodeMirror's next line
        // is the block beyond.
        let md = e.markdown();
        e.select(4, 0, 4, 0);
        assert!(e.input("B"));
        e.select(1, 5, 1, 5);
        assert!(e.input("A"));
        assert_eq!(e.markdown(), md.replace("Above", "AboveA").replace("Below", "BBelow"));
        // Backspace at the code's start goes up, Delete at its end down:
        // neither takes a fence's line break.
        let md = e.markdown();
        e.select(2, 0, 2, 0);
        e.backspace(false);
        e.select(3, 7, 3, 7);
        e.delete(false);
        assert_eq!(e.markdown(), md);
        // A caret the page put on a fence is shown in the lines between.
        e.caret = Caret::at(Pos { slot: Slot::block(e.doc.blocks()[1].id), byte: 0 });
        assert_eq!(e.caret_lc().2, 2);
    }

    /// The finding: code with no line between its fences showed them,
    /// there being no line to show. It shows one empty line, as code
    /// made by `/code` does, and what is typed there goes between them.
    #[test]
    fn code_with_no_line_between_its_fences_shows_one_empty_line() {
        for fence in ["```", "```rust"] {
            let mut e = Editor::open(&format!("# T\n\nAbove\n\n{fence}\n```\n\nBelow\n"));
            let texts: Vec<&str> = e.lines().iter().map(|l| l.text.as_str()).collect();
            assert_eq!(texts, ["T", "Above", "", "Below"]);
            assert!(!e.text().contains('`'));
            e.select(2, 0, 2, 0);
            assert_eq!(e.caret_lc(), (2, 0, 2, 0));
            keys(&mut e, "let x;");
            assert_eq!(e.markdown(), format!("# T\n\nAbove\n\n{fence}\nlet x;\n```\n\nBelow\n"));
            assert_eq!(e.lines()[2].text, "let x;");
            assert_eq!(e.caret_lc(), (2, 6, 2, 6));
        }
    }

    /// A picture put in at the end of the words before a table: no empty
    /// line between, the caret on in the table's first cell.
    #[test]
    fn a_picture_before_a_table_leaves_no_empty_line() {
        let mut e = Editor::open("# T\n\nIntro\n\n| a |\n| --- |\n");
        e.select(1, 5, 1, 5);
        assert!(e.insert_picture("attachments/a.png"));
        assert_eq!(e.markdown(), "# T\n\nIntro\n\n![](attachments/a.png)\n\n| a |\n| --- |\n");
        assert_eq!(e.cell_caret().map(|c| (c.1, c.2)), Some((0, 0)));
    }

    #[test]
    fn links_made_changed_and_taken_off() {
        let mut e = Editor::open("# T\n\nsee the site\n");
        e.select(1, 4, 1, 12);
        assert!(e.set_link(Some("example.com")));
        assert!(e.markdown().contains("see [the site](https://example.com)"), "{}", e.markdown());
        assert_eq!(e.lines()[1].text, "see the site");
        e.select(1, 6, 1, 6);
        assert_eq!(e.link_here().as_deref(), Some("https://example.com"));
        assert!(e.set_link(Some("https://carnet.example")));
        assert!(e.markdown().contains("(https://carnet.example)"));
        assert_eq!(e.link_to_open().as_deref(), Some("https://carnet.example"));
        assert!(e.set_link(None));
        assert!(e.markdown().contains("see the site"));
        assert_eq!(e.link_here(), None);
        assert_eq!(e.link_to_open(), None);
    }

    /// A link that would run (`javascript:`) is there to change or take
    /// off, but not to open; one to a note or a mail is.
    #[test]
    fn only_a_link_a_page_may_follow_opens() {
        for (url, opens) in [("javascript:alert(1)", None), ("JavaScript:x", None), ("data:text/html,x", None), ("mailto:a@b", Some("mailto:a@b")), ("notes/x.md", Some("notes/x.md")), ("#top", Some("#top"))] {
            let mut e = Editor::open(&format!("# T\n\n[go]({url})\n"));
            e.select(1, 1, 1, 1);
            assert!(e.link_here().is_some(), "{url}: {}", e.markdown());
            assert_eq!(e.link_to_open().as_deref(), opens, "{url}");
        }
    }

    /// The link under a press, as the editor's pointer asks: inside its
    /// words; at their end only when a click doesn't open it; never one
    /// a page may not follow; never in the title.
    #[test]
    fn the_link_under_a_press() {
        let mut e = Editor::open("# T\n\nsee [docs](https://a.test) [run](javascript:x)\n");
        assert_eq!(e.link_under(1, 6, "click").as_deref(), Some("https://a.test"));
        assert_eq!(e.link_under(1, 8, "click"), None, "at the words' end");
        assert_eq!(e.link_under(1, 8, "cmd-click").as_deref(), Some("https://a.test"));
        assert_eq!(e.link_under(1, 6, "double-click").as_deref(), Some("https://a.test"));
        assert_eq!(e.link_under(1, 2, "click"), None);
        assert_eq!(e.link_under(1, 11, "click"), None, "a script's link");
        assert_eq!(e.link_under(0, 1, "click"), None, "the title");
        e.select(1, 0, 1, 0);
        assert_eq!(e.link_under(1, 6, "nonsense").as_deref(), Some("https://a.test"), "⌘-click when unknown");
    }

    /// A picture carried into a table's cell, along to another, and out
    /// to a line of its own again, as the desktop carries one: written
    /// plainly in a cell (its alt kept, no width nor side), out again with
    /// no width, each move one step of the history, and the picture
    /// selected where it lands.
    #[test]
    fn a_picture_is_carried_into_a_cell_between_cells_and_out() {
        let md = "# T\n\nWords\n\n![view|right|260](a.png)\n\n| A | B |\n| --- | --- |\n| x | y z |\n";
        let mut e = Editor::open(md);
        assert_eq!(e.look(2).kind, "image");
        assert!(e.move_picture(2, PicTo::Cell(3, 1, 0, None)));
        assert_eq!(e.markdown(), "# T\n\nWords\n\n| A | B |\n| --- | --- |\n| x![view](a.png) | y z |\n");
        assert_eq!(e.cell_caret(), Some((2, 1, 0, 1, 2)), "selected in the cell");
        assert_eq!(e.look(2).cells[1][0].pictures.len(), 1);
        assert!(e.undo());
        assert_eq!(e.markdown(), md, "one undo puts it back");
        assert!(e.redo());
        // To the next cell, between its words.
        assert!(e.move_cell_picture(2, 1, 0, 1, PicTo::Cell(2, 1, 1, Some(2))));
        assert!(e.markdown().contains("| x | y ![view](a.png)z |"), "{}", e.markdown());
        assert_eq!(e.cell_caret(), Some((2, 1, 1, 2, 3)));
        // Out, above the words: a line of its own.
        assert!(e.move_cell_picture(2, 1, 1, 2, PicTo::Line(1)));
        assert_eq!(e.markdown(), "# T\n\n![view](a.png)\n\nWords\n\n| A | B |\n| --- | --- |\n| x | y z |\n");
        assert_eq!(e.picture_here().map(|p| p.0), Some(1), "selected");
        assert!(e.undo());
        assert!(e.markdown().contains("| x | y ![view](a.png)z |"), "{}", e.markdown());
        // Beside a picture: at the left with it.
        let mut e = Editor::open("# T\n\n![](b.png)\n\n| A |\n| --- |\n| ![|90](c.png) |\n");
        assert!(e.move_cell_picture(2, 1, 0, 0, PicTo::Beside(1, true)));
        assert_eq!(e.markdown(), "# T\n\n![|left](b.png)\n\n![|left](c.png)\n\n| A |\n| --- |\n|  |\n");
        // Nothing there: nothing done.
        assert!(!e.move_cell_picture(3, 1, 0, 0, PicTo::Line(0)));
        assert!(!e.move_picture(3, PicTo::Line(0)));
    }

    /// The header row turned off and on, as the desktop does: its words
    /// stay, a plain row under an empty header line; the page is told
    /// which row, if any, is the header.
    #[test]
    fn a_tables_header_goes_and_comes_back() {
        let mut e = Editor::open("# T\n\n| Day | Miles |\n| --- | --- |\n| Fri | 30 |\n");
        assert!(e.look(1).head);
        e.cell_select(1, 0, 0, 0, 0);
        assert!(e.table_header());
        assert!(e.table_op(4));
        assert!(!e.look(1).head && !e.table_header());
        assert_eq!(e.look(1).cells.len(), 2, "no row goes");
        assert!(e.markdown().contains("| | |\n| --- | --- |\n| Day | Miles |\n| Fri | 30 |"), "{}", e.markdown());
        assert!(e.table_op(4));
        assert!(e.look(1).head);
        assert_eq!(e.look(1).cells.len(), 2);
        assert!(e.markdown().contains("| Day | Miles |\n| --- | --- |\n| Fri | 30 |"), "{}", e.markdown());
    }

    /// The toolbar's divider, as the desktop's: between two paragraphs it
    /// makes no empty line, and the caret goes on into the next.
    #[test]
    fn a_divider_from_the_toolbar_goes_between_without_a_line() {
        let mut e = Editor::open("# T\n\nIntro\n\nNext\n");
        e.select(1, 5, 1, 5);
        assert!(e.insert_divider());
        assert_eq!(e.markdown(), "# T\n\nIntro\n\n---\n\nNext\n");
        assert_eq!(e.lines().iter().map(|l| l.text.as_str()).filter(|t| t.is_empty()).count(), 0, "no empty line made");
    }

    /// The finding: a picture chosen (→, or dropped out of a cell) with a
    /// table after it, "Q" typed made the header cell "QA". As on the
    /// desktop, the letter takes the picture's place; the table is kept.
    #[test]
    fn a_letter_typed_over_a_chosen_picture_takes_its_place() {
        let mut e = Editor::open("# T\n\nAbove\n\n![](a.png)\n\n| A |\n| --- |\n| 1 |\n");
        assert_eq!(e.lines()[2].text, OBJECT.to_string());
        e.select(2, 0, 2, 1);
        assert!(e.input("Q"));
        assert_eq!(e.markdown(), "# T\n\nAbove\n\nQ\n\n| A |\n| --- |\n| 1 |\n");
        let mut e = Editor::open("# T\n\n![](a.png)\n\nbelow words\n");
        e.select(1, 1, 1, 0);
        e.input("Q");
        assert_eq!(e.markdown(), "# T\n\nQ\n\nbelow words\n");
        // Bold switched on with it chosen: on for the words in its place.
        let mut e = Editor::open("# T\n\n![](a.png)\n\nbelow words\n");
        e.select(1, 0, 1, 1);
        assert!(e.style(inline::Kind::Bold));
        keys(&mut e, "Qr");
        assert_eq!(e.markdown(), "# T\n\n**Qr**\n\nbelow words\n");
    }

    /// The finding: an input method composing over a chosen picture put
    /// its words at the start of the paragraph after it. Room is made in
    /// its place first, the words composed there, bold if Bold was on;
    /// one Undo brings the picture back.
    #[test]
    fn room_made_over_a_chosen_picture_for_an_input_method() {
        let md = "# T\n\n![](a.png)\n\nBelow.\n";
        let mut e = Editor::open(md);
        e.select(1, 0, 1, 1);
        assert!(e.style(inline::Kind::Bold));
        assert!(e.make_room());
        assert_eq!(e.lines()[1].text, "");
        assert_eq!(e.caret_lc(), (1, 0, 1, 0));
        keys(&mut e, "あい");
        assert_eq!(e.markdown(), "# T\n\n**あい**\n\nBelow.\n");
        assert!(e.undo());
        assert_eq!(e.markdown(), md, "one step");
        // Nothing chosen: nothing made.
        e.select(2, 1, 2, 1);
        assert!(!e.make_room());
        assert_eq!(e.markdown(), md);
    }

    /// The finding: the Divider button before a divider chose that one,
    /// so the next key took it away. The caret stands past the new one,
    /// and what is typed there is a line between them.
    #[test]
    fn a_divider_before_a_divider_chooses_neither() {
        let mut e = Editor::open("# T\n\nIntro\n\n---\n\nNext\n");
        e.select(1, 5, 1, 5);
        assert!(e.insert_divider());
        assert_eq!(e.markdown(), "# T\n\nIntro\n\n---\n\n---\n\nNext\n");
        let (al, ac, hl, hc) = e.caret_lc();
        assert_eq!((al, ac), (hl, hc), "nothing chosen");
        assert_eq!((hl, hc), (2, 1), "just past the new divider");
        e.input("x");
        assert_eq!(e.markdown(), "# T\n\nIntro\n\n---\n\nx\n\n---\n\nNext\n");
    }

    /// As the desktop's `a_picture_straight_after_the_title_is_typed_over`:
    /// a picture straight after the title, come to by ↓ or → from the
    /// title's end (chosen whole by the editor's arrows, or a bare caret
    /// put on it), is chosen, and typed over as one with a paragraph
    /// above it is: the words a paragraph in its place.
    #[test]
    fn a_picture_straight_after_the_title_is_typed_over() {
        let md = "# Title\n\n![x|left|220](attachments/x.png)\n\n- [ ] Book\n";
        for to in [(1, 0, 1, 1), (1, 1, 1, 0), (1, 0, 1, 0), (1, 1, 1, 1)] {
            let mut e = Editor::open(md);
            assert_eq!(e.lines()[1].text, OBJECT.to_string());
            e.select(0, 5, 0, 5);
            e.select(to.0, to.1, to.2, to.3);
            assert!(page::chosen_atom(&e.doc, e.caret).is_some(), "{to:?}: chosen");
            keys(&mut e, "replaced");
            assert_eq!(e.markdown(), "# Title\n\nreplaced\n\n- [ ] Book\n", "{to:?}");
        }
        // In the body already, a bare caret there is beside it, as ever.
        let mut e = Editor::open(md);
        e.select(2, 4, 2, 4);
        e.select(1, 0, 1, 0);
        assert!(page::chosen_atom(&e.doc, e.caret).is_none());
    }

    /// As the desktop's: lines copied from inside a code block are the
    /// bare lines as text, and code (its language named) as HTML.
    #[test]
    fn code_copied_is_code_in_its_html() {
        let mut e = Editor::open("# T\n\n```rust\nlet a = 1;\nlet b = 2;\n```\n");
        assert_eq!((e.lines()[1].text.as_str(), e.lines()[2].text.as_str()), ("let a = 1;", "let b = 2;"));
        e.select(1, 0, 2, 10);
        assert_eq!(e.copy().as_deref(), Some("let a = 1;\nlet b = 2;"));
        let html = carnet_core::export::html(&e.copy_for_html().unwrap(), |s, _| s.to_string());
        assert_eq!(html, "<pre><code class=\"language-rust\">let a = 1;\nlet b = 2;</code></pre>");
        // In the title: its words.
        e.select(0, 0, 0, 1);
        assert_eq!(e.copy_for_html().as_deref(), Some("T"));
    }

    /// The finding: words with U+202E in, selected, ⌘K, an address and
    /// Enter made no link, and said nothing. A link now, the control kept
    /// in the file, its words drawn without it; the selection still the
    /// same words.
    #[test]
    fn words_with_a_bidi_control_in_are_made_a_link() {
        let mut e = Editor::open("# T\n\nsee abc\u{202E}txt here\n");
        assert_eq!(e.lines()[1].text, "see abc\u{202E}txt here");
        e.select(1, 4, 1, 11);
        assert!(e.set_link(Some("https://example.com")));
        assert_eq!(e.markdown(), "# T\n\nsee [abc\u{202E}txt](https://example.com) here\n");
        assert_eq!(e.lines()[1].text, "see abctxt here");
        assert_eq!(e.caret_lc(), (1, 4, 1, 10));
        assert_eq!(e.link_here().as_deref(), Some("https://example.com"));
    }

    /// Autolinks are links in the editor, a mail's address alone too; a
    /// link's words come without bidi controls, and so does its address
    /// in the link sheet. The note keeps them as written.
    #[test]
    fn autolinks_and_bidi_controls_in_links() {
        let md = "# T\n\nsee <https://example.com> and [abc\u{202E}txt.exe](https://x.com/\u{202E}y) end.\n";
        let mut e = Editor::open(md);
        assert_eq!(e.lines()[1].text, "see https://example.com and abctxt.exe end.");
        e.select(1, 8, 1, 8);
        assert_eq!(e.link_here().as_deref(), Some("https://example.com"));
        e.select(1, 30, 1, 30);
        assert_eq!(e.link_here().as_deref(), Some("https://x.com/y"));
        assert_eq!(e.markdown(), md);
        // A mail's address alone: its words the address, opening a mail.
        let md = "# T\n\nwrite to <me@x.com> today\n";
        let mut e = Editor::open(md);
        assert_eq!(e.lines()[1].text, "write to me@x.com today");
        e.select(1, 12, 1, 12);
        assert_eq!(e.link_to_open().as_deref(), Some("mailto:me@x.com"));
        assert_eq!(e.markdown(), md);
    }

    #[test]
    fn a_tables_cells_are_written_in() {
        let mut e = Editor::open("# T\n\n| Day | Plan |\n| --- | --- |\n| Sat | **Beach** |\n");
        assert_eq!(e.lines()[1].text, OBJECT.to_string());
        let look = e.look(1);
        assert_eq!(look.kind, "table");
        assert_eq!(look.cells[1][1].words, "Beach");
        assert!(look.cells[1][1].runs.iter().any(|r| r.2.bold));
        // Typing in a cell: bold carries on, as in the text.
        assert!(e.cell_select(1, 1, 1, 5, 5));
        e.input("s");
        assert!(e.markdown().contains("| Sat | **Beachs** |"), "{}", e.markdown());
        assert_eq!(e.cell_caret(), Some((1, 1, 1, 6, 6)));
        // Enter: down the column, though nothing changed.
        e.cell_select(1, 0, 1, 0, 0);
        assert!(e.enter(false), "moved");
        assert_eq!(e.cell_caret().map(|c| (c.1, c.2)), Some((1, 1)));
        e.cell_select(1, 1, 1, 6, 6);
        // Tab to the next cell (a new row past the last).
        e.tab(false);
        assert_eq!(e.cell_caret().map(|c| (c.1, c.2)), Some((2, 0)));
        e.input("Sun");
        e.select(1, 0, 1, 0);
        e.cell_select(1, 2, 0, 3, 3);
        e.table_op(1);
        assert!(e.look(1).cells[0].len() == 3, "a column added");
        e.table_op(3);
        assert!(e.look(1).cells[0].len() == 2, "and taken out");
        // Sorted by the first column, then the other way.
        e.cell_select(1, 1, 0, 0, 0);
        e.sort_table();
        assert_eq!(e.sorted(), Some(false), "A to Z (already so: nothing moved)");
        assert_eq!(e.look(1).cells[1][0].words, "Sat");
        assert!(e.sort_table());
        assert_eq!(e.sorted(), Some(true));
        assert_eq!(e.look(1).cells[1][0].words, "Sun");
        assert!(e.markdown().contains("| Sun |"), "{}", e.markdown());
        // A new table in an empty paragraph.
        let mut e = Editor::open("# T\n\n");
        e.select(1, 0, 1, 0);
        assert!(e.insert_table());
        assert_eq!(e.cell_caret().map(|c| (c.1, c.2)), Some((0, 0)));
        e.input("a");
        assert!(e.markdown().contains("| a |"), "{}", e.markdown());
    }

    /// A picture in a cell: drawn as one letter of the cell's words, with
    /// what it shows; the caret steps over it, Backspace takes it whole,
    /// and one put in from the toolbar goes into the cell.
    #[test]
    fn a_picture_in_a_cell_is_one_letter() {
        let mut e = Editor::open("# T\n\n| img1 | img2 |\n|---|---|\n| ![|left|260](../attachments/aerial.jpg) é | |\n");
        let look = e.look(1);
        let cell = &look.cells[1][0];
        assert_eq!(cell.words, "! é");
        assert_eq!(cell.pictures, [CellPicture { at: 0, src: "../attachments/aerial.jpg".into(), alt: String::new(), width: Some(260), wiki: false }]);
        assert!(cell.runs.iter().any(|r| (r.0, r.1) == (0, 1) && r.2.picture), "{:?}", cell.runs);
        // After the picture, then Backspace: gone whole.
        assert!(e.cell_select(1, 1, 0, 1, 1));
        assert_eq!(e.cell_caret(), Some((1, 1, 0, 1, 1)));
        assert!(e.backspace(false));
        assert!(e.markdown().contains("|  é |  |"), "{}", e.markdown());
        // One put in from the toolbar while the caret is in a cell.
        assert!(e.cell_select(1, 1, 1, 0, 0));
        assert!(e.insert_picture("../attachments/b c.png"));
        assert!(e.markdown().contains("| ![](<../attachments/b c.png>) |"), "{}", e.markdown());
        assert_eq!(e.cell_caret(), Some((1, 1, 1, 1, 1)), "after it");
        assert_eq!(e.look(1).cells[1][1].pictures.len(), 1);
    }

    /// ← and → in a cell: beside a picture, the page's move, which chooses
    /// it (its letter, and nothing more) before it passes it; any other
    /// move left to the browser. A letter typed over a chosen picture takes
    /// its place; one pasted in is written plainly.
    #[test]
    fn the_arrows_choose_a_cells_picture_first() {
        let mut e = Editor::open("# T\n\n| A | B |\n|---|---|\n| a![](p.png)b | |\n\nAfter\n");
        // From after it: ← chooses it, ← again is before it.
        assert!(e.cell_select(1, 1, 0, 2, 2));
        assert!(e.cell_arrow(false));
        assert_eq!(e.cell_caret(), Some((1, 1, 0, 2, 1)), "chosen");
        assert!(e.cell_arrow(false));
        assert_eq!(e.cell_caret(), Some((1, 1, 0, 1, 1)), "before it");
        // Further on: the browser's.
        assert!(!e.cell_arrow(false));
        assert_eq!(e.cell_caret(), Some((1, 1, 0, 1, 1)), "as it was");
        // From before it: → chooses it, → again is after it.
        assert!(e.cell_arrow(true));
        assert_eq!(e.cell_caret(), Some((1, 1, 0, 1, 2)), "chosen");
        assert!(e.cell_arrow(true));
        assert_eq!(e.cell_caret(), Some((1, 1, 0, 2, 2)), "after it");
        assert!(!e.cell_arrow(true));
        // Out of the cell: not this.
        assert!(e.cell_select(1, 1, 1, 0, 0));
        assert!(!e.cell_arrow(true) && !e.cell_arrow(false));
        // Chosen, and a letter typed: in its place.
        assert!(e.cell_select(1, 1, 0, 1, 1));
        assert!(e.cell_arrow(true));
        assert!(e.input("z"));
        assert!(e.markdown().contains("| azb |"), "{}", e.markdown());
        // Pasted in: plainly, its alt kept.
        assert!(e.cell_select(1, 1, 1, 0, 0));
        assert!(e.paste("![sea|right|160](q.png)"));
        assert!(e.markdown().contains("| azb | ![sea](q.png) |"), "{}", e.markdown());
    }

    /// As on the desktop: a line of an exported paragraph (Apple Notes'
    /// lines end in line breaks) turned alone, the rest of the file as it
    /// was; a selection over three lines, three items.
    #[test]
    fn a_line_of_an_exported_paragraph_is_turned_alone() {
        const EXPORT: &str = include_str!("../../carnet-core/tests/fixtures/apple-notes.md");
        let mut e = Editor::open(EXPORT);
        assert_eq!(e.markdown(), EXPORT);
        assert_eq!(e.lines()[1].text, "путь: жильё", "the line break's spaces are no words");
        e.select(1, 3, 1, 3);
        e.turn(page::TURN_BULLET);
        assert_eq!(e.markdown(), EXPORT.replacen("**путь:** жильё   \nhostel / ryokan  \n", "- **путь:** жильё\n\nhostel / ryokan  \n", 1));
        assert!(e.turned_to(page::TURN_BULLET));
        assert_eq!(e.caret_lc(), (1, 3, 1, 3));
        assert!(e.undo());
        assert_eq!(e.markdown(), EXPORT);
        let n = e.lines().iter().position(|l| l.text == "полка для книг в коридоре").unwrap();
        e.select(n, 2, n + 2, 4);
        e.turn(page::TURN_TODO);
        assert_eq!(
            e.markdown(),
            EXPORT.replacen(
                "мысли  \nполка для книг в коридоре  \npicture rail  \nskylight window  \npocket",
                "мысли  \n\n- [ ] полка для книг в коридоре\n- [ ] picture rail\n- [ ] skylight window\n\npocket",
                1
            )
        );
    }

    /// As on the desktop: a line broken as Markdown's hard break (two
    /// spaces, a backslash) ends at its last letter; typed at, it keeps
    /// its break; Backspace at the next line's start takes the break and
    /// its marks; a line turned below leaves none showing above.
    #[test]
    fn a_hard_line_break_shows_nothing_of_its_marks() {
        for mark in ["  ", "\\"] {
            let md = format!("# T\n\nstay{mark}\napart\n");
            let mut e = Editor::open(&md);
            assert_eq!(e.markdown(), md, "as it was read");
            assert_eq!((e.lines()[1].text.as_str(), e.lines()[2].text.as_str()), ("stay", "apart"));
            type_at(&mut e, 1, 4, "s");
            assert_eq!(e.markdown(), format!("# T\n\nstays{mark}\napart\n"));
            assert_eq!(e.caret_lc(), (1, 5, 1, 5));
            e.select(2, 0, 2, 0);
            assert!(e.backspace(false));
            assert_eq!(e.markdown(), "# T\n\nstaysapart\n");
            assert_eq!(e.caret_lc(), (1, 5, 1, 5));
            assert!(e.undo());
            e.select(2, 2, 2, 2);
            e.turn(page::TURN_BULLET);
            assert_eq!(e.lines()[1].text, "stays");
            let above = if mark == "  " { "stays  " } else { "stays" };
            assert_eq!(e.markdown(), format!("# T\n\n{above}\n\n- apart\n"));
        }
    }

    /// As on the desktop: a selection that only starts in bold isn't
    /// bold to the toolbar; one all bold is.
    #[test]
    fn a_style_is_on_for_a_selection_only_when_all_of_it_has_it() {
        let mut e = Editor::open("# T\n\n**bold** plain ==lit==\n");
        e.select(1, 0, 1, 10);
        assert!(!e.style_here().bold);
        e.select(1, 0, 1, 4);
        assert!(e.style_here().bold);
        e.select(1, 11, 1, 14);
        assert_ne!(e.style_here().mark, 0);
        e.select(1, 6, 1, 14);
        assert_eq!(e.style_here().mark, 0);
    }

    /// As on the desktop: pictures one after another on a line are a row
    /// of pictures, not words with marks showing.
    #[test]
    fn pictures_on_one_line_are_pictures() {
        let e = Editor::open("# T\n\n![a|left|200](x.png)![b|left|200](y.png)\n\nWords\n");
        let pics: Vec<usize> = (0..e.lines().len()).filter(|&n| e.look(n).kind == "image").collect();
        assert_eq!(pics.len(), 2, "{:?}", (0..e.lines().len()).map(|n| e.look(n).kind).collect::<Vec<_>>());
        assert!(!e.text().contains('!'), "{}", e.text());
    }

    #[test]
    fn turned_and_back_and_undone() {
        let mut e = Editor::open("# T\n\nmilk\n");
        e.select(1, 2, 1, 2);
        e.turn(page::TURN_TODO);
        assert!(e.markdown().contains("- [ ] milk"));
        assert!(e.turned_to(page::TURN_TODO));
        e.turn(page::TURN_TODO);
        assert!(e.markdown().contains("\nmilk"));
        assert!(e.undo());
        assert!(e.markdown().contains("- [ ] milk"));
        assert!(e.redo());
        assert!(!e.markdown().contains("- [ ]"));
    }

    #[test]
    fn utf16_columns_and_a_line_break_in_a_paragraph() {
        let mut e = Editor::open("# T\n\nnaïve 😀 **bold**\n");
        assert_eq!(e.lines()[1].text, "naïve 😀 bold");
        // After the emoji (two UTF-16 units): column 8.
        type_at(&mut e, 1, 8, "!");
        assert!(e.markdown().contains("naïve 😀! **bold**"), "{}", e.markdown());
        e.select(1, 5, 1, 5);
        e.enter(true);
        assert_eq!(e.lines()[1].text, "naïve");
        assert_eq!(e.lines()[2].text, " 😀! bold");
        assert_eq!(e.look(2).kind, "text");
        assert!(!e.look(2).first);
    }
}

