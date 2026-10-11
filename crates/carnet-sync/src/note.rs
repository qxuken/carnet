//! One note's CRDT: a Loro text holding the note's file exactly as it is
//! on disk. A new version of the file becomes a change by a character diff
//! (`LoroText::update`), so the editor never has to say what it did; a
//! line diff would duplicate a line two devices both touched
//! (docs/SYNC.md §3).

use std::fmt;
use std::ops::ControlFlow;

use loro::{ExportMode, Frontiers, ID, IdSpan, LoroDoc, UpdateOptions};

/// The one container in a note's doc: its file's text.
const TEXT: &str = "md";

/// A writer's commits closer together than this, from the same origin,
/// are one change: what the history lists as one version, a sitting of
/// writing rather than every save (docs/SYNC.md §9).
const SITTING_S: i64 = 10 * 60;

/// A note's identity across renames, moves and, later, devices: 128
/// random bits, written as 32 hex digits. A file's path is where it is,
/// not which note it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(u128);

impl Key {
    pub fn random() -> Self {
        Key((u128::from(random_u64()) << 64) | u128::from(random_u64()))
    }

    pub fn parse(s: &str) -> Option<Self> {
        if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u128::from_str_radix(s, 16).ok().map(Key)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// Where a change came from, kept as its commit message: what a note's
/// history says beside each version. With sync on, the message names the
/// device too (`save by 0a1b…`): a run's Loro peer is fresh each time, so
/// only the message says which device wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The file as first seen: where a note's history starts.
    Import,
    /// Written here.
    Save,
    /// Changed on disk by something else: another editor, a folder sync.
    OtherApp,
    /// An older version brought back.
    Restore,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Import => "import",
            Origin::Save => "save",
            Origin::OtherApp => "other app",
            Origin::Restore => "restore",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.split_once(BY).map_or(s, |(o, _)| o);
        [Origin::Import, Origin::Save, Origin::OtherApp, Origin::Restore]
            .into_iter()
            .find(|o| o.as_str() == s)
    }
}

/// Between a commit message's origin and the device that wrote it.
const BY: &str = " by ";

/// The device a commit message names, if it names one.
fn author_of(message: &str) -> Option<Vec<u8>> {
    let hex = message.split_once(BY)?.1;
    (hex.len() % 2 == 0 && !hex.is_empty())
        .then(|| (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect())?
}

/// A version of a note, as bytes that can be kept: the doc's frontiers.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Version(Frontiers);

impl Version {
    pub(crate) fn frontiers(&self) -> &Frontiers {
        &self.0
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.encode()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Frontiers::decode(bytes).ok().map(Version)
    }
}

/// One version in a note's history: a sitting of saves, another app's
/// edit, a restore.
#[derive(Clone, Debug, PartialEq)]
pub struct Revision {
    /// The note as it was when this change was done.
    pub version: Version,
    /// When it began, in seconds since 1970.
    pub time: i64,
    /// `None` for a change made by something that said nothing.
    pub origin: Option<Origin>,
    /// Who wrote it: a run of the app.
    pub peer: u64,
    /// The device that wrote it, when sync was on there; `None` for one
    /// written before, or with sync off.
    pub device: Option<Vec<u8>>,
}

/// What went wrong inside the CRDT: a file of state that does not decode,
/// or a version this doc does not have.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err(e: impl fmt::Display) -> Error {
    Error(e.to_string())
}

/// One note's text and every version of it.
pub struct NoteDoc {
    doc: LoroDoc,
    /// The device writing here, as hex, for each change's message; empty
    /// with sync off.
    author: std::cell::RefCell<String>,
}

impl NoteDoc {
    /// An empty note, changed from here on as `peer`. A peer is one writer
    /// at a time: the app takes a fresh one each run.
    pub fn new(peer: u64) -> Self {
        let doc = LoroDoc::new();
        let _ = doc.set_peer_id(peer);
        NoteDoc::configure(doc)
    }

    fn configure(doc: LoroDoc) -> Self {
        doc.set_record_timestamp(true);
        doc.set_change_merge_interval(SITTING_S);
        NoteDoc { doc, author: Default::default() }
    }

