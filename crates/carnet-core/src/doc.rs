//! The document model — a flat list of blocks — and its Markdown form.
//!
//! The vocabulary is deliberately tiny: text, heading, list item (bullet,
//! numbered, checklist), table, divider, and `Raw` — code fences and front
//! matter, kept byte for byte. Every block maps to one GFM construct and
//! back, so a note on disk is a file any other tool reads.
//!
//! The rule the writer and the reader keep together: a backslash this file
//! adds to stop text reading as structure is the only one it takes away.
//! A line from anywhere else — `\frac`, `\(x\)` — is never touched.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::inline;

/// Ids are process-unique and never reused: an editor's buffer is keyed by
/// the block's id, so a fresh id is a fresh editor.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn new_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListStyle {
    Bullet,
    Number,
    Todo,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: u64,
    pub cells: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// One id per column, so a column keeps its cells' editors when one
    /// before it is inserted or removed.
    pub cols: Vec<u64>,
    /// Each column's separator cell as written (`---`, `:--:`, `--:`), so
    /// alignment survives a save.
    pub aligns: Vec<String>,
    /// Row 0 is the header, if it has one.
    pub rows: Vec<Row>,
    /// Whether row 0 is a header. Without one, every row is a plain one,
    /// and the table is written under an empty header row (`| | |`), as
    /// Markdown has no table without; an empty header row reads back as
    /// none.
    pub header: bool,
}

const PLAIN_ALIGN: &str = "---";

impl Table {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        Self {
            cols: (0..cols).map(|_| new_id()).collect(),
            aligns: vec![PLAIN_ALIGN.to_string(); cols],
            rows: (0..rows.max(1))
                .map(|_| Row {
                    id: new_id(),
                    cells: vec![String::new(); cols],
                })
                .collect(),
            header: true,
        }
    }

    pub fn from_cells(mut cells: Vec<Vec<String>>) -> Self {
        let width = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for row in &mut cells {
            row.resize(width, String::new());
        }
        if cells.is_empty() {
            cells.push(vec![String::new(); width]);
        }
        Self {
            cols: (0..width).map(|_| new_id()).collect(),
            aligns: vec![PLAIN_ALIGN.to_string(); width],
            rows: cells
                .into_iter()
                .map(|cells| Row { id: new_id(), cells })
                .collect(),
            header: true,
        }
    }

    /// The first row that isn't the header.
    pub fn body(&self) -> usize {
        usize::from(self.header)
    }

    /// Whether row `r` is the header.
    pub fn is_header(&self, r: usize) -> bool {
        self.header && r == 0
    }

    /// The header taken away or put back, no words lost either way: off,
    /// the header row is the first plain row, under the empty header line
    /// the file has then; on, the first plain row is the header again.
    /// True when it changed.
    pub fn set_header(&mut self, on: bool) -> bool {
        let changed = self.header != on;
        self.header = on;
        changed
    }

    pub fn width(&self) -> usize {
        self.cols.len()
    }

    pub fn row_index(&self, id: u64) -> Option<usize> {
        self.rows.iter().position(|r| r.id == id)
    }

    pub fn col_index(&self, id: u64) -> Option<usize> {
        self.cols.iter().position(|c| *c == id)
    }

    pub fn insert_row(&mut self, at: usize) -> u64 {
        let id = new_id();
        let at = at.min(self.rows.len());
        self.rows.insert(
            at,
            Row {
                id,
                cells: vec![String::new(); self.width()],
            },
        );
        id
    }

    pub fn insert_col(&mut self, at: usize) -> u64 {
        let id = new_id();
        let at = at.min(self.width());
        self.cols.insert(at, id);
        self.aligns.insert(at, PLAIN_ALIGN.to_string());
        for row in &mut self.rows {
            row.cells.insert(at, String::new());
        }
        id
    }

    /// Never removes the last row or column: a table always has a cell.
    /// The header removed, the table has none.
    pub fn remove_row(&mut self, at: usize) -> bool {
        if self.rows.len() <= 1 || at >= self.rows.len() {
            return false;
        }
        self.rows.remove(at);
        if at == 0 {
            self.header = false;
        }
        true
    }

    pub fn remove_col(&mut self, at: usize) -> bool {
        if self.width() <= 1 || at >= self.width() {
            return false;
        }
        self.cols.remove(at);
        self.aligns.remove(at);
        for row in &mut self.rows {
            row.cells.remove(at);
        }
        true
    }

    /// Sorts the body rows (never the header) by column `col`, numbers
    /// numerically, empty cells last. Returns whether anything moved.
    pub fn sort_by(&mut self, col: usize, descending: bool) -> bool {
        let from = self.body();
        if self.rows.len() < from + 2 || col >= self.width() {
            return false;
        }
        let before: Vec<u64> = self.rows.iter().map(|r| r.id).collect();
        let body = &mut self.rows[from..];
        body.sort_by(|a, b| {
            let (x, y) = (a.cells[col].trim(), b.cells[col].trim());
            match (x.is_empty(), y.is_empty()) {
                (true, true) => return std::cmp::Ordering::Equal,
                (true, false) => return std::cmp::Ordering::Greater,
                (false, true) => return std::cmp::Ordering::Less,
                _ => {}
            }
            let ord = match (parse_num(x), parse_num(y)) {
                (Some(p), Some(q)) => p.partial_cmp(&q).unwrap_or(std::cmp::Ordering::Equal),
                _ => x.to_lowercase().cmp(&y.to_lowercase()),
            };
            if descending { ord.reverse() } else { ord }
        });
        self.rows.iter().map(|r| r.id).ne(before)
    }

    fn is_blank_row(&self, r: usize) -> bool {
        self.rows[r].cells.iter().all(|c| c.trim().is_empty())
    }

    /// Whether row `r` is the last one and holds nothing: what Enter in it
    /// steps out of the table over, as an empty list item steps out of a
    /// list.
    pub fn is_empty_last_row(&self, r: usize) -> bool {
        r > 0 && r + 1 == self.rows.len() && self.is_blank_row(r)
    }
}

fn parse_num(s: &str) -> Option<f64> {
    let s: String = s
        .chars()
        .filter(|c| !matches!(c, ',' | '$' | '€' | '£' | '%' | ' '))
        .collect();
    s.parse().ok()
}

/// Where a picture sits: on a line of its own, or at the left or right
/// edge with the blocks after it running beside it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Float {
    #[default]
    None,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Text(String),
    /// Levels 1–6, as written; the editor draws 3–6 alike.
    Heading { level: u8, text: String },
    Item {
        style: ListStyle,
        indent: u8,
        done: bool,
        /// The number a numbered item was written with (0 for one made
        /// here): the first of a list keeps its start, `3.` stays `3.`.
        num: u32,
        text: String,
    },
    Table(Table),
    Divider,
    /// A picture on a line of its own: `![alt](src)`, `src` as written —
    /// relative to the note's folder, or absolute, or a URL. A width the
    /// reader gave it is written as Obsidian does, `![alt|400](src)`; a
    /// picture the words run beside, `![alt|right|400](src)`. With `wiki`,
    /// it was written as Obsidian embeds one, `![[src|alt|400]]`, and is
    /// written back so: `src` is then a path from the notes folder or a
    /// name alone, looked for as Obsidian looks ([`find_picture`]).
    Image { src: String, alt: String, width: Option<u32>, float: Float, wiki: bool },
    /// A quotation: `> ` before each of its lines.
    Quote(String),
    /// Written back exactly as read: a fenced code block, or front matter.
    Raw(String),
}

impl Kind {
    /// A block with no text to put a caret in: a divider, an image. The
    /// editor gives it one slot of length 1 — it is selected, moved and
    /// removed whole.
    pub fn is_atomic(&self) -> bool {
        matches!(self, Kind::Divider | Kind::Image { .. })
    }

    pub fn item(style: ListStyle, indent: u8, text: impl Into<String>) -> Self {
        Kind::Item {
            style,
            indent,
            done: false,
            num: 0,
            text: text.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub id: u64,
    pub kind: Kind,
    /// A paragraph whose first letter drops, a few lines tall. Kept in
    /// the file as a tag around the letter (`<span class="initial">T</span>`,
    /// see [`inline::put_initial`]), out of the words while they are
    /// edited. Only a paragraph's (`Kind::Text`) is drawn and written.
    pub initial: bool,
}

impl Block {
    pub fn new(kind: Kind) -> Self {
        Self { id: new_id(), kind, initial: false }
    }

    pub fn text(s: impl Into<String>) -> Self {
        Self::new(Kind::Text(s.into()))
    }

    /// The editable text of a one-editor block.
    pub fn text_mut(&mut self) -> Option<&mut String> {
        match &mut self.kind {
            Kind::Text(t)
            | Kind::Raw(t)
            | Kind::Quote(t)
            | Kind::Heading { text: t, .. }
            | Kind::Item { text: t, .. } => Some(t),
            _ => None,
        }
    }

    pub fn text_ref(&self) -> Option<&str> {
        match &self.kind {
            Kind::Text(t)
            | Kind::Raw(t)
            | Kind::Quote(t)
            | Kind::Heading { text: t, .. }
            | Kind::Item { text: t, .. } => Some(t),
            _ => None,
        }
    }
}

pub const MAX_INDENT: u8 = 6;

/// The blocks are shared: a [`Doc::snapshot`] is a vector of pointers, and
/// an edit copies only the block it writes (`Rc::make_mut`). That is the
/// undo history's whole cost model — the lightweight cousin of a persistent
/// piece table, at block granularity: every state shares every block it
/// did not change with the states around it.
#[derive(Clone, Debug, Default)]
pub struct Doc {
    blocks: Vec<Rc<Block>>,
    /// Where each block is, by id: built on the first lookup after the
    /// list changed shape, so the page's per-slot lookups are O(1) and not
    /// a scan each. A block's id never changes while it is in a doc, so
    /// writing a block's contents leaves it standing.
    index: std::cell::OnceCell<std::collections::HashMap<u64, usize>>,
}

/// Two docs are equal by their blocks; the index is only a cache.
impl PartialEq for Doc {
    fn eq(&self, other: &Self) -> bool {
        self.blocks == other.blocks
    }
}

impl Doc {
    pub fn new(blocks: Vec<Block>) -> Self {
        Self {
            blocks: blocks.into_iter().map(Rc::new).collect(),
            index: Default::default(),
        }
    }

    /// The list changed shape: the index is rebuilt when next asked.
    fn reshaped(&mut self) {
        self.index = Default::default();
    }

    fn index(&self) -> &std::collections::HashMap<u64, usize> {
        self.index
            .get_or_init(|| self.blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect())
    }

    pub fn blocks(&self) -> &[Rc<Block>] {
        &self.blocks
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Block `i`, to write: copied first if a snapshot still shares it.
    pub fn block_mut(&mut self, i: usize) -> &mut Block {
        Rc::make_mut(&mut self.blocks[i])
    }

    pub fn insert(&mut self, i: usize, block: Block) {
        self.reshaped();
        self.blocks.insert(i, Rc::new(block));
    }

    pub fn push(&mut self, block: Block) {
        self.reshaped();
        self.blocks.push(Rc::new(block));
    }

    pub fn remove(&mut self, i: usize) -> Block {
        self.reshaped();
        Rc::unwrap_or_clone(self.blocks.remove(i))
    }

    pub fn set(&mut self, i: usize, block: Block) {
        self.reshaped();
        self.blocks[i] = Rc::new(block);
    }


    /// Replaces `range` with `with`; the blocks taken out come back.
    pub fn splice(
        &mut self,
        range: std::ops::Range<usize>,
        with: impl IntoIterator<Item = Block>,
    ) -> Vec<Block> {
        self.reshaped();
        self.blocks
            .splice(range, with.into_iter().map(Rc::new))
            .map(Rc::unwrap_or_clone)
            .collect()
    }

    /// Every block as it is now, shared: O(blocks) pointers, no text.
    pub fn snapshot(&self) -> Vec<Rc<Block>> {
        self.blocks.clone()
    }

    pub fn restore(&mut self, blocks: Vec<Rc<Block>>) {
        self.reshaped();
        self.blocks = blocks;
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.index().get(&id).copied()
    }

    pub fn get(&self, id: u64) -> Option<&Block> {
        self.index_of(id).map(|i| &*self.blocks[i])
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Block> {
        let i = self.index_of(id)?;
        Some(self.block_mut(i))
    }

    /// The invariant the editor relies on: there is always a text block at
    /// the end, so there is a place to type below a list or a table.
    /// Returns whether it had to add one.
    pub fn ensure_tail(&mut self) -> bool {
        if matches!(self.blocks.last().map(|b| &b.kind), Some(Kind::Text(_))) {
            return false;
        }
        self.push(Block::text(""));
        true
    }


    pub fn word_count(&self) -> usize {
        let words = |s: &str| s.split_whitespace().count();
        self.blocks
            .iter()
            .map(|b| match &b.kind {
                Kind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).map(|c| words(c)).sum(),
                _ => b.text_ref().map_or(0, words),
            })
            .sum()
    }

    /// Checklist progress: (done, all), `None` without a checklist.
    pub fn todos(&self) -> Option<(usize, usize)> {
        let (mut done, mut all) = (0, 0);
        for b in &self.blocks {
            if let Kind::Item {
                style: ListStyle::Todo,
                done: d,
                ..
            } = b.kind
            {
                all += 1;
                done += d as usize;
            }
        }
        (all > 0).then_some((done, all))
    }

    /// Whether `needle` (already lowercased) appears anywhere in the body.
    pub fn contains(&self, needle: &str) -> bool {
        self.blocks.iter().any(|b| match &b.kind {
            Kind::Table(t) => t
                .rows
                .iter()
                .flat_map(|r| &r.cells)
                .any(|c| c.to_lowercase().contains(needle)),
            _ => b.text_ref().is_some_and(|t| t.to_lowercase().contains(needle)),
        })
    }

    /// The first line of real text, for the library's preview line, as
    /// pieces: one for a line of text, a table's filled cells for a table
    /// (the library draws its own dots between them).
    pub fn snippet_parts(&self) -> Vec<String> {
        let clip = |s: &str| crate::inline::plain(s.trim()).chars().take(120).collect::<String>();
        for b in &self.blocks {
            match &b.kind {
                Kind::Table(t) => {
                    let cells: Vec<String> = t
                        .rows
                        .iter()
                        .map(|r| r.cells.iter().map(|c| without_pictures(c)).filter(|c| !c.trim().is_empty()).map(|c| clip(&c)).collect::<Vec<_>>())
                        .find(|cells| !cells.is_empty())
                        .unwrap_or_default();
                    if !cells.is_empty() {
                        return cells;
                    }
                }
                Kind::Raw(_) => {}
                _ => {
                    let s = b.text_ref().unwrap_or("");
                    if let Some(line) = s.lines().map(str::trim).find(|l| !l.is_empty()) {
                        return vec![clip(line)];
                    }
                }
            }
        }
        Vec::new()
    }

    /// The preview as one line, for a screen reader.
    pub fn snippet(&self) -> String {
        self.snippet_parts().join(", ")
    }
}

/// A cell's text with its pictures left out: its words, for a preview.
fn without_pictures(cell: &str) -> String {
    let mut out = cell.to_string();
    for p in inline::cell_pictures(cell).iter().rev() {
        out.replace_range(p.at.0..p.at.1, "");
    }
    out
}

// ------------------------------------------------------------ shortcuts

/// What a Markdown prefix typed at the start of a line turns it into: the
/// new kind, with the prefix taken off. `None` leaves it as text.
pub fn shortcut(text: &str) -> Option<Kind> {
    if text.contains('\n') {
        return None;
    }
    let item = |style, done, rest: &str| Kind::Item {
        style,
        indent: 0,
        done,
        num: 0,
        text: rest.to_string(),
    };
    for (prefix, done) in [("- [ ] ", false), ("- [x] ", true), ("[] ", false), ("[ ] ", false), ("[x] ", true)] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return Some(item(ListStyle::Todo, done, rest));
        }
    }
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return Some(item(ListStyle::Bullet, false, rest));
        }
    }
    if let Some((_, rest)) = numbered_prefix(text) {
        return Some(item(ListStyle::Number, false, rest));
    }
    if let Some(rest) = text.strip_prefix("> ") {
        return Some(Kind::Quote(rest.to_string()));
    }
    for level in 1..=3u8 {
        let prefix = format!("{} ", "#".repeat(level as usize));
        if let Some(rest) = text.strip_prefix(&prefix) {
            return Some(Kind::Heading {
                level,
                text: rest.to_string(),
            });
        }
    }
    match text {
        "---" | "***" | "___" => Some(Kind::Divider),
        _ => None,
    }
}

