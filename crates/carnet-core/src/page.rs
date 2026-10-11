//! The page editor's model: where the caret is, and every edit the keyboard
//! makes, as plain functions over a [`Doc`]. Nothing here draws or measures
//! — the view turns keys and clicks into these calls, and keeps the motions
//! that need laid-out text (up, down, home, end) for itself.
//!
//! A caret stands in a *slot*: a block's text, one cell of a table, or a
//! divider, which has no text but can be stood on, selected and removed — it
//! counts one unit wide. A selection runs from an anchor to a head that may
//! be in different slots, so it can cross blocks: whole blocks in the middle,
//! and the part of the first and last that it covers.

use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;

use crate::doc::{self, Block, Doc, Kind, ListStyle};
use crate::inline::{self, Visible};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Slot {
    pub block: u64,
    /// For a table: the cell's (row id, column id).
    pub cell: Option<(u64, u64)>,
}

impl Slot {
    pub fn block(block: u64) -> Self {
        Self { block, cell: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub slot: Slot,
    pub byte: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    pub head: Pos,
    pub anchor: Pos,
}

impl Caret {
    pub fn at(p: Pos) -> Self {
        Self { head: p, anchor: p }
    }

    pub fn is_empty(&self) -> bool {
        self.head == self.anchor
    }
}

// ------------------------------------------------------------ slots

/// Every slot, in reading order: blocks top to bottom, a table's cells row
/// by row.
pub fn slots(doc: &Doc) -> Vec<Slot> {
    let mut out = Vec::new();
    for b in doc.blocks() {
        slots_of(b, &mut out);
    }
    out
}

fn slots_of(b: &Block, out: &mut Vec<Slot>) {
    match &b.kind {
        Kind::Table(t) => {
            for row in &t.rows {
                for &col in &t.cols {
                    out.push(Slot {
                        block: b.id,
                        cell: Some((row.id, col)),
                    });
                }
            }
        }
        _ => out.push(Slot::block(b.id)),
    }
}

/// The slots of one block.
pub fn block_slots(b: &Block) -> Vec<Slot> {
    let mut out = Vec::new();
    slots_of(b, &mut out);
    out
}

fn block_of(doc: &Doc, slot: Slot) -> Option<&Block> {
    doc.get(slot.block)
}

/// A divider's or an image's slot: no text, length 1, taken whole.
pub fn is_atomic(doc: &Doc, slot: Slot) -> bool {
    block_of(doc, slot).is_some_and(|b| b.kind.is_atomic())
}

/// A slot's text; a divider's, or a slot that is gone, is empty.
pub fn text(doc: &Doc, slot: Slot) -> &str {
    let Some(b) = block_of(doc, slot) else { return "" };
    match (&b.kind, slot.cell) {
        (Kind::Table(t), Some((row, col))) => t
            .row_index(row)
            .zip(t.col_index(col))
            .map_or("", |(r, c)| t.rows[r].cells[c].as_str()),
        _ => b.text_ref().unwrap_or(""),
    }
}

fn text_mut(doc: &mut Doc, slot: Slot) -> Option<&mut String> {
    let b = doc.get_mut(slot.block)?;
    match slot.cell {
        Some((row, col)) => match &mut b.kind {
            Kind::Table(t) => {
                let (r, c) = (t.row_index(row)?, t.col_index(col)?);
                Some(&mut t.rows[r].cells[c])
            }
            _ => None,
        },
        // A divider has no text: `text_mut` says so.
        None => b.text_mut(),
    }
}

/// How far a caret can go in a slot: its text's length, or one for a
/// divider.
pub fn len(doc: &Doc, slot: Slot) -> usize {
    if is_atomic(doc, slot) {
        1
    } else {
        text(doc, slot).len()
    }
}

pub fn start(slot: Slot) -> Pos {
    Pos { slot, byte: 0 }
}

pub fn end(doc: &Doc, slot: Slot) -> Pos {
    Pos {
        slot,
        byte: len(doc, slot),
    }
}

fn index(slots: &[Slot], s: Slot) -> usize {
    slots.iter().position(|x| *x == s).unwrap_or(0)
}

/// The caret's two ends, earlier first.
pub fn ordered(doc: &Doc, c: Caret) -> (Pos, Pos) {
    let slots = slots(doc);
    let key = |p: Pos| (index(&slots, p.slot), p.byte);
    if key(c.anchor) <= key(c.head) {
        (c.anchor, c.head)
    } else {
        (c.head, c.anchor)
    }
}

/// A caret put back where it can be after the document changed under it
/// (an undo, a reload): onto a slot that exists, on a character boundary.
pub fn fix(doc: &Doc, c: Caret) -> Caret {
    let first = slots(doc).first().copied().unwrap_or(Slot::block(0));
    let fix_pos = |p: Pos| {
        let slot = if block_of(doc, p.slot).is_some() && slots(doc).contains(&p.slot) {
            p.slot
        } else {
            first
        };
        let t = text(doc, slot);
        let mut byte = p.byte.min(len(doc, slot));
        if !is_atomic(doc, slot) {
            while !t.is_char_boundary(byte) {
                byte -= 1;
            }
        }
        Pos { slot, byte }
    };
    Caret {
        head: fix_pos(c.head),
        anchor: fix_pos(c.anchor),
    }
}

/// The first place in the note to put a caret.
pub fn first(doc: &Doc) -> Pos {
    start(slots(doc).first().copied().unwrap_or(Slot::block(0)))
}

/// The indices of the blocks a caret touches: what a block's tools act
/// on. A selection that only reaches the very start of a block, or leaves
/// from the very end of one with words in it, takes nothing of that block:
/// it is not drawn selected, and it is not moved, turned or indented with
/// the rest (Shift+↓ from a paragraph's start lands at the next one's).
pub fn touched(doc: &Doc, c: Caret) -> RangeInclusive<usize> {
    let r = spanned(doc, c);
    let (mut i, mut j) = (*r.start(), *r.end());
    if c.is_empty() || i == j {
        return r;
    }
    let (a, b) = ordered(doc, c);
    if b == start(block_slots(&doc.blocks()[j])[0]) {
        j -= 1;
    }
    let last = *block_slots(&doc.blocks()[i]).last().unwrap_or(&a.slot);
    if i < j && a.byte > 0 && a == end(doc, last) {
        i += 1;
    }
    i..=j
}

/// The indices of the blocks from the caret's one end to its other, both
/// included however little of them it takes: what a copy cuts its ends
/// from.
pub fn spanned(doc: &Doc, c: Caret) -> RangeInclusive<usize> {
    let (a, b) = ordered(doc, c);
    let i = doc.index_of(a.slot.block).unwrap_or(0);
    let j = doc.index_of(b.slot.block).unwrap_or(i);
    i.min(j)..=i.max(j)
}

/// What a selection covers, for drawing: blocks it covers whole, and the
/// byte range it covers in each slot it covers part of.
#[derive(Default, Debug)]
pub struct Coverage {
    pub whole: HashSet<u64>,
    pub parts: HashMap<Slot, (usize, usize)>,
}

pub fn coverage(doc: &Doc, c: Caret) -> Coverage {
    let mut out = Coverage::default();
    if c.is_empty() {
        return out;
    }
    let (a, b) = ordered(doc, c);
    let all = slots(doc);
    let (ia, ib) = (index(&all, a.slot), index(&all, b.slot));
    for (i, &s) in all.iter().enumerate().take(ib + 1).skip(ia) {
        let from = if i == ia { a.byte } else { 0 };
        let to = if i == ib { b.byte } else { len(doc, s) };
        out.parts.insert(s, (from, to));
    }
    // A block is covered whole when every slot of it is, end to end — and
    // the selection is more than one slot, or the block is a divider.
    if ia != ib || is_atomic(doc, a.slot) {
        for blk in doc.blocks() {
            let ss = block_slots(blk);
            let whole = ss.iter().all(|s| {
                out.parts
                    .get(s)
                    .is_some_and(|&(f, t)| f == 0 && t == len(doc, *s))
            });
            if whole {
                out.whole.insert(blk.id);
            }
        }
    }
    out
}

// ------------------------------------------------------------ characters

fn floor_char(s: &str, mut b: usize) -> usize {
    b = b.min(s.len());
    while !s.is_char_boundary(b) {
        b -= 1;
    }
    b
}

fn prev_char(s: &str, b: usize) -> usize {
    s[..floor_char(s, b)].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn next_char(s: &str, b: usize) -> usize {
    let b = floor_char(s, b);
    s[b..].chars().next().map_or(b, |c| b + c.len_utf8())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn prev_word(s: &str, b: usize) -> usize {
    let mut i = floor_char(s, b);
    // Skip what is not a word, then the word.
    while i > 0 && !s[..i].chars().next_back().is_some_and(is_word) {
        i = prev_char(s, i);
    }
    while i > 0 && s[..i].chars().next_back().is_some_and(is_word) {
        i = prev_char(s, i);
    }
    i
}

fn next_word(s: &str, b: usize) -> usize {
    let mut i = floor_char(s, b);
    while i < s.len() && !s[i..].chars().next().is_some_and(is_word) {
        i = next_char(s, i);
    }
    while i < s.len() && s[i..].chars().next().is_some_and(is_word) {
        i = next_char(s, i);
    }
    i
}

/// Whether a slot's text has Markdown's marks in it: not code, which is
/// kept as written, and not a divider or a picture.
fn styled(doc: &Doc, slot: Slot) -> bool {
    !is_atomic(doc, slot) && !matches!(kind_at(doc, slot), Some(Kind::Raw(_)))
}

/// A slot's words as the page draws them: its marks hidden, code whole.
pub fn visible(doc: &Doc, slot: Slot) -> Visible {
    let t = text(doc, slot);
    if styled(doc, slot) { Visible::of(t) } else { Visible::all(t) }
}

/// `rest` put on the end of `t`, pairs left touching joined; returns
/// where the caret goes — where `t` ended.
fn join_text(t: &mut String, rest: &str, styled: bool) -> usize {
    if !styled {
        let before = t.len();
        t.push_str(rest);
        return before;
    }
    let d = Visible::of(t).drawn_len();
    *t = inline::merge(&inline::join(t, rest));
    Visible::of(t).at(d)
}

/// `t` split at `byte`: each side's marks whole, in styled text.
fn split_text(t: &str, byte: usize, styled: bool) -> (String, String) {
    if styled {
        inline::split(t, byte)
    } else {
        (t[..byte].to_string(), t[byte..].to_string())
    }
}

// ------------------------------------------------------------ motions

/// One character (or word) left, into the previous slot at an edge; with
/// `extend` the anchor stays. A selection collapses to its start.
pub fn left(doc: &Doc, c: Caret, word: bool, extend: bool) -> Caret {
    if !extend && !c.is_empty() {
        return Caret::at(ordered(doc, c).0);
    }
    let p = c.head;
    let t = text(doc, p.slot);
    // By the words: marks are not there to step over.
    let v = visible(doc, p.slot);
    let d = v.drawn(p.byte);
    let head = if is_atomic(doc, p.slot) && p.byte > 0 {
        Pos { byte: 0, ..p }
    } else if !is_atomic(doc, p.slot) && d > 0 {
        let w = v.text(t);
        let nd = if word { prev_word(&w, d) } else { prev_char(&w, d) };
        Pos { byte: v.at(nd), ..p }
    } else {
        let all = slots(doc);
        match index(&all, p.slot).checked_sub(1) {
            Some(i) => end(doc, all[i]),
            None => p,
        }
    };
    Caret {
        head,
        anchor: if extend { c.anchor } else { head },
    }
}

pub fn right(doc: &Doc, c: Caret, word: bool, extend: bool) -> Caret {
    if !extend && !c.is_empty() {
        return Caret::at(ordered(doc, c).1);
    }
    let p = c.head;
    let t = text(doc, p.slot);
    let v = visible(doc, p.slot);
    let d = v.drawn(p.byte);
    let head = if is_atomic(doc, p.slot) && p.byte < 1 {
        Pos { byte: 1, ..p }
    } else if !is_atomic(doc, p.slot) && d < v.drawn_len() {
        let w = v.text(t);
        let nd = if word { next_word(&w, d) } else { next_char(&w, d) };
        Pos { byte: v.at(nd), ..p }
    } else {
        let all = slots(doc);
        match all.get(index(&all, p.slot) + 1) {
            Some(&s) => start(s),
            None => p,
        }
    };
    Caret {
        head,
        anchor: if extend { c.anchor } else { head },
    }
}

/// The word under a position (a double click).
pub fn word_at(doc: &Doc, p: Pos) -> Caret {
    let v = visible(doc, p.slot);
    let t = v.text(text(doc, p.slot));
    let mut a = floor_char(&t, v.drawn(p.byte));
    let mut b = a;
    while a > 0 && t[..a].chars().next_back().is_some_and(is_word) {
        a = prev_char(&t, a);
    }
    while b < t.len() && t[b..].chars().next().is_some_and(is_word) {
        b = next_char(&t, b);
    }
    // From the word's first letter, inside its marks, to after its last.
    let start = if a < b { v.source(a) } else { v.at(a) };
    Caret {
        anchor: Pos { byte: start, ..p },
        head: Pos { byte: v.at(b), ..p },
    }
}

/// Whether `a` comes before `b` in the document.
pub fn before(doc: &Doc, a: Pos, b: Pos) -> bool {
    let slots = slots(doc);
    (index(&slots, a.slot), a.byte) < (index(&slots, b.slot), b.byte)
}

/// A drag that began with a double or triple click, now at `to`: it grows
/// by what the click chose — whole words, or whole lines (`words` false).
/// What was clicked stays selected whichever way the drag goes, and the
/// end that moves takes in the whole word or line under the pointer.
pub fn extend_by(doc: &Doc, clicked: Caret, to: Pos, words: bool) -> Caret {
    let (s, e) = ordered(doc, clicked);
    let unit = if words { word_at(doc, to) } else { select_slot(doc, to.slot) };
    let (us, ue) = ordered(doc, unit);
    if before(doc, to, s) {
        Caret { anchor: e, head: us }
    } else if before(doc, e, to) {
        Caret { anchor: s, head: ue }
    } else {
        Caret { anchor: s, head: e }
    }
}

/// The whole of one slot (a triple click, the first Ctrl+A).
pub fn select_slot(doc: &Doc, slot: Slot) -> Caret {
    Caret {
        anchor: start(slot),
        head: end(doc, slot),
    }
}

/// Whole blocks `i..=j`.
pub fn select_blocks(doc: &Doc, i: usize, j: usize) -> Caret {
    let (i, j) = (i.min(j), i.max(j).min(doc.len().saturating_sub(1)));
    let first = block_slots(&doc.blocks()[i])[0];
    let last = *block_slots(&doc.blocks()[j]).last().unwrap_or(&first);
    Caret {
        anchor: start(first),
        head: end(doc, last),
    }
}

pub fn select_all(doc: &Doc) -> Caret {
    select_blocks(doc, 0, doc.len().saturating_sub(1))
}

/// The slot above or below `slot` as the eye reads it: in a table, the
/// cell in the same column of the next row; else the next block's slots
/// (several for a table — the view picks the column under the caret).
pub fn vertical(doc: &Doc, slot: Slot, up: bool) -> Vec<Slot> {
    let Some(i) = doc.index_of(slot.block) else { return Vec::new() };
    if let (Some((row, col)), Kind::Table(t)) = (slot.cell, &doc.blocks()[i].kind) {
        let r = t.row_index(row).unwrap_or(0);
        let next = if up { r.checked_sub(1) } else { Some(r + 1) };
        if let Some(row) = next.and_then(|n| t.rows.get(n)) {
            return vec![Slot {
                block: slot.block,
                cell: Some((row.id, col)),
            }];
        }
    }
    let j = if up { i.checked_sub(1) } else { Some(i + 1) };
    let Some(b) = j.and_then(|j| doc.blocks().get(j)) else {
        return Vec::new();
    };
    match &b.kind {
        // Into a table: the row it is entered from.
        Kind::Table(t) => {
            let row = if up { t.rows.last() } else { t.rows.first() };
            row.map(|r| {
                t.cols
                    .iter()
                    .map(|&c| Slot {
                        block: b.id,
                        cell: Some((r.id, c)),
                    })
                    .collect()
            })
            .unwrap_or_default()
        }
        _ => vec![Slot::block(b.id)],
    }
}

// ------------------------------------------------------------ edits

/// What the caret is in, for deciding what a key does.
fn kind_at(doc: &Doc, slot: Slot) -> Option<&Kind> {
    block_of(doc, slot).map(|b| &b.kind)
}

fn single_line(kind: &Kind) -> bool {
    matches!(kind, Kind::Heading { .. } | Kind::Item { .. } | Kind::Table(_))
}

/// Takes out what the selection covers; the caret ends at its start. True
/// when anything changed. Blocks covered whole go whole — what Backspace,
/// Delete and Cut do; see [`replace_selection`] for typing over them.
pub fn delete_selection(doc: &mut Doc, c: &mut Caret) -> bool {
    remove_selection(doc, c, false)
}

/// The selection taken out to be typed over: the first block stays, empty,
/// so what is typed keeps its kind — a list item stays a list item.
pub fn replace_selection(doc: &mut Doc, c: &mut Caret) -> bool {
    remove_selection(doc, c, true)
}

fn remove_selection(doc: &mut Doc, c: &mut Caret, keep_first: bool) -> bool {
    if c.is_empty() {
        return false;
    }
    let (a, b) = ordered(doc, *c);
    let all = slots(doc);
    let (ia, ib) = (index(&all, a.slot), index(&all, b.slot));
    // Within one slot: its text.
    if ia == ib {
        if is_atomic(doc, a.slot) {
            let i = doc.index_of(a.slot.block).unwrap_or(0);
            doc.remove(i);
            *c = Caret::at(after_removal(doc, i));
            return true;
        }
        // The words between, their marks kept whole.
        let marks = styled(doc, a.slot);
        let mut at = a.byte;
        if let Some(t) = text_mut(doc, a.slot) {
            if marks {
                let (n, caret) = inline::delete(t, a.byte, b.byte);
                *t = n;
                at = caret;
            } else {
                t.replace_range(a.byte..b.byte, "");
            }
        }
        *c = Caret::at(Pos { byte: at, ..a });
        return true;
    }
    let cov = coverage(doc, *c);
    let (ba, bb) = (
        doc.index_of(a.slot.block).unwrap_or(0),
        doc.index_of(b.slot.block).unwrap_or(0),
    );
    // Within one table: the cells' text; the table stays, unless the
    // selection covers it end to end.
    if ba == bb && !cov.whole.contains(&a.slot.block) {
        let mut at = a.byte;
        for (s, &(f, t)) in &cov.parts {
            if let Some(text) = text_mut(doc, *s) {
                let (n, caret) = inline::delete(text, f, t);
                *text = n;
                if *s == a.slot {
                    at = caret;
                }
            }
        }
        *c = Caret::at(Pos { byte: at, ..a });
        return true;
    }
    // Across blocks. The first and the last keep what lies outside the
    // selection; everything between goes.
    let (a_marks, b_marks) = (styled(doc, a.slot), styled(doc, b.slot));
    let tail = if cov.whole.contains(&b.slot.block) {
        None
    } else {
        Some(split_text(text(doc, b.slot), b.byte, b_marks).1)
    };
    // Cells of a partly covered table are cleared where covered.
    for (s, &(f, t)) in &cov.parts {
        if s.cell.is_some() && !cov.whole.contains(&s.block)
            && let Some(text) = text_mut(doc, *s)
        {
            *text = inline::delete(text, f, t).0;
        }
    }
    let first_textual_kind = a.slot.cell.is_none() && !is_atomic(doc, a.slot);
    let first_kept = !cov.whole.contains(&a.slot.block) || (keep_first && first_textual_kind);
    let last_kept = !cov.whole.contains(&b.slot.block) && bb != ba;
    // A divider or a picture at either end has no text to cut or join:
    // it is kept, or taken, whole.
    let first_textual = first_kept && a.slot.cell.is_none() && !is_atomic(doc, a.slot);
    let last_textual = last_kept && b.slot.cell.is_none() && !is_atomic(doc, b.slot);
    let mut a = a;
    if first_textual && let Some(t) = text_mut(doc, a.slot) {
        *t = split_text(t, a.byte, a_marks).0;
        a.byte = t.len();
    }
    if last_textual && let Some(t) = text_mut(doc, b.slot) {
        *t = split_text(t, b.byte, b_marks).1;
    }
    // Remove the middle, and the ends the selection covers whole.
    let remove_from = if first_kept { ba + 1 } else { ba };
    let remove_to = if last_kept { bb } else { bb + 1 };
    if remove_from < remove_to {
        doc.splice(remove_from..remove_to, []);
    }
    // Two ends of text meet: the last joins the first.
    if first_textual && last_textual {
        let joined = remove_from; // the last block now sits right after the first
        if joined < doc.len() {
            let mut rest = tail.unwrap_or_default();
            let first_single = single_line(&doc.blocks()[ba].kind);
            if first_single {
                rest = rest.replace('\n', " ");
            }
            doc.remove(joined);
            if let Some(t) = text_mut(doc, a.slot) {
                a.byte = join_text(t, &rest, a_marks);
            }
        }
    }
    *c = Caret::at(if first_kept {
        a
    } else {
        after_removal(doc, ba)
    });
    keep_writable(doc, c);
    true
}

/// Where a caret goes when the block at `i` was taken out: the start of
/// what took its place, else the end of what is above.
fn after_removal(doc: &mut Doc, i: usize) -> Pos {
    if doc.is_empty() {
        doc.push(Block::text(""));
    }
    if let Some(b) = doc.blocks().get(i) {
        return start(block_slots(b)[0]);
    }
    let b = &doc.blocks()[doc.len() - 1];
    let last = *block_slots(b).last().unwrap();
    end(doc, last)
}

/// The note always has somewhere to type at the end.
fn keep_writable(doc: &mut Doc, c: &mut Caret) {
    if doc.is_empty() {
        doc.push(Block::text(""));
    }
    doc.ensure_tail();
    *c = fix(doc, *c);
}

/// Types `s` over the selection, or at the caret. On a divider the text
/// goes into a new paragraph beside it. A Markdown prefix typed at the
/// start of a paragraph's line makes that line a block.
pub fn insert(doc: &mut Doc, c: &mut Caret, s: &str) -> bool {
    if s.is_empty() {
        return delete_selection(doc, c);
    }
    replace_selection(doc, c);
    let p = c.head;
    if is_atomic(doc, p.slot) {
        let i = doc.index_of(p.slot.block).unwrap_or(0);
        let at = if p.byte == 0 { i } else { i + 1 };
        let b = Block::text(s);
        let id = b.id;
        doc.insert(at, b);
        *c = Caret::at(Pos {
            slot: Slot::block(id),
            byte: s.len(),
        });
        return true;
    }
    let single = kind_at(doc, p.slot).is_some_and(single_line);
    let s = if single { s.replace('\n', " ") } else { s.to_string() };
    let old = text(doc, p.slot).to_string();
    let Some(t) = text_mut(doc, p.slot) else { return false };
    let byte = floor_char(t, p.byte);
    t.insert_str(byte, &s);
    *c = Caret::at(Pos {
        byte: byte + s.len(),
        ..p
    });
    shortcuts(doc, c, &old);
    true
}

/// After typing: a paragraph line that now reads as structure becomes it,
/// and two leading spaces on a list item indent it.
fn shortcuts(doc: &mut Doc, c: &mut Caret, old: &str) {
    let p = c.head;
    let Some(i) = doc.index_of(p.slot.block) else { return };
    let new = text(doc, p.slot).to_string();
    match &doc.blocks()[i].kind {
        Kind::Item { indent, .. } if new.starts_with("  ") && !old.starts_with("  ") => {
            let indent = (*indent + 1).min(doc::MAX_INDENT);
            if let Kind::Item { indent: n, text, .. } = &mut doc.block_mut(i).kind {
                *n = indent;
                text.replace_range(..2, "");
            }
            c.head.byte = c.head.byte.saturating_sub(2);
            c.anchor = c.head;
        }
        Kind::Text(_) => {
            // The line the caret is on, and where it starts.
            let line_start = new[..p.byte].rfind('\n').map_or(0, |n| n + 1);
            let line_end = new[p.byte..].find('\n').map_or(new.len(), |n| p.byte + n);
            let line = &new[line_start..line_end];
            let old_line = old.get(line_start..).map(|s| s.split('\n').next().unwrap_or(""));
            let Some(kind) = doc::shortcut(line) else { return };
            if old_line.is_some_and(|l| doc::shortcut(l).is_some()) {
                return;
            }
            let prefix = line.len() - kind_text_len(&kind).min(line.len());
            let caret_in = (p.byte - line_start).saturating_sub(prefix);
            let before = new[..line_start].trim_end_matches('\n').to_string();
            let after = new.get(line_end + 1..).unwrap_or("").to_string();
            let mut blocks = Vec::new();
            if !before.is_empty() {
                blocks.push(Block::text(before));
            }
            let made = Block::new(kind);
            let target = made.clone();
            blocks.push(made);
            if line_end < new.len() {
                blocks.push(Block::text(after));
            }
            doc.splice(i..i + 1, blocks);
            *c = Caret::at(entry(doc, &target, caret_in));
            if target.kind.is_atomic() {
                // Nothing to type on: carry on in the block below.
                carry_on_unless_picture(doc, doc.index_of(target.id).unwrap_or(0) + 1, c);
            }
            doc.ensure_tail();
        }
        _ => {}
    }
}

fn kind_text_len(k: &Kind) -> usize {
    match k {
        Kind::Heading { text, .. } | Kind::Item { text, .. } | Kind::Quote(text) => text.len(),
        _ => 0,
    }
}

/// Where the caret goes in a block just made: its text at `byte`, a
/// table's first cell, or inside a code fence.
fn entry(doc: &Doc, b: &Block, byte: usize) -> Pos {
    match &b.kind {
        Kind::Table(t) => start(Slot {
            block: b.id,
            cell: Some((t.rows[0].id, t.cols[0])),
        }),
        Kind::Raw(t) => Pos {
            slot: Slot::block(b.id),
            byte: t.find('\n').map_or(0, |n| n + 1),
        },
        _ => {
            let slot = Slot::block(b.id);
            Pos {
                slot,
                byte: byte.min(len(doc, slot)),
            }
        }
    }
}

/// Backspace: the selection, else the character (or word) before the
/// caret. At the start of a block it undoes the block's structure first —
/// an item outdents, then becomes text; a heading becomes text — and then
/// joins the text to the block above. A divider above goes; a table above
/// is selected, so the next Backspace removes it.
pub fn backspace(doc: &mut Doc, c: &mut Caret, word: bool) -> bool {
    if delete_selection(doc, c) {
        return true;
    }
    let p = c.head;
    let Some(i) = doc.index_of(p.slot.block) else { return false };
    if is_atomic(doc, p.slot) {
        doc.remove(i);
        *c = Caret::at(after_removal_up(doc, i));
        keep_writable(doc, c);
        return true;
    }
    // A letter (or a word) before the caret: taken by the words, so a
    // mark is never half deleted and goes with the last of its words.
    let v = visible(doc, p.slot);
    let d = v.drawn(p.byte);
    if d > 0 {
        let t = text(doc, p.slot);
        let w = v.text(t);
        let nd = if word { prev_word(&w, d) } else { prev_char(&w, d) };
        let (from, to) = (v.source(nd), v.at(d));
        let marks = styled(doc, p.slot);
        let mut at = from;
        if let Some(t) = text_mut(doc, p.slot) {
            if marks {
                let (n, caret) = inline::delete(t, from, to);
                *t = n;
                at = caret;
            } else {
                t.replace_range(from..to, "");
            }
        }
        *c = Caret::at(Pos { byte: at, ..p });
        return true;
    }
    // At the start of a slot.
    if let Some((row, col)) = p.slot.cell {
        // The first cell of an empty table takes the table with it.
        let Kind::Table(t) = &doc.blocks()[i].kind else { return false };
        let first = t.rows[0].id == row && t.cols[0] == col;
        let empty = t.rows.iter().all(|r| r.cells.iter().all(|c| c.is_empty()));
        if first && empty {
            doc.remove(i);
            *c = Caret::at(after_removal_up(doc, i));
            keep_writable(doc, c);
            return true;
        }
        *c = left(doc, *c, false, false);
        return false;
    }
    let kind = doc.blocks()[i].kind.clone();
    match kind {
        Kind::Item { indent, .. } if indent > 0 => {
            if let Kind::Item { indent, .. } = &mut doc.block_mut(i).kind {
                *indent -= 1;
            }
            true
        }
        // A list item lets go of its structure first, as every editor's
        // does; a heading does not: Backspace at its start is about the
        // line above — an empty one goes, a written one takes the heading's
        // words. Only a heading with nothing above it turns into text.
        Kind::Item { text, .. } | Kind::Quote(text) => {
            doc.block_mut(i).kind = Kind::Text(text);
            true
        }
        Kind::Heading { text, .. } if i == 0 => {
            doc.block_mut(i).kind = Kind::Text(text);
            true
        }
        Kind::Heading { .. } if matches!(&doc.blocks()[i - 1].kind, Kind::Text(t) if t.is_empty()) => {
            doc.remove(i - 1);
            *c = Caret::at(Pos { slot: p.slot, byte: 0 });
            true
        }
        Kind::Raw(t) => {
            if t.is_empty() {
                doc.remove(i);
                *c = Caret::at(after_removal_up(doc, i));
                keep_writable(doc, c);
                true
            } else {
                *c = left(doc, *c, false, false);
                false
            }
        }
        Kind::Text(t) | Kind::Heading { text: t, .. } => {
            let Some(prev) = i.checked_sub(1) else { return false };
            match doc.blocks()[prev].kind.clone() {
                Kind::Divider => {
                    doc.remove(prev);
                    true
                }
                Kind::Table(_) | Kind::Image { .. } => {
                    // An empty line under a table goes, and the table is
                    // selected; a line with words only selects it.
                    if t.is_empty() && i + 1 < doc.len() {
                        doc.remove(i);
                    }
                    *c = select_blocks(doc, prev, prev);
                    t.is_empty()
                }
                Kind::Raw(_) => {
                    if t.is_empty() {
                        doc.remove(i);
                        *c = Caret::at(end(doc, Slot::block(doc.blocks()[prev].id)));
                        keep_writable(doc, c);
                        return true;
                    }
                    *c = Caret::at(end(doc, Slot::block(doc.blocks()[prev].id)));
                    false
                }
                _ => {
                    let prev_id = doc.blocks()[prev].id;
                    let single = single_line(&doc.blocks()[prev].kind);
                    let joined = if single { t.replace('\n', " ") } else { t };
                    doc.remove(i);
                    let marks = styled(doc, Slot::block(prev_id));
                    let mut at = 0;
                    if let Some(pt) = text_mut(doc, Slot::block(prev_id)) {
                        at = join_text(pt, &joined, marks);
                    }
                    *c = Caret::at(Pos {
                        slot: Slot::block(prev_id),
                        byte: at,
                    });
                    keep_writable(doc, c);
                    true
                }
            }
        }
        _ => false,
    }
}

/// After removing block `i` going backwards: the end of what is above.
fn after_removal_up(doc: &mut Doc, i: usize) -> Pos {
    if doc.is_empty() {
        doc.push(Block::text(""));
        return first(doc);
    }
    let j = i.saturating_sub(1).min(doc.len() - 1);
    let b = &doc.blocks()[j];
    if i == 0 {
        return start(block_slots(b)[0]);
    }
    let last = *block_slots(b).last().unwrap();
    end(doc, last)
}

/// Delete: the selection, else the character (or word) after the caret.
/// At the end of a block the next one's text joins it; a divider below
/// goes, a table below is selected.
pub fn delete(doc: &mut Doc, c: &mut Caret, word: bool) -> bool {
    if delete_selection(doc, c) {
        return true;
    }
    let p = c.head;
    let Some(i) = doc.index_of(p.slot.block) else { return false };
    if is_atomic(doc, p.slot) {
        doc.remove(i);
        *c = Caret::at(after_removal(doc, i));
        keep_writable(doc, c);
        return true;
    }
    let t = text(doc, p.slot);
    let v = visible(doc, p.slot);
    let d = v.drawn(p.byte);
    if d < v.drawn_len() {
        let w = v.text(t);
        let nd = if word { next_word(&w, d) } else { next_char(&w, d) };
        let (from, to) = (v.source(d), v.at(nd));
        let marks = styled(doc, p.slot);
        if let Some(t) = text_mut(doc, p.slot) {
            if marks {
                let (n, caret) = inline::delete(t, from, to);
                *t = n;
                c.head.byte = caret;
                c.anchor = c.head;
            } else {
                t.replace_range(from..to, "");
            }
        }
        return true;
    }
    if p.slot.cell.is_some() || matches!(doc.blocks()[i].kind, Kind::Raw(_)) {
        *c = right(doc, *c, false, false);
        return false;
    }
    let Some(next) = doc.blocks().get(i + 1).cloned() else { return false };
    match &next.kind {
        Kind::Divider => {
            doc.remove(i + 1);
            true
        }
        Kind::Table(_) | Kind::Image { .. } => {
            *c = select_blocks(doc, i + 1, i + 1);
            false
        }
        Kind::Raw(_) => false,
        _ => {
            let single = single_line(&doc.blocks()[i].kind);
            let rest = next.text_ref().unwrap_or("").to_string();
            let rest = if single { rest.replace('\n', " ") } else { rest };
            doc.remove(i + 1);
            let marks = styled(doc, p.slot);
            if let Some(t) = text_mut(doc, p.slot) {
                c.head.byte = join_text(t, &rest, marks);
                c.anchor = c.head;
            }
            keep_writable(doc, c);
            true
        }
    }
}

/// Enter. A paragraph splits into two (Shift+Enter: a line break inside
/// it). A list item continues the list, or steps out of it when empty; at
/// its very start it opens an empty item above, leaving this one — and its
/// tick — where it is. A heading opens a paragraph. In a table Enter goes
/// down a column, adds a row at the bottom, and in an empty last row steps
/// out of the table. In code, it is a newline.
pub fn enter(doc: &mut Doc, c: &mut Caret, soft: bool) -> bool {
    replace_selection(doc, c);
    let p = c.head;
    let Some(i) = doc.index_of(p.slot.block) else { return false };
    let kind = doc.blocks()[i].kind.clone();
    let t = text(doc, p.slot).to_string();
    let byte = floor_char(&t, p.byte);
    // Split by the words: a bold run cut by Enter is bold on both lines.
    let marks = styled(doc, p.slot);
    let (head, tail) = split_text(&t, byte, marks);
    let v = visible(doc, p.slot);
    let (at_start, at_end) = (v.drawn(byte) == 0, v.drawn(byte) == v.drawn_len());
    let put = |doc: &mut Doc, c: &mut Caret, at: usize, b: Block, byte: usize| {
        let slot = Slot::block(b.id);
        doc.insert(at, b);
        *c = Caret::at(Pos { slot, byte });
    };
    match kind {
        Kind::Divider | Kind::Image { .. } => put(doc, c, if p.byte == 0 { i } else { i + 1 }, Block::text(""), 0),
        Kind::Raw(_) => return insert(doc, c, "\n"),
        Kind::Text(_) | Kind::Quote(_) if soft => return insert(doc, c, "\n"),
        // A quotation goes on a line at a time; Enter on an empty last
        // line steps out of it, and on an empty quotation undoes it.
        Kind::Quote(_) => {
            if t.is_empty() {
                doc.block_mut(i).kind = Kind::Text(String::new());
            } else if at_end && t.ends_with('\n') {
                if let Some(t) = text_mut(doc, p.slot) {
                    t.pop();
                }
                put(doc, c, i + 1, Block::text(""), 0);
            } else {
                return insert(doc, c, "\n");
            }
        }
        Kind::Text(_) => {
            if let Some(t) = text_mut(doc, p.slot) {
                *t = head.clone();
            }
            put(doc, c, i + 1, Block::text(tail), 0);
        }
        Kind::Heading { .. } => {
            if t.is_empty() {
                doc.block_mut(i).kind = Kind::Text(String::new());
            } else if at_start {
                put(doc, c, i, Block::text(""), 0);
                *c = Caret::at(start(p.slot));
            } else {
                if let Some(t) = text_mut(doc, p.slot) {
                    *t = head.clone();
                }
                put(doc, c, i + 1, Block::text(tail), 0);
            }
        }
        Kind::Item { style, indent, .. } => {
            if soft {
                return false;
            }
            if v.text(&t).trim().is_empty() {
                if indent > 0 {
                    if let Kind::Item { indent, .. } = &mut doc.block_mut(i).kind {
                        *indent -= 1;
                    }
                } else {
                    doc.block_mut(i).kind = Kind::Text(String::new());
                }
            } else if at_start {
                // At the start: a new item above; this one keeps its text,
                // its tick and the caret.
                doc.insert(i, Block::new(Kind::item(style, indent, "")));
            } else {
                if let Some(t) = text_mut(doc, p.slot) {
                    *t = head.clone();
                }
                put(doc, c, i + 1, Block::new(Kind::item(style, indent, &*tail)), 0);
            }
        }
        Kind::Table(_) => return table_enter(doc, c),
    }
    keep_writable(doc, c);
    true
}

fn table_enter(doc: &mut Doc, c: &mut Caret) -> bool {
    let p = c.head;
    let (Some(i), Some((row, col))) = (doc.index_of(p.slot.block), p.slot.cell) else {
        return false;
    };
    let Kind::Table(t) = &mut doc.block_mut(i).kind else { return false };
    let r = t.row_index(row).unwrap_or(0);
    if t.is_empty_last_row(r) && t.rows.len() > 2 {
        t.remove_row(r);
        let next = match doc.blocks().get(i + 1) {
            Some(b) if matches!(b.kind, Kind::Text(_)) => b.id,
            _ => {
                let b = Block::text("");
                let id = b.id;
                doc.insert(i + 1, b);
                id
            }
        };
        *c = Caret::at(start(Slot::block(next)));
        return true;
    }
    let (below, added) = match t.rows.get(r + 1) {
        Some(next) => (next.id, false),
        None => (t.insert_row(r + 1), true),
    };
    let slot = Slot {
        block: p.slot.block,
        cell: Some((below, col)),
    };
    *c = Caret::at(end(doc, slot));
    added
}

/// Tab and Shift+Tab: a list item indents and outdents (every item in the
/// selection); a table moves between cells, adding a row past the last.
/// Elsewhere Tab is a tab character — never spaces: typed at the caret
/// ([`tab_types`] says when, for the caller to type it as letters are),
/// or, with a selection over several lines, put at the start of each.
/// Shift+Tab takes one back. The first is false where Tab is not the
/// page's.
pub fn tab(doc: &mut Doc, c: &mut Caret, back: bool) -> (bool, bool) {
    let p = c.head;
    let Some(i) = doc.index_of(p.slot.block) else { return (false, false) };
    match &doc.blocks()[i].kind {
        Kind::Item { .. } if !spans_lines(doc, *c) => (true, indent(doc, *c, if back { -1 } else { 1 })),
        Kind::Table(t) => {
            let all = block_slots(&doc.blocks()[i]);
            let k = index(&all, p.slot);
            if back {
                if k > 0 {
                    *c = select_slot(doc, all[k - 1]);
                }
                return (true, false);
            }
            if let Some(&s) = all.get(k + 1) {
                *c = select_slot(doc, s);
                return (true, false);
            }
            let first_col = t.cols[0];
            let Kind::Table(t) = &mut doc.block_mut(i).kind else { return (true, false) };
            let id = t.insert_row(t.rows.len());
            *c = Caret::at(start(Slot {
                block: p.slot.block,
                cell: Some((id, first_col)),
            }));
            (true, true)
        }
        _ if spans_lines(doc, *c) => (true, tab_lines(doc, c, back)),
        _ if back => (true, untab(doc, c)),
        // Typed by the caller (`tab_types`). Tab is the page's everywhere:
        // on a picture or a divider it does nothing rather than throw the
        // keyboard to the next field (Esc, Esc is the way out of the page).
        _ => (true, false),
    }
}

/// Whether Tab at the caret types a tab character, as a letter is typed:
/// in a paragraph, a heading, a quotation or code, with the selection, if
/// any, on one line.
pub fn tab_types(doc: &Doc, c: Caret, back: bool) -> bool {
    !back && c.head.slot.cell.is_none() && tabbed(kind_at(doc, c.head.slot)) && !spans_lines(doc, c)
}

/// Text that takes a tab character: not a list item's (Tab indents it) nor
/// a cell's (Tab goes to the next).
fn tabbed(kind: Option<&Kind>) -> bool {
    matches!(kind, Some(Kind::Text(_) | Kind::Quote(_) | Kind::Heading { .. } | Kind::Raw(_)))
}

/// Whether the selection runs over more than one line: across blocks, or
/// over a line break in one.
fn spans_lines(doc: &Doc, c: Caret) -> bool {
    let (a, b) = ordered(doc, c);
    a.slot != b.slot || text(doc, a.slot).get(a.byte..b.byte).is_some_and(|s| s.contains('\n'))
}

/// Tab over several lines: a tab put at the start of each line the
/// selection touches, or one taken away (`back`); list items in it indent
/// or outdent, and a code block's fences are left as they are. The
/// selection keeps its words.
fn tab_lines(doc: &mut Doc, c: &mut Caret, back: bool) -> bool {
    let (a, b) = ordered(doc, *c);
    let forward = c.anchor == a;
    let mut changed = indent(doc, *c, if back { -1 } else { 1 });
    let all = slots(doc);
    let (mut na, mut nb) = (a, b);
    for &s in &all[index(&all, a.slot)..=index(&all, b.slot)] {
        let kind = kind_at(doc, s);
        // A block the selection only reaches the start of is not in it.
        if s.cell.is_some() || !tabbed(kind) || (s == b.slot && s != a.slot && b.byte == 0) {
            continue;
        }
        let raw = matches!(kind, Some(Kind::Raw(_)));
        let t = text(doc, s).to_string();
        let from = if s == a.slot { a.byte.min(t.len()) } else { 0 };
        let to = if s == b.slot { b.byte.min(t.len()) } else { t.len() };
        // The lines from the one the selection starts on; one it ends at
        // the very start of is not in it.
        let mut starts = vec![t[..from].rfind('\n').map_or(0, |n| n + 1)];
        starts.extend(t[from..to].match_indices('\n').map(|(k, _)| from + k + 1).filter(|&st| st < to));
        let last = t.rfind('\n').map_or(0, |n| n + 1);
        let mut out = t.clone();
        // Where bytes went in (+) or came out (-), the last line first.
        let mut moved: Vec<(usize, isize)> = Vec::new();
        for &st in starts.iter().rev() {
            if raw && (st == 0 || st == last) {
                continue;
            }
            if back {
                let n = lead_tab(&out[st..], raw);
                if n > 0 {
                    out.replace_range(st..st + n, "");
                    moved.push((st, -(n as isize)));
                }
            } else {
                out.insert(st, '\t');
                moved.push((st, 1));
            }
        }
        if moved.is_empty() {
            continue;
        }
        let map = |p: usize| {
            moved.iter().fold(p, |p, &(st, d)| {
                if d > 0 && p >= st {
                    p + d as usize
                } else if d < 0 && p > st {
                    p - (p - st).min(d.unsigned_abs())
                } else {
                    p
                }
            })
        };
        if s == na.slot {
            na.byte = map(na.byte);
        }
        if s == nb.slot {
            nb.byte = map(nb.byte);
        }
        if let Some(text) = text_mut(doc, s) {
            *text = out;
            changed = true;
        }
    }
    *c = if forward { Caret { anchor: na, head: nb } } else { Caret { anchor: nb, head: na } };
    changed
}

/// How much of a line's start Shift+Tab takes: its tab, or, in code, up
/// to four spaces written in place of one.
fn lead_tab(line: &str, raw: bool) -> usize {
    if line.starts_with('\t') {
        1
    } else if raw {
        line.bytes().take(4).take_while(|&b| b == b' ').count()
    } else {
        0
    }
}

/// Shift+Tab on one line: the tab just before the caret taken back, else
/// the one the line starts with.
fn untab(doc: &mut Doc, c: &mut Caret) -> bool {
    let (a, _) = ordered(doc, *c);
    let raw = matches!(kind_at(doc, a.slot), Some(Kind::Raw(_)));
    let t = text(doc, a.slot);
    let byte = floor_char(t, a.byte);
    let (at, n) = if c.is_empty() && t[..byte].ends_with('\t') {
        (byte - 1, 1)
    } else {
        let st = t[..byte].rfind('\n').map_or(0, |n| n + 1);
        (st, lead_tab(&t[st..], raw))
    };
    if n == 0 {
        return false;
    }
    let Some(text) = text_mut(doc, a.slot) else { return false };
    text.replace_range(at..at + n, "");
    let back = |p: Pos| if p.slot == a.slot && p.byte > at { Pos { byte: p.byte - (p.byte - at).min(n), ..p } } else { p };
    *c = Caret { anchor: back(c.anchor), head: back(c.head) };
    true
}

/// Every list item the caret touches, indented by `by`.
pub fn indent(doc: &mut Doc, c: Caret, by: i64) -> bool {
    let mut changed = false;
    for i in touched(doc, c) {
        if let Kind::Item { indent, .. } = &doc.blocks()[i].kind {
            let n = (*indent as i64 + by).clamp(0, doc::MAX_INDENT as i64) as u8;
            if n != *indent {
                if let Kind::Item { indent, .. } = &mut doc.block_mut(i).kind {
                    *indent = n;
                }
                changed = true;
            }
        }
    }
    changed
}

/// The tick of item `id`.
/// A to-do flips; a bullet or numbered item becomes a to-do first (a tick
/// on a bullet would have nowhere to be written).
pub fn toggle(doc: &mut Doc, id: u64) -> bool {
    match doc.get_mut(id).map(|b| &mut b.kind) {
        Some(Kind::Item { style: ListStyle::Todo, done, .. }) => {
            *done = !*done;
            true
        }
        Some(Kind::Item { style, done, .. }) => {
            *style = ListStyle::Todo;
            *done = false;
            true
        }
        _ => false,
    }
}

/// The blocks the caret touches, again, right below them — copies with
/// ids of their own — and selected.
pub fn duplicate(doc: &mut Doc, c: &mut Caret) -> bool {
    let r = touched(doc, *c);
    let (i, j) = (*r.start(), *r.end());
    let copies: Vec<Block> = (i..=j).map(|k| Block::new(doc.blocks()[k].kind.clone())).collect();
    let n = copies.len();
    doc.splice(j + 1..j + 1, copies);
    *c = select_blocks(doc, j + 1, j + n);
    true
}

/// The blocks the caret touches, moved up (`by` < 0) or down past their
/// neighbour.
pub fn move_blocks(doc: &mut Doc, c: Caret, by: i64) -> bool {
    let r = touched(doc, c);
    let (i, j) = (*r.start(), *r.end());
    // The empty line kept after a last list, table or picture is room to
    // type, not a block to pass: past it, another would be made under
    // it, and another, as long as the key was held.
    let tail = j + 2 == doc.len() && matches!(&doc.blocks()[j + 1].kind, Kind::Text(t) if t.is_empty());
    if by > 0 && tail {
        return false;
    }
    if by < 0 && i > 0 {
        let b = doc.remove(i - 1);
        doc.insert(j, b);
    } else if by > 0 && j + 1 < doc.len() {
        let b = doc.remove(j + 1);
        doc.insert(i, b);
    } else {
        return false;
    }
    doc.ensure_tail();
    true
}

/// Every block the caret touches turned into kind `to` — a paragraph of
/// several lines into a block per line. The caret keeps to the turned
/// blocks.
pub fn turn(doc: &mut Doc, c: &mut Caret, to: u64) -> bool {
    let r = touched(doc, *c);
    let single = c.is_empty() || r.start() == r.end();
    let (start_i, mut end_i) = (*r.start(), *r.end());
    let mut changed = false;
    for i in r.rev() {
        if let Some(kinds) = turned_lines(&doc.blocks()[i].kind, to)
            && (kinds.len() != 1 || doc.blocks()[i].kind != kinds[0])
        {
            let n = kinds.len();
            let id = doc.blocks()[i].id;
            let mut blocks: Vec<Block> = kinds.into_iter().map(Block::new).collect();
            // The first keeps the id, so a caret in it stays in it.
            blocks[0].id = id;
            doc.splice(i..i + 1, blocks);
            end_i += n - 1;
            changed = true;
        }
    }
    if !changed {
        return false;
    }
    if single && end_i == start_i {
        *c = fix(doc, *c);
    } else {
        *c = select_blocks(doc, start_i, end_i);
    }
    doc.ensure_tail();
    true
}

pub const TURN_TEXT: u64 = 0;
pub const TURN_H1: u64 = 1;
pub const TURN_H2: u64 = 2;
pub const TURN_H3: u64 = 3;
pub const TURN_BULLET: u64 = 4;
pub const TURN_NUMBER: u64 = 5;
pub const TURN_TODO: u64 = 6;
pub const TURN_QUOTE: u64 = 7;

/// `turned`, a block per line: a paragraph of five lines made a list is
/// five items, not one long one.
pub fn turned_lines(kind: &Kind, to: u64) -> Option<Vec<Kind>> {
    match kind {
        Kind::Text(t) | Kind::Quote(t) if !matches!(to, TURN_TEXT | TURN_QUOTE) && t.contains('\n') => {
            let lines: Vec<Kind> = t
                .split('\n')
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| turned(&Kind::Text(l.to_string()), to))
                .collect();
            (!lines.is_empty()).then_some(lines)
        }
        _ => turned(kind, to).map(|k| vec![k]),
    }
}

/// What `to` makes of a block of `kind`; `None` for a table or code.
pub fn turned(kind: &Kind, to: u64) -> Option<Kind> {
    let (text, indent, done) = match kind {
        Kind::Table(_) | Kind::Raw(_) | Kind::Image { .. } => return None,
        Kind::Divider => (String::new(), 0, false),
        Kind::Text(t) | Kind::Quote(t) | Kind::Heading { text: t, .. } => (t.clone(), 0, false),
        Kind::Item {
            text, indent, done, ..
        } => (text.clone(), *indent, *done),
    };
    let line = text.replace('\n', " ");
    // Only a to-do keeps a tick: Markdown has nowhere to write one on a
    // bullet.
    let item = |style| Kind::Item {
        style,
        indent,
        done: done && style == ListStyle::Todo,
        num: 0,
        text: line.clone(),
    };
    Some(match to {
        TURN_TEXT => Kind::Text(text),
        TURN_H1 | TURN_H2 | TURN_H3 => Kind::Heading {
            level: to as u8,
            text: line,
        },
        TURN_BULLET => item(ListStyle::Bullet),
        TURN_NUMBER => item(ListStyle::Number),
        TURN_TODO => item(ListStyle::Todo),
        TURN_QUOTE => Kind::Quote(text),
        _ => return None,
    })
}

/// Puts a new block of `kind` below the caret's block (in place of an
/// empty paragraph), with the caret in it.
/// Blocks dropped before block `at` (the end for `at` past the last),
/// whatever the caret was doing: the last of them is selected, so what
/// came in is what the keyboard acts on next.
pub fn insert_blocks_at(doc: &mut Doc, c: &mut Caret, at: usize, kinds: Vec<Kind>) -> bool {
    if kinds.is_empty() {
        return false;
    }
    let at = at.min(doc.len());
    let n = kinds.len();
    for (k, kind) in kinds.into_iter().enumerate() {
        doc.insert(at + k, Block::new(kind));
    }
    doc.ensure_tail();
    *c = select_blocks(doc, at + n - 1, at + n - 1);
    true
}

/// A picture's width (`None`: its own). True when it changed.
pub fn set_image_width(doc: &mut Doc, id: u64, width: Option<u32>) -> bool {
    let Some(i) = doc.index_of(id) else { return false };
    if matches!(&doc.blocks()[i].kind, Kind::Image { width: w, .. } if *w == width) {
        return false;
    }
    if let Kind::Image { width: w, .. } = &mut doc.block_mut(i).kind {
        *w = width;
        return true;
    }
    false
}

/// Where a picture sits: on its own line, or at an edge with the words
/// beside it. True when it changed.
pub fn set_image_float(doc: &mut Doc, id: u64, to: doc::Float) -> bool {
    let Some(i) = doc.index_of(id) else { return false };
    if matches!(&doc.blocks()[i].kind, Kind::Image { float, .. } if *float == to) {
        return false;
    }
    if let Kind::Image { float, .. } = &mut doc.block_mut(i).kind {
        *float = to;
        return true;
    }
    false
}

/// Where a dragged picture is let go: before block `i` (a line of its own
/// between blocks), or beside another picture — the two side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicDrop {
    Before(usize),
    Beside { id: u64, after: bool },
}

/// Picture `id` moved to `to`, and selected. Beside a picture on its own
/// line, both go to the left edge, side by side; beside one at an edge,
/// it joins that edge. One step of the history either way.
pub fn move_picture(doc: &mut Doc, c: &mut Caret, id: u64, to: PicDrop) -> bool {
    let Some(from) = doc.index_of(id) else { return false };
    match to {
        PicDrop::Before(i) => {
            if i == from || i == from + 1 {
                return false;
            }
            let b = doc.remove(from);
            doc.insert(if i > from { i - 1 } else { i }, b);
        }
        PicDrop::Beside { id: t, after } => {
            if t == id {
                return false;
            }
            let side = match doc.get(t).map(|b| &b.kind) {
                Some(Kind::Image { float: doc::Float::None, .. }) => doc::Float::Left,
                Some(Kind::Image { float, .. }) => *float,
                _ => return false,
            };
            let b = doc.remove(from);
            let Some(ti) = doc.index_of(t) else { return false };
            doc.insert(if after { ti + 1 } else { ti }, b);
            set_image_float(doc, t, side);
            set_image_float(doc, id, side);
        }
    }
    doc.ensure_tail();
    if let Some(i) = doc.index_of(id) {
        *c = select_blocks(doc, i, i);
    }
    true
}

pub fn insert_block(doc: &mut Doc, c: &mut Caret, kind: Kind) -> bool {
    if kind == Kind::Divider {
        return insert_divider(doc, c);
    }
    let i = doc.index_of(c.head.slot.block).unwrap_or(doc.len().saturating_sub(1));
    let empty_text = matches!(&doc.blocks()[i].kind, Kind::Text(t) if t.is_empty());
    let b = Block::new(kind);
    let made = b.clone();
    if empty_text {
        doc.set(i, b);
    } else {
        doc.insert(i + 1, b);
    }
    *c = Caret::at(entry(doc, &made, 0));
    if made.kind.is_atomic() {
        *c = Caret::at(carry_on(doc, doc.index_of(made.id).unwrap_or(0) + 1));
    }
    doc.ensure_tail();
    true
}

/// Where the caret carries on past a block with nothing to type on (a
/// divider, a picture) that ends before block `j`: the start of block `j`
/// when it has text to type in, else a new empty line made there.
fn carry_on(doc: &mut Doc, j: usize) -> Pos {
    let next = match doc.blocks().get(j) {
        Some(b) if matches!(b.kind, Kind::Text(_) | Kind::Heading { .. } | Kind::Item { .. } | Kind::Quote(_)) => b.id,
        _ => {
            let b = Block::text("");
            let id = b.id;
            doc.insert(j, b);
            id
        }
    };
    start(Slot::block(next))
}

/// Where the caret carries on past a divider put in just before block
/// `j`: as `carry_on`, but where a picture (or another divider) follows,
/// just past the divider, where typing starts a paragraph — no empty
/// line is made between the divider and the picture, to show as a gap.
fn carry_on_unless_picture(doc: &mut Doc, j: usize, c: &mut Caret) {
    if doc.blocks().get(j).is_some_and(|b| b.kind.is_atomic()) {
        let divider = Slot::block(doc.blocks()[j - 1].id);
        *c = Caret::at(end(doc, divider));
        return;
    }
    *c = Caret::at(carry_on(doc, j));
}

/// A divider where the caret is. In the middle of a block's words the
/// block is cut there, the divider between the halves and the caret at
/// the start of the second; at the start of a block it goes before it,
/// the caret staying; at the end, after it, and on an empty line in its
/// place, the caret going on to the next block with text — a new line is
/// made only when there is none, and none before a picture, where the
/// caret stays. In a table, or on a picture (selected, it stays: the
/// divider goes after it, not in its place), a divider or code, it goes
/// after the block.
pub fn insert_divider(doc: &mut Doc, c: &mut Caret) -> bool {
    let on_one_atomic = !c.is_empty() && c.anchor.slot == c.head.slot && is_atomic(doc, c.head.slot);
    if !on_one_atomic {
        replace_selection(doc, c);
    }
    let p = c.head;
    let Some(i) = doc.index_of(p.slot.block) else { return false };
    let kind = doc.blocks()[i].kind.clone();
    let words = p.slot.cell.is_none() && matches!(kind, Kind::Text(_) | Kind::Heading { .. } | Kind::Item { .. } | Kind::Quote(_));
    if !words {
        doc.insert(i + 1, Block::new(Kind::Divider));
        carry_on_unless_picture(doc, i + 2, c);
        doc.ensure_tail();
        return true;
    }
    let t = text(doc, p.slot).to_string();
    let byte = floor_char(&t, p.byte);
    let v = visible(doc, p.slot);
    let (at_start, at_end) = (v.drawn(byte) == 0, v.drawn(byte) == v.drawn_len());
    if t.is_empty() && matches!(kind, Kind::Text(_)) {
        doc.set(i, Block::new(Kind::Divider));
        carry_on_unless_picture(doc, i + 1, c);
    } else if at_start {
        doc.insert(i, Block::new(Kind::Divider));
    } else if at_end {
        doc.insert(i + 1, Block::new(Kind::Divider));
        carry_on_unless_picture(doc, i + 2, c);
    } else {
        // Cut as Enter cuts (a bold run cut is bold on both sides); the
        // spaces or the line break at the cut go.
        let (head, tail) = split_text(&t, byte, styled(doc, p.slot));
        let head = head.trim_end_matches(' ');
        let tail = tail.trim_start_matches(' ');
        let (head, tail) = (head.strip_suffix('\n').unwrap_or(head).to_string(), tail.strip_prefix('\n').unwrap_or(tail).to_string());
        let second = match &kind {
            Kind::Item { style, indent, .. } => Kind::item(*style, *indent, tail),
            Kind::Quote(_) => Kind::Quote(tail),
            _ => Kind::Text(tail),
        };
        if let Some(t) = text_mut(doc, p.slot) {
            *t = head;
        }
        let b = Block::new(second);
        let id = b.id;
        doc.insert(i + 1, Block::new(Kind::Divider));
        doc.insert(i + 2, b);
        *c = Caret::at(start(Slot::block(id)));
    }
    doc.ensure_tail();
    true
}

// ------------------------------------------------------------ styles

/// A range edit on the selection, slot by slot: `f` takes a slot's text and
/// a range of it and gives back the new text and the range now holding
/// the same words (`None`: nothing there to change). The selection keeps
/// its words. Code and pictures are passed over.
fn edit_ranges(
    doc: &mut Doc,
    c: &mut Caret,
    f: impl Fn(&str, usize, usize) -> Option<(String, usize, usize)>,
) -> bool {
    let (a, b) = ordered(doc, *c);
    let forward = c.anchor == a;
    let parts: Vec<(Slot, usize, usize)> = if a.slot == b.slot {
        vec![(a.slot, a.byte, b.byte)]
    } else {
        coverage(doc, *c).parts.iter().map(|(s, &(f, t))| (*s, f, t)).collect()
    };
    let (mut na, mut nb) = (a, b);
    let mut changed = false;
    for (s, from, to) in parts {
        if from >= to || !styled(doc, s) {
            continue;
        }
        let Some((t, x, y)) = f(text(doc, s), from, to) else { continue };
        if s == a.slot {
            na.byte = x;
        }
        if s == b.slot {
            nb.byte = y;
        }
        if let Some(text) = text_mut(doc, s) {
            *text = t;
            changed = true;
        }
    }
    // An end in a slot that took no edit stays where it was.
    let (na, nb) = (clamp(doc, na), clamp(doc, nb));
    *c = if forward { Caret { anchor: na, head: nb } } else { Caret { anchor: nb, head: na } };
    changed
}

fn clamp(doc: &Doc, p: Pos) -> Pos {
    Pos { byte: p.byte.min(len(doc, p.slot)), ..p }
}

/// The style the caret is in: the letter before it (the first one
/// selected, with a selection) — what typing there would carry on.
pub fn style_at(doc: &Doc, c: Caret) -> inline::Style {
    let (a, _) = ordered(doc, c);
    if !styled(doc, a.slot) {
        return inline::Style::default();
    }
    let t = text(doc, a.slot);
    let v = Visible::of(t);
    let d = v.drawn(a.byte);
    let at = if c.is_empty() { d.checked_sub(1) } else { Some(d) };
    match at.filter(|&d| d < v.drawn_len()) {
        Some(d) => inline::styles(t)[v.source(d)],
        None => inline::Style::default(),
    }
}

/// Bold, italic, struck or code on the selection's words: off where they
/// all have it, else on for all of them.
pub fn style(doc: &mut Doc, c: &mut Caret, kind: inline::Kind) -> bool {
    edit_ranges(doc, c, |t, a, b| inline::emphasis(t, a, b, kind))
}

/// Colour on the selection (`inline::tint`): the words' own when `ink`,
/// else a highlighter; colour 0 takes it away.
pub fn tint(doc: &mut Doc, c: &mut Caret, ink: bool, color: u32) -> bool {
    edit_ranges(doc, c, |t, a, b| inline::tint(t, a, b, ink, color))
}

/// The link the caret is in: its slot, a place in its words and its
/// address.
pub fn link_at(doc: &Doc, c: Caret) -> Option<(Slot, usize, String)> {
    let p = c.head;
    if !styled(doc, p.slot) {
        return None;
    }
    inline::link_at(text(doc, p.slot), p.byte).map(|(_, url)| (p.slot, p.byte, url))
}

/// The link at `at` in `slot` given address `url`, or taken off its words
/// when `url` is `None`.
pub fn set_link(doc: &mut Doc, slot: Slot, at: usize, url: Option<&str>) -> bool {
    let t = text(doc, slot);
    let new = match url {
        Some(u) => inline::set_link(t, at, u),
        None => inline::unlink(t, at),
    };
    match (new, text_mut(doc, slot)) {
        (Some(n), Some(t)) => {
            *t = n;
            true
        }
        _ => false,
    }
}

/// The selection made a link to `url`; with nothing selected, the address
/// typed in as its own link.
pub fn make_link(doc: &mut Doc, c: &mut Caret, url: &str) -> bool {
    if c.is_empty() {
        if !styled(doc, c.head.slot) {
            return false;
        }
        let p = c.head;
        let link = format!("[{}]({})", url.trim(), inline::with_scheme(url).replace(' ', "%20"));
        if let Some(t) = text_mut(doc, p.slot) {
            t.insert_str(p.byte, &link);
            let v = Visible::of(t);
            let d = v.drawn(p.byte + link.len());
            *c = Caret::at(Pos { byte: v.at(d), ..p });
        }
        return true;
    }
    edit_ranges(doc, c, |t, a, b| inline::make_link(t, a, b, url))
}

/// `s` typed at the caret inside `pre` and `post`: a style switched on
/// (or off) with nothing selected.
pub fn insert_styled(doc: &mut Doc, c: &mut Caret, s: &str, pre: &str, post: &str) -> bool {
    if !c.is_empty() || !styled(doc, c.head.slot) {
        return insert(doc, c, s);
    }
    let p = c.head;
    let Some(t) = text_mut(doc, p.slot) else { return false };
    let (n, caret) = inline::insert_styled(t, p.byte, s, pre, post);
    *t = n;
    *c = Caret::at(Pos { byte: caret, ..p });
    true
}

// ------------------------------------------------------------ typing

/// Styles switched on (or off) with nothing selected, for what is typed
/// next while the caret stays where it was: each a layer — which style
/// (the caller's name for it), and the marks it goes between — so bold
/// then italic types both.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub caret: Caret,
    pub layers: Vec<(String, String, String)>,
}

impl Pending {
    /// The marks around what is typed: the layers nested, first outermost.
    pub fn marks(&self) -> (String, String) {
        marks_of(&self.layers)
    }
}

fn marks_of(layers: &[(String, String, String)]) -> (String, String) {
    let pre = layers.iter().map(|l| l.1.as_str()).collect();
    let post = layers.iter().rev().map(|l| l.2.as_str()).collect();
    (pre, post)
}

/// A space typed at the end of a styled run, put just past its closing
/// marks (Markdown's `**bold **` is no bold at all): `close` is where those
/// marks are, `end` where the spaces end. The next letter typed there
/// takes the spaces back inside with it, so the run goes on; the caret
/// going anywhere else leaves them plain, past the run, as a file has them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Carry {
    slot: Slot,
    close: (usize, usize),
    end: usize,
}

