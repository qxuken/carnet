//! Inline styles: `**bold**`, `*italic*`, `~~struck~~`, `` `code` ``,
//! `[links](url)` and colour — `==highlighted==` as Obsidian writes it,
//! `<mark style="background: …">` for another highlighter and
//! `<span style="color: …; background: …">` for coloured words, as HTML any
//! Markdown viewer shows — kept in the text as the Markdown they are. A
//! backslash before a mark makes it a letter (`\*`), as in CommonMark.
//!
//! The page never draws a mark: it draws the words, styled. So every edit
//! here is made in terms of the words. A range is first cut out with
//! [`cut`] — each pair of marks it crosses closed at its edge and opened
//! again past it — so the part between holds only whole pairs; that part
//! is then restyled, copied or taken out, and pairs left touching, `**a**`
//! and `**b**`, are joined again ([`merge`]). The caret sits after a word's
//! last letter and before its closing marks ([`Visible::at`]), so what is
//! typed takes the style of what is before it, as in a word processor.
//!
//! A paragraph's dropped first letter is written as other apps show a
//! plain one, `<span class="initial">T</span>he…`, but is no style of the
//! words: [`take_initial`] reads it off as the paragraph is read, and
//! [`put_initial`] puts it back as it is written (`doc`).

use std::collections::{BTreeMap, BinaryHeap, HashMap};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
    /// A highlighter behind the words, 0xRRGGBBAA; `HIGHLIGHT` for the
    /// plain one (`==…==`), 0 none.
    pub mark: u32,
    /// The words' own colour, 0xRRGGBBAA; 0 none.
    pub ink: u32,
    /// A marker itself: `**`, `` ` ``, `[`, `](url)`, `<mark …>`.
    pub marker: bool,
}

/// The plain highlighter, `==…==` or a `<mark>` with no colour: the
/// theme's own yellow wash. (A real colour always has alpha, so is never 1.)
pub const HIGHLIGHT: u32 = 1;
/// The plain highlighter spelled out, where it shares a tag with a colour.
const HIGHLIGHT_VALUE: u32 = 0xFFF3A3A6;

/// What a pair of marks does to the words between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bold,
    Italic,
    Strike,
    Code,
    Link,
    Tint,
}

impl Kind {
    /// The marks a style is written with.
    pub fn marker(self) -> &'static str {
        match self {
            Kind::Bold => "**",
            Kind::Italic => "*",
            Kind::Strike => "~~",
            Kind::Code => "`",
            Kind::Link | Kind::Tint => "",
        }
    }

    /// Whether words of style s have it.
    pub fn on(self, s: &Style) -> bool {
        match self {
            Kind::Bold => s.bold,
            Kind::Italic => s.italic,
            Kind::Strike => s.strike,
            Kind::Code => s.code,
            Kind::Link => s.link,
            Kind::Tint => s.ink != 0 || s.mark != 0,
        }
    }
}

/// A pair of marks: what it does, and where its opening and closing marks
/// are. The words are between `open.1` and `close.0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub kind: Kind,
    pub open: (usize, usize),
    pub close: (usize, usize),
}

/// A coloured run: the words' colour and the highlighter's (either may be
/// missing), and where its marks are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tint {
    pub ink: Option<u32>,
    pub mark: Option<u32>,
    pub open: (usize, usize),
    pub close: (usize, usize),
}

struct Read {
    styles: Vec<Style>,
    tints: Vec<Tint>,
    pairs: Vec<Pair>,
}

/// The line as runs of one style each: `(start, end, style)`, in order,
/// covering every byte.
pub fn runs(text: &str) -> Vec<(usize, usize, Style)> {
    let styles = styles(text);
    let mut out: Vec<(usize, usize, Style)> = Vec::new();
    for (i, s) in styles.iter().enumerate() {
        match out.last_mut() {
            Some(last) if last.2 == *s => last.1 = i + 1,
            _ => out.push((i, i + 1, *s)),
        }
    }
    out
}

/// The style of every byte of `text`.
pub fn styles(text: &str) -> Vec<Style> {
    read(text).styles
}

/// Every coloured run of `text`, in order.
pub fn tints(text: &str) -> Vec<Tint> {
    read(text).tints
}

/// Every pair of marks in `text`, in no particular order.
pub fn pairs(text: &str) -> Vec<Pair> {
    read(text).pairs
}

fn read(text: &str) -> Read {
    read_counting(text, Count::Whole)
}

/// How the rule of three counts a run of stars: as written, as CommonMark
/// does; or by what is left of it once it has closed a pair, which reads
/// a run where pairs meet (`**b*****old**`) as the pieces put together
/// meant it — what `join` checks its joints against.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Count {
    Whole,
    Left,
}

