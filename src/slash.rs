//! The `/` menu: a line that is nothing but `/` and a word offers the
//! blocks that word could mean — `/ta` offers a table, `/h` the headings —
//! and Enter (or a click) makes the line one.

use carnet_core::doc::Kind;
use carnet_core::inline;
use crate::icons::Glyph;
use crate::page::{self, Caret, TURN_BULLET, TURN_H1, TURN_H2, TURN_H3, TURN_NUMBER, TURN_QUOTE, TURN_TODO};
use carnet_core::doc::Doc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Turns the line into another kind (`page::turn`).
    Turn(u64),
    Table,
    Divider,
    Code,
    /// Asks for picture files.
    Picture,
}

pub struct Command {
    pub name: &'static str,
    /// What to type instead, next time.
    pub hint: &'static str,
    /// Words it answers to besides its name's.
    pub keys: &'static [&'static str],
    pub glyph: Glyph,
    pub action: Action,
}

pub const COMMANDS: [Command; 11] = [
    Command { name: "Heading 1", hint: "#", keys: &["h1", "title", "heading"], glyph: Glyph::H1, action: Action::Turn(TURN_H1) },
    Command { name: "Heading 2", hint: "##", keys: &["h2", "subheading", "heading"], glyph: Glyph::H2, action: Action::Turn(TURN_H2) },
    Command { name: "Heading 3", hint: "###", keys: &["h3", "heading"], glyph: Glyph::H3, action: Action::Turn(TURN_H3) },
    Command { name: "Bulleted list", hint: "-", keys: &["bullet", "list", "ul"], glyph: Glyph::Bullets, action: Action::Turn(TURN_BULLET) },
    Command { name: "Numbered list", hint: "1.", keys: &["number", "list", "ol"], glyph: Glyph::Numbers, action: Action::Turn(TURN_NUMBER) },
    Command { name: "Checklist", hint: "[]", keys: &["todo", "task", "check", "list"], glyph: Glyph::Todo, action: Action::Turn(TURN_TODO) },
    Command { name: "Quote", hint: ">", keys: &["blockquote", "citation", "cite"], glyph: Glyph::Quote, action: Action::Turn(TURN_QUOTE) },
    Command { name: "Table", hint: "", keys: &["grid"], glyph: Glyph::Table, action: Action::Table },
    Command { name: "Divider", hint: "---", keys: &["rule", "line", "hr", "separator"], glyph: Glyph::Divider, action: Action::Divider },
    Command { name: "Code block", hint: "```", keys: &["code", "pre", "snippet"], glyph: Glyph::Code, action: Action::Code },
    Command { name: "Picture", hint: "", keys: &["image", "photo", "img", "screenshot"], glyph: Glyph::Picture, action: Action::Picture },
];

/// The word being typed after a `/`, when the caret ends a line of text
/// or a quote that holds only that — the whole block, or one line of it,
/// a paragraph's made by Shift+Enter as a quote's by Enter: its block and
/// the word.
pub fn query(doc: &Doc, c: Caret) -> Option<(u64, String)> {
    let (b, (start, end, _)) = query_line(doc, c)?;
    let t = b.text_ref()?;
    Some((b.id, t[start + 1..end].to_lowercase()))
}

/// The block and the `/word` line of it the caret ends: where the line
/// starts, where its word ends, and where the line does — past the marks
/// of a hard break the file gave it (two spaces, a backslash), which are
/// no words of it.
fn query_line(doc: &Doc, c: Caret) -> Option<(&carnet_core::doc::Block, (usize, usize, usize))> {
    if !c.is_empty() || c.head.slot.cell.is_some() {
        return None;
    }
    let b = doc.get(c.head.slot.block)?;
    let (Kind::Text(t) | Kind::Quote(t)) = &b.kind else { return None };
    let end = c.head.byte;
    if end > t.len() || !t.is_char_boundary(end) {
        return None;
    }
    let line_end = t[end..].find('\n').map_or(t.len(), |n| end + n);
    if end != line_end && !inline::breaks(t).contains(&(end, line_end)) {
        return None;
    }
    let start = t[..end].rfind('\n').map_or(0, |n| n + 1);
    let word = t[start..end].strip_prefix('/')?;
    if word.chars().any(char::is_whitespace) || word.len() > 24 {
        return None;
    }
    Some((b, (start, end, line_end)))
}