/// What typing keeps between one key and the next: the styles switched on
/// for what comes next ([`Pending`]), and spaces carried past a run's
/// marks ([`Carry`]). Text goes in with [`Typing::type_text`], so a space
/// never stands where Markdown's `**`, `*`, `~~` or `==` can't have one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Typing {
    pub pending: Option<Pending>,
    carry: Option<Carry>,
    /// The colour a pending highlighter (0) and words' colour (1) give
    /// what is typed next, 0 for none: read while their layer is pending.
    tinted: [u32; 2],
}

/// A space, as far as marks are concerned (a line break is not typed).
fn spacing(ch: char) -> bool {
    ch.is_whitespace() && ch != '\n'
}

/// A pair whose marks can't stand beside a space (`**`, `*`, `~~`, `==`).
fn flanked(text: &str, p: &inline::Pair) -> bool {
    match p.kind {
        inline::Kind::Bold | inline::Kind::Italic | inline::Kind::Strike => true,
        inline::Kind::Tint => &text[p.open.0..p.open.1] == "==",
        _ => false,
    }
}

/// Where a run of such pairs' closing marks, starting at `at`, ends.
fn closings(text: &str, at: usize) -> Option<usize> {
    let ps = inline::pairs(text);
    let mut end = at;
    while let Some(p) = ps.iter().find(|p| p.close.0 == end && flanked(text, p)) {
        end = p.close.1;
    }
    (end > at).then_some(end)
}

