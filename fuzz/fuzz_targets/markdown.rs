//! Any text as a note file: read, written, read again, made HTML, counted.
//! Nothing may panic. (That the round trip settles is the property tests'
//! to hold, on notes as people write them: crates/carnet-core/tests.)
#![no_main]

use carnet_core::{doc, export};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let (title, d) = doc::from_markdown(text);
    let once = doc::to_markdown(title.as_deref(), &d);
    let (title, again) = doc::from_markdown(&once);
    let _ = doc::to_markdown(title.as_deref(), &again);
    let _ = export::html(text, |src, _| src.to_string());
    let _ = (d.word_count(), d.todos(), d.snippet(), d.contains("word"));
});
