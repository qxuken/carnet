//! The page: the note's title and meta line, and its body — every block
//! drawn by the app as wrapped text, edited through one key sink.
//!
//! Each slot is a `line` row (kui's word for a line of a custom editor): it
//! names the text to a screen reader, carries the caret for the IME and the
//! blink, and is what a press's `line`/`byte` point into. The caret is a
//! 2px bar placed where `caret_rect` says the last layout put that byte;
//! text that changed since is not measured — its caret waits one frame.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use kui_native::{KeyPress, Span};

use super::*;
use crate::icons::{self, Glyph};
use crate::keys;
use carnet_core::inline;
use carnet_core::doc::Float;
use crate::page::Coverage;
use crate::slash;

/// How far apart pictures side by side are, their blocks' insets included:
/// their rings and handles never meet.
const PICTURE_GAP: f32 = 28.0;
/// The least room beside a picture worth running words in.
const BESIDE_MIN: f32 = 180.0;
/// How strongly a picture that's the user's own file, not Carnet's, is
/// drawn: faint, so it reads as apart from the note's own.
const THEIRS_OPACITY: f32 = 0.45;
/// What the card in place of the user's own picture says when it isn't on
/// this device (another device's file, which doesn't sync): the web's words.
pub(super) const ELSEWHERE: &str = "Not on this device";
/// What stands in for a picture of Carnet's that sync is bringing here
/// (another device's, not come yet): the web's words.
pub(super) const DOWNLOADING: &str = "Downloading\u{2026}";
/// A picture's height for its width while it's on its way, as the web's
/// `.picture-card.coming`: 4:3.
pub(super) const COMING_SHAPE: f32 = 0.75;
/// How long the writing pauses, in seconds, before an empty line says
/// what can go on it.
const HINT_AFTER: f64 = 1.2;
/// On a typewriter, where the line being written is held: this far down
/// the page's height.
const TYPEWRITER_AT: f32 = 0.45;

/// What every block of a page shares as it is drawn: the selection, the
/// `/` menu, the running list numbers, the body's type.
struct Cx {
    cov: Coverage,
    slash: Option<(u64, Vec<usize>)>,
    numbers: Vec<u32>,
    body: TextStyle,
    lone: bool,
    /// Focus: the block being written, while the rest steps back.
    focus: Option<u64>,
}