/// Where a run of such pairs' opening marks, ending at `at`, starts.
fn openings(text: &str, at: usize) -> Option<usize> {
    let ps = inline::pairs(text);
    let mut start = at;
    while let Some(p) = ps.iter().find(|p| p.open.1 == start && flanked(text, p)) {
        start = p.open.0;
    }
    (start < at).then_some(start)
}

/// Whether a pending layer switches off a run the place `at` is in (its
/// marks the run's closing and opening ones, as [`inline::switch_at`]
/// gives them), and not on: of the runs `within` only.
fn switches_off(text: &str, at: usize, l: &(String, String, String), within: impl Fn(&inline::Pair) -> bool) -> bool {
    inline::pairs(text).iter().any(|p| {
        p.open.1 <= at && at <= p.close.0 && within(p) && text[p.close.0..p.close.1] == l.1 && text[p.open.0..p.open.1] == l.2
    })
}

/// A slot's text, `a..b` made `with`, the caret at `caret`.
fn splice(doc: &mut Doc, c: &mut Caret, slot: Slot, a: usize, b: usize, with: &str, caret: usize) -> bool {
    let Some(t) = text_mut(doc, slot) else { return false };
    t.replace_range(a..b, with);
    *c = Caret::at(Pos { slot, byte: caret });
    true
}

