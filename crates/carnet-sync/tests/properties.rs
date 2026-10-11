//! Property tests for a note's CRDT and this device's replica: random
//! texts and edits, random orders of arrival, runs cut off and reopened.
//! proptest shrinks a failure to the smallest case and keeps it in
//! `proptest-regressions/`.
//!
//! What must hold:
//! - a note holds every byte it was given, across a reload;
//! - copies that saw the same changes hold the same text, whatever order
//!   the changes came in and however often;
//! - a merge with one side unchanged is the other side, and edits to
//!   lines apart both survive one;
//! - every version in a history can be read and restored, and a restore
//!   is a version;
//! - the replica remembers each note's last text and path across reopens.

use std::collections::HashMap;
use std::path::PathBuf;

use carnet_sync::{Key, NoteDoc, Origin, Replica, text_hash};
use proptest::prelude::*;
use proptest::sample::{Index, select};

const PIECES: &[&str] = &[
    "word", " ", "\n", "\n\n", "\r\n", "- [ ] ", "- [x] ", "# ", "| a | b |\n", "**", "é", "名前",
    "👩\u{200d}👩\u{200d}👧", "e\u{301}", "\t", "milk", "eggs",
];

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => prop::collection::vec(select(PIECES), 0..30).prop_map(|v| v.concat()),
        1 => any::<String>(),
    ]
}