/// The `/word` line, when its block has more lines than it: made a
/// block of its own, of its block's kind, the lines before and after
/// it staying where they were. The caret in it.
fn own_line(doc: &mut Doc, c: &mut Caret) {
    let Some((b, (start, end, line_end))) = query_line(doc, *c) else { return };
    let Some(t) = b.text_ref() else { return };
    if start == 0 && line_end == t.len() {
        return;
    }
    let quote = matches!(b.kind, Kind::Quote(_));
    let (id, before, after) = (b.id, t[..start.saturating_sub(1)].to_string(), t.get(line_end + 1..).map(str::to_string));
    let kind = |s: String| if quote { Kind::Quote(s) } else { Kind::Text(s) };
    let Some(at) = doc.index_of(id) else { return };
    let line = carnet_core::doc::Block::new(kind(t[start..end].to_string()));
    let slot = page::Slot::block(line.id);
    let mut blocks = Vec::new();
    if start > 0 {
        let mut first = (*doc.blocks()[at]).clone();
        if let Some(t) = first.text_mut() {
            *t = before;
        }
        blocks.push(first);
    }
    blocks.push(line);
    if let Some(after) = after {
        blocks.push(carnet_core::doc::Block::new(kind(after)));
    }
    doc.splice(at..at + 1, blocks);
    *c = Caret::at(page::end(doc, slot));
}

/// The commands a word picks: those whose name starts with it (a word of
/// the name, or the name run together) first, then those one of whose
/// keys does; each group in the menu's order.
pub fn matches(word: &str) -> Vec<usize> {
    let rank = |c: &Command| {
        let name = c.name.to_lowercase();
        if word.is_empty() || name.split(' ').any(|w| w.starts_with(word)) || name.replace(' ', "").starts_with(word) {
            Some(0)
        } else if c.keys.iter().any(|k| k.starts_with(word)) {
            Some(1)
        } else {
            None
        }
    };
    let mut found: Vec<(u8, usize)> = COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(i, c)| rank(c).map(|r| (r, i)))
        .collect();
    found.sort();
    found.into_iter().map(|(_, i)| i).collect()
}