fn read_counting(text: &str, count: Count) -> Read {
    let b = text.as_bytes();
    let mut st = vec![Style::default(); b.len()];
    let mut pairs: Vec<Pair> = Vec::new();
    // Bytes a later pass leaves alone: code, and a link's address.
    let mut quiet = vec![false; b.len()];

    // Code spans: a run of backticks, closed by a run as long; and,
    // outside them, a backslash before a mark (any ASCII punctuation, as
    // CommonMark has it): the backslash a marker, the mark a letter.
    // Every whole run is listed once, by length, so a run's closer is
    // looked up rather than searched for along the rest of the line.
    let mut ticks: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut j = 0;
    while j < b.len() {
        if b[j] == b'`' {
            let n = run_len(b, j, b'`');
            ticks.entry(n).or_default().push(j);
            j += n;
        } else {
            j += 1;
        }
    }
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && b.get(i + 1).is_some_and(u8::is_ascii_punctuation) {
            st[i].marker = true;
            quiet[i] = true;
            quiet[i + 1] = true;
            i += 2;
            continue;
        }
        if b[i] != b'`' {
            i += 1;
            continue;
        }
        let n = run_len(b, i, b'`');
        let close = ticks.get(&n).and_then(|at| at.get(at.partition_point(|&j| j < i + n)).copied());
        match close {
            Some(j) => {
                // A space inside each end, with more than spaces between,
                // is the marks' (CommonMark's): ``` `` `a ``` holds `` `a ``.
                let pad = b[i + n] == b' ' && b[j - 1] == b' ' && b[i + n..j].iter().any(|&c| c != b' ');
                let (o, c) = if pad { (i + n + 1, j - 1) } else { (i + n, j) };
                for k in i..j + n {
                    quiet[k] = true;
                    if k < o || k >= c {
                        st[k].marker = true;
                    } else {
                        st[k].code = true;
                    }
                }
                pairs.push(Pair { kind: Kind::Code, open: (i, o), close: (c, j + n) });
                i = j + n;
            }
            None => i += n,
        }
    }

    // Links: `[text](url)`, the text to read and the rest marks.
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'[' || quiet[i] {
            i += 1;
            continue;
        }
        let close = (i + 1..b.len()).find(|&j| b[j] == b']' && !quiet[j]);
        let Some(j) = close else { break };
        if b.get(j + 1) != Some(&b'(') {
            i = j + 1;
            continue;
        }
        let Some(k) = (j + 2..b.len()).find(|&k| b[k] == b')' && !quiet[k]) else {
            break;
        };
        st[i].marker = true;
        quiet[i] = true;
        for s in &mut st[i + 1..j] {
            s.link = true;
        }
        for x in j..=k {
            st[x].marker = true;
            quiet[x] = true;
        }
        pairs.push(Pair { kind: Kind::Link, open: (i, i + 1), close: (j, k + 1) });
        i = k + 1;
    }

    // Colour: `<mark …>…</mark>` and `<span style="…">…</span>`, nested
    // as HTML nests; then `==…==`.
    // A tag is checked against what is quiet in a count kept beside the
    // line, and against the closing tags already taken, so a long tag
    // that fails is not walked again for every `<` inside it.
    let mut found: Vec<Tint> = Vec::new();
    let tags = Tags::of(text);
    let mut was_quiet = vec![0usize; b.len() + 1];
    for k in 0..b.len() {
        was_quiet[k + 1] = was_quiet[k] + usize::from(quiet[k]);
    }
    let any_quiet = |(x, y): (usize, usize)| was_quiet[y] > was_quiet[x];
    let mut taken: BTreeMap<usize, usize> = BTreeMap::new();
    let any_taken = |taken: &BTreeMap<usize, usize>, (x, y): (usize, usize)| taken.range(..y).next_back().is_some_and(|(_, &e)| e > x);
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' || quiet[i] {
            i += 1;
            continue;
        }
        let Some((open_end, close)) = tags.closed(text, i) else {
            i += 1;
            continue;
        };
        let open = (i, open_end);
        if any_quiet(open) || any_quiet(close) || any_taken(&taken, open) || any_taken(&taken, close) {
            i += 1;
            continue;
        }
        for k in (i..open_end).chain(close.0..close.1) {
            st[k].marker = true;
            quiet[k] = true;
        }
        taken.insert(close.0, close.1);
        let (ink, mark) = tag_colors(&text[i..open_end]);
        found.push(Tint { ink, mark, open, close });
        i = open_end;
    }
    let mut open: Option<usize> = None;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'=' || quiet[i] {
            i += 1;
            continue;
        }
        let len = run_len(b, i, b'=');
        if len == 2 {
            let before = text[..i].chars().next_back();
            let after = text[i + 2..].chars().next();
            let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
            match open {
                Some(o) if !space(before) => {
                    for k in (o..o + 2).chain(i..i + 2) {
                        st[k].marker = true;
                        quiet[k] = true;
                    }
                    found.push(Tint { ink: None, mark: Some(HIGHLIGHT), open: (o, o + 2), close: (i, i + 2) });
                    open = None;
                }
                _ if !space(after) => open = Some(i),
                _ => {}
            }
        }
        i += len;
    }
    // The run opened last wins, so an inner one's colour shows over an
    // outer one's. Walked once along the line, the runs open at each
    // byte kept by where they open: runs nested deep are not painted
    // over each other, once per level.
    found.sort_by_key(|t| t.open.0);
    for (which, colour) in [(0, (|t: &Tint| t.ink) as fn(&Tint) -> Option<u32>), (1, |t: &Tint| t.mark)] {
        let mut by_start: Vec<_> = found.iter().filter_map(|t| colour(t).map(|c| (t.open.1, t.open.0, t.close.0, c))).collect();
        by_start.sort_unstable();
        let mut next = by_start.into_iter().peekable();
        let mut open: BinaryHeap<(usize, usize, u32)> = BinaryHeap::new();
        for (k, s) in st.iter_mut().enumerate() {
            while let Some(&(_, o, e, c)) = next.peek().filter(|t| t.0 <= k) {
                open.push((o, e, c));
                next.next();
            }
            while open.peek().is_some_and(|&(_, e, _)| e <= k) {
                open.pop();
            }
            if let Some(&(_, _, c)) = open.peek() {
                if which == 0 {
                    s.ink = c;
                } else {
                    s.mark = c;
                }
            }
        }
    }
    for t in &found {
        pairs.push(Pair { kind: Kind::Tint, open: t.open, close: t.close });
    }

    // Emphasis: delimiter runs of `*`, `_` and `~~`, matched closer to the
    // nearest opener of the same character, as CommonMark does it in the
    // common cases. A run opens only before a word and closes only after
    // one, and one between a mark and a word only on the word's side:
    // the `*` of `~~*b*` opens and cannot close. Marks count as such, as
    // CommonMark's punctuation: any of ASCII's (other scripts' punctuation
    // and symbols, which CommonMark counts too, are letters here), and the
    // marks of code, links and colour. And a run that could open or close is not paired
    // with one whose length makes a multiple of three with its own
    // (CommonMark's rule of three): the `**` of `*w**o**rd` pair with each
    // other, and the lone `*` stays a star.
    struct Run {
        at: usize,
        ch: u8,
        len: usize,
        /// The run's length as written, before any of it was paired.
        whole: usize,
        both: bool,
    }
    let punct: Vec<bool> = (0..b.len()).map(|k| b[k].is_ascii_punctuation() || st[k].marker).collect();
    let mut stack: Vec<Run> = Vec::new();
    let mut floor = [0usize; 18];
    let mut i = 0;
    while i < b.len() {
        let ch = b[i];
        if quiet[i] || !matches!(ch, b'*' | b'_' | b'~') {
            i += 1;
            continue;
        }
        let len = run_len(b, i, ch);
        if ch == b'~' && len < 2 {
            i += len;
            continue;
        }
        let before = text[..i].chars().next_back();
        let after = text[i + len..].chars().next();
        let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
        let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        let mark_before = i > 0 && punct[i - 1];
        let mark_after = i + len < b.len() && punct[i + len];
        let mut can_open = !space(after) && (!mark_after || space(before) || mark_before);
        let mut can_close = !space(before) && (!mark_before || space(after) || mark_after);
        if ch == b'_' {
            // Inside a word an underscore is just an underscore.
            can_open &= !word(before);
            can_close &= !word(after);
        }
        let both = can_open && can_close;
        let threes = |r: &Run, left: usize| {
            let (o, c) = if count == Count::Whole { (r.whole, len) } else { (r.len, left) };
            ch != b'~' && (r.both || both) && (o + c).is_multiple_of(3) && !(o.is_multiple_of(3) && c.is_multiple_of(3))
        };
        let c_len = |left: usize| if count == Count::Whole { len } else { left };
        let mut left = len;
        let mut at = i;
        if can_close {
            while left > 0 {
                // Below `floor`, nothing closes a run like this one: a
                // search that found nothing is not made again past what
                // it read (CommonMark's `openers_bottom`).
                let key = (ch == b'_') as usize * 6 + (ch == b'~') as usize * 12 + both as usize * 3 + c_len(left) % 3;
                let lo = floor[key].min(stack.len());
                let Some(pos) = stack[lo..].iter().rposition(|r| r.ch == ch && !threes(r, left)).map(|p| p + lo) else {
                    floor[key] = stack.len();
                    break;
                };
                for f in &mut floor {
                    *f = (*f).min(pos);
                }
                let opener = &mut stack[pos];
                let mut used = if opener.len >= 2 && left >= 2 { 2 } else { 1 };
                if ch == b'~' {
                    if opener.len < 2 || left < 2 {
                        break;
                    }
                    used = 2;
                }
                let o_end = opener.at + opener.len;
                let (o_start, c_start) = (o_end - used, at);
                for s in &mut st[o_start..o_end] {
                    s.marker = true;
                }
                for s in &mut st[c_start..c_start + used] {
                    s.marker = true;
                }
                let kind = match (ch, used) {
                    (b'~', _) => Kind::Strike,
                    (_, 2) => Kind::Bold,
                    _ => Kind::Italic,
                };
                for s in &mut st[o_end..c_start] {
                    match kind {
                        Kind::Strike => s.strike = true,
                        Kind::Bold => s.bold = true,
                        _ => s.italic = true,
                    }
                }
                pairs.push(Pair { kind, open: (o_start, o_end), close: (c_start, c_start + used) });
                opener.len -= used;
                left -= used;
                at += used;
                // Openers between are closed by nothing now.
                stack.truncate(pos + 1);
                if stack[pos].len == 0 {
                    stack.pop();
                }
            }
        }
        if left > 0 && can_open {
            stack.push(Run { at, ch, len: left, whole: len, both });
        }
        i += len;
    }
    // A marker's own byte is only ever a marker.
    for s in &mut st {
        if s.marker {
            *s = Style {
                marker: true,
                ..Style::default()
            };
        }
    }
    Read { styles: st, tints: found, pairs }
}

/// Where a line's colour tags open and close: every `<mark`, `</mark>`,
/// `<span` and `</span>`, and every `>`, found once, so each opening tag
/// finds the tag that closes it without reading the line again.
struct Tags {
    gts: Vec<usize>,
    mark: Nesting,
    span: Nesting,
}

/// One tag name's opening and closing tags, in order, and for each place
/// among them the first later one where more have closed than opened.
struct Nesting {
    /// Where each tag starts, and whether it closes.
    at: Vec<(usize, bool)>,
    /// `up[t]`: the first `u > t` where, of the tags before `u`, closing
    /// ones less opening ones is more than of those before `t`.
    up: Vec<Option<usize>>,
}

impl Nesting {
    fn of(text: &str, name: &str) -> Nesting {
        let (opener, closer) = (format!("<{name}"), format!("</{name}>"));
        let mut at: Vec<(usize, bool)> = text.match_indices(&opener).map(|(k, _)| (k, false)).collect();
        at.extend(text.match_indices(&closer).map(|(k, _)| (k, true)));
        at.sort_unstable();
        // Closing tags less opening ones before each place, and for each
        // place the next that is higher: a stack walked from the end.
        let mut level = vec![0isize; at.len() + 1];
        for (t, &(_, closes)) in at.iter().enumerate() {
            level[t + 1] = level[t] + if closes { 1 } else { -1 };
        }
        let mut up = vec![None; at.len() + 1];
        let mut higher: Vec<usize> = Vec::new();
        for t in (0..level.len()).rev() {
            while higher.last().is_some_and(|&u| level[u] <= level[t]) {
                higher.pop();
            }
            up[t] = higher.last().copied();
            higher.push(t);
        }
        Nesting { at, up }
    }

    /// The closing tag of one opened before `from`, nesting counted from
    /// there: the first that closes more than have opened since.
    fn close(&self, from: usize) -> Option<usize> {
        let t = self.at.partition_point(|&(k, _)| k < from);
        self.up[t].map(|u| self.at[u - 1].0)
    }
}

impl Tags {
    fn of(text: &str) -> Tags {
        let gts = if text.contains('<') { text.match_indices('>').map(|(k, _)| k).collect() } else { Vec::new() };
        Tags { gts, mark: Nesting::of(text, "mark"), span: Nesting::of(text, "span") }
    }

    /// An HTML colour tag opening at `at` — `<mark>`, `<mark style="…">`
    /// or `<span style="…">` — and the tag that closes it, nesting
    /// counted: where the opening tag ends, and where the closing one is.
    fn closed(&self, text: &str, at: usize) -> Option<(usize, (usize, usize))> {
        let (name, nesting) = match self.mark.at.binary_search(&(at, false)) {
            Ok(_) => ("mark", &self.mark),
            Err(_) if self.span.at.binary_search(&(at, false)).is_ok() => ("span", &self.span),
            Err(_) => return None,
        };
        let gt = *self.gts.get(self.gts.partition_point(|&k| k < at))?;
        // Past the name, a space or the tag's end: `<marker>` is no mark.
        let after = at + 1 + name.len();
        if gt != after && !text[after..].starts_with(char::is_whitespace) {
            return None;
        }
        let close = nesting.close(gt + 1)?;
        Some((gt + 1, (close, close + name.len() + 3)))
    }
}

/// The words' colour and the highlighter's that an opening colour tag's
/// style gives; a `<mark>` with none is the plain highlighter.
fn tag_colors(tag: &str) -> (Option<u32>, Option<u32>) {
    let attrs = &tag[5..tag.len() - 1];
    let (mut ink, mut mark) = (None, None);
    for (prop, value) in style_props(attrs) {
        match prop.as_str() {
            "color" => ink = parse_color(&value),
            "background" | "background-color" => mark = parse_color(&value),
            _ => {}
        }
    }
    if tag.starts_with("<mark") && mark.is_none() {
        mark = Some(HIGHLIGHT);
    }
    (ink, mark)
}

/// Whether a tag's `class` attribute names `initial`.
fn has_initial_class(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    let Some(k) = lower.find("class") else { return false };
    let after = lower[k + 5..].trim_start();
    let Some(after) = after.strip_prefix('=') else { return false };
    let after = after.trim_start();
    let Some(q) = after.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
        return false;
    };
    let body = &after[1..];
    body[..body.find(q).unwrap_or(body.len())].split_whitespace().any(|w| w == "initial")
}

