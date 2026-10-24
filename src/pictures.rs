//! Pictures between devices (docs/SYNC.md §4): the files in the notes
//! folder's `attachments/`, Carnet's own, each a sealed blob on the server
//! by its id, and the vault doc saying which picture is at which path. Any
//! other picture a note shows is the user's, and stays on its device
//! (store_owned.rs).
//!
//! A picture here and not in the vault is offered; the server says
//! whether it lacks it, and if so it is sent. Only once the server has it
//! does the vault say the path holds it, so a device never asks for a
//! picture not there yet. A picture in the vault and not here is fetched,
//! opened (it must be the picture its id names), and written.
//!
//! A picture goes when the last note showing it is deleted forever
//! (`Store::delete_forever`): its path dropped in the vault, and the server
//! told it may drop it. Dropped on another device, its file here goes too,
//! unless a note here shows it: then it's wanted, and offered again, which
//! keeps it on the server.
//!
//! Hashing a picture to know its id is done once: an index of path, size,
//! modified time and id is kept beside the replica (`pictures.txt`).
//!
//! A picture's id is made under the account's vault key, and the notes
//! folder (its replica, this index) outlives an account: set up anew, with
//! another server or afresh, its pictures have other ids there, and none of
//! them is on that server yet. So the index says whose ids it holds, and
//! one made under another key is started again: every picture hashed
//! anew, offered, and named in the vault by its new id.
//!
//! A Carnet before this one synced the pictures beside notes too. What it
//! named of those in the vault is let go (dropped there, so the server
//! may drop it); the files stay where they are, the user's.
//!
//! Only pictures and documents sync ([`syncs`]): a file of `attachments/`
//! by a picture's name, as an import names one, or a document's (a PDF,
//! a sheet, a recording: the kinds that open in an app of their own and
//! never run). Another device (or one that isn't what it seems) can't put
//! a program, a script or a page in the notes folder by naming it in the
//! vault. Documents are named in a map of the vault's own (vault.rs), as
//! a Carnet from before them would let them go in the pictures'. And what
//! comes is marked as from elsewhere ([`quarantine`]), as a download is:
//! on macOS the Finder asks before opening it, on Windows Office opens
//! it in Protected View.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use carnet_sync::crypto::{BlobId, MAX_BLOB, Random};
use carnet_sync::session::{Event, Session};

use crate::store::{Store, is_owned};

/// Whether a path from the notes folder is one sync puts up or brings
/// down: Carnet's ([`is_owned`]), and a picture by its name, of the kinds
/// an import takes ([`carnet_core::doc::is_picture_name`]), or a document
/// a note links to, of the kinds that open in an app of their own and
/// never run ([`carnet_core::doc::is_document_name`]).
pub fn syncs(rel: &str) -> bool {
    is_owned(rel) && (carnet_core::doc::is_picture_name(rel) || carnet_core::doc::is_document_name(rel))
}

/// A picture come by sync, written at `path`: whole or not at all, and
/// on macOS marked as from elsewhere before it's there to open.
fn write_received(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp-carnet");
    fs::write(&tmp, bytes)?;
    quarantine(&tmp);
    fs::rename(&tmp, path)?;
    crate::store::mark_own(path);
    Ok(())
}

/// Marks a file as come from elsewhere (macOS's `com.apple.quarantine`,
/// as a browser marks a download), so the Finder asks before opening it.
/// Not marked, it's still a picture; so a failure is let be.
#[cfg(target_os = "macos")]
pub fn quarantine(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn setxattr(path: *const std::ffi::c_char, name: *const std::ffi::c_char, value: *const std::ffi::c_void, size: usize, position: u32, options: std::ffi::c_int) -> std::ffi::c_int;
    }
    let Ok(p) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return };
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // Flags (0081: from elsewhere, not yet opened), when, and by whom.
    let value = format!("0081;{secs:08x};Carnet;");
    // SAFETY: both names are NUL-terminated and live through the call;
    // the value is `value.len()` bytes, read only.
    unsafe {
        setxattr(p.as_ptr(), c"com.apple.quarantine".as_ptr(), value.as_ptr().cast(), value.len(), 0, 0);
    }
}

