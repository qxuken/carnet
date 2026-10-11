//! This device's replica of a notes folder: every note's [`NoteDoc`], and
//! an index from each note's [`Key`] to where its file is and what it
//! last held.
//!
//! ```text
//! <dir>/index.log        a line per change: key, text hash, version, path
//! <dir>/notes/<key>.loro each note's doc, history and all
//! <dir>/vault.loro       the vault doc: where each note lives (vault.rs)
//! <dir>/vault.applied    the vault's version last applied to the files
//! <dir>/version          the layout these files are in (LAYOUT)
//! ```
//!
//! The desktop keeps two more files here, under the same layout:
//! `pictures.txt` (pictures.rs) and `forgotten.txt` (sync.rs).
//!
//! A replica from a release that laid it out differently is brought up to
//! date as it opens, a step at a time ([`LAYOUT`]); one from a newer
//! release is left as it is, and does not open ([`Newer`]).
//!
//! It lives in the app's data folder, keyed by the notes folder, never
//! inside it: a folder synced by Dropbox would sync this too and conflict
//! (docs/SYNC.md §4). Nothing in it is needed to read a note. Lost, it is
//! rebuilt from the files, without their history; that is why its writes
//! are atomic but not flushed to disk as the notes' are.
//!
//! The index is a log, appended to as notes change and rewritten whole
//! when it opens with more stale lines than live ones, so a save appends
//! one short line rather than rewriting the index of 10,000 notes.
//!
//! Docs are written by a thread of their own: a save hands over the bytes
//! and goes on, and a doc still waiting to be written is read from memory.
//! Opening a folder of 5,000 notes for the first time makes 5,000 docs in
//! about a tenth of a second; writing their files takes six times that,
//! and the window need not wait for it. Dropping the replica waits for
//! every write.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

use crate::note::{Key, NoteDoc, Origin, Revision, Version, random_u64};
use crate::session::Docs;
use crate::vault;

const INDEX: &str = "index.log";
const NOTES: &str = "notes";
const VAULT: &str = "vault.loro";
const APPLIED: &str = "vault.applied";
const STAMP: &str = "version";

/// How the replica's files are laid out and what they hold, kept in
/// `<dir>/version`. Raised when a release changes either in a way an
/// older one would misread, with a step in [`UPGRADES`] from the one
/// before. A replica with no stamp was made before there were stamps, as
/// layout 1.
pub const LAYOUT: u32 = 1;

/// A step from one layout to the next: given the replica's folder, it
/// changes the files as the next layout has them.
type Upgrade = fn(&Path) -> io::Result<()>;

/// The steps, oldest first: `UPGRADES[n - 1]` takes layout n to n + 1.
/// A step may be cut short (a crash, the power) and is run again from the
/// start, so each must be safe to run twice: the stamp moves on only once
/// it is done.
const UPGRADES: &[Upgrade] = &[];
const _: () = assert!(UPGRADES.len() + 1 == LAYOUT as usize, "a step per layout");
/// Docs kept in memory: the open note and the few saved around it.
const CACHED: usize = 32;

/// What the replica knows of a note's file.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Relative to the notes folder, `/` between the parts.
    pub path: String,
    /// The file's text as last written or read here ([`text_hash`]): what
    /// tells a file changed while the app was closed, and finds a file
    /// renamed by another app.
    pub hash: u64,
    /// The doc's version when the file held that text: the base of a
    /// three-way merge.
    pub version: Version,
}

pub struct Replica {
    dir: PathBuf,
    /// This run's writer: a fresh one each time the app opens, so two
    /// runs never write as one peer (docs/SYNC.md §5).
    peer: u64,
    entries: HashMap<Key, Entry>,
    /// Lowercased path to key: file systems people use compare names
    /// without case.
    by_path: HashMap<String, Key>,
    log: File,
    cache: HashMap<Key, NoteDoc>,
    writer: Writer,
    /// Docs sync asked for under a name that is not a key: kept apart,
    /// never written.
    strays: HashMap<String, NoteDoc>,
    vault: NoteDoc,
    /// The vault's version as last applied to the files.
    applied: Version,
    /// The device writing here, named in each change; none with sync off.
    author: Option<Vec<u8>>,
}

