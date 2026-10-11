//! Any two notes, compared as the history compares versions. Nothing may
//! panic, and a note compared with itself is marked nowhere.
#![no_main]

use carnet_core::{compare, doc};
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    old: &'a str,
    new: &'a str,
}

fuzz_target!(|input: Input<'_>| {
    let _ = compare::marked(input.old, input.new);
    let (_, same) = compare::marked(input.new, input.new);
    let (_, plain) = doc::from_markdown(input.new);
    assert_eq!(same.len(), plain.len());
    for (m, p) in same.blocks().iter().zip(plain.blocks()) {
        assert_eq!(m.text_ref(), p.text_ref(), "a note compared with itself was marked");
    }
});
