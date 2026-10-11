//! Undo and redo for one note: a lightweight take on kawoosh's history.
//!
//! kawoosh keeps retained roots of a persistent piece table, so a state is
//! a pointer and a checkpoint costs nothing. A note here is a list of
//! blocks, so the same idea is a list of shared blocks: a state is a vector
//! of `Rc<Block>` that shares every block it did not change with the states
//! around it, and an edit copies only the block it writes.
//!
//! As there, a checkpoint is taken when a command begins, and typing is
//! one command until it pauses, moves to another block, or turns into a
//! different kind of edit — so Ctrl+Z takes back a word or a sentence, not
//! a letter. The caret is part of a state: undo puts it back where the
//! change was made.

use std::rc::Rc;
use std::time::Duration;

use crate::doc::{Block, Doc};
use crate::page::Caret;

/// One state a note was in.
#[derive(Clone, Debug)]
pub struct State {
    pub blocks: Vec<Rc<Block>>,
    pub caret: Caret,
}

impl State {
    pub fn of(doc: &Doc, caret: Caret) -> Self {
        Self {
            blocks: doc.snapshot(),
            caret,
        }
    }
}

/// What an edit was, for grouping: a run of one kind in one block is one
/// step. Anything structural is a step of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Typing,
    Deleting,
    Other,
}

/// How many states a note keeps behind it; past this, the oldest go.
const DEPTH: usize = 1000;
/// A pause this long ends a run of typing.
const PAUSE: Duration = Duration::from_millis(1200);

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<State>,
    redo: Vec<State>,
    /// The step under way: its kind, its block, when it last moved (on
    /// the caller's clock: [`History::record_at`]).
    open: Option<(Step, u64, Duration)>,
}

impl History {
    /// How many steps there are to undo and to redo.
    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    /// Called before an edit, with the state it starts from. A step that
    /// continues the one under way adds nothing; any other closes it and
    /// keeps `before`. A new edit forgets the redo states, as it does
    /// everywhere.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn record(&mut self, step: Step, block: u64, before: State) {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        self.record_at(step, block, before, START.get_or_init(Instant::now).elapsed());
    }

    /// [`record`](History::record) with the time given: `now` on any clock
    /// that only goes forward (a browser has no `Instant`).
    pub fn record_at(&mut self, step: Step, block: u64, before: State, now: Duration) {
        let continues = matches!(
            self.open,
            Some((s, b, at)) if s == step && s != Step::Other && b == block && now.saturating_sub(at) < PAUSE
        );
        self.open = Some((step, block, now));
        if continues {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Ends the step under way: the next edit starts a new one. The caret
    /// moving somewhere else is the usual reason.
    pub fn seal(&mut self) {
        self.open = None;
    }

    /// The state to go back to, given the one the note is in now.
    pub fn undo(&mut self, now: State) -> Option<State> {
        self.open = None;
        let back = self.undo.pop()?;
        self.redo.push(now);
        Some(back)
    }

    pub fn redo(&mut self, now: State) -> Option<State> {
        self.open = None;
        let ahead = self.redo.pop()?;
        self.undo.push(now);
        Some(ahead)
    }

    /// Every state kept, to undo and to redo, with each block `f` gives
    /// another for in its place: a file the note links to moved, say, and
    /// no state should bring back the path it had.
    pub fn rewrite(&mut self, mut f: impl FnMut(&Block) -> Option<Block>) {
        for state in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            for b in state.blocks.iter_mut() {
                if let Some(new) = f(b) {
                    *b = Rc::new(new);
                }
            }
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Block;
    use crate::page::{Caret, Pos, Slot};

    fn caret(block: u64) -> Caret {
        Caret::at(Pos {
            slot: Slot { block, cell: None },
            byte: 0,
        })
    }

    #[test]
    fn typing_is_one_step_and_states_share_blocks() {
        let mut doc = Doc::new(vec![Block::text("a"), Block::text("b")]);
        let (a, b) = (doc.blocks()[0].id, doc.blocks()[1].id);
        let mut h = History::default();
        for ch in ["x", "y", "z"] {
            h.record(Step::Typing, a, State::of(&doc, caret(a)));
            doc.block_mut(0).text_mut().unwrap().push_str(ch);
        }
        // Three keystrokes, one step; the untouched block is still shared
        // between the state kept and the text now.
        assert_eq!(h.undo.len(), 1);
        assert!(Rc::ptr_eq(&h.undo[0].blocks[1], &doc.blocks()[1]));
        assert!(!Rc::ptr_eq(&h.undo[0].blocks[0], &doc.blocks()[0]));

        // Another block, another step.
        h.record(Step::Typing, b, State::of(&doc, caret(b)));
        doc.block_mut(1).text_mut().unwrap().push('!');
        assert_eq!(h.undo.len(), 2);

        let back = h.undo(State::of(&doc, caret(b))).unwrap();
        doc.restore(back.blocks);
        assert_eq!(doc.blocks()[1].text_ref(), Some("b"));
        let back = h.undo(State::of(&doc, caret(a))).unwrap();
        doc.restore(back.blocks);
        assert_eq!(doc.blocks()[0].text_ref(), Some("a"));
        assert!(h.undo(State::of(&doc, caret(a))).is_none());
        let ahead = h.redo(State::of(&doc, caret(a))).unwrap();
        doc.restore(ahead.blocks);
        assert_eq!(doc.blocks()[0].text_ref(), Some("axyz"));
    }
}