/// The `prop: value` pairs of a tag's `style` attribute, props lowercased.
fn style_props(attrs: &str) -> Vec<(String, String)> {
    let lower = attrs.to_ascii_lowercase();
    let Some(s) = lower.find("style") else { return Vec::new() };
    let after = attrs[s + 5..].trim_start();
    let Some(after) = after.strip_prefix('=') else { return Vec::new() };
    let after = after.trim_start();
    let Some(q) = after.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
        return Vec::new();
    };
    let body = &after[1..];
    let body = &body[..body.find(q).unwrap_or(body.len())];
    body.split(';')
        .filter_map(|d| d.split_once(':'))
        .map(|(p, v)| (p.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect()
}

/// A CSS colour: `#rgb`, `#rrggbb`, `#rrggbbaa`, or a common name.
pub fn parse_color(v: &str) -> Option<u32> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(h) = v.strip_prefix('#') {
        let n = u32::from_str_radix(h, 16).ok()?;
        return match h.len() {
            3 => {
                let x = |s: u32| ((n >> s) & 0xf) * 0x11;
                Some((x(8) << 24) | (x(4) << 16) | (x(0) << 8) | 0xff)
            }
            6 => Some((n << 8) | 0xff),
            8 => Some(n),
            _ => None,
        };
    }
    let rgb = match v.as_str() {
        "black" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "orange" => 0xffa500,
        "yellow" => 0xffff00,
        "green" => 0x008000,
        "lime" => 0x00ff00,
        "blue" => 0x0000ff,
        "navy" => 0x000080,
        "purple" => 0x800080,
        "pink" => 0xffc0cb,
        "brown" => 0xa52a2a,
        "gray" | "grey" => 0x808080,
        "teal" => 0x008080,
        "cyan" | "aqua" => 0x00ffff,
        "magenta" | "fuchsia" => 0xff00ff,
        _ => return None,
    };
    Some((rgb << 8) | 0xff)
}

/// A colour as CSS writes it: `#RRGGBB`, with alpha only when it has some.
pub fn css_color(c: u32) -> String {
    if c & 0xff == 0xff { format!("#{:06X}", c >> 8) } else { format!("#{c:08X}") }
}

/// The marks a run of these colours is written with; none for no colour.
/// A highlighter alone is `==` (the plain one) or a `<mark>`; a colour, a
/// `<span>`, which also carries a highlighter beside it.
pub fn tint_tags(ink: Option<u32>, mark: Option<u32>) -> Option<(String, String)> {
    let mark_value = |m: u32| css_color(if m == HIGHLIGHT { HIGHLIGHT_VALUE } else { m });
    Some(match (ink, mark) {
        (None, None) => return None,
        (None, Some(HIGHLIGHT)) => ("==".into(), "==".into()),
        (None, Some(m)) => (format!("<mark style=\"background: {}\">", mark_value(m)), "</mark>".into()),
        (Some(i), None) => (format!("<span style=\"color: {}\">", css_color(i)), "</span>".into()),
        (Some(i), Some(m)) => (
            format!("<span style=\"color: {}; background: {}\">", css_color(i), mark_value(m)),
            "</span>".into(),
        ),
    })
}

/// The marks for these colours around `words`: the plain highlighter's
/// `==` only where it pairs, and a `<mark>` where it would not — around
/// nothing, or words that start or end with a space.
fn tags_around(ink: Option<u32>, mark: Option<u32>, words: &str) -> (String, String) {
    match tint_tags(ink, mark) {
        Some((o, _)) if o == "==" && (words.trim() != words || words.is_empty()) => ("<mark>".into(), "</mark>".into()),
        tags => tags.unwrap_or_default(),
    }
}

/// The marks of a code span around `words`: a run of backticks as long
/// as none in them, and a space inside each end where they start or end
/// with a backtick, or with a space at both (which the reader takes away).
fn code_marks(words: &str) -> (String, String) {
    let b = words.as_bytes();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let n = if b[i] == b'`' { run_len(b, i, b'`') } else { 1 };
        if b[i] == b'`' {
            runs.push(n);
        }
        i += n;
    }
    let ticks = "`".repeat((1..).find(|n| !runs.contains(n)).unwrap_or(1));
    let spaced = words.starts_with(' ') && words.ends_with(' ') && words.bytes().any(|c| c != b' ');
    if words.starts_with('`') || words.ends_with('`') || spaced {
        (format!("{ticks} "), format!(" {ticks}"))
    } else {
        (ticks.clone(), ticks)
    }
}

fn run_len(b: &[u8], i: usize, ch: u8) -> usize {
    b[i..].iter().take_while(|&&c| c == ch).count()
}

/// The line as it reads, markers left out: what a preview shows.
pub fn plain(text: &str) -> String {
    runs(text)
        .into_iter()
        .filter(|r| !r.2.marker)
        .map(|(a, b, _)| &text[a..b])
        .collect()
}

// ------------------------------------------------------------ the first letter

/// What may stand before a dropped first letter and drop with it: an
/// opening quote or bracket, as kui's `TextStyle::initial_letter` takes.
const OPENERS: [char; 13] = [
    '"', '\'', '(', '[', '{', '¿', '¡', '\u{201c}', '\u{2018}', '\u{201e}', '\u{201a}', '\u{ab}', '\u{2039}',
];

/// The first letter of a paragraph's words: where it starts with any
/// openers before it, where the letter itself starts, and where it ends,
/// in the source, marks left out. None where the words start with no
/// letter or digit, or in code.
fn initial_parts(text: &str) -> Option<(usize, usize, usize)> {
    let v = Visible::of(text);
    let words = v.text(text);
    let mut chars = words.char_indices().skip_while(|(_, c)| c.is_whitespace()).peekable();
    let (start, _) = *chars.peek()?;
    let (at, ch) = chars.find(|(_, c)| !OPENERS.contains(c))?;
    if !ch.is_alphanumeric() {
        return None;
    }
    let (s, l) = (v.source(start), v.source(at));
    let e = l + ch.len_utf8();
    let st = styles(text);
    if st[l..e].iter().any(|s| s.code) {
        return None;
    }
    Some((s, l, e))
}

/// The first letter of a paragraph's words, with any opening quote or
/// bracket before it: its range in the source, marks left out — the
/// letter that drops.
pub fn initial_letter(text: &str) -> Option<(usize, usize)> {
    initial_parts(text).map(|(s, _, e)| (s, e))
}

/// A paragraph's words with its first letter marked to drop, as the file
/// keeps it: `<span class="initial">T</span>he…`, which any Markdown
/// viewer shows as a plain T. A letter given a colour of its own carries
/// the class in its colour's tag: `<span class="initial" style="color:
/// #C0553F">T</span>`. As it was where there is no letter to drop.
pub fn put_initial(text: &str) -> String {
    let Some((s, l, e)) = initial_parts(text) else { return text.to_string() };
    let st = styles(text);
    // The openers drop with the letter, inside the tag, unless a mark
    // stands between them: then the letter alone.
    let s = if (s..e).any(|i| st[i].marker) { l } else { s };
    let own = tints(text)
        .into_iter()
        .find(|t| t.open.1 == s && t.close.0 == e && text[t.open.0..t.open.1].starts_with("<span"));
    match own {
        Some(t) => format!("{}<span class=\"initial\"{}", &text[..t.open.0], &text[t.open.0 + 5..]),
        None => format!("{}<span class=\"initial\">{}</span>{}", &text[..s], &text[s..e], &text[e..]),
    }
}

/// A paragraph's words with the mark of a dropped first letter taken off,
/// a colour it had kept: what `put_initial` writes, read back. None when
/// there is no such mark at the paragraph's start.
pub fn take_initial(text: &str) -> Option<String> {
    let t = tints(text)
        .into_iter()
        .find(|t| text[t.open.0..t.open.1].starts_with("<span") && has_initial_class(&text[t.open.0..t.open.1]))?;
    let v = Visible::of(text);
    let words = v.text(text);
    let (a, b) = (v.drawn(t.open.1), v.drawn(t.close.0));
    let lead = words.get(..a)?;
    if a >= b || !lead.chars().all(|c| c.is_whitespace() || OPENERS.contains(&c)) {
        return None;
    }
    let (o, c) = tags_around(t.ink, t.mark, &text[t.open.1..t.close.0]);
    let mut out = text.to_string();
    out.replace_range(t.close.0..t.close.1, &c);
    out.replace_range(t.open.0..t.open.1, &o);
    Some(out)
}

// ------------------------------------------------------------ the words

/// The words of a line without its marks — what the page draws — and how
/// a place in one is a place in the other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visible {
    /// The source's length.
    pub len: usize,
    /// The source's ranges that are words, in order.
    pub runs: Vec<(usize, usize)>,
}

impl Visible {
    pub fn of(text: &str) -> Self {
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for (a, b, s) in self::runs(text) {
            if s.marker {
                continue;
            }
            match runs.last_mut() {
                Some(last) if last.1 == a => last.1 = b,
                _ => runs.push((a, b)),
            }
        }
        Self { len: text.len(), runs }
    }

    /// Every byte a word: code, kept as written.
    pub fn all(text: &str) -> Self {
        let runs = if text.is_empty() { Vec::new() } else { vec![(0, text.len())] };
        Self { len: text.len(), runs }
    }


    /// How long the words are.
    pub fn drawn_len(&self) -> usize {
        self.runs.iter().map(|(a, b)| b - a).sum()
    }

    /// The words alone.
    pub fn text(&self, source: &str) -> String {
        self.runs.iter().map(|&(a, b)| &source[a..b]).collect()
    }