impl Typing {
    /// The styles switched on for what is typed at `c`, if any.
    pub fn pending_at(&self, c: Caret) -> Option<&Pending> {
        self.pending.as_ref().filter(|p| p.caret == c)
    }

    /// Layer `key` for what is typed at `c`: put in, or taken out when it
    /// is in already. Ones switched on elsewhere are let go.
    pub fn pend(&mut self, c: Caret, key: &str, pre: String, post: String) {
        let p = self.pending.get_or_insert_with(|| Pending { caret: c, layers: Vec::new() });
        if p.caret != c {
            *p = Pending { caret: c, layers: Vec::new() };
        }
        match p.layers.iter().position(|l| l.0 == key) {
            Some(i) => {
                p.layers.remove(i);
            }
            None => p.layers.push((key.to_string(), pre, post)),
        }
        if p.layers.is_empty() {
            self.pending = None;
        }
    }

    /// Layer `key` taken out of what is typed at `c`, if it is in.
    pub fn unpend(&mut self, c: Caret, key: &str) {
        if let Some(p) = self.pending.as_mut().filter(|p| p.caret == c) {
            p.layers.retain(|l| l.0 != key);
            if p.layers.is_empty() {
                self.pending = None;
            }
        }
    }

    /// The caret went elsewhere, or something other than typing was done:
    /// spaces carried past a run stay there, plain.
    pub fn moved(&mut self) {
        self.carry = None;
    }

