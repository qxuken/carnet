//! The `/` menu: a line that is nothing but `/` and a word offers the
//! blocks that word could mean — `/ta` offers a table, `/h` the headings —
//! and Enter (or a click) makes the line one.

use carnet_core::doc::Kind;
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
/// that holds only that: its block and the word.
pub fn query(doc: &Doc, c: Caret) -> Option<(u64, String)> {
    if !c.is_empty() || c.head.slot.cell.is_some() {
        return None;
    }
    let b = doc.get(c.head.slot.block)?;
    let Kind::Text(t) = &b.kind else { return None };
    let word = t.strip_prefix('/')?;
    if c.head.byte != t.len() || word.chars().any(char::is_whitespace) || word.len() > 24 {
        return None;
    }
    Some((b.id, word.to_lowercase()))
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
    let Some((block, _)) = query(doc, *c) else { return false };
    let Some(cmd) = COMMANDS.get(i) else { return false };
    let Some(at) = doc.index_of(block) else { return false };
    if let Some(t) = doc.block_mut(at).text_mut() {
        t.clear();
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
}