/// What a line opens when Enter is pressed at its end: a code block for
/// `` ``` `` (a language after it kept on the fence, `` ```rust ``) or
/// `/code`, and a table for `/table`. Not as it is typed, as other
/// shortcuts are: the language comes after the marks, and the Enter that
/// follows them would be an empty first line in the code.
pub fn fence(line: &str) -> Option<Kind> {
    // `/table` the same way: as it's typed, a slash menu (the desktop's)
    // is still choosing, and Enter is its choice.
    if line.trim_end() == "/table" {
        return Some(Kind::Table(Table::new(3, 3)));
    }
    let lang = match line.trim_end() {
        "/code" => "",
        l => l.strip_prefix("```")?.trim(),
    };
    // Backticks in a fence's language would end it: no fence.
    if lang.contains('`') {
        return None;
    }
    Some(Kind::Raw(format!("```{lang}\n\n```")))
}

/// `"12. rest"` → `(12, "rest")`.
fn numbered_prefix(text: &str) -> Option<(u32, &str)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let n = text[..digits].parse().ok()?;
    let rest = &text[digits..];
    let rest = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))?;
    Some((n, rest))
}

// ------------------------------------------------------------ markdown out

/// The note as Markdown. `title` is written as the first `# ` heading when
/// given — `None` for a file that never had one, which keeps not having
/// one. Front matter stays on top, above the title.
pub fn to_markdown(title: Option<&str>, doc: &Doc) -> String {
    let mut out = String::new();
    // Trailing empty text blocks are the editor's, not the note's.
    let end = doc
        .blocks
        .iter()
        .rposition(|b| !matches!(&b.kind, Kind::Text(t) if t.trim().is_empty()))
        .map_or(0, |i| i + 1);
    let blocks: Vec<&Block> = doc.blocks[..end].iter().map(|b| &**b).collect();
    let front = blocks
        .first()
        .filter(|b| matches!(&b.kind, Kind::Raw(t) if is_front_matter(t)))
        .map_or(0, |_| 1);
    if front == 1
        && let Kind::Raw(t) = &blocks[0].kind
    {
        out.push_str(t);
        out.push_str("\n\n");
    }
    if let Some(title) = title {
        let title = title.trim();
        if !title.is_empty() {
            out.push_str("# ");
            out.push_str(&title.replace('\n', " "));
            out.push_str("\n\n");
        } else if matches!(blocks.get(front).map(|b| &b.kind), Some(Kind::Heading { level: 1, .. })) {
            // No title, and the body opens with a level-one heading that
            // would read back as one: an empty title holds its place.
            out.push_str("#\n\n");
        }
    }
    let mut list = ListWriter::default();
    for (i, b) in blocks.iter().enumerate().skip(front) {
        let next_is_item = matches!(blocks.get(i + 1).map(|b| &b.kind), Some(Kind::Item { .. }));
        if !matches!(b.kind, Kind::Item { .. }) {
            list = ListWriter::default();
        }
        match &b.kind {
            Kind::Text(t) => {
                if t.trim().is_empty() {
                    // An empty paragraph between blocks is kept as a gap.
                    out.push('\n');
                    continue;
                }
                // A dropped first letter goes on before the lines are
                // escaped: a line opening with its tag reads as no list or
                // heading, and needs no backslash.
                let t = if b.initial { inline::put_initial(t) } else { t.clone() };
                for line in t.split('\n') {
                    out.push_str(&escape_line(line));
                    out.push('\n');
                }
                out.push('\n');
            }
            Kind::Heading { level, text } => {
                out.push_str(&"#".repeat((*level).clamp(1, 6) as usize));
                out.push(' ');
                out.push_str(&text.replace('\n', " "));
                out.push_str("\n\n");
            }
            Kind::Item {
                style,
                indent,
                done,
                num,
                text,
            } => {
                out.push_str(&list.line(*style, *indent as usize, *done, *num, text));
                out.push('\n');
                if !next_is_item {
                    out.push('\n');
                }
            }
            Kind::Table(t) => {
                let separator = |out: &mut String| {
                    out.push('|');
                    for c in 0..t.width() {
                        let a = t.aligns.get(c).map_or(PLAIN_ALIGN, String::as_str);
                        out.push(' ');
                        out.push_str(a);
                        out.push_str(" |");
                    }
                    out.push('\n');
                };
                // No header: an empty one, which reads back as none.
                if !t.header {
                    out.push('|');
                    out.push_str(&" |".repeat(t.width()));
                    out.push('\n');
                    separator(&mut out);
                }
                for (r, row) in t.rows.iter().enumerate() {
                    out.push('|');
                    for cell in &row.cells {
                        out.push(' ');
                        out.push_str(&escape_cell(cell));
                        out.push_str(" |");
                    }
                    out.push('\n');
                    if t.is_header(r) {
                        separator(&mut out);
                    }
                }
                out.push('\n');
            }
            // A note that opens with `---` and has another later opens with
            // front matter; `----` is a divider all the same.
            Kind::Divider if out.is_empty() => out.push_str("----\n\n"),
            Kind::Divider => out.push_str("---\n\n"),
            Kind::Image { src, alt, width, float, wiki } => {
                let line = match wiki {
                    true => image_wiki(alt, src, *width, *float),
                    false => None,
                };
                out.push_str(&line.unwrap_or_else(|| image_markdown(alt, src, *width, *float)));
                out.push_str("\n\n");
            }
            Kind::Quote(t) => {
                for line in t.split('\n') {
                    out.push_str(if line.is_empty() { ">" } else { "> " });
                    out.push_str(line);
                    out.push('\n');
                }
                out.push('\n');
            }
            Kind::Raw(t) => {
                out.push_str(t);
                out.push_str("\n\n");
            }
        }
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Writes one list's items: the numbers a numbered run counts with, and
/// each level's indent — the width of its parent's marker, which is what
/// CommonMark nests by (three spaces under `1. `, two under `- `).
#[derive(Default)]
struct ListWriter {
    /// Per depth: where that level's text starts, and the last number.
    levels: Vec<(usize, u32)>,
}

impl ListWriter {
    fn line(&mut self, style: ListStyle, depth: usize, done: bool, num: u32, text: &str) -> String {
        let depth = depth.min(self.levels.len());
        self.levels.truncate(depth + 1);
        let lead = if depth == 0 { 0 } else { self.levels[depth - 1].0 };
        let prev = self.levels.get(depth).map_or(0, |l| l.1);
        let (marker, n) = match style {
            ListStyle::Bullet => ("- ".to_string(), 0),
            ListStyle::Todo => ((if done { "- [x] " } else { "- [ ] " }).to_string(), 0),
            ListStyle::Number => {
                let n = if prev > 0 {
                    prev + 1
                } else if num > 0 {
                    num
                } else {
                    1
                };
                (format!("{n}. "), n)
            }
        };
        // The text column under a checklist box is the bullet's: the box is
        // the text's, as far as nesting goes.
        let width = if style == ListStyle::Todo { 2 } else { marker.len() };
        let level = (lead + width, n);
        if self.levels.len() > depth {
            self.levels[depth] = level;
        } else {
            self.levels.push(level);
        }
        let mut text = text.replace('\n', " ");
        // A bullet whose words start like a checkbox would read back as
        // one; one whose words are dashes (`- ---`), as a divider.
        if style == ListStyle::Bullet
            && (checkbox_prefix(&text).is_some() || is_rule(&format!("{marker}{text}")))
        {
            text.insert(0, '\\');
        }
        format!("{}{marker}{text}", " ".repeat(lead))
    }
}

fn checkbox_prefix(s: &str) -> Option<(bool, &str)> {
    if let Some(t) = s.strip_prefix("[ ] ") {
        return Some((false, t));
    }
    if let Some(t) = s.strip_prefix("[x] ").or_else(|| s.strip_prefix("[X] ")) {
        return Some((true, t));
    }
    match s {
        "[ ]" => Some((false, "")),
        "[x]" | "[X]" => Some((true, "")),
        _ => None,
    }
}

/// A line of a paragraph that would read back as something else gets a
/// backslash where CommonMark takes one: before the marker (`\- `, `\# `),
/// or before a number's dot (`1\. `). A blank line inside a paragraph is a
/// lone backslash, so the paragraph is not split in two. A line's own
/// backslash before a mark is written as it is: read back, `\# x` loses
/// it and `\*x` keeps it, and the words are the same either way, a
/// backslash before a mark being one (see `inline`).
fn escape_line(line: &str) -> String {
    if line.trim().is_empty() {
        return "\\".to_string();
    }
    let ws = line.len() - line.trim_start().len();
    let (lead, body) = line.split_at(ws);
    let special = heading(body).is_some()
        || body.starts_with('|')
        || list_marker(body).is_some()
        || is_rule(body)
        || is_setext_eq(body)
        || fence_of(body).is_some()
        || image_line(body).is_some()
        || image_row(body).is_some()
        || body.starts_with('>')
        || body == "\\";
    if !special {
        return line.to_string();
    }
    if let Some((_, _)) = numbered_prefix(body) {
        let digits = body.bytes().take_while(u8::is_ascii_digit).count();
        return format!("{lead}{}\\{}", &body[..digits], &body[digits..]);
    }
    format!("{lead}\\{body}")
}

/// The inverse of `escape_line`, and only of it: the line without the
/// backslash this file would have added, when adding it back gives the
/// line exactly.
fn unescape_line(line: &str) -> String {
    if line == "\\" {
        return String::new();
    }
    let ws = line.len() - line.trim_start().len();
    let (lead, body) = line.split_at(ws);
    let candidate = if let Some(rest) = body.strip_prefix('\\') {
        Some(format!("{lead}{rest}"))
    } else {
        let digits = body.bytes().take_while(u8::is_ascii_digit).count();
        (digits > 0 && body[digits..].starts_with('\\'))
            .then(|| format!("{lead}{}{}", &body[..digits], &body[digits + 1..]))
    };
    match candidate {
        Some(c) if escape_line(&c) == line => c,
        _ => line.to_string(),
    }
}

/// `![alt](src)`, `![alt|width](src)` or `![alt|right|width](src)`: a
/// `src` with spaces or brackets goes in `<…>`, as CommonMark allows.
pub fn image_markdown(alt: &str, src: &str, width: Option<u32>, float: Float) -> String {
    let mut alt = alt.replace(['[', ']', '\n'], " ");
    match float {
        Float::None => {}
        Float::Left => alt.push_str("|left"),
        Float::Right => alt.push_str("|right"),
    }
    if let Some(w) = width {
        alt.push_str(&format!("|{w}"));
    }
    if src.contains([' ', '(', ')', '<', '>']) {
        format!("![{alt}](<{src}>)")
    } else {
        format!("![{alt}]({src})")
    }
}

/// `![[src]]`, `![[src|alt|right|400]]`: a picture as Obsidian embeds one,
/// the alt, side and width after the source as they are in `![…](…)`.
/// `None` for a source the form can't hold.
pub fn image_wiki(alt: &str, src: &str, width: Option<u32>, float: Float) -> Option<String> {
    if src.is_empty() || src.contains(['[', ']', '|', '\n']) {
        return None;
    }
    let mut out = format!("![[{src}");
    let alt = alt.replace(['[', ']', '\n'], " ");
    if !alt.is_empty() {
        out.push('|');
        out.push_str(&alt);
    }
    match float {
        Float::None => {}
        Float::Left => out.push_str("|left"),
        Float::Right => out.push_str("|right"),
    }
    if let Some(w) = width {
        out.push_str(&format!("|{w}"));
    }
    out.push_str("]]");
    Some(out)
}

/// The extensions of the pictures a note shows, lowercase: what an
/// Obsidian embed (`![[…]]`) of a file is a picture for.
const PICTURE_EXTENSIONS: [&str; 12] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "heic", "heif", "tif", "tiff"];

/// Whether a file name or path names a picture, by its extension.
pub fn is_picture_name(name: &str) -> bool {
    has_extension(name, &PICTURE_EXTENSIONS)
}

/// The extensions of the files besides pictures a note keeps with it and
/// links to, lowercase: documents, sheets, slides, sound and film, each
/// opened from its link by an app of its own (the app's `opens_safely`).
/// Nothing a system runs or a browser scripts (a program, a script,
/// `.html`), none of Office's kinds that can carry macros (`.doc`,
/// `.docm`, `.xlsm`), no bundle that is a folder (`.pages`), nothing that
/// changes something as it opens (`.ics`), and no `.md`, which the notes
/// folder would read as a note.
const DOCUMENT_EXTENSIONS: [&str; 22] = [
    "pdf", "txt", "csv", "rtf", "docx", "xlsx", "pptx", "odt", "ods", "odp", "m4a", "mp3", "wav", "aac", "aiff", "caf", "flac", "ogg",
    "mov", "mp4", "m4v", "webm",
];

/// Whether a file name or path names a document a note may keep and link
/// to ([`DOCUMENT_EXTENSIONS`]), by its extension.
pub fn is_document_name(name: &str) -> bool {
    has_extension(name, &DOCUMENT_EXTENSIONS)
}

fn has_extension(name: &str, of: &[&str]) -> bool {
    let name = name.rsplit('/').next().unwrap_or(name);
    name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && of.contains(&ext.to_ascii_lowercase().as_str()))
}