fn hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl Carnet {
    pub(super) fn page(&mut self, ui: &mut Ui<'_>, l: &Look, main_w: f32) {
        let t = l.t;
        let created = self.settings.dates_created.then(|| self.date_created());
        let note = &self.store.notes[self.current];
        let (note_id, modified, rev) = (note.id, note.modified, note.rev);
        if self.words.0 != note_id || self.words.1 != rev {
            self.words = (note_id, rev, note.doc.word_count());
            self.todos = note.doc.todos();
        }
        let words = self.words.2;
        // Calm on purpose: saving is invisible, as it should be when it
        // always works; only a failure says anything.
        // Within a day, how long ago; older, the day.
        let now = SystemTime::now();
        let when = match created {
            Some(at) => format!("Created {}", super::chrome::when_words(now, at)),
            None if now.duration_since(modified).unwrap_or_default().as_secs() < 86_400 => ago(now, modified),
            None => format!("Edited {}", super::chrome::when_words(now, modified)),
        };
        let mut meta = format!(
            "{when}  ·  {} {}",
            words,
            if words == 1 { "word" } else { "words" }
        );
        if let Some((done, all)) = self.todos {
            meta.push_str(&format!("  ·  {done} of {all} done"));
        }
        let error = self.error.clone();
        let notice = self.notice.as_ref().map(|n| (n.text.clone(), n.undo.is_some()));
        // Reading a deleted note: when it went, and the ways out.
        let peek = self.peek.as_ref().map(|p| (p.what.clone(), p.armed, p.is_folder));
        // Reading a version: the line says which, and offers it back.
        let history = self.versions.as_ref().map(|v| {
            let r = &v.list[v.shown];
            let when = if v.shown == 0 { "The note as it is".to_string() } else { format!("The version from {}", super::revision_ago(r)) };
            let what = format!("{when}  ·  {}", super::lower_first(&self.version_label(r)));
            (what, v.shown != 0, v.against_now && v.shown != 0)
        });
        // A narrow window keeps less margin, so the text keeps its width.
        let pad = if main_w < 640.0 { 18.0 } else { 48.0 };
        // A notice is one line: one too long for the room beside the meta
        // line (and its Undo) is cut after a word, "…" after it, its whole
        // words its tooltip and its name.
        let notice = notice.map(|(text, can_undo)| {
            let small = TextStyle::new(Type::SMALL.size);
            let mut room = main_w.min(l.page_w) - 2.0 * pad - 16.0 - 6.0 - ui.measure_text(&meta, &small, None).width - 10.0;
            if can_undo {
                room -= ui.measure_text("Undo", &small, None).width + 16.0 + 10.0;
            }
            let cut = fit_words(&text, room, |s| ui.measure_text(s, &small, None).width);
            (text, can_undo, cut)
        });
        // The text's own width: the page's, less its padding, a block's
        // inset and the scrollbar. Pictures are sized to it.
        self.text_w = (main_w.min(l.page_w) - 2.0 * pad - 16.0 - 6.0).max(80.0);
        // Focus: the title and the meta line step back with the blocks.
        let dim_head = self.focus_on() && self.in_flow();
        let scroll_key = ui.child_key_indexed(note_id);
        self.page_rect = ui.layout_of(scroll_key);
        let tail = match self.page_rect {
            Some(r) if self.typewriter_on() => (r.h * (1.0 - TYPEWRITER_AT)).max(240.0),
            _ => 240.0,
        };
        // Keyed by the note: each note keeps its own scroll position.
        ui.with_indexed(
            note_id,
            NodeSpec::column()
                .grow_width()
                .grow_height()
                .scroll_y()
                .scrollbar_width(5.0)
                // What follows the caret glides there; the wheel and the
                // thumb still land at once.
                .transition(150.0)
                .cross_align(Align::Center)
                // The page's pointer, anywhere on it, margins included; its
                // keys are the body's, below, which Tab reaches after the
                // title, as it reads.
                .on_drag(Msg::PageDrag)
                .on_context_menu(Msg::Page)
                // Where it is: a dragged file's drop line, and the edges
                // that scroll it.
                .on_layout(Value::Null),
            |ui| {
                ui.with(
                    NodeSpec::column()
                        .grow_width()
                        .max_width(l.page_w)
                        .padding(Edges { l: pad, r: pad, t: 30.0, b: 0.0 }),
                    |ui| {
                        // Title and meta sit 8px in, level with the text of
                        // the blocks inside their wash-ready rows.
                        // Selectable: the words, the error and the notice
                        // copy like any text.
                        let line = NodeSpec::row()
                            .grow_width()
                            .pad_xy(8.0, 0.0)
                            .gap(10.0)
                            .cross_align(Align::Center)
                            .selectable()
                            .clip();
                        // A version's line wraps: which version, then what
                        // can be done with it, each going down a line
                        // whole when the page has no room for both.
                        let line = if history.is_some() { line.wrap().cross_gap(4.0) } else { line };
                        let mut notice_keys = Vec::new();
                        ui.with(
                            step_back(line, self.focus_on(), dim_head),
                            |ui| {
                                if let Some((what, armed, is_folder)) = &peek {
                                    self.peek_line(ui, l, what, *armed, *is_folder);
                                    return;
                                }
                                if let Some((what, can_restore, against_now)) = &history {
                                    self.history_line(ui, l, what, *can_restore, *against_now);
                                    return;
                                }
                                ui.text(&meta, TextStyle::new(Type::SMALL.size).color(t.muted).nowrap());
                                if let Some(e) = &error {
                                    // Read out the moment it appears.
                                    ui.text_in(
                                        NodeSpec::row().live(kui_native::Live::Assertive),
                                        e,
                                        TextStyle::new(Type::SMALL.size).color(t.danger).nowrap(),
                                    );
                                } else if let Some((text, can_undo, cut)) = &notice {
                                    // Pointed at, it stays (see `view`).
                                    let spec = NodeSpec::row()
                                        .enter(kui_native::Enter::from(-6.0, 0.0).opacity(0.0))
                                        .transition(220.0)
                                        .hoverable();
                                    let spec = if cut.is_some() { spec.role(Role::StaticText).label(text.as_str()).tooltip(text) } else { spec };
                                    notice_keys.push(ui.text_in_keyed(
                                        "notice",
                                        spec,
                                        cut.as_deref().unwrap_or(text),
                                        TextStyle::new(Type::SMALL.size).color(t.fg).nowrap(),
                                    ));
                                    if *can_undo {
                                        notice_keys.push(ui.text_in_keyed(
                                            "undo",
                                            NodeSpec::row()
                                                .pad_xy(8.0, 2.0)
                                                .radius(6.0)
                                                .hover_bg(t.accent_soft)
                                                .on_click(Msg::Undo)
                                                .label("Undo")
                                                .tooltip(&keys::hint("Undo", keys::UNDO)),
                                            "Undo",
                                            TextStyle::new(Type::SMALL.size).color(t.focus_ring).nowrap(),
                                        ));
                                    }
                                }
                            },
                        );
                        self.notice_keys = notice_keys;
                        ui.leaf(NodeSpec::row().height(l.px(6.0)));
                        self.title(ui, l, dim_head);
                        ui.leaf(NodeSpec::row().height(l.px(18.0)));
                        // The body is one editor: its keys, its text. Its own
                        // container too: blocks are keyed by id, and an id
                        // must not meet a sibling's auto key (its index).
                        self.page_key = Some(ui.child_key("body"));
                        let body = NodeSpec::column()
                            .grow_width()
                            .on_key(Msg::Page)
                            // A press in the text is the editor's own,
                            // as kui sends it; the scroller above takes
                            // one beside it.
                            .on_drag(Msg::PageDrag)
                            .on_context_menu(Msg::Page)
                            .role(Role::MultilineTextInput)
                            .label("Note");
                        // Over a link a press would open, a hand.
                        let body = if self.link_hand(ui) { body.cursor(kui_native::CursorShape::Pointer) } else { body };
                        ui.with_keyed("body", body, |ui| self.blocks(ui, l));
                        // Room below the last block — on a typewriter, enough
                        // for the last line to come up to the middle. A
                        // click here is the page's (`Msg::PageDrag`): it
                        // puts the caret at the end.
                        ui.leaf_keyed("tail", NodeSpec::row().grow_width().height(tail));
                    },
                );
            },
        );
        if std::mem::take(&mut self.focus_page)
            && let Some(k) = self.page_key
        {
            ui.focus(k);
            self.editing = true;
        }
        self.scroll_key = Some(scroll_key);
        self.drag_scroll(ui, scroll_key);
        if std::mem::take(&mut self.reveal) {
            self.follow_caret(ui, scroll_key);
        }
    }

    /// Scrolls the page to the caret. While words are going down it keeps
    /// room under the line being written, so it is never at the window's
    /// foot (on a typewriter, holds it near the middle); otherwise it only
    /// brings it into sight, and a click never scrolls the page under the
    /// pointer. The caret is read where the last layout drew it: text that
    /// changed since is measured on the next frame, and scrolled to then.
    fn follow_caret(&mut self, ui: &mut Ui<'_>, page_key: Key) {
        let slot = self.caret.head.slot;
        let Some(&k) = self.slot_keys.get(&slot) else { return };
        if !self.caret_fresh && self.reveal_wait < 3 {
            self.reveal_wait += 1;
            self.reveal = true;
            ui.request_frame();
            return;
        }
        self.reveal_wait = 0;
        let head = self.caret.head.byte.min(page::text(self.doc(), slot).len());
        let r = match self.cell_pieces.contains_key(&slot) {
            true => self.rect_at(ui.core(), slot, head),
            false => ui.caret_rect(k, self.drawn_byte(slot, head)),
        };
        let (Some(g), Some(r)) = (ui.scroll_geometry(page_key), r) else {
            ui.reveal(k);
            return;
        };
        let (top, h, off) = (g.rect.y, g.rect.h, g.offset.y);
        let (bottom, line) = (r.y + r.h, r.y + r.h / 2.0);
        let target = if self.typing && self.typewriter_on() {
            off + line - (top + h * TYPEWRITER_AT)
        } else {
            // Room kept above and below the caret's line: a breath while
            // writing, the line itself otherwise.
            let (above, below) = if self.typing { (32.0, (h * 0.25).clamp(48.0, 220.0)) } else { (8.0, 8.0) };
            if bottom > top + h - below {
                off + bottom - (top + h - below)
            } else if r.y < top + above {
                off - (top + above - r.y)
            } else {
                off
            }
        };
        let target = target.clamp(0.0, g.max_offset.y.max(0.0));
        if (target - off).abs() > 0.5 {
            ui.set_scroll(page_key, Vec2::new(g.offset.x, target));
        }
    }

    /// The meta line while a version is read: which one, what its marks
    /// mean, then what can be done with it — the actions are words in the
    /// line, as Undo is. Three parts that the line wraps whole: where they
    /// don't all fit, the actions go down a line together (and wrap among
    /// themselves only when even that line is too narrow for them), and
    /// the words break between words, never cut short.
    fn history_line(&self, ui: &mut Ui<'_>, l: &Look, what: &str, can_restore: bool, against_now: bool) {
        let t = l.t;
        ui.text_in_keyed("version-what", NodeSpec::row(), what, TextStyle::new(Type::SMALL.size).color(t.muted));
        // Against now, the marks mean something else than the list's line
        // says: their key, drawn as they are.
        if against_now {
            use carnet_core::compare::{ADDED, REMOVED};
            let (dark, small) = (l.c.dark, Type::SMALL.size);
            let (came, went) = ("comes back", "goes");
            ui.with_keyed("version-marks", NodeSpec::row().gap(10.0).cross_align(Align::Center), |ui| {
                ui.text_in(
                    NodeSpec::row().pad_xy(4.0, 0.0).radius(3.0).bg(crate::design::tint::wash(ADDED, dark, t.bg)),
                    came,
                    TextStyle::new(small).color(t.fg).nowrap(),
                );
                ui.text(went, TextStyle::new(small).color(crate::design::tint::ink(REMOVED, dark)).strikethrough().nowrap());
            });
        }
        let action = |ui: &mut Ui<'_>, key: &str, label: &str, msg: Msg, tip: &str| {
            ui.text_in_keyed(
                key,
                NodeSpec::row().pad_xy(8.0, 2.0).radius(6.0).hover_bg(t.accent_soft).on_click(msg).label(label).tooltip(tip),
                label,
                TextStyle::new(Type::SMALL.size).color(t.focus_ring).nowrap(),
            );
        };
        ui.with_keyed("version-actions", NodeSpec::row().gap(10.0).wrap().cross_gap(4.0).cross_align(Align::Center), |ui| {
            if can_restore {
                action(ui, "restore", "Restore", Msg::RestoreVersion, "The note becomes this version; Undo brings it back");
                action(ui, "restore-copy", "Restore as a copy", Msg::RestoreVersionCopy, "A new note with this version's text");
                let (label, tip) = if against_now {
                    ("Compare with the one before", "Mark what this version changed")
                } else {
                    ("Compare with now", "Mark what restoring it would bring back and take away")
                };
                action(ui, "compare", label, Msg::CompareWithNow, tip);
            }
            action(ui, "close-history", "Done", Msg::CloseHistory, "Back to the note (Esc)");
        });
    }

    /// The meta line while a deleted note (or folder) is read: when it
    /// went and where from, then Restore and Delete forever — which asks
    /// to be sure first, in the line itself.
    fn peek_line(&self, ui: &mut Ui<'_>, l: &Look, what: &str, armed: bool, is_folder: bool) {
        let t = l.t;
        let action = |ui: &mut Ui<'_>, key: &str, label: &str, msg: Msg, tip: &str, ink: kui_native::Color| {
            ui.text_in_keyed(
                key,
                NodeSpec::row().pad_xy(8.0, 2.0).radius(6.0).hover_bg(t.accent_soft).on_click(msg).label(label).tooltip(tip),
                label,
                TextStyle::new(Type::SMALL.size).color(ink).nowrap(),
            );
        };
        if armed {
            let ask = if is_folder {
                "Delete this folder's notes and their history on every device? This can't be undone."
            } else {
                "Delete this note and its history on every device? This can't be undone."
            };
            icons::icon(ui, crate::app::chrome::Icon::Trash, t.danger, 14.0, false);
            ui.text(ask, TextStyle::new(Type::SMALL.size).color(t.fg));
            action(ui, "forever-cancel", "Cancel", Msg::DeleteForeverCancel, "Keep it in Deleted", t.focus_ring);
            action(ui, "forever-yes", "Yes, delete forever", Msg::DeleteForever, "Gone for good, here and on every device", t.danger);
            return;
        }
        icons::icon(ui, crate::app::chrome::Icon::Trash, t.muted, 14.0, false);
        ui.text(what, TextStyle::new(Type::SMALL.size).color(t.muted).nowrap());
        action(ui, "peek-restore", "Restore", Msg::RestorePeek, "Put it back where it was", t.focus_ring);
        action(ui, "forever", "Delete forever", Msg::DeleteForever, "Its history too, on every device", t.danger);
        action(ui, "peek-close", "Done", Msg::CloseHistory, "Back to the note (Esc)", t.focus_ring);
    }

    /// The title: kui's own field, "Untitled" while it is empty; a
    /// version in the history's, or a deleted note's, as text to read.
    fn title(&mut self, ui: &mut Ui<'_>, l: &Look, dim: bool) {
        let t = l.t;
        let title = self.store.notes[self.current].title.clone();
        let style = l.page_type(Type::TITLE).family(l.serif);
        let row = step_back(NodeSpec::row().grow_width().pad_xy(8.0, 0.0), self.focus_on(), dim);
        if let Some(p) = &self.peek {
            let shown = if p.title.trim().is_empty() { "Untitled" } else { p.title.as_str() };
            ui.with(row, |ui| {
                ui.text_in(NodeSpec::row().grow_width().selectable().label("Title"), shown, style.color(t.fg));
            });
            return;
        }
        if let Some(v) = &self.versions {
            let shown = if v.title.trim().is_empty() { "Untitled" } else { v.title.as_str() };
            ui.with(row, |ui| {
                ui.text_in(NodeSpec::row().grow_width().selectable().label("Title"), shown, style.color(t.fg));
            });
            return;
        }
        ui.with(row, |ui| {
            let k = ui.text_edit(
                "title",
                &title,
                &EditOptions {
                    style: style.color(t.fg),
                    wrap: true,
                    placeholder: Some("Untitled".into()),
                    ..Default::default()
                },
                NodeSpec::row().grow_width().label("Title"),
            );
            self.fields.insert(k, Field::Title);
            // kui's field keeps its own words: a title changed elsewhere —
            // by another device, or in the file by another app — is put in
            // it, unless it's being written here.
            if ui.focused() != Some(k) && ui.core().edit_text(k).is_some_and(|now| now != title) {
                ui.set_edit_text(k, &title);
            }
            self.apply_focus(ui, k, Field::Title);
            // Come up to from the note: the caret at the end, then, for ↑,
            // along the last line to the column it left.
            if let Some(column) = self.title_to.take() {
                let core = ui.core();
                core.edit.set_selection(k, (usize::MAX, usize::MAX), (usize::MAX, usize::MAX));
                if let Some(x) = column
                    && let Some(end) = core.edit_caret_rect(k)
                {
                    core.edit_caret_to(k, Vec2::new(x, end.y + end.h * 0.5));
                }
            }
        });
    }

    fn blocks(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let t = l.t;
        let body = l.page_type(Type::BODY).family(l.body).color(t.fg);
        let cov = page::coverage(self.doc(), self.caret);
        // Set again where the caret's line is drawn.
        self.caret_fresh = false;
        // The `/` menu: a new word highlights its first command; a line
        // that is no longer a command forgets the Esc that closed it.
        let slash = self.slash_now();
        match slash::query(self.doc(), self.caret) {
            Some((_, word)) if word != self.slash_word => {
                self.slash_word = word;
                self.slash_sel = 0;
            }
            Some(_) => {}
            None => {
                self.slash_word.clear();
                self.slash_off = None;
            }
        }
        // The rows as last laid out: how tall the blocks beside a picture
        // are.
        let prev: HashMap<u64, Rect> = self.row_rects.iter().copied().collect();
        self.row_rects.clear();
        self.cell_rects.clear();
        self.cell_pieces.clear();
        self.slot_keys.clear();
        self.shown.clear();
        // Pointers, not text: the blocks stay shared with the note.
        let blocks: Vec<Rc<Block>> = self.doc().blocks().to_vec();
        let drop_at = self.drop_index;
        // First letters dropped: of each paragraph that asks for it.
        self.initial = blocks
            .iter()
            .filter(|b| b.initial)
            .filter(|b| matches!(&b.kind, Kind::Text(t) if !t.trim().is_empty()))
            .map(|b| Slot::block(b.id))
            .collect();
        let lone = blocks.len() == 1;
        let focus = (self.focus_on() && self.in_flow()).then_some(self.caret.head.slot.block);
        let mut cx = Cx { cov, slash, numbers: Vec::new(), body, lone, focus };
        let mut bi = 0;
        while bi < blocks.len() {
            if drop_at == Some(bi) {
                drop_line(ui, l);
            }
            // A picture at an edge: the pictures in a row with it, and the
            // blocks after it beside them.
            if let Some((k, end)) = self.float_group(&blocks, bi, &prev) {
                // Who sits beside it is read from the last layout: when
                // that changes, the next frame measures again, until it
                // settles — a few times at most, should it swing.
                let e = self.float_ends.entry(blocks[bi].id).or_insert((usize::MAX, 0));
                if e.0 != end {
                    *e = (end, e.1.saturating_add(1));
                    if e.1 < 6 {
                        ui.request_frame();
                    }
                } else {
                    e.1 = 0;
                }
                self.float_row(ui, l, &blocks, (bi, k, end), drop_at, &mut cx);
                bi = end;
                continue;
            }
            self.block(ui, l, &blocks[bi], &mut cx);
            bi += 1;
        }
        if drop_at.is_some_and(|i| i >= blocks.len()) {
            drop_line(ui, l);
        }
        self.picture_ghost(ui, l);
        // Files carried over a table's cell go into it: the cell ringed.
        if self.pic_drag.is_none()
            && let Some(slot) = self.file_drag.as_ref().and_then(|d| d.at).and_then(|p| self.cell_at(p.x, p.y))
        {
            self.cell_ring(ui, l, slot);
        }
    }

    /// Where a floated picture's row ends, for one at block `i`: its
    /// pictures (`i..k`, side by side), and the blocks that run beside
    /// them (`k..end`) — as many as it takes, by last frame's heights, to
    /// pass the tallest picture's foot. Only words run beside them
    /// (`page::runs_beside`): a picture on a line of its own ends it, and
    /// so do a divider and a table, which start below the pictures at
    /// their full width.
    fn float_group(&self, blocks: &[Rc<Block>], i: usize, prev: &HashMap<u64, Rect>) -> Option<(usize, usize)> {
        let floated = |b: &Block| matches!(b.kind, Kind::Image { float, .. } if float != Float::None);
        if !floated(&blocks[i]) {
            return None;
        }
        let k = (i..blocks.len()).find(|&j| !floated(&blocks[j])).unwrap_or(blocks.len());
        let tall = (i..k)
            .filter_map(|j| prev.get(&blocks[j].id))
            .fold(0.0_f32, |a, r| a.max(r.h));
        // Pictures that leave the words too little room have none beside
        // them: a row of pictures, side by side.
        let wide: f32 = (i..k).filter_map(|j| prev.get(&blocks[j].id)).map(|r| r.w + PICTURE_GAP).sum();
        if self.text_w + 16.0 - wide < BESIDE_MIN {
            return Some((k, k));
        }
        let mut end = k;
        let mut beside = 0.0;
        while end < blocks.len() && beside < tall - 8.0 {
            // A divider ends a part of the note, and a table squeezed
            // beside them would wrap every cell: each goes across the
            // page, under the pictures, not in a column beside them.
            if !page::runs_beside(&blocks[end].kind) {
                break;
            }
            beside += prev.get(&blocks[end].id).map_or(30.0, |r| r.h);
            end += 1;
        }
        Some((k, end))
    }

    /// A row of pictures at an edge with the blocks `k..end` in a column
    /// beside them.
    fn float_row(
        &mut self,
        ui: &mut Ui<'_>,
        l: &Look,
        blocks: &[Rc<Block>],
        (i, k, end): (usize, usize, usize),
        drop_at: Option<usize>,
        cx: &mut Cx,
    ) {
        let right = matches!(blocks[i].kind, Kind::Image { float: Float::Right, .. });
        self.float_count = k - i;
        self.float_row = blocks[i].id;
        // Its pictures say their heights again as they're drawn: none
        // gone from it is snapped to.
        self.pic_rows.retain(|_, (row, ..)| *row != blocks[i].id);
        let key = format!("float-{}", blocks[i].id);
        // The row's own width as last laid out: what its pictures share,
        // measured, so they never crowd each other however the page's
        // margins come out.
        self.float_room = ui.layout_of(ui.child_key(&key)).map_or(self.text_w + 16.0, |r| r.w);
        ui.with_keyed(&key, NodeSpec::row().grow_width().gap(4.0).cross_align(Align::Start), |ui| {
            for side in [!right, right] {
                if side {
                    ui.with_keyed("pictures", NodeSpec::row().gap(PICTURE_GAP - 16.0), |ui| {
                        for b in &blocks[i..k] {
                            self.block(ui, l, b, cx);
                        }
                    });
                } else if end > k {
                    ui.with_keyed("beside", NodeSpec::column().grow_width().min_width(120.0), |ui| {
                        for (j, b) in blocks.iter().enumerate().take(end).skip(k) {
                            if drop_at == Some(j) {
                                drop_line(ui, l);
                            }
                            self.block(ui, l, b, cx);
                        }
                    });
                }
            }
        });
        self.float_count = 0;
    }

    /// One block, in the row that reports where it was laid out.
    fn block(&mut self, ui: &mut Ui<'_>, l: &Look, b: &Block, cx: &mut Cx) {
        let t = l.t;
        let id = b.id;
        let (body, lone) = (cx.body, cx.lone);
        let (cov, slash, numbers) = (&cx.cov, &cx.slash, &mut cx.numbers);
        if !matches!(b.kind, Kind::Item { .. }) {
            numbers.clear();
        }
        // Every block sits in one row that reports where it was laid
        // out (what a margin drag's `y` is read against) and wears the
        // selection's wash when it is covered whole.
        let row = ui.child_key_indexed(id);
        if let Some(r) = ui.layout_of(row) {
            self.row_rects.push((id, r));
        }
        let whole = cov.whole.contains(&id);
        // The caret alone on an empty line: it says what can go there —
        // once the writing pauses, not on every line Enter makes.
        let mut empty_here = self.editing
            && self.caret.is_empty()
            && self.caret.head.slot == Slot::block(id)
            && b.text_ref().is_some_and(str::is_empty);
        if empty_here && self.typing && ui.now() < self.typed_at + HINT_AFTER {
            empty_here = false;
            ui.request_frame_at(self.typed_at + HINT_AFTER);
        }
        let floated = matches!(b.kind, Kind::Image { float, .. } if float != Float::None);
        let mut wrap = NodeSpec::column()
            .width(if floated { Sizing::Fit } else { Sizing::GROW })
            .pad_xy(8.0, 0.0)
            .radius(radius::TIGHT)
            .on_layout(Value::Null);
        // A picture says it is selected with its own ring, hugging
        // it: a wash the page's width would mostly cover nothing. A
        // divider wears its own, round the ornament.
        if whole && !matches!(b.kind, Kind::Image { .. } | Kind::Divider) {
            wrap = wrap.bg(t.selection);
        } else if self.focus_on() {
            // Eased with focus: no wash is the wash with no alpha.
            wrap = wrap.bg(t.selection.with_alpha(0.0));
        }
        wrap = step_back(wrap, self.focus_on(), cx.focus.is_some_and(|f| f != id));
        ui.with_indexed(id, wrap, |ui| match &b.kind {
            Kind::Text(text) => {
                // An empty note says what it can do, once, quietly.
                let hint = if lone && text.is_empty() {
                    Some("Start writing  —  or type / for blocks")
                } else if empty_here {
                    Some("Type / for blocks")
                } else {
                    None
                };
                // Paragraphs sit 12px apart; list items closer, so a
                // list reads as one thing and prose as several.
                ui.with(NodeSpec::row().grow_width().pad_xy(0.0, l.air(6.0)), |ui| {
                    self.slot_line(ui, l, Slot::block(id), text, body, hint, false, cov);
                    if let Some((_, shown)) = slash.as_ref().filter(|(b, _)| *b == id) {
                        self.slash_menu(ui, l, id, shown);
                    }
                });
            }
            // A quotation: set in from a rule down its left edge, in
            // the serif, a step quieter than the words around it.
            Kind::Quote(text) => {
                ui.with(NodeSpec::row().grow_width().pad_xy(0.0, l.air(8.0)), |ui| {
                    ui.with(
                        NodeSpec::row()
                            .grow_width()
                            .gap(l.px(14.0))
                            .label("Quote"),
                        |ui| {
                            let rule = l.px(3.0).max(2.0);
                            ui.leaf(NodeSpec::row().width(rule).grow_height().radius(rule / 2.0).bg(t.border_strong).role(Role::None));
                            let hint = empty_here.then_some("Quote");
                            let ink = body.family(l.serif).color(t.muted);
                            self.slot_line(ui, l, Slot::block(id), text, ink, hint, false, cov);
                        },
                    );
                    // The `/` menu, as under a paragraph.
                    if let Some((_, shown)) = slash.as_ref().filter(|(b, _)| *b == id) {
                        self.slash_menu(ui, l, id, shown);
                    }
                });
            }
            Kind::Heading { level, text } => {
                // A serif scale of about 1.25: size and face are what
                // tell a heading from a paragraph.
                let (style, top) = match level {
                    1 => (l.page_type(Type::H1).family(l.serif), 28.0),
                    2 => (l.page_type(Type::H2).family(l.serif), 22.0),
                    _ => (l.page_type(Type::H3).family(l.serif), 16.0),
                };
                ui.with(
                    NodeSpec::row()
                        .grow_width()
                        .padding(Edges { l: 0.0, r: 0.0, t: l.air(top), b: l.air(6.0) }),
                    |ui| {
                        let hint = empty_here.then_some("Heading");
                        self.slot_line(ui, l, Slot::block(id), text, style.color(t.fg), hint, false, cov);
                    },
                );
            }
            Kind::Item {
                style,
                indent,
                done,
                num,
                text,
            } => {
                let depth = *indent as usize;
                numbers.truncate(depth + 1);
                numbers.resize(depth + 1, 0);
                if *style == ListStyle::Number {
                    // A run starts at the number it was written with
                    // ("2." after bullets that broke the list), as
                    // Markdown has it; then counts on.
                    numbers[depth] = if numbers[depth] == 0 && *num > 0 { *num } else { numbers[depth] + 1 };
                } else {
                    numbers[depth] = 0;
                }
                let n = numbers[depth];
                ui.with(
                    NodeSpec::row()
                        .grow_width()
                        .gap(l.px(8.0))
                        .pad_xy(0.0, l.air(2.0))
                        .cross_align(Align::Start),
                    |ui| {
                        // A child's marker sits under its parent's text:
                        // marker (22) plus gap (8) per level, the row's
                        // own gap already one step of it — at 16px.
                        if depth > 0 {
                            ui.leaf(NodeSpec::row().width(depth as f32 * (l.px(22.0) + l.px(8.0)) - l.px(8.0)));
                        }
                        marker(ui, l, id, *style, *done, n, depth, text);
                        let ink = if *done { body.color(t.muted) } else { body };
                        let hint = empty_here.then_some(if *style == ListStyle::Todo { "To-do" } else { "List" });
                        self.slot_line(ui, l, Slot::block(id), text, ink, hint, *done, cov);
                    },
                );
            }
            Kind::Divider => {
                // Drawn as the settings say (three dots, a line, stars, a
                // flourish). Chosen — by the arrows, a click, or the caret
                // on it — one way: the ornament in the accent on the
                // selection's wash, as wide as the text and a band round
                // the ornament, not its whole tall row; so Backspace or
                // Delete is seen to have a target.
                let here = self.editing && self.caret.head.slot == Slot::block(id);
                let chosen = whole || here;
                let ink = if chosen { t.accent } else { t.faint };
                let band = l.px(8.0).min(l.air(20.0));
                let mut wash = NodeSpec::row()
                    .grow_width()
                    .pad_xy(0.0, band)
                    .radius(radius::CONTROL)
                    .center()
                    .hover_bg(t.hover);
                if chosen {
                    wash = wash.bg(t.selection);
                }
                ui.with(
                    NodeSpec::row()
                        .grow_width()
                        .pad_xy(0.0, l.air(20.0) - band)
                        .on_click(Msg::Select { id })
                        .label("Divider"),
                    |ui| {
                        ui.with(wash, |ui| icons::ornament(ui, self.settings.divider, ink, l.k));
                    },
                );
            }
            Kind::Image { src, alt, width, float, wiki } => self.picture(ui, l, id, src, *wiki, alt, *width, *float, whole),
            Kind::Table(table) => self.table(ui, l, id, table, body, cov),
            // Code and front matter: kept as written and edited as plain
            // text on a well of their own — only the lines between their
            // fences drawn (`line_part`), as on the web: the fences, and
            // the language after the first, are Markdown.
            Kind::Raw(text) => {
                let front = text.starts_with("---\n");
                let style = l.page_type(Type::CODE)
                    .family(l.code)
                    .color(if front { t.muted } else { t.fg });
                ui.with(NodeSpec::column().grow_width().pad_xy(0.0, l.air(14.0)), |ui| {
                    ui.with(
                        NodeSpec::column()
                            .grow_width()
                            .pad_xy(l.px(16.0), l.px(12.0))
                            .radius(radius::CONTROL)
                            .bg(t.sunken),
                        |ui| {
                            self.slot_line(ui, l, Slot::block(id), text, style, None, false, cov);
                        },
                    );
                });
            }
        });
    }

    /// A picture on its own line: its own width, or the one the reader
    /// gave it, the text's width at most; the height follows. At an edge,
    /// under half the text's width unless given more, and its share of the
    /// row with the pictures beside it at most. Until it is decoded, a
    /// quiet well of exactly its size holds its place. Selected, a ring in
    /// the accent, a handle on each side and one at a corner that resize it.
    #[allow(clippy::too_many_arguments)]
    fn picture(&mut self, ui: &mut Ui<'_>, l: &Look, id: u64, src: &str, wiki: bool, alt: &str, width: Option<u32>, float: Float, whole: bool) {
        let t = l.t;
        let here = self.editing && self.caret.head.slot == Slot::block(id);
        let selected = here || whole;
        let folder = self.page_folder();
        let file = self.store.picture_file(&folder, src, wiki);
        let entry = file.as_ref().map(|p| self.images.get(p));
        // The user's own file, not Carnet's (store_owned.rs): drawn faint,
        // with a mark, and selected, a line saying so with ways in.
        let theirs = file.as_deref().is_some_and(|p| !self.store.owns(p));
        let label = if alt.is_empty() { "Picture".to_string() } else { format!("Picture: {alt}") };
        let floated = float != Float::None;
        let n = self.float_count.max(1) as f32;
        let max_w = if floated {
            // Its share of the row: each picture's block is 16 wider than
            // it, and pictures sit PICTURE_GAP apart.
            ((self.float_room - 16.0 - PICTURE_GAP * (n - 1.0)) / n - 2.0).floor().max(48.0)
        } else {
            (self.text_w - 10.0).max(48.0)
        };
        // Its shape, from the pixels or the file's header.
        let natural = match &entry {
            Some(images::Entry::Ready { w, h, .. }) | Some(images::Entry::Loading { w, h }) if *w > 0 && *h > 0 => {
                Some((*w as f32, *h as f32))
            }
            _ => None,
        };
        let resizing = self.resizing.filter(|r| r.id == id);
        ui.with(
            NodeSpec::row()
                .width(if floated { Sizing::Fit } else { Sizing::GROW })
                .pad_xy(0.0, l.air(14.0))
                // A press selects it; a drag carries it between lines, or
                // beside another picture.
                .on_drag(Msg::PictureDrag { id })
                .role(Role::Image)
                .label(label),
            |ui| {
                // The ring is always there, clear when not wanted, so the
                // picture never moves when it is selected.
                let ring = if selected { t.accent } else { Color::TRANSPARENT };
                let Some((nw, nh)) = natural else {
                    // Nothing to show: a small card saying why, as wide as
                    // its words, at an edge too — never a well of the
                    // page's width, nor a sliver of an edge's (but for
                    // another device's own picture, below). No text in
                    // it to select: a press selects it as it would the
                    // picture, to be moved or deleted.
                    let (glyph, why) = self.unshown_why(entry.as_ref(), file.as_deref(), theirs, src, wiki);
                    let path = store::decode_src(src);
                    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                    // Another device's own picture, or one on its way here,
                    // stands in the box the picture takes there, as the web
                    // draws it: the width the note gives it, or the text's
                    // (under half of it at an edge), and a picture's height
                    // at least. One on its way is as tall as a picture of
                    // that width (4:3, a photo's shape: nothing here says
                    // its own), so the page doesn't move when it comes.
                    let elsewhere = why == ELSEWHERE || why == DOWNLOADING;
                    let box_w = width.map_or(if floated { (self.text_w * 0.45).max(BESIDE_MIN) } else { max_w }, |w| w as f32).clamp(48.0, max_w);
                    let box_h = if why == DOWNLOADING { (box_w * COMING_SHAPE).max(120.0) } else { 120.0 };
                    let card = if elsewhere {
                        NodeSpec::row().min_width(box_w).max_width(box_w).min_height(box_h).center()
                    } else {
                        NodeSpec::row().width(Sizing::Fit).max_width(max_w).cross_align(Align::Center)
                    };
                    ui.with(card.pad_xy(12.0, 10.0).gap(10.0).radius(radius::CARD).bg(t.sunken).border(2.0, ring), |ui| {
                        icons::icon(ui, glyph, t.muted, 18.0, false);
                        let words_w = ((if elsewhere { box_w } else { max_w }) - 64.0).max(40.0);
                        ui.with(NodeSpec::column().gap(2.0).max_width(words_w), |ui| {
                            if elsewhere {
                                // A line or two, in the box's room.
                                ui.text_in(NodeSpec::row().max_width(words_w), why, TextStyle::new(Type::UI.size).color(t.fg));
                            } else {
                                ui.text(why, TextStyle::new(Type::UI.size).color(t.fg).nowrap());
                            }
                            ui.text_in(NodeSpec::row().max_width(words_w), &name, TextStyle::new(Type::SMALL.size).color(t.muted).ellipsis());
                        });
                    });
                    return;
                };
                let (drawn, fit) = page::picture_width(nw, width, self.text_w, max_w, floated);
                let dw = resizing.map_or(drawn, |r| r.w.clamp(page::PICTURE_MIN.min(nw), max_w));
                let dh = dw * nh / nw;
                self.pic_w.insert(id, (dw, fit, max_w));
                if floated && self.float_count > 1 {
                    self.pic_rows.insert(id, (self.float_row, dh, nh / nw));
                } else {
                    self.pic_rows.remove(&id);
                }
                // Lined up with the one being resized: a thin line in the
                // accent under each, across the gap between them.
                let snap = self.resizing.and_then(|r| r.snap.filter(|_| floated && self.pic_rows.get(&r.id).is_some_and(|(row, ..)| *row == self.float_row)));
                let lined = snap.is_some_and(|h| (h - dh).abs() < 0.75);
                // The picture on the text's own edge; its ring an overlay
                // 5px out, so selecting it moves nothing.
                ui.with(NodeSpec::row().size(dw, dh), |ui| {
                    match &entry {
                        Some(images::Entry::Ready { id: img, .. }) => {
                            ui.image(*img, NodeSpec::row().size(dw, dh).radius(7.0).opacity(if theirs { THEIRS_OPACITY } else { 1.0 }));
                        }
                        // Decoding: the well it will fill, a faint picture
                        // in its middle.
                        _ => {
                            ui.with(
                                NodeSpec::row().size(dw, dh).radius(7.0).bg(t.sunken).center(),
                                |ui| icons::icon(ui, Glyph::Picture, t.faint, 28.0_f32.min(dh * 0.5), false),
                            );
                        }
                    }
                    ui.leaf(
                        NodeSpec::row()
                            .size(dw + 10.0, dh + 10.0)
                            .radius(radius::CARD)
                            .border(2.0, ring)
                            .role(Role::None)
                            // Clipped by the page's scroller like the picture,
                            // not painted over the toolbar when scrolled under it.
                            .float(FloatConfig::parent().offset(-5.0, -5.0).clipped()),
                    );
                    if selected {
                        // The handles: a bar on each side and a dot on the
                        // outer corner, dragged to resize; the shape holds.
                        // A picture at the right edge grows to the left, so
                        // its corner is the bottom-left one.
                        let right = float == Float::Right;
                        let handle = |w: f32, h: f32| {
                            NodeSpec::row()
                                .size(w, h)
                                .radius(w.min(h) / 2.0)
                                .bg(l.card)
                                .border(2.0, t.accent)
                                .label("Resize picture")
                                .tooltip("Drag to resize  \u{b7}  right-click for sizes")
                        };
                        for (key, left) in [("resize-l", true), ("resize-r", false)] {
                            let at = if left { Align::Start } else { Align::End };
                            ui.leaf_keyed(
                                key,
                                handle(8.0, 30.0_f32.min(dh * 0.5))
                                    .float(FloatConfig::parent().inside(at, Align::Center).offset(if left { -9.0 } else { 9.0 }, 0.0).clipped())
                                    .on_drag(Msg::ResizePicture { id, side: left as u64 })
                                    .cursor(kui_native::CursorShape::EwResize),
                            );
                        }
                        let (at, dx, cursor) = if right {
                            (Align::Start, -10.0, kui_native::CursorShape::NeswResize)
                        } else {
                            (Align::End, 10.0, kui_native::CursorShape::NwseResize)
                        };
                        ui.leaf_keyed(
                            "resize",
                            handle(14.0, 14.0)
                                .float(FloatConfig::parent().inside(at, Align::End).offset(dx, 10.0).clipped())
                                .on_drag(Msg::ResizePicture { id, side: right as u64 })
                                .cursor(cursor),
                        );
                    }
                    if theirs {
                        // The ways in only for a picture shown: one that
                        // isn't one stays the user's.
                        let shown = matches!(entry, Some(images::Entry::Ready { .. }));
                        self.theirs_mark(ui, l, shown && selected && self.editing && !self.reading(), float == Float::Right);
                    }
                    if lined {
                        ui.leaf_keyed(
                            "snap",
                            NodeSpec::row()
                                .size(dw + PICTURE_GAP, 1.5)
                                .bg(t.accent)
                                .role(Role::None)
                                .float(FloatConfig::parent().inside(Align::Start, Align::End).offset(-PICTURE_GAP / 2.0, 9.0).clipped()),
                        );
                    }
                    if resizing.is_some() {
                        ui.text_in(
                            NodeSpec::row()
                                .pad_xy(8.0, 3.0)
                                .radius(radius::CONTROL)
                                .bg(l.card)
                                .float(FloatConfig::parent().inside(Align::Start, Align::End).offset(8.0, -8.0).clipped()),
                            &format!("{} \u{00d7} {}", dw.round(), dh.round()),
                            TextStyle::new(Type::SMALL.size).color(t.fg).nowrap(),
                        );
                    }
                });
            },
        );
    }

    /// Why a picture isn't shown, by what reading it said (`None`: it was
    /// never looked for), and the glyph that goes with it: a block's card
    /// and a cell's say the same.
    fn unshown_why(&self, entry: Option<&images::Entry>, file: Option<&std::path::Path>, theirs: bool, src: &str, wiki: bool) -> (Glyph, &'static str) {
        match entry {
            // Carnet's own, not here yet: sync is bringing it from the
            // server (pictures.rs). Missing only once it's given up on.
            Some(images::Entry::Failed(e)) if e == "not found" && !theirs && file.is_some_and(|f| self.sync.picture_coming(&self.store, f)) => {
                (Glyph::Cloud, DOWNLOADING)
            }
            // The user's own picture, not here: with sync on, it's on the
            // device the note was written on, and pictures outside
            // Carnet's attachments don't sync (docs/SYNC.md §4). Said as
            // the web says it.
            Some(images::Entry::Failed(e)) if e == "not found" && theirs && self.sync.is_on() => (Glyph::CloudOff, ELSEWHERE),
            Some(images::Entry::Failed(e)) if e == "not found" => (Glyph::Warning, "Picture not found"),
            Some(images::Entry::Failed(e)) if e == images::HEIC_ELSEWHERE => (Glyph::Warning, "HEIC pictures show on macOS; this one can't show here"),
            Some(images::Entry::Failed(e)) if e == images::TOO_BIG => (Glyph::Warning, "This picture is too big to show"),
            Some(images::Entry::Failed(e)) if e.contains("not supported") || e.contains("Unsupported") => (Glyph::Warning, "Carnet can't show this kind of picture"),
            Some(images::Entry::Failed(_)) => (Glyph::Warning, "This picture couldn't be opened"),
            Some(_) => (Glyph::Picture, "Opening the picture\u{2026}"),
            // A file on another computer, never looked at.
            None if !wiki && !store::is_web(src) => (Glyph::Warning, "Picture not found"),
            None => (Glyph::Picture, "A picture on the web, not loaded"),
        }
    }

    /// On a picture that's the user's own file, not Carnet's: a mark in
    /// its corner saying so, and, selected, a bar below it that says what
    /// that means and offers to take it in, moved or copied. At the right
    /// edge the bar hangs from the picture's right side.
    fn theirs_mark(&self, ui: &mut Ui<'_>, l: &Look, bar: bool, right: bool) {
        let t = l.t;
        ui.with_keyed(
            "theirs",
            NodeSpec::row()
                .pad(4.0)
                .radius(radius::CONTROL)
                .bg(l.card)
                .float(FloatConfig::parent().inside(Align::Start, Align::Start).offset(6.0, 6.0).clipped())
                .label("Your file, not Carnet's")
                .tooltip("Your file, not Carnet's: it isn't synced, and Carnet won't remove it"),
            |ui| icons::icon(ui, Glyph::CloudOff, t.muted, 14.0, false),
        );
        if !bar {
            return;
        }
        let x = if right { Align::End } else { Align::Start };
        // As wide as the words need on two lines, the page's width at most.
        let bar_w = (self.text_w - 10.0).clamp(200.0, 420.0);
        let action = |ui: &mut Ui<'_>, key: &str, label: &str, msg: Msg, tip: &str| {
            ui.text_in_keyed(
                key,
                NodeSpec::row().pad_xy(8.0, 3.0).radius(6.0).hover_bg(t.accent_soft).on_click(msg).label(label).tooltip(tip),
                label,
                TextStyle::new(Type::SMALL.size).color(t.focus_ring).nowrap(),
            );
        };
        ui.with_keyed(
            "theirs-bar",
            NodeSpec::column()
                .min_width(bar_w)
                .max_width(bar_w)
                .pad_xy(12.0, 9.0)
                .gap(6.0)
                .radius(radius::CARD)
                .bg(l.card)
                .border(1.0, t.border)
                .shadow_color(l.shadow)
                .shadow_blur(14.0)
                .shadow_y(3.0)
                .float(FloatConfig::parent().at(x, Align::End).self_at(x, Align::Start).offset(if right { 5.0 } else { -5.0 }, 22.0).clipped()),
            |ui| {
                ui.with(NodeSpec::row().grow_width().gap(8.0).cross_align(Align::Start), |ui| {
                    icons::icon(ui, Glyph::CloudOff, t.muted, 14.0, false);
                    ui.text_in(
                        NodeSpec::row().grow_width().min_width(0.0),
                        "Your file, not Carnet's: it isn't synced, and Carnet won't remove it",
                        TextStyle::new(Type::SMALL.size).color(t.fg),
                    );
                });
                ui.with(NodeSpec::row().gap(2.0).pad_xy(14.0, 0.0), |ui| {
                    action(
                        ui,
                        "move-in",
                        "Move into Carnet",
                        Msg::IntoCarnet { copy: 0 },
                        "Into the attachments folder: every note showing it follows, and it syncs",
                    );
                    action(
                        ui,
                        "copy-in",
                        "Copy into Carnet",
                        Msg::IntoCarnet { copy: 1 },
                        "A copy for this note, which syncs; your file stays where it is",
                    );
                });
            },
        );
    }

    /// The gap between blocks nearest `y` (last frame's layout): the index
    /// of the block a drop there goes before, in the note's order.
    pub(super) fn gap_at(&self, y: f32) -> usize {
        let doc = self.doc();
        doc.blocks()
            .iter()
            .position(|b| self.row_rects.iter().find(|(id, _)| *id == b.id).is_some_and(|(_, r)| y < r.y + r.h / 2.0))
            .unwrap_or(doc.len())
    }

    /// Where a carried picture would land under `at`: among a table
    /// cell's words, where the pointer is in them; beside another picture
    /// when over one's outer third; else between the blocks.
    fn picture_target(&self, core: &Core, what: Carried, at: Vec2) -> Option<page::PicDrop> {
        let page = self.page_rect?;
        let inside = |r: &Rect| at.x >= r.x && at.x <= r.x + r.w && at.y >= r.y && at.y <= r.y + r.h;
        if !inside(&page) {
            return None;
        }
        if let Some(slot) = self.cell_at(at.x, at.y) {
            // Below the words (in a row a taller cell makes tall): after
            // them, at their end.
            let end = self.rect_at(core, slot, page::text(self.doc(), slot).len());
            let below = end.is_some_and(|e| at.y > e.y + e.h);
            let byte = if below { None } else { self.byte_under(core, slot, at) };
            return Some(page::PicDrop::Cell { slot, at: byte.unwrap_or(usize::MAX) });
        }
        for (b, r) in &self.row_rects {
            if what == Carried::Block(*b) || !inside(r) || !matches!(self.doc().get(*b).map(|b| &b.kind), Some(Kind::Image { .. })) {
                continue;
            }
            let fx = (at.x - r.x) / r.w.max(1.0);
            if !(0.3..=0.7).contains(&fx) {
                return Some(page::PicDrop::Beside { id: *b, after: fx > 0.7 });
            }
        }
        Some(page::PicDrop::Before(self.gap_at(at.y)))
    }

    /// A picture pressed: selected; dragged, carried — the drop line shows
    /// where it goes between blocks, a bar beside a picture where the two
    /// will sit side by side, a ring round a table's cell it goes into.
    pub(super) fn picture_drag(&mut self, id: u64, ev: &UiEvent, core: &Core) {
        let p = &ev.payload;
        let num = |k: &str| p.get_float(k).or_else(|| p.get_int(k).map(|v| v as f64)).unwrap_or(0.0) as f32;
        let at = Vec2::new(num("x"), num("y"));
        if p.get_str("phase") == Some("start") {
            self.pic_drag = Some(PicDrag { what: Carried::Block(id), from: at, at, moved: false, target: None });
            self.on_plain(Msg::Select { id });
            return;
        }
        self.carry(core, p.get_str("phase"), at);
    }

    /// A carried picture's pointer moved (`move`) or let go (anything
    /// else): where it would land followed, or it put there, as one step
    /// of the history. A press that never travelled moves nothing.
    fn carry(&mut self, core: &Core, phase: Option<&str>, at: Vec2) {
        let Some(what) = self.pic_drag.as_ref().map(|d| d.what) else { return };
        if phase == Some("move") {
            let target = self.picture_target(core, what, at);
            let Some(d) = self.pic_drag.as_mut() else { return };
            d.at = at;
            d.moved = d.moved || (at.x - d.from.x).abs() + (at.y - d.from.y).abs() > 6.0;
            d.target = if d.moved { target } else { None };
            self.drop_index = match d.target {
                Some(page::PicDrop::Before(i)) => Some(i),
                _ => None,
            };
            return;
        }
        self.drop_index = None;
        let Some(d) = self.pic_drag.take() else { return };
        if let (true, Some(to)) = (d.moved, d.target) {
            self.edit(Step::Other, |doc, c| match what {
                Carried::Block(id) => page::move_picture(doc, c, id, to),
                Carried::Cell(slot, at) => page::move_cell_picture(doc, c, slot, at, to),
            });
            self.reveal = false;
        }
    }

    /// While a picture is carried: it, small, under the pointer, and the
    /// bar where it would sit beside another, or the ring round the cell
    /// it would go into.
    fn picture_ghost(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let Some(d) = self.pic_drag.clone().filter(|d| d.moved) else { return };
        let t = l.t;
        if let Some(page::PicDrop::Beside { id, after }) = d.target
            && let Some((_, r)) = self.row_rects.iter().find(|(b, _)| *b == id)
        {
            let x = if after { r.x + r.w - 2.0 } else { r.x - 2.0 };
            ui.leaf_keyed(
                "picture-bar",
                NodeSpec::row()
                    .size(4.0, (r.h - 16.0).max(8.0))
                    .radius(2.0)
                    .bg(t.accent)
                    .role(Role::None)
                    .float(FloatConfig::viewport().offset(x, r.y + 8.0)),
            );
        }
        if let Some(page::PicDrop::Cell { slot, .. }) = d.target {
            self.cell_ring(ui, l, slot);
        }
        let folder = self.page_folder();
        let (src, wiki) = match d.what {
            Carried::Block(id) => match self.doc().get(id).map(|b| &b.kind) {
                Some(Kind::Image { src, wiki, .. }) => (src.clone(), *wiki),
                _ => return,
            },
            Carried::Cell(slot, at) => match inline::cell_pictures(page::text(self.doc(), slot)).into_iter().find(|p| p.at.0 == at) {
                Some(p) => (p.src, p.wiki),
                None => return,
            },
        };
        if let Some(images::Entry::Ready { id: img, w, h }) = self.store.picture_file(&folder, &src, wiki).map(|p| self.images.get(&p)) {
            let gw = 120.0_f32;
            let gh = gw * h as f32 / (w as f32).max(1.0);
            // Keyed, as the bar and the ring are: the blocks beside it are
            // keyed by their ids, which a counted key could meet.
            let at = FloatConfig::viewport().offset(d.at.x + 12.0, d.at.y + 12.0);
            ui.with_keyed("picture-ghost", NodeSpec::row().size(gw, gh).role(Role::None).float(at), |ui| {
                ui.image(img, NodeSpec::row().size(gw, gh).radius(6.0).opacity(0.75).role(Role::None));
            });
        }
        ui.request_frame();
    }

    /// A table's cell something carried will go into: a ring in the
    /// accent round it, on a wash of it, as the drop line is drawn
    /// between blocks. It takes no room.
    pub(super) fn cell_ring(&self, ui: &mut Ui<'_>, l: &Look, slot: Slot) {
        let Some(r) = self.cell_rects.get(&slot) else { return };
        let t = l.t;
        // Keyed: the page's blocks are keyed by their ids, which a
        // counted key could meet.
        ui.leaf_keyed(
            "cell-ring",
            NodeSpec::row()
                .size(r.w - 2.0, r.h - 2.0)
                .radius(radius::CONTROL)
                .bg(t.accent.with_alpha(0.08))
                .border(2.0, t.accent)
                .role(Role::None)
                .float(FloatConfig::viewport().offset(r.x + 1.0, r.y + 1.0)),
        );
    }

    /// A drag on a picture's handle: the width follows the pointer, and
    /// lands as one step of the history when it is let go. Back at its own
    /// width (or the page's, for one wider), the width is forgotten.
    pub(super) fn resize_drag(&mut self, id: u64, left: bool, ev: &UiEvent) {
        let p = &ev.payload;
        let x = p.get_float("x").or_else(|| p.get_int("x").map(|v| v as f64)).unwrap_or(0.0) as f32;
        let full = (self.text_w - 10.0).max(48.0);
        let (now, fit, max_w) = self.pic_w.get(&id).copied().unwrap_or((full, full, full));
        match p.get_str("phase") {
            Some("start") => {
                self.resizing = Some(Resize { id, w0: now, x0: x, w: now, left, snap: None });
                if let Some(i) = self.doc().index_of(id) {
                    let c = page::select_blocks(self.doc(), i, i);
                    self.move_to(c);
                }
                self.focus_page = true;
                self.editing = true;
            }
            Some("move") => {
                // Near the height of a picture beside it in its row, it
                // takes that height: the row lines up.
                let row = self.pic_rows.get(&id).copied();
                let heights: Vec<f32> = row
                    .map(|(r, ..)| self.pic_rows.iter().filter(|(o, (or, ..))| **o != id && *or == r).map(|(_, (_, h, _))| *h).collect())
                    .unwrap_or_default();
                if let Some(r) = self.resizing.as_mut() {
                    let dx = if r.left { r.x0 - x } else { x - r.x0 };
                    let w = page::dragged_width(r.w0, dx, max_w);
                    (r.w, r.snap) = page::snapped_width(w, row.map_or(0.0, |(.., a)| a), &heights, max_w);
                }
            }
            _ => {
                let Some(r) = self.resizing.take() else { return };
                // Back at the width it has with none given: none given.
                let width = page::resized_width(r.w, fit);
                self.edit(Step::Other, |d, _| page::set_image_width(d, id, width));
                self.reveal = false;
            }
        }
    }
    /// A kui table: rows whose cells line up in columns, the grid drawn as
    /// hairline rules. Every cell grows by the same weight from no floor,
    /// so a column's width is a share of the table, not of its text —
    /// typing can wrap a cell but never move the grid.
    fn table(&mut self, ui: &mut Ui<'_>, l: &Look, id: u64, table: &Table, body: TextStyle, cov: &Coverage) {
        let t = l.t;
        // The header, if it has one, is small and quiet: a label for its
        // column.
        let head = l.page_type(Type::LABEL).family(l.body).color(t.muted);
        let names: Vec<String> = match table.header {
            true => table.rows[0].cells.iter().map(|c| c.trim().to_string()).collect(),
            false => Vec::new(),
        };
        // A cell's share of the table's width, its padding left out: as
        // wide as a picture among its words is drawn at most. From the
        // table's own row, as the last frame laid it out (its inset and
        // frame left out), which no cell's words move.
        let pad = l.px(14.0);
        let table_w = self.row_rects.iter().rev().find(|(b, _)| *b == id).map_or(self.text_w, |(_, r)| r.w - 16.0) - 4.0;
        let room = (table_w / table.width().max(1) as f32 - 2.0 * pad).floor().max(page::PICTURE_MIN);
        ui.with(NodeSpec::column().grow_width().pad_xy(0.0, l.air(14.0)), |ui| {
            ui.with(
                // One warm hairline for the frame and the rules, and the
                // rows inset by it, so the header's wash never paints over
                // the frame.
                NodeSpec::table()
                    .grow_width()
                    .pad(1.0)
                    .rules(t.border)
                    .border(1.0, t.border)
                    .radius(radius::CONTROL)
                    .clip()
                    .role(Role::Group)
                    .label(format!("Table, {} rows, {} columns", table.rows.len(), table.width())),
                |ui| {
                    for (r, row) in table.rows.iter().enumerate() {
                        let is_head = table.is_header(r);
                        // Counted from 1 among the plain rows.
                        let n = r + 1 - table.body();
                        let mut spec = NodeSpec::row().grow_width().on_layout(Value::Null);
                        if is_head {
                            spec = spec.bg(t.sunken);
                        }
                        // The row's height is its tallest cell's: each
                        // cell is matched against all of it, so the room
                        // under short words beside a picture is theirs.
                        let row_rect = ui.layout_of(ui.child_key_indexed(row.id));
                        ui.with_indexed(row.id, spec, |ui| {
                            for (c, &col) in table.cols.iter().enumerate() {
                                let slot = Slot {
                                    block: id,
                                    cell: Some((row.id, col)),
                                };
                                // A cell is named for its column, so a reader
                                // hears "Price, row 3", not coordinates.
                                let name = match names.get(c) {
                                    _ if is_head => format!("Column {} header", c + 1),
                                    Some(h) if !h.is_empty() => format!("{h}, row {n}"),
                                    _ => format!("Column {}, row {n}", c + 1),
                                };
                                // Where the cell was laid out: what a click
                                // is matched against, by x as well as y.
                                let ck = ui.child_key_indexed(col);
                                if let Some(mut rect) = ui.layout_of(ck) {
                                    if let Some(rr) = row_rect {
                                        (rect.y, rect.h) = (rr.y, rr.h);
                                    }
                                    self.cell_rects.insert(slot, rect);
                                }
                                ui.with_indexed(
                                    col,
                                    NodeSpec::column()
                                        .width(Sizing::Grow(1.0))
                                        .min_width(48.0)
                                        .pad_xy(pad, l.px(if is_head { 9.0 } else { 8.0 }))
                                        .on_layout(Value::Null)
                                        .label(name),
                                    |ui| {
                                        let style = if is_head { head } else { body };
                                        self.cell_line(ui, l, slot, &row.cells[c], style, cov, room);
                                    },
                                );
                            }
                        });
                    }
                },
            );
        });
    }

    /// A table cell's text: as any slot's, but where pictures stand among
    /// its words, one under another with the stretches of words around
    /// them, each picture its own width, `room` at most: the cell decides,
    /// whatever width the note writes. A picture is one letter to the
    /// caret, which stands beside it where no words are; chosen, it wears
    /// the accent's ring, and the caret isn't shown.
    #[allow(clippy::too_many_arguments)]
    fn cell_line(&mut self, ui: &mut Ui<'_>, l: &Look, slot: Slot, text: &str, style: TextStyle, cov: &Coverage, room: f32) {
        let pictures = carnet_core::inline::cell_pictures(text);
        if pictures.is_empty() {
            self.cell_pieces.remove(&slot);
            self.slot_line(ui, l, slot, text, style, None, false, cov);
            return;
        }
        // Its pieces in order: words, a picture, words…
        let mut parts: Vec<(usize, usize, Option<&carnet_core::doc::CellPicture>)> = Vec::new();
        let mut last = 0;
        for p in &pictures {
            if p.at.0 > last {
                parts.push((last, p.at.0, None));
            }
            parts.push((p.at.0, p.at.1, Some(p)));
            last = p.at.1;
        }
        if last < text.len() {
            parts.push((last, text.len(), None));
        }
        // The caret: in the words that hold it, else beside its picture.
        let here = self.editing && self.caret.head.slot == slot;
        let byte = self.caret.head.byte.min(text.len());
        let caret_in = here
            .then(|| {
                let words = parts.iter().position(|&(a, b, p)| p.is_none() && a <= byte && byte <= b);
                words.or_else(|| parts.iter().position(|&(a, b, _)| a == byte || b == byte))
            })
            .flatten();
        let editing = self.editing;
        let chosen = |a: usize, b: usize| {
            cov.whole.contains(&slot.block) || cov.parts.get(&slot).is_some_and(|&(f, to)| editing && f <= a && b <= to && f < to)
        };
        let mut pieces = Vec::new();
        let column = ui.with(NodeSpec::column().grow_width().gap(l.px(6.0)), |ui| {
            for (k, &(a, b, pic)) in parts.iter().enumerate() {
                let at_caret = caret_in == Some(k);
                ui.with_indexed(k as u64, NodeSpec::column().grow_width(), |ui| match pic {
                    None => {
                        if let Some(key) = self.line_part(ui, l, slot, text, (a, b), style, None, false, cov, at_caret) {
                            pieces.push(Piece { from: a, to: b, key, picture: false });
                        }
                    }
                    Some(p) => {
                        let side = at_caret.then_some(byte >= b);
                        let key = self.cell_picture(ui, l, p, room, chosen(a, b), side);
                        pieces.push(Piece { from: a, to: b, key, picture: true });
                    }
                });
            }
        });
        // What was drawn, as a line's text says it (`line_part`).
        let _cell = page::reading(slot);
        let hidden: Vec<(usize, usize)> = inline::runs(text).iter().filter(|r| r.2.marker).map(|r| (r.0, r.1)).collect();
        self.drawn.insert(slot, hash(&format!("{hidden:?}{text}")));
        self.cell_pieces.insert(slot, pieces);
        // A cell of pictures alone has no words' node: its pieces' is
        // what stands for it.
        self.slot_keys.entry(slot).or_insert(column);
    }

    /// A picture among a cell's words: its own width, `room` at most, its
    /// shape kept (a width the note writes for it is not drawn by: the
    /// cell decides); one not shown a small card saying why, as a picture
    /// on its own line has. `chosen`, the accent's ring round it; the
    /// caret beside it on the left or the right (`side`, `true` the
    /// right), drawn while nothing is selected. Returns its box's node.
    fn cell_picture(&mut self, ui: &mut Ui<'_>, l: &Look, p: &carnet_core::doc::CellPicture, room: f32, chosen: bool, side: Option<bool>) -> Key {
        let t = l.t;
        let folder = self.page_folder();
        let file = self.store.picture_file(&folder, &p.src, p.wiki);
        let entry = file.as_ref().map(|f| self.images.get(f));
        let theirs = file.as_deref().is_some_and(|f| !self.store.owns(f));
        let ring = if chosen { t.accent } else { Color::TRANSPARENT };
        let natural = match &entry {
            Some(images::Entry::Ready { w, h, .. }) | Some(images::Entry::Loading { w, h }) if *w > 0 && *h > 0 => Some((*w as f32, *h as f32)),
            _ => None,
        };
        let label = if p.alt.is_empty() { "Picture".to_string() } else { format!("Picture: {}", p.alt) };
        let key = ui.child_key("picture");
        let blink = ui.caret_visible();
        // A selection (this picture chosen, or more) shows no caret.
        let bar = side.filter(|_| self.caret.is_empty());
        // The caret beside it, `right` or left: its height, outside its edge.
        let caret = |h: f32, right: bool| {
            NodeSpec::row()
                .size(2.0, h.max(8.0))
                .radius(1.0)
                .bg(t.accent)
                .opacity(if blink { 1.0 } else { 0.0 })
                .float(FloatConfig::parent().inside(if right { Align::End } else { Align::Start }, Align::Center).offset(if right { 5.0 } else { -5.0 }, 0.0))
                .role(Role::None)
        };
        let Some((nw, nh)) = natural else {
            let (glyph, why) = self.unshown_why(entry.as_ref(), file.as_deref(), theirs, &p.src, p.wiki);
            let path = store::decode_src(&p.src);
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            let w = room;
            let words_w = (w - 54.0).max(40.0);
            ui.with_keyed(
                "picture",
                NodeSpec::row()
                    .width(Sizing::Fit)
                    .max_width(w.max(page::PICTURE_MIN))
                    .cross_align(Align::Center)
                    .pad_xy(10.0, 8.0)
                    .gap(8.0)
                    .radius(radius::CARD)
                    .bg(t.sunken)
                    .border(2.0, ring)
                    .on_layout(Value::Null)
                    .role(Role::Image)
                    .label(format!("{why}: {name}")),
                |ui| {
                    icons::icon(ui, glyph, t.muted, 16.0, false);
                    ui.with(NodeSpec::column().gap(2.0).max_width(words_w), |ui| {
                        ui.text_in(NodeSpec::row().max_width(words_w), why, TextStyle::new(Type::SMALL.size).color(t.fg));
                        ui.text_in(NodeSpec::row().max_width(words_w), &name, TextStyle::new(Type::SMALL.size).color(t.muted).ellipsis());
                    });
                    if let Some(right) = bar {
                        ui.leaf_keyed("caret", caret(36.0, right));
                    }
                },
            );
            if side.is_some() {
                self.caret_fresh = true;
            }
            return key;
        };
        let (dw, _) = page::picture_width(nw, None, room, room, false);
        let dh = dw * nh / nw;
        ui.with_keyed("picture", NodeSpec::row().size(dw, dh).on_layout(Value::Null).role(Role::Image).label(label), |ui| {
            match &entry {
                Some(images::Entry::Ready { id: img, .. }) => {
                    ui.image(*img, NodeSpec::row().size(dw, dh).radius(7.0).opacity(if theirs { THEIRS_OPACITY } else { 1.0 }));
                }
                // Decoding: the well it will fill.
                _ => {
                    ui.with(NodeSpec::row().size(dw, dh).radius(7.0).bg(t.sunken).center(), |ui| {
                        icons::icon(ui, Glyph::Picture, t.faint, 24.0_f32.min(dh * 0.5), false)
                    });
                }
            }
            // The ring an overlay 4px out, so choosing it moves nothing.
            ui.leaf(
                NodeSpec::row()
                    .size(dw + 8.0, dh + 8.0)
                    .radius(radius::CARD)
                    .border(2.0, ring)
                    .role(Role::None)
                    .float(FloatConfig::parent().offset(-4.0, -4.0).clipped()),
            );
            if let Some(right) = bar {
                ui.leaf_keyed("caret", caret(dh, right));
            }
        });
        if side.is_some() {
            self.caret_fresh = true;
        }
        key
    }

    /// One slot's text: a `line` row holding the wrapped text — its inline
    /// styles drawn, their markers faint, the part the selection covers on
    /// the selection's wash, an IME composition underlined at the caret —
    /// and the caret.
    #[allow(clippy::too_many_arguments)]
    fn slot_line(
        &mut self,
        ui: &mut Ui<'_>,
        l: &Look,
        slot: Slot,
        text: &str,
        style: TextStyle,
        hint: Option<&str>,
        strike: bool,
        cov: &Coverage,
    ) {
        let here = self.editing && self.caret.head.slot == slot;
        self.line_part(ui, l, slot, text, (0, text.len()), style, hint, strike, cov, here);
    }

    /// The words of `slot` from `lo` to `hi` as [`slot_line`](Self::slot_line)
    /// draws a slot's — all of them, or a stretch of a cell's between its
    /// pictures — with the caret if it is drawn here (`here`). Returns
    /// the text's node.
    #[allow(clippy::too_many_arguments)]
    fn line_part(
        &mut self,
        ui: &mut Ui<'_>,
        l: &Look,
        slot: Slot,
        text: &str,
        (lo, hi): (usize, usize),
        style: TextStyle,
        hint: Option<&str>,
        strike: bool,
        cov: &Coverage,
        here: bool,
    ) -> Option<Key> {
        let t = &l.t;
        // Code blocks are code: no inline styles in them, and only the
        // lines between their fences drawn, a place on a fence at the
        // nearer end of them (`page::drawn`).
        let code_block = matches!(self.doc().get(slot.block).map(|b| &b.kind), Some(Kind::Raw(_)));
        let (lo, hi) = if code_block { page::writable(self.doc(), slot) } else { (lo, hi) };
        let byte = self.caret.head.byte.min(text.len());
        let byte = if code_block { byte.clamp(lo, hi) } else { byte };
        // A cell's text as a cell's: its pictures among its words.
        let _cell = page::reading(slot);
        let runs = if code_block {
            vec![(0, text.len(), inline::Style::default())]
        } else {
            inline::runs(text)
        };
        // Markdown's marks are never drawn: the words are, styled. Code
        // keeps every byte.
        let hidden: Vec<(usize, usize)> = if code_block {
            Vec::new()
        } else {
            runs.iter().filter(|r| r.2.marker).map(|r| (r.0, r.1)).collect()
        };
        if code_block {
            self.shown.insert(slot, page::drawn(self.doc(), slot));
        } else if !hidden.is_empty() && !text.is_empty() {
            // What is drawn: what a press's byte is read back through.
            self.shown.insert(slot, inline::Visible::of(text));
        }
        // Bytes of the drawn text from where this part starts.
        let d0 = self.drawn_byte(slot, lo);
        let mut row = NodeSpec::row().grow_width().role(Role::Line);
        if here {
            row = row.caret(self.drawn_byte(slot, byte).saturating_sub(d0) as u32);
            if self.caret.anchor.slot == slot && !self.caret.is_empty() {
                row = row.selection_anchor((self.drawn_byte(slot, self.caret.anchor.byte.clamp(lo, hi)) - d0) as u32);
            }
        }
        // A stretch of a cell's words: its cell says what was drawn, once
        // all of it is (`cell_line`).
        let drawn = hash(&format!("{hidden:?}{text}"));
        let fresh = self.drawn.get(&slot) == Some(&drawn);
        if (lo, hi) == (0, text.len()) || code_block {
            self.drawn.insert(slot, drawn);
        }
        let (drawn_byte, drawn_0) = (self.drawn_byte(slot, byte).saturating_sub(d0), 0);
        let preedit = if here { self.preedit.clone() } else { None };
        // A selection shows only while the page has the keyboard: one
        // selection in the window at a time. A block covered whole wears
        // the wash on its row instead.
        let part = cov
            .parts
            .get(&slot)
            .copied()
            .filter(|_| self.editing && !cov.whole.contains(&slot.block))
            .map(|(f, to)| (f.max(lo), to.min(hi)))
            .filter(|(f, to)| f < to);
        if here {
            self.caret_fresh = fresh;
        }
        let blink = ui.caret_visible();
        // The dropped first letter: where it is in the source, with any
        // opening quote before it. Not while a composition stands in
        // front of it, which would be dropped in its place.
        let cap = (self.initial.contains(&slot) && !code_block)
            .then(|| inline::initial_letter(text))
            .flatten()
            .filter(|&(f, _)| !(preedit.is_some() && byte <= f));
        let (dark, page_bg) = (l.c.dark, t.bg);
        let (cap_lines, cap_accent, text_w) = (self.settings.initial_lines, self.settings.initial_accent, self.text_w);
        let mut text_key = None;
        ui.with(row, |ui| {
            let key = ui.child_key_indexed(0);
            text_key = Some(key);
            let mut spans: Vec<Span<'_>> = Vec::new();
            // The span holding the first letter, if it may drop, and
            // whether the letter has a colour of its own.
            let mut letter: Option<usize> = None;
            let mut letter_own = false;
            if lo == hi && preedit.is_none() {
                // An empty line still has a line's height, and a place for
                // the caret; its placeholder floats over it, easing in.
                spans.push(Span::new("\u{200B}"));
            } else {
                // Cut points: the styles' runs, the selection's ends and
                // the composition.
                let mut cuts = vec![lo, hi];
                cuts.extend(runs.iter().map(|r| r.0));
                cuts.extend(hidden.iter().flat_map(|&(f, to)| [f, to]));
                if let Some((f, to)) = part {
                    cuts.extend([f.min(text.len()), to.min(text.len())]);
                }
                if preedit.is_some() {
                    cuts.push(byte);
                }
                if let Some((f, to)) = cap {
                    cuts.extend([f, to]);
                }
                cuts.retain(|&c| lo <= c && c <= hi);
                cuts.sort_unstable();
                cuts.dedup();
                for w in cuts.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    if preedit.is_some() && a == byte {
                        spans.push(Span::new(preedit.as_deref().unwrap_or("")).underline());
                    }
                    let s = runs
                        .iter()
                        .find(|r| r.0 <= a && a < r.1)
                        .map(|r| r.2)
                        .unwrap_or_default();
                    if hidden.iter().any(|&(f, to)| f <= a && b <= to) {
                        continue;
                    }
                    let mut sp = Span::new(&text[a..b]);
                    if s.marker {
                        // The marks show on the line being written; on the
                        // others they step back further.
                        sp = sp.color(if here { t.faint } else { t.faint.with_alpha(0.4) });
                    } else {
                        // Colour: the words' own, unless it would vanish
                        // into the page (black words in the dark); a
                        // highlighter's wash, the words on it in whichever
                        // of black and white reads.
                        // Colour: the words' own and a highlighter's wash,
                        // both; see `words_ink`.
                        let plain = style.color.unwrap_or(t.fg);
                        let own = (s.ink != 0).then(|| crate::design::tint::ink(s.ink, dark));
                        let wash = (s.mark != 0).then(|| {
                            if s.mark == inline::HIGHLIGHT {
                                crate::design::tint::highlight(dark)
                            } else {
                                crate::design::tint::wash(s.mark, dark, page_bg)
                            }
                        });
                        if let Some(w) = wash {
                            sp = sp.bg(w).bg_radius(3.0);
                        }
                        let ink = words_ink(own, plain, wash, page_bg);
                        if ink != plain {
                            sp = sp.color(ink);
                        }
                        if s.bold {
                            sp = sp.bold();
                        }
                        if s.italic {
                            sp = sp.italic();
                        }
                        if s.strike || strike {
                            sp = sp.strikethrough();
                        }
                        // In the words' own ink, as a link's is in its:
                        // a link underlined is still one line.
                        if s.underline {
                            sp = sp.underline();
                        }
                        if s.code {
                            // Code reads as code: in the code's face, a size
                            // down as on the web (14px in 16), its own ink on
                            // a wash of the text's colour, clear in either
                            // theme.
                            sp = sp
                                .family(l.code)
                                .size((style.size * 0.875).round())
                                .color(l.c.accent_ink)
                                .bg(t.fg.with_alpha(0.09))
                                .bg_radius(4.0);
                        }
                        if s.link {
                            sp = sp.color(t.focus_ring).underline();
                        }
                    }
                    if cap.is_some_and(|(f, to)| a >= f && b <= to) {
                        // Styled below, once it's known to drop.
                        letter = Some(spans.len());
                        letter_own |= !s.marker && s.ink != 0;
                    }
                    if part.is_some_and(|(f, to)| a >= f && b <= to) {
                        sp = sp.bg(t.selection).bg_radius(4.0);
                    }
                    spans.push(sp);
                }
                if preedit.is_some() && byte == hi {
                    spans.push(Span::new(preedit.as_deref().unwrap_or("")).underline());
                }
                if spans.is_empty() {
                    // Nothing but marks: still a line's height.
                    spans.push(Span::new("\u{200B}"));
                }
            }
            // A drop cap: the letter as tall as the first lines, which
            // wrap beside it; the file is as it was. As tall as the lines
            // the paragraph fills, up to the setting: a sentence of two
            // lines drops two, and one of a single line none, rather than
            // a tall letter beside a short line.
            let drop = letter.and_then(|i| Some((i, drop_lines(cap_lines, ui.measure_rich_text(&spans, &style, Some(text_w)).lines)?)));
            let style = match drop {
                Some((i, n)) => {
                    // In its face (the headings', unless one is chosen), in
                    // its own colour if it was given one, else the accent
                    // deepened for text or the words'; kui sizes it to the
                    // lines it drops through. Undropped, it's a letter
                    // like the rest.
                    let mut sp = spans[i].family(l.initial);
                    if cap_accent && !letter_own {
                        sp = sp.color(l.c.accent_ink);
                    }
                    spans[i] = sp;
                    style.initial_letter(n)
                }
                None => style,
            };
            ui.rich_text(&spans, style);
            self.slot_keys.insert(slot, key);
            if let Some(h) = hint.filter(|_| text.is_empty() && preedit.is_none()) {
                ui.text_in_keyed(
                    "hint",
                    NodeSpec::row()
                        .float(FloatConfig::parent())
                        .enter(kui_native::Enter::default().opacity(0.0))
                        .transition(320.0)
                        .role(Role::None),
                    h,
                    style.color(t.faint).nowrap(),
                );
            }
            if !(here && self.caret.is_empty() && preedit.is_none()) {
                return;
            }
            // The bar is declared on every frame and blinks by its opacity:
            // a float that came and went with the blink would come back
            // each time over whatever opened since (the `/` menu).
            let bar = |h: f32, (dx, dy): (f32, f32), on: bool| {
                NodeSpec::row()
                    .size(2.0, h.max(8.0))
                    .radius(1.0)
                    .bg(t.accent)
                    .opacity(if on { 1.0 } else { 0.0 })
                    .float(FloatConfig::parent().offset(dx, dy))
                    .role(Role::None)
            };
            // On an empty line the caret is the line's start: nothing to
            // measure, so nothing to wait for.
            if lo == hi {
                ui.leaf_keyed("caret", bar(style.line_height, (0.0, 0.0), blink));
                return;
            }
            // Elsewhere, where the last layout put this byte, measured from
            // where it put the first, so the bar rides along however the
            // page scrolled since. Text not measured yet: out of sight for
            // the frame it takes.
            match (fresh, ui.caret_rect(key, drawn_byte), ui.caret_rect(key, drawn_0)) {
                (true, Some(r), Some(o)) => {
                    ui.leaf_keyed("caret", bar(r.h, (r.x - o.x, r.y - o.y), blink));
                }
                _ => {
                    ui.leaf_keyed("caret", bar(style.line_height, (0.0, 0.0), false));
                    ui.request_frame();
                }
            }
        });
        text_key
    }

    /// Where the drawn text has source byte `b` of `slot`: past the marks
    /// hidden before it.
    fn drawn_byte(&self, slot: Slot, b: usize) -> usize {
        self.shown.get(&slot).map_or(b, |v| v.drawn(b))
    }

    /// The source byte of `slot` at byte `r` of the drawn text: after the
    /// letter before it, inside that letter's marks (`Visible::at`).
    fn source_byte(&self, slot: Slot, r: usize) -> usize {
        self.shown.get(&slot).map_or(r, |v| v.at(r))
    }

    /// Where the last layout put source byte `b` of `slot`. In a cell with
    /// pictures, in the stretch of words that holds it, else at the edge
    /// of the picture beside it.
    pub(super) fn rect_at(&self, core: &Core, slot: Slot, b: usize) -> Option<Rect> {
        if let Some(pieces) = self.cell_pieces.get(&slot) {
            let words = pieces.iter().find(|p| !p.picture && p.from <= b && b <= p.to);
            let p = words.or_else(|| pieces.iter().find(|p| p.from <= b && b <= p.to))?;
            if p.picture {
                let r = core.layout_of(p.key)?;
                let x = if b <= p.from { r.x } else { r.x + r.w };
                return Some(Rect { x, w: 0.0, ..r });
            }
            return core.caret_rect(p.key, self.drawn_byte(slot, b) - self.drawn_byte(slot, p.from));
        }
        let k = *self.slot_keys.get(&slot)?;
        core.caret_rect(k, self.drawn_byte(slot, b))
    }

    /// The source byte of `slot` under a point, by the last layout. In a
    /// cell with pictures, of the piece nearest it up or down: a byte of
    /// its words, or before or after its picture, by the picture's half.
    fn byte_under(&self, core: &Core, slot: Slot, at: Vec2) -> Option<usize> {
        if let Some(pieces) = self.cell_pieces.get(&slot) {
            let span = |p: &Piece| -> Option<(f32, f32)> {
                if p.picture {
                    let r = core.layout_of(p.key)?;
                    return Some((r.y, r.y + r.h));
                }
                let n = self.drawn_byte(slot, p.to) - self.drawn_byte(slot, p.from);
                let (a, e) = (core.caret_rect(p.key, 0)?, core.caret_rect(p.key, n)?);
                Some((a.y, e.y + e.h))
            };
            let off = |(top, bottom): (f32, f32)| (top - at.y).max(0.0).max(at.y - bottom);
            let (p, _) = pieces.iter().filter_map(|p| Some((p, off(span(p)?)))).min_by(|a, b| a.1.total_cmp(&b.1))?;
            if p.picture {
                let r = core.layout_of(p.key)?;
                return Some(if at.x < r.x + r.w / 2.0 { p.from } else { p.to });
            }
            return core.text_hit(p.key, at).map(|h| self.source_byte(slot, self.drawn_byte(slot, p.from) + h.byte));
        }
        let k = *self.slot_keys.get(&slot)?;
        core.text_hit(k, at).map(|h| self.source_byte(slot, h.byte))
    }

    /// The table cell under a point, by the last layout.
    pub(super) fn cell_at(&self, x: f32, y: f32) -> Option<Slot> {
        self.cell_rects.iter().find(|(_, r)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h).map(|(s, _)| *s)
    }

    /// The picture among a cell's words under a point, by the last
    /// layout: its cell and where it is in the cell's text.
    fn cell_picture_under(&self, core: &Core, x: f32, y: f32) -> Option<(Slot, usize, usize)> {
        self.cell_pieces.iter().find_map(|(slot, pieces)| {
            pieces.iter().filter(|p| p.picture).find_map(|p| {
                let r = core.layout_of(p.key)?;
                (x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h).then_some((*slot, p.from, p.to))
            })
        })
    }

    // ------------------------------------------------------------ input

    /// The page sink's events: a key, typed or pasted text, an IME
    /// composition.
    pub(super) fn page_event(&mut self, ev: &UiEvent, core: &mut Core) {
        match ev.kind() {
            Some("key") => {
                if let Some((KeyPhase::Down, k)) = ev.key_press() {
                    // A letter's press is writing (it types its text:
                    // `page_key_press`); Enter, Tab and the deletes are
                    // too, with a modifier or without (⌘⌫ takes the line
                    // back); Esc
                    // steps out of it. Everything else — the arrows, the
                    // chords that move or style — leaves it as it was:
                    // what takes the keyboard elsewhere (the search, a new
                    // note, the link editor) ends it by leaving the page.
                    match k.code {
                        KeyCode::Escape => self.typing = false,
                        KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace | KeyCode::Delete => {
                            self.typing = true;
                            self.typed_at = core.now();
                        }
                        _ => {}
                    }
                    self.page_key_press(k, core);
                }
            }
            Some("text") => {
                let s = ev.payload.get_str("text").unwrap_or("").to_string();
                let pasted = std::mem::take(&mut self.paste_pending) || ev.payload.get("pasted").is_some();
                self.preedit = None;
                if pasted {
                    self.edit(Step::Other, |d, c| page::paste(d, c, &s));
                } else {
                    // Writing on: the colour picker has been left behind.
                    self.tint_open = None;
                    self.typing = true;
                    self.typed_at = core.now();
                    self.type_text(&s);
                }
            }
            Some("preedit") => {
                // A letter being composed (an accent held, an input
                // method) is typing too.
                self.typing = true;
                self.typed_at = core.now();
                let s = ev.payload.get_str("text").unwrap_or("");
                self.preedit = (!s.is_empty()).then(|| s.to_string());
            }
            // A right-click: the caret goes where it was unless that is
            // inside the selection, which the menu then acts on.
            Some("contextmenu") => {
                let p = &ev.payload;
                let num = |k: &str| {
                    p.get_float(k)
                        .or_else(|| p.get_int(k).map(|v| v as f64))
                        .unwrap_or(0.0) as f32
                };
                let (x, y) = (num("x"), num("y"));
                if let Some(pos) = self.hit(core, x, y)
                    && !page::contains(self.doc(), self.caret, pos)
                {
                    self.caret = Caret::at(pos);
                }
                self.editing = true;
                self.focus_page = true;
                self.writing.moved();
                self.history().seal();
                self.pending_menu = Some((ev.key, Vec2::new(x, y), MenuFor::Page));
                self.menu_at = Vec2::new(x, y);
            }
            _ => {}
        }
    }

    /// The page's right-click menu: the clipboard, the styles, what the
    /// block can turn into, and the table's tools inside a table.
    pub(super) fn page_menu(&self) -> Vec<MenuItem> {
        let doc = self.doc();
        let selected = !self.caret.is_empty();
        let in_table = self.caret.head.slot.cell.is_some();
        let item = |label: &str, action: &str| {
            MenuItem::new(label).id(Value::map([("page", Value::str(action))]))
        };
        let picture = self.picture_at_caret().is_some();
        let mut items = Vec::new();
        if let Some((_, _, url)) = page::link_at(doc, self.caret) {
            // Only a link a note may follow is offered to open (export.rs
            // `safe_href`): a `javascript:` one isn't, as on the web.
            items.extend([
                item("Open link", "open-link").enabled(carnet_core::export::safe_href(&url).is_some()),
                item("Copy link address", "copy-link"),
                item("Remove link", "unlink"),
                MenuItem::separator(),
            ]);
        }
        items.extend([
            item("Cut", "cut").accel("mod+x").enabled(selected || picture),
            item("Copy", "copy").accel("mod+c").enabled(selected || picture),
            item("Paste", "paste").accel("mod+v"),
            item("Select all", "all").accel("mod+a"),
            item("Copy note as Markdown", "copy-note"),
            item("Note history", "history"),
            MenuItem::separator(),
        ]);
        if let Some((_, float, now)) = self.picture_here() {
            items.extend([item("Copy picture", "copy-picture"), item("Copy picture as Markdown", "copy-picture-md"), MenuItem::separator()]);
            // Sizes: shares of the text's width, the one it has now ticked.
            let full = (self.text_w - 10.0).max(48.0);
            for (label, pct) in page::PICTURE_SIZES {
                let w = (full * pct as f32 / 100.0).round();
                items.push(item(label, &format!("size:{pct}")).checked((w - now).abs() < 2.0));
            }
            items.push(item("Original size", "picture-size").enabled(self.picture_width_set()));
            items.push(MenuItem::separator());
            for (label, to, spec) in [
                ("On its own line", Float::None, None),
                ("Left of the text", Float::Left, Some(keys::FLOAT_LEFT)),
                ("Right of the text", Float::Right, Some(keys::FLOAT_RIGHT)),
            ] {
                let mut it = item(label, &format!("float:{}", to as u8)).checked(float == to);
                if let Some(spec) = spec {
                    it = it.accel(spec);
                }
                items.push(it);
            }
            items.push(MenuItem::separator());
            // The user's own file: ways to make it Carnet's.
            if self.picture_at_caret().is_some_and(|(src, wiki)| {
                self.store.picture_file(&self.page_folder(), &src, wiki).is_some_and(|p| p.is_file() && !self.store.owns(&p) && self.images.is_ready(&p))
            }) {
                items.extend([item("Move into Carnet", "into-carnet:0"), item("Copy into Carnet", "into-carnet:1"), MenuItem::separator()]);
            }
            items.extend([item(super::reveal_label(), "reveal-picture"), item("Remove picture", "remove")]);
            return items;
        }
        items.extend([
            item("Bold", "style:0").accel("mod+b"),
            item("Italic", "style:1").accel("mod+i"),
            item("Strikethrough", "style:2").accel("mod+shift+x"),
            item("Underline", "style:4").accel(keys::UNDERLINE),
            item("Code", "style:3").accel("mod+e"),
            item(if page::link_at(doc, self.caret).is_some() { "Edit link\u{2026}" } else { "Link\u{2026}" }, "link")
                .accel("mod+k"),
            item("Highlight", "highlight").accel(keys::HIGHLIGHT),
            item("Text colour\u{2026}", "tint-menu:1"),
            item("Highlight colour\u{2026}", "tint-menu:0"),
            MenuItem::separator(),
        ]);
        if in_table {
            items.extend([
                item("Add row below", "addrow"),
                item("Add column right", "addcol"),
                item("Remove row", "delrow"),
                item("Remove column", "delcol"),
                item("Header row", "header").checked(self.table_here().is_some_and(|t| t.header)),
                MenuItem::separator(),
                item("Remove table", "remove"),
            ]);
        } else {
            let convertible = page::touched(doc, self.caret)
                .any(|i| page::turned(&doc.blocks()[i].kind, TURN_TEXT).is_some());
            for (label, to, spec) in [
                ("Text", TURN_TEXT, keys::TEXT),
                ("Heading", TURN_H2, keys::H2),
                ("Bulleted list", TURN_BULLET, keys::BULLETS),
                ("Numbered list", TURN_NUMBER, keys::NUMBERS),
                ("Checklist", TURN_TODO, keys::TODO),
                ("Quote", TURN_QUOTE, keys::QUOTE),
            ] {
                items.push(item(label, &format!("turn:{to}")).accel(spec).enabled(convertible));
            }
            let initial = page::initial_at(doc, self.caret);
            items.push(
                item("Drop the first letter", "initial")
                    .accel(keys::INITIAL)
                    .enabled(initial.is_some())
                    .checked(initial == Some(true)),
            );
            items.extend([
                MenuItem::separator(),
                item("Insert table", "table").accel(keys::TABLE),
                item("Insert divider", "rule").accel(keys::DIVIDER),
                MenuItem::separator(),
                item(if selected { "Duplicate blocks" } else { "Duplicate block" }, "duplicate").accel(keys::DUPLICATE),
                item(if selected { "Remove blocks" } else { "Remove block" }, "remove").accel(keys::REMOVE),
            ]);
        }
        items
    }

    /// What the page's menu (or its shortcuts) asked for.
    pub(super) fn page_action(&mut self, action: &str) {
        if action == "history" {
            self.show_history();
            return;
        }
        self.focus_page = true;
        self.editing = true;
        match action {
            "cut" => {
                if self.copy_selection() {
                    if self.picture_at_caret().is_some() && self.caret.is_empty() {
                        self.on_plain(Msg::DeleteBlock);
                    } else {
                        self.edit(Step::Other, page::delete_selection);
                    }
                }
            }
            "copy" => {
                self.copy_selection();
            }
            "paste" => self.paste(),
            "copy-note" => self.on_plain(Msg::CopyText { what: 1 }),
            "duplicate" => {
                self.edit(Step::Other, page::duplicate);
            }
            "copy-picture" => {
                if let Some((src, wiki)) = self.picture_at_caret() {
                    self.copy_picture(&src, wiki);
                }
            }
            "copy-picture-md" => {
                if let Some((src, wiki)) = self.picture_at_caret() {
                    // As the note writes it: an Obsidian embed stays one.
                    let none = carnet_core::doc::Float::None;
                    let md = wiki
                        .then(|| carnet_core::doc::image_wiki("", &src, None, none))
                        .flatten()
                        .unwrap_or_else(|| carnet_core::doc::image_markdown("", &src, None, none));
                    let folder = self.store.notes[self.current].folder.clone();
                    self.copy_markdown(md, &folder);
                }
            }
            "picture-size" => {
                if let Some(i) = page::touched(self.doc(), self.caret).next() {
                    let id = self.doc().blocks()[i].id;
                    self.edit(Step::Other, |d, _| page::set_image_width(d, id, None));
                }
            }
            "reveal-picture" => {
                let folder = self.store.notes[self.current].folder.clone();
                if let Some(p) = self.picture_at_caret().and_then(|(s, wiki)| self.store.picture_file(&folder, &s, wiki))
                    && let Err(e) = store::reveal(&p)
                {
                    self.error = Some(format!("Couldn't open the folder: {e}"));
                }
            }
            "all" => {
                let all = page::select_all(self.doc());
                self.move_to(all);
            }
            "link" => self.open_link_editor(),
            "open-link" => {
                if let Some((_, _, url)) = page::link_at(self.doc(), self.caret) {
                    self.open_url(&url);
                }
            }
            "copy-link" => {
                if let Some((_, _, url)) = page::link_at(self.doc(), self.caret) {
                    self.clip_out = Some((url, None));
                }
            }
            "unlink" => {
                if let Some((slot, at, _)) = page::link_at(self.doc(), self.caret) {
                    self.edit(Step::Other, |d, _| page::set_link(d, slot, at, None));
                }
            }
            "highlight" => self.on_plain(Msg::Tint { ink: 0, color: inline::HIGHLIGHT as u64 }),
            "addrow" => self.on_plain(Msg::AddRow),
            "addcol" => self.on_plain(Msg::AddCol),
            "delrow" => self.on_plain(Msg::DelRow),
            "delcol" => self.on_plain(Msg::DelCol),
            "header" => self.on_plain(Msg::TableHeader),
            "remove" => self.on_plain(Msg::DeleteBlock),
            "table" => self.on_plain(Msg::InsertTable),
            "rule" => self.on_plain(Msg::InsertDivider),
            "initial" => self.on_plain(Msg::ToggleInitial),
            a => {
                if let Some(n) = a.strip_prefix("style:").and_then(|n| n.parse().ok()) {
                    self.on_plain(Msg::Style { which: n });
                } else if let Some(copy) = a.strip_prefix("into-carnet:").and_then(|n| n.parse::<u64>().ok()) {
                    self.take_in_picture(copy == 1);
                } else if let Some(pct) = a.strip_prefix("size:").and_then(|n| n.parse::<f32>().ok()) {
                    if let Some((id, _, _)) = self.picture_here() {
                        let w = ((self.text_w - 10.0).max(48.0) * pct / 100.0).round() as u32;
                        self.edit(Step::Other, |d, _| page::set_image_width(d, id, Some(w)));
                    }
                } else if let Some(to) = a.strip_prefix("float:").and_then(|n| n.parse::<u8>().ok()) {
                    // A shortcut on a picture already there puts it back on
                    // its own line.
                    let to = match to {
                        1 => Float::Left,
                        2 => Float::Right,
                        _ => Float::None,
                    };
                    if let Some((id, now, _)) = self.picture_here() {
                        let to = page::float_toggled(now, to);
                        self.edit(Step::Other, |d, _| page::set_image_float(d, id, to));
                    }
                } else if let Some(ink) = a.strip_prefix("tint-menu:").and_then(|n| n.parse::<u64>().ok()) {
                    // From the page's menu: the picker where the menu was.
                    self.tint_open = Some((ink == 1, self.menu_at));
                    self.tint_from = None;
                } else if let Some(to) = a.strip_prefix("turn:").and_then(|n| n.parse().ok()) {
                    self.on_plain(Msg::Turn { to });
                }
            }
        }
    }

    /// The page's key handling, for tests that need a press the driver
    /// cannot spell (a key on another layout).
    #[cfg(test)]
    pub(super) fn page_key_press_for_test(&mut self, k: KeyPress, core: &Core) {
        self.page_key_press(k, core);
    }

    pub(super) fn page_key_press(&mut self, k: KeyPress, core: &Core) {
        let m = k.mods;
        let chord = m.ctrl || m.super_key;
        // Reading a version or a deleted note: Esc goes back to the note.
        if k.code == KeyCode::Escape && self.reading() {
            self.close_history();
            return;
        }
        // The colour picker leaves the keyboard here: Esc puts it away.
        if k.code == KeyCode::Escape && self.tint_open.is_some() {
            self.tint_open = None;
            return;
        }
        // The `/` menu has the arrows, Enter, Tab and Esc while it shows.
        if let Some((block, shown)) = self.slash_now()
            && !chord
            && !m.alt
        {
            let n = shown.len();
            let sel = self.slash_sel.min(n - 1);
            match k.code {
                KeyCode::Up => {
                    self.slash_sel = (sel + n - 1) % n;
                    return;
                }
                KeyCode::Down => {
                    self.slash_sel = (sel + 1) % n;
                    return;
                }
                KeyCode::Enter | KeyCode::Tab if !m.shift => {
                    self.run_slash(shown[sel]);
                    return;
                }
                KeyCode::Escape => {
                    self.slash_off = Some(block);
                    return;
                }
                _ => {}
            }
        }
        // Word motion: Ctrl on Windows and Linux, Option on a Mac. Cmd on a
        // Mac goes to the edge: of the line (←→), of the note (↑↓).
        let word = m.ctrl || m.alt;
        let shift = m.shift;
        let code = if chord || m.alt { keys::chord_code(&k) } else { k.code };
        let doc = self.doc();
        match code {
            // The note above or below in the library.
            KeyCode::Up | KeyCode::Down if chord && m.alt => self.step_note(code == KeyCode::Up),
            KeyCode::Up | KeyCode::Down if m.alt => {
                let by = if code == KeyCode::Up { -1 } else { 1 };
                self.edit(Step::Other, |d, c| page::move_blocks(d, *c, by));
            }
            // The note's top or bottom; with Shift, selected to there.
            KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End if chord => {
                let all = page::slots(doc);
                let p = if matches!(code, KeyCode::Up | KeyCode::Home) {
                    page::start(all[0])
                } else {
                    page::end(doc, *all.last().unwrap_or(&all[0]))
                };
                let anchor = if shift { self.caret.anchor } else { p };
                self.goal_x = None;
                self.move_to(Caret { head: p, anchor });
            }
            KeyCode::Left | KeyCode::Right if m.super_key => {
                self.line_edge(core, code == KeyCode::Left, shift);
            }
            // Cmd+Backspace: to the start of the line.
            KeyCode::Backspace if m.super_key && !shift => {
                self.line_edge(core, true, true);
                self.edit(Step::Deleting, page::delete_selection);
            }
            // Up from the first line, back from the very start: the title,
            // as it would be in one text.
            KeyCode::Up | KeyCode::Left
                if !shift
                    && (self.caret.is_empty() || page::chosen_atom(doc, self.caret).is_some())
                    && self.at_top(core, code == KeyCode::Left) =>
            {
                // ↑ keeps the column it is in; ← comes up to the end.
                let column = (code == KeyCode::Up)
                    .then(|| self.goal_x.or_else(|| self.rect_at(core, self.caret.head.slot, self.caret.head.byte).map(|r| r.x)))
                    .flatten();
                self.up_to_title(column);
            }
            KeyCode::Left => {
                let c = page::left(doc, self.caret, word, shift);
                self.goal_x = None;
                self.move_to(c);
            }
            KeyCode::Right => {
                let c = page::right(doc, self.caret, word, shift);
                self.goal_x = None;
                self.move_to(c);
            }
            KeyCode::Up => self.vertical(core, true, shift, 1),
            KeyCode::Down => self.vertical(core, false, shift, 1),
            KeyCode::PageUp => self.vertical(core, true, shift, 15),
            KeyCode::PageDown => self.vertical(core, false, shift, 15),
            KeyCode::Home => self.line_edge(core, true, shift),
            KeyCode::End => self.line_edge(core, false, shift),
            KeyCode::Backspace => {
                self.edit(Step::Deleting, |d, c| page::backspace(d, c, word));
            }
            KeyCode::Delete => {
                self.edit(Step::Deleting, |d, c| page::delete(d, c, word));
            }
            KeyCode::Enter if chord => {
                let id = self.caret.head.slot.block;
                self.edit(Step::Other, |d, _| page::toggle(d, id));
            }
            KeyCode::Enter => {
                self.edit(Step::Other, |d, c| page::enter(d, c, shift));
            }
            // Alt+Tab is the OS's: the page must not take it for Tab and
            // hand the keyboard to the next control on its way out.
            // In text, a tab character, typed as letters are.
            KeyCode::Tab if !chord && !m.alt && page::tab_types(doc, self.caret, shift) => self.type_text("\t"),
            KeyCode::Tab if !chord && !m.alt => {
                let mut handled = false;
                self.edit(Step::Other, |d, c| {
                    let (h, changed) = page::tab(d, c, shift);
                    handled = h;
                    changed
                });
                if !handled {
                    // Not the page's: Tab leaves it, as it does a field.
                    self.step = Some(!shift);
                }
            }
            KeyCode::Tab => {}
            KeyCode::Escape => {
                let c = self.caret;
                let i = doc.index_of(c.head.slot.block).unwrap_or(0);
                let block = page::select_blocks(doc, i, i);
                if !c.is_empty() && c != block {
                    self.move_to(Caret::at(c.head));
                } else if c != block {
                    // Esc with nothing selected selects the block — the
                    // one Backspace, Ctrl+C or the toolbar then act on.
                    self.move_to(block);
                } else {
                    // And again: the page lets the keyboard go.
                    self.move_to(Caret::at(c.head));
                    self.editing = false;
                    self.blur = true;
                }
            }
            KeyCode::Char('a') | KeyCode::Char('A') if chord => {
                // Once: the slot; twice: the whole note.
                let slot = page::select_slot(doc, self.caret.head.slot);
                let all = page::select_all(doc);
                let next = if self.caret == slot || page::len(doc, slot.head.slot) == 0 {
                    all
                } else {
                    slot
                };
                self.move_to(next);
            }
            KeyCode::Char('c') | KeyCode::Char('C') if chord => {
                self.copy_selection();
            }
            KeyCode::Char('x') | KeyCode::Char('X') if chord && shift => self.page_action("style:2"),
            KeyCode::Char('x') | KeyCode::Char('X') if chord => self.page_action("cut"),
            KeyCode::Char('v') | KeyCode::Char('V') if chord => self.paste(),
            KeyCode::Char('h') if chord && shift => self.page_action("highlight"),
            KeyCode::Char('l') if chord && m.alt => self.page_action("float:1"),
            KeyCode::Char('r') if chord && m.alt => self.page_action("float:2"),
            // The block becomes: Ctrl/Cmd+Alt+0 text, 1–3 headings,
            // 4 checklist, 5 bullets, 6 numbers, 7 a quote.
            KeyCode::Char(d @ '0'..='7') if chord && m.alt => {
                let to = match d {
                    '0' => TURN_TEXT,
                    '1' => TURN_H1,
                    '2' => TURN_H2,
                    '3' => TURN_H3,
                    '4' => TURN_TODO,
                    '5' => TURN_BULLET,
                    '7' => TURN_QUOTE,
                    _ => TURN_NUMBER,
                };
                self.on_plain(Msg::Turn { to });
            }
            KeyCode::Char('8') if chord && m.alt => self.on_plain(Msg::ToggleInitial),
            KeyCode::Char('t') if chord && m.alt => self.on_plain(Msg::InsertTable),
            KeyCode::Char('-') if chord && m.alt => self.on_plain(Msg::InsertDivider),
            KeyCode::Char('d') if chord => {
                self.edit(Step::Other, page::duplicate);
            }
            KeyCode::Char('k') if chord && shift => self.on_plain(Msg::DeleteBlock),
            KeyCode::Char('b') | KeyCode::Char('B') if chord => self.page_action("style:0"),
            KeyCode::Char('i') | KeyCode::Char('I') if chord => self.page_action("style:1"),
            KeyCode::Char('u') | KeyCode::Char('U') if chord => self.page_action("style:4"),
            // With Shift too: ⌘⇧E is code as well, as hands that know it
            // from elsewhere press it.
            KeyCode::Char('e') | KeyCode::Char('E') if chord => self.page_action("style:3"),
            KeyCode::Char('k') | KeyCode::Char('K') if chord => self.page_action("link"),
            _ if chord => {
                self.chord(&k);
            }
            _ => {
                if let Some(s) = k.text.as_deref().filter(|s| !s.is_empty()) {
                    let s = s.to_string();
                    // A typed letter is writing, as text from an input
                    // method is: the colour picker left behind.
                    self.tint_open = None;
                    self.typing = true;
                    self.typed_at = core.now();
                    self.type_text(&s);
                }
            }
        }
    }

    /// The `/` menu, when the caret ends a `/word` line it has not been
    /// sent away from: the line, and the commands it offers.
    pub(super) fn slash_now(&self) -> Option<(u64, Vec<usize>)> {
        if !self.editing {
            return None;
        }
        let (block, word) = slash::query(self.doc(), self.caret)?;
        if self.slash_off == Some(block) {
            return None;
        }
        let shown = slash::matches(&word);
        (!shown.is_empty()).then_some((block, shown))
    }

    /// Runs a `/` command on the line it was typed on.
    pub(super) fn run_slash(&mut self, i: usize) {
        let picture = slash::COMMANDS
            .get(i)
            .is_some_and(|c| c.action == slash::Action::Picture);
        self.focus_page = true;
        self.edit(Step::Other, |d, c| slash::apply(d, c, i));
        self.slash_sel = 0;
        if picture {
            self.pick_pictures = true;
        }
    }

    /// The `/` menu under its line: each command's icon, name and what
    /// to type for it next time; the highlighted one on the accent's wash.
    fn slash_menu(&mut self, ui: &mut Ui<'_>, l: &Look, id: u64, shown: &[usize]) {
        let t = l.t;
        let sel = self.slash_sel.min(shown.len() - 1);
        // Under the line, or over it when the page has no room below: never
        // over the line being typed.
        let tall = 12.0 + shown.len() as f32 * 35.0;
        let above = match (self.row_rects.iter().find(|(b, _)| *b == id).map(|(_, r)| *r), self.page_rect) {
            (Some(r), Some(p)) => r.y + r.h + tall + 8.0 > p.y + p.h && r.y - tall - 8.0 >= p.y,
            _ => false,
        };
        let (at, me, dy) = if above { (Align::Start, Align::End, -2.0) } else { (Align::End, Align::Start, 2.0) };
        ui.with_keyed(
            "slash",
            NodeSpec::column()
                .float(FloatConfig::parent().at(Align::Start, at).self_at(Align::Start, me).offset(-16.0, dy).fit())
                .width(280.0)
                .pad(6.0)
                .gap(1.0)
                .radius(radius::CARD)
                .bg(l.card)
                .border(1.0, t.border)
                .shadow_color(l.shadow)
                .shadow_blur(18.0)
                .shadow_y(4.0)
                // Out of the line it hangs from, quickly.
                .enter(kui_native::Enter::from(0.0, if above { 4.0 } else { -4.0 }).opacity(0.0))
                .transition(140.0)
                .role(Role::List)
                .label("Blocks"),
            |ui| {
                for (row, &i) in shown.iter().enumerate() {
                    let cmd = &slash::COMMANDS[i];
                    let on = row == sel;
                    ui.with_indexed(
                        i as u64,
                        NodeSpec::row()
                            .grow_width()
                            .height(34.0)
                            .pad_xy(10.0, 0.0)
                            .gap(10.0)
                            .radius(radius::CONTROL)
                            .cross_align(Align::Center)
                            .bg(if on { t.accent_soft } else { Color::TRANSPARENT })
                            .hover_bg(if on { t.accent_soft } else { t.hover })
                            .on_click(Msg::Slash { index: i as u64 })
                            .selected(on)
                            .label(cmd.name),
                        |ui| {
                            icons::icon(ui, cmd.glyph, if on { t.fg } else { t.muted }, 18.0, false);
                            ui.text_in(
                                NodeSpec::row().grow_width().min_width(0.0),
                                cmd.name,
                                TextStyle::new(Type::STRONG.size).color(t.fg).nowrap(),
                            );
                            // A picture comes by pasting: its shortcut as this
                            // platform writes it.
                            let hint = if cmd.action == slash::Action::Picture { keys::label(keys::PASTE) } else { cmd.hint.to_string() };
                            ui.text(&hint, TextStyle::new(Type::SMALL.size).mono().color(t.muted).nowrap());
                        },
                    );
                }
            },
        );
    }

    /// Whether the picture at the caret was given a width.
    fn picture_width_set(&self) -> bool {
        page::touched(self.doc(), self.caret)
            .next()
            .is_some_and(|i| matches!(self.doc().blocks()[i].kind, Kind::Image { width: Some(_), .. }))
    }

    /// Ctrl+K: the link editor, on the link the caret is in or for the
    /// selection (a word, with none) to become one.
    fn open_link_editor(&mut self) {
        if self.caret.is_empty() && page::link_at(self.doc(), self.caret).is_none() {
            let w = page::word_at(self.doc(), self.caret.head);
            if !w.is_empty() {
                self.caret = w;
            }
        }
        let existing = page::link_at(self.doc(), self.caret);
        self.link_edit = Some(LinkEdit {
            caret: self.caret,
            // Shown with no bidi control in it to turn it round.
            url: existing.as_ref().map(|l| inline::without_bidi(&l.2)).unwrap_or_default(),
            existing: existing.is_some(),
            at: None,
        });
    }

    /// The link editor's buttons, and Enter in it.
    pub(super) fn link_act(&mut self, what: u64) {
        let Some(le) = self.link_edit.clone() else { return };
        let url = le.url.trim().to_string();
        self.caret = page::fix(self.doc(), le.caret);
        match what {
            1 => {
                if !url.is_empty() {
                    self.open_url(&url);
                }
                return;
            }
            2 => {
                if let Some((slot, at, _)) = page::link_at(self.doc(), self.caret) {
                    self.edit(Step::Other, |d, _| page::set_link(d, slot, at, None));
                }
            }
            _ if url.is_empty() => {}
            _ if le.existing => {
                if let Some((slot, at, _)) = page::link_at(self.doc(), self.caret) {
                    self.edit(Step::Other, |d, _| page::set_link(d, slot, at, Some(&url)));
                }
            }
            _ => {
                self.edit(Step::Other, |d, c| page::make_link(d, c, &url));
            }
        }
        self.link_edit = None;
        self.focus_page = true;
        self.editing = true;
    }

    /// A link's address opened by the system: a web page or a mail. One
    /// written without a scheme is given one (`example.com` the web's);
    /// anything else stays shut, as does a mail link that would attach a
    /// file of this computer's (export.rs `safe_href`), whatever opens it.
    pub(super) fn open_url(&mut self, url: &str) {
        // A file in the notes folder (a PDF or a recording an export
        // brought): opened in its own app, or, one that could run, shown
        // in its folder instead.
        let folder = self.store.notes[self.current].folder.clone();
        if let Some(path) = self.store.linked_file(&folder, url) {
            let opened = if store::opens_safely(&path) {
                store::open_url(&path.to_string_lossy())
            } else {
                store::reveal(&path)
            };
            if let Err(e) = opened {
                self.error = Some(format!("Couldn't open the file: {}", store::plain_error(&e)));
            }
            return;
        }
        // Read as a browser reads it, given its scheme, then read again:
        // `me@example.com?attach=…` is a mail link only once it has one.
        // What opens is the address as read, not as written.
        let safe = carnet_core::export::safe_href(url)
            .map(|u| inline::with_scheme(&u))
            .and_then(|u| carnet_core::export::safe_href(&u))
            .filter(|u| ["http://", "https://", "mailto:"].iter().any(|s| u.to_ascii_lowercase().starts_with(s)));
        let Some(url) = safe else {
            let url = inline::with_scheme(url);
            self.error = Some(if url.to_ascii_lowercase().starts_with("mailto:") {
                "A mail link that attaches a file doesn't open from a note".to_string()
            } else {
                format!("Only web and mail links open from a note: {url}")
            });
            return;
        };
        if let Err(e) = store::open_url(&url) {
            self.error = Some(format!("Couldn't open the link: {e}"));
        }
    }

    /// The link editor: the address, Enter to keep it, a way to open the
    /// link and to take it off its words. Hangs under the caret.
    pub(super) fn link_editor(&mut self, ui: &mut Ui<'_>, l: &Look) {
        let Some(le) = self.link_edit.clone() else { return };
        let t = l.t;
        // Its first frame: placed now, and its field given this link's
        // address. kui keeps a field's text while it is away, so it would
        // show the last link's (a `javascript:` one beside this one's
        // Open, say).
        let fresh = le.at.is_none();
        let at = match le.at {
            Some(at) => at,
            None => {
                let p = self.caret.head;
                let at = self
                    .rect_at(ui.core(), p.slot, p.byte)
                    .map_or(self.menu_at, |r| Vec2::new(r.x - 12.0, r.y + r.h + 8.0));
                if let Some(le) = self.link_edit.as_mut() {
                    le.at = Some(at);
                }
                at
            }
        };
        let mut field = None;
        ui.with(
            NodeSpec::column()
                .float(FloatConfig::viewport().offset(at.x, at.y).fit())
                .width(360.0)
                .pad(12.0)
                .gap(10.0)
                .radius(radius::CARD)
                .bg(l.card)
                .border(1.0, t.border)
                .shadow_color(l.shadow)
                .shadow_blur(18.0)
                .shadow_y(4.0)
                .modal(Msg::CloseLink)
                .role(Role::Dialog)
                .label("Link"),
            |ui| {
                ui.text(if le.existing { "Link" } else { "Make a link" }, TextStyle::new(Type::SMALL.size).color(t.muted).nowrap());
                ui.with(NodeSpec::row().grow_width().pad_xy(10.0, 7.0).radius(radius::CONTROL).bg(t.sunken), |ui| {
                    field = Some(ui.text_edit(
                        "link-url",
                        &le.url,
                        &EditOptions { style: TextStyle::new(Type::UI.size).color(t.fg), ..Default::default() },
                        NodeSpec::row().grow_width().label("Link address"),
                    ));
                });
                ui.with(NodeSpec::row().grow_width().gap(6.0).cross_align(Align::Center), |ui| {
                    let button = |ui: &mut Ui<'_>, key: &str, name: &str, what: u64, primary: bool| {
                        ui.text_in(
                            NodeSpec::row()
                                .pad_xy(12.0, 6.0)
                                .radius(radius::CONTROL)
                                .bg(if primary { l.c.primary } else { Color::TRANSPARENT })
                                .border(1.0, if primary { l.c.primary } else { t.border })
                                .hover_bg(if primary { l.c.primary } else { t.hover })
                                .on_click(Msg::LinkAct { what })
                                .role(Role::Button)
                                .label(key),
                            name,
                            TextStyle::new(Type::SMALL.size).color(if primary { l.c.on_accent } else { t.fg }).nowrap(),
                        );
                    };
                    if le.existing {
                        button(ui, "link-remove", "Remove link", 2, false);
                    }
                    // Only a link a note may follow (export.rs `safe_href`).
                    if carnet_core::export::safe_href(&le.url).is_some() {
                        button(ui, "link-open", "Open", 1, false);
                    }
                    ui.leaf(NodeSpec::row().grow_width());
                    button(ui, "link-done", "Done", 0, true);
                });
            },
        );
        if let Some(k) = field {
            if fresh {
                ui.set_edit_text(k, &le.url);
            }
            self.fields.insert(k, Field::Link);
            self.apply_focus(ui, k, Field::Link);
        }
    }

    /// The table the caret is in.
    pub(super) fn table_here(&self) -> Option<&Table> {
        let doc = self.doc();
        self.caret.head.slot.cell?;
        match &doc.blocks()[doc.index_of(self.caret.head.slot.block)?].kind {
            Kind::Table(t) => Some(t),
            _ => None,
        }
    }

    /// The picture the caret is on, or the one the selection is: its id,
    /// where it sits and its width as last drawn.
    pub(super) fn picture_here(&self) -> Option<(u64, Float, f32)> {
        let doc = self.doc();
        let b = &doc.blocks()[page::picture_at(doc, self.caret)?];
        match &b.kind {
            Kind::Image { float, .. } => Some((b.id, *float, self.pic_w.get(&b.id).map_or(0.0, |w| w.0))),
            _ => None,
        }
    }

    /// The picture the caret is on, or the one the selection is: its `src`,
    /// and whether an Obsidian embed names it.
    pub(super) fn picture_at_caret(&self) -> Option<(String, bool)> {
        let doc = self.doc();
        match &doc.blocks()[page::picture_at(doc, self.caret)?].kind {
            Kind::Image { src, wiki, .. } => Some((src.clone(), *wiki)),
            _ => None,
        }
    }

    /// Copies what is selected: a picture alone as the picture, anything
    /// else as Markdown with its HTML. True when something was copied.
    fn copy_selection(&mut self) -> bool {
        if let Some((src, wiki)) = self.picture_at_caret() {
            self.copy_picture(&src, wiki);
            return true;
        }
        match page::copy(self.doc(), self.caret) {
            Some(md) => {
                let folder = self.store.notes[self.current].folder.clone();
                // Lines of code are HTML's code too (`page::copy_for_html`).
                let html_md = page::copy_for_html(self.doc(), self.caret).unwrap_or_else(|| md.clone());
                self.copy_markdown_as(md, &html_md, &folder);
                true
            }
            None => false,
        }
    }

    /// A picture's pixels on the clipboard.
    fn copy_picture(&mut self, src: &str, wiki: bool) {
        let folder = self.store.notes[self.current].folder.clone();
        let result = match self.store.picture_file(&folder, src, wiki) {
            Some(p) => images::decode(&p).and_then(|d| clip::write_image(d.w, d.h, d.rgba)),
            None => Err("it is on the web".into()),
        };
        match result {
            Ok(()) => self.say = Some("Picture copied".into()),
            Err(e) => self.error = Some(format!("Couldn't copy the picture: {e}")),
        }
    }

    /// Ctrl+V: a picture or picture files from the clipboard go in as
    /// pictures; anything else is text, which kui hands over.
    pub(super) fn paste(&mut self) {
        match clip::read_media() {
            Some(clip::Media::Image { w, h, rgba }) => self.paste_picture(w, h, &rgba),
            Some(clip::Media::Files(files)) => self.insert_files(&files),
            None => self.paste_pending = true,
        }
    }

    /// A picture's pixels kept as a PNG beside the notes, and put in at the
    /// caret.
    pub fn paste_picture(&mut self, w: u32, h: u32, rgba: &[u8]) {
        if self.reading() {
            return;
        }
        let folder = self.store.notes[self.current].folder.clone();
        let saved = images::encode_png(w, h, rgba)
            .and_then(|png| self.store.save_attachment(&folder, "png", &png).map_err(|e| e.to_string()));
        match saved {
            Ok(src) => {
                // The pixels are in hand: shown at once, not read back.
                if let Some(p) = self.store.resolve(&folder, &src) {
                    self.images.preload(&p, images::Decoded { w, h, rgba: rgba.to_vec() });
                }
                self.insert_pictures(vec![src], None);
            }
            Err(e) => self.error = Some(format!("Couldn't keep the picture: {e}")),
        }
    }

    /// Files pasted or picked: the pictures among them go in at the caret.
    pub(super) fn insert_files(&mut self, paths: &[std::path::PathBuf]) {
        self.insert_files_at(paths, None);
    }

    /// Files dropped or pasted: the pictures among them are copied in
    /// beside the notes and put in before block `at` (the drop line), or
    /// at the caret.
    pub(super) fn insert_files_at(&mut self, paths: &[std::path::PathBuf], at: Option<usize>) {
        if self.reading() {
            return;
        }
        let folder = self.store.notes[self.current].folder.clone();
        let mut srcs = Vec::new();
        let mut skipped = 0;
        for p in paths {
            if !images::is_image(p) {
                skipped += 1;
                continue;
            }
            // An iPhone's photo goes in as a JPEG, which shows everywhere.
            #[cfg(target_os = "macos")]
            if images::is_heic(p) {
                let saved = images::heic_to_jpeg(p)
                    .and_then(|jpeg| self.store.save_attachment_as(&folder, &store::stem_of(p), "jpg", &jpeg).map_err(|e| e.to_string()));
                match saved {
                    Ok(src) => srcs.push(src),
                    Err(e) => self.error = Some(format!("Couldn't keep {}: {e}", p.display())),
                }
                continue;
            }
            match self.store.import_attachment(&folder, p) {
                Ok(src) => srcs.push(src),
                Err(e) => self.error = Some(format!("Couldn't copy {}: {e}", p.display())),
            }
        }
        if skipped > 0 {
            self.notify(
                if skipped == 1 { "Left out a file that isn't a picture".to_string() }
                else { format!("Left out {skipped} files that aren't pictures") },
                None,
            );
        }
        if !srcs.is_empty() {
            self.insert_pictures(srcs, at);
        }
    }

    fn insert_pictures(&mut self, srcs: Vec<String>, at: Option<usize>) {
        self.editing = true;
        self.focus_page = true;
        // The caret in a table's cell: among its words, one after another.
        if at.is_none() && self.caret.head.slot.cell.is_some() {
            self.edit(Step::Other, |d, c| {
                let mut changed = false;
                for src in &srcs {
                    changed |= page::insert_cell_picture(d, c, src);
                }
                changed
            });
            return;
        }
        let kinds: Vec<Kind> = srcs
            .into_iter()
            .map(|src| Kind::Image { src, alt: String::new(), width: None, float: carnet_core::doc::Float::None, wiki: false })
            .collect();
        self.edit(Step::Other, |d, c| match at {
            Some(i) => page::insert_blocks_at(d, c, i, kinds),
            None => {
                let mut changed = false;
                for kind in kinds {
                    changed |= page::insert_block(d, c, kind);
                }
                changed
            }
        });
    }

    /// Up and Down `times` visual lines: within a wrapped paragraph by its
    /// laid-out rows, then on to the next block — in a table, the same
    /// column of the next row — keeping to the x the caret started from.
    /// Whether the caret is at the top of the note: on its first line (or,
    /// `start`, at its very start), with nothing above it but the title.
    fn at_top(&self, core: &Core, start: bool) -> bool {
        let doc = self.doc();
        let p = self.caret.head;
        let first = page::first(doc);
        if start {
            return p == first;
        }
        if !page::vertical(doc, p.slot, true).is_empty() || p.slot != first.slot {
            return false;
        }
        // On the slot's first row.
        match (self.rect_at(core, p.slot, p.byte), self.rect_at(core, p.slot, 0)) {
            (Some(r), Some(f)) => r.y <= f.y + 1.0,
            _ => true,
        }
    }

    /// The keyboard to the title: its caret at the end, or under `column`.
    fn up_to_title(&mut self, column: Option<f32>) {
        self.typing = false;
        self.title_to = Some(column);
        self.focus(Field::Title);
    }

    /// Where the note's first line is under `x`: the first block's top row,
    /// at the letter nearest that column.
    pub(super) fn first_line_at(&self, core: &Core, x: f32) -> Option<Pos> {
        let first = page::first(self.doc());
        let top = self.rect_at(core, first.slot, 0)?;
        let byte = self.byte_under(core, first.slot, Vec2::new(x, top.y + top.h * 0.5))?;
        Some(page::fix(self.doc(), Caret::at(Pos { slot: first.slot, byte })).head)
    }

    fn vertical(&mut self, core: &Core, up: bool, extend: bool, times: usize) {
        // A divider or a picture come to is chosen whole, as ← and →
        // choose one, and the next press is past it.
        let from = page::vertical_from(self.doc(), self.caret, up, extend);
        let mut head = from.head;
        for _ in 0..times {
            head = self.vertical_once(core, head, up);
        }
        self.caret = page::vertical_to(self.doc(), from, head, up, extend);
        self.writing.moved();
        self.history().seal();
        self.reveal = true;
    }

    fn vertical_once(&mut self, core: &Core, p: Pos, up: bool) -> Pos {
        let key = self.slot_keys.get(&p.slot).copied();
        let r = self.rect_at(core, p.slot, p.byte);
        let x = match (self.goal_x, r) {
            (Some(x), _) => x,
            (None, Some(r)) => r.x,
            (None, None) => 0.0,
        };
        self.goal_x = Some(x);
        let doc = self.doc();
        let clamp = |p: Pos| page::fix(doc, Caret::at(p)).head;
        // Another row of this paragraph above or below? In code, a row
        // between its fences: the fences are drawn, not stood on, so ↓
        // from its last line is on to the block below.
        if let (Some(_), Some(r)) = (key, r) {
            let (lo, hi) = page::writable(doc, p.slot);
            let more = if up {
                self.rect_at(core, p.slot, lo).is_some_and(|f| r.y > f.y + 1.0)
            } else {
                self.rect_at(core, p.slot, hi).is_some_and(|e| r.y + 1.0 < e.y)
            };
            if more {
                let y = if up { r.y - r.h * 0.5 } else { r.y + r.h * 1.5 };
                if let Some(byte) = self.byte_under(core, p.slot, Vec2::new(x, y)) {
                    return clamp(Pos { slot: p.slot, byte: byte.clamp(lo, hi) });
                }
            }
        }
        let cands = page::vertical(doc, p.slot, up);
        if cands.is_empty() {
            // The first line up, the last down: to its start, or its end.
            let (lo, hi) = page::writable(doc, p.slot);
            return Pos { slot: p.slot, byte: if up { lo } else { hi } };
        }
        // Of a table row, the cell under the caret's x.
        let left_of = |s: &Slot| self.rect_at(core, *s, 0).is_some_and(|o| o.x <= x + 1.0);
        let pick = if cands.len() == 1 {
            cands[0]
        } else {
            cands.iter().rev().find(|s| left_of(s)).copied().unwrap_or(cands[0])
        };
        if !self.slot_keys.contains_key(&pick) {
            return page::start(pick);
        }
        // Its last row coming up, its first going down (in code, the
        // last and first between its fences).
        let (lo, hi) = page::writable(doc, pick);
        let edge = self.rect_at(core, pick, if up { hi } else { lo });
        let byte = edge
            .and_then(|e| self.byte_under(core, pick, Vec2::new(x, e.y + e.h * 0.5)))
            .unwrap_or(if up { hi } else { lo });
        clamp(Pos { slot: pick, byte: byte.clamp(lo, hi) })
    }

    /// Home and End: the start or end of the visual line the caret is on.
    fn line_edge(&mut self, core: &Core, home: bool, extend: bool) {
        let p = self.caret.head;
        let doc = self.doc();
        let (lo, hi) = page::writable(doc, p.slot);
        let byte = (|| {
            let r = self.rect_at(core, p.slot, p.byte)?;
            let o = self.rect_at(core, p.slot, lo)?;
            let x = if home { o.x - 1.0 } else { o.x + 100_000.0 };
            self.byte_under(core, p.slot, Vec2::new(x, r.y + r.h * 0.5))
        })()
        .unwrap_or(if home { lo } else { hi })
        .clamp(lo, hi);
        let head = page::fix(doc, Caret::at(Pos { slot: p.slot, byte })).head;
        let anchor = if extend { self.caret.anchor } else { head };
        self.goal_x = None;
        self.move_to(Caret { head, anchor });
    }

    /// The page's pointer: in text, a press places the caret (two take a
    /// word, three the line) and a drag selects — across blocks and cells,
    /// since every block is part of this one editor; in the margin, a drag
    /// selects whole blocks and a click puts the caret in the block beside
    /// it. Where a press lands is the page's own question (`hit`): kui's
    /// `line` is the row a press is level with, the same for every cell of
    /// a table row and for the margin beside it.
    pub(super) fn page_drag(&mut self, ev: &UiEvent, core: &Core) {
        let p = &ev.payload;
        let num = |k: &str| {
            p.get_float(k)
                .or_else(|| p.get_int(k).map(|v| v as f64))
                .unwrap_or(0.0) as f32
        };
        let (x, y) = (num("x"), num("y"));
        let clicks = p.get_int("clicks").unwrap_or(1);
        // A cell's picture pressed: carried, not a selection.
        if p.get_str("phase") != Some("start") && self.pic_drag.as_ref().is_some_and(|d| matches!(d.what, Carried::Cell(..))) {
            self.carry(core, p.get_str("phase"), Vec2::new(x, y));
            self.reveal = false;
            return;
        }
        match p.get_str("phase") {
            Some("start") => {
                self.focus_page = true;
                self.editing = true;
                self.goal_x = None;
                self.preedit = None;
                self.writing.moved();
                self.history().seal();
                // A link opens as the settings say: Ctrl+click (⌘ on a
                // Mac), a click once let go, or a double-click. A ⌘-click
                // on one that won't open says why; a click or two on it
                // do what they do anywhere.
                let m = core.modifiers();
                let cmd = m.ctrl || m.super_key;
                let how = self.settings.link_open;
                self.link_press = None;
                let pos = self.hit(core, x, y).filter(|_| !self.in_margin(x, y));
                let url = |doc: &Doc| match pos {
                    Some(pos) if cmd => page::link_at(doc, Caret::at(pos)).map(|l| l.2),
                    Some(pos) => page::link_under(doc, pos, how),
                    None => None,
                };
                match how.opens(clicks.max(0) as u32, cmd, m.shift) {
                    Some(page::OpensOn::Press) => {
                        if let Some(url) = url(self.doc()) {
                            self.drag = None;
                            self.open_url(&url);
                            return;
                        }
                    }
                    Some(page::OpensOn::Release) => {
                        let caret = self.caret;
                        self.link_press = url(self.doc()).map(|url| LinkPress { url, at: Vec2::new(x, y), caret });
                    }
                    None => {}
                }
                // A press on a picture among a cell's words chooses it,
                // whole; a drag carries it, as one on its own line.
                if clicks == 1
                    && !core.modifiers().shift
                    && let Some((slot, a, b)) = self.cell_picture_under(core, x, y)
                {
                    self.caret = Caret { anchor: Pos { slot, byte: a }, head: Pos { slot, byte: b } };
                    self.drag = None;
                    let at = Vec2::new(x, y);
                    self.pic_drag = Some(PicDrag { what: Carried::Cell(slot, a), from: at, at, moved: false, target: None });
                    self.reveal = false;
                    return;
                }
                match self.hit(core, x, y).filter(|_| !self.in_margin(x, y)) {
                    Some(pos) => {
                        self.caret = match clicks {
                            2 => page::word_at(self.doc(), pos),
                            n if n >= 3 => page::select_slot(self.doc(), pos.slot),
                            _ if core.modifiers().shift => Caret {
                                head: pos,
                                anchor: self.caret.anchor,
                            },
                            _ => Caret::at(pos),
                        };
                        let unit = (clicks >= 2).then_some((self.caret, clicks == 2));
                        self.drag = Some(Drag::Text { unit });
                    }
                    None => self.drag = Some(Drag::Margin { y0: y, moved: false, from: self.block_at(y) }),
                }
                self.drag_at = Some((Vec2::new(x, y), Vec2::new(x, y), core.now()));
            }
            Some("move") => {
                if let Some((at, _, _)) = self.drag_at.as_mut() {
                    *at = Vec2::new(x, y);
                }
                // Dragged from a link: a selection, not a click on it.
                if self.link_press.as_ref().is_some_and(|lp| (lp.at.x - x).hypot(lp.at.y - y) >= 4.0) {
                    self.link_press = None;
                }
                self.drag_to(core, x, y);
            }
            _ => {
                self.drag_at = None;
                // A click on a link, let go where it was pressed: opened,
                // and the caret left where it was.
                if let Some(lp) = self.link_press.take()
                    && (lp.at.x - x).hypot(lp.at.y - y) < 4.0
                {
                    self.caret = lp.caret;
                    self.drag = None;
                    self.reveal = false;
                    self.open_url(&lp.url);
                    return;
                }
                if let Some(Drag::Margin { y0, moved: false, .. }) = self.drag {
                    // A click in the margin: the caret into the block
                    // beside it, at its end; below them all, the last.
                    if let Some(i) = self.block_at(y0) {
                        let doc = self.doc();
                        let slots = page::block_slots(&doc.blocks()[i]);
                        let pos = page::end(doc, *slots.last().unwrap_or(&slots[0]));
                        self.caret = Caret::at(pos);
                    }
                }
                self.drag = None;
            }
        }
        self.reveal = false;
    }

    /// The selection's moving end, for a drag now at `x`, `y`: in text, to
    /// the letter there (by words or lines for a drag begun with two or
    /// three clicks); in the margin, whole blocks.
    fn drag_to(&mut self, core: &Core, x: f32, y: f32) {
        match self.drag {
            Some(Drag::Text { unit }) => {
                if let Some(pos) = self.hit(core, x, y) {
                    self.caret = match unit {
                        Some((clicked, words)) => page::extend_by(self.doc(), clicked, pos, words),
                        None => Caret { head: pos, ..self.caret },
                    };
                }
            }
            // Past a few pixels, a press in the margin is a drag.
            Some(Drag::Margin { y0, moved, from }) if moved || (y - y0).abs() >= 4.0 => {
                self.drag = Some(Drag::Margin { y0, moved: true, from });
                if let (Some(i), Some(j)) = (from, self.block_at(y)) {
                    self.caret = page::select_blocks(self.doc(), i, j);
                }
            }
            _ => {}
        }
    }

    /// A selection dragged to the page's top or bottom edge, or past it:
    /// the page scrolls, faster the further the pointer goes — from a
    /// band inside the edge, up to a cap — and goes on while the pointer
    /// is held still, the moving end following the words under it. kui's
    /// own rate: 10 px a second for each px, 1000 at most.
    pub(super) fn drag_scroll(&mut self, ui: &mut Ui<'_>, key: Key) {
        const BAND: f32 = 24.0;
        const REACH: f32 = 100.0;
        const RATE: f32 = 10.0;
        let (Some((at, from, then)), Some(r), Some(_)) = (self.drag_at, self.page_rect, self.drag) else { return };
        // A press near the edge that has not moved is a click, not a drag.
        if (at.x - from.x).hypot(at.y - from.y) < 4.0 || matches!(self.drag, Some(Drag::Margin { moved: false, .. })) {
            return;
        }
        let past = if at.y < r.y + BAND {
            -(r.y + BAND - at.y)
        } else if at.y > r.y + r.h - BAND {
            at.y - (r.y + r.h - BAND)
        } else {
            0.0
        };
        let now = ui.now();
        self.drag_at = Some((at, from, now));
        let Some(g) = ui.scroll_geometry(key).filter(|_| past != 0.0) else { return };
        let dt = (now - then).clamp(0.0, 0.1) as f32;
        let dt = if dt > 0.0 { dt } else { 1.0 / 60.0 };
        let step = past.clamp(-REACH, REACH) * RATE * dt;
        let to = (g.offset.y + step).clamp(0.0, g.max_offset.y.max(0.0));
        if (to - g.offset.y).abs() < 0.01 {
            return;
        }
        // At once, not eased: the page moves at the drag's rate.
        let d = Vec2::new(0.0, to - g.offset.y);
        ui.shift_scroll(key, d, d);
        // The end under the pointer, against the layout as drawn now; the
        // next frame, against the one scrolled.
        let core = ui.core();
        self.drag_to(core, at.x, at.y);
        ui.request_frame();
    }

    /// Whether the pointer is over a link that a press there, with the keys
    /// held now, would open (the settings' way links open): it's a hand.
    pub(super) fn link_hand(&self, ui: &mut Ui<'_>) -> bool {
        let core = ui.core();
        let (Some(p), m) = (core.cursor(), core.modifiers()) else { return false };
        let how = self.settings.link_open;
        if self.drag.is_some() || self.resizing.is_some() || !how.hand(m.ctrl || m.super_key, m.shift) {
            return false;
        }
        self.hit(core, p.x, p.y)
            .filter(|_| !self.in_margin(p.x, p.y))
            .and_then(|pos| page::link_under(self.doc(), pos, how))
            .is_some()
    }

    /// Whether a point is in the page's margin: beside the blocks' rows,
    /// or well below the last.
    fn in_margin(&self, x: f32, y: f32) -> bool {
        self.block_under(x, y)
            .and_then(|i| {
                let id = self.doc().blocks()[i].id;
                self.row_rects.iter().find(|(b, _)| *b == id).map(|(_, r)| *r)
            })
            .is_none_or(|r| x < r.x || x > r.x + r.w || y > r.y + r.h + 40.0)
    }

    /// The caret position under a point (viewport px, last frame's
    /// layout): the block whose row spans `y`; in a table, the cell nearest
    /// the point; the byte `text_hit` finds there, with the point held
    /// inside the text's own rows so padding and gaps land on the nearest
    /// line.
    pub(super) fn hit(&self, core: &Core, x: f32, y: f32) -> Option<Pos> {
        let i = self.block_under(x, y)?;
        let doc = self.doc();
        let b = &doc.blocks()[i];
        let slots = page::block_slots(b);
        let slot = if slots.len() > 1 {
            let dist = |s: &Slot| {
                self.cell_rects.get(s).map_or(f32::MAX, |r| {
                    let dx = (r.x - x).max(0.0).max(x - (r.x + r.w));
                    let dy = (r.y - y).max(0.0).max(y - (r.y + r.h));
                    dx * dx + dy * dy
                })
            };
            slots.iter().copied().min_by(|a, b| dist(a).total_cmp(&dist(b)))?
        } else {
            slots[0]
        };
        if page::is_atomic(doc, slot) {
            return Some(page::start(slot));
        }
        // In code, between its fences: they aren't drawn.
        let (lo, hi) = page::writable(doc, slot);
        let byte = (|| {
            let o = self.rect_at(core, slot, lo)?;
            let e = self.rect_at(core, slot, hi).unwrap_or(o);
            let py = y.clamp(o.y + 1.0, (e.y + e.h - 1.0).max(o.y + 1.0));
            self.byte_under(core, slot, Vec2::new(x.max(o.x), py))
        })()
        .unwrap_or(lo)
        .clamp(lo, hi);
        Some(page::fix(doc, Caret::at(Pos { slot, byte })).head)
    }

    /// The block under a point: the one whose row holds it — beside a
    /// picture at an edge, rows share a height, so `x` tells them apart —
    /// else the one whose row spans `y`. Nearest in `x`, among those
    /// level with the point.
    fn block_under(&self, x: f32, y: f32) -> Option<usize> {
        let level: Vec<&(u64, Rect)> = self.row_rects.iter().filter(|(_, r)| y >= r.y && y < r.y + r.h).collect();
        if level.len() > 1 {
            let dx = |r: &Rect| (r.x - x).max(0.0).max(x - (r.x + r.w));
            let id = level.iter().min_by(|a, b| dx(&a.1).total_cmp(&dx(&b.1)))?.0;
            return self.doc().index_of(id);
        }
        self.block_at(y)
    }

    /// The block whose row spans `y` (viewport px, last frame's layout);
    /// above the first is the first, below the last is the last.
    fn block_at(&self, y: f32) -> Option<usize> {
        let (first, last) = (self.row_rects.first()?, self.row_rects.last()?);
        let id = if y < first.1.y {
            first.0
        } else {
            self.row_rects
                .iter()
                .find(|(_, r)| y < r.y + r.h)
                .unwrap_or(last)
                .0
        };
        self.doc().index_of(id)
    }
}