    /// The device writing here from now on, named in each change it makes
    /// (`None`: sync off, nobody named).
    pub fn set_author(&self, device: Option<&[u8]>) {
        *self.author.borrow_mut() = device.map(|d| d.iter().map(|b| format!("{b:02x}")).collect()).unwrap_or_default();
    }

    /// A change's message: where it came from, and who wrote it.
    fn message(&self, origin: Origin) -> String {
        let author = self.author.borrow();
        if author.is_empty() { origin.as_str().to_string() } else { format!("{}{BY}{author}", origin.as_str()) }
    }

    /// A note from [`NoteDoc::to_bytes`].
    pub fn load(bytes: &[u8], peer: u64) -> Result<Self, Error> {
        let doc = LoroDoc::from_snapshot(bytes).map_err(err)?;
        doc.set_peer_id(peer).map_err(err)?;
        Ok(NoteDoc::configure(doc))
    }

    /// The whole doc, history and all.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.doc.export(ExportMode::Snapshot).unwrap_or_default()
    }

    pub fn text(&self) -> String {
        self.doc.get_text(TEXT).to_string()
    }

    /// Every change this doc holds, as bytes another copy of the note
    /// takes in with [`NoteDoc::import`]. (Phase 2 sends only what the
    /// other side lacks; this is all of it.)
    pub fn updates(&self) -> Vec<u8> {
        self.doc.export(ExportMode::all_updates()).unwrap_or_default()
    }

    /// Another copy's changes, merged in. Taken in twice, or out of
    /// order, they make the same note.
    pub fn import(&self, bytes: &[u8]) -> Result<(), Error> {
        self.doc.import(bytes).map(|_| ()).map_err(err)
    }

    /// Who writes here: this run's peer.
    pub fn peer(&self) -> u64 {
        self.doc.peer_id()
    }

    /// How many of `peer`'s operations this doc holds: the end of its span.
    pub fn counter(&self, peer: u64) -> u64 {
        self.doc.oplog_vv().get(&peer).map_or(0, |&c| c.max(0) as u64)
    }

    /// Every peer's operations held, as (peer, count): what a device says
    /// it has when it joins a room, and a snapshot's header carries.
    pub fn version_vector(&self) -> Vec<(u64, u64)> {
        let mut vv: Vec<(u64, u64)> = self.doc.oplog_vv().iter().map(|(&p, &c)| (p, c.max(0) as u64)).collect();
        vv.sort_unstable();
        vv
    }

    /// `peer`'s operations `start..end`, as bytes another copy imports: a
    /// delta record's contents (docs/SYNC.md §6).
    pub fn span(&self, peer: u64, start: u64, end: u64) -> Vec<u8> {
        let span = IdSpan::new(peer, start as i32, end as i32);
        self.doc.export(ExportMode::updates_in_range(vec![span])).unwrap_or_default()
    }

    /// The Loro doc itself, for the vault's own reading and writing.
    pub(crate) fn loro(&self) -> &LoroDoc {
        &self.doc
    }

    pub fn version(&self) -> Version {
        Version(self.doc.oplog_frontiers())
    }

    /// The note becomes `text`, as one change from `origin`. False when it
    /// already was: an unchanged file makes no version.
    pub fn record(&self, text: &str, origin: Origin) -> bool {
        let t = self.doc.get_text(TEXT);
        if t.to_string() == text {
            return false;
        }
        self.doc.set_next_commit_message(&self.message(origin));
        // With no timeout set the diff always finishes.
        let _ = t.update(text, UpdateOptions::default());
        self.doc.commit();
        true
    }

    /// `text` written from `base`, as a change beside what this doc has
    /// since: the editor's unsaved edits when another device's arrived
    /// under them. Both stay; the merged text comes back.
    pub fn apply_from(&self, base: &Version, text: &str, origin: Origin) -> Result<String, Error> {
        let fork = self.doc.fork_at(&base.0).map_err(err)?;
        fork.set_peer_id(random_u64()).map_err(err)?;
        fork.set_record_timestamp(true);
        let from = fork.oplog_vv();
        fork.set_next_commit_message(&self.message(origin));
        let _ = fork.get_text(TEXT).update(text, UpdateOptions::default());
        fork.commit();
        let ours = fork.export(ExportMode::updates(&from)).map_err(err)?;
        self.doc.import(&ours).map_err(err)?;
        Ok(self.text())
    }

    /// The file and the editor both changed since `base`: `theirs` is what
    /// another app wrote, `ours` what is being saved here. Each becomes a
    /// change from `base`, the two merge, and the merged text is returned,
    /// for the file and the page. Nothing either side wrote is lost.
    pub fn merge(&self, base: &Version, theirs: &str, ours: &str) -> Result<String, Error> {
        let fork = self.doc.fork_at(&base.0).map_err(err)?;
        fork.set_peer_id(random_u64()).map_err(err)?;
        fork.set_record_timestamp(true);
        let from = fork.oplog_vv();
        fork.set_next_commit_message(&self.message(Origin::OtherApp));
        let _ = fork.get_text(TEXT).update(theirs, UpdateOptions::default());
        fork.commit();
        self.record(ours, Origin::Save);
        let theirs = fork.export(ExportMode::updates(&from)).map_err(err)?;
        self.doc.import(&theirs).map_err(err)?;
        Ok(self.text())
    }
}

