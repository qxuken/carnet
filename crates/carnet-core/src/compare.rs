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
    let pair = |i: usize, j: usize| family(&before[i].kind).is_some_and(|f| Some(f) == family(&after[j].kind));
    for step in align(before.len(), after.len(), eq, pair) {
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
/// one `pair` allows, the old ones it passes removed, and a new one with
/// none added. Common ends are trimmed first, so a note of 2,000 blocks
/// with one edited costs one comparison per block.
fn align(n: usize, m: usize, eq: impl Fn(usize, usize) -> bool, pair: impl Fn(usize, usize) -> bool) -> Vec<Step> {
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
        // The gap before this match.
        let mut oi = i;
        for nj in j..mj {
            match (oi..mi).find(|&p| pair(p, nj)) {
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
    let spans = vis.spans(0, vis.drawn_len());
    let (open, close) = tags(how);
    let mut inserts = Vec::new();
    for (a, b) in spans {
        inserts.push((a, Put::Open, open.clone()));
        inserts.push((b, Put::Close, close.clone()));
    }
    splice(text, inserts)
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
    let steps = align(a.len(), b.len(), |i, j| a[i] == b[j], |_, _| true);
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
                inserts.push((sa, Put::Open, wash_open.clone()));
                inserts.push((sb, Put::Close, wash_close.clone()));
            }
        }
        let gone = gone.trim();
        if !gone.is_empty() {
            // After the word before, outside the marks of the word after;
            // at the very start, before every mark.
            let src = if at == 0 { 0 } else { vis.at(at) };
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
}