/// `![[photo.png]]` and its kin, alone on a line: a picture, when the
/// file it embeds is one by its name (`![[Other note]]` and
/// `![[file.pdf]]` are not). After the source, a width (`|300`, or
/// `|300x200`, its height left to the picture's shape), a side
/// (`|left`, `|right`), and the rest the alt text.
fn wiki_image(line: &str) -> Option<Kind> {
    let inner = line.strip_prefix("![[")?.strip_suffix("]]")?;
    if inner.contains(['[', ']']) {
        return None;
    }
    let mut parts = inner.split('|');
    let src = parts.next()?.trim();
    if !is_picture_name(src) || src.contains("://") {
        return None;
    }
    let mut rest: Vec<&str> = parts.map(str::trim).collect();
    let mut width = None;
    if let Some(last) = rest.last() {
        let w = last.split_once('x').map_or(*last, |(w, h)| if !h.is_empty() && h.bytes().all(|b| b.is_ascii_digit()) { w } else { last });
        if !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()) {
            width = w.parse().ok();
            rest.pop();
        }
    }
    let float = match rest.last() {
        Some(&"left") => Float::Left,
        Some(&"right") => Float::Right,
        _ => Float::None,
    };
    if float != Float::None {
        rest.pop();
    }
    Some(Kind::Image { src: src.to_string(), alt: rest.join("|"), width, float, wiki: true })
}

/// A line that is one image and nothing else, as its block: the alt text,
/// the source, a width (`|400` at the alt's end) and a side (`|left`,
/// `|right` before it). A title after the source (`"…"`) is dropped. A
/// source with spaces written bare, as some exports write it, is the
/// source all the same; an HTML `<img>` alone on its line is a picture
/// too, and so is an Obsidian embed of one, `![[photo.png]]`.
pub fn image_line(line: &str) -> Option<Kind> {
    let line = line.trim();
    if line.starts_with("<img") {
        return html_image(line);
    }
    if line.starts_with("![[") {
        return wiki_image(line);
    }
    let rest = line.strip_prefix("![")?;
    let close = rest.find("](")?;
    let alt = &rest[..close];
    let target = rest[close + 2..].strip_suffix(')')?.trim();
    let src = match target.strip_prefix('<') {
        Some(t) => t.strip_suffix('>')?,
        None => {
            let t = target.split_once(" \"").map_or(target, |(a, _)| a);
            if t.contains(['\t', '\n', '(', ')']) {
                return None;
            }
            t
        }
    };
    if src.is_empty() || alt.contains(['[', ']']) {
        return None;
    }
    let (alt, width, float) = alt_parts(alt);
    Some(Kind::Image { src: src.to_string(), alt: alt.to_string(), width, float, wiki: false })
}

/// Pictures one after another on a line of their own, nothing but spaces
/// between (`![a](x.png)![b|left|200](y.png)`): a row of them, as other
/// apps draw such a line, side by side. One with no side written stands
/// at the left, as the others in the row do. Written back (only once the
/// note is edited), a picture a line, each with its side, as Carnet
/// writes a row. `None` for fewer than two, or anything else on the line.
pub fn image_row(line: &str) -> Option<Vec<Kind>> {
    let pics = inline_pictures(line);
    if pics.len() < 2 {
        return None;
    }
    let mut at = 0;
    let mut out = Vec::new();
    for p in &pics {
        if !line[at..p.at.0].trim().is_empty() {
            return None;
        }
        out.push(image_line(&line[p.at.0..p.at.1])?);
        at = p.at.1;
    }
    if !line[at..].trim().is_empty() {
        return None;
    }
    for k in &mut out {
        if let Kind::Image { float, .. } = k
            && *float == Float::None
        {
            *float = Float::Left;
        }
    }
    Some(out)
}

/// A picture among a table cell's words, as the cell's text has it: the
/// text stays the Markdown, and this says what part of it is a picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellPicture {
    /// Where it is in the text, `!` to its closing bracket.
    pub at: (usize, usize),
    /// Where its `src` is in the text: what a move rewrites.
    pub src_at: (usize, usize),
    pub src: String,
    pub alt: String,
    /// The width written for it. A cell gives its pictures the room it
    /// has, so it is read, never drawn by: see [`CellPicture::plain`].
    pub width: Option<u32>,
    /// The side written for it (`|left`), which means nothing among words.
    pub float: Float,
    /// Written as Obsidian embeds one (`![[…]]`): `src` a name to look
    /// for ([`find_picture`]).
    pub wiki: bool,
}

impl CellPicture {
    /// The picture as a cell holds one: its alt text and its source, and
    /// no width nor side, which a cell decides for it. An embed stays one.
    pub fn plain(&self) -> String {
        let wiki = self.wiki.then(|| image_wiki(&self.alt, &self.src, None, Float::None)).flatten();
        wiki.unwrap_or_else(|| image_markdown(&self.alt, &self.src, None, Float::None))
    }

    /// Whether it is written with a width or a side: more than [`Self::plain`].
    pub fn dressed(&self) -> bool {
        self.width.is_some() || self.float != Float::None
    }
}

/// `text` with each picture among its words (as a cell reads them)
/// written plainly ([`CellPicture::plain`]): what goes into a cell.
pub fn plain_pictures(text: &str) -> String {
    let mut out = text.to_string();
    for p in inline::cell_pictures(text).iter().rev().filter(|p| p.dressed()) {
        out.replace_range(p.at.0..p.at.1, &p.plain());
    }
    out
}

/// The alt text's own words, the width at its end (`|400`) and the side
/// before that (`|left`, `|right`).
fn alt_parts(alt: &str) -> (&str, Option<u32>, Float) {
    let (alt, width) = match alt.rsplit_once('|') {
        Some((a, w)) if !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()) => (a, w.parse().ok()),
        _ => (alt, None),
    };
    let (alt, float) = match alt.rsplit_once('|') {
        Some((a, "left")) => (a, Float::Left),
        Some((a, "right")) => (a, Float::Right),
        _ => (alt, Float::None),
    };
    (alt, width, float)
}

/// The picture that starts at byte `at` of a line of words, if one does:
/// `![alt](src)`, `![alt|260](<a b.png>)`, or an Obsidian embed of one,
/// `![[photo.png|260]]`, as [`image_line`] reads one alone on its line.
/// Its `|`s are its own, and may be written `\|`, as a table's cell has
/// other editors write them; a side (`|left`) is kept in the text but
/// means nothing among words.
pub fn inline_picture(text: &str, at: usize) -> Option<CellPicture> {
    Pictures::new(text).at(at)
}

/// Where the next byte that `is` it is, from a point on, kept from one
/// question to the next: asked along a line, left to right, it reads the
/// line once, however many pictures start (or almost start) on it.
struct Next {
    is: fn(&[u8], usize) -> bool,
    /// The last question, and its answer.
    from: usize,
    found: Option<usize>,
}

impl Next {
    fn new(is: fn(&[u8], usize) -> bool) -> Self {
        Next { is, from: usize::MAX, found: None }
    }

    fn after(&mut self, b: &[u8], from: usize) -> Option<usize> {
        // Nothing found from an earlier point, or found no earlier than
        // here: the same answer.
        if from < self.from || self.found.is_some_and(|f| f < from) {
            self.from = from;
            self.found = (from..b.len()).find(|&i| (self.is)(b, i));
        }
        self.found
    }
}

/// The pictures of a line of words, read at one point after another
/// ([`Pictures::at`]): what each reads past is remembered, so that a line
/// of things that only start pictures (`![[![[![[…`) is read in one go,
/// not once for each.
pub(crate) struct Pictures<'a> {
    text: &'a str,
    /// The first `[`, `]` or line break: what ends a picture's alt text
    /// (or an embed's words), well or not.
    bracket: Next,
    paren: Next,
    angle: Next,
    newline: Next,
    title: Next,
    /// What a source written bare can't hold.
    unsafe_src: Next,
    /// A bare source's last `)`, and where its spaces before it start.
    trimmed: Option<(usize, usize)>,
}