    /// Where source byte `b` is in the words: past the marks before it.
    pub fn drawn(&self, b: usize) -> usize {
        let mut out = 0;
        for &(f, to) in &self.runs {
            if b <= f {
                break;
            }
            out += b.min(to) - f;
        }
        out
    }

    /// The source byte of word byte `d` itself (the end, past them all).
    pub fn source(&self, d: usize) -> usize {
        let mut left = d;
        for &(f, to) in &self.runs {
            if left < to - f {
                return f + left;
            }
            left -= to - f;
        }
        self.len
    }

    /// The source ranges that hold words `a..b`, one per stretch of
    /// words between marks: each can be wrapped in a new pair of tags
    /// that nests inside whatever marks are around it.
    pub fn spans(&self, a: usize, b: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut d = 0;
        for &(f, to) in &self.runs {
            let (lo, hi) = (a.max(d), b.min(d + to - f));
            if lo < hi {
                out.push((f + lo - d, f + hi - d));
            }
            d += to - f;
        }
        out
    }

    /// Where the caret goes for place `d` in the words: right after the
    /// letter before it, inside that letter's marks, so typing carries its
    /// style on; at the very start, before the first letter, inside its.
    pub fn at(&self, d: usize) -> usize {
        if d == 0 {
            return self.runs.first().map_or(0, |r| r.0);
        }
        let mut left = d;
        for &(f, to) in &self.runs {
            if left <= to - f {
                return f + left;
            }
            left -= to - f;
        }
        self.len
    }
}

/// `a..b` held in from the marks at its ends, so it starts and ends on a
/// word's letters.
pub fn tighten(text: &str, a: usize, b: usize) -> (usize, usize) {
    let st = styles(text);
    let (mut a, mut b) = (a.min(b), a.max(b).min(text.len()));
    while a < b && st[a].marker {
        a += 1;
    }
    while b > a && st[b - 1].marker {
        b -= 1;
    }
    (a, b)
}

// ------------------------------------------------------------ edits

/// The line cut at `a` and `b` (`a <= b`): every pair of marks crossing
/// either is closed there and opened again past it, and a pair that would
/// be left holding no words is dropped, so what lies between holds whole
/// pairs only. Returns the new text and where `a` and `b` are in it.
pub fn cut(text: &str, a: usize, b: usize) -> (String, usize, usize) {
    cut_keeping(text, a, b, None)
}

/// `cut`, but a pair holding all of `a..b` is left whole unless it is of
/// `keep`'s kind — the one being changed: italic put on inside a bold run
/// nests in it, rather than cutting it in three.
fn cut_keeping(text: &str, a: usize, b: usize, keep: Option<Kind>) -> (String, usize, usize) {
    let r = read(text);
    let len = text.len();
    let (a, b) = (a.min(len), b.min(len));
    let words = |s: usize, e: usize| (s..e).any(|i| !r.styles[i].marker);
    let mut drop = vec![false; len];
    // Marks put in at a place: closing ones before the cut there, opening
    // ones after it; each with its pair's start, for the nesting order.
    let mut closing: Vec<Vec<(usize, String)>> = vec![Vec::new(); len + 1];
    let mut opening: Vec<Vec<(usize, String)>> = vec![Vec::new(); len + 1];
    for p in &r.pairs {
        let (o0, o1) = p.open;
        let (c0, c1) = p.close;
        let crosses = |x: usize| o1 <= x && x <= c0;
        if !crosses(a) && !crosses(b) {
            continue;
        }
        // Code is never kept whole around a change: its words are literal,
        // so a style goes outside it, not as marks inside.
        if keep.is_some_and(|k| k != p.kind) && p.kind != Kind::Code && o1 <= a && b <= c0 {
            continue;
        }
        let mut at = vec![o1, c0];
        at.extend([a, b].into_iter().filter(|&x| crosses(x)));
        at.sort_unstable();
        at.dedup();
        drop[o0..o1].fill(true);
        drop[c0..c1].fill(true);
        // Emphasis written with `*`, `_`, `~` or `=` opens only before a
        // word and closes only after one: a piece's marks go on its
        // words, the spaces at its ends left outside, and a piece of
        // spaces alone gets none. They also go past the marks of pairs
        // ending where the piece starts (and before those of pairs
        // starting where it ends): `*b* **c**`, not `*b*** c**`, whose
        // run of stars reads as other pairs. (Tags and code take spaces
        // and neighbours as they are.)
        let delimited = text[o0..o1].starts_with(['*', '_', '~', '=']);
        let space = |i: usize| !r.styles[i].marker && text.as_bytes()[i].is_ascii_whitespace();
        let closer = |i: usize| r.pairs.iter().any(|q| q.close.0 <= i && i < q.close.1);
        let opener = |i: usize| r.pairs.iter().any(|q| q.open.0 <= i && i < q.open.1);
        for w in at.windows(2) {
            let (mut s, mut e) = (w[0], w[1]);
            if delimited {
                while s < e && (space(s) || closer(s)) {
                    s += 1;
                }
                while e > s && (space(e - 1) || opener(e - 1)) {
                    e -= 1;
                }
            }
            if s < e && words(s, e) {
                // A piece of code takes the marks its own words need.
                let (o, c) = if p.kind == Kind::Code {
                    code_marks(&text[s..e])
                } else {
                    (text[o0..o1].to_string(), text[c0..c1].to_string())
                };
                opening[s].push((o0, o));
                closing[e].push((o0, c));
            }
        }
    }
    let src = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(len + 16);
    let (mut na, mut nb) = (0, 0);
    for i in 0..=len {
        // Inner pairs close first; outer ones open first.
        closing[i].sort_by_key(|&(o, _)| std::cmp::Reverse(o));
        for (_, m) in &closing[i] {
            out.extend_from_slice(m.as_bytes());
        }
        if i == a {
            na = out.len();
        }
        if i == b {
            nb = out.len();
        }
        opening[i].sort_by_key(|&(o, _)| o);
        for (_, m) in &opening[i] {
            out.extend_from_slice(m.as_bytes());
        }
        if i < len && !drop[i] {
            out.push(src[i]);
        }
    }
    (String::from_utf8(out).unwrap_or_else(|_| text.to_string()), na, nb)
}

/// The line split at `at`, each side whole: what Enter makes of it.
pub fn split(text: &str, at: usize) -> (String, String) {
    let (t, a, _) = cut(text, at, at);
    (merge(&t[..a]), merge(&t[a..]))
}

/// `a..b` of the line, its marks made whole: what a copy takes.
pub fn slice(text: &str, a: usize, b: usize) -> String {
    let (a, b) = tighten(text, a, b);
    let (t, a, b) = cut(text, a, b);
    t[a..b].to_string()
}

/// Pairs of the same marks left touching — `~~a~~~~b~~` — made one, where
/// that keeps every word and its style: a stray mark beside them can read
/// otherwise once the marks between are gone.
pub fn merge(text: &str) -> String {
    let mut text = text.to_string();
    'joints: loop {
        let ps = pairs(&text);
        let look = look(&text);
        // The pairs by where they open, so a pair finds those opening at
        // its end without looking through every other.
        let mut opening: HashMap<usize, Vec<&Pair>> = HashMap::new();
        for q in &ps {
            opening.entry(q.open.0).or_default().push(q);
        }
        for p in &ps {
            if matches!(p.kind, Kind::Link | Kind::Code) {
                continue;
            }
            let same = |q: &&Pair| {
                q.kind == p.kind
                    && q.open.0 == p.close.1
                    && text[q.open.0..q.open.1] == text[p.open.0..p.open.1]
                    && text[q.close.0..q.close.1] == text[p.close.0..p.close.1]
            };
            for &q in opening.get(&p.close.1).into_iter().flatten().filter(|q| same(q)) {
                let mut t = text.clone();
                t.replace_range(q.open.0..q.open.1, "");
                t.replace_range(p.close.0..p.close.1, "");
                if self::look(&t) == look {
                    text = t;
                    continue 'joints;
                }
            }
        }
        return text;
    }
}

/// `left` and `right` put end to end, pairs of the same marks meeting at
/// the joint — `**a**` and `**b**` — made one. Which marks meet is read
/// from each side alone, not from the two together: read together,
/// `*a**b*` is one pair holding a literal `**` (as in CommonMark),
/// `==a====b==` no pair at all, and nor are two code spans meeting,
/// `` `a``b` ``.
pub fn join(left: &str, right: &str) -> String {
    let (lp, rp) = (pairs(left), pairs(right));
    // From the joint out: the last closing marks of `left` and the first
    // opening ones of `right`, while they are the same. Those met are all
    // of `left` past `end` and all of `right` before `start`.
    let (mut end, mut start) = (left.len(), 0);
    while let Some(p) = lp.iter().find(|p| p.close.1 == end && p.kind != Kind::Link) {
        // Two code spans: one, its marks made for the words of both.
        if p.kind == Kind::Code {
            let Some(q) = rp.iter().find(|q| q.open.0 == start && q.kind == Kind::Code) else { break };
            let words = format!("{}{}", &left[p.open.1..p.close.0], &right[q.open.1..q.close.0]);
            let (o, c) = code_marks(&words);
            return format!("{}{o}{words}{c}{}", &left[..p.open.0], &right[q.close.1..]);
        }
        let Some(q) = rp.iter().find(|q| {
            q.open.0 == start
                && q.kind == p.kind
                && right[q.open.0..q.open.1] == left[p.open.0..p.open.1]
                && right[q.close.0..q.close.1] == left[p.close.0..p.close.1]
        }) else {
            break;
        };
        (end, start) = (p.close.0, q.open.1);
    }
    respell_joint(format!("{}{}", &left[..end], &right[start..]), end)
}

