//! Two versions of a note, compared: the newer one as a [`Doc`] whose
//! changes are marked in the note's own styles, so the page draws them
//! with nothing new to learn (docs/SYNC.md §9).
//!
//! - **Added** words are washed with the Green highlighter.
//! - **Removed** words are put back where they were, struck through in Red
//!   ink. A removed block comes back whole, struck.
//! - A block whose words stayed but which changed otherwise (a box ticked,
//!   a heading's level, an item's indent) is washed with the Yellow
//!   highlighter.
//!
//! The comparison goes block by block, matching the blocks that did not
//! change, then word by word inside a block that did. It is for reading:
//! the doc it makes is never written to a file.

use crate::doc::{self, Block, Doc, Kind, Table};
use crate::inline::{self, Visible};
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// The Green hue's wash, as a file writes it (design.rs `HUES`): added.
pub const ADDED: u32 = 0xd8eac0cc;
/// The Red hue's ink, as a file writes it: removed.
pub const REMOVED: u32 = 0xc0553fff;
/// The Yellow hue's wash: the same words, changed otherwise.
pub const CHANGED: u32 = 0xf8eba6cc;

/// Past this many block pairs (or word pairs in a block) the middle of a
/// comparison is taken as replaced whole rather than matched: a version
/// so unlike the last is read as new anyway.
const MATCH_CELLS: usize = 250_000;

/// `new`'s title, and its blocks with what changed since `old` marked.
pub fn marked(old: &str, new: &str) -> (Option<String>, Doc) {
    let (_, before) = doc::from_markdown(old);
    let (title, after) = doc::from_markdown(new);
    let (before, after) = (before.blocks(), after.blocks());
    let mut out = Vec::new();
    let eq = |i: usize, j: usize| same(&before[i].kind, &after[j].kind);
    for step in align(before.len(), after.len(), eq, |i| family(&before[i].kind), |j| family(&after[j].kind)) {
        match step {
            Step::Same(_, j) => out.push(Block::new(after[j].kind.clone())),
            Step::Changed(i, j) => out.push(Block::new(changed(&before[i].kind, &after[j].kind))),
            Step::Added(j) => out.push(Block::new(whole(&after[j].kind, Mark::Added))),
            Step::Removed(i) => {
                if let Some(k) = removed(&before[i].kind) {
                    out.push(Block::new(k));
                }
            }
        }
    }
    (title, Doc::new(out))
}

/// Words `new` has that `old` did not, and the other way round, counted
/// as bags of words: a moved paragraph is neither, a ticked box (`[x]`
/// for `[ ]`) no word at all.
pub fn word_delta(old: &str, new: &str) -> (usize, usize) {
    let mut count: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    for w in old.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)) {
        *count.entry(w).or_default() -= 1;
    }
    for w in new.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)) {
        *count.entry(w).or_default() += 1;
    }
    let added = count.values().filter(|&&n| n > 0).sum::<i64>() as usize;
    let gone = count.values().filter(|&&n| n < 0).map(|n| -n).sum::<i64>() as usize;
    (added, gone)
}

/// Blocks whose words can be compared: text, headings, items, quotes,
/// tables, each with its own kind. A picture, a divider, code: none.
fn family(k: &Kind) -> Option<u8> {
    match k {
        Kind::Text(_) => Some(0),
        Kind::Heading { .. } => Some(1),
        Kind::Item { .. } => Some(2),
        Kind::Quote(_) => Some(3),
        Kind::Table(_) => Some(4),
        _ => None,
    }
}

/// The same block, ids aside: a table is its cells.
fn same(a: &Kind, b: &Kind) -> bool {
    match (a, b) {
        (Kind::Table(x), Kind::Table(y)) => cells(x) == cells(y) && x.aligns == y.aligns,
        _ => a == b,
    }
}

fn cells(t: &Table) -> Vec<&Vec<String>> {
    t.rows.iter().map(|r| &r.cells).collect()
}

/// One block of the comparison, by its place in the old blocks (`i`) and
/// the new (`j`).
#[derive(Debug, PartialEq)]
enum Step {
    Same(usize, usize),
    /// Between the same unchanged ones, and paired: a paragraph edited,
    /// not one paragraph gone and another come.
    Changed(usize, usize),
    Added(usize),
    Removed(usize),
}

