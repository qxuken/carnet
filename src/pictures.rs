//! Pictures between devices (docs/SYNC.md §4): the files in the notes
//! folder's `attachments/`, each a sealed blob on the server by its id,
//! and the vault doc saying which picture is at which path.
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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use carnet_sync::crypto::{BlobId, MAX_BLOB, Random};
use carnet_sync::session::{Event, Session};

use crate::store::{ATTACHMENTS, Store};

/// A picture asked for and not come is asked again after this.
const RETRY: Duration = Duration::from_secs(30);

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
    /// Where the index is kept; none in tests that don't keep it.
    file: Option<PathBuf>,
    /// Offered or being sent, the server's word awaited.
    waiting: HashSet<BlobId>,
    /// Asked for, and when.
    asked: HashMap<BlobId, Instant>,
    /// Dropped pictures a note shows again (a version brought back),
    /// asked for by the id they had: the paths each goes back to.
    reviving: HashMap<BlobId, Vec<String>>,
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
        let text: String = self
            .index
            .iter()
            .filter(|(p, _)| !p.contains(['\t', '\n']))
            .map(|(p, k)| format!("{p}\t{}\t{}\t{}\t{}\n", k.size, k.modified, hex(&k.id), u8::from(k.stored)))
            .collect();
        let tmp = file.with_extension("tmp");
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, file);
        }
    }

    /// The pictures in the notes folder: their paths from it (`/` between
    /// parts), with their size and modified time.
    fn scan(store: &Store) -> Vec<(String, PathBuf, u64, u64)> {
        let mut out = Vec::new();
        let mut stack = vec![(store.dir.join(ATTACHMENTS), ATTACHMENTS.to_string(), 0)];
        while let Some((dir, rel, depth)) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || name.ends_with(".tmp-carnet") {
                    continue;
                }
                let path = e.path();
                let Ok(meta) = e.metadata() else { continue };
                if meta.is_dir() {
                    if depth < 4 {
                        stack.push((path, format!("{rel}/{name}"), depth + 1));
                    }
                } else if meta.len() as usize <= MAX_BLOB {
                    out.push((format!("{rel}/{name}"), path, meta.len(), meta.modified().map_or(0, millis)));
                }
            }
        }
        out
    }

    /// One round: new pictures here hashed and offered, sent ones put in
    /// the vault, missing ones asked for. True if the vault changed.
    pub fn tick(&mut self, store: &mut Store, session: &mut Session) -> bool {
        let Some(replica) = store.replica.as_ref() else { return false };
        let vault = replica.vault().pictures();
        let dropped = replica.vault().dropped_pictures();
        let mut changed_index = false;

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
        let mut wrote = false;
        let mut offer = Vec::new();
        for (rel, k) in &self.index {
            if vault.get(rel).is_some_and(|v| *v == hex(&k.id)) {
                continue;
            }
            if k.stored {
                wrote |= store.set_picture(rel, &hex(&k.id));
            } else if !self.waiting.contains(&k.id) && !offer.contains(&k.id) {
                offer.push(k.id);
            }
        }
        self.waiting.extend(offer.iter().copied());
        session.offer_blobs(offer);

        // In the vault and not here: asked for.
        let now = Instant::now();
        for (rel, id) in &vault {
            if present.contains(rel.as_str()) {
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
            if store.dir.join(path).exists() {
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
        match e {
            Event::BlobWanted(id) => {
                let path = self.index.iter().find(|(_, k)| k.id == *id).map(|(p, _)| store.dir.join(p));
                match path.and_then(|p| fs::read(p).ok()) {
                    // Still the picture it was: sent.
                    Some(bytes) if session.blob_id(&bytes) == *id => {
                        session.put_blob(&bytes, rng);
                    }
                    _ => {
                        self.waiting.remove(id);
                    }
                }
                Ok(false)
            }
            Event::BlobStored(id) => {
                self.waiting.remove(id);
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
                for rel in self.reviving.remove(id).unwrap_or_default() {
                    let path = store.dir.join(&rel);
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
                for (rel, v) in vault.iter().filter(|(_, v)| unhex(v) == Some(*id)) {
                    let _ = v;
                    let path = store.dir.join(rel);
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
                self.waiting.remove(id);
                self.reviving.remove(id);
                Ok(false)
            }
            _ => Ok(false),
        }
    }
}