impl Replica {
    /// The replica in `dir`, made if it is not there, and brought up to
    /// this release's layout if it is older. One from a newer release is
    /// left untouched: the error is a [`Newer`].
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        settle(dir, UPGRADES, LAYOUT)?;
        fs::create_dir_all(dir.join(NOTES))?;
        let index = dir.join(INDEX);
        let text = match fs::read_to_string(&index) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let mut entries = HashMap::new();
        let mut lines = 0;
        for line in text.lines() {
            lines += 1;
            if let Some((key, entry)) = parse_line(line) {
                entries.insert(key, entry);
            } else if let Some(key) = parse_gone(line) {
                entries.remove(&key);
            }
        }
        if lines > 2 * entries.len() + 64 {
            let mut out = String::new();
            for (key, entry) in &entries {
                out.push_str(&format_line(*key, entry));
            }
            let tmp = dir.join(format!("{INDEX}.tmp"));
            fs::write(&tmp, out)?;
            fs::rename(&tmp, &index)?;
        }
        let by_path = entries
            .iter()
            .map(|(k, e)| (e.path.to_lowercase(), *k))
            .collect();
        let log = OpenOptions::new().create(true).append(true).open(&index)?;
        let peer = random_u64();
        let vault = fs::read(dir.join(VAULT)).ok().and_then(|b| NoteDoc::load(&b, peer).ok()).unwrap_or_else(|| NoteDoc::new(peer));
        let applied = fs::read(dir.join(APPLIED)).ok().and_then(|b| Version::from_bytes(&b)).unwrap_or_default();
        Ok(Replica {
            dir: dir.to_path_buf(),
            peer,
            entries,
            by_path,
            log,
            cache: HashMap::new(),
            writer: Writer::start(dir.join(NOTES)),
            strays: HashMap::new(),
            vault,
            applied,
            author: None,
        })
    }

    /// Where it is kept.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The vault doc (vault.rs).
    pub fn vault(&self) -> &NoteDoc {
        &self.vault
    }

    /// The vault doc kept: after a change here, or from another device.
    pub fn persist_vault(&self) -> io::Result<()> {
        write_atomic(&self.dir.join(VAULT), &self.vault.to_bytes())
    }

    /// The vault's version as last applied to the files.
    pub fn applied(&self) -> &Version {
        &self.applied
    }

    /// The files now are as the vault is: its version now is the one last
    /// applied.
    pub fn set_applied(&mut self) -> io::Result<()> {
        self.applied = self.vault.version();
        write_atomic(&self.dir.join(APPLIED), &self.applied.to_bytes())
    }

    pub fn entry(&self, key: Key) -> Option<&Entry> {
        self.entries.get(&key)
    }

    /// Which note the file at `path` holding `text` is: the one last seen
    /// at that path, or else the one last seen holding exactly this text at
    /// a path where no file is now (renamed in another app) — when only one
    /// note fits, and `taken` says no note open now has it.
    pub fn key_for(
        &self,
        path: &str,
        text: &str,
        taken: impl Fn(Key) -> bool,
        exists: impl Fn(&str) -> bool,
    ) -> Option<Key> {
        if let Some(&k) = self.by_path.get(&path.to_lowercase())
            && !taken(k)
        {
            return Some(k);
        }
        let hash = text_hash(text);
        let mut moved = self
            .entries
            .iter()
            .filter(|(k, e)| e.hash == hash && !taken(**k) && !exists(&e.path))
            .map(|(k, _)| *k);
        match (moved.next(), moved.next()) {
            (Some(k), None) => Some(k),
            _ => None,
        }
    }

    /// The file at `path` holds `text` now: written here, or read from
    /// disk. A text the note did not hold yet becomes a version, from
    /// `origin` (or [`Origin::Import`] for a note seen the first time).
    pub fn record(&mut self, key: Key, path: &str, text: &str, origin: Origin) -> io::Result<()> {
        let hash = text_hash(text);
        if let Some(e) = self.entries.get(&key)
            && e.hash == hash
        {
            return if e.path == path { Ok(()) } else { self.moved(key, path) };
        }
        let doc = self.doc(key);
        let origin = if doc.text().is_empty() { Origin::Import } else { origin };
        if doc.record(text, origin) {
            self.write_doc(key)?;
        }
        let version = self.cache[&key].version();
        self.set(key, Entry { path: path.to_string(), hash, version })
    }

    /// The note's file moved to `path`, unchanged.
    pub fn moved(&mut self, key: Key, path: &str) -> io::Result<()> {
        let Some(mut e) = self.entries.get(&key).cloned() else { return Ok(()) };
        e.path = path.to_string();
        self.set(key, e)
    }

    /// The note's file was changed elsewhere to `theirs` while `ours` was
    /// being written here: both become changes from the version the file
    /// last held, and the merged text comes back, to be written and then
    /// [`record`](Replica::record)ed. `None` when the replica never saw this
    /// note's file, so has no version to merge from.
    pub fn merge(&mut self, key: Key, theirs: &str, ours: &str) -> io::Result<Option<String>> {
        let Some(base) = self.entries.get(&key).map(|e| e.version.clone()) else {
            return Ok(None);
        };
        let doc = self.doc(key);
        let merged = doc.merge(&base, theirs, ours).map_err(io::Error::other)?;
        self.write_doc(key)?;
        Ok(Some(merged))
    }

    /// The note and its history are gone from the replica, as if never
    /// seen: its entry, its doc, and so its room, which sync no longer
    /// sends. For a note that was never anyone's but this device's own,
    /// such as an untouched welcome note on a device joining an account.
    pub fn forget(&mut self, key: Key) -> io::Result<()> {
        if let Some(old) = self.entries.remove(&key) {
            self.log.write_all(format!("{key}\t-\n").as_bytes())?;
            if self.by_path.get(&old.path.to_lowercase()) == Some(&key) {
                self.by_path.remove(&old.path.to_lowercase());
            }
        }
        self.cache.remove(&key);
        self.writer.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
        match fs::remove_file(self.doc_path(key)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Every note the replica has an entry for.
    pub fn keys(&self) -> Vec<Key> {
        self.entries.keys().copied().collect()
    }

    /// The note's text as its doc has it now: what sync brought in, past
    /// what its file holds.
    pub fn text(&mut self, key: Key) -> String {
        self.doc(key).text()
    }

    /// Changes sync brought in, kept: the doc written as it is now.
    pub fn persist(&mut self, key: Key) -> io::Result<()> {
        if self.cache.contains_key(&key) { self.write_doc(key) } else { Ok(()) }
    }

    /// The editor's unsaved `text` for a note another device changed
    /// meanwhile: written as a change from what the file last held,
    /// beside the other device's, and the merged text back. `None` for a
    /// note the replica never saw.
    pub fn absorb(&mut self, key: Key, text: &str) -> io::Result<Option<String>> {
        self.absorb_as(key, text, Origin::Save)
    }

    /// [`absorb`](Replica::absorb), the change named for where it came
    /// from: another app's, for a file changed outside before it was read.
    pub fn absorb_as(&mut self, key: Key, text: &str, origin: Origin) -> io::Result<Option<String>> {
        let Some(base) = self.entries.get(&key).map(|e| e.version.clone()) else { return Ok(None) };
        let merged = self.doc(key).apply_from(&base, text, origin).map_err(io::Error::other)?;
        self.write_doc(key)?;
        Ok(Some(merged))
    }

    /// Whether `needle` is anywhere in what the note's doc keeps, history
    /// and all, as its bytes have it: a quick way to pass over notes that
    /// never held a text (text typed a letter at a time may be kept in
    /// pieces, so `true` only says it may have).
    pub fn may_have_held(&mut self, key: Key, needle: &str) -> bool {
        if !self.entries.contains_key(&key) || needle.is_empty() {
            return false;
        }
        let bytes = self.doc(key).to_bytes();
        bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
    }

    /// Every version of the note, newest first; none for a note the
    /// replica never saw.
    pub fn history(&mut self, key: Key) -> Vec<Revision> {
        if !self.entries.contains_key(&key) {
            return Vec::new();
        }
        self.doc(key).history()
    }

    /// The note's text at `version`.
    pub fn text_at(&mut self, key: Key, version: &Version) -> io::Result<String> {
        self.doc(key).text_at(version).map_err(io::Error::other)
    }

    /// The note becomes what it was at `version`, as a new version, and
    /// its text comes back, to be written and then
    /// [`record`](Replica::record)ed.
    pub fn restore(&mut self, key: Key, version: &Version) -> io::Result<String> {
        let doc = self.doc(key);
        let changed = doc.restore(version).map_err(io::Error::other)?;
        let text = doc.text();
        if changed {
            self.write_doc(key)?;
        }
        Ok(text)
    }

    /// The note's doc: in memory, else from its file, else a new one. A
    /// file that does not decode is set aside, not lost.
    fn doc(&mut self, key: Key) -> &NoteDoc {
        if !self.cache.contains_key(&key) {
            if self.cache.len() >= CACHED {
                self.cache.clear();
            }
            let path = self.doc_path(key);
            let waiting = self.writer.waiting(key);
            let doc = match waiting.map_or_else(|| fs::read(&path), Ok) {
                Ok(bytes) => NoteDoc::load(&bytes, self.peer).unwrap_or_else(|_| {
                    let _ = fs::rename(&path, path.with_extension("loro.bad"));
                    NoteDoc::new(self.peer)
                }),
                Err(_) => NoteDoc::new(self.peer),
            };
            doc.set_author(self.author.as_deref());
            self.cache.insert(key, doc);
        }
        &self.cache[&key]
    }

    /// The device writing here from now on (sync on), named in each change
    /// a note's history lists; `None`, nobody (sync off).
    pub fn set_author(&mut self, device: Option<&[u8]>) {
        if self.author.as_deref() == device {
            return;
        }
        self.author = device.map(<[u8]>::to_vec);
        for doc in self.cache.values().chain([&self.vault]) {
            doc.set_author(device);
        }
    }

    fn doc_path(&self, key: Key) -> PathBuf {
        self.dir.join(NOTES).join(format!("{key}.loro"))
    }

    fn write_doc(&self, key: Key) -> io::Result<()> {
        self.writer.write(key, self.cache[&key].to_bytes());
        Ok(())
    }

    fn set(&mut self, key: Key, entry: Entry) -> io::Result<()> {
        self.log.write_all(format_line(key, &entry).as_bytes())?;
        if let Some(old) = self.entries.get(&key)
            && self.by_path.get(&old.path.to_lowercase()) == Some(&key)
        {
            self.by_path.remove(&old.path.to_lowercase());
        }
        self.by_path.insert(entry.path.to_lowercase(), key);
        self.entries.insert(key, entry);
        Ok(())
    }
}

/// Sync's view of the replica: a room is a note, by its key in hex.
impl Docs for Replica {
    fn rooms(&self) -> Vec<String> {
        let mut rooms: Vec<String> = self.entries.keys().chain(self.cache.keys()).map(Key::to_string).collect();
        rooms.sort_unstable();
        rooms.dedup();
        // The vault first: where the notes go comes before the notes.
        rooms.insert(0, vault::ROOM.to_string());
        rooms
    }

    fn doc(&mut self, room: &str) -> &NoteDoc {
        match Key::parse(room) {
            Some(key) => Replica::doc(self, key),
            None if room == vault::ROOM => &self.vault,
            None => {
                let peer = self.peer;
                self.strays.entry(room.to_string()).or_insert_with(|| NoteDoc::new(peer))
            }
        }
    }
}

/// The thread that writes docs. Only the newest bytes for a doc are kept
/// while it waits, so a burst of saves to one note is one write.
struct Writer {
    waiting: Arc<Mutex<HashMap<Key, Vec<u8>>>>,
    tx: Option<mpsc::Sender<Key>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    fn start(dir: PathBuf) -> Self {
        let waiting: Arc<Mutex<HashMap<Key, Vec<u8>>>> = Arc::default();
        let (tx, rx) = mpsc::channel::<Key>();
        let shared = waiting.clone();
        let thread = std::thread::Builder::new()
            .name("carnet-replica".into())
            .spawn(move || {
                for key in rx {
                    let Some(bytes) = shared.lock().map_or(None, |w| w.get(&key).cloned()) else {
                        continue;
                    };
                    let path = dir.join(format!("{key}.loro"));
                    let tmp = path.with_extension("loro.tmp");
                    // A write that fails leaves the replica behind, not
                    // wrong: the files are the truth.
                    let _ = fs::write(&tmp, &bytes).and_then(|()| fs::rename(&tmp, &path));
                    // Done, unless newer bytes came while it was written;
                    // and taken off again if the note was forgotten
                    // meanwhile (`forget` found no file to remove yet).
                    if let Ok(mut w) = shared.lock() {
                        match w.get(&key) {
                            Some(newest) if *newest == bytes => {
                                w.remove(&key);
                            }
                            Some(_) => {}
                            None => {
                                let _ = fs::remove_file(&path);
                            }
                        }
                    }
                }
            })
            .ok();
        Writer { waiting, tx: thread.is_some().then_some(tx), thread }
    }

    fn write(&self, key: Key, bytes: Vec<u8>) {
        if let Ok(mut w) = self.waiting.lock() {
            w.insert(key, bytes);
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(key);
        }
    }

    /// A doc handed over and not on disk yet.
    fn waiting(&self, key: Key) -> Option<Vec<u8>> {
        self.waiting.lock().ok()?.get(&key).cloned()
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// FNV-1a over the text's bytes: stable across runs and Rust versions,
/// which `std`'s hasher does not promise.
pub fn text_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3))
}