/// Stars left meeting at a joint, where pairs close and others open —
/// `*a **b***` and `**c**` make `*a **b*****c**` — are one run to
/// CommonMark, which counts it whole and leaves stars showing. There one
/// or two of the pairs that meet are written with `_` (`*a **b***__c__`),
/// the first way that reads as the pieces meant.
fn respell_joint(text: String, joint: usize) -> String {
    let meant = look_counting(&text, Count::Left);
    if look(&text) == meant {
        return text;
    }
    let b = text.as_bytes();
    let star = |i: usize| b.get(i).is_some_and(|c| matches!(c, b'*' | b'_'));
    let (mut lo, mut hi) = (joint, joint);
    while lo > 0 && star(lo - 1) {
        lo -= 1;
    }
    while star(hi) {
        hi += 1;
    }
    let at_joint = |r: (usize, usize)| r.0 < hi && lo < r.1;
    let ps: Vec<Pair> = read_counting(&text, Count::Left)
        .pairs
        .into_iter()
        .filter(|p| matches!(p.kind, Kind::Bold | Kind::Italic) && (at_joint(p.open) || at_joint(p.close)))
        .collect();
    let respelled = |which: &[&Pair]| {
        let mut out = text.clone().into_bytes();
        for p in which {
            for i in (p.open.0..p.open.1).chain(p.close.0..p.close.1) {
                out[i] = if out[i] == b'*' { b'_' } else { b'*' };
            }
        }
        String::from_utf8(out).unwrap_or_default()
    };
    let ones = ps.iter().map(|p| vec![p]);
    let twos = ps.iter().enumerate().flat_map(|(i, p)| ps[i + 1..].iter().map(move |q| vec![p, q]));
    ones.chain(twos).map(|w| respelled(&w)).find(|t| look(t) == meant).unwrap_or(text)
}

/// The words of a line and the style of each byte of them: what an edit
/// that only moves marks must keep.
fn look(text: &str) -> Vec<(u8, Style)> {
    look_counting(text, Count::Whole)
}

fn look_counting(text: &str, count: Count) -> Vec<(u8, Style)> {
    let st = read_counting(text, count).styles;
    text.bytes().zip(st).filter(|(_, s)| !s.marker).collect()
}

/// The words in `a..b` taken out, the marks around them kept whole (and
/// dropped with the last of their words). Returns the text and the caret.
pub fn delete(text: &str, a: usize, b: usize) -> (String, usize) {
    let (a, b) = tighten(text, a, b);
    let d = Visible::of(text).drawn(a);
    if a >= b {
        return (text.to_string(), Visible::of(text).at(d));
    }
    let (t, na, nb) = cut(text, a, b);
    let out = merge(&join(&t[..na], &t[nb..]));
    let caret = Visible::of(&out).at(d);
    (out, caret)
}

/// What a range edit makes of the cut-out part, and where the selection
/// goes: the same words, held in from the marks. Where a stray mark took
/// one put in for one of a pair (a lone `*` and the `*` of a letter made
/// italic, in `*w*o*rd`), the edit is made again with the strays escaped;
/// where that changes the words too, there are no marks that say it
/// (bold on the `*` of `a*b`: `a**\***b` does not open), and it is `None`.
fn edit_range(text: &str, a: usize, b: usize, kind: Kind, f: impl Fn(&str) -> String) -> Option<(String, usize, usize)> {
    let once = edit_once(text, a, b, kind, &f)?;
    if plain(&once.0) == plain(text) {
        return Some(once);
    }
    let (escaped, at) = escape_strays(text);
    let again = edit_once(&escaped, at[a.min(text.len())], at[b.min(text.len())], kind, &f)?;
    (plain(&again.0) == plain(text)).then_some(again)
}

/// Marks that are no pair's — a stray `*`, `_`, `~`, `=`, `` ` ``, `[`,
/// `]`, `<` or `\` — each given a backslash, so marks put beside them
/// cannot take them for one of a pair, nor a backslash take them for a
/// mark to escape. Returns the text and where each byte of
/// the old one is in it (a stray's place, its backslash's).
fn escape_strays(text: &str) -> (String, Vec<usize>) {
    let st = styles(text);
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len() + 8);
    let mut at = Vec::with_capacity(b.len() + 1);
    for (i, &c) in b.iter().enumerate() {
        at.push(out.len());
        let escaped = i > 0 && b[i - 1] == b'\\' && st[i - 1].marker;
        if matches!(c, b'*' | b'_' | b'~' | b'=' | b'`' | b'[' | b']' | b'<' | b'\\') && !st[i].marker && !st[i].code && !escaped {
            out.push(b'\\');
        }
        out.push(c);
    }
    at.push(out.len());
    // Only ASCII went in, between whole characters.
    (String::from_utf8(out).unwrap_or_else(|_| text.to_string()), at)
}

fn edit_once(text: &str, a: usize, b: usize, kind: Kind, f: &impl Fn(&str) -> String) -> Option<(String, usize, usize)> {
    let (a, b) = tighten(text, a, b);
    if a >= b {
        return None;
    }
    let v = Visible::of(text);
    let (da, db) = (v.drawn(a), v.drawn(b));
    let (t, na, nb) = cut_keeping(text, a, b, Some(kind));
    let mid = f(&t[na..nb]);
    let out = merge(&join(&join(&t[..na], &mid), &t[nb..]));
    let w = Visible::of(&out);
    let start = if da == 0 { w.at(0) } else { w.source(da) };
    Some((out.clone(), start.min(out.len()), w.at(db).max(start)))
}

/// The marks of every pair of `kind` in `text` taken out.
fn unwrap_all(text: &str, kind: Kind) -> String {
    let mut drop: Vec<(usize, usize)> = pairs(text)
        .into_iter()
        .filter(|p| p.kind == kind)
        .flat_map(|p| [p.open, p.close])
        .collect();
    drop.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut out = text.to_string();
    for (s, e) in drop {
        out.replace_range(s..e, "");
    }
    out
}

/// Where to wrap all of `text` in a new pair: inside the pairs that
/// already wrap all of it (not inside code), so new marks sit next to the
/// words and runs on either side can join theirs.
fn innermost(text: &str) -> (usize, usize) {
    let ps = pairs(text);
    let (mut s, mut e) = (0, text.len());
    while let Some(p) = ps.iter().find(|p| p.open.0 == s && p.close.1 == e && p.kind != Kind::Code) {
        s = p.open.1;
        e = p.close.0;
    }
    (s, e)
}

fn wrap(text: &str, at: (usize, usize), open: &str, close: &str) -> String {
    format!("{}{open}{}{close}{}", &text[..at.0], &text[at.0..at.1], &text[at.1..])
}

/// Bold, italic, struck or code on `a..b`: off where all of it has it
/// already, else on for all of it. `None` when there are no words there,
/// or no marks that say it (see `edit_range`).
pub fn emphasis(text: &str, a: usize, b: usize, kind: Kind) -> Option<(String, usize, usize)> {
    // Spaces at the ends stay outside: `**word **` would not close. Code
    // could hold them, but a word chosen with the space after it is the
    // word; spaces alone are still made code.
    let (mut a, mut b) = tighten(text, a, b);
    let st = styles(text);
    let white = |i: usize| text.as_bytes()[i].is_ascii_whitespace();
    let space = |i: usize| (kind != Kind::Code && st[i].marker) || white(i);
    if kind != Kind::Code || (a..b).any(|i| !white(i)) {
        while a < b && space(a) {
            a += 1;
        }
        while b > a && space(b - 1) {
            b -= 1;
        }
    }
    edit_range(text, a, b, kind, |mid| {
        let st = styles(mid);
        let all = st.iter().filter(|s| !s.marker).all(|s| kind.on(s));
        let m = unwrap_all(mid, kind);
        if all {
            return m;
        }
        if kind == Kind::Code {
            let m = plain(&m);
            let (o, c) = code_marks(&m);
            return format!("{o}{m}{c}");
        }
        wrap(&m, innermost(&m), kind.marker(), kind.marker())
    })
}

/// Colour on `a..b`: the words' own when `ink`, else a highlighter's;
/// colour 0 takes it away, and so does the colour all of it has already.
/// A run of colour the range is all of takes the new one beside its own,
/// in its one tag; a range across runs is cut from them first.
pub fn tint(text: &str, a: usize, b: usize, ink: bool, color: u32) -> Option<(String, usize, usize)> {
    edit_range(text, a, b, Kind::Tint, |mid| {
        let field = |s: &Style| if ink { s.ink } else { s.mark };
        let st = styles(mid);
        // Spaces at the ends do not count: `==` leaves them outside, so
        // words highlighted with the space after them are all of it.
        let letters: Vec<(u8, &Style)> = mid.bytes().zip(&st).filter(|(_, s)| !s.marker).collect();
        let a = letters.iter().position(|(b, _)| !b.is_ascii_whitespace()).unwrap_or(0);
        let b = letters.iter().rposition(|(b, _)| !b.is_ascii_whitespace()).map_or(letters.len(), |i| i + 1);
        let same = color != 0 && letters[a..b].iter().all(|(_, s)| field(s) == color);
        let target = (!same && color != 0).then_some(color);
        // That colour out of every run in it, the last first, each found
        // again in the text as it is by then: one run can hold another
        // (`<mark>…==…==…</mark>`), and its tags move when the inner's are
        // written again.
        let mut m = mid.to_string();
        for k in (0..tints(&m).len()).rev() {
            let mut runs = tints(&m);
            runs.sort_by_key(|t| t.open.0);
            let Some(t) = runs.get(k) else { continue };
            let (i, kk) = if ink { (None, t.mark) } else { (t.ink, None) };
            let t = Tint { ink: t.ink, mark: t.mark, open: t.open, close: t.close };
            retag(&mut m, &t, i, kk);
        }
        let Some(c) = target else { return m };
        // …then given to the run that is all of it, beside its other
        // colour in its one tag, or to a new run.
        let ps = pairs(&m);
        let (mut s, mut e) = (0, m.len());
        let mut whole = None;
        while let Some(p) = ps.iter().find(|p| p.open.0 == s && p.close.1 == e && p.kind != Kind::Code) {
            if p.kind == Kind::Tint {
                whole = Some(p.open.0);
            }
            s = p.open.1;
            e = p.close.0;
        }
        match whole.and_then(|o| tints(&m).into_iter().find(|t| t.open.0 == o)) {
            Some(t) => {
                let (i, k) = if ink { (Some(c), t.mark) } else { (t.ink, Some(c)) };
                retag(&mut m, &t, i, k);
                m
            }
            None => {
                // `==` opens only before a word and closes only after one:
                // spaces at the ends of the words stay outside (spaces
                // alone take a `<mark>`).
                let (ink, mark) = (ink.then_some(c), (!ink).then_some(c));
                let (mut s, mut e) = (s, e);
                if tint_tags(ink, mark).is_some_and(|(o, _)| o == "==") && !m[s..e].trim().is_empty() {
                    s += m[s..e].len() - m[s..e].trim_start().len();
                    e = s + m[s..e].trim_end().len();
                }
                let (o, cl) = tags_around(ink, mark, &m[s..e]);
                wrap(&m, (s, e), &o, &cl)
            }
        }
    })
}