/// The steps from `n` old items to `m` new ones: the longest run of equal
/// ones kept; between two of those, each new one paired with the next old
/// one of its kind (`old_kind` and `new_kind` the same, none pairing with
/// nothing), the old ones it passes removed, and a new one with none
/// added. Common ends are trimmed first, so a note of 2,000 blocks with
/// one edited costs one comparison per block.
fn align<K: Eq + Hash>(
    n: usize,
    m: usize,
    eq: impl Fn(usize, usize) -> bool,
    old_kind: impl Fn(usize) -> Option<K>,
    new_kind: impl Fn(usize) -> Option<K>,
) -> Vec<Step> {
    let mut head = 0;
    while head < n && head < m && eq(head, head) {
        head += 1;
    }
    let mut tail = 0;
    while tail < n - head && tail < m - head && eq(n - 1 - tail, m - 1 - tail) {
        tail += 1;
    }
    let (on, om) = (n - head - tail, m - head - tail);
    let mut matched: Vec<(usize, usize)> = Vec::new();
    if on * om <= MATCH_CELLS {
        // Longest common subsequence over the middle.
        let mut len = vec![vec![0u32; om + 1]; on + 1];
        for i in (0..on).rev() {
            for j in (0..om).rev() {
                len[i][j] = if eq(head + i, head + j) {
                    len[i + 1][j + 1] + 1
                } else {
                    len[i + 1][j].max(len[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < on && j < om {
            if eq(head + i, head + j) {
                matched.push((head + i, head + j));
                i += 1;
                j += 1;
            } else if len[i + 1][j] >= len[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    let mut out: Vec<Step> = (0..head).map(|k| Step::Same(k, k)).collect();
    let (mut i, mut j) = (head, head);
    let ends = matched.into_iter().chain(std::iter::once((n - tail, m - tail)));
    for (mi, mj) in ends {
        // The gap before this match. Its old ones by kind, in order, each
        // list passed along as the new ones take from it: a gap of the
        // whole note (every block of it replaced) is read once.
        let mut oi = i;
        let mut olds: HashMap<K, VecDeque<usize>> = HashMap::new();
        if j < mj {
            for p in oi..mi {
                if let Some(k) = old_kind(p) {
                    olds.entry(k).or_default().push_back(p);
                }
            }
        }
        for nj in j..mj {
            let next = new_kind(nj).and_then(|k| {
                let q = olds.get_mut(&k)?;
                while q.front().is_some_and(|&p| p < oi) {
                    q.pop_front();
                }
                q.front().copied()
            });
            match next {
                Some(p) => {
                    out.extend((oi..p).map(Step::Removed));
                    out.push(Step::Changed(p, nj));
                    oi = p + 1;
                }
                None => out.push(Step::Added(nj)),
            }
        }
        out.extend((oi..mi).map(Step::Removed));
        if mi < n - tail {
            out.push(Step::Same(mi, mj));
        }
        (i, j) = (mi + 1, mj + 1);
    }
    out.extend((0..tail).map(|k| Step::Same(n - tail + k, m - tail + k)));
    out
}

#[derive(Clone, Copy)]
enum Mark {
    Added,
    Removed,
    Changed,
}

/// A new block put where an old one of its [`family`] was: its words are
/// compared. (`align` pairs no others; one that came anyway is marked
/// added whole.)
fn changed(old: &Kind, new: &Kind) -> Kind {
    if family(old).is_none() || family(old) != family(new) {
        return whole(new, Mark::Added);
    }
    if let (Kind::Table(a), Kind::Table(b)) = (old, new) {
        let mut t = b.clone();
        for (r, row) in t.rows.iter_mut().enumerate() {
            for (c, cell) in row.cells.iter_mut().enumerate() {
                *cell = match a.rows.get(r).and_then(|x| x.cells.get(c)) {
                    Some(was) => words(was, cell),
                    None => mark(cell, Mark::Added),
                };
            }
        }
        return Kind::Table(t);
    }
    let mut k = new.clone();
    let (Some(was), Some(text)) = (text_of(old), text_mut(&mut k)) else { return k };
    *text = if inline::plain(was) == inline::plain(text) {
        // The same words: the tick, the level or the indent changed.
        mark(text, Mark::Changed)
    } else {
        words(was, text)
    };
    k
}

/// A block marked whole: every word added, or removed, or changed.
fn whole(k: &Kind, how: Mark) -> Kind {
    let mut k = k.clone();
    match &mut k {
        Kind::Table(t) => {
            for row in &mut t.rows {
                for cell in &mut row.cells {
                    *cell = mark(cell, how);
                }
            }
        }
        _ => {
            if let Some(t) = text_mut(&mut k) {
                *t = mark(t, how);
            }
        }
    }
    k
}

/// An old block, put back struck. Not a picture, a divider or code: they
/// have no words to strike, and are left out.
fn removed(k: &Kind) -> Option<Kind> {
    match k {
        Kind::Text(_) | Kind::Heading { .. } | Kind::Item { .. } | Kind::Quote(_) | Kind::Table(_) => {
            Some(whole(k, Mark::Removed))
        }
        _ => None,
    }
}

fn text_of(k: &Kind) -> Option<&str> {
    match k {
        Kind::Text(t) | Kind::Quote(t) | Kind::Heading { text: t, .. } | Kind::Item { text: t, .. } => Some(t),
        _ => None,
    }
}

fn text_mut(k: &mut Kind) -> Option<&mut String> {
    match k {
        Kind::Text(t) | Kind::Quote(t) | Kind::Heading { text: t, .. } | Kind::Item { text: t, .. } => Some(t),
        _ => None,
    }
}

/// `text` with every word in it marked `how`.
fn mark(text: &str, how: Mark) -> String {
    let vis = Visible::of(text);
    let sealed = inline::sealed(text);
    let spans = vis.spans(0, vis.drawn_len());
    let (open, close) = tags(how);
    let mut inserts = Vec::new();
    for (a, b) in spans {
        let (a, b) = widen(&sealed, a, b);
        inserts.push((a, Put::Open, open.clone()));
        inserts.push((b, Put::Close, close.clone()));
    }
    splice(text, inserts)
}

/// `a..b` taken out to the ends of any code span (or autolink) it starts
/// or ends in: the tags around it go round the code, never inside it,
/// where they would be shown as written.
fn widen(sealed: &[(usize, usize, usize)], a: usize, b: usize) -> (usize, usize) {
    let a = sealed.iter().find(|s| s.0 < a && a < s.2).map_or(a, |s| s.0);
    let b = sealed.iter().find(|s| s.0 < b && b < s.2).map_or(b, |s| s.2);
    (a, b)
}

/// A place inside a code span (or an autolink) put outside it: before it
/// from the start of its words, else after it.
fn outside(sealed: &[(usize, usize, usize)], at: usize) -> usize {
    sealed.iter().find(|s| s.0 < at && at < s.2).map_or(at, |s| if at <= s.1 { s.0 } else { s.2 })
}

/// The tags a mark is written with: a wash, or red ink struck through.
/// No `==` or `**`-like marks, which could pair with one in the text.
fn tags(how: Mark) -> (String, String) {
    let tint = |ink: Option<u32>, wash: Option<u32>| inline::tint_tags(ink, wash).unwrap_or_default();
    match how {
        Mark::Added => tint(None, Some(ADDED)),
        Mark::Changed => tint(None, Some(CHANGED)),
        Mark::Removed => {
            let (o, c) = tint(Some(REMOVED), None);
            (format!("~~{o}"), format!("{c}~~"))
        }
    }
}

/// What goes in at one place, in the order it goes in: a closing tag
/// ends what came before, struck words stand between, an opening tag
/// starts what comes after.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Put {
    Close,
    Struck,
    Open,
}

/// `text` with `inserts` put in at their source bytes, in one pass: no
/// place is worked out from a text already changed, so nothing put in
/// can move another (docs/SYNC.md §9; the property tests hold it to
/// that, on any text).
fn splice(text: &str, mut inserts: Vec<(usize, Put, String)>) -> String {
    inserts.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.cmp(&y.1)));
    let mut out = String::with_capacity(text.len() + inserts.iter().map(|i| i.2.len()).sum::<usize>());
    let mut from = 0;
    for (at, _, s) in inserts {
        out.push_str(&text[from..at]);
        out.push_str(&s);
        from = at;
    }
    out.push_str(&text[from..]);
    out
}

/// `new` with the words it has that `old` did not washed, and the words
/// `old` had that it lost put back, struck, where they were.
fn words(old: &str, new: &str) -> String {
    let was = inline::plain(old);
    let vis = Visible::of(new);
    let sealed = inline::sealed(new);
    let now = vis.text(new);
    let (a, b) = (tokens(&was), tokens(&now));
    // Where each new token starts in the new words.
    let starts: Vec<usize> = b
        .iter()
        .scan(0, |at, t| {
            let s = *at;
            *at += t.len();
            Some(s)
        })
        .collect();
    let (wash_open, wash_close) = tags(Mark::Added);
    let (struck_open, struck_close) = tags(Mark::Removed);
    let mut inserts: Vec<(usize, Put, String)> = Vec::new();
    let steps = align(a.len(), b.len(), |i, j| a[i] == b[j], |_| Some(()), |_| Some(()));
    let mut k = 0;
    while k < steps.len() {
        // A run of differences: what went, then what came, at one place.
        let mut gone = String::new();
        let mut came: Option<(usize, usize)> = None;
        while k < steps.len() && !matches!(steps[k], Step::Same(..)) {
            match steps[k] {
                Step::Changed(i, j) => {
                    gone.push_str(a[i]);
                    came = Some(span(came, starts[j], starts[j] + b[j].len()));
                }
                Step::Removed(i) => gone.push_str(a[i]),
                Step::Added(j) => came = Some(span(came, starts[j], starts[j] + b[j].len())),
                Step::Same(..) => unreachable!(),
            }
            k += 1;
        }
        // The wash on the words alone, not the spaces at its ends.
        let came = came.map(|(s, e)| {
            let seg = &now[s..e];
            (s + seg.len() - seg.trim_start().len(), e - (seg.len() - seg.trim_end().len()))
        });
        let at = came.map_or_else(|| next_start(&steps, k, &starts, now.len()), |c| c.0);
        if let Some((s, e)) = came
            && s < e
        {
            for (sa, sb) in vis.spans(s, e) {
                let (sa, sb) = widen(&sealed, sa, sb);
                inserts.push((sa, Put::Open, wash_open.clone()));
                inserts.push((sb, Put::Close, wash_close.clone()));
            }
        }
        let gone = gone.trim();
        if !gone.is_empty() {
            // After the word before, outside the marks of the word after;
            // at the very start, before every mark.
            let src = if at == 0 { 0 } else { outside(&sealed, vis.at(at)) };
            let lead = if src > 0 && !new[..src].ends_with(char::is_whitespace) { " " } else { "" };
            let trail = if src < new.len() && !new[src..].starts_with(char::is_whitespace) { " " } else { "" };
            inserts.push((src, Put::Struck, format!("{lead}{struck_open}{gone}{struck_close}{trail}")));
        }
        k += 1;
    }
    splice(new, inserts)
}

fn span(sofar: Option<(usize, usize)>, s: usize, e: usize) -> (usize, usize) {
    sofar.map_or((s, e), |(a, _)| (a, e))
}

/// Where the next unchanged new token starts: the place words removed
/// with nothing in their stead are put back.
fn next_start(steps: &[Step], k: usize, starts: &[usize], end: usize) -> usize {
    match steps.get(k) {
        Some(Step::Same(_, j)) => starts[*j],
        _ => end,
    }
}

/// Words and the spaces between them, each a token: what the comparison
/// counts in.
fn tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (i, c) in s.char_indices() {
        let w = c.is_whitespace();
        if space.is_some_and(|was| was != w) {
            out.push(&s[start..i]);
            start = i;
        }
        space = Some(w);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}


// ------------------------------------------------------------ carrying a caret

/// A caret in `old` put where it stands in `new`: the same note, changed
/// elsewhere meanwhile (another device's edit taken in, a merge). Blocks
/// are matched as a comparison matches them, so the caret stays in its
/// block wherever that block now is; in a block whose words changed, it
/// keeps its place among the words around it ([`carry_byte`]). A block
/// gone: the caret goes to the one put in its place, else the start of
/// the next. Nothing here checks `new` itself: [`crate::page::fix`] after.
pub fn carry(old: &Doc, new: &Doc, c: crate::page::Caret) -> crate::page::Caret {
    let (ob, nb) = (old.blocks(), new.blocks());
    let eq = |i: usize, j: usize| same(&ob[i].kind, &nb[j].kind);
    let kind = |b: &Block| Some(std::mem::discriminant(&b.kind));
    let steps = align(ob.len(), nb.len(), eq, |i| kind(&ob[i]), |j| kind(&nb[j]));
    let pos = |p: crate::page::Pos| carry_pos(old, new, &steps, p);
    crate::page::Caret { head: pos(c.head), anchor: pos(c.anchor) }
}

fn carry_pos(old: &Doc, new: &Doc, steps: &[Step], p: crate::page::Pos) -> crate::page::Pos {
    use crate::page::{self, Pos, Slot};
    let Some(i) = old.index_of(p.slot.block) else { return p };
    let nb = new.blocks();
    let at = |j: usize, end: bool| {
        let slots = page::block_slots(&nb[j]);
        match (end, slots.first(), slots.last()) {
            (false, Some(&s), _) => page::start(s),
            (true, _, Some(&s)) => page::end(new, s),
            _ => page::first(new),
        }
    };
    let k = steps.iter().position(|s| matches!(*s, Step::Same(x, _) | Step::Changed(x, _) | Step::Removed(x) if x == i));
    let j = match k.map(|k| (k, &steps[k])) {
        Some((_, Step::Same(_, j) | Step::Changed(_, j))) => *j,
        // Gone: the block that came in its place, if one did (a line
        // turned into another kind), else the next one's start, else the
        // last one's end.
        Some((k, _)) => {
            // The run of blocks gone and come between two kept ones: the
            // n-th gone is taken to be replaced by the n-th come.
            let gap = |s: &Step| matches!(s, Step::Removed(_) | Step::Added(_));
            let from = steps[..k].iter().rposition(|s| !gap(s)).map_or(0, |x| x + 1);
            let to = steps[k..].iter().position(|s| !gap(s)).map_or(steps.len(), |x| k + x);
            let nth = steps[from..k].iter().filter(|s| matches!(s, Step::Removed(_))).count();
            let came: Vec<usize> = steps[from..to].iter().filter_map(|s| if let Step::Added(j) = s { Some(*j) } else { None }).collect();
            let rest = &steps[k + 1..];
            match came.get(nth).or(came.last()) {
                Some(&j) => j,
                None => {
                    if let Some(j) = rest.iter().find_map(|s| match s {
                        Step::Same(_, j) | Step::Changed(_, j) | Step::Added(j) => Some(*j),
                        Step::Removed(_) => None,
                    }) {
                        return at(j, false);
                    }
                    return match steps[..k].iter().rev().find_map(|s| match s {
                        Step::Same(_, j) | Step::Changed(_, j) | Step::Added(j) => Some(*j),
                        Step::Removed(_) => None,
                    }) {
                        Some(j) => at(j, true),
                        None => page::first(new),
                    };
                }
            }
        }
        None => return page::first(new),
    };
    let to = &nb[j];
    // The cell at the same row and column, in a table still.
    let slot = match (p.slot.cell, &old.blocks()[i].kind, &to.kind) {
        (Some((row, col)), Kind::Table(a), Kind::Table(b)) => {
            let (r, c) = (a.row_index(row).unwrap_or(0), a.col_index(col).unwrap_or(0));
            match (b.rows.get(r.min(b.rows.len().saturating_sub(1))), b.cols.get(c.min(b.cols.len().saturating_sub(1)))) {
                (Some(r), Some(&c)) => Slot { block: to.id, cell: Some((r.id, c)) },
                _ => return at(j, false),
            }
        }
        (None, _, Kind::Table(_)) | (Some(_), _, _) => return at(j, false),
        (None, _, _) => Slot::block(to.id),
    };
    if page::is_atomic(old, p.slot) || page::is_atomic(new, slot) {
        return Pos { slot, byte: p.byte.min(page::len(new, slot)) };
    }
    Pos { slot, byte: carry_byte(page::text(old, p.slot), page::text(new, slot), p.byte) }
}

/// A place in `old` put where it stands in `new`, the same text changed
/// in one run: before the change it stays, after it it moves with the
/// words after it, inside it it goes to the nearest edge of what replaced
/// it. Always on a character boundary of `new`.
pub fn carry_byte(old: &str, new: &str, b: usize) -> usize {
    let b = b.min(old.len());
    let mut pre = old.bytes().zip(new.bytes()).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(pre) || !new.is_char_boundary(pre) {
        pre -= 1;
    }
    let most = old.len().min(new.len()) - pre;
    let mut suf = old.bytes().rev().zip(new.bytes().rev()).take(most).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(old.len() - suf) || !new.is_char_boundary(new.len() - suf) {
        suf -= 1;
    }
    let mut out = if b <= pre {
        b
    } else if b >= old.len() - suf {
        new.len() - (old.len() - b)
    } else {
        b.clamp(pre, new.len() - suf)
    };
    while !new.is_char_boundary(out) {
        out -= 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Found by the property tests: words put back struck held stray
    /// marks that paired with others, and every later place moved, past a
    /// letter's middle. Nothing put in is read again now.
    #[test]
    fn stray_marks_and_wide_letters_do_not_break_a_comparison() {
        let old = "- 1. **- **|---\n1. **[`| a | b |\n|---|---|\n| 1 | 2 |\n ==- [x] - [ ] *Ünï-  *👩\u{200d}👩\u{200d}👧\n\n](";
        let new = "- 1. **- *| a | b |\n|---|---|\n| 1 | 2 |\n*|---\n1. **[`| a | b |\n|---|---|\n| 1 | 2\n |\n ==- [x] - [ ] *Ünï-  *👩\u{200d}👩\u{200d}👧(";
        let _ = marked(old, new);
        let _ = marked("x *a ~~b ==c", "x *a ~~d ==c e");
    }

    fn texts(md_old: &str, md_new: &str) -> Vec<String> {
        let (_, d) = marked(md_old, md_new);
        d.blocks()
            .iter()
            .map(|b| match &b.kind {
                Kind::Table(t) => t.rows.iter().map(|r| r.cells.join(" | ")).collect::<Vec<_>>().join(" / "),
                k => text_of(k).unwrap_or("").to_string(),
            })
            .collect()
    }

    const G: &str = "<mark style=\"background: #D8EAC0CC\">";
    /// Struck, in red.
    fn r(s: &str) -> String {
        format!("~~<span style=\"color: #C0553F\">{s}</span>~~")
    }

    #[test]
    fn words_added_and_removed_are_marked_where_they_are() {
        let t = texts("# N\n\nThe quick brown fox.\n", "# N\n\nThe slow brown fox jumps.\n");
        // Words are what spaces part: "fox." is not "fox".
        assert_eq!(t, [format!("The {} {G}slow</mark> brown {} {G}fox jumps.</mark>", r("quick"), r("fox."))]);
    }

    #[test]
    fn unchanged_blocks_are_left_and_whole_blocks_marked() {
        let t = texts(
            "# N\n\nKept.\n\nGone away.\n\n- [ ] milk\n",
            "# N\n\nKept.\n\n- [x] milk\n\nNew here.\n",
        );
        assert_eq!(t[0], "Kept.");
        // A paragraph and an item, paired by place: not one family.
        assert_eq!(t[1], r("Gone away."), "put back, struck, where it was");
        assert!(t.iter().any(|s| s == "<mark style=\"background: #F8EBA6CC\">milk</mark>"), "the tick: same words, marked changed: {t:?}");
        assert!(t.iter().any(|s| s == &format!("{G}New here.</mark>")), "{t:?}");
    }

    #[test]
    fn styled_words_keep_their_marks_and_tables_compare_by_cell() {
        let t = texts("# N\n\nA **bold** word.\n", "# N\n\nA **bold** new word.\n");
        assert_eq!(t, [format!("A **bold** {G}new</mark> word.")]);
        let t = texts(
            "# N\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            "# N\n\n| a | b |\n|---|---|\n| 1 | 3 |\n| 4 | 5 |\n",
        );
        assert_eq!(t[0], format!("a | b / 1 | {} {G}3</mark> / {G}4</mark> | {G}5</mark>", r("2")));
    }

    #[test]
    fn the_same_text_is_the_same_doc_and_a_big_note_is_quick() {
        let big: String = (0..2000).map(|i| format!("- [ ] item {i}\n")).collect();
        let md = format!("# Big\n\n{big}");
        let edited = md.replace("item 1000\n", "item 1000 changed\n");
        let t0 = std::time::Instant::now();
        let (_, d) = marked(&md, &edited);
        assert!(t0.elapsed().as_millis() < 500, "{:?}", t0.elapsed());
        let marked_blocks = d.blocks().iter().filter(|b| text_of(&b.kind).is_some_and(|t| t.contains("<mark"))).count();
        assert_eq!(marked_blocks, 1);
        let (_, same) = marked(&md, &md);
        assert!(same.blocks().iter().all(|b| !text_of(&b.kind).is_some_and(|t| t.contains('<') || t.contains("=="))));
    }

    /// The finding: a coloured word gone from beside inline code came
    /// back inside the code, `` `code ~~<span …>and</span>~~` ``, shown as
    /// written. Marks go round code, never in it: a change in or next to
    /// it reads as words.
    #[test]
    fn a_change_by_or_in_code_is_marked_outside_it() {
        let t = texts("# N\n\nRun `code` <span style=\"color: #C0553F\">and</span>\n", "# N\n\nRun `code`\n");
        assert_eq!(t, [format!("Run `code` {}", r("and"))]);
        let t = texts("# N\n\nRun `code` <span style=\"color: #C0553F\">and</span> more.\n", "# N\n\nRun `code` more.\n");
        assert_eq!(t, [format!("Run `code` {} more.", r("and"))]);
        let t = texts("# N\n\nRun `ab` now.\n", "# N\n\nRun `abc` now.\n");
        assert_eq!(t, [format!("Run {} {G}`abc`</mark> now.", r("ab"))]);
        let t = texts("# N\n\nOld.\n", "# N\n\nOld.\n\nNew `x` and <https://a.com>\n");
        assert_eq!(t[1], format!("{G}New </mark>{G}`x`</mark>{G} and </mark>{G}<https://a.com></mark>"));
        for line in &t {
            let (_, d) = doc::from_markdown(line);
            for b in d.blocks() {
                let text = b.text_ref().unwrap_or("");
                let shown = inline::Visible::of(text).text(text);
                assert!(!shown.contains('<') && !shown.contains('~'), "{shown}");
            }
        }
    }
}

#[cfg(test)]
mod carry_tests {
    use super::{carry, carry_byte};
    use crate::doc::{self, Doc, Kind};
    use crate::page::{self, Caret, Pos, Slot};

    fn parsed(md: &str) -> Doc {
        doc::from_markdown(md).1
    }

    /// The caret at `byte` of block `i`.
    fn at(d: &Doc, i: usize, byte: usize) -> Caret {
        Caret::at(Pos { slot: Slot::block(d.blocks()[i].id), byte })
    }

    /// Where a caret is, as (block index, byte).
    fn place(d: &Doc, c: Caret) -> (usize, usize) {
        (d.index_of(c.head.slot.block).unwrap(), c.head.byte)
    }

    #[test]
    fn a_byte_keeps_its_place_among_the_words() {
        assert_eq!(carry_byte("bread", "bread WEB", 5), 5, "an insertion at the place: before it");
        assert_eq!(carry_byte("milk bread", "fresh milk bread", 10), 16, "after an insertion: moved by it");
        assert_eq!(carry_byte("milk bread", "fresh milk bread", 0), 0);
        assert_eq!(carry_byte("milk and bread", "milk bread", 12), 8, "after a deletion");
        assert_eq!(carry_byte("milk and bread", "milk bread", 6), 5, "inside a deletion: to its edge");
        assert_eq!(carry_byte("café crème", "café, crème", 10), 11, "on a character boundary");
        assert_eq!(carry_byte("", "new words", 0), 0);
    }

    /// The finding: the caret at the end of the second item, " WEB" put at
    /// the end of the first on another device: the caret stays where it
    /// was, not on the first item.
    #[test]
    fn an_edit_in_another_block_leaves_the_caret_where_it_was() {
        let old = parsed("- milk1\n- bread1\n");
        let new = parsed("- milk1 WEB\n- bread1\n");
        let c = carry(&old, &new, at(&old, 1, 6));
        assert_eq!(place(&new, c), (1, 6));
        assert!(c.is_empty());
    }

    #[test]
    fn an_edit_before_the_caret_moves_it_with_its_words() {
        // A block put in before it.
        let old = parsed("First\n\nSecond line\n");
        let new = parsed("New one\n\nFirst\n\nSecond line\n");
        assert_eq!(place(&new, carry(&old, &new, at(&old, 1, 6))), (2, 6));
        // Words put in before it, in its own block.
        let new = parsed("First\n\nThe Second line\n");
        assert_eq!(place(&new, carry(&old, &new, at(&old, 1, 6))), (1, 10));
        // Words put in after it, in its own block: it stays.
        let new = parsed("First\n\nSecond line too\n");
        assert_eq!(place(&new, carry(&old, &new, at(&old, 1, 6))), (1, 6));
        // A block before it taken away.
        let new = parsed("Second line\n");
        assert_eq!(place(&new, carry(&old, &new, at(&old, 1, 6))), (0, 6));
    }

    #[test]
    fn a_selection_is_carried_end_by_end() {
        let old = parsed("One\n\nTwo words here\n");
        let new = parsed("Zero\n\nOne\n\nSo two words here\n");
        let c = Caret { anchor: Pos { slot: Slot::block(old.blocks()[1].id), byte: 4 }, head: Pos { slot: Slot::block(old.blocks()[1].id), byte: 9 } };
        let c = carry(&old, &new, c);
        let slot = Slot::block(new.blocks()[2].id);
        assert_eq!((c.anchor, c.head), (Pos { slot, byte: 7 }, Pos { slot, byte: 12 }));
        assert_eq!(&page::text(&new, slot)[7..12], "words");
    }

    #[test]
    fn a_block_gone_or_turned_takes_the_caret_to_its_place() {
        // Turned into a heading: the caret in the heading, by its words.
        let old = parsed("Intro\n\nPlans\n\nEnd\n");
        let new = parsed("Intro\n\n## Plans\n\nEnd\n");
        assert_eq!(place(&new, carry(&old, &new, at(&old, 1, 3))), (1, 3));
        assert!(matches!(new.blocks()[1].kind, Kind::Heading { .. }));
        // Taken away: the next one's start.
        let new = parsed("Intro\n\nEnd\n");
        assert_eq!(place(&new, carry(&old, &new, at(&old, 1, 3))), (1, 0));
    }

    #[test]
    fn a_cell_keeps_its_row_and_column() {
        let old = parsed("| A | B |\n| --- | --- |\n| one | two |\n");
        let new = parsed("Above\n\n| A | B |\n| --- | --- |\n| one | the two |\n");
        let Kind::Table(t) = &old.blocks()[0].kind else { panic!() };
        let c = Caret::at(Pos { slot: Slot { block: old.blocks()[0].id, cell: Some((t.rows[1].id, t.cols[1])) }, byte: 3 });
        let c = carry(&old, &new, c);
        let Kind::Table(t) = &new.blocks()[1].kind else { panic!() };
        assert_eq!(c.head, Pos { slot: Slot { block: new.blocks()[1].id, cell: Some((t.rows[1].id, t.cols[1])) }, byte: 7 });
    }

    #[test]
    fn a_note_replaced_whole_is_carried_through_quickly() {
        // Every block another kind: each new one finds none of its kind
        // among the old ones.
        let n = 100_000;
        let old = parsed(&(0..n).map(|k| format!("Line {k}\n\n")).collect::<String>());
        let new = parsed(&(0..n).map(|k| format!("## Head {k}\n\n")).collect::<String>());
        let t0 = std::time::Instant::now();
        let c = carry(&old, &new, at(&old, n / 2, 2));
        let took = t0.elapsed();
        assert!(took.as_millis() < 3000, "{took:?}");
        assert_eq!(place(&new, c), (n / 2, 2), "turned into a heading: the heading, by its words");
    }
}