/// A replica laid out by a newer Carnet than this one: not opened, nor
/// changed. The notes still open from their files, without history or
/// sync, until Carnet is updated.
#[derive(Debug, PartialEq)]
pub struct Newer(pub u32);

impl Newer {
    /// The [`Newer`] an error from [`Replica::open`] is, if it is one.
    pub fn of(e: &io::Error) -> Option<&Newer> {
        e.get_ref()?.downcast_ref()
    }
}

impl fmt::Display for Newer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("These notes' history was written by a newer Carnet. Update Carnet to open it.")
    }
}

impl std::error::Error for Newer {}

/// The layout the replica in `dir` is in: its stamp, or, with none, 1
/// for one made before there were stamps and `layout` for a new one.
fn stamp(dir: &Path, layout: u32) -> io::Result<(u32, bool)> {
    match fs::read_to_string(dir.join(STAMP)) {
        Ok(t) => match t.trim().parse() {
            Ok(n) if n >= 1 => Ok((n, true)),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "the replica's version doesn't read")),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let old = [INDEX, NOTES, VAULT, APPLIED].iter().any(|f| dir.join(f).exists());
            Ok((if old { 1 } else { layout }, false))
        }
        Err(e) => Err(e),
    }
}

/// The replica in `dir` brought to `layout` by `steps`, stamped after
/// each; or, stamped newer than `layout`, left alone and refused.
fn settle(dir: &Path, steps: &[Upgrade], layout: u32) -> io::Result<()> {
    let (mut at, stamped) = stamp(dir, layout)?;
    if at > layout {
        return Err(io::Error::other(Newer(at)));
    }
    if at == layout && !stamped {
        return set_stamp(dir, layout);
    }
    while at < layout {
        steps[at as usize - 1](dir)?;
        at += 1;
        set_stamp(dir, at)?;
    }
    Ok(())
}