impl<'a> Pictures<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Pictures {
            text,
            bracket: Next::new(|b, i| matches!(b[i], b'[' | b']' | b'\n')),
            paren: Next::new(|b, i| b[i] == b')'),
            angle: Next::new(|b, i| b[i] == b'>' && b.get(i + 1) == Some(&b')')),
            newline: Next::new(|b, i| b[i] == b'\n'),
            title: Next::new(|b, i| b[i] == b' ' && b.get(i + 1) == Some(&b'"')),
            unsafe_src: Next::new(|b, i| matches!(b[i], b'\t' | b'(' | b'\n')),
            trimmed: None,
        }
    }

    /// The picture that starts at byte `at`, as [`inline_picture`].
    pub(crate) fn at(&mut self, at: usize) -> Option<CellPicture> {
        let text = self.text;
        let b = text.as_bytes();
        let rest = text.get(at..)?;
        if rest.starts_with("![[") {
            // Its words up to `]]`, with no other bracket nor line break.
            let close = self.bracket.after(b, at + 3)?;
            if b[close] != b']' || b.get(close + 1) != Some(&b']') {
                return None;
            }
            let len = close - (at + 3);
            let raw = &text[at + 3..close];
            let Some(Kind::Image { src, alt, width, float, wiki }) = wiki_image(&format!("![[{}]]", raw.replace("\\|", "|"))) else { return None };
            // The source as written: before the first `|` (or `\|`), its
            // spaces left out.
            let first = raw.split('|').next().unwrap_or(raw);
            let first = first.strip_suffix('\\').unwrap_or(first);
            let lead = first.len() - first.trim_start().len();
            let s = at + 3 + lead;
            if text.get(s..s + src.len()) != Some(src.as_str()) {
                return None;
            }
            return Some(CellPicture { at: (at, at + 3 + len + 2), src_at: (s, s + src.len()), src, alt, width, float, wiki });
        }
        if !rest.starts_with("![") {
            return None;
        }
        // The alt text, up to `]`, with no other bracket nor line break.
        let close = self.bracket.after(b, at + 2)?;
        if b[close] != b']' || b.get(close + 1) != Some(&b'(') {
            return None;
        }
        let alt = &text[at + 2..close];
        let t0 = close + 2;
        let ((s, e), end) = if b.get(t0) == Some(&b'<') {
            let n = self.angle.after(b, t0 + 1)?;
            ((t0 + 1, n), n + 2)
        } else {
            let paren = self.paren.after(b, t0)?;
            let t = &text[t0..paren];
            let s = t0 + (t.len() - t.trim_start().len());
            // Where its spaces before the `)` start: the same `)` closes
            // every picture started before it, so it is looked for once.
            let bare_end = match self.trimmed {
                Some((p, e)) if p == paren => e,
                _ => {
                    let e = t0 + t.trim_end().len();
                    self.trimmed = Some((paren, e));
                    e
                }
            };
            // A title after the source (`"…"`) is dropped, as alone.
            let e = match self.title.after(b, s) {
                Some(q) if q < paren => q,
                _ => bare_end.max(s),
            };
            if self.unsafe_src.after(b, s).is_some_and(|k| k < e) {
                return None;
            }
            ((s, e), paren + 1)
        };
        if s == e || self.newline.after(b, s).is_some_and(|k| k < e) {
            return None;
        }
        let src = &text[s..e];
        let alt = alt.replace("\\|", "|");
        let (alt, width, float) = alt_parts(&alt);
        Some(CellPicture { at: (at, end), src_at: (s, e), src: src.to_string(), alt: alt.to_string(), width, float, wiki: false })
    }
}

/// Every picture in a line of words, as [`inline_picture`] reads them,
/// one after another: a line that only looks like one counts (in code,
/// say), as for [`picture_paths`].
pub fn inline_pictures(text: &str) -> Vec<CellPicture> {
    let mut out = Vec::new();
    let mut pictures = Pictures::new(text);
    let mut i = 0;
    while let Some(k) = text[i..].find("![") {
        match pictures.at(i + k) {
            Some(p) => {
                i = p.at.1;
                out.push(p);
            }
            None => i += k + 2,
        }
    }
    out
}

/// A cell's text with the `src` of each of its pictures `to` gives one
/// (`None`: as it is): the rest of the text as it was. `None` when no
/// picture changed.
pub fn relink_cell(cell: &str, mut to: impl FnMut(&CellPicture) -> Option<String>) -> Option<String> {
    let mut out = cell.to_string();
    let mut changed = false;
    // From the last, so the places before stay where they are.
    for p in inline::cell_pictures(cell).iter().rev() {
        let Some(new) = to(p).filter(|n| *n != p.src) else { continue };
        let (s, e) = p.src_at;
        // A bare source that now needs its `<…>`; an embed's that can't
        // hold the new one stays.
        let bare = !p.wiki && cell.as_bytes().get(s.wrapping_sub(1)) != Some(&b'<');
        let new = match p.wiki {
            true if new.contains(['[', ']', '|', '\n']) => continue,
            false if bare && new.contains([' ', '(', ')', '<', '>']) => format!("<{new}>"),
            _ => new,
        };
        out.replace_range(s..e, &new);
        changed = true;
    }
    changed.then_some(out)
}

/// A picture's `src`, from a note in `folder`, as a path from the notes
/// folder (`attachments/x.png`); `None` for a web address or a path that
/// climbs out of it.
pub fn resolve_picture(folder: &str, src: &str) -> Option<String> {
    if src.contains("://") || src.starts_with("data:") || src.starts_with(['/', '\\']) {
        return None;
    }
    join_path(folder, &src.replace("%20", " "))
}