impl NoteDoc {
    /// Every version, newest first.
    pub fn history(&self) -> Vec<Revision> {
        let heads: Vec<ID> = self.doc.oplog_frontiers().iter().collect();
        let mut out = Vec::new();
        let _ = self.doc.travel_change_ancestors(&heads, &mut |c| {
            let last = ID::new(c.id.peer, c.id.counter + c.len as i32 - 1);
            out.push((
                c.lamport,
                Revision {
                    version: Version(Frontiers::from_id(last)),
                    time: c.timestamp,
                    origin: c.message.as_deref().and_then(Origin::parse),
                    peer: c.id.peer,
                    device: c.message.as_deref().and_then(author_of),
                },
            ));
            ControlFlow::Continue(())
        });
        // Newest first by time; changes made apart (a merge) by their
        // place in the doc's order when the clocks agree.
        out.sort_by(|(la, a), (lb, b)| b.time.cmp(&a.time).then(lb.cmp(la)));
        out.into_iter().map(|(_, r)| r).collect()
    }

    /// When it last changed, in seconds since 1970: the newest of its
    /// heads, read without walking the history.
    pub fn modified(&self) -> i64 {
        self.doc.oplog_frontiers().iter().filter_map(|id| self.doc.get_change(id)).map(|c| c.timestamp).max().unwrap_or(0)
    }

    /// The note's text as it was at `version`.
    pub fn text_at(&self, version: &Version) -> Result<String, Error> {
        let past = self.doc.fork_at(&version.0).map_err(err)?;
        Ok(past.get_text(TEXT).to_string())
    }

    /// The note becomes what it was at `version`, by a new change: history
    /// only grows, so a restore is itself a version, and can be undone by
    /// restoring the one before it. False when it already was.
    pub fn restore(&self, version: &Version) -> Result<bool, Error> {
        if self.text_at(version)? == self.text() {
            return Ok(false);
        }
        self.doc.set_next_commit_message(&self.message(Origin::Restore));
        self.doc.revert_to(&version.0).map_err(err)?;
        self.doc.commit();
        Ok(true)
    }
}