/// On Windows the mark is the Mark of the Web: a `Zone.Identifier`
/// stream beside the file, the Internet's zone, as a browser leaves on a
/// download, so Office opens a document in Protected View and Windows
/// warns before opening what it doubts. Not marked, it's still the file;
/// so a failure is let be (a disk without such streams, FAT).
#[cfg(windows)]
pub fn quarantine(path: &Path) {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    let _ = fs::write(PathBuf::from(stream), "[ZoneTransfer]\r\nZoneId=3\r\n");
}

/// Elsewhere there's no such mark: nothing to do.
#[cfg(not(any(target_os = "macos", windows)))]
pub fn quarantine(_: &Path) {}

/// The index's first line: whose ids it holds ([`Pictures::mark`]).
const UNDER: &str = "under\t";

/// A picture asked for and not come is asked again after this; one
/// offered and not answered, offered again.
const RETRY: Duration = Duration::from_secs(30);

/// A picture the server held back (the account full, or read only) is
/// offered again after this, though nothing said there's room: room may
/// have been made without a word (pictures deleted on another device).
/// Said there's room ([`Pictures::retry`]), it's offered at once.
const HELD_AGAIN: Duration = Duration::from_secs(5 * 60);

/// A picture whose sending failed is offered again after a wait: this
/// long the first time, twice as long each time after, up to
/// [`PUT_WAIT_MOST`]; back to the first once one goes.
const PUT_WAIT_FIRST: Duration = Duration::from_secs(15);
const PUT_WAIT_MOST: Duration = Duration::from_secs(5 * 60);

/// Why pictures wait when the server's picture storage refuses them.
pub use carnet_sync::session::PICTURES_TURNED_AWAY as TURNED_AWAY;

/// Whether a failed send (`error`, as the transfer has it: `HTTP` and the
/// status, or other words) won't go however often it's tried: then why,
/// in words. None for a failure that passes (offline, a timeout, the
/// storage busy or down a while).
fn lasting(error: &str) -> Option<&'static str> {
    let code: u16 = error.strip_prefix("HTTP ")?.trim().parse().ok()?;
    ((400..500).contains(&code) && code != 408 && code != 429).then_some(TURNED_AWAY)
}

#[derive(Clone, Debug, PartialEq)]
struct Known {
    size: u64,
    /// Modified, in milliseconds since 1970.
    modified: u64,
    id: BlobId,
    /// The server has it.
    stored: bool,
}

#[derive(Default)]
pub struct Pictures {
    index: BTreeMap<String, Known>,
    /// Whose ids the index holds: [`Pictures::mark`] of the session they
    /// were made in. `None` in an index from before it was kept.
    under: Option<String>,
    /// Where the index is kept; none in tests that don't keep it.
    file: Option<PathBuf>,
    /// Offered, and when: the server's word awaited.
    offered: HashMap<BlobId, Instant>,
    /// Wanted by the server, and being sent.
    sending: HashSet<BlobId>,
    /// Offered and held back by the server (the account full, or read
    /// only), and when: offered again once there's room.
    held: HashMap<BlobId, Instant>,
    /// Asked for, and when.
    asked: HashMap<BlobId, Instant>,
    /// The vault's pictures not here, by path, as the last round found
    /// them: on their way, unless given up ([`Pictures::coming`]).
    missing: HashMap<String, BlobId>,
    /// Asked for, and answered with nothing to show: the server hasn't
    /// it, or what came isn't the picture its id names. Asked again all
    /// the same, in case; forgotten once it comes.
    gone: HashSet<BlobId>,
    /// A fetch's failure, as the transfer had it: whether it will last
    /// (a 4xx), until the session says the fetch failed.
    get_failed: HashMap<BlobId, bool>,
    /// Sent and failed: offered again once [`Pictures::put_after`].
    failed: HashSet<BlobId>,
    /// When those are offered again, and how long the wait was.
    put_after: Option<Instant>,
    put_wait: Duration,
    /// Why the pictures waiting wait, when it won't mend by itself
    /// ([`TURNED_AWAY`]): for Settings to say. Forgotten once one goes, or
    /// at a new login.
    held_why: Option<&'static str>,
    /// Dropped pictures a note shows again (a version brought back),
    /// asked for by the id they had: the paths each goes back to.
    reviving: HashMap<BlobId, Vec<String>>,
    /// The vault's pictures that aren't Carnet's let go this run.
    let_go: bool,
    /// Pictures written here from another device since last asked: the
    /// app shows them.
    pub arrived: bool,
}

