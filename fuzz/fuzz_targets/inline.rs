//! Any line, and any edit the page makes to one at any caret: styling,
//! colour, links, cutting, typing, splitting, joining. Nothing may panic.
//! Carets are where the page puts them: between letters, inside the marks
//! of the letter before (`Visible::at`). Nor may a line be slow: reading
//! one takes time about in proportion to its length, so libFuzzer's `-timeout` (and `-max_len`,
//! 4 KB unless raised) finds one that is not.
#![no_main]

use carnet_core::inline::{self, Kind, Visible};
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    text: &'a str,
    a: usize,
    b: usize,
    op: u8,
    color: u32,
    typed: &'a str,
}

/// The page's caret for word place `i`, on a letter's edge.
fn caret(text: &str, i: usize) -> usize {
    let vis = Visible::of(text);
    let words = vis.text(text);
    let mut d = i % (words.len() + 1);
    while !words.is_char_boundary(d) {
        d += 1;
    }
    vis.at(d)
}

fuzz_target!(|input: Input<'_>| {
    // One block's text: a line.
    let text = input.text.replace('\n', " ");
    let (a, b) = (caret(&text, input.a), caret(&text, input.b));
    let (a, b) = (a.min(b), a.max(b));
    let kinds = [Kind::Bold, Kind::Italic, Kind::Strike, Kind::Code];
    match input.op % 14 {
        k @ 0..4 => drop(inline::emphasis(&text, a, b, kinds[k as usize])),
        4 => drop(inline::tint(&text, a, b, true, input.color)),
        5 => drop(inline::tint(&text, a, b, false, input.color)),
        6 => drop(inline::cut(&text, a, b)),
        7 => drop(inline::delete(&text, a, b)),
        8 => drop(inline::split(&text, a)),
        9 => drop(inline::slice(&text, a, b)),
        10 => drop(inline::insert_styled(&text, a, input.typed, "**", "**")),
        11 => drop(inline::make_link(&text, a, b, input.typed)),
        12 => {
            let _ = inline::link_at(&text, a);
            let _ = inline::set_link(&text, a, input.typed);
            let _ = inline::unlink(&text, a);
        }
        _ => {
            let _ = inline::merge(&text);
            let _ = inline::switch_at(&text, a, kinds[(b % 4) as usize]);
            let _ = (inline::styles(&text), inline::tints(&text), inline::pairs(&text));
        }
    }
});