/// `path` from `folder`, `..` and `.` worked out, as a path from the
/// notes folder; `None` if it climbs out, or has a part that isn't one
/// plain name ([`is_plain_name`](crate::name::is_plain_name): a drive, a
/// device). `\` parts it as `/` does, as Windows reads it: what a
/// note's text says is never more than it looks.
fn join_path(folder: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> = folder.split('/').filter(|p| !p.is_empty()).collect();
    for p in path.split(['/', '\\']) {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p if crate::name::is_plain_name(p) => parts.push(p),
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

/// The files in a notes folder, by their paths from it (`/` between
/// parts): what a picture an Obsidian embed names is looked for among.
/// Case aside, as the file systems notes mostly live on see names.
#[derive(Clone, Debug, Default)]
pub struct Files {
    /// Each path, lowercase, to the path as it is.
    paths: std::collections::HashMap<String, String>,
    /// Each file name, lowercase, to the paths (lowercase) that end in it.
    names: std::collections::HashMap<String, Vec<String>>,
}

impl Files {
    pub fn new<S: Into<String>>(paths: impl IntoIterator<Item = S>) -> Self {
        let mut f = Files::default();
        for p in paths {
            f.insert(p.into());
        }
        f
    }

    pub fn insert(&mut self, path: String) {
        let low = path.to_lowercase();
        let name = low.rsplit('/').next().unwrap_or(&low).to_string();
        if self.paths.insert(low.clone(), path).is_none() {
            self.names.entry(name).or_default().push(low);
        }
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Every file's path, as it is written.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.paths.values().map(String::as_str)
    }

    /// The file at `path`, as its path is written.
    pub fn get(&self, path: &str) -> Option<&str> {
        self.paths.get(&path.to_lowercase()).map(String::as_str)
    }

    /// The file whose path ends in `tail` (a name, or a path's last
    /// parts): one in the attachments folder first, then the one with the
    /// fewest folders above it, as Obsidian takes the shortest path.
    fn ending(&self, tail: &str) -> Option<&str> {
        let tail = tail.trim_start_matches("./").to_lowercase();
        let name = tail.rsplit('/').next().unwrap_or(&tail);
        let suffix = format!("/{tail}");
        self.names
            .get(name)?
            .iter()
            .filter(|p| **p == tail || p.ends_with(&suffix))
            .min_by_key(|p| (!p.starts_with("attachments/"), p.matches('/').count(), p.len(), p.as_str()))
            .and_then(|p| self.get(p))
    }
}

/// Where a picture is, as a path from the notes folder, for a note in
/// `folder`. One written as a path (`![](…)`) is where its path says
/// ([`resolve_picture`]). One an Obsidian embed names (`wiki`) is looked
/// for among `files` as Obsidian looks: from the note's folder, from the
/// notes folder, then by its name (or its path's last parts) anywhere,
/// the attachments folder first. Not found, it is where its path from the
/// notes folder says. `None` for a web address or a path out of the notes
/// folder.
pub fn find_picture(folder: &str, src: &str, wiki: bool, files: &Files) -> Option<String> {
    if !wiki {
        return resolve_picture(folder, src);
    }
    if src.contains("://") || src.starts_with('/') {
        return None;
    }
    let (here, top) = (join_path(folder, src), join_path("", src));
    for p in [&here, &top].into_iter().flatten() {
        if let Some(f) = files.get(p) {
            return Some(f.to_string());
        }
    }
    if let Some(f) = files.ending(src) {
        return Some(f.to_string());
    }
    top.or(here)
}

/// The pictures a note in `folder` shows, and the documents it links to
/// ([`document_paths`]), as paths from the notes folder: what keeps a
/// file wanted — a picture on a line of its own, and one among a table
/// cell's words. A line that only looks like one (in a code block, say)
/// counts too: a file kept by mistake costs little, one lost is gone.
/// `files` is where an Obsidian embed's picture is looked for
/// ([`find_picture`]).
pub fn picture_paths(folder: &str, md: &str, files: &Files) -> std::collections::BTreeSet<String> {
    let mut out = document_paths(folder, md, files);
    for line in md.lines() {
        match image_line(line) {
            Some(Kind::Image { src, wiki, .. }) => out.extend(find_picture(folder, &src, wiki, files)),
            // Among a table's cell's words (a row, or a line that looks
            // like one); among a paragraph's, a picture is no picture.
            _ if line.trim_start().starts_with('|') => {
                out.extend(inline_pictures(line).iter().filter_map(|p| find_picture(folder, &p.src, p.wiki, files)));
            }
            _ => {}
        }
    }
    out
}

/// The documents ([`is_document_name`]) a note in `folder` links to, as
/// paths from the notes folder: `[…](report.pdf)`, `[…](<a b.pdf>)` and
/// an Obsidian `[[report.pdf]]`, its `%` escapes read. Only by a
/// document's name, so a link to a note or a page is none; one in code
/// counts too, as for pictures.
pub fn document_paths(folder: &str, md: &str, files: &Files) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let b = md.as_bytes();
    let mut i = 0;
    while let Some(at) = md[i..].find("](") {
        let start = i + at + 2;
        let line_end = md[start..].find('\n').map_or(md.len(), |n| start + n);
        let rest = &md[start..line_end];
        // `<…>`, or up to the `)` that closes it (parentheses in pairs),
        // a title after it left out. The next looked for past what this
        // one read — its end, or the line's when it has none — so a line
        // of a thousand `](` is read once, not a thousand times.
        let (target, read) = match rest.strip_prefix('<') {
            Some(r) => match r.find('>') {
                Some(e) => (Some(&r[..e]), e + 1),
                None => (None, rest.len()),
            },
            None => {
                let mut depth = 0usize;
                let close = rest.char_indices().find(|&(_, c)| match c {
                    '(' => {
                        depth += 1;
                        false
                    }
                    ')' if depth == 0 => true,
                    ')' => {
                        depth -= 1;
                        false
                    }
                    _ => false,
                });
                match close {
                    Some((e, _)) => {
                        let t = &rest[..e];
                        (Some(t.find(" \"").or_else(|| t.find(" '")).map_or(t, |s| &t[..s]).trim()), e)
                    }
                    None => (None, rest.len()),
                }
            }
        };
        if let Some(t) = target.map(percent_decoded).filter(|t| is_document_name(t)) {
            out.extend(resolve_picture(folder, &t));
        }
        i = (start + read).min(b.len());
    }
    let mut i = 0;
    while let Some(at) = md[i..].find("[[") {
        let start = i + at + 2;
        let Some(len) = md[start..].find(['|', ']', '[', '\n']) else { break };
        let t = md[start..start + len].trim();
        if is_document_name(t) {
            out.extend(find_picture(folder, t, true, files));
        }
        i = start + len;
    }
    out
}

/// `%` escapes read back (`%20` a space, `%28` a parenthesis); one that
/// isn't two hex digits, or that makes no UTF-8, kept as written.
fn percent_decoded(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = b
                .get(i + 1..i + 3)
                .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// `<img src="…" alt="…" width="…">` (or `/>`), as an export writes a
/// picture: the picture, its alt text and width kept.
fn html_image(line: &str) -> Option<Kind> {
    let inner = line.strip_prefix("<img")?.strip_suffix('>')?;
    let inner = inner.strip_suffix('/').unwrap_or(inner);
    if !inner.starts_with(char::is_whitespace) || inner.contains(['<', '>']) {
        return None;
    }
    let attr = |name: &str| -> Option<String> {
        let pat = format!("{name}=");
        let mut from = 0;
        while let Some(at) = inner[from..].find(&pat) {
            let at = from + at;
            from = at + pat.len();
            if !inner[..at].ends_with(char::is_whitespace) {
                continue;
            }
            let rest = &inner[from..];
            let value = match rest.chars().next()? {
                q @ ('"' | '\'') => &rest[1..1 + rest[1..].find(q)?],
                _ => rest.split(char::is_whitespace).next()?,
            };
            let value = value.replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">");
            return Some(value.replace("&amp;", "&"));
        }
        None
    };
    let src = attr("src").filter(|s| !s.trim().is_empty())?;
    let alt = attr("alt").unwrap_or_default().replace(['[', ']', '\n'], " ");
    let width = attr("width").and_then(|w| w.trim_end_matches("px").parse().ok());
    Some(Kind::Image { src, alt, width, float: Float::None, wiki: false })
}

/// A cell as its row writes it: its `|`s escaped, but for a picture's,
/// which are written as they are (`![|left|260](a.png)`), as other
/// editors write them and [`split_row`] reads them.
fn escape_cell(cell: &str) -> String {
    let mut out = String::with_capacity(cell.len());
    let mut last = 0;
    for p in inline_pictures(cell) {
        out.push_str(&cell[last..p.at.0].replace('|', "\\|"));
        out.push_str(&cell[p.at.0..p.at.1]);
        last = p.at.1;
    }
    out.push_str(&cell[last..].replace('|', "\\|"));
    out.replace('\n', " ")
}

// ------------------------------------------------------------ markdown in

/// Parses a note: the first `# ` heading is the title (else `None`), the
/// rest is blocks. Always returns a document that ends in a text block.
pub fn from_markdown(src: &str) -> (Option<String>, Doc) {
    let (title, blocks) = parse(src, true);
    let mut doc = Doc::new(blocks);
    doc.ensure_tail();
    (title, doc)
}

/// The blocks a piece of a note's body makes: what a paragraph becomes
/// when a line typed or pasted into it reads as structure. No title, and
/// no setext headings — a `---` typed under a line is a divider, not a
/// heading made of the line above.
pub fn parse_blocks(src: &str) -> Vec<Block> {
    parse(src, false).1
}

/// `md` with its `n`th checkbox (from 0, in the note's order) ticked or
/// unticked: the one character inside its `[ ]` changed, and nothing else,
/// so a tap is a one-byte edit that merges with anything else. `None` if
/// the note has no such checkbox, or the lines can't be matched to the
/// checkboxes the note reads as (then nothing is touched).
pub fn toggle_task(md: &str, n: usize) -> Option<String> {
    // As the note reads whole (front matter its own), as the viewer draws it.
    let expected = from_markdown(md).1.blocks().iter().filter(|b| matches!(b.kind, Kind::Item { style: ListStyle::Todo, .. })).count();
    // Where each checkbox's inner character is, in the note's order.
    let mut boxes = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut at = 0;
    let mut front = md.starts_with("---\n") || md.starts_with("---\r\n");
    for (i, line) in md.split_inclusive('\n').enumerate() {
        let start = at;
        at += line.len();
        let line = line.trim_end_matches(['\n', '\r']);
        if front {
            if i > 0 && line == "---" {
                front = false;
            }
            continue;
        }
        if let Some((ch, len)) = fence {
            if closes_fence(line, ch, len) {
                fence = None;
            }
            continue;
        }
        if let Some(f) = fence_of(line.trim_start()) {
            fence = Some(f);
            continue;
        }
        if let Some((_, _, Kind::Item { style: ListStyle::Todo, .. })) = list_marker(line) {
            let lead = line.len() - line.trim_start_matches([' ', '\t']).len();
            // The marker (`- `), then `[`, then the character that says.
            boxes.push(start + lead + 3);
        }
    }
    if boxes.len() != expected {
        return None;
    }
    let i = *boxes.get(n)?;
    let flipped = match md.as_bytes()[i] {
        b' ' => "x",
        b'x' | b'X' => " ",
        _ => return None,
    };
    Some(format!("{}{flipped}{}", &md[..i], &md[i + 1..]))
}

fn parse(src: &str, whole_note: bool) -> (Option<String>, Vec<Block>) {
    // A lone `\r` ends a line too, as CommonMark has it.
    let src = src.strip_prefix('\u{feff}').unwrap_or(src).replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = src.split('\n').collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut title = None;
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, blocks: &mut Vec<Block>| {
        // A lone `\` is the writer's blank line inside a paragraph, and it
        // writes no paragraph of blank lines alone: lines of one `\` and
        // nothing else are the backslashes they say.
        if para.iter().all(String::is_empty) {
            para.iter_mut().for_each(|l| *l = "\\".into());
        }
        if !para.is_empty() {
            let text = para.join("\n");
            // A dropped first letter: the paragraph's, out of its words.
            let b = match inline::take_initial(&text) {
                Some(words) => Block { initial: true, ..Block::text(words) },
                None => Block::text(text),
            };
            blocks.push(b);
            para.clear();
        }
    };
    // Whether a heading here is still the note's title: nothing but front
    // matter above it.
    let title_slot = |title: &Option<String>, blocks: &[Block]| {
        whole_note && title.is_none() && blocks.iter().all(|b| matches!(&b.kind, Kind::Raw(t) if is_front_matter(t)))
    };
    // The open list's levels: where each item's marker and text start.
    let mut list: Vec<(usize, usize)> = Vec::new();
    let mut blank_run = 0;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            flush(&mut para, &mut blocks);
            blank_run += 1;
            // Two blank lines in a row is an intentional gap: keep it.
            if blank_run == 2 && !blocks.is_empty() {
                blocks.push(Block::text(""));
                list.clear();
            }
            i += 1;
            continue;
        }
        blank_run = 0;
        // Front matter: a `---` fence on the very first line.
        if i == 0
            && whole_note
            && line == "---"
            && let Some(end) = (1..lines.len()).find(|&j| lines[j] == "---" || lines[j] == "...")
        {
            blocks.push(Block::new(Kind::Raw(lines[..=end].join("\n"))));
            i = end + 1;
            continue;
        }
        if let Some((ch, n)) = fence_of(line.trim_start()) {
            flush(&mut para, &mut blocks);
            list.clear();
            let end = (i + 1..lines.len())
                .find(|&j| closes_fence(lines[j], ch, n))
                .unwrap_or(lines.len() - 1);
            blocks.push(Block::new(Kind::Raw(lines[i..=end].join("\n"))));
            i = end + 1;
            continue;
        }
        if line.starts_with('\\') || line.trim_start().starts_with('\\') || has_escaped_number(line)
        {
            // Escaped by the writer: a paragraph line, whatever it says.
            list.clear();
            para.push(unescape_line(line));
            i += 1;
            continue;
        }
        if quote_line(line).is_some() {
            flush(&mut para, &mut blocks);
            list.clear();
            let mut body = Vec::new();
            while let Some(rest) = lines.get(i).and_then(|l| quote_line(l)) {
                body.push(rest);
                i += 1;
            }
            blocks.push(Block::new(Kind::Quote(body.join("\n"))));
            continue;
        }
        if let Some((level, text)) = heading(line) {
            flush(&mut para, &mut blocks);
            list.clear();
            if level == 1 && title_slot(&title, &blocks) {
                title = Some(text.to_string());
            } else {
                blocks.push(Block::new(Kind::Heading {
                    level,
                    text: text.to_string(),
                }));
            }
            i += 1;
            continue;
        }
        // A setext heading: a paragraph underlined with `===` or `---`.
        if whole_note && !para.is_empty() && (is_setext_eq(line) || is_dash_rule(line)) {
            let level = if is_setext_eq(line) { 1 } else { 2 };
            // Its spaces at the end go, as a `## ` heading's do.
            let text = para.join(" ").trim_end().to_string();
            para.clear();
            if level == 1 && title_slot(&title, &blocks) {
                title = Some(text);
            } else {
                blocks.push(Block::new(Kind::Heading { level, text }));
            }
            i += 1;
            continue;
        }
        if is_rule(line) {
            flush(&mut para, &mut blocks);
            list.clear();
            blocks.push(Block::new(Kind::Divider));
            i += 1;
            continue;
        }
        // Several pictures on the line first: read whole, `![a](<x>)![b](<y>)`
        // could pass for one whose source runs on.
        if let Some(row) = image_row(line) {
            flush(&mut para, &mut blocks);
            list.clear();
            blocks.extend(row.into_iter().map(Block::new));
            i += 1;
            continue;
        }
        if let Some(image) = image_line(line) {
            flush(&mut para, &mut blocks);
            list.clear();
            blocks.push(Block::new(image));
            i += 1;
            continue;
        }
        if let Some((lead, width, mut kind)) = list_marker(line) {
            flush(&mut para, &mut blocks);
            // Nesting: under the item above when indented to its text (or
            // by two spaces, which is how lists are often hand-written).
            while let Some(&(plead, ptext)) = list.last() {
                if lead >= ptext || lead >= plead + 2 {
                    break;
                }
                list.pop();
            }
            if let Kind::Item { indent, .. } = &mut kind {
                *indent = (list.len() as u8).min(MAX_INDENT);
            }
            list.push((lead, lead + width));
            blocks.push(Block::new(kind));
            i += 1;
            continue;
        }
        if line.trim_start().starts_with('|') && lines.get(i + 1).is_some_and(|l| is_separator(l))
        {
            flush(&mut para, &mut blocks);
            list.clear();
            let mut cells = vec![split_row(line)];
            let aligns = split_row(lines[i + 1]);
            i += 2;
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                cells.push(split_row(lines[i]));
                i += 1;
            }
            let mut table = Table::from_cells(cells);
            // A cell of the separator that is not dashes (`|||-`) keeps the
            // plain one: written as it is, the row would not read back as one.
            for (c, a) in aligns.into_iter().enumerate().take(table.width()) {
                if a.contains('-') && a.chars().all(|c| matches!(c, '-' | ':')) {
                    table.aligns[c] = a;
                }
            }
            // An empty header row over others: a table without a header.
            if table.rows.len() > 1 && table.is_blank_row(0) {
                table.remove_row(0);
            }
            blocks.push(Block::new(Kind::Table(table)));
            continue;
        }
        list.clear();
        para.push(line.to_string());
        i += 1;
    }
    flush(&mut para, &mut blocks);
    (title, blocks)
}

/// A line of a quotation, without its `>` (and the space after it). Up
/// to three spaces may come first: spaces, not any whitespace, which
/// could end inside a letter (`\u{85}` is two bytes; found by fuzzing).
fn quote_line(line: &str) -> Option<&str> {
    let ws = line.bytes().take_while(|b| *b == b' ').count();
    let rest = line[ws.min(3)..].strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

fn has_escaped_number(line: &str) -> bool {
    let body = line.trim_start();
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && body[digits..].starts_with('\\')
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    let text = rest.strip_prefix(' ').or((rest.is_empty()).then_some(""))?;
    Some((hashes as u8, text.trim_end()))
}

fn is_rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && ['-', '*', '_'].iter().any(|c| t.chars().all(|x| x == *c))
}

fn is_dash_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| c == '-')
}

fn is_setext_eq(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 2 && t.chars().all(|c| c == '=')
}

fn is_front_matter(t: &str) -> bool {
    t.starts_with("---\n")
}

/// A code fence opening: its character and length.
fn fence_of(body: &str) -> Option<(char, usize)> {
    let ch = body.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = body.chars().take_while(|c| *c == ch).count();
    (n >= 3).then_some((ch, n))
}

fn closes_fence(line: &str, ch: char, n: usize) -> bool {
    let t = line.trim();
    t.chars().take_while(|c| *c == ch).count() >= n && t.chars().all(|c| c == ch)
}

/// `lead` spaces, a marker `width` wide, and the item it makes.
fn list_marker(line: &str) -> Option<(usize, usize, Kind)> {
    let lead = line.len() - line.trim_start_matches([' ', '\t']).len();
    let tabs = line[..lead].matches('\t').count();
    let lead_cols = lead + tabs; // a tab counts two
    let body = &line[lead..];
    let item = |style, done, num, text: &str| Kind::Item {
        style,
        indent: 0,
        done,
        num,
        text: text.to_string(),
    };
    if let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|p| body.strip_prefix(p)) {
        if let Some((done, t)) = checkbox_prefix(rest) {
            return Some((lead_cols, 2, item(ListStyle::Todo, done, 0, t)));
        }
        // `- \[ ] x`, `- \---`: a bullet the writer kept from reading as a
        // checkbox, or as a divider.
        let text = match rest.strip_prefix('\\') {
            Some(r) if checkbox_prefix(r).is_some() || is_rule(&format!("- {r}")) => r,
            _ => rest,
        };
        return Some((lead_cols, 2, item(ListStyle::Bullet, false, 0, text)));
    }
    if let Some((n, rest)) = numbered_prefix(body) {
        let width = body.len() - rest.len();
        return Some((lead_cols, width, item(ListStyle::Number, false, n, rest)));
    }
    if matches!(body, "-" | "*" | "+") {
        return Some((lead_cols, 2, item(ListStyle::Bullet, false, 0, "")));
    }
    None
}