    /// The spaces carried, if the caret is just past them and they are
    /// still where they were put.
    fn carry_at(&self, doc: &Doc, c: Caret) -> Option<Carry> {
        let k = self.carry?;
        if !c.is_empty() || c.head != (Pos { slot: k.slot, byte: k.end }) {
            return None;
        }
        let t = text(doc, k.slot);
        let spaces = t.get(k.close.1..k.end)?;
        (closings(t, k.close.0) == Some(k.close.1) && spaces.chars().all(spacing)).then_some(k)
    }

    /// Where the caret counts as being for styles: inside the run its
    /// carried spaces are past.
    fn styled_at(&self, doc: &Doc, c: Caret) -> Caret {
        match self.carry_at(doc, c) {
            Some(k) => Caret::at(Pos { slot: k.slot, byte: k.close.0 }),
            None => c,
        }
    }

    /// The style the caret is in ([`style_at`]): spaces carried past a
    /// run's marks are the run's, still.
    pub fn style_at(&self, doc: &Doc, c: Caret) -> inline::Style {
        style_at(doc, self.styled_at(doc, c))
    }

    /// The marks that switch `kind` on at the caret, or off when it is in
    /// a run of it ([`inline::switch_at`]), for [`Typing::pend`].
    pub fn switch(&self, doc: &Doc, c: Caret, kind: inline::Kind) -> (String, String) {
        let p = self.styled_at(doc, c).head;
        inline::switch_at(text(doc, p.slot), p.byte, kind)
    }

    /// A colour for what is typed next at `c`, with nothing selected: the
    /// words' own (`ink`) or a highlighter's. A switch, as Bold is: the
    /// colour typing would carry already turns it off, and 0 takes any
    /// off — inside a run of it, the run is closed for the words typed,
    /// its other colour kept.
    pub fn pend_tint(&mut self, doc: &Doc, c: Caret, ink: bool, color: u32) {
        let key = if ink { "ink" } else { "mark" };
        let k = ink as usize;
        let style = self.style_at(doc, c);
        let run = if ink { style.ink } else { style.mark };
        let pending = |w: &Typing, key: &str| w.pending_at(c).is_some_and(|p| p.layers.iter().any(|l| l.0 == key));
        let now = if pending(self, key) { self.tinted[k] } else { run };
        let target = if color == now { 0 } else { color };
        self.unpend(c, key);
        if target == run {
            return;
        }
        self.tinted[k] = target;
        if target != 0 {
            let (pre, post) = inline::tint_tags(ink.then_some(target), (!ink).then_some(target)).unwrap_or_default();
            self.pend(c, key, pre, post);
            return;
        }
        let Some((close, open, other)) = self.tint_off(doc, c, ink) else { return };
        self.pend(c, key, close, open);
        // The run's other colour goes on in the words typed.
        let other_key = if ink { "mark" } else { "ink" };
        if let Some(o) = other.filter(|_| !pending(self, other_key)) {
            let (pre, post) = inline::tint_tags((!ink).then_some(o), ink.then_some(o)).unwrap_or_default();
            self.pend(c, other_key, pre, post);
            self.tinted[1 - k] = o;
        }
    }