/// A coloured run's tags written again for these colours (none: gone).
fn retag(text: &mut String, t: &Tint, ink: Option<u32>, mark: Option<u32>) {
    let (o, c) = tags_around(ink, mark, &text[t.open.1..t.close.0]);
    text.replace_range(t.close.0..t.close.1, &c);
    text.replace_range(t.open.0..t.open.1, &o);
}

/// The link the caret at `at` is in: its pair and its address.
pub fn link_at(text: &str, at: usize) -> Option<(Pair, String)> {
    pairs(text)
        .into_iter()
        .find(|p| p.kind == Kind::Link && p.open.1 <= at && at <= p.close.0)
        .map(|p| (p, text[p.close.0 + 2..p.close.1 - 1].to_string()))
}

/// An address given the scheme it was written without: `example.com` is
/// taken for the web (`https://`), `me@example.com` for a mail. One that
/// names its scheme, or is a path (`/`, `./`, `#`, `~`), stays as it is;
/// `localhost:8080` is a host and its port, not a scheme.
pub fn with_scheme(url: &str) -> String {
    let url = url.trim();
    let scheme = url.split_once(':').filter(|(s, rest)| {
        let mut cs = s.chars();
        cs.next().is_some_and(|c| c.is_ascii_alphabetic())
            && cs.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
            && !rest.starts_with(|c: char| c.is_ascii_digit())
    });
    if url.is_empty() || scheme.is_some() || url.starts_with(['/', '.', '#', '~', '\\']) {
        url.to_string()
    } else if url.contains('@') && !url.contains('/') {
        format!("mailto:{url}")
    } else {
        format!("https://{url}")
    }
}

/// An address as a link holds it: a scheme if it was written without one,
/// and no spaces or brackets to end it early.
fn link_target(url: &str) -> String {
    with_scheme(url).replace(' ', "%20").replace('(', "%28").replace(')', "%29")
}

/// The link at `at` given another address.
pub fn set_link(text: &str, at: usize, url: &str) -> Option<String> {
    let (p, _) = link_at(text, at)?;
    let mut out = text.to_string();
    out.replace_range(p.close.0 + 2..p.close.1 - 1, &link_target(url));
    Some(out)
}

/// The link at `at` taken off its words.
pub fn unlink(text: &str, at: usize) -> Option<String> {
    let (p, _) = link_at(text, at)?;
    let mut out = text.to_string();
    out.replace_range(p.close.0..p.close.1, "");
    out.replace_range(p.open.0..p.open.1, "");
    Some(out)
}

/// `a..b` made a link to `url`.
pub fn make_link(text: &str, a: usize, b: usize, url: &str) -> Option<(String, usize, usize)> {
    let url = link_target(url);
    edit_range(text, a, b, Kind::Link, |mid| format!("[{}]({url})", unwrap_all(mid, Kind::Link)))
}

/// `s` typed at `at` inside `pre` and `post` — a style switched on (or
/// off, `pre` a closing mark) with nothing selected. Returns the text and
/// the caret, after what was typed.
pub fn insert_styled(text: &str, at: usize, s: &str, pre: &str, post: &str) -> (String, usize) {
    // A style switched off at a run's very end (or start) is the run left:
    // typed just past its closing marks (or before its opening ones), not
    // closed and opened again around nothing.
    let same = |p: &Pair| text[p.close.0..p.close.1] == *pre && text[p.open.0..p.open.1] == *post;
    let ps = pairs(text);
    let past = ps.iter().find(|p| p.close.0 == at && same(p)).map(|p| p.close.1);
    let before = ps.iter().find(|p| p.open.1 == at && same(p)).map(|p| p.open.0);
    if let Some(at) = past.or(before) {
        let out = format!("{}{s}{}", &text[..at], &text[at..]);
        return (out, at + s.len());
    }
    let d = Visible::of(text).drawn(at) + s.len();
    let out = merge(&join(&join(&text[..at], &format!("{pre}{s}{post}")), &text[at..]));
    let caret = Visible::of(&out).at(d);
    (out, caret)
}