/// The stamp, whole or not at all, and on the disk before anything goes
/// on: a step is never taken as done that is not.
fn set_stamp(dir: &Path, layout: u32) -> io::Result<()> {
    let path = dir.join(STAMP);
    let tmp = path.with_extension("tmp");
    let mut f = File::create(&tmp)?;
    f.write_all(format!("{layout}\n").as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, &path)?;
    // The rename itself, kept: a folder can be synced on Unix only.
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    Ok(())
}

/// `bytes` at `path`, whole or not at all: written beside it, then moved.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// `key \t hash \t version \t path`, the path last: it is the one part
/// that could hold a tab.
fn format_line(key: Key, e: &Entry) -> String {
    let version: String = e.version.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    format!("{key}\t{:016x}\t{version}\t{}\n", e.hash, e.path)
}

/// A note [forgotten](Replica::forget): its key, then `-`.
fn parse_gone(line: &str) -> Option<Key> {
    let (key, rest) = line.split_once('\t')?;
    (rest == "-").then(|| Key::parse(key)).flatten()
}

fn parse_line(line: &str) -> Option<(Key, Entry)> {
    let mut parts = line.splitn(4, '\t');
    let key = Key::parse(parts.next()?)?;
    let hash = u64::from_str_radix(parts.next()?, 16).ok()?;
    let hex = parts.next()?;
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()))
        .collect::<Option<Vec<u8>>>()?;
    let version = Version::from_bytes(&bytes)?;
    let path = parts.next()?.to_string();
    Some((key, Entry { path, hash, version }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("carnet-replica-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_note_forgotten_is_gone_for_good() {
        let dir = temp("forget");
        let (a, b) = (Key::random(), Key::random());
        {
            let mut r = Replica::open(&dir).unwrap();
            r.record(a, "a.md", "kept\n", Origin::Save).unwrap();
            r.record(b, "b.md", "welcome\n", Origin::Save).unwrap();
            r.forget(b).unwrap();
            assert_eq!(r.keys(), [a]);
            assert_eq!(r.rooms(), [crate::vault::ROOM.to_string(), a.to_string()], "not sent (the vault, always)");
            assert_eq!(r.key_for("b.md", "welcome\n", |_| false, |_| false), None);
        }
        let mut r = Replica::open(&dir).unwrap();
        assert_eq!(r.keys(), [a], "nor back after a restart");
        assert_eq!(r.text(a), "kept\n");
        assert!(!r.doc_path(b).exists());
        r.forget(b).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_are_remembered_across_runs() {
        let dir = temp("runs");
        let k = Key::random();
        {
            let mut r = Replica::open(&dir).unwrap();
            r.record(k, "Shop.md", "# Shop\n\n- milk\n", Origin::Save).unwrap();
            r.record(k, "Shop.md", "# Shop\n\n- milk\n- eggs\n", Origin::Save).unwrap();
        }
        let mut r = Replica::open(&dir).unwrap();
        let e = r.entry(k).unwrap().clone();
        assert_eq!((e.path.as_str(), e.hash), ("Shop.md", text_hash("# Shop\n\n- milk\n- eggs\n")));
        assert_eq!(r.doc(k).text(), "# Shop\n\n- milk\n- eggs\n");
        assert_eq!(r.key_for("shop.MD", "", |_| false, |_| true), Some(k), "paths compare without case");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_renamed_elsewhere_is_the_same_note() {
        let dir = temp("rename");
        let mut r = Replica::open(&dir).unwrap();
        let (a, b) = (Key::random(), Key::random());
        r.record(a, "Old name.md", "# Plan\n", Origin::Save).unwrap();
        r.record(b, "Other.md", "# Other\n", Origin::Save).unwrap();
        // Old name.md is gone, New name.md holds what it held.
        let gone = |p: &str| p != "Old name.md";
        assert_eq!(r.key_for("New name.md", "# Plan\n", |_| false, gone), Some(a));
        // Not when it is still there: that is a copy.
        assert_eq!(r.key_for("Copy.md", "# Plan\n", |_| false, |_| true), None);
        // Not when that note is open already.
        assert_eq!(r.key_for("New name.md", "# Plan\n", |k| k == a, gone), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_index_is_compacted_and_a_bad_doc_set_aside() {
        let dir = temp("compact");
        let k = Key::random();
        {
            let mut r = Replica::open(&dir).unwrap();
            for i in 0..200 {
                r.record(k, "Log.md", &format!("line {i}\n"), Origin::Save).unwrap();
            }
        }
        let _ = Replica::open(&dir).unwrap();
        let lines = fs::read_to_string(dir.join(INDEX)).unwrap().lines().count();
        assert_eq!(lines, 1);
        fs::write(dir.join(NOTES).join(format!("{k}.loro")), b"not a doc").unwrap();
        let mut r = Replica::open(&dir).unwrap();
        r.record(k, "Log.md", "rebuilt\n", Origin::Save).unwrap();
        assert_eq!(r.doc(k).text(), "rebuilt\n");
        assert!(dir.join(NOTES).join(format!("{k}.loro.bad")).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Every file under `dir`, and what it holds.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p.clone());
                    out.push((p, Vec::new()));
                } else {
                    out.push((p.clone(), fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_replica_from_before_stamps_is_stamped_and_reads_as_it_did() {
        let dir = temp("unstamped");
        let k = Key::random();
        {
            let mut r = Replica::open(&dir).unwrap();
            r.record(k, "Plan.md", "# Plan\n", Origin::Save).unwrap();
        }
        assert_eq!(fs::read_to_string(dir.join(STAMP)).unwrap(), format!("{LAYOUT}\n"), "a new one, stamped");
        // As a release before stamps left it.
        fs::remove_file(dir.join(STAMP)).unwrap();
        let mut r = Replica::open(&dir).unwrap();
        assert_eq!(r.text(k), "# Plan\n");
        assert_eq!(fs::read_to_string(dir.join(STAMP)).unwrap(), "1\n");
        drop(r);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_older_layout_is_brought_up_a_step_at_a_time() {
        let dir = temp("upgrade");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(INDEX), "").unwrap();
        // Layout 1 to 2 puts a file down; 2 to 3 fails the first time.
        fn one(d: &Path) -> io::Result<()> {
            fs::write(d.join("two"), "2")
        }
        fn two(d: &Path) -> io::Result<()> {
            if !d.join("try").exists() {
                fs::write(d.join("try"), "")?;
                return Err(io::Error::other("cut short"));
            }
            fs::write(d.join("three"), "3")
        }
        let steps: &[Upgrade] = &[one, two];
        assert!(settle(&dir, steps, 3).is_err());
        assert_eq!(fs::read_to_string(dir.join(STAMP)).unwrap(), "2\n", "the step done, kept");
        assert!(!dir.join("three").exists());
        settle(&dir, steps, 3).unwrap();
        assert_eq!(fs::read_to_string(dir.join(STAMP)).unwrap(), "3\n");
        assert!(dir.join("three").exists());
        // Done: no step runs again.
        fs::remove_file(dir.join("two")).unwrap();
        settle(&dir, steps, 3).unwrap();
        assert!(!dir.join("two").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_newer_layout_is_refused_and_left_as_it_is() {
        let dir = temp("newer");
        let k = Key::random();
        {
            let mut r = Replica::open(&dir).unwrap();
            r.record(k, "Plan.md", "# Plan\n", Origin::Save).unwrap();
        }
        fs::write(dir.join(STAMP), format!("{}\n", LAYOUT + 1)).unwrap();
        let before = snapshot(&dir);
        let Err(e) = Replica::open(&dir) else { panic!("opened a newer replica") };
        assert_eq!(Newer::of(&e), Some(&Newer(LAYOUT + 1)));
        assert!(e.to_string().contains("newer Carnet"));
        assert_eq!(snapshot(&dir), before, "untouched");
        fs::write(dir.join(STAMP), "two\n").unwrap();
        assert!(Replica::open(&dir).is_err_and(|e| Newer::of(&e).is_none()), "nor one it can't read");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_merge_starts_from_what_the_file_last_held() {
        let dir = temp("merge");
        let mut r = Replica::open(&dir).unwrap();
        let k = Key::random();
        r.record(k, "Shop.md", "- milk\n- eggs\n", Origin::Save).unwrap();
        let merged = r.merge(k, "- milk\n- [x] eggs\n", "- oat milk\n- eggs\n").unwrap().unwrap();
        assert_eq!(merged, "- oat milk\n- [x] eggs\n");
        r.record(k, "Shop.md", &merged, Origin::Save).unwrap();
        assert_eq!(r.entry(k).unwrap().hash, text_hash(&merged));
        assert_eq!(r.merge(Key::random(), "a", "b").unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