#[derive(Clone, Debug)]
enum Change {
    Put(Index, &'static str),
    Cut(Index, usize),
}

fn changes() -> impl Strategy<Value = Vec<Change>> {
    let one = prop_oneof![
        (any::<Index>(), select(PIECES)).prop_map(|(i, s)| Change::Put(i, s)),
        (any::<Index>(), 1..10usize).prop_map(|(i, n)| Change::Cut(i, n)),
    ];
    prop::collection::vec(one, 1..5)
}

fn boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn apply(s: &str, changes: &[Change]) -> String {
    let mut s = s.to_string();
    for c in changes {
        match c {
            Change::Put(i, p) => {
                let at = boundary(&s, i.index(s.len() + 1));
                s.insert_str(at, p);
            }
            Change::Cut(i, n) => {
                let a = boundary(&s, i.index(s.len() + 1));
                let b = boundary(&s, a + n);
                s.replace_range(a..b, "");
            }
        }
    }
    s
}

fn temp(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "carnet-sync-prop-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// `n` cases, or as many as `PROPTEST_CASES` asks for: a deep run is
/// `PROPTEST_CASES=10000 cargo test --release`.
fn cases(n: u32) -> ProptestConfig {
    let n = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(n);
    ProptestConfig::with_cases(n)
}

proptest! {
    #![proptest_config(cases(256))]

    #[test]
    fn a_note_keeps_every_byte(start in text(), edits in prop::collection::vec(changes(), 1..8)) {
        let n = NoteDoc::new(1);
        n.record(&start, Origin::Import);
        let mut want = start;
        for e in &edits {
            want = apply(&want, e);
            n.record(&want, Origin::Save);
            prop_assert_eq!(n.text(), want.clone());
        }
        let back = NoteDoc::load(&n.to_bytes(), 2).unwrap();
        prop_assert_eq!(back.text(), want);
    }

    /// Two to four copies of a note, each edited apart, then given each
    /// other's changes in a random order, some twice: all the same text.
    #[test]
    fn copies_converge_in_any_order(
        base in text(),
        edits in prop::collection::vec(prop::collection::vec(changes(), 1..4), 2..5),
        order in prop::collection::vec((any::<Index>(), any::<Index>()), 0..24),
    ) {
        let first = NoteDoc::new(1);
        first.record(&base, Origin::Import);
        let bytes = first.to_bytes();
        let copies: Vec<NoteDoc> = (0..edits.len()).map(|i| NoteDoc::load(&bytes, 10 + i as u64).unwrap()).collect();
        for (c, rounds) in copies.iter().zip(&edits) {
            let mut t = c.text();
            for e in rounds {
                t = apply(&t, e);
                c.record(&t, Origin::Save);
            }
        }
        // Some of it in a random order, then all of it, so every copy has
        // everything in the end.
        for (from, to) in &order {
            let (f, t) = (from.index(copies.len()), to.index(copies.len()));
            copies[t].import(&copies[f].updates()).unwrap();
        }
        for t in &copies {
            for f in &copies {
                t.import(&f.updates()).unwrap();
            }
        }
        let texts: Vec<String> = copies.iter().map(NoteDoc::text).collect();
        for t in &texts {
            prop_assert_eq!(t, &texts[0]);
        }
    }

    #[test]
    fn a_merge_with_one_side_unchanged_is_the_other(base in text(), e in changes()) {
        let changed = apply(&base, &e);
        let n = NoteDoc::new(1);
        n.record(&base, Origin::Import);
        let v = n.version();
        prop_assert_eq!(n.merge(&v, &base, &changed).unwrap(), changed.clone());
        let n = NoteDoc::new(1);
        n.record(&base, Origin::Import);
        let v = n.version();
        prop_assert_eq!(n.merge(&v, &changed, &base).unwrap(), changed);
    }

    /// The editor changes one line and another app another, two or more
    /// apart: the merge has both new lines.
    #[test]
    fn edits_to_lines_apart_both_survive_a_merge(
        count in 4..12usize,
        a in any::<Index>(),
        gap in 2..6usize,
        ours in select(&["oat milk", "Мой", "👩\u{200d}👩\u{200d}👧 fed"][..]),
        theirs in select(&["eggs ×12", "名前", "e\u{301}clair"][..]),
    ) {
        let lines: Vec<String> = (0..count).map(|i| format!("- [ ] item {i}")).collect();
        let i = a.index(count);
        let j = (i + gap) % count;
        prop_assume!(i.abs_diff(j) >= 2);
        let base = lines.join("\n") + "\n";
        let mut mine = lines.clone();
        mine[i] = format!("- [ ] {ours}");
        let mut other = lines.clone();
        other[j] = format!("- [x] {theirs}");
        let n = NoteDoc::new(1);
        n.record(&base, Origin::Import);
        let v = n.version();
        let merged = n.merge(&v, &(other.join("\n") + "\n"), &(mine.join("\n") + "\n")).unwrap();
        let got: Vec<&str> = merged.lines().collect();
        prop_assert!(got.contains(&mine[i].as_str()), "{merged:?}");
        prop_assert!(got.contains(&other[j].as_str()), "{merged:?}");
        prop_assert_eq!(got.len(), count);
    }

    #[test]
    fn every_version_reads_and_restores(start in text(), edits in prop::collection::vec(changes(), 1..6)) {
        let n = NoteDoc::new(1);
        n.record(&start, Origin::Import);
        let mut t = start;
        // Origins taking turns, so the history has versions to choose from.
        for (k, e) in edits.iter().enumerate() {
            t = apply(&t, e);
            n.record(&t, if k % 2 == 0 { Origin::Save } else { Origin::OtherApp });
        }
        for r in n.history() {
            let then = n.text_at(&r.version).unwrap();
            let before = n.text();
            let changed = n.restore(&r.version).unwrap();
            prop_assert_eq!(n.text(), then.clone());
            prop_assert_eq!(changed, before != then);
            if changed {
                prop_assert_eq!(n.history()[0].origin, Some(Origin::Restore));
            }
        }
    }

    /// Notes recorded at paths, moved, recorded again, the replica closed
    /// and opened in between: each note's last text and path are what it
    /// remembers.
    #[test]
    fn the_replica_remembers_across_reopens(
        steps in prop::collection::vec((0..4usize, 0..3usize, 0..3usize, text()), 1..24),
    ) {
        let dir = temp("replica");
        let keys: Vec<Key> = (0..3).map(|_| Key::random()).collect();
        let mut last: HashMap<Key, (String, String)> = HashMap::new();
        let mut r = Replica::open(&dir).unwrap();
        for (what, k, p, t) in steps {
            let key = keys[k];
            let path = format!("Folder/Note {p}.md");
            match what {
                0 | 1 => {
                    r.record(key, &path, &t, Origin::Save).unwrap();
                    last.insert(key, (path, t));
                }
                2 => {
                    r.moved(key, &path).unwrap();
                    if let Some(l) = last.get_mut(&key) {
                        l.0 = path;
                    }
                }
                _ => {
                    drop(r);
                    r = Replica::open(&dir).unwrap();
                }
            }
        }
        drop(r);
        let mut r = Replica::open(&dir).unwrap();
        for (key, (path, t)) in &last {
            let e = r.entry(*key).unwrap().clone();
            prop_assert_eq!(&e.path, path);
            prop_assert_eq!(e.hash, text_hash(t));
            prop_assert_eq!(&r.text_at(*key, &e.version).unwrap(), t);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
