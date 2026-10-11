//! A note's CRDT driven by any sequence of saves, merges, restores and
//! reloads, and fed any bytes as a saved doc or as another copy's changes
//! (what a corrupt replica file, or phase 2's network, could hand it).
//!
//! What must hold: a save leaves exactly the text saved; a restore leaves
//! exactly the version's text; a reload changes nothing; bytes that are
//! not a doc are an error, never a panic.
#![no_main]

use carnet_sync::{NoteDoc, Origin};
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
enum Op {
    Save(String, bool),
    /// Another app wrote `theirs` while `ours` was being saved, both from
    /// the version the history has at `base`.
    Merge { base: usize, theirs: String, ours: String },
    Restore(usize),
    Reload,
    /// Any bytes as a saved doc.
    Load(Vec<u8>),
    /// Any bytes as another copy's changes.
    Import(Vec<u8>),
}

fuzz_target!(|ops: Vec<Op>| {
    let mut n = NoteDoc::new(1);
    let mut peer = 2;
    for op in ops.into_iter().take(32) {
        match op {
            Op::Save(text, other) => {
                n.record(&text, if other { Origin::OtherApp } else { Origin::Save });
                assert_eq!(n.text(), text);
            }
            Op::Merge { base, theirs, ours } => {
                let h = n.history();
                if let Some(r) = h.get(base % h.len().max(1)) {
                    let _ = n.merge(&r.version, &theirs, &ours);
                }
            }
            Op::Restore(i) => {
                let h = n.history();
                if let Some(r) = h.get(i % h.len().max(1)) {
                    let then = n.text_at(&r.version).expect("a listed version reads");
                    n.restore(&r.version).expect("a listed version restores");
                    assert_eq!(n.text(), then);
                }
            }
            Op::Reload => {
                let (text, versions) = (n.text(), n.history().len());
                peer += 1;
                n = NoteDoc::load(&n.to_bytes(), peer).expect("its own bytes load");
                assert_eq!(n.text(), text);
                assert_eq!(n.history().len(), versions);
            }
            Op::Load(bytes) => {
                peer += 1;
                if let Ok(d) = NoteDoc::load(&bytes, peer) {
                    let _ = (d.text(), d.history());
                }
            }
            Op::Import(bytes) => {
                let _ = n.import(&bytes);
                let _ = n.text();
            }
        }
    }
});