    /// The colour a pending layer gives what is typed at `c`: the words'
    /// own (`ink`) or a highlighter's; `None` with none pending.
    pub fn pending_tint(&self, c: Caret, ink: bool) -> Option<u32> {
        let key = if ink { "ink" } else { "mark" };
        self.pending_at(c).filter(|p| p.layers.iter().any(|l| l.0 == key)).map(|_| self.tinted[ink as usize])
    }

    /// The marks that switch a colour off at the caret, for what is typed
    /// next, when it is in a run of one: the words' own (`ink`) or a
    /// highlighter's. The run's closing tag and its opening one, and the
    /// run's other colour, if it has one, which the words typed keep.
    pub fn tint_off(&self, doc: &Doc, c: Caret, ink: bool) -> Option<(String, String, Option<u32>)> {
        let p = self.styled_at(doc, c).head;
        let t = text(doc, p.slot);
        let run = inline::tints(t)
            .into_iter()
            .filter(|r| r.open.1 <= p.byte && p.byte <= r.close.0)
            .filter(|r| if ink { r.ink.is_some() } else { r.mark.is_some() })
            .max_by_key(|r| r.open.0)?;
        let other = if ink { run.mark } else { run.ink };
        Some((t[run.close.0..run.close.1].to_string(), t[run.open.0..run.open.1].to_string(), other))
    }

    /// `s` typed over the selection, or at the caret: inside the styles
    /// switched on for it, if any. Spaces and words go in a run at a time,
    /// so a space never stands where a run's marks can't have it: at a
    /// run's end it goes just past its closing marks, and is carried
    /// ([`Carry`]); at its start, before its opening ones; with a style
    /// switched on, plain, the style still on for the letters after it.
    pub fn type_text(&mut self, doc: &mut Doc, c: &mut Caret, s: &str) -> bool {
        let mut changed = false;
        if !c.is_empty() {
            self.pending = None;
            self.carry = None;
            if s.is_empty() {
                return delete_selection(doc, c);
            }
            changed = replace_selection(doc, c);
        }
        let mut rest = s;
        while !rest.is_empty() {
            let space = rest.starts_with(spacing);
            let n = rest.find(|ch: char| spacing(ch) != space).unwrap_or(rest.len());
            changed |= if space { self.spaces(doc, c, &rest[..n]) } else { self.letters(doc, c, &rest[..n]) };
            rest = &rest[n..];
        }
        changed
    }

    /// Spaces alone, at the caret.
    fn spaces(&mut self, doc: &mut Doc, c: &mut Caret, s: &str) -> bool {
        let pending = self.pending.take().filter(|q| q.caret == *c);
        let p = c.head;
        if !styled(doc, p.slot) {
            self.carry = None;
            return insert(doc, c, s);
        }
        if let Some(k) = self.carry_at(doc, *c) {
            // More spaces after those carried: carried too.
            let changed = splice(doc, c, p.slot, k.end, k.end, s, k.end + s.len());
            self.carry = Some(Carry { end: k.end + s.len(), ..k });
            self.pending = pending.map(|q| Pending { caret: *c, ..q });
            return changed;
        }
        self.carry = None;
        let t = text(doc, p.slot).to_string();
        let (off, on): (Vec<_>, Vec<_>) = pending
            .map(|q| q.layers)
            .unwrap_or_default()
            .into_iter()
            .partition(|l| switches_off(&t, p.byte, l, |_| true));
        let changed = if !off.is_empty() {
            // A style switched off, then a space: the space outside its
            // run, as the letters after it will be.
            let (pre, post) = marks_of(&off);
            insert_styled(doc, c, s, &pre, &post)
        } else if let Some(end) = closings(&t, p.byte) {
            // At a run's end: past its marks, carried.
            let changed = splice(doc, c, p.slot, end, end, s, end + s.len());
            self.carry = Some(Carry { slot: p.slot, close: (p.byte, end), end: end + s.len() });
            changed
        } else if let Some(start) = openings(&t, p.byte) {
            // At a run's start: before its marks.
            splice(doc, c, p.slot, start, start, s, start + s.len())
        } else {
            insert(doc, c, s)
        };
        // A style switched on stays on for the letters after the space.
        if !on.is_empty() {
            self.pending = Some(Pending { caret: *c, layers: on });
        }
        changed
    }

    /// Letters alone (no space among them), at the caret.
    fn letters(&mut self, doc: &mut Doc, c: &mut Caret, s: &str) -> bool {
        let pending = self.pending.take().filter(|q| q.caret == *c);
        let Some(k) = self.carry_at(doc, *c) else {
            self.carry = None;
            return match pending {
                Some(q) => {
                    let (pre, post) = q.marks();
                    insert_styled(doc, c, s, &pre, &post)
                }
                None => insert(doc, c, s),
            };
        };
        self.carry = None;
        let t = text(doc, k.slot).to_string();
        let carried = |p: &inline::Pair| k.close.0 <= p.close.0 && p.close.1 <= k.close.1;
        let (off, on): (Vec<_>, Vec<_>) = pending
            .map(|q| q.layers)
            .unwrap_or_default()
            .into_iter()
            .partition(|l| switches_off(&t, k.close.0, l, carried));
        if !off.is_empty() {
            // The carried run switched off: the letters past it, after
            // the spaces, in what else is switched on.
            if on.is_empty() {
                return insert(doc, c, s);
            }
            let (pre, post) = marks_of(&on);
            return insert_styled(doc, c, s, &pre, &post);
        }
        // The run goes on: the spaces back inside it, with these.
        let spaces = &t[k.close.1..k.end];
        let marks = &t[k.close.0..k.close.1];
        if on.is_empty() {
            let moved = format!("{spaces}{s}{marks}");
            return splice(doc, c, k.slot, k.close.0, k.end, &moved, k.close.0 + spaces.len() + s.len());
        }
        let back = format!("{}{spaces}{marks}{}", &t[..k.close.0], &t[k.end..]);
        let (pre, post) = marks_of(&on);
        let (out, caret) = inline::insert_styled(&back, k.close.0 + spaces.len(), s, &pre, &post);
        splice(doc, c, k.slot, 0, t.len(), &out, caret)
    }
}

/// Whether `p` lies inside the caret's selection.
pub fn contains(doc: &Doc, c: Caret, p: Pos) -> bool {
    if c.is_empty() {
        return false;
    }
    let (a, b) = ordered(doc, c);
    let all = slots(doc);
    let key = |q: Pos| (index(&all, q.slot), q.byte);
    key(a) <= key(p) && key(p) <= key(b)
}

// ------------------------------------------------------------ clipboard

/// What Ctrl+C takes: the text, within one slot; across blocks, the
/// blocks it covers as Markdown, cut to the selection at both ends. A list
/// item's words chosen whole are the item: `- [ ] milk`, its box (or
/// bullet, or number) and all, as a copy of several items is.
pub fn copy(doc: &Doc, c: Caret) -> Option<String> {
    if c.is_empty() {
        return None;
    }
    let (a, b) = ordered(doc, c);
    if a.slot == b.slot && !is_atomic(doc, a.slot) {
        let t = text(doc, a.slot);
        let item = a.slot.cell.is_none() && matches!(kind_at(doc, a.slot), Some(Kind::Item { .. }));
        let v = visible(doc, a.slot);
        if item && v.drawn_len() > 0 && v.drawn(a.byte) == 0 && v.drawn(b.byte) == v.drawn_len() {
            let block = doc.get(a.slot.block).map(|b| (*b).clone())?;
            return Some(doc::to_markdown(None, &Doc::new(vec![block])));
        }
        // A part of a line keeps the styles it was in, its marks whole.
        return Some(if styled(doc, a.slot) { inline::slice(t, a.byte, b.byte) } else { t[a.byte..b.byte].to_string() });
    }
    let r = spanned(doc, c);
    let mut blocks: Vec<Block> = doc.blocks()[r.clone()].iter().map(|b| (**b).clone()).collect();
    let last = blocks.len() - 1;
    let marks = |b: &Block| !matches!(b.kind, Kind::Raw(_));
    if b.slot.cell.is_none() {
        let m = marks(&blocks[last]);
        if let Some(t) = blocks[last].text_mut() {
            *t = split_text(t, b.byte.min(t.len()), m).0;
        }
    }
    if a.slot.cell.is_none() {
        let m = marks(&blocks[0]);
        if let Some(t) = blocks[0].text_mut() {
            *t = split_text(t, a.byte.min(t.len()), m).1;
        }
    }
    Some(doc::to_markdown(None, &Doc::new(blocks)))
}

/// Ctrl+V: one line types in; several lines that read as structure (a
/// list, a table, headings) become blocks at the caret, the text after the
/// caret carrying on below them.
pub fn paste(doc: &mut Doc, c: &mut Caret, s: &str) -> bool {
    let s = s.replace("\r\n", "\n");
    let p = c.head;
    let raw_or_cell = p.slot.cell.is_some()
        || matches!(kind_at(doc, p.slot), Some(Kind::Raw(_)));
    if !s.contains('\n') || raw_or_cell {
        return insert(doc, c, &s);
    }
    let parsed = doc::parse_blocks(s.trim_end_matches('\n'));
    // Only blank lines (an empty line copied in an editor is "\r\n"):
    // a line break, as Enter makes.
    if parsed.is_empty() {
        replace_selection(doc, c);
        return enter(doc, c, false);
    }
    if parsed.len() == 1 && matches!(parsed[0].kind, Kind::Text(_)) {
        return insert(doc, c, s.trim_end_matches('\n'));
    }
    replace_selection(doc, c);
    let p = c.head;
    let Some(i) = doc.index_of(p.slot.block) else { return false };
    // Split the block at the caret; the pasted blocks go between.
    let t = text(doc, p.slot).to_string();
    let byte = floor_char(&t, p.byte);
    let tail = t[byte..].to_string();
    let head_empty = byte == 0;
    if !is_atomic(doc, p.slot)
        && let Some(t) = text_mut(doc, p.slot)
    {
        t.truncate(byte);
    }
    let last = parsed.last().cloned();
    let at = if head_empty && matches!(doc.blocks()[i].kind, Kind::Text(_)) {
        doc.remove(i);
        i
    } else {
        i + 1
    };
    let n = parsed.len();
    doc.splice(at..at, parsed);
    let last_i = at + n - 1;
    // The text after the caret: onto the last pasted block if it has text,
    // else a paragraph of its own.
    let last = last.unwrap();
    let end_byte = last.text_ref().map_or(0, str::len);
    if !tail.is_empty() {
        if last.text_ref().is_some() && !matches!(last.kind, Kind::Raw(_)) {
            if let Some(t) = doc.block_mut(last_i).text_mut() {
                t.push_str(&tail.replace('\n', " "));
            }
        } else {
            doc.insert(last_i + 1, Block::text(tail));
        }
    }
    *c = Caret::at(match &last.kind {
        Kind::Table(_) | Kind::Divider | Kind::Image { .. } => {
            let b = &doc.blocks()[last_i];
            end(doc, *block_slots(b).last().unwrap())
        }
        _ => Pos {
            slot: Slot::block(last.id),
            byte: end_byte,
        },
    });
    keep_writable(doc, c);
    true
}

// ------------------------------------------------------------ tables