/// The bullet, number or check in front of a list item, one line tall so
/// it sits on the item's first line. A checkbox takes clicks over the whole
/// marker, not just its 18px ring, and is a checkbox to a screen reader,
/// named by its item.
#[allow(clippy::too_many_arguments)]
fn marker(ui: &mut Ui<'_>, l: &Look, id: u64, style: ListStyle, done: bool, n: u32, depth: usize, text: &str) {
    let t = &l.t;
    let cell = NodeSpec::row().width(l.px(22.0)).height(l.px(BODY_LH)).center();
    match style {
        ListStyle::Bullet => {
            let size = l.px([6.0, 5.0, 4.0][depth % 3]).max(3.0);
            ui.with(cell.role(Role::None), |ui| {
                let dot = NodeSpec::row().size(size, size).radius(size / 2.0);
                ui.leaf(if depth % 3 == 1 {
                    dot.border(1.2, t.accent)
                } else {
                    dot.bg(t.accent)
                });
            });
        }
        ListStyle::Number => {
            ui.text_in(
                cell.main_align(Align::End).role(Role::None),
                &format!("{n}."),
                // The text's own face, size and line, so the number sits
                // on the text's baseline.
                l.page_type(Type::BODY).family(l.body).color(t.muted).nowrap(),
            );
        }
        ListStyle::Todo => {
            let hovered = ui.is_hovered(ui.child_key("check"));
            ui.with_keyed(
                "check",
                cell.on_click(Msg::Toggle { id })
                    .role(Role::Checkbox)
                    .checked(done)
                    .label(if text.trim().is_empty() { "Checklist item" } else { text }),
                |ui| {
                    // The ring fills (and empties) rather than switching.
                    let d = l.px(18.0);
                    let ring = NodeSpec::row().size(d, d).radius(d / 2.0).transition(160.0);
                    let ring = match (done, hovered) {
                        (true, false) => ring.bg(t.accent).border(1.5, t.accent),
                        (true, true) => ring.bg(t.accent_hover).border(1.5, t.accent_hover),
                        // Empty is the accent with no alpha, so filling it
                        // eases through the accent, not through black.
                        (false, false) => ring.bg(t.accent.with_alpha(0.0)).border(1.5, l.ring),
                        (false, true) => ring.bg(t.accent_soft).border(1.5, t.accent),
                    };
                    ui.with(ring, |ui| {
                        // The tick is drawn in the ring's own box, so it sits
                        // where the ring is and not where a glyph's metrics
                        // would put it.
                        if done {
                            ui.polyline(
                                &[Vec2::new(5.2, 9.4), Vec2::new(8.0, 12.2), Vec2::new(12.9, 6.6)].map(|p| Vec2::new(p.x * d / 18.0, p.y * d / 18.0)),
                                Stroke::new((1.7 * d / 18.0).max(1.2), t.on_accent),
                                // The tick comes in with a small give.
                                NodeSpec::row()
                                    .enter(kui_native::Enter::default().scale(0.4).opacity(0.0))
                                    .transition(220.0)
                                    .bounce(0.3),
                            );
                        }
                    });
                },
            );
        }
    }
}