fn is_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut pictures = Pictures::new(t);
    let mut i = 0;
    while let Some(c) = t[i..].chars().next() {
        // A picture's `|`s are its own (`![|left|260](a.png)`), as other
        // editors read them: it is taken whole, as written.
        if c == '!'
            && let Some(p) = pictures.at(i)
        {
            cur.push_str(&t[i..p.at.1]);
            i = p.at.1;
            continue;
        }
        i += c.len_utf8();
        match c {
            // Only an escaped pipe is the writer's; every other backslash
            // is the cell's own.
            '\\' if t[i..].starts_with('|') => {
                i += 1;
                cur.push('|');
            }
            '|' => cells.push(std::mem::take(&mut cur).trim().to_string()),
            c => cur.push(c),
        }
    }
    // A row that does not end in `|` still has its last cell.
    if !cur.trim().is_empty() {
        cells.push(cur.trim().to_string());
    }
    cells
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_tap_ticks_one_checkbox_and_touches_nothing_else() {
        let md = "# Shop\n\n- [ ] milk\n* [X] bread\n  + [ ] rye\n\n```\n- [ ] not a box\n```\n\n> - [ ] quoted\n\n- [ ]\n";
        let t = toggle_task(md, 0).unwrap();
        assert_eq!(t, md.replacen("- [ ] milk", "- [x] milk", 1));
        assert_eq!(toggle_task(md, 1).unwrap(), md.replacen("[X] bread", "[ ] bread", 1));
        assert_eq!(toggle_task(md, 2).unwrap(), md.replacen("+ [ ] rye", "+ [x] rye", 1));
        assert_eq!(toggle_task(md, 3).unwrap(), md.replacen("- [ ]\n", "- [x]\n", 1), "an empty one, last");
        assert_eq!(toggle_task(md, 4), None, "the fenced and quoted ones aren't boxes");
        // Twice is as before; front matter is not the note.
        assert_eq!(toggle_task(&toggle_task(md, 0).unwrap(), 0).unwrap(), md);
        let fm = "---\ntags: [x]\n- [ ] in front matter\n---\n- [ ] real\n";
        assert_eq!(toggle_task(fm, 0).unwrap(), fm.replacen("- [ ] real", "- [x] real", 1));
        // Windows line ends stay.
        assert_eq!(toggle_task("- [ ] a\r\n- [ ] b\r\n", 1).unwrap(), "- [ ] a\r\n- [x] b\r\n");
    }

    use super::*;

    /// A document is one that opens in an app of its own and never runs:
    /// by its extension, any case, never a bare name nor a page, a
    /// script, a note or a sheet that carries macros.
    #[test]
    fn documents_are_named_by_kinds_that_never_run() {
        for ok in ["report.pdf", "attachments/Notes.TXT", "a/b/sums.xlsx", "talk.m4a", "film.mov"] {
            assert!(is_document_name(ok), "{ok}");
        }
        for no in ["page.html", "logo.svg", "run.sh", "setup.exe", "Tool.app", "sums.xlsm", "old.doc", "note.md", "readme", ".pdf", "photo.png"] {
            assert!(!is_document_name(no), "{no}");
        }
    }

    /// A document a note links to is wanted as a picture it shows is:
    /// by a path (escapes read, `<…>`, a title after it), from the
    /// note's folder, or an Obsidian link found by its name. A link to a
    /// page, a note or a program is no document.
    #[test]
    fn the_documents_a_note_links_to_are_in_use() {
        let md = "See [the plan](../attachments/Trip-plan-%28v2%29.pdf \"v2\") and [notes](<../attachments/a b.txt>).\n\
                  ![](../attachments/p.png)\n\
                  And [[talk.m4a]], [[Other note]], [x](https://example.com/a.pdf), [y](run.sh)";
        let files = Files::new(["attachments/talk.m4a"]);
        let found: Vec<String> = document_paths("Trips", md, &files).into_iter().collect();
        assert_eq!(found, ["attachments/Trip-plan-(v2).pdf", "attachments/a b.txt", "attachments/talk.m4a"]);
        let all: Vec<String> = picture_paths("Trips", md, &files).into_iter().collect();
        assert_eq!(all, ["attachments/Trip-plan-(v2).pdf", "attachments/a b.txt", "attachments/p.png", "attachments/talk.m4a"]);
        assert_eq!(percent_decoded("a%20b%2"), "a b%2");
    }

    /// Found by fuzzing: a line led by wide whitespace was cut inside a
    /// letter looking for a quote's `>`.
    #[test]
    fn a_line_led_by_wide_whitespace_reads() {
        let (_, d) = from_markdown("\u{85}\u{85}*\u{85}\u{85}");
        assert_eq!(d.len(), 1);
        let (_, d) = from_markdown("   > quoted\n\u{2003}> not\n");
        assert!(matches!(&d.blocks()[0].kind, Kind::Quote(t) if t == "quoted"));
    }

    /// Found by the property tests: a bullet of dashes read back as a
    /// divider, and a setext heading kept its trailing spaces once.
    #[test]
    fn a_bullet_of_dashes_stays_a_bullet_and_headings_lose_end_spaces() {
        let once = to_markdown(None, &from_markdown("* ---\n").1);
        assert_eq!(once, "- \\---\n");
        let (_, d) = from_markdown(&once);
        assert!(matches!(&d.blocks()[0].kind, Kind::Item { text, .. } if text == "---"));
        let once = to_markdown(None, &from_markdown("Some words\n\nTitle  \n---\n").1);
        assert_eq!(to_markdown(None, &from_markdown(&once).1), once);
        assert!(once.contains("## Title\n"), "{once}");
    }

    /// A note read and written once.
    fn written(s: &str) -> String {
        let (title, d) = from_markdown(s);
        to_markdown(title.as_deref(), &d)
    }

    /// A line's own backslash before a mark is written as it is, so other
    /// apps show it as Carnet does: `\*x` keeps it, and `\# y` reads back
    /// as `# y`, the same words.
    #[test]
    fn a_line_that_starts_with_an_escape_is_written_as_it_is() {
        let md = to_markdown(None, &Doc::new(vec![Block::text("\\*x\n\\# y")]));
        assert_eq!(md, "\\*x\n\\# y\n");
        let (_, back) = from_markdown(&md);
        assert_eq!(back.blocks()[0].text_ref(), Some("\\*x\n# y"));
        assert_eq!(to_markdown(None, &back), md);
    }

    /// Found by the property tests: a lone `\r` stayed in its line, and
    /// with the `\n` written after it read back as one line end.
    #[test]
    fn a_lone_carriage_return_ends_a_line() {
        assert_eq!(written("a\rb"), "a\nb\n");
        assert_eq!(written("*\r"), "- \n");
        let once = written("<mér1. k># \r");
        assert_eq!(written(&once), once);
    }

    /// The finding: pictures one after another on a line were read as
    /// words, `!a|left|200` and all. They are a row of pictures, each with
    /// its side (the left, where none is written); the line is the file's
    /// until the note is edited, then written a picture a line.
    #[test]
    fn pictures_on_one_line_are_a_row() {
        let (_, d) = from_markdown("Above\n\n![a](x.png)![b|left|200](y.png) ![[z.png|120]]\n\nBelow\n");
        let pics: Vec<(String, Option<u32>, Float)> = d
            .blocks()
            .iter()
            .filter_map(|b| match &b.kind {
                Kind::Image { src, width, float, .. } => Some((src.clone(), *width, *float)),
                _ => None,
            })
            .collect();
        assert_eq!(pics, [("x.png".into(), None, Float::Left), ("y.png".into(), Some(200), Float::Left), ("z.png".into(), Some(120), Float::Left)]);
        assert_eq!(kinds(&d).len(), 5);
        let once = to_markdown(None, &d);
        assert_eq!(once, "Above\n\n![a|left](x.png)\n\n![b|left|200](y.png)\n\n![[z.png|left|120]]\n\nBelow\n");
        assert_eq!(written(&once), once);
        // Sides kept as written; sources in brackets read apart.
        let (_, d) = from_markdown("![a|right](<x y.png>)  ![b](<z.png>)\n");
        assert!(matches!(&d.blocks()[0].kind, Kind::Image { src, float: Float::Right, .. } if src == "x y.png"));
        assert!(matches!(&d.blocks()[1].kind, Kind::Image { src, float: Float::Left, .. } if src == "z.png"));
        // Anything else on the line: words, as ever.
        for words in ["see ![a](x.png)![b](y.png)", "![a](x.png)![b](y.png) after", "![a](x.png)"] {
            let (_, d) = from_markdown(words);
            assert!(!d.blocks().iter().all(|b| matches!(b.kind, Kind::Image { .. })) || words == "![a](x.png)", "{words}");
        }
        // Typed so in a paragraph: kept words, written so they read back.
        let d = Doc::new(vec![Block::text("![a](x.png)![b](y.png)")]);
        let md = to_markdown(None, &d);
        assert_eq!(from_markdown(&md).1.blocks()[0].text_ref(), Some("![a](x.png)![b](y.png)"), "{md}");
    }

    /// Found by the property tests: a paragraph of one `\` read as the
    /// writer's blank line, which it writes only inside a paragraph; on
    /// its own it is the backslash it says.
    #[test]
    fn a_line_of_one_backslash_alone_is_a_backslash() {
        let (_, d) = from_markdown("\\\n- ");
        assert_eq!(d.blocks()[0].text_ref(), Some("\\"));
        let once = written("\\\n- ");
        assert_eq!(written(&once), once);
        let (_, d) = from_markdown("a\n\\\nb");
        assert_eq!(d.blocks()[0].text_ref(), Some("a\n\nb"), "inside a paragraph, a blank line");
    }

    /// Found by the property tests: a divider opening a note was written
    /// `---`, which a `---` further down made front matter.
    #[test]
    fn a_divider_on_top_is_not_front_matter() {
        let once = written("----\nwords\n```\ncode\n```\n---\nmore");
        assert!(once.starts_with("----\n"), "{once}");
        assert_eq!(kinds(&from_markdown(&once).1)[0], Kind::Divider);
        assert_eq!(written(&once), once);
    }

    /// Found by the property tests: a heading under a code block was taken
    /// for the title, as if the code were front matter, and moved above it.
    #[test]
    fn a_heading_under_code_is_not_the_title() {
        let (title, d) = from_markdown("```\ncode\n```\n# Late");
        assert_eq!(title, None);
        assert!(matches!(&d.blocks()[1].kind, Kind::Heading { level: 1, text } if text == "Late"));
        let once = written("#\n```\ncode\n```\n# ");
        assert_eq!(written(&once), once);
    }

    /// Found by the property tests: the space before a `>` was cut at three
    /// bytes, through the middle of a wider one (U+2006 is three itself).
    /// Only spaces lead a quote, as in CommonMark: a wide one is text.
    #[test]
    fn a_wide_space_before_a_quote_is_not_cut() {
        assert_eq!(quote_line(" \u{2006}> b"), None);
        assert_eq!(quote_line("\u{2006}> b"), None);
        assert_eq!(quote_line("   > b"), Some("b"));
    }

    /// Found by the property tests: a separator row of empty cells (`|||-`)
    /// gave the table empty alignments, written as a row that did not read
    /// back as a separator.
    #[test]
    fn a_separator_of_empty_cells_keeps_the_plain_alignment() {
        let once = written("| 1 | 2 |\n|||-");
        assert_eq!(once, "| 1 | 2 |\n| --- | --- |\n");
        assert_eq!(written(&once), once);
    }

    /// Kinds only: ids differ between parses by design.
    fn kinds(doc: &Doc) -> Vec<Kind> {
        doc.blocks
            .iter()
            .map(|b| match &b.kind {
                Kind::Table(t) => Kind::Table(Table {
                    header: t.header,
                    cols: vec![],
                    aligns: t.aligns.clone(),
                    rows: t
                        .rows
                        .iter()
                        .map(|r| Row {
                            id: 0,
                            cells: r.cells.clone(),
                        })
                        .collect(),
                }),
                k => k.clone(),
            })
            .collect()
    }

    /// Parse, write, compare bytes.
    fn same(src: &str) {
        let (title, doc) = from_markdown(src);
        assert_eq!(to_markdown(title.as_deref(), &doc), src, "for:\n{src}");
    }

    const SAMPLE: &str = "\
# Groceries

Things for the weekend.
Second line of the same paragraph.

## Produce

- [ ] apples
- [x] pears
  - [ ] the green ones
- bread
1. first
2. second

| item | qty | price |
| --- | :-: | --: |
| milk | 2 | 1.20 |
| a \\| b |  | C:\\x |

---