pub fn hex(id: &BlobId) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<BlobId> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Pictures {
    /// With its index kept in `dir` (the replica's folder).
    pub fn open(dir: Option<&Path>) -> Self {
        let file = dir.map(|d| d.join("pictures.txt"));
        let mut p = Pictures { file, ..Default::default() };
        if let Some(text) = p.file.as_ref().and_then(|f| fs::read_to_string(f).ok()) {
            for line in text.lines() {
                if let Some(mark) = line.strip_prefix(UNDER) {
                    p.under = Some(mark.to_string());
                    continue;
                }
                let mut f = line.split('\t');
                let (Some(path), Some(size), Some(modified), Some(id), Some(stored)) = (f.next(), f.next(), f.next(), f.next(), f.next()) else {
                    continue;
                };
                let (Ok(size), Ok(modified), Some(id)) = (size.parse(), modified.parse(), unhex(id)) else { continue };
                p.index.insert(path.to_string(), Known { size, modified, id, stored: stored == "1" });
            }
        }
        p
    }

    fn keep(&self) {
        let Some(file) = &self.file else { return };
        // Its first line says whose ids these are; a Carnet from before
        // reads it as a line that isn't a picture's, and skips it.
        let under = self.under.iter().map(|m| format!("{UNDER}{m}\n"));
        let text: String = under
            .chain(
                self.index
                    .iter()
                    .filter(|(p, _)| !p.contains(['\t', '\n']))
                    .map(|(p, k)| format!("{p}\t{}\t{}\t{}\t{}\n", k.size, k.modified, hex(&k.id), u8::from(k.stored))),
            )
            .collect();
        let tmp = file.with_extension("tmp");
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, file);
        }
    }

    /// The pictures in the attachments folder: their paths from the notes
    /// folder (`/` between parts), with their size and modified time.
    fn scan(store: &Store) -> Vec<(String, PathBuf, u64, u64)> {
        store
            .attachments()
            .into_iter()
            .filter_map(|(rel, path)| {
                let meta = fs::metadata(&path).ok()?;
                (syncs(&rel) && meta.len() as usize <= MAX_BLOB).then(|| (rel, path, meta.len(), meta.modified().map_or(0, millis)))
            })
            .collect()
    }

    /// Whether pictures are on their way, up or down: the sweep waits.
    /// One held back by the server isn't on its way.
    pub fn busy(&self) -> bool {
        let now = Instant::now();
        let recent = |at: &Instant| now.duration_since(*at) < RETRY;
        !self.sending.is_empty() || self.offered.values().any(recent) || !self.reviving.is_empty() || self.asked.values().any(recent)
    }

    /// How many pictures put in here the server hasn't got yet: offered,
    /// being sent, or held back (the account full, or read only).
    pub fn waiting(&self) -> usize {
        let mut ids: HashSet<&BlobId> = self.offered.keys().collect();
        ids.extend(&self.sending);
        ids.extend(self.held.keys());
        ids.extend(&self.failed);
        ids.len()
    }

    /// Why the pictures waiting wait, when it's something that won't mend
    /// by itself, in words; none while none waits.
    pub fn held_why(&self) -> Option<&'static str> {
        self.held_why.filter(|_| self.waiting() > 0)
    }

    /// How a send or a fetch over HTTP went, before the session hears of
    /// it: a send that failed waits before it's offered again, longer
    /// each time; and why, if trying again won't mend it.
    pub fn transferred(&mut self, id: BlobId, put: bool, result: &Result<Vec<u8>, String>) {
        if let (false, Err(error)) = (put, result) {
            self.get_failed.insert(id, lasting(error).is_some());
        }
        let (true, Err(error)) = (put, result) else { return };
        let now = Instant::now();
        self.failed.insert(id);
        // Pictures failing together are one failure: the wait grows once.
        if self.put_after.is_none_or(|at| now >= at) {
            self.put_wait = if self.put_wait.is_zero() { PUT_WAIT_FIRST } else { (self.put_wait * 2).min(PUT_WAIT_MOST) };
            self.put_after = Some(now + self.put_wait);
        }
        self.held_why = lasting(error).or(self.held_why);
    }

    /// One went: the others needn't wait, nor the next failure long.
    fn put_went(&mut self) {
        self.failed.clear();
        self.put_after = None;
        self.put_wait = Duration::ZERO;
        self.held_why = None;
    }

    /// The wait before failed pictures are offered again, as if over.
    #[cfg(test)]
    pub fn wait_over(&mut self) {
        self.put_after = Some(Instant::now());
    }

    /// The wait the last failure set.
    #[cfg(test)]
    pub fn put_wait(&self) -> Duration {
        self.put_wait
    }

    /// Whether the vault's picture at `rel` (a path from the notes folder),
    /// not here, is on its way: asked for, and not given up on. None the
    /// vault doesn't name, or not seen missing yet, is.
    pub fn coming(&self, rel: &str) -> bool {
        self.missing.get(rel).is_some_and(|id| !self.gone.contains(id))
    }

    /// The pictures being sent, held back, or wanted back: paths from the
    /// notes folder.
    pub fn busy_paths(&self) -> BTreeSet<String> {
        let going = |id: &BlobId| self.offered.contains_key(id) || self.sending.contains(id) || self.held.contains_key(id) || self.failed.contains(id);
        let mut out: BTreeSet<String> = self.index.iter().filter(|(_, k)| going(&k.id)).map(|(p, _)| p.clone()).collect();
        out.extend(self.reviving.values().flatten().cloned());
        out
    }

    /// The server held back what was offered (`Refused`: the account
    /// full, or read only): no answer comes for it, so it's not awaited,
    /// and it's offered again once there's room.
    pub fn held_back(&mut self) {
        let now = Instant::now();
        for (id, _) in self.offered.drain() {
            self.held.insert(id, now);
        }
    }

    /// There's room again (the plan raised, the account writable again,
    /// or a new login): what was held back, or offered and not answered,
    /// is offered at the next [`Pictures::tick`].
    pub fn retry(&mut self) {
        self.held.clear();
        self.offered.clear();
        self.put_went();
    }

    /// Whether any picture is held back by the server.
    #[cfg(test)]
    pub fn is_holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// What names the key a session's picture ids are made under: the id
    /// it gives no bytes at all. Two sessions of one account give the
    /// same; of two accounts, not.
    fn mark(session: &Session) -> String {
        hex(&session.blob_id(&[]))
    }

    /// The index, if its ids were made under another key than `session`'s
    /// (another account's; or kept before it said), started again: what
    /// it knew is of no use here.
    fn under(&mut self, session: &Session) {
        let mark = Self::mark(session);
        if self.under.as_deref() == Some(mark.as_str()) {
            return;
        }
        self.index.clear();
        self.offered.clear();
        self.sending.clear();
        self.held.clear();
        self.asked.clear();
        self.missing.clear();
        self.gone.clear();
        self.get_failed.clear();
        self.reviving.clear();
        self.put_went();
        self.under = Some(mark);
        self.keep();
    }

    /// One round: new pictures here hashed and offered, sent ones put in
    /// the vault, missing ones asked for. True if the vault changed.
    pub fn tick(&mut self, store: &mut Store, session: &mut Session) -> bool {
        self.under(session);
        let Some(replica) = store.replica.as_ref() else { return false };
        let vault = replica.vault().pictures();
        let dropped = replica.vault().dropped_pictures();
        let mut changed_index = false;

        // Named by an earlier Carnet, which synced the pictures beside
        // notes too: the user's, let go in the vault, once a run (one such
        // Carnet still running would name them again). Their files stay.
        let mut wrote = false;
        let before = self.index.len();
        self.index.retain(|p, _| syncs(p));
        changed_index |= self.index.len() != before;
        if !std::mem::replace(&mut self.let_go, true) {
            for rel in vault.keys().filter(|p| !syncs(p)) {
                wrote |= store.unname(rel);
            }
        }

        // Dropped on another device: gone here, or, shown by a note here,
        // offered again.
        let mut here = Self::scan(store);
        if here.iter().any(|(rel, ..)| dropped.contains(rel)) {
            let used = store.pictures_in_use();
            here.retain(|(rel, path, ..)| {
                if !dropped.contains(rel) || used.contains(rel) {
                    return true;
                }
                fs::remove_file(path).is_err()
            });
            for (rel, ..) in here.iter().filter(|(rel, ..)| dropped.contains(rel)) {
                if let Some(k) = self.index.get_mut(rel) {
                    k.stored = false;
                }
            }
        }

        // What is here, by id; a file changed since it was hashed, again.
        let present: HashSet<&str> = here.iter().map(|(p, ..)| p.as_str()).collect();
        let before = self.index.len();
        self.index.retain(|p, _| present.contains(p.as_str()));
        changed_index |= self.index.len() != before;
        for (rel, path, size, modified) in &here {
            if self.index.get(rel).is_some_and(|k| k.size == *size && k.modified == *modified) {
                continue;
            }
            let Ok(bytes) = fs::read(path) else { continue };
            let id = session.blob_id(&bytes);
            let stored = vault.get(rel).is_some_and(|v| *v == hex(&id));
            self.index.insert(rel.clone(), Known { size: *size, modified: *modified, id, stored });
            changed_index = true;
        }

        // Here and not (or not so) in the vault: in it if the server has
        // it, else offered.
        // Not while one is awaited or being sent; one held back, after
        // a while, or once there's room.
        let now = Instant::now();
        self.offered.retain(|_, at| now.duration_since(*at) < RETRY);
        self.held.retain(|_, at| now.duration_since(*at) < HELD_AGAIN);
        if self.put_after.is_some_and(|at| now >= at) {
            self.failed.clear();
        }
        let mut offer = Vec::new();
        for (rel, k) in &self.index {
            if vault.get(rel).is_some_and(|v| *v == hex(&k.id)) {
                continue;
            }
            if k.stored {
                wrote |= store.set_picture(rel, &hex(&k.id));
            } else if !self.offered.contains_key(&k.id) && !self.sending.contains(&k.id) && !self.held.contains_key(&k.id) && !self.failed.contains(&k.id) && !offer.iter().any(|(id, _)| *id == k.id) {
                offer.push((k.id, k.size as usize));
            }
        }
        // Offered only if it goes: one not sent (logging in) is offered
        // at the next tick.
        if session.is_ready() {
            self.offered.extend(offer.iter().map(|(id, _)| (*id, now)));
            session.offer_blobs(offer);
        }

        // In the vault and not here: asked for.
        self.missing.clear();
        for (rel, id) in &vault {
            // One beside a note is the user's, on the device that has it.
            if present.contains(rel.as_str()) || !syncs(rel) {
                continue;
            }
            let Some(id) = unhex(id) else { continue };
            self.missing.insert(rel.clone(), id);
            if self.asked.get(&id).is_some_and(|at| now.duration_since(*at) < RETRY) {
                continue;
            }
            self.asked.insert(id, now);
            session.get_blob(id);
        }
        if changed_index {
            self.keep();
        }
        wrote
    }

    /// Of `paths` (what notes just saved show), those dropped whose file
    /// isn't here: asked for again by the id they had, while the server
    /// still keeps it, and put back when they come.
    pub fn want_back(&mut self, store: &Store, session: &mut Session, paths: &std::collections::BTreeSet<String>) {
        let Some(replica) = store.replica.as_ref() else { return };
        for path in paths {
            if !syncs(path) || store.dir.join(path).exists() {
                continue;
            }
            let Some(id) = replica.vault().dropped_id(path).and_then(|h| unhex(&h)) else { continue };
            let wanting = self.reviving.entry(id).or_default();
            if !wanting.contains(path) {
                wanting.push(path.clone());
            }
            session.get_blob(id);
        }
    }

    /// What the session said of a picture. True if the vault changed.
    pub fn event(&mut self, e: &Event, store: &mut Store, session: &mut Session, rng: &mut impl Random) -> io::Result<bool> {
        self.under(session);
        match e {
            Event::BlobWanted(id) => {
                self.offered.remove(id);
                self.held.remove(id);
                let path = self.index.iter().find(|(_, k)| k.id == *id).map(|(p, _)| store.dir.join(p));
                match path.and_then(|p| fs::read(p).ok()) {
                    // Still the picture it was: sent.
                    Some(bytes) if session.blob_id(&bytes) == *id && session.put_blob(&bytes, rng) => {
                        self.sending.insert(*id);
                    }
                    _ => {}
                }
                Ok(false)
            }
            Event::BlobStored(id) => {
                self.offered.remove(id);
                self.sending.remove(id);
                self.held.remove(id);
                self.put_went();
                let mut wrote = false;
                let mut changed = false;
                for (rel, k) in self.index.iter_mut().filter(|(_, k)| k.id == *id) {
                    k.stored = true;
                    changed = true;
                    wrote |= store.set_picture(rel, &hex(id));
                }
                if changed {
                    self.keep();
                }
                Ok(wrote)
            }
            Event::BlobGot(id, bytes) => {
                self.asked.remove(id);
                self.gone.remove(id);
                self.get_failed.remove(id);
                self.missing.retain(|_, m| m != id);
                // Brought back: written where it was, and named again.
                let mut wrote = false;
                for rel in self.reviving.remove(id).unwrap_or_default().into_iter().filter(|p| syncs(p)) {
                    let Ok(path) = store.inside(store.dir.join(&rel)) else { continue };
                    if !path.exists() {
                        write_received(&path, bytes)?;
                        let meta = fs::metadata(&path)?;
                        self.index.insert(rel.clone(), Known { size: meta.len(), modified: meta.modified().map_or(0, millis), id: *id, stored: true });
                        self.arrived = true;
                    }
                    wrote |= store.set_picture(&rel, &hex(id));
                }

                let vault = store.replica.as_ref().map(|r| r.vault().pictures()).unwrap_or_default();
                for rel in vault.iter().filter(|(p, v)| unhex(v) == Some(*id) && syncs(p)).map(|(p, _)| p) {
                    // Its path checked twice over: plain (the vault reads
                    // no other), and in the notes folder once joined.
                    let Ok(path) = store.inside(store.dir.join(rel)) else { continue };
                    if path.exists() {
                        continue;
                    }
                    write_received(&path, bytes)?;
                    let meta = fs::metadata(&path)?;
                    self.index.insert(rel.clone(), Known { size: meta.len(), modified: meta.modified().map_or(0, millis), id: *id, stored: true });
                    self.arrived = true;
                }
                self.keep();
                Ok(wrote)
            }
            // Asked, or offered, again later.
            Event::BlobFailed(id) => {
                // A fetch answered (connected still) with nothing to show,
                // or failed for good: given up on, for now. One cut off,
                // or failing a while (offline, the storage busy), isn't.
                let lasting = self.get_failed.remove(id).unwrap_or_else(|| session.is_ready());
                if self.asked.contains_key(id) && lasting {
                    self.gone.insert(*id);
                }
                self.offered.remove(id);
                self.sending.remove(id);
                self.reviving.remove(id);
                Ok(false)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4xx from the picture storage won't mend by itself; a timeout,
    /// "too many", a 5xx or no answer at all will.
    #[test]
    fn only_a_refusal_is_lasting() {
        for e in ["HTTP 400", "HTTP 401", "HTTP 403", "HTTP 404", "HTTP 413"] {
            assert_eq!(lasting(e), Some(TURNED_AWAY), "{e}");
        }
        for e in ["HTTP 408", "HTTP 429", "HTTP 500", "HTTP 503", "connection refused", "timed out", "HTTP x"] {
            assert_eq!(lasting(e), None, "{e}");
        }
    }
}