/// How many lines a first letter drops through, for a paragraph that
/// fills `lines` at the page's width: as many, up to `most`; none for one
/// line.
fn drop_lines(most: u32, lines: u32) -> Option<u32> {
    Some(most.min(lines)).filter(|&n| n >= 2)
}

/// What colour words are drawn in. The colour the reader gave them, on
/// a highlighter's wash or not — unless it would all but vanish into what
/// is behind it (black words in the dark, white on paper). Words of no
/// colour of their own on a wash: in whichever of black and white reads
/// on it, when the text's colour would not.
pub(super) fn words_ink(own: Option<Color>, plain: Color, wash: Option<Color>, page: Color) -> Color {
    use crate::design::contrast;
    let behind = wash.unwrap_or(page);
    if let Some(c) = own.filter(|&c| contrast(c, behind) >= 1.6) {
        return c;
    }
    match wash {
        Some(w) if contrast(plain, w) < 3.0 => {
            let (black, white) = (Color::hex(0x1a1a1aff), Color::hex(0xffffffff));
            if contrast(black, w) >= contrast(white, w) { black } else { white }
        }
        _ => plain,
    }
}

/// Focus: a part of the page steps back while another block is written,
/// eased — down slowly enough not to be seen as a change, up at once.
/// Nothing at all when focus is off.
/// `text` cut after a whole word, "…" after it, to fit `room` as `width`
/// measures it; `None` when it fits whole (or is one word: nothing to cut
/// at). Its first word always stays.
pub(super) fn fit_words(text: &str, room: f32, mut width: impl FnMut(&str) -> f32) -> Option<String> {
    let ends: Vec<usize> = text.char_indices().filter(|(_, c)| *c == ' ').map(|(i, _)| i).collect();
    if ends.is_empty() || width(text) <= room {
        return None;
    }
    // Up to a word's end, without the stop or dash that led to the next.
    let cut = |end: usize| {
        let kept = text[..end].trim_end_matches(|c: char| c.is_whitespace() || matches!(c, ':' | ';' | ',' | '\u{b7}' | '\u{2014}' | '('));
        format!("{kept}\u{2026}")
    };
    // The longest that fits, by halving: more words are never narrower.
    let (mut lo, mut hi, mut best) = (1, ends.len(), cut(ends[0]));
    while lo < hi {
        let mid = (lo + hi) / 2;
        let s = cut(ends[mid]);
        if width(&s) <= room {
            best = s;
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Some(best)
}

fn step_back(spec: NodeSpec, focus: bool, dim: bool) -> NodeSpec {
    if !focus {
        return spec;
    }
    spec.opacity(if dim { 0.32 } else { 1.0 }).transition(if dim { 600.0 } else { 160.0 })
}

/// Where dropped files will go: a line in the accent across the page, a
/// ring at its start. It takes no room, so nothing moves under it.
fn drop_line(ui: &mut Ui<'_>, l: &Look) {
    let t = l.t;
    ui.with_keyed("drop-line", NodeSpec::row().grow_width().height(0.0).role(Role::None), |ui| {
        ui.leaf(
            NodeSpec::row()
                .grow_width()
                .height(3.0)
                .radius(1.5)
                .bg(t.accent)
                .float(FloatConfig::parent().offset(0.0, -1.5).clipped()),
        );
        ui.leaf(
            NodeSpec::row()
                .size(10.0, 10.0)
                .radius(5.0)
                .bg(l.card)
                .border(2.5, t.accent)
                .float(FloatConfig::parent().offset(-6.0, -5.0).clipped()),
        );
    });
}