/// Makes the `/word` line what command `i` says. True when the note
/// changed. A picture leaves the line empty for the files to come.
pub fn apply(doc: &mut Doc, c: &mut Caret, i: usize) -> bool {
    let Some(cmd) = COMMANDS.get(i) else { return false };
    own_line(doc, c);
    let Some((block, _)) = query(doc, *c) else { return false };
    let Some(at) = doc.index_of(block) else { return false };
    if let Some(t) = doc.block_mut(at).text_mut() {
        t.clear();
    }
    // A quote's line made a table, a divider, code or a picture: the
    // block takes the quote's place, as it would a paragraph's.
    if matches!(doc.blocks()[at].kind, Kind::Quote(_)) && !matches!(cmd.action, Action::Turn(_)) {
        doc.block_mut(at).kind = Kind::Text(String::new());
    }
    *c = Caret::at(page::start(c.head.slot));
    match cmd.action {
        Action::Turn(to) => {
            page::turn(doc, c, to);
        }
        Action::Table => {
            page::insert_block(doc, c, Kind::Table(carnet_core::doc::Table::new(3, 3)));
        }
        Action::Divider => {
            page::insert_block(doc, c, Kind::Divider);
        }
        Action::Code => {
            page::insert_block(doc, c, Kind::Raw("```\n\n```".to_string()));
        }
        Action::Picture => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use carnet_core::doc::Block;
    use crate::page::{Pos, Slot};

    fn line(t: &str) -> (Doc, Caret) {
        let d = Doc::new(vec![Block::text(t)]);
        let slot = Slot::block(d.blocks()[0].id);
        (d, Caret::at(Pos { slot, byte: t.len() }))
    }

    #[test]
    fn a_slash_word_offers_and_makes_blocks() {
        assert_eq!(matches("").len(), COMMANDS.len());
        let names = |w: &str| matches(w).iter().map(|&i| COMMANDS[i].name).collect::<Vec<_>>();
        assert_eq!(names("ta"), ["Table", "Checklist"], "its name first, then a key (task)");
        assert_eq!(names("h2"), ["Heading 2"]);
        assert_eq!(names("head"), ["Heading 1", "Heading 2", "Heading 3"]);
        assert_eq!(names("todo"), ["Checklist"]);
        assert_eq!(names("pic"), ["Picture"]);
        assert!(names("zzz").is_empty());

        let (d, c) = line("/ta");
        assert_eq!(query(&d, c), Some((d.blocks()[0].id, "ta".into())));
        let (d, c) = line("a /ta");
        assert_eq!(query(&d, c), None, "only a line that is the command");

        let (mut d, mut c) = line("/h2");
        assert!(apply(&mut d, &mut c, 1));
        assert!(matches!(&d.blocks()[0].kind, Kind::Heading { level: 2, text } if text.is_empty()));

        let (mut d, mut c) = line("/tab");
        assert!(apply(&mut d, &mut c, 7));
        assert!(matches!(d.blocks()[0].kind, Kind::Table(_)));
        assert!(c.head.slot.cell.is_some(), "the caret goes into the first cell");
    }

    #[test]
    fn a_quote_offers_the_menu_as_a_paragraph_does() {
        let quote = |t: &str| {
            let d = Doc::new(vec![Block::new(Kind::Quote(t.into()))]);
            let slot = Slot::block(d.blocks()[0].id);
            (d, Caret::at(Pos { slot, byte: t.len() }))
        };
        let (d, c) = quote("/code");
        assert_eq!(query(&d, c), Some((d.blocks()[0].id, "code".into())));
        let code = matches("code")[0];
        let (mut d, mut c) = quote("/code");
        assert!(apply(&mut d, &mut c, code));
        assert!(matches!(&d.blocks()[0].kind, Kind::Raw(r) if r.starts_with("```")), "{:?}", d.blocks()[0].kind);
        assert!(!d.blocks().iter().any(|b| matches!(b.kind, Kind::Quote(_))), "no empty quote left behind");
        let (mut d, mut c) = quote("/h1");
        assert!(apply(&mut d, &mut c, 0));
        assert!(matches!(&d.blocks()[0].kind, Kind::Heading { level: 1, text } if text.is_empty()));
    }

    /// The finding: Enter at the end of "> quoted words" and `/` offered
    /// nothing, `/h2` typed as words. A quote goes on a line at a time,
    /// and any line of it offers the menu; its choice takes that line
    /// alone, the quote's other lines staying a quote.
    #[test]
    fn a_quotes_later_line_offers_the_menu() {
        let quote = |t: &str, at: usize| {
            let d = Doc::new(vec![Block::new(Kind::Quote(t.into()))]);
            let slot = Slot::block(d.blocks()[0].id);
            (d, Caret::at(Pos { slot, byte: at }))
        };
        let (d, c) = quote("quoted words\n/h2", 16);
        assert_eq!(query(&d, c), Some((d.blocks()[0].id, "h2".into())));
        let (d, c) = quote("quoted words\n/h2", 15);
        assert_eq!(query(&d, c), None, "not with the caret inside the word");
        let (d, c) = quote("quoted words\na /h2", 18);
        assert_eq!(query(&d, c), None, "only a line that is the command");
        // Made a heading: after the quote, which keeps its words.
        let (mut d, mut c) = quote("quoted words\n/h2", 16);
        assert!(apply(&mut d, &mut c, 1));
        assert!(matches!(&d.blocks()[0].kind, Kind::Quote(t) if t == "quoted words"), "{:?}", d.blocks()[0].kind);
        assert!(matches!(&d.blocks()[1].kind, Kind::Heading { level: 2, text } if text.is_empty()), "{:?}", d.blocks()[1].kind);
        assert_eq!(c.head.slot.block, d.blocks()[1].id);
        // In the middle of a quote: code between its two halves.
        let (mut d, mut c) = quote("one\n/code\ntwo", 9);
        assert!(apply(&mut d, &mut c, matches("code")[0]));
        let kinds: Vec<String> = d.blocks().iter().map(|b| format!("{:?}", b.kind)).collect();
        assert!(matches!(&d.blocks()[0].kind, Kind::Quote(t) if t == "one"), "{kinds:?}");
        assert!(matches!(&d.blocks()[1].kind, Kind::Raw(r) if r.starts_with("```")), "{kinds:?}");
        assert!(matches!(&d.blocks()[2].kind, Kind::Quote(t) if t == "two"), "{kinds:?}");
    }

    /// A paragraph's line made by Shift+Enter offers the menu as a
    /// quote's does, and so does a line ending in the marks of a hard
    /// break the file gave it, which are no words of the line; one with
    /// words before the `/` is still words.
    #[test]
    fn a_paragraphs_later_line_offers_the_menu() {
        let text = |t: &str, at: usize| {
            let d = Doc::new(vec![Block::text(t)]);
            let slot = Slot::block(d.blocks()[0].id);
            (d, Caret::at(Pos { slot, byte: at }))
        };
        let (d, c) = text("hello\n/h2", 9);
        assert_eq!(query(&d, c), Some((d.blocks()[0].id, "h2".into())));
        let (d, c) = text("hello\neither/or", 15);
        assert_eq!(query(&d, c), None, "mid-sentence, a slash");
        for (t, at) in [("/h2  \nmore", 3), ("one\n/h2\\\nmore", 7), ("one\n/h2  ", 7)] {
            let (d, c) = text(t, at);
            assert_eq!(query(&d, c), Some((d.blocks()[0].id, "h2".into())), "{t:?}");
        }
        let (d, c) = text("/h2 a  \nmore", 3);
        assert_eq!(query(&d, c), None, "words after the caret");
        // Made a heading: the line alone, its marks gone with it.
        let (mut d, mut c) = text("one\n/h2  \nmore", 7);
        assert!(apply(&mut d, &mut c, 1));
        let kinds: Vec<String> = d.blocks().iter().map(|b| format!("{:?}", b.kind)).collect();
        assert!(matches!(&d.blocks()[0].kind, Kind::Text(t) if t == "one"), "{kinds:?}");
        assert!(matches!(&d.blocks()[1].kind, Kind::Heading { level: 2, text } if text.is_empty()), "{kinds:?}");
        assert!(matches!(&d.blocks()[2].kind, Kind::Text(t) if t == "more"), "{kinds:?}");
    }
}
