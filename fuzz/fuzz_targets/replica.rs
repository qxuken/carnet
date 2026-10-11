//! The replica opened on any index and any doc file — what a crash
//! mid-write, a disk error or another program could leave — then used.
//! Nothing may panic: a replica that cannot be read is rebuilt from the
//! files, never a reason for the app to stop.
#![no_main]

use std::fs;

use carnet_sync::{Key, Origin, Replica};
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    index: &'a [u8],
    doc: &'a [u8],
    text: &'a str,
}

fuzz_target!(|input: Input<'_>| {
    let dir = std::env::temp_dir().join(format!("carnet-fuzz-replica-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("notes")).unwrap();
    fs::write(dir.join("index.log"), input.index).unwrap();
    // The doc of the first key the index names, if it names one.
    let first = String::from_utf8_lossy(input.index).lines().next().and_then(|l| l.split('\t').next().and_then(Key::parse));
    if let Some(k) = first {
        fs::write(dir.join("notes").join(format!("{k}.loro")), input.doc).unwrap();
    }
    if let Ok(mut r) = Replica::open(&dir) {
        let key = first.unwrap_or_else(Key::random);
        let _ = r.key_for("Note.md", input.text, |_| false, |_| false);
        let _ = r.history(key);
        let _ = r.record(key, "Note.md", input.text, Origin::Save);
        let _ = r.history(key);
        drop(r);
        let _ = Replica::open(&dir);
    }
    let _ = fs::remove_dir_all(&dir);
});