/// The toolbar's table tools, at the caret's cell. The caret follows.
pub fn table_op(doc: &mut Doc, c: &mut Caret, op: TableOp, desc: bool) -> bool {
    let p = c.head;
    let (Some(i), Some((row, col))) = (doc.index_of(p.slot.block), p.slot.cell) else {
        return false;
    };
    let block = p.slot.block;
    let Kind::Table(t) = &mut doc.block_mut(i).kind else { return false };
    let (Some(r), Some(k)) = (t.row_index(row), t.col_index(col)) else { return false };
    let cell = |row, col| Slot {
        block,
        cell: Some((row, col)),
    };
    let to = match op {
        TableOp::AddRow => Some(cell(t.insert_row(r + 1), col)),
        TableOp::AddCol => Some(cell(row, t.insert_col(k + 1))),
        TableOp::DelRow => t
            .remove_row(r)
            .then(|| cell(t.rows[r.min(t.rows.len() - 1)].id, col)),
        TableOp::DelCol => t
            .remove_col(k)
            .then(|| cell(row, t.cols[k.min(t.width() - 1)])),
        TableOp::Sort => {
            let moved = t.sort_by(k, desc);
            return moved;
        }
    };
    match to {
        Some(s) => {
            *c = Caret::at(start(s));
            true
        }
        None => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableOp {
    AddRow,
    AddCol,
    DelRow,
    DelCol,
    Sort,
}

/// The table the caret is in, whole: what the toolbar's "remove table"
/// does.
pub fn remove_block(doc: &mut Doc, c: &mut Caret) -> bool {
    let r = touched(doc, *c);
    let i = *r.start();
    doc.splice(i..*r.end() + 1, []);
    *c = Caret::at(after_removal(doc, i));
    keep_writable(doc, c);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Float, Table};

    fn doc(blocks: Vec<Block>) -> Doc {
        Doc::new(blocks)
    }

    fn at(doc: &Doc, i: usize, byte: usize) -> Caret {
        let s = block_slots(&doc.blocks()[i])[0];
        Caret::at(Pos { slot: s, byte })
    }

    fn texts(doc: &Doc) -> Vec<String> {
        doc.blocks()
            .iter()
            .map(|b| match &b.kind {
                Kind::Divider => "---".to_string(),
                Kind::Table(_) => "<table>".to_string(),
                Kind::Item { text, done, .. } => format!("[{}]{text}", if *done { "x" } else { " " }),
                k => format!("{:?}", k).chars().take(4).collect::<String>() + ":" + b.text_ref().unwrap_or(""),
            })
            .collect()
    }

    fn todo(text: &str, done: bool) -> Block {
        Block::new(Kind::Item {
            style: ListStyle::Todo,
            indent: 0,
            done,
            num: 0,
            text: text.into(),
        })
    }

    #[test]
    fn a_last_list_item_moves_down_no_further_than_the_end() {
        let mut d = doc(vec![todo("one", false), todo("two", false)]);
        d.ensure_tail();
        let n = d.len();
        let c = at(&d, 0, 0);
        assert!(move_blocks(&mut d, c, 1), "past its neighbour");
        let c = at(&d, 1, 0);
        assert!(!move_blocks(&mut d, c, 1), "not past the empty line after the list");
        assert_eq!(d.len(), n, "and no empty lines made");
    }

    #[test]
    fn a_selection_takes_no_block_it_only_reaches_the_edge_of() {
        let mut d = doc(vec![Block::text("one"), Block::text("two"), Block::text("three"), Block::text("")]);
        let pos = |d: &Doc, i: usize, byte: usize| at(d, i, byte).head;
        // "two" from its start to the start of "three": "two" alone, as drawn.
        let c = Caret { anchor: pos(&d, 1, 0), head: pos(&d, 2, 0) };
        assert_eq!(touched(&d, c), 1..=1);
        assert_eq!(spanned(&d, c), 1..=2);
        // The other way round, and from the end of "one" to the end of "two".
        let c = Caret { anchor: pos(&d, 2, 0), head: pos(&d, 1, 0) };
        assert_eq!(touched(&d, c), 1..=1);
        let c = Caret { anchor: pos(&d, 0, 3), head: pos(&d, 1, 3) };
        assert_eq!(touched(&d, c), 1..=1);
        // A byte into the next block takes it; an empty line it starts on too.
        let c = Caret { anchor: pos(&d, 1, 0), head: pos(&d, 2, 1) };
        assert_eq!(touched(&d, c), 1..=2);
        let c = Caret { anchor: pos(&d, 3, 0), head: pos(&d, 2, 0) };
        assert_eq!(touched(&d, c), 2..=2);
        // Only the seam between two blocks: one of them, never none.
        let c = Caret { anchor: pos(&d, 0, 3), head: pos(&d, 1, 0) };
        assert_eq!(touched(&d, c), 0..=0);
        // Alt+↓ on "two" so selected moves "two" alone.
        let c = Caret { anchor: pos(&d, 1, 0), head: pos(&d, 2, 0) };
        assert!(move_blocks(&mut d, c, 1));
        assert_eq!(texts(&d)[..3], ["Text:one", "Text:three", "Text:two"]);
    }

    #[test]
    fn a_double_click_drag_keeps_its_word_either_way() {
        let d = doc(vec![Block::text("alpha beta gamma"), Block::text("delta epsilon")]);
        let s = block_slots(&d.blocks()[0])[0];
        let pos = |slot, byte| Pos { slot, byte };
        let beta = word_at(&d, pos(s, 7));
        // Back into "alpha": beta stays, alpha comes whole.
        let c = extend_by(&d, beta, pos(s, 2), true);
        assert_eq!((c.anchor.byte, c.head.byte), (10, 0));
        // Forward into "gamma".
        let c = extend_by(&d, beta, pos(s, 13), true);
        assert_eq!((c.anchor.byte, c.head.byte), (6, 16));
        // Inside the word itself: just the word.
        let c = extend_by(&d, beta, pos(s, 8), true);
        assert_eq!((c.anchor.byte, c.head.byte), (6, 10));
        // A triple click's line, dragged down: whole lines.
        let t = block_slots(&d.blocks()[1])[0];
        let c = extend_by(&d, select_slot(&d, s), pos(t, 3), false);
        assert_eq!((c.anchor, c.head), (pos(s, 0), pos(t, 13)));
    }

    #[test]
    fn enter_at_the_start_of_a_checked_item_opens_one_above() {
        let mut d = doc(vec![todo("milk", true)]);
        let mut c = at(&d, 0, 0);
        enter(&mut d, &mut c, false);
        assert_eq!(texts(&d)[..2], ["[ ]", "[x]milk"]);
        // The caret stays with "milk".
        assert_eq!(text(&d, c.head.slot), "milk");
        assert_eq!(c.head.byte, 0);
    }

    #[test]
    fn enter_splits_items_and_paragraphs() {
        let mut d = doc(vec![todo("applepie", true)]);
        let mut c = at(&d, 0, 5);
        enter(&mut d, &mut c, false);
        assert_eq!(texts(&d)[..2], ["[x]apple", "[ ]pie"]);
        let mut d = doc(vec![Block::text("one two")]);
        let mut c = at(&d, 0, 3);
        enter(&mut d, &mut c, false);
        assert_eq!(d.blocks()[1].text_ref(), Some(" two"));
        assert_eq!(c.head.slot.block, d.blocks()[1].id);
    }

    #[test]
    fn backspace_deletes_an_empty_line_and_joins_lines() {
        let mut d = doc(vec![Block::text("one"), Block::text(""), Block::text("two")]);
        let mut c = at(&d, 1, 0);
        assert!(backspace(&mut d, &mut c, false));
        assert_eq!(d.len(), 2);
        assert_eq!(c.head, Pos { slot: Slot::block(d.blocks()[0].id), byte: 3 });
        // "two" joins "one" where the caret was.
        let mut c = at(&d, 1, 0);
        backspace(&mut d, &mut c, false);
        assert_eq!(d.blocks()[0].text_ref(), Some("onetwo"));
        assert_eq!(c.head.byte, 3);
    }

    #[test]
    fn backspace_undoes_structure_before_joining() {
        let mut d = doc(vec![Block::text("x"), Block::new(Kind::item(ListStyle::Bullet, 1, "a"))]);
        let mut c = at(&d, 1, 0);
        backspace(&mut d, &mut c, false);
        assert!(matches!(d.blocks()[1].kind, Kind::Item { indent: 0, .. }));
        backspace(&mut d, &mut c, false);
        assert!(matches!(&d.blocks()[1].kind, Kind::Text(t) if t == "a"));
        backspace(&mut d, &mut c, false);
        assert_eq!(d.blocks()[0].text_ref(), Some("xa"));
    }

    #[test]
    fn a_blank_paste_is_a_line_break_and_a_selection_from_a_divider_keeps_the_text() {
        let mut d = doc(vec![Block::text("ab")]);
        let mut c = at(&d, 0, 0);
        assert!(paste(&mut d, &mut c, "\r\n"));
        assert!(paste(&mut d, &mut c, "  \n  \n"));
        assert_eq!(texts(&d).len(), 3, "no panic: two line breaks");

        let mut d = doc(vec![Block::new(Kind::Divider), Block::text("hello world")]);
        let a = Pos { slot: Slot::block(d.blocks()[0].id), byte: 1 };
        let b = Pos { slot: Slot::block(d.blocks()[1].id), byte: 5 };
        let mut c = Caret { anchor: a, head: b };
        delete_selection(&mut d, &mut c);
        assert_eq!(texts(&d), ["---", "Text: world"], "only what was selected goes");
    }

    #[test]
    fn backspace_at_a_heading_is_about_the_line_above() {
        let heading = |t: &str| Block::new(Kind::Heading { level: 2, text: t.into() });
        // An empty line above goes; the heading stays a heading.
        let mut d = doc(vec![Block::text("intro"), Block::text(""), heading("Plan")]);
        let mut c = at(&d, 2, 0);
        assert!(backspace(&mut d, &mut c, false));
        assert_eq!(texts(&d), ["Text:intro", "Head:Plan"]);
        assert_eq!(c.head, Pos { slot: Slot::block(d.blocks()[1].id), byte: 0 });
        // A written line above takes the heading's words.
        assert!(backspace(&mut d, &mut c, false));
        assert_eq!(texts(&d), ["Text:introPlan"]);
        assert_eq!(c.head.byte, 5);
        // With nothing above, it turns into text.
        let mut d = doc(vec![heading("Top")]);
        let mut c = at(&d, 0, 0);
        backspace(&mut d, &mut c, false);
        assert_eq!(texts(&d), ["Text:Top"]);
    }

    #[test]
    fn a_table_goes_in_two_backspaces() {
        let mut d = doc(vec![
            Block::text("before"),
            Block::new(Kind::Table(Table::new(2, 2))),
            Block::text(""),
        ]);
        let mut c = at(&d, 2, 0);
        // The empty line goes and the table is selected...
        backspace(&mut d, &mut c, false);
        assert_eq!(texts(&d), ["Text:before", "<table>", "Text:"]);
        assert!(coverage(&d, c).whole.contains(&d.blocks()[1].id));
        // ...and the next Backspace takes it.
        backspace(&mut d, &mut c, false);
        assert_eq!(texts(&d), ["Text:before", "Text:"]);
    }

    /// A checklist item's words chosen whole copy as the item, its box
    /// with them; part of them, as words. A bullet and a number alike.
    #[test]
    fn a_whole_list_item_copies_with_its_marker() {
        let mut done = Block::new(Kind::item(ListStyle::Todo, 0, "**milk** and eggs"));
        if let Kind::Item { done: d, .. } = &mut done.kind {
            *d = true;
        }
        let d = doc(vec![
            Block::new(Kind::item(ListStyle::Todo, 1, "bread")),
            done,
            Block::new(Kind::item(ListStyle::Bullet, 0, "a point")),
            Block::new(Kind::Item { style: ListStyle::Number, indent: 0, done: false, num: 3, text: "third".into() }),
        ]);
        let whole = |i: usize| {
            let s = block_slots(&d.blocks()[i])[0];
            Caret { anchor: start(s), head: end(&d, s) }
        };
        assert_eq!(copy(&d, whole(0)).as_deref(), Some("- [ ] bread\n"));
        assert_eq!(copy(&d, whole(1)).as_deref(), Some("- [x] **milk** and eggs\n"));
        assert_eq!(copy(&d, whole(2)).as_deref(), Some("- a point\n"));
        assert_eq!(copy(&d, whole(3)).as_deref(), Some("3. third\n"));
        // From inside the bold marks to the end: still the whole words.
        let s = block_slots(&d.blocks()[1])[0];
        let c = Caret { anchor: Pos { slot: s, byte: 2 }, head: end(&d, s) };
        assert_eq!(copy(&d, c).as_deref(), Some("- [x] **milk** and eggs\n"));
        // Part of the words: the words alone.
        let c = Caret { anchor: Pos { slot: s, byte: 2 }, head: Pos { slot: s, byte: 6 } };
        assert_eq!(copy(&d, c).as_deref(), Some("**milk**"));
        // Two items: both with their boxes, as ever.
        let s0 = block_slots(&d.blocks()[0])[0];
        let c = Caret { anchor: start(s0), head: end(&d, s) };
        assert_eq!(copy(&d, c).as_deref(), Some("- [ ] bread\n- [x] **milk** and eggs\n"));
    }

    #[test]
    fn a_selection_across_blocks_deletes_and_joins() {
        let mut d = doc(vec![
            Block::text("keep this"),
            Block::new(Kind::Divider),
            Block::new(Kind::item(ListStyle::Bullet, 0, "gone")),
            Block::text("and this end"),
        ]);
        let a = Pos { slot: Slot::block(d.blocks()[0].id), byte: 4 };
        let b = Pos { slot: Slot::block(d.blocks()[3].id), byte: 8 };
        let mut c = Caret { anchor: a, head: b };
        assert_eq!(copy(&d, c).as_deref(), Some(" this\n\n---\n\n- gone\n\nand this\n"));
        delete_selection(&mut d, &mut c);
        assert_eq!(texts(&d), ["Text:keep end"]);
        assert_eq!(c.head.byte, 4);
    }

    #[test]
    fn arrows_cross_blocks_and_cells() {
        let d = doc(vec![Block::text("ab"), Block::new(Kind::Table(Table::new(2, 1))), Block::text("")]);
        let mut c = at(&d, 0, 2);
        c = right(&d, c, false, false);
        assert_eq!(c.head.slot.cell.map(|_| ()), Some(()), "into the first cell");
        c = right(&d, c, false, false);
        c = right(&d, c, false, false);
        assert_eq!(c.head.slot, Slot::block(d.blocks()[2].id), "past the last cell");
        c = left(&d, c, false, false);
        assert!(c.head.slot.cell.is_some());
        // Up from the paragraph below lands in the table's last row.
        let up = vertical(&d, Slot::block(d.blocks()[2].id), true);
        assert_eq!(up.len(), 2);
        let down = vertical(&d, up[0], false);
        assert_eq!(down, [Slot::block(d.blocks()[2].id)]);
    }

    #[test]
    fn shortcuts_and_paste() {
        let mut d = doc(vec![Block::text("")]);
        let mut c = at(&d, 0, 0);
        insert(&mut d, &mut c, "-");
        insert(&mut d, &mut c, " ");
        assert!(matches!(d.blocks()[0].kind, Kind::Item { style: ListStyle::Bullet, .. }));
        insert(&mut d, &mut c, "milk");
        assert_eq!(text(&d, c.head.slot), "milk");

        let mut d = doc(vec![Block::text("intro after")]);
        let mut c = at(&d, 0, 6);
        paste(&mut d, &mut c, "- a\n- b\n");
        // The text after the caret carries on at the end of what was pasted.
        assert_eq!(texts(&d)[..3], ["Text:intro ", "[ ]a", "[ ]bafter"]);
        assert_eq!(text(&d, c.head.slot), "bafter");
        assert_eq!(c.head.byte, 1);
    }

    #[test]
    fn styles_wrap_and_unwrap_the_selection() {
        let mut d = doc(vec![Block::text("make this bold")]);
        let s = Slot::block(d.blocks()[0].id);
        let mut c = Caret {
            anchor: Pos { slot: s, byte: 10 },
            head: Pos { slot: s, byte: 14 },
        };
        style(&mut d, &mut c, inline::Kind::Bold);
        assert_eq!(text(&d, s), "make this **bold**");
        assert_eq!((c.anchor.byte, c.head.byte), (12, 16), "still selects the word");
        style(&mut d, &mut c, inline::Kind::Bold);
        assert_eq!(text(&d, s), "make this bold");
    }

    #[test]
    fn tab_indents_items_and_walks_cells() {
        let mut d = doc(vec![Block::new(Kind::item(ListStyle::Bullet, 0, "a"))]);
        let mut c = at(&d, 0, 1);
        assert_eq!(tab(&mut d, &mut c, false), (true, true));
        assert!(matches!(d.blocks()[0].kind, Kind::Item { indent: 1, .. }));
        let mut d = doc(vec![Block::new(Kind::Table(Table::new(2, 1)))]);
        let mut c = at(&d, 0, 0);
        tab(&mut d, &mut c, false);
        tab(&mut d, &mut c, false);
        let Kind::Table(t) = &d.blocks()[0].kind else { panic!() };
        assert_eq!(t.rows.len(), 2, "past the last cell, a row");
    }

    /// A divider goes where the caret is, and makes no empty line where
    /// there is a block to carry on in.
    #[test]
    fn a_divider_goes_where_the_caret_is() {
        let heading = || Block::new(Kind::Heading { level: 2, text: "Next".into() });
        // In the middle of a paragraph: cut there.
        let mut d = doc(vec![Block::text("Hello world"), heading()]);
        let mut c = at(&d, 0, 5);
        assert!(insert_block(&mut d, &mut c, Kind::Divider));
        assert_eq!(texts(&d)[..4], ["Text:Hello", "---", "Text:world", "Head:Next"]);
        assert_eq!(c, at(&d, 2, 0), "the caret at the second half's start");
        // At a paragraph's end, before a heading: no empty line between.
        let mut d = doc(vec![Block::text("Hello"), heading()]);
        let mut c = at(&d, 0, 5);
        insert_block(&mut d, &mut c, Kind::Divider);
        assert_eq!(texts(&d)[..3], ["Text:Hello", "---", "Head:Next"]);
        assert_eq!(c, at(&d, 2, 0));
        // At its start: before it, the caret staying.
        let mut d = doc(vec![Block::text("Hello"), heading()]);
        let mut c = at(&d, 0, 0);
        insert_block(&mut d, &mut c, Kind::Divider);
        assert_eq!(texts(&d)[..3], ["---", "Text:Hello", "Head:Next"]);
        assert_eq!(c, at(&d, 1, 0));
        // On an empty line: in its place.
        let mut d = doc(vec![Block::text("a"), Block::text(""), heading()]);
        let mut c = at(&d, 1, 0);
        insert_block(&mut d, &mut c, Kind::Divider);
        assert_eq!(texts(&d)[..3], ["Text:a", "---", "Head:Next"]);
        // At the note's end: a line to go on writing on.
        let mut d = doc(vec![Block::text("end")]);
        let mut c = at(&d, 0, 3);
        insert_block(&mut d, &mut c, Kind::Divider);
        assert_eq!(texts(&d), ["Text:end", "---", "Text:"]);
        assert_eq!(c, at(&d, 2, 0));
        // In a list item's middle: two items, the divider between.
        let mut d = doc(vec![Block::new(Kind::item(ListStyle::Bullet, 1, "one two"))]);
        let mut c = at(&d, 0, 3);
        insert_block(&mut d, &mut c, Kind::Divider);
        assert_eq!(texts(&d)[..3], ["[ ]one", "---", "[ ]two"]);
        assert!(matches!(d.blocks()[2].kind, Kind::Item { indent: 1, .. }));
        // Typed `---` on a line of its own before a heading: no empty line.
        let mut d = doc(vec![Block::text(""), heading()]);
        let mut c = at(&d, 0, 0);
        let mut t = Typing::default();
        t.type_text(&mut d, &mut c, "---");
        assert_eq!(texts(&d)[..2], ["---", "Head:Next"]);
    }

    /// Two pictures at the left edge and one on its own line below: a
    /// divider put in between makes no empty line under it (a gap before
    /// the picture), and a selected picture stays, the divider after it.
    #[test]
    fn a_divider_between_pictures_makes_no_empty_line() {
        let pic = |src: &str, float: Float| Block::new(Kind::Image { src: src.into(), alt: String::new(), width: Some(160), float, wiki: false });
        let note = || doc(vec![pic("a.png", Float::Left), pic("b.png", Float::Left), pic("c.png", Float::None), Block::text("Words")]);
        let kinds = |d: &Doc| {
            d.blocks()
                .iter()
                .map(|b| match &b.kind {
                    Kind::Image { src, .. } => src.clone(),
                    Kind::Divider => "---".into(),
                    k => format!("{k:?}"),
                })
                .collect::<Vec<_>>()
        };
        // The second picture selected.
        let mut d = note();
        let b = Slot::block(d.blocks()[1].id);
        let mut c = Caret { anchor: start(b), head: end(&d, b) };
        assert!(insert_block(&mut d, &mut c, Kind::Divider));
        assert_eq!(kinds(&d), ["a.png", "b.png", "---", "c.png", "Text(\"Words\")"]);
        let rule = Slot::block(d.blocks()[2].id);
        assert_eq!(c, Caret::at(end(&d, rule)), "the caret just past the divider");
        // Typing there starts a paragraph after it, before the picture.
        insert(&mut d, &mut c, "x");
        assert_eq!(kinds(&d)[2..5], ["---", "Text(\"x\")", "c.png"]);
        // An empty line between them: the divider in its place.
        let mut d = note();
        d.insert(2, Block::text(""));
        let mut c = at(&d, 2, 0);
        insert_block(&mut d, &mut c, Kind::Divider);
        assert_eq!(kinds(&d), ["a.png", "b.png", "---", "c.png", "Text(\"Words\")"]);
        // `---` typed there, the same.
        let mut d = note();
        d.insert(2, Block::text(""));
        let mut c = at(&d, 2, 0);
        Typing::default().type_text(&mut d, &mut c, "---");
        assert_eq!(kinds(&d), ["a.png", "b.png", "---", "c.png", "Text(\"Words\")"]);
        // At the end of words before a picture.
        let mut d = doc(vec![Block::text("Hello"), pic("c.png", Float::None)]);
        let mut c = at(&d, 0, 5);
        insert_block(&mut d, &mut c, Kind::Divider);
        assert_eq!(kinds(&d)[..3], ["Text(\"Hello\")", "---", "c.png"]);
    }

    /// Tab in text is a tab character, never spaces: typed in the middle
    /// of a line as at its start; Shift+Tab takes it back. Over several
    /// lines, each line gets one; code's fences are left alone.
    #[test]
    fn tab_in_text_is_a_tab_character() {
        let mut t = Typing::default();
        // In the middle of a paragraph: typed as a letter, inside no mark.
        let (mut d, mut c) = para("one **two** three", 3);
        assert!(tab_types(&d, c, false));
        t.type_text(&mut d, &mut c, "\t");
        assert_eq!(text(&d, c.head.slot), "one\t **two** three");
        // Shift+Tab takes back the tab before the caret.
        assert!(!tab_types(&d, c, true));
        assert_eq!(tab(&mut d, &mut c, true), (true, true));
        assert_eq!(text(&d, c.head.slot), "one **two** three");
        assert_eq!(c.head.byte, 3);
        // In a list item, the item goes in, wherever the caret is.
        let mut d = doc(vec![Block::new(Kind::item(ListStyle::Bullet, 0, "an item"))]);
        let mut c = at(&d, 0, 3);
        assert!(!tab_types(&d, c, false));
        assert_eq!(tab(&mut d, &mut c, false), (true, true));
        assert!(matches!(&d.blocks()[0].kind, Kind::Item { indent: 1, text, .. } if text == "an item"));
        // In code: a tab, and over several lines one at each line's start;
        // Shift+Tab takes a tab, or four spaces written for one.
        let mut d = doc(vec![Block::new(Kind::Raw("```\nfn a() {\n    x\n}\n```".into()))]);
        let s = Slot::block(d.blocks()[0].id);
        let mut c = Caret { anchor: Pos { slot: s, byte: 4 }, head: Pos { slot: s, byte: 20 } };
        assert!(!tab_types(&d, c, false));
        assert_eq!(tab(&mut d, &mut c, false), (true, true));
        assert_eq!(text(&d, s), "```\n\tfn a() {\n\t    x\n\t}\n```");
        assert_eq!((c.anchor.byte, c.head.byte), (5, 23), "the selection keeps its words");
        assert_eq!(tab(&mut d, &mut c, true), (true, true));
        assert_eq!(text(&d, s), "```\nfn a() {\n    x\n}\n```");
        assert_eq!(tab(&mut d, &mut c, true), (true, true));
        assert_eq!(text(&d, s), "```\nfn a() {\nx\n}\n```");
        assert!(!text(&d, s).contains("  "), "no spaces put in");
        // Paragraphs across blocks: a tab at each, the items among them in.
        let mut d = doc(vec![Block::text("first"), Block::new(Kind::item(ListStyle::Bullet, 0, "item")), Block::text("last")]);
        let (s0, s2) = (Slot::block(d.blocks()[0].id), Slot::block(d.blocks()[2].id));
        let mut c = Caret { anchor: Pos { slot: s0, byte: 2 }, head: Pos { slot: s2, byte: 2 } };
        assert_eq!(tab(&mut d, &mut c, false), (true, true));
        assert_eq!(texts(&d), ["Text:\tfirst", "[ ]item", "Text:\tlast"]);
        assert!(matches!(d.blocks()[1].kind, Kind::Item { indent: 1, .. }));
        assert_eq!((c.anchor.byte, c.head.byte), (3, 3));
        assert_eq!(tab(&mut d, &mut c, true), (true, true));
        assert_eq!(texts(&d), ["Text:first", "[ ]item", "Text:last"]);
    }

    /// A paragraph of `md`, the caret at place `d` of its words.
    fn para(md: &str, d: usize) -> (Doc, Caret) {
        let doc = doc(vec![Block::text(md)]);
        let s = Slot::block(doc.blocks()[0].id);
        let byte = visible(&doc, s).at(d);
        (doc, Caret::at(Pos { slot: s, byte }))
    }

    /// Types `s` a letter at a time, as a keyboard does; no mark is ever
    /// among the words drawn.
    fn keys(t: &mut Typing, d: &mut Doc, c: &mut Caret, s: &str) {
        for ch in s.chars() {
            t.type_text(d, c, &ch.to_string());
            let src = text(d, c.head.slot);
            let words = visible(d, c.head.slot).text(src);
            assert!(!words.contains(['*', '~', '=']), "a mark drawn in {words:?} ({src:?})");
        }
    }

    fn bold(t: &mut Typing, d: &Doc, c: Caret) {
        let (pre, post) = t.switch(d, c, inline::Kind::Bold);
        t.pend(c, "bold", pre, post);
    }

    #[test]
    fn a_space_after_bold_goes_past_its_marks_and_back_in_with_a_letter() {
        let (mut d, mut c) = para("", 0);
        let mut t = Typing::default();
        bold(&mut t, &d, c);
        keys(&mut t, &mut d, &mut c, "Bold ");
        assert_eq!(text(&d, c.head.slot), "**Bold** ", "never `**Bold **`");
        assert!(t.style_at(&d, c).bold, "the carried space is the run's");
        keys(&mut t, &mut d, &mut c, "here");
        assert_eq!(text(&d, c.head.slot), "**Bold here**");
        // Several spaces, carried together.
        keys(&mut t, &mut d, &mut c, "  now");
        assert_eq!(text(&d, c.head.slot), "**Bold here  now**");
        // Struck and highlighted runs too; and a word typed with its
        // space at once.
        for (md, words, out) in [("~~go~~", 2, "~~going on~~"), ("==go==", 2, "==going on=="), ("***go***", 2, "***going on***")] {
            let (mut d, mut c) = para(md, words);
            Typing::default().type_text(&mut d, &mut c, "ing on");
            assert_eq!(text(&d, c.head.slot), out);
        }
    }

    #[test]
    fn a_space_carried_stays_plain_when_the_caret_goes() {
        let (mut d, mut c) = para("say **hi**", 6);
        let mut t = Typing::default();
        keys(&mut t, &mut d, &mut c, " ");
        assert_eq!(text(&d, c.head.slot), "say **hi** ");
        t.moved();
        assert!(!t.style_at(&d, c).bold);
        keys(&mut t, &mut d, &mut c, "x");
        assert_eq!(text(&d, c.head.slot), "say **hi** x");
        // Undone under it: the carry is let go, not followed.
        let (mut d, mut c) = para("**hi**", 2);
        let mut t = Typing::default();
        keys(&mut t, &mut d, &mut c, " ");
        *doc_text(&mut d, c.head.slot) = "**hi**".into();
        c = Caret::at(Pos { byte: 6, ..c.head });
        keys(&mut t, &mut d, &mut c, "x");
        assert_eq!(text(&d, c.head.slot), "**hi**x");
    }

    fn doc_text(d: &mut Doc, s: Slot) -> &mut String {
        text_mut(d, s).unwrap()
    }

    #[test]
    fn a_space_with_a_style_switched_on_goes_in_plain() {
        let (mut d, mut c) = para("word.", 5);
        let mut t = Typing::default();
        bold(&mut t, &d, c);
        keys(&mut t, &mut d, &mut c, " ");
        assert_eq!(text(&d, c.head.slot), "word. ");
        assert!(t.pending_at(c).is_some(), "still on for the letters");
        keys(&mut t, &mut d, &mut c, "Bold here");
        assert_eq!(text(&d, c.head.slot), "word. **Bold here**");
        // A space at a run's start goes before its marks.
        let (mut d, mut c) = para("**bold**", 0);
        keys(&mut Typing::default(), &mut d, &mut c, " ");
        assert_eq!(text(&d, c.head.slot), " **bold**");
    }

    #[test]
    fn a_space_with_a_style_switched_off_goes_outside_its_run() {
        // At the run's end.
        let (mut d, mut c) = para("plain **bolder**", 12);
        let mut t = Typing::default();
        bold(&mut t, &d, c);
        keys(&mut t, &mut d, &mut c, " after");
        assert_eq!(text(&d, c.head.slot), "plain **bolder** after");
        // In its middle.
        let (mut d, mut c) = para("**abcd**", 2);
        let mut t = Typing::default();
        bold(&mut t, &d, c);
        keys(&mut t, &mut d, &mut c, " x");
        assert_eq!(text(&d, c.head.slot), "**ab** x**cd**");
        // Switched off after a space carried: the letters past the run.
        let (mut d, mut c) = para("**hi**", 2);
        let mut t = Typing::default();
        keys(&mut t, &mut d, &mut c, " ");
        bold(&mut t, &d, c);
        keys(&mut t, &mut d, &mut c, "x");
        assert_eq!(text(&d, c.head.slot), "**hi** x");
        // Another style switched on there: inside the run, as both.
        let (mut d, mut c) = para("**hi**", 2);
        let mut t = Typing::default();
        keys(&mut t, &mut d, &mut c, " ");
        let (pre, post) = t.switch(&d, c, inline::Kind::Italic);
        t.pend(c, "italic", pre, post);
        keys(&mut t, &mut d, &mut c, "x");
        let s = inline::styles(text(&d, c.head.slot));
        let v = visible(&d, c.head.slot);
        assert_eq!(v.text(text(&d, c.head.slot)), "hi x");
        let x = s[v.source(3)];
        assert!(x.bold && x.italic, "{}", text(&d, c.head.slot));
    }

    #[test]
    fn typing_over_a_selection_and_list_shortcuts_still_work() {
        let (mut d, _) = para("one **two** three", 0);
        let s = Slot::block(d.blocks()[0].id);
        let mut c = Caret { anchor: Pos { slot: s, byte: 12 }, head: Pos { slot: s, byte: 17 } };
        Typing::default().type_text(&mut d, &mut c, "3 and 4");
        assert_eq!(text(&d, s), "one **two** 3 and 4");
        let (mut d, mut c) = para("", 0);
        keys(&mut Typing::default(), &mut d, &mut c, "- milk");
        assert!(matches!(&d.blocks()[0].kind, Kind::Item { style: ListStyle::Bullet, text, .. } if text == "milk"));
    }
}