/// The marks that would switch `kind` on at `at`, or off when the caret
/// is in a run of it: the run's closing mark and its opening one, typed
/// around what comes next.
pub fn switch_at(text: &str, at: usize, kind: Kind) -> (String, String) {
    let inside = pairs(text)
        .into_iter()
        .filter(|p| p.kind == kind && p.open.1 <= at && at <= p.close.0)
        .max_by_key(|p| p.open.0);
    match inside {
        Some(p) => (text[p.close.0..p.close.1].to_string(), text[p.open.0..p.open.1].to_string()),
        None => (kind.marker().to_string(), kind.marker().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dropped first letter is written as a tag around the letter and
    /// read back off it, the words as they were — styled, coloured, after
    /// a quote — and a tag anywhere but the start is left alone.
    #[test]
    fn a_dropped_first_letter_is_written_and_read_back() {
        for (words, file) in [
            ("The rest", "<span class=\"initial\">T</span>he rest"),
            ("**Bold** start", "**<span class=\"initial\">B</span>old** start"),
            ("\u{201c}Quoted,\u{201d} she said", "<span class=\"initial\">\u{201c}Q</span>uoted,\u{201d} she said"),
            (
                "<span style=\"color: #C0553F\">T</span>he rest",
                "<span class=\"initial\" style=\"color: #C0553F\">T</span>he rest",
            ),
            (
                "<span style=\"color: #C0553F\">The rest</span>",
                "<span style=\"color: #C0553F\"><span class=\"initial\">T</span>he rest</span>",
            ),
            ("==Lit== up", "==<span class=\"initial\">L</span>it== up"),
            ("Ёлка", "<span class=\"initial\">Ё</span>лка"),
        ] {
            assert_eq!(put_initial(words), file, "{words}");
            assert_eq!(take_initial(file).as_deref(), Some(words), "{file}");
            assert_eq!(plain(file), plain(words), "other apps show the letter plain");
        }
        // Nothing to drop: no letter first, or code.
        for words in ["— a dash", "`code` first", ""] {
            assert_eq!(put_initial(words), words);
        }
        // Not at the start: a tag like any other.
        assert_eq!(take_initial("Some <span class=\"initial\">x</span> here"), None);
        assert_eq!(take_initial("<span class=\"other\">T</span>he"), None);
        // Another app's way of writing it, in single quotes, among others.
        assert_eq!(take_initial("<span class='lead initial'>O</span>nce").as_deref(), Some("Once"));
    }

    /// Found by the property tests: a run of colour inside another
    /// (`<mark>…==…==…</mark>`) had its tags written again first, and the
    /// outer run's were then cut where they no longer were.
    #[test]
    fn a_colour_inside_another_is_taken_off_whole() {
        let text = "e\u{301}<mark>wordé==word==word</mark>";
        let (out, _, _) = tint(text, 0, text.len(), false, HIGHLIGHT).unwrap();
        assert_eq!(plain(&out), plain(text));
    }

    /// Found by the property tests: bold cut where a space follows wrote
    /// its `**` before the space, where it cannot open.
    #[test]
    fn a_cut_keeps_emphasis_off_the_spaces() {
        let text = "x **a *b* c** y";
        let vis = Visible::of(text);
        let (a, b) = (vis.at(0), vis.at(3));
        let (out, _, _) = emphasis(text, a, b, Kind::Bold).unwrap();
        assert_eq!(plain(&out), plain(text), "{out}");
    }

    /// Found by the property tests: a `*` between a mark and a word was
    /// taken to close a pair, so in `~~*b*` it closed the bold around it,
    /// and code put just before one (`` `a `*b* ``) let it do the same.
    #[test]
    fn a_mark_between_a_mark_and_a_word_only_opens() {
        let text = "x **a ~~*b* c~~** y";
        assert_eq!(show(text), "..mmbbmmmBmbbmmmm..");
        let vis = Visible::of(text);
        let (out, _, _) = emphasis(text, vis.at(3), vis.at(7), Kind::Strike).unwrap();
        assert_eq!(out, "x **a *b* c** y");
        let text = "x **a *b* c** y";
        let vis = Visible::of(text);
        // (A space chosen alone is code still, so code can still end
        // just before the `*`.)
        let (out, _, _) = emphasis(text, vis.at(3), vis.at(4), Kind::Code).unwrap();
        assert_eq!(out, "x **a` `*b* c** y");
        assert_eq!(plain(&out), plain(text));
        // With a word, the space at its end stays out of the code.
        let (out, _, _) = emphasis(text, vis.at(2), vis.at(4), Kind::Code).unwrap();
        assert_eq!(out, "x **`a` *b* c** y");
        assert_eq!(plain(&out), plain(text));
    }

    /// A word chosen with the space after it is the word: a second ⌘⇧H
    /// or ⌘E on the same choice takes the style off again.
    #[test]
    fn a_style_given_with_a_space_beside_comes_off_again() {
        let text = "some words here";
        for (a, b) in [(0, 5), (4, 10), (3, 7), (0, 15)] {
            let (once, x, y) = tint(text, a, b, false, HIGHLIGHT).unwrap();
            assert!(once.contains("=="), "{once}");
            let (twice, _, _) = tint(&once, x, y, false, HIGHLIGHT).unwrap();
            assert_eq!(twice, text, "highlight {a}..{b}: {once}");
            let (once, x, y) = emphasis(text, a, b, Kind::Code).unwrap();
            let inner = once.split('`').nth(1).unwrap_or_default();
            assert_eq!(inner, inner.trim(), "no space inside the code: {once}");
            let (twice, _, _) = emphasis(&once, x, y, Kind::Code).unwrap();
            assert_eq!(twice, text, "code {a}..{b}: {once}");
        }
    }

    /// Found by the property tests: a lone `*` took the `**` of a letter
    /// made bold for its pair. By CommonMark's rule of three it cannot.
    #[test]
    fn a_stray_star_is_not_paired_with_new_bold() {
        let (out, _, _) = emphasis("*wordword", 2, 3, Kind::Bold).unwrap();
        assert_eq!(out, "*w**o**rdword");
        assert_eq!(show(&out), "..mmbmm......");
    }

    /// Stars where pairs meet, closing two and opening a third, are one
    /// run to CommonMark, which counts it whole and leaves some showing;
    /// one of the pairs is written with `_` instead.
    #[test]
    fn where_pairs_meet_one_is_written_with_underscores() {
        let text = "word word **bold**";
        let vis = Visible::of(text);
        let (out, _, _) = emphasis(text, vis.at(0), vis.at(11), Kind::Italic).unwrap();
        assert_eq!(out, "_word word **b**_**old**");
        assert_eq!(plain(&out), plain(text));
        assert_eq!(plain("*word word **b*****old**"), "*word word b*old", "stars, as one run");
    }

    /// Found by the property tests: the plain highlighter on words ending
    /// in a space wrote its `==` after the space, where it cannot close.
    #[test]
    fn the_plain_highlighter_stays_off_spaces() {
        let text = "  word ~~gone~~ x";
        let vis = Visible::of(text);
        let (out, ..) = tint(text, vis.at(0), vis.at(7), false, HIGHLIGHT).unwrap();
        assert_eq!(out, "  ==word== ~~gone~~ x");
        // Spaces alone, or a run that starts with one, take a `<mark>`.
        assert_eq!(tint("a  b", 1, 3, false, HIGHLIGHT).unwrap().0, "a<mark>  </mark>b");
        // A highlighted run losing its colour keeps the plain highlighter.
        let text = "<mark style=\"color: red\"> hi</mark>";
        assert_eq!(tint(text, 0, text.len(), true, 0).unwrap().0, "<mark> hi</mark>");
    }

    /// Found by the property tests: code put on words running into a code
    /// span left the two meeting, `` `a``b` ``, which reads as neither.
    #[test]
    fn code_meeting_code_is_joined() {
        let text = "Ünï `code`";
        let vis = Visible::of(text);
        let (out, ..) = emphasis(text, vis.at(0), vis.at(7), Kind::Code).unwrap();
        assert_eq!(out, "`Ünï code`");
    }

    /// Found by the property tests: two runs joined where a stray mark
    /// beside them then read otherwise: `~~**~~~~*a**~~` is left as it is.
    #[test]
    fn joining_keeps_the_words_or_does_not_join() {
        let text = "~~**~~~~*a**~~";
        assert_eq!(merge(text), text);
        assert_eq!(merge("*~~**word~~**"), "*~~**word~~**");
    }

    /// A backslash before a mark makes it a letter, as in CommonMark; one
    /// before anything else is a backslash.
    #[test]
    fn a_backslash_escapes_a_mark() {
        assert_eq!(plain("\\*not it\\* \\[x\\](u) C:\\path \\\\"), "*not it* [x](u) C:\\path \\");
        assert_eq!(show("\\*a*"), "m...", "the other star has no pair");
        assert_eq!(plain("`a\\`"), "a\\", "not in code");
    }

    /// A stray mark a new one would pair with is escaped, and only then:
    /// italic on the `o` of `*wordword` cannot be written otherwise.
    #[test]
    fn a_stray_is_escaped_where_a_new_mark_would_take_it() {
        let (out, ..) = emphasis("*wordword", 2, 3, Kind::Italic).unwrap();
        assert_eq!(out, "\\*w*o*rdword");
        assert_eq!(emphasis("2 * 3 * 4", 0, 1, Kind::Bold).unwrap().0, "**2** * 3 * 4");
        // A backslash of the line's own would escape a mark put after it.
        let (out, ..) = emphasis("a\\ b", 0, 2, Kind::Bold).unwrap();
        assert_eq!(out, "**a\\\\** b");
    }

    /// Where no marks can say it, the line is left as it is: emphasis
    /// cannot open between a letter and a mark (`a**\***b`).
    #[test]
    fn what_no_marks_can_say_is_not_done() {
        assert_eq!(emphasis("a*b", 1, 2, Kind::Bold), None);
        // Punctuation is ASCII's, as CommonMark has it.
        assert_eq!(show("a**\"b\"**c"), ".........");
        assert_eq!(show("**\"b\"** c"), "mmbbbmm..");
    }

    /// Code takes as many backticks as no run in it, and a space inside
    /// each end where it starts or ends with one (CommonMark takes it away).
    #[test]
    fn code_marks_hold_their_backticks() {
        assert_eq!(plain("`` `b ``"), "`b");
        assert_eq!(plain("` a `"), "a");
        assert_eq!(plain("`  `"), "  ", "all spaces stay");
        assert_eq!(emphasis("a`b", 0, 3, Kind::Code).unwrap().0, "``a`b``");
        assert_eq!(emphasis("`b", 0, 2, Kind::Code).unwrap().0, "`` `b ``");
        assert_eq!(join("`a`", "`` `b ``"), "``a`b``");
        // A span cut takes the marks each piece needs.
        let (out, ..) = emphasis("``a`b``", 2, 3, Kind::Bold).unwrap();
        assert_eq!(plain(&out), "a`b", "{out}");
    }

    /// The line with each byte's style as one letter: m marker, b bold,
    /// i italic, s struck, c code, l link, . plain.
    fn show(text: &str) -> String {
        styles(text)
            .iter()
            .map(|s| match s {
                s if s.marker => 'm',
                s if s.code => 'c',
                s if s.link => 'l',
                s if s.bold && s.italic => 'B',
                s if s.bold => 'b',
                s if s.italic => 'i',
                s if s.strike => 's',
                _ => '.',
            })
            .collect()
    }

    #[test]
    fn reads_the_common_cases() {
        assert_eq!(show("a **bold** b"), "..mmbbbbmm..");
        assert_eq!(show("*it*"), "miim");
        assert_eq!(show("_it_ snake_case"), "miim...........");
        assert_eq!(show("~~x~~"), "mmsmm");
        assert_eq!(show("`*no*`"), "mccccm");
        assert_eq!(show("[a](u)"), "mlmmmm");
        assert_eq!(show("***both***"), "mmmBBBBmmm");
        assert_eq!(show("2 * 3 * 4"), ".........");
        assert_eq!(show("**open"), "......");
    }

    #[test]
    fn plain_reads_without_the_marks() {
        assert_eq!(plain("Ship **folders** and [docs](http://x) `now`"), "Ship folders and docs now");
        assert_eq!(plain("2 * 3 = 6"), "2 * 3 = 6");
    }

    #[test]
    fn reads_colour() {
        let st = styles("a ==hi== b");
        assert!(st[4].mark == HIGHLIGHT && st[2].marker && st[6].marker && st[0].mark == 0);
        let t = "<mark style=\"background: #FF5582A6\">x</mark> <span style=\"color: red; background: #fff\">y</span> <mark>z</mark>";
        let runs = tints(t);
        assert_eq!(runs.len(), 3);
        assert_eq!((runs[0].ink, runs[0].mark), (None, Some(0xFF5582A6)));
        assert_eq!((runs[1].ink, runs[1].mark), (Some(0xff0000ff), Some(0xffffffff)));
        assert_eq!(runs[2].mark, Some(HIGHLIGHT));
        assert_eq!(plain(t), "x y z");
        assert_eq!(plain("a == b and c==d"), "a == b and c==d", "no pair, no highlight");
        assert_eq!(plain("`<mark>x</mark>`"), "<mark>x</mark>", "code is code");
        // Nested like HTML: the outer run closes at its own tag.
        let t = "<span style=\"color: red\">a <span style=\"color: blue\">b</span> c</span>";
        assert_eq!(plain(t), "a b c");
        let st = styles(t);
        let at = |w: &str| st[t.find(w).unwrap()].ink;
        assert_eq!((at("a "), at("b<"), at(" c")), (0xff0000ff, 0x0000ffff, 0xff0000ff));
        assert_eq!(parse_color("#abc"), Some(0xaabbccff));
        assert_eq!(css_color(0x1f497dff), "#1F497D");
    }

    #[test]
    fn the_words_and_the_source_map_both_ways() {
        let t = "a **bold** c";
        let v = Visible::of(t);
        assert_eq!(v.text(t), "a bold c");
        assert_eq!(v.drawn(4), 2);
        assert_eq!(v.drawn(8), 6, "the closing marks are where the word ends");
        assert_eq!(v.source(2), 4, "b itself");
        assert_eq!(v.at(6), 8, "after d, inside the bold");
        assert_eq!(v.at(2), 2, "after the space, before the marks");
        assert_eq!(v.at(8), t.len());
        assert_eq!(v.at(0), 0);
        assert_eq!(Visible::of("**b**").at(0), 2, "a line that opens with a mark: inside it");
    }

    #[test]
    fn cutting_keeps_every_pair_whole() {
        let (t, a, b) = cut("**abcd**", 3, 5);
        assert_eq!(t, "**a****bc****d**");
        assert_eq!(&t[a..b], "**bc**");
        // A pair the range holds whole is left alone; one it starts
        // exactly at goes inside it.
        let (t, a, b) = cut("x **ab** y", 2, 8);
        assert_eq!(&t[a..b], "**ab**");
        let (h, tl) = split("a **bold** b", 6);
        assert_eq!((h.as_str(), tl.as_str()), ("a **bo**", "**ld** b"));
        let (h, tl) = split("**bold**", 2);
        assert_eq!((h.as_str(), tl.as_str()), ("", "**bold**"), "nothing bold is left behind");
        assert_eq!(slice("a **bold** [link](u) b", 6, 13), "**ld** [l](u)");
        assert_eq!(merge("~~a~~~~b~~ <mark>x</mark><mark>y</mark>"), "~~ab~~ <mark>xy</mark>");
        // Bold put end to end is joined where it meets; read together,
        // `**a****b**` is one bold run holding four stars, and stays so.
        assert_eq!(join("x **a**", "**b** y"), "x **ab** y");
        assert_eq!(merge("**a****b**"), "**a****b**");
    }

    #[test]
    fn deleting_takes_words_and_keeps_marks_whole() {
        // Backspace after the last letter of a bold word.
        let (t, c) = delete("a **b** c", 4, 5);
        assert_eq!((t.as_str(), c), ("a  c", 2));
        let (t, c) = delete("**ab** c", 3, 4);
        assert_eq!((t.as_str(), c), ("**a** c", 3));
        // Across a style's edge: what is left of each stays styled.
        let (t, _) = delete("**ab**cd", 3, 7);
        assert_eq!(t, "**a**d");
        // Only marks in the range: nothing to take.
        assert_eq!(delete("a**b**", 1, 3).0, "a**b**");
    }

    #[test]
    fn emphasis_goes_on_and_off_by_the_words() {
        let (t, a, b) = emphasis("make bold", 5, 9, Kind::Bold).unwrap();
        assert_eq!((t.as_str(), &t[a..b]), ("make **bold**", "bold"));
        let (t, a, b) = emphasis(&t, a, b, Kind::Bold).unwrap();
        assert_eq!((t.as_str(), &t[a..b]), ("make bold", "bold"));
        // Part of a bold run: that part only, the rest stays bold.
        let (t, ..) = emphasis("**abcd**", 3, 5, Kind::Bold).unwrap();
        assert_eq!(t, "**a**bc**d**");
        // Italic inside bold sits inside its marks.
        let (t, ..) = emphasis("**abcd**", 3, 5, Kind::Italic).unwrap();
        assert_eq!(t, "**a*bc*d**");
        // Some of it bold: all of it bold, one run.
        let (t, ..) = emphasis("x **y** z", 0, 9, Kind::Bold).unwrap();
        assert_eq!(t, "**x y z**");
        // A selection that runs over hidden marks is held to the words.
        let (t, ..) = emphasis("**ab** cd", 0, 6, Kind::Italic).unwrap();
        assert_eq!(t, "***ab*** cd");
        // Spaces at its ends stay out, so the marks close.
        let (t, ..) = emphasis("a bc d", 1, 5, Kind::Strike).unwrap();
        assert_eq!(t, "a ~~bc~~ d");
        // Inside code, a style goes around the code, never into it.
        let (t, ..) = emphasis("x `code` y", 3, 7, Kind::Bold).unwrap();
        assert_eq!(t, "x **`code`** y");
        let (t, ..) = tint("`code`", 1, 5, true, 0xff0000ff).unwrap();
        assert_eq!(t, "<span style=\"color: #FF0000\">`code`</span>");
    }

    #[test]
    fn colours_combine_in_one_tag_and_come_off() {
        let red = 0xff0000ff;
        let (t, a, b) = tint("make red", 5, 8, true, red).unwrap();
        assert_eq!(t, "make <span style=\"color: #FF0000\">red</span>");
        assert_eq!(&t[a..b], "red");
        // A highlighter on the same words joins the colour in its tag.
        let (t, ..) = tint(&t, a, b, false, 0xffff00ff).unwrap();
        assert_eq!(t, "make <span style=\"color: #FF0000; background: #FFFF00\">red</span>");
        // Selected by the words alone (the caret lands past the tag's
        // close), the same.
        let (t2, ..) = tint(&t, 5, t.len(), false, 0).unwrap();
        assert_eq!(t2, "make <span style=\"color: #FF0000\">red</span>", "the highlighter off");
        let (t3, ..) = tint(&t2, 0, t2.len(), true, red).unwrap();
        assert_eq!(t3, "<span style=\"color: #FF0000\">make red</span>", "all of it red, one run");
        let (t4, ..) = tint(&t3, 0, t3.len(), true, red).unwrap();
        assert_eq!(t4, "make red", "the same colour again takes it off");
        // The plain highlighter is Obsidian's `==`; a colour, a <mark>.
        let (t, a, b) = tint("hi", 0, 2, false, HIGHLIGHT).unwrap();
        assert_eq!(t, "==hi==");
        let (t, ..) = tint(&t, a, b, false, 0xc0504dff).unwrap();
        assert_eq!(t, "<mark style=\"background: #C0504D\">hi</mark>");
        // Part of a run: that part recoloured, the rest as it was.
        let (t, ..) = tint("<span style=\"color: red\">abc</span>", 26, 27, true, 0x0000ffff).unwrap();
        assert_eq!(
            t,
            "<span style=\"color: red\">a</span><span style=\"color: #0000FF\">b</span><span style=\"color: red\">c</span>"
        );
    }

    #[test]
    fn links_are_edited_by_their_words() {
        let t = "see [docs](http://a) now";
        let at = t.find("docs").unwrap() + 2;
        assert_eq!(link_at(t, at).map(|l| l.1), Some("http://a".into()));
        assert_eq!(set_link(t, at, "http://b c").unwrap(), "see [docs](http://b%20c) now");
        assert_eq!(unlink(t, at).unwrap(), "see docs now");
        let (t, a, b) = make_link("go here", 3, 7, "http://x").unwrap();
        assert_eq!((t.as_str(), &t[a..b]), ("go [here](http://x)", "here"));
    }

    #[test]
    fn an_address_without_a_scheme_is_given_one() {
        assert_eq!(with_scheme(" example.com/a "), "https://example.com/a");
        assert_eq!(with_scheme("www.x.test"), "https://www.x.test");
        assert_eq!(with_scheme("localhost:8080/a"), "https://localhost:8080/a");
        assert_eq!(with_scheme("me@x.test"), "mailto:me@x.test");
        // A scheme of its own, or a path, is kept.
        for kept in ["http://x", "HTTPS://x", "mailto:me@x.test", "ftp://x", "/a/b", "./a.md", "#top", ""] {
            assert_eq!(with_scheme(kept), kept);
        }
        let (t, _, _) = make_link("go here", 3, 7, "x.test").unwrap();
        assert_eq!(t, "go [here](https://x.test)");
        assert_eq!(set_link(&t, 5, "y.test").unwrap(), "go [here](https://y.test)");
    }

    #[test]
    fn a_style_switched_on_with_nothing_selected_wraps_what_is_typed() {
        assert_eq!(switch_at("ab", 1, Kind::Bold), ("**".into(), "**".into()));
        let (t, c) = insert_styled("ab", 1, "X", "**", "**");
        assert_eq!((t.as_str(), c), ("a**X**b", 4));
        // Inside bold, off: the run closed and opened around it.
        let (pre, post) = switch_at("**abcd**", 4, Kind::Bold);
        let (t, _) = insert_styled("**abcd**", 4, "X", &pre, &post);
        assert_eq!(t, "**ab**X**cd**");
        // Off at the end of a run: typed past its marks.
        let (pre, post) = switch_at("**ab** c", 4, Kind::Bold);
        assert_eq!(insert_styled("**ab** c", 4, "X", &pre, &post), ("**ab**X c".to_string(), 7));
        // Right after a bold run, on: it joins the run.
        let (t, _) = insert_styled("**ab** c", 6, "X", "**", "**");
        assert_eq!(t, "**abX** c");
    }

    /// A long line built to make a reader rescan it — tags nested deep,
    /// tags never closed, backtick runs that close nothing, openers a
    /// closer has to look past — is read in one pass, not one per mark:
    /// 200 KB in a fraction of a second, where nested tags once took
    /// seconds at 12 KB.
    #[test]
    fn a_long_pathological_line_is_read_quickly() {
        let n = 200_000;
        let lines = [
            format!("{}a{}", "<mark>".repeat(n / 14), "</mark>".repeat(n / 14)),
            format!("{}a{}", "<span style=\"color: red\">".repeat(n / 34), "</span>".repeat(n / 34)),
            "<mark ".repeat(n / 6) + ">",
            "<mark".repeat(n / 5),
            format!("{}`>`", "<mark ".repeat(n / 6)),
            (1..600).map(|k| "`".repeat(k) + " ").collect::<String>(),
            "` ".repeat(n / 2),
            "_a ".repeat(n / 6) + &"a* ".repeat(n / 6),
            "*a ".repeat(n / 6) + &"a_ ".repeat(n / 6),
            " **a".repeat(n / 8) + &"a*b".repeat(n / 6),
            "[".repeat(n / 2) + &"](".repeat(n / 4),
            "== a".repeat(n / 4),
            "**a ".repeat(n / 4),
            "~~a ".repeat(n / 4),
        ];
        for line in &lines {
            // An unoptimised build, the whole way a note's line goes: read,
            // drawn, made a note, written and its marks joined.
            let t0 = std::time::Instant::now();
            let _ = (read(line), Visible::of(line), merge(line));
            let (_, d) = crate::doc::from_markdown(line);
            let _ = crate::doc::to_markdown(None, &d);
            let took = t0.elapsed();
            assert!(took.as_millis() < 800, "{took:?} for {}…", &line[..40]);
        }
    }
}