\\- not a list
";

    #[test]
    fn round_trips() {
        let (title, doc) = from_markdown(SAMPLE);
        assert_eq!(title.as_deref(), Some("Groceries"));
        let out = to_markdown(Some("Groceries"), &doc);
        assert_eq!(out, SAMPLE);
        let (_, again) = from_markdown(&out);
        assert_eq!(kinds(&doc), kinds(&again));
    }

    #[test]
    fn parses_structure() {
        let (_, doc) = from_markdown(SAMPLE);
        let k = kinds(&doc);
        assert!(matches!(&k[0], Kind::Text(t) if t.lines().count() == 2));
        assert!(matches!(&k[1], Kind::Heading { level: 2, .. }));
        assert!(matches!(
            &k[4],
            Kind::Item { style: ListStyle::Todo, indent: 1, done: false, .. }
        ));
        let Kind::Table(t) = &k[8] else { panic!("no table: {k:?}") };
        assert_eq!(t.rows[2].cells, ["a | b", "", "C:\\x"]);
        assert_eq!(t.aligns, ["---", ":-:", "--:"]);
        assert!(matches!(&k[10], Kind::Text(t) if t == "- not a list"));
        // The editor's invariant: a text block to type in at the end.
        assert!(matches!(k.last(), Some(Kind::Text(_))));
    }

    #[test]
    fn text_that_looks_like_markdown_survives() {
        for s in [
            "# not a heading",
            "- not a list",
            "  * nor this",
            "| not a table",
            "1. nope",
            "12) nope",
            "---",
            "```",
            "\\(already escaped\\)",
            "\\ backslash",
            "#tag stays a tag",
            "a\n\nblank line inside",
            // Tab typed in text: a tab, at a line's start too.
            "\tindented\tby a tab",
            "a line\n\tand one under it",
            "\t- a tab, not a list",
            "\t# nor a heading",
        ] {
            let doc = Doc::new(vec![Block::text(s)]);
            let md = to_markdown(Some("t"), &doc);
            let (_, back) = from_markdown(&md);
            assert_eq!(back.blocks()[0].text_ref(), Some(s), "for {s:?} written as {md:?}");
        }
        // Tags and numbers are escaped where CommonMark wants, or not at all.
        let doc = Doc::new(vec![Block::text("#tag\n1. x")]);
        assert_eq!(to_markdown(None, &doc), "#tag\n1\\. x\n");
    }

    #[test]
    fn other_editors_files_come_back_byte_for_byte() {
        same("---\ntags: [a, b]\n---\n\n# Title\n\nBody.\n");
        same("Intro\n\n```rust\n# not a heading\n- not a list\n\nfn main() {}\n```\n\nAfter.\n");
        same("\\frac{a}{b} and \\(x\\)\n");
        same("#### Deep heading\n\nText.\n");
        same("3. three\n4. four\n");
        same("1986. A good year\n");
        same("1. a\n   1. nested under a number\n2. b\n");
        same("- [x] done\n  - child\n");
        same("- \\[ ] a bullet, not a box\n");
        same("| a | b |\n| :-- | --: |\n| `C:\\x` | 2 |\n");
    }

    #[test]
    fn bom_setext_and_a_title_that_is_not_there() {
        let (title, doc) = from_markdown("\u{feff}# T\n\nx\n");
        assert_eq!(title.as_deref(), Some("T"));
        assert_eq!(doc.blocks()[0].text_ref(), Some("x"));
        let (title, doc) = from_markdown("Title\n=====\n\nSub\n---\n\nx\n");
        assert_eq!(title.as_deref(), Some("Title"));
        assert!(matches!(&doc.blocks()[0].kind, Kind::Heading { level: 2, text } if text == "Sub"));
        // A file with no title stays without one.
        let (title, doc) = from_markdown("Just text.\n");
        assert_eq!(title, None);
        assert_eq!(to_markdown(title.as_deref(), &doc), "Just text.\n");
        // An empty title above a level-one heading keeps its place.
        let doc = Doc::new(vec![Block::new(Kind::Heading {
                level: 1,
                text: "Intro".into(),
            })]);
        let md = to_markdown(Some(""), &doc);
        let (title, back) = from_markdown(&md);
        assert_eq!(title.as_deref(), Some(""));
        assert!(matches!(&back.blocks()[0].kind, Kind::Heading { level: 1, .. }));
    }

    #[test]
    fn numbering_restarts_per_list_and_level() {
        let n = |indent, text: &str| Block::new(Kind::item(ListStyle::Number, indent, text));
        let doc = Doc::new(vec![
                n(0, "a"),
                n(1, "a1"),
                n(1, "a2"),
                n(0, "b"),
                Block::text("break"),
                n(0, "c"),
            ]);
        assert_eq!(
            to_markdown(None, &doc),
            "1. a\n   1. a1\n   2. a2\n2. b\n\nbreak\n\n1. c\n"
        );
    }

    #[test]
    fn old_two_space_numbered_nesting_still_nests() {
        let (_, doc) = from_markdown("1. a\n  1. a1\n2. b\n");
        assert!(matches!(doc.blocks()[1].kind, Kind::Item { indent: 1, .. }));
        assert!(matches!(doc.blocks()[2].kind, Kind::Item { indent: 0, .. }));
    }

    #[test]
    fn shortcuts() {
        assert!(matches!(shortcut("- "), Some(Kind::Item { style: ListStyle::Bullet, .. })));
        assert!(matches!(shortcut("[] buy"), Some(Kind::Item { style: ListStyle::Todo, ref text, .. }) if text == "buy"));
        assert!(matches!(shortcut("3. x"), Some(Kind::Item { style: ListStyle::Number, .. })));
        assert!(matches!(shortcut("## "), Some(Kind::Heading { level: 2, .. })));
        assert!(matches!(shortcut("---"), Some(Kind::Divider)));
        assert!(shortcut("```").is_none() && shortcut("/code").is_none() && shortcut("/table").is_none(), "code and tables open on Enter");
        assert!(matches!(fence("/table"), Some(Kind::Table(_))));
        assert_eq!(fence("```"), Some(Kind::Raw("```\n\n```".into())));
        assert_eq!(fence("```rust "), Some(Kind::Raw("```rust\n\n```".into())));
        assert_eq!(fence("/code"), Some(Kind::Raw("```\n\n```".into())));
        assert_eq!(fence("```a`b"), None);
        assert_eq!(fence("``"), None);
        assert!(shortcut("-x").is_none());
        assert!(shortcut("- a\nb").is_none());
    }

    #[test]
    fn sort_keeps_header_and_orders_numbers() {
        let mut t = Table::from_cells(vec![
            vec!["name".into(), "n".into()],
            vec!["b".into(), "10".into()],
            vec!["a".into(), "9".into()],
            vec!["c".into(), "".into()],
        ]);
        assert!(t.sort_by(1, false));
        let names: Vec<_> = t.rows.iter().map(|r| r.cells[0].as_str()).collect();
        assert_eq!(names, ["name", "a", "b", "c"]);
        assert!(t.sort_by(0, true));
        let names: Vec<_> = t.rows.iter().map(|r| r.cells[0].as_str()).collect();
        assert_eq!(names, ["name", "c", "b", "a"]);
    }

    /// A table without a header is written under an empty one, which
    /// GitHub and the others draw as a thin empty row, and reads back as
    /// none; one with only an empty header row keeps it (it has no other).
    #[test]
    fn a_table_without_a_header_round_trips() {
        let mut t = Table::from_cells(vec![vec!["Fri".into(), "30".into()], vec!["Sat".into(), "45".into()]]);
        assert!(t.header);
        t.header = false;
        let md = to_markdown(None, &Doc::new(vec![Block::new(Kind::Table(t))]));
        assert_eq!(md, "| | |\n| --- | --- |\n| Fri | 30 |\n| Sat | 45 |\n");
        let (_, doc) = from_markdown(&md);
        let Kind::Table(t) = &doc.blocks()[0].kind else { panic!() };
        assert!(!t.header);
        assert_eq!(t.rows.iter().map(|r| r.cells[0].as_str()).collect::<Vec<_>>(), ["Fri", "Sat"]);
        assert_eq!(to_markdown(None, &doc), md, "as it was");
        // As others write one: spaces of any count, still empty.
        let (_, doc) = from_markdown("|   |  |\n|:--|--:|\n| a | b |\n");
        let Kind::Table(t) = &doc.blocks()[0].kind else { panic!() };
        assert!(!t.header);
        assert_eq!((t.rows.len(), t.aligns.as_slice()), (1, [":--".to_string(), "--:".to_string()].as_slice()));
        let (_, doc) = from_markdown("| | |\n|---|---|\n");
        let Kind::Table(t) = &doc.blocks()[0].kind else { panic!() };
        assert!(t.header && t.rows.len() == 1, "its only row stays");
    }

    /// The header taken away keeps its words, as the first plain row
    /// under an empty header line; put back, the first row is the header
    /// again. A table without one sorts every row; one of a single row can
    /// lose its header too. Removing the header's row takes the header.
    #[test]
    fn a_header_comes_and_goes() {
        let mut t = Table::from_cells(vec![vec!["name".into()], vec!["b".into()], vec!["a".into()]]);
        assert!(!t.set_header(true), "it has one");
        assert!(t.set_header(false));
        assert_eq!((t.header, t.rows.len()), (false, 3), "no row goes");
        assert!(!t.set_header(false), "it has none");
        let md = to_markdown(None, &Doc::new(vec![Block::new(Kind::Table(t.clone()))]));
        assert_eq!(md, "| |\n| --- |\n| name |\n| b |\n| a |\n", "its words kept, under an empty header line");
        let (_, back) = from_markdown(&md);
        let Kind::Table(b) = &back.blocks()[0].kind else { panic!() };
        assert!(!b.header && b.rows.iter().map(|r| &r.cells).eq(t.rows.iter().map(|r| &r.cells)), "and read back so");
        assert!(t.sort_by(0, false), "every row sorts");
        assert_eq!(t.rows.iter().map(|r| r.cells[0].as_str()).collect::<Vec<_>>(), ["a", "b", "name"]);
        assert!(t.set_header(true));
        assert_eq!((t.header, t.rows.len(), t.rows[0].cells[0].as_str()), (true, 3, "a"), "the first row is the header again");
        assert!(t.remove_row(0));
        assert!(!t.header, "its row removed, no header");
        let mut one = Table::from_cells(vec![vec!["only".into()]]);
        assert!(one.set_header(false), "a table of one row too");
        let md = to_markdown(None, &Doc::new(vec![Block::new(Kind::Table(one))]));
        assert_eq!(md, "| |\n| --- |\n| only |\n");
    }

    #[test]
    fn crlf_and_ragged_tables() {
        let (_, doc) = from_markdown("| a | b |\r\n|---|---|\r\n| 1 |\r\n");
        let Kind::Table(t) = &doc.blocks()[0].kind else { panic!() };
        assert_eq!(t.rows[1].cells, ["1", ""]);
    }

    #[test]
    fn images_on_their_own_line_round_trip() {
        let md = "# Trip\n\nBefore\n\n![the view](attachments/view.png)\n\n![](<My Pictures/a b.jpg>)\n\n![small|320](s.png)\n\n\\![not](x.png)\n\nsee ![inline](x.png) here";
        let (_, doc) = from_markdown(md);
        let kinds: Vec<&Kind> = doc.blocks().iter().map(|b| &b.kind).collect();
        assert_eq!(
            kinds[1],
            &Kind::Image { src: "attachments/view.png".into(), alt: "the view".into(), width: None, float: Float::None, wiki: false }
        );
        assert_eq!(kinds[2], &Kind::Image { src: "My Pictures/a b.jpg".into(), alt: String::new(), width: None, float: Float::None, wiki: false });
        assert_eq!(kinds[3], &Kind::Image { src: "s.png".into(), alt: "small".into(), width: Some(320), float: Float::None, wiki: false });
        assert_eq!(kinds[4], &Kind::Text("![not](x.png)".into()), "an escaped one is text");
        assert!(matches!(kinds[5], Kind::Text(_)), "one inside a paragraph stays text");
        assert_eq!(to_markdown(Some("Trip"), &doc), format!("{md}\n"));
    }

    #[test]
    fn quotes_read_and_write_as_quotes() {
        let md = "# Q\n\n> To be,\n> or not.\n>\n> Hamlet\n\n\\> not a quote\n\nAfter";
        let (_, doc) = from_markdown(md);
        let k = kinds(&doc);
        assert_eq!(k[0], Kind::Quote("To be,\nor not.\n\nHamlet".into()));
        assert_eq!(k[1], Kind::Text("> not a quote".into()), "an escaped one is text");
        assert_eq!(to_markdown(Some("Q"), &doc), format!("{md}\n"));
        assert_eq!(shortcut("> said"), Some(Kind::Quote("said".into())));
        // A paragraph line that starts like a quote is escaped on the way out.
        let doc = Doc::new(vec![Block::text("> x")]);
        assert_eq!(to_markdown(None, &doc), "\\> x\n");
    }

    /// Pictures as exports write them: HTML, and paths with bare spaces.
    /// Written back as Carnet writes a picture.
    #[test]
    fn pictures_as_exports_write_them() {
        let md = "<img src=\"Attachments/a&amp;b.png\" alt=\"the view\" width=\"300\">\n\n![](Attachments/IMG 0412.jpeg)\n\n<img src='x.png'/>\n\n<img src=\"y.png\"> and words\n\n<img alt=\"none\">";
        let (_, doc) = from_markdown(md);
        let k = kinds(&doc);
        assert_eq!(k[0], Kind::Image { src: "Attachments/a&b.png".into(), alt: "the view".into(), width: Some(300), float: Float::None, wiki: false });
        assert_eq!(k[1], Kind::Image { src: "Attachments/IMG 0412.jpeg".into(), alt: String::new(), width: None, float: Float::None, wiki: false });
        assert_eq!(k[2], Kind::Image { src: "x.png".into(), alt: String::new(), width: None, float: Float::None, wiki: false });
        assert!(matches!(k[3], Kind::Text(_)), "one beside words stays as it is");
        assert!(matches!(k[4], Kind::Text(_)), "and one with no source");
        assert_eq!(
            to_markdown(None, &doc),
            "![the view|300](Attachments/a&b.png)\n\n![](<Attachments/IMG 0412.jpeg>)\n\n![](x.png)\n\n<img src=\"y.png\"> and words\n\n<img alt=\"none\">\n"
        );
        // A title still ends the source at its space.
        assert_eq!(image_line("![](a.png \"Title\")"), Some(Kind::Image { src: "a.png".into(), alt: String::new(), width: None, float: Float::None, wiki: false }));
    }

    #[test]
    fn pictures_at_an_edge_round_trip() {
        let md = "![view|right|300](v.png)\n\n![|left](w.png)\n\n![a|b](c.png)";
        let (_, doc) = from_markdown(md);
        let k = kinds(&doc);
        assert_eq!(k[0], Kind::Image { src: "v.png".into(), alt: "view".into(), width: Some(300), float: Float::Right, wiki: false });
        assert_eq!(k[1], Kind::Image { src: "w.png".into(), alt: String::new(), width: None, float: Float::Left, wiki: false });
        assert_eq!(k[2], Kind::Image { src: "c.png".into(), alt: "a|b".into(), width: None, float: Float::None, wiki: false });
        assert_eq!(to_markdown(None, &doc), format!("{md}\n"));
    }

    /// The pictures a note shows, as paths from the notes folder, from
    /// whichever folder it is in; web addresses and paths out are none.
    #[test]
    fn a_notes_pictures_are_found_from_its_folder() {
        use super::{picture_paths, resolve_picture};
        assert_eq!(resolve_picture("", "attachments/a.png").as_deref(), Some("attachments/a.png"));
        assert_eq!(resolve_picture("Trips/Lisbon", "../../attachments/a%20b.png").as_deref(), Some("attachments/a b.png"));
        assert_eq!(resolve_picture("", "../x.png"), None, "out of the notes folder");
        assert_eq!(resolve_picture("", "https://e.com/x.png"), None);
        // A Windows path climbs as one with `/` does, and a drive, a
        // device or a share isn't a path from the notes folder.
        assert_eq!(resolve_picture("", r"attachments\..\..\..\Users\me\x.docx"), None);
        assert_eq!(resolve_picture("Work", r"..\attachments\a.png").as_deref(), Some("attachments/a.png"));
        for src in [r"\\host\share\x.png", r"\x.png", "C:/x.png", r"C:\x.png", "attachments/con.png", "attachments/a:b.png", "attachments/a\0.png"] {
            assert_eq!(resolve_picture("", src), None, "{src:?}");
        }
        let md = "# Trip\n\n![](../attachments/a.png)\n\nText ![not](inline.png) here\n![w|right|300](<../attachments/b c.png>)\n![](https://e.com/x.png)\n";
        let found: Vec<String> = picture_paths("Trips", md, &Files::default()).into_iter().collect();
        assert_eq!(found, ["attachments/a.png", "attachments/b c.png"]);
    }

    /// Obsidian's embeds are pictures, read with their width and alt text,
    /// and written back as they were written.
    #[test]
    fn obsidian_embeds_are_pictures_and_round_trip() {
        let md = "# Trip\n\n![[photo.png]]\n\n![[attachments/photo.png]]\n\n![[photo.png|300]]\n\n![[photo.png|alt text]]\n\n![[My Photo.JPG|the view|right|320]]\n\n![[Other note]]\n\n![[file.pdf]]\n\nsee ![[photo.png]] here\n\n\\![[photo.png]]";
        let (title, doc) = from_markdown(md);
        let k = kinds(&doc);
        let wiki = |src: &str, alt: &str, width, float| Kind::Image { src: src.into(), alt: alt.into(), width, float, wiki: true };
        assert_eq!(k[0], wiki("photo.png", "", None, Float::None));
        assert_eq!(k[1], wiki("attachments/photo.png", "", None, Float::None));
        assert_eq!(k[2], wiki("photo.png", "", Some(300), Float::None));
        assert_eq!(k[3], wiki("photo.png", "alt text", None, Float::None));
        assert_eq!(k[4], wiki("My Photo.JPG", "the view", Some(320), Float::Right));
        assert_eq!(k[5], Kind::Text("![[Other note]]".into()), "a note embedded is not a picture");
        assert_eq!(k[6], Kind::Text("![[file.pdf]]".into()), "nor is a PDF");
        assert!(matches!(&k[7], Kind::Text(t) if t.contains("![[photo.png]]")), "one inside words stays text");
        assert_eq!(k[8], Kind::Text("![[photo.png]]".into()), "an escaped one is text");
        assert_eq!(to_markdown(title.as_deref(), &doc), format!("{md}\n"));
        // A size as width × height keeps the width; the height follows the
        // picture's shape.
        assert_eq!(image_line("![[a.png|300x200]]"), Some(wiki("a.png", "", Some(300), Float::None)));
        // Changed here, it is written as it was: an embed still.
        let mut doc = doc;
        let i = doc.blocks().iter().position(|b| matches!(b.kind, Kind::Image { width: Some(300), .. })).unwrap();
        if let Kind::Image { width, float, .. } = &mut doc.block_mut(i).kind {
            *width = Some(240);
            *float = Float::Left;
        }
        assert!(to_markdown(None, &doc).contains("\n![[photo.png|left|240]]\n"));
        // A text line that would read back as one is kept from it.
        assert_eq!(to_markdown(None, &Doc::new(vec![Block::text("![[x.png]]")])), "\\![[x.png]]\n");
        // Pictures this app puts in are written as Markdown links.
        let doc = Doc::new(vec![Block::new(Kind::Image { src: "attachments/p.png".into(), alt: String::new(), width: None, float: Float::None, wiki: false })]);
        assert_eq!(to_markdown(None, &doc), "![](attachments/p.png)\n");
    }

    /// A picture in a table's cell is the cell's, its `|`s its own: the
    /// table keeps its columns, the cell its text as written, and the
    /// file comes back byte for byte — with a width, at a side (which a
    /// cell keeps but means nothing there), escaped as GFM writes it, or
    /// as Obsidian embeds one.
    #[test]
    fn a_picture_in_a_cell_is_the_cells_and_round_trips() {
        let (_, doc) = from_markdown("| img1 | img2 |\n|---|---|\n| ![|left|260](../attachments/aerial.jpg) | |\n");
        let Kind::Table(t) = &doc.blocks()[0].kind else { panic!("no table") };
        assert_eq!(t.width(), 2);
        assert_eq!(t.rows[1].cells, ["![|left|260](../attachments/aerial.jpg)", ""]);
        same("| img1 | img2 |\n| --- | --- |\n| ![|left|260](../attachments/aerial.jpg) |  |\n");
        same("| a | b |\n| --- | --- |\n| ![view|300](attachments/v.png) and words | x \\| y |\n");
        same("| a |\n| --- |\n| ![view\\|right\\|300](<attachments/a b.png>) |\n");
        same("| a | b |\n| --- | --- |\n| ![[aerial.jpg|260]] | ![[Other note]] |\n");
        let (_, doc) = from_markdown("| a | b |\n| --- | --- |\n| ![[aerial.jpg|260]] | ![[Other note]] |\n");
        let Kind::Table(t) = &doc.blocks()[0].kind else { panic!("no table") };
        assert_eq!(t.rows[1].cells, ["![[aerial.jpg|260]]", "![[Other note]]"], "an embed of a note is no picture, and keeps its column");
        // What a cell's picture says.
        let p = inline_picture("x ![view|left|260](../attachments/aerial.jpg) y", 2).unwrap();
        assert_eq!((p.src.as_str(), p.alt.as_str(), p.width, p.wiki), ("../attachments/aerial.jpg", "view", Some(260), false));
        assert_eq!(&"x ![view|left|260](../attachments/aerial.jpg) y"[p.at.0..p.at.1], "![view|left|260](../attachments/aerial.jpg)");
        let p = inline_picture("![a\\|300](<b c.png>)", 0).unwrap();
        assert_eq!((p.src.as_str(), p.alt.as_str(), p.width), ("b c.png", "a", Some(300)));
        let p = inline_picture("![[photo.png|the view|260]]", 0).unwrap();
        assert_eq!((p.src.as_str(), p.alt.as_str(), p.width, p.wiki), ("photo.png", "the view", Some(260), true));
        assert_eq!(inline_picture("![[Other note]]", 0), None);
        assert_eq!(inline_picture("![a](", 0), None);
        // The pictures a note shows: those in its cells too, not a
        // paragraph's among its words.
        let md = "| a | b |\n| --- | --- |\n| ![](../attachments/a.png) | ![[b.png]] ![](<../attachments/c d.png>) |\n\nText ![not](inline.png) here\n";
        let found: Vec<String> = picture_paths("Trips", md, &Files::new(["attachments/b.png"])).into_iter().collect();
        assert_eq!(found, ["attachments/a.png", "attachments/b.png", "attachments/c d.png"]);
        // A cell's pictures given new sources, the rest as it was; code
        // holds none.
        let cell = "x ![v|260](a.png) `![](b.png)` ![[c.png|9]]";
        let out = relink_cell(cell, |p| Some(format!("new/{}", p.src))).unwrap();
        assert_eq!(out, "x ![v|260](new/a.png) `![](b.png)` ![[new/c.png|9]]");
        assert_eq!(relink_cell("![](a.png)", |_| Some("my pics/a.png".into())).as_deref(), Some("![](<my pics/a.png>)"));
        assert_eq!(relink_cell("![](a.png)", |_| None), None);
    }

    /// An embed's picture is looked for as Obsidian looks: beside the
    /// note, from the top, then by name, the attachments folder first and
    /// the shortest path next.
    #[test]
    fn an_embeds_picture_is_found_as_obsidian_finds_it() {
        use super::{find_picture, picture_paths};
        let files = Files::new(["Trips/here.png", "attachments/a.png", "Old/deep/a.png", "Old/b.png", "Older/x/b.png", "Trips/Lisbon/c.png", "Photo.PNG"]);
        let find = |folder: &str, src: &str| find_picture(folder, src, true, &files);
        assert_eq!(find("Trips", "here.png").as_deref(), Some("Trips/here.png"), "beside the note");
        assert_eq!(find("Trips", "attachments/a.png").as_deref(), Some("attachments/a.png"), "from the top");
        assert_eq!(find("Trips", "a.png").as_deref(), Some("attachments/a.png"), "the attachments folder first");
        assert_eq!(find("", "b.png").as_deref(), Some("Old/b.png"), "then the shortest path");
        assert_eq!(find("", "Lisbon/c.png").as_deref(), Some("Trips/Lisbon/c.png"), "a path's last parts");
        assert_eq!(find("", "photo.png").as_deref(), Some("Photo.PNG"), "case aside, as written on disk");
        assert_eq!(find("Trips", "gone.png").as_deref(), Some("gone.png"), "not found: from the top");
        assert_eq!(find("", "https://e.com/x.png"), None);
        // A Markdown link is where its path says, whatever is elsewhere.
        assert_eq!(find_picture("Trips", "a.png", false, &files).as_deref(), Some("Trips/a.png"));
        let md = "![[a.png]]\n![](here.png)\n![[Lisbon/c.png|200]]\n![[Other note]]\n";
        let found: Vec<String> = picture_paths("Trips", md, &files).into_iter().collect();
        assert_eq!(found, ["Trips/Lisbon/c.png", "Trips/here.png", "attachments/a.png"]);
    }

    #[test]
    fn a_cell_of_many_pictures_or_almost_pictures_is_read_quickly() {
        let n = 60_000;
        let cells = [
            "![[".repeat(n / 3),
            "![[".repeat(n / 3) + "]]",
            "![".repeat(n / 2),
            "![".repeat(n / 2) + "]",
            "![a](".repeat(n / 5),
            "![a](".repeat(n / 5) + ")",
            "![a](<".repeat(n / 6) + ">)",
            "![a](".repeat(n / 10) + &" ".repeat(n / 2) + ")",
            "![a](x \"".repeat(n / 9) + ")",
            "![a](b.png) ".repeat(n / 12),
            "![[b.png|20]] ".repeat(n / 14),
        ];
        for cell in &cells {
            let md = format!("| a |\n| --- |\n| {cell} |\n");
            // An unoptimised build, every way a cell's pictures are read:
            // the note's blocks, the pictures it keeps, the page made of
            // it, and the cell's own words.
            let t0 = std::time::Instant::now();
            let _ = parse_blocks(&md);
            let _ = picture_paths("", &md, &Files::default());
            let _ = crate::export::viewer(&md, |s, _| crate::export::Picture::Url(s.into()));
            let _ = inline::cell_pictures(cell);
            let _cell = inline::cell_text(true);
            let _ = inline::runs(cell);
            let took = t0.elapsed();
            assert!(took.as_millis() < 3000, "{took:?} for {}…", &cell[..40]);
        }
    }
}