/// 64 random bits from Loro's own source, which it already has on every
/// target the app and the PWA build for: a new doc's peer id is one.
pub(crate) fn random_u64() -> u64 {
    LoroDoc::new().peer_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "# Shop\n\n- [ ] milk\n- [ ] bread\n- [ ] eggs\n";

    #[test]
    fn a_note_keeps_every_byte_and_skips_no_op_saves() {
        let n = NoteDoc::new(1);
        assert!(n.record("# Покупки\r\n\r\n- [x] хлеб 🥖\r\n", Origin::Import));
        assert!(!n.record("# Покупки\r\n\r\n- [x] хлеб 🥖\r\n", Origin::Save));
        let back = NoteDoc::load(&n.to_bytes(), 2).unwrap();
        assert_eq!(back.text(), "# Покупки\r\n\r\n- [x] хлеб 🥖\r\n");
        assert_eq!(back.version(), n.version());
    }

    #[test]
    fn the_editor_and_another_app_merge() {
        let n = NoteDoc::new(1);
        n.record(BASE, Origin::Import);
        let base = n.version();
        let disk = BASE.replace("- [ ] eggs", "- [x] eggs");
        let editor = BASE.replace("milk", "oat milk");
        let merged = n.merge(&base, &disk, &editor).unwrap();
        assert_eq!(merged, "# Shop\n\n- [ ] oat milk\n- [ ] bread\n- [x] eggs\n");
        // And it is the doc's text from now on, history and all.
        assert_eq!(NoteDoc::load(&n.to_bytes(), 3).unwrap().text(), merged);
    }

    #[test]
    fn history_lists_versions_and_a_restore_adds_one() {
        let n = NoteDoc::new(1);
        n.record("one\n", Origin::Import);
        let first = n.version();
        n.record("one\ntwo\n", Origin::Save);
        n.record("one\ntwo\nthree\n", Origin::Save);
        n.record("ONE\ntwo\nthree\n", Origin::OtherApp);
        // Import, a sitting of two saves, another app's edit.
        let h = n.history();
        let origins: Vec<_> = h.iter().map(|r| r.origin).collect();
        assert_eq!(origins, [Some(Origin::OtherApp), Some(Origin::Save), Some(Origin::Import)]);
        assert_eq!(n.text_at(&h[1].version).unwrap(), "one\ntwo\nthree\n");
        assert_eq!(n.text_at(&first).unwrap(), "one\n");

        assert!(n.restore(&first).unwrap());
        assert_eq!(n.text(), "one\n");
        assert!(!n.restore(&first).unwrap(), "already that version");
        let h = n.history();
        assert_eq!(h[0].origin, Some(Origin::Restore));
        // Restoring the version before the restore undoes it.
        assert!(n.restore(&h[1].version).unwrap());
        assert_eq!(n.text(), "ONE\ntwo\nthree\n");
        // Two restores in one sitting are one version, like two saves:
        // the list shows where it ended up.
        let h = n.history();
        assert_eq!(h.len(), 4);
        assert_eq!(n.text_at(&h[0].version).unwrap(), "ONE\ntwo\nthree\n");
        // And all of it survives a reload.
        let back = NoteDoc::load(&n.to_bytes(), 2).unwrap();
        assert_eq!(back.history(), h);
    }

    /// With an author set, each change names its device, and another copy
    /// reads it; before (or with sync off), none is named, and the origin
    /// reads as ever.
    #[test]
    fn a_change_names_the_device_that_made_it() {
        let a = NoteDoc::new(1);
        a.record("first\n", Origin::Save);
        a.set_author(Some(&[0x0a, 0xbc]));
        a.record("first\nsecond\n", Origin::Save);
        let b = NoteDoc::new(2);
        b.import(&a.updates()).unwrap();
        b.set_author(Some(&[0x0b]));
        b.restore(&b.history()[1].version).unwrap();
        let h = b.history();
        let by: Vec<(Option<Origin>, Option<Vec<u8>>)> = h.iter().map(|r| (r.origin, r.device.clone())).collect();
        assert_eq!(
            by,
            [(Some(Origin::Restore), Some(vec![0x0b])), (Some(Origin::Save), Some(vec![0x0a, 0xbc])), (Some(Origin::Save), None)]
        );
        assert_eq!(Origin::parse("other app by 0a"), Some(Origin::OtherApp));
        assert_eq!(author_of("save by zz"), None, "not hex: nobody");
    }

    #[test]
    fn keys_and_versions_round_trip() {
        let k = Key::random();
        assert_eq!(Key::parse(&k.to_string()), Some(k));
        assert_ne!(Key::random(), k);
        assert_eq!(Key::parse("nope"), None);
        let n = NoteDoc::new(1);
        n.record("a", Origin::Import);
        let v = n.version();
        assert_eq!(Version::from_bytes(&v.to_bytes()), Some(v));
        assert_eq!(Origin::parse("other app"), Some(Origin::OtherApp));
    }
}
