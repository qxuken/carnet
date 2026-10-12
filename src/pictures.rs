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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use carnet_sync::crypto::{BlobId, MAX_BLOB, Random};
use carnet_sync::session::{Event, Session};

use crate::store::{Store, is_owned};

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
                (meta.len() as usize <= MAX_BLOB).then(|| (rel, path, meta.len(), meta.modified().map_or(0, millis)))
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
        ids.len()
    }

    /// The pictures being sent, held back, or wanted back: paths from the
    /// notes folder.
    pub fn busy_paths(&self) -> BTreeSet<String> {
        let going = |id: &BlobId| self.offered.contains_key(id) || self.sending.contains(id) || self.held.contains_key(id);
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
        self.reviving.clear();
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
        self.index.retain(|p, _| is_owned(p));
        changed_index |= self.index.len() != before;
        if !std::mem::replace(&mut self.let_go, true) {
            for rel in vault.keys().filter(|p| !is_owned(p)) {
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
        let mut offer = Vec::new();
        for (rel, k) in &self.index {
            if vault.get(rel).is_some_and(|v| *v == hex(&k.id)) {
                continue;
            }
            if k.stored {
                wrote |= store.set_picture(rel, &hex(&k.id));
            } else if !self.offered.contains_key(&k.id) && !self.sending.contains(&k.id) && !self.held.contains_key(&k.id) && !offer.iter().any(|(id, _)| *id == k.id) {
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
        for (rel, id) in &vault {
            // One beside a note is the user's, on the device that has it.
            if present.contains(rel.as_str()) || !is_owned(rel) {
                continue;
            }
            let Some(id) = unhex(id) else { continue };
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
            if !is_owned(path) || store.dir.join(path).exists() {
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
                // Brought back: written where it was, and named again.
                let mut wrote = false;
                for rel in self.reviving.remove(id).unwrap_or_default().into_iter().filter(|p| is_owned(p)) {
                    let Ok(path) = store.inside(store.dir.join(&rel)) else { continue };
                    if !path.exists() {
                        if let Some(dir) = path.parent() {
                            fs::create_dir_all(dir)?;
                        }
                        let tmp = path.with_extension("tmp-carnet");
                        fs::write(&tmp, bytes)?;
                        fs::rename(&tmp, &path)?;
                        let meta = fs::metadata(&path)?;
                        self.index.insert(rel.clone(), Known { size: meta.len(), modified: meta.modified().map_or(0, millis), id: *id, stored: true });
                        self.arrived = true;
                    }
                    wrote |= store.set_picture(&rel, &hex(id));
                }

                let vault = store.replica.as_ref().map(|r| r.vault().pictures()).unwrap_or_default();
                for rel in vault.iter().filter(|(p, v)| unhex(v) == Some(*id) && is_owned(p)).map(|(p, _)| p) {
                    // Its path checked twice over: plain (the vault reads
                    // no other), and in the notes folder once joined.
                    let Ok(path) = store.inside(store.dir.join(rel)) else { continue };
                    if path.exists() {
                        continue;
                    }
                    if let Some(dir) = path.parent() {
                        fs::create_dir_all(dir)?;
                    }
                    let tmp = path.with_extension("tmp-carnet");
                    fs::write(&tmp, bytes)?;
                    fs::rename(&tmp, &path)?;
                    let meta = fs::metadata(&path)?;
                    self.index.insert(rel.clone(), Known { size: meta.len(), modified: meta.modified().map_or(0, millis), id: *id, stored: true });
                    self.arrived = true;
                }
                self.keep();
                Ok(wrote)
            }
            // Asked, or offered, again later.
            Event::BlobFailed(id) => {
                self.offered.remove(id);
                self.sending.remove(id);
                self.reviving.remove(id);
                Ok(false)
            }
            _ => Ok(false),
        }
    }
}
