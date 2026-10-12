//! Pictures (docs/SYNC.md §4, §6). Their bytes go over HTTP, to and from a
//! short-lived signed link (`links.rs`); the socket says only which, and
//! when one is kept. The hub decides who may (a socket logged in, its
//! account) and hands the rest here as a [`Job`]: the [`Worker`], on a
//! thread of its own, answers with links, and checks a picture put is
//! there before it's said to be kept. Where the pictures are is behind
//! [`Blobs`]: files beside the databases for a server of one's own
//! ([`FileBlobs`], its links to this server's `/v1/blobs/…`), object
//! storage for a hosted one (its links pre-signed there), the same to a
//! device either way.
//!
//! A picture no note shows any more is dropped by a device's word
//! (`BlobDrop`): marked, and gone [`KEEP`] later, unless a device offers,
//! asks for or puts it meanwhile. The server can't read the notes to
//! know; the wait is for the device that still has a use for it.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use carnet_sync::crypto::MAX_BLOB;
use carnet_sync::session::Transfer;
use carnet_sync::wire::{self, Carnet, Frame};

use crate::hub::{ConnId, KEEP, Out};
use crate::links::{Grant, LIFETIME, Signer};
use crate::quota::{FULL, Limit, READ_ONLY};

/// The most a sealed picture may be: the largest picture, sealed.
pub const MAX: u64 = (MAX_BLOB + 64) as u64;

/// Picture work the hub has allowed, for the worker to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// Which of `ids` are kept (`BlobStored`), and where to put the rest
    /// (`BlobLink`): while the account may write, and has `room` left for
    /// pictures (`None`: no limit).
    Offer { conn: ConnId, account: Vec<u8>, ids: Vec<[u8; 32]>, room: Option<u64>, writable: bool },
    /// What the account keeps: its `notes`' bytes, as the hub has them,
    /// and its pictures' (`UsageIs`).
    Usage { conn: ConnId, account: Vec<u8>, notes: u64, limit: Limit },
    /// Where to fetch each of `ids` (`BlobLink`), or that it isn't kept
    /// (`BlobNone`).
    Fetch { conn: ConnId, account: Vec<u8>, ids: Vec<[u8; 32]> },
    /// No note shows `ids` any more: each marked, to go [`KEEP`] from
    /// now (`BlobDropped`).
    Drop { conn: ConnId, account: Vec<u8>, ids: Vec<[u8; 32]> },
    /// An account removed: its pictures go.
    Gone { account: Vec<u8> },
}

/// Where pictures are kept: by account and id, sealed.
pub trait Blobs: Send {
    /// Its size, if kept.
    fn size(&mut self, account: &[u8], id: &[u8; 32]) -> Option<u64>;
    /// A link to `PUT` (`put`) or `GET` it, good until `expires`.
    fn link(&mut self, account: &[u8], id: &[u8; 32], put: bool, expires: u64) -> String;
    /// The account was removed: its pictures go.
    fn forget(&mut self, account: &[u8]);
    /// Marked to go, as of `now`, if kept and not marked already.
    fn mark(&mut self, account: &[u8], id: &[u8; 32], now: u64);
    /// Wanted after all: not marked.
    fn unmark(&mut self, account: &[u8], id: &[u8; 32]);
    /// Those marked before `before` gone: how many.
    fn sweep(&mut self, before: u64) -> u64;
    /// The bytes the account's pictures take.
    fn usage(&mut self, account: &[u8]) -> u64;
}

/// Boxed, still one: the server runs files or a bucket alike.
impl<T: Blobs + ?Sized> Blobs for Box<T> {
    fn size(&mut self, account: &[u8], id: &[u8; 32]) -> Option<u64> {
        (**self).size(account, id)
    }
    fn link(&mut self, account: &[u8], id: &[u8; 32], put: bool, expires: u64) -> String {
        (**self).link(account, id, put, expires)
    }
    fn forget(&mut self, account: &[u8]) {
        (**self).forget(account)
    }
    fn mark(&mut self, account: &[u8], id: &[u8; 32], now: u64) {
        (**self).mark(account, id, now)
    }
    fn unmark(&mut self, account: &[u8], id: &[u8; 32]) {
        (**self).unmark(account, id)
    }
    fn sweep(&mut self, before: u64) -> u64 {
        (**self).sweep(before)
    }
    fn usage(&mut self, account: &[u8]) -> u64 {
        (**self).usage(account)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Pictures as files: `data/blobs/<account>/<id>`, each written whole
/// (into `data/spool` first, then moved in), never changed after.
#[derive(Clone, Debug)]
pub struct BlobFiles {
    pub root: PathBuf,
    pub spool: PathBuf,
}

impl BlobFiles {
    /// In `data`; the spool emptied of what a last run left.
    pub fn open(data: &Path) -> io::Result<Self> {
        let files = BlobFiles { root: data.join("blobs"), spool: data.join("spool") };
        fs::create_dir_all(&files.root)?;
        fs::create_dir_all(&files.spool)?;
        for e in fs::read_dir(&files.spool)?.flatten() {
            let _ = fs::remove_file(e.path());
        }
        Ok(files)
    }

    pub fn path(&self, account: &[u8], id: &[u8; 32]) -> PathBuf {
        self.root.join(hex(account)).join(hex(id))
    }

    /// Beside a picture marked to go: when it was, in seconds since 1970.
    fn mark_path(&self, account: &[u8], id: &[u8; 32]) -> PathBuf {
        self.root.join(hex(account)).join(format!("{}.drop", hex(id)))
    }

    /// `id` marked to go, as of `now`: kept, and not marked already.
    pub fn mark(&self, account: &[u8], id: &[u8; 32], now: u64) -> io::Result<()> {
        let mark = self.mark_path(account, id);
        if !self.path(account, id).exists() || mark.exists() {
            return Ok(());
        }
        fs::write(mark, now.to_string())
    }

    /// `id` wanted after all.
    pub fn unmark(&self, account: &[u8], id: &[u8; 32]) {
        let _ = fs::remove_file(self.mark_path(account, id));
    }

    /// Every picture marked before `before`, gone: how many.
    pub fn sweep(&self, before: u64) -> u64 {
        let mut gone = 0;
        for account in fs::read_dir(&self.root).into_iter().flatten().flatten() {
            for e in fs::read_dir(account.path()).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(id) = name.strip_suffix(".drop") else { continue };
                let at = fs::read_to_string(e.path()).ok().and_then(|t| t.trim().parse::<u64>().ok()).unwrap_or(0);
                if at < before {
                    let _ = fs::remove_file(account.path().join(id));
                    let _ = fs::remove_file(e.path());
                    gone += 1;
                }
            }
        }
        gone
    }

    /// `sealed` kept as `id`, unless it is already (a picture never
    /// changes under its id). True if it wasn't.
    pub fn keep(&self, account: &[u8], id: &[u8; 32], sealed: &[u8]) -> io::Result<bool> {
        use std::io::Write;
        let tmp = self.spool.join(format!("{}-{}", hex(account), hex(id)));
        let mut file = fs::File::create(&tmp)?;
        file.write_all(sealed)?;
        file.sync_all()?;
        self.keep_file(account, id, &tmp)
    }

    /// The file at `tmp` (in the spool, written and synced) kept as `id`,
    /// and on the disk when this returns: a power cut after can't lose it.
    /// True if it wasn't kept already.
    pub fn keep_file(&self, account: &[u8], id: &[u8; 32], tmp: &Path) -> io::Result<bool> {
        let to = self.path(account, id);
        if to.exists() {
            fs::remove_file(tmp)?;
            return Ok(false);
        }
        let dir = to.parent().expect("in a folder");
        if !dir.exists() {
            fs::create_dir_all(dir)?;
            sync_dir(&self.root)?;
        }
        fs::rename(tmp, &to)?;
        sync_dir(dir)?;
        Ok(true)
    }
}

/// A folder's list of files on the disk: what makes a file moved into it
/// stay there. (Windows has no such call; its renames are kept as made.)
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// A server's own pictures: files, and links to its own `/v1/blobs/…`.
pub struct FileBlobs {
    pub files: BlobFiles,
    origin: String,
    signer: Arc<Signer>,
}

impl FileBlobs {
    pub fn new(files: BlobFiles, origin: &str, signer: Arc<Signer>) -> Self {
        FileBlobs { files, origin: origin.trim_end_matches('/').to_string(), signer }
    }
}

impl Blobs for FileBlobs {
    fn size(&mut self, account: &[u8], id: &[u8; 32]) -> Option<u64> {
        fs::metadata(self.files.path(account, id)).ok().map(|m| m.len())
    }

    fn link(&mut self, account: &[u8], id: &[u8; 32], put: bool, expires: u64) -> String {
        let token = self.signer.sign(&Grant { account: account.to_vec(), id: *id, put, max: if put { MAX } else { 0 }, expires });
        format!("{}/v1/blobs/{token}", self.origin)
    }

    fn forget(&mut self, account: &[u8]) {
        let _ = fs::remove_dir_all(self.files.root.join(hex(account)));
    }

    fn mark(&mut self, account: &[u8], id: &[u8; 32], now: u64) {
        let _ = self.files.mark(account, id, now);
    }

    fn unmark(&mut self, account: &[u8], id: &[u8; 32]) {
        self.files.unmark(account, id);
    }

    fn sweep(&mut self, before: u64) -> u64 {
        self.files.sweep(before)
    }

    fn usage(&mut self, account: &[u8]) -> u64 {
        fs::read_dir(self.files.root.join(hex(account)))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().ends_with(".drop"))
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    }
}

/// Pictures in memory, and links that are only names, as an object
/// store's on a public host would read ([`MEMORY_LINKS`]; a device follows
/// no other kind): the tests', whose harnesses do the transfers with
/// [`MemoryBlobs::serve`].
#[derive(Default)]
pub struct MemoryBlobs {
    held: HashMap<(Vec<u8>, [u8; 32]), Vec<u8>>,
    marked: HashMap<(Vec<u8>, [u8; 32]), u64>,
}

impl MemoryBlobs {
    pub fn is_marked(&self, account: &[u8], id: &[u8; 32]) -> bool {
        self.marked.contains_key(&(account.to_vec(), *id))
    }

    pub fn get(&self, account: &[u8], id: &[u8; 32]) -> Option<&Vec<u8>> {
        self.held.get(&(account.to_vec(), *id))
    }

    pub fn put(&mut self, account: &[u8], id: &[u8; 32], sealed: &[u8]) {
        self.held.entry((account.to_vec(), *id)).or_insert_with(|| sealed.to_vec());
    }

    /// A transfer done as HTTP would: a put kept, a get's bytes given.
    pub fn serve(&mut self, t: &Transfer) -> Result<Vec<u8>, String> {
        let (url, sealed) = match t {
            Transfer::Put { url, sealed, .. } => (url, Some(sealed)),
            Transfer::Get { url, .. } => (url, None),
        };
        let mut parts = url.strip_prefix(MEMORY_LINKS).ok_or("not a memory link")?.split('/');
        let (way, account, id) = (parts.next(), parts.next().and_then(unhex), parts.next().and_then(unhex));
        let (Some(account), Some(id)) = (account, id.and_then(|i| <[u8; 32]>::try_from(i).ok())) else {
            return Err("not a memory link".into());
        };
        match (way, sealed) {
            (Some("put"), Some(sealed)) if sealed.len() as u64 <= MAX => {
                self.put(&account, &id, sealed);
                Ok(Vec::new())
            }
            (Some("get"), None) => self.get(&account, &id).cloned().ok_or_else(|| "404".to_string()),
            _ => Err("403".into()),
        }
    }
}

/// Where [`MemoryBlobs`]' links say they are.
pub const MEMORY_LINKS: &str = "https://blobs.test/";

fn unhex(s: &str) -> Option<Vec<u8>> {
    s.len().is_multiple_of(2).then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect())?
}

impl Blobs for MemoryBlobs {
    fn size(&mut self, account: &[u8], id: &[u8; 32]) -> Option<u64> {
        self.get(account, id).map(|b| b.len() as u64)
    }
    fn link(&mut self, account: &[u8], id: &[u8; 32], put: bool, _expires: u64) -> String {
        format!("{MEMORY_LINKS}{}/{}/{}", if put { "put" } else { "get" }, hex(account), hex(id))
    }
    fn forget(&mut self, account: &[u8]) {
        self.held.retain(|(a, _), _| a != account);
        self.marked.retain(|(a, _), _| a != account);
    }

    fn mark(&mut self, account: &[u8], id: &[u8; 32], now: u64) {
        let k = (account.to_vec(), *id);
        if self.held.contains_key(&k) {
            self.marked.entry(k).or_insert(now);
        }
    }

    fn unmark(&mut self, account: &[u8], id: &[u8; 32]) {
        self.marked.remove(&(account.to_vec(), *id));
    }

    fn sweep(&mut self, before: u64) -> u64 {
        let gone: Vec<_> = self.marked.iter().filter(|(_, at)| **at < before).map(|(k, _)| k.clone()).collect();
        for k in &gone {
            self.marked.remove(k);
            self.held.remove(k);
        }
        gone.len() as u64
    }

    fn usage(&mut self, account: &[u8]) -> u64 {
        self.held.iter().filter(|((a, _), _)| a == account).map(|(_, b)| b.len() as u64).sum()
    }
}

/// What the worker has done, for `/metrics`: read from any thread.
#[derive(Debug, Default)]
pub struct Counts {
    pub links: AtomicU64,
    /// Marked to go by a device's word, and gone after.
    pub dropped: AtomicU64,
    pub swept: AtomicU64,
}

pub struct Worker<B: Blobs> {
    pub blobs: B,
    pub counts: Arc<Counts>,
}

impl<B: Blobs> Worker<B> {
    pub fn new(blobs: B) -> Self {
        Worker { blobs, counts: Arc::default() }
    }

    /// Does `job`, saying what it sends in `out`.
    pub fn run(&mut self, job: Job, out: &mut dyn FnMut(Out)) {
        let expires = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) + LIFETIME;
        match job {
            Job::Offer { conn, account, ids, room, writable } => {
                // New pictures wait while the account can't write, or is
                // full; those kept are said kept either way.
                let refused = match room {
                    _ if !writable => Some(READ_ONLY),
                    Some(left) if self.blobs.usage(&account) >= left => Some(FULL),
                    _ => None,
                };
                let mut told = false;
                for id in ids {
                    if self.blobs.size(&account, &id).is_some() {
                        self.blobs.unmark(&account, &id);
                        out(send(conn, Carnet::BlobStored { id }));
                    } else if let Some(why) = refused {
                        if !std::mem::replace(&mut told, true) {
                            out(send(conn, Carnet::Refused { reason: why.into() }));
                        }
                    } else {
                        let url = self.blobs.link(&account, &id, true, expires);
                        self.counts.links.fetch_add(1, Ordering::Relaxed);
                        out(send(conn, Carnet::BlobLink { id, put: true, url, expires }));
                    }
                }
            }
            Job::Fetch { conn, account, ids } => {
                for id in ids {
                    if self.blobs.size(&account, &id).is_some() {
                        self.blobs.unmark(&account, &id);
                        let url = self.blobs.link(&account, &id, false, expires);
                        self.counts.links.fetch_add(1, Ordering::Relaxed);
                        out(send(conn, Carnet::BlobLink { id, put: false, url, expires }));
                    } else {
                        out(send(conn, Carnet::BlobNone { id }));
                    }
                }
            }
            Job::Drop { conn, account, ids } => {
                let now = expires - LIFETIME;
                for id in &ids {
                    self.blobs.mark(&account, id, now);
                }
                self.counts.dropped.fetch_add(ids.len() as u64, Ordering::Relaxed);
                out(send(conn, Carnet::BlobDropped { ids }));
            }
            Job::Usage { conn, account, notes, limit } => {
                let pictures = self.blobs.usage(&account);
                out(send(conn, Carnet::UsageIs {
                    notes,
                    pictures,
                    limit: limit.bytes.unwrap_or(0),
                    writable: limit.writable,
                    plan: limit.plan,
                    until: limit.until,
                }));
            }
            Job::Gone { account } => self.blobs.forget(&account),
        }
    }

    /// Pictures marked to go more than [`KEEP`] before `now`, gone.
    pub fn sweep(&mut self, now: u64) -> u64 {
        let n = self.blobs.sweep(now.saturating_sub(KEEP));
        self.counts.swept.fetch_add(n, Ordering::Relaxed);
        n
    }

    /// `outs` with the picture work in them done: what the hub sent and
    /// what the worker did, in order, for a driver that runs both on one
    /// thread (a test's) to send on.
    pub fn done(&mut self, outs: Vec<Out>) -> Vec<Out> {
        let mut all = Vec::new();
        for o in outs {
            match o {
                Out::Blob(job) => self.run(job, &mut |o| all.push(o)),
                o => all.push(o),
            }
        }
        all
    }
}

impl Worker<MemoryBlobs> {
    /// One on memory: the tests'.
    pub fn memory() -> Self {
        Worker::new(MemoryBlobs::default())
    }
}

fn send(conn: ConnId, c: Carnet) -> Out {
    Out::Send(conn, wire::encode(&Frame::Carnet(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(w: &mut Worker<MemoryBlobs>, job: Job) -> Vec<Carnet> {
        let mut outs = Vec::new();
        w.run(job, &mut |o| outs.push(o));
        outs.iter()
            .filter_map(|o| match o {
                Out::Send(_, b) => match wire::decode(b) {
                    Ok(Frame::Carnet(c)) => Some(c),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    /// Offered: kept, or a link to put it; put there, kept. Asked for: a
    /// link, or none. An account removed takes its pictures.
    #[test]
    fn links_to_put_and_to_fetch_and_kept_only_once_there() {
        let mut w = Worker::memory();
        let acct = b"acct".to_vec();
        let said_offer = said(&mut w, Job::Offer { conn: 1, account: acct.clone(), ids: vec![[1; 32]], room: None, writable: true });
        let [Carnet::BlobLink { id, put: true, url, .. }] = &said_offer[..] else { panic!("{said_offer:?}") };
        assert_eq!(*id, [1; 32]);
        w.blobs.serve(&Transfer::Put { id: [1; 32], url: url.clone(), sealed: b"sealed".to_vec() }).unwrap();
        assert_eq!(said(&mut w, Job::Offer { conn: 1, account: acct.clone(), ids: vec![[1; 32]], room: None, writable: true }), [Carnet::BlobStored { id: [1; 32] }]);
        let fetched = said(&mut w, Job::Fetch { conn: 1, account: acct.clone(), ids: vec![[1; 32], [2; 32]] });
        let [Carnet::BlobLink { put: false, url, .. }, Carnet::BlobNone { id: missing }] = &fetched[..] else { panic!("{fetched:?}") };
        assert_eq!(*missing, [2; 32]);
        assert_eq!(w.blobs.serve(&Transfer::Get { id: [1; 32], url: url.clone() }).unwrap(), b"sealed");
        // A get link doesn't put.
        assert!(w.blobs.serve(&Transfer::Put { id: [1; 32], url: url.clone(), sealed: b"other".to_vec() }).is_err());
        w.run(Job::Gone { account: acct.clone() }, &mut |_| {});
        assert_eq!(w.blobs.size(&acct, &[1; 32]), None);
        assert_eq!(w.counts.links.load(Ordering::Relaxed), 2, "one to put, one to fetch");
    }

    /// Files: kept once, moved in whole; links signed for this server.
    #[test]
    fn files_are_kept_whole_and_linked_by_signature() {
        let data = std::env::temp_dir().join(format!("carnet-blobfiles-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        let files = BlobFiles::open(&data).unwrap();
        let signer = Arc::new(Signer::new([9; 32]));
        let mut b = FileBlobs::new(files.clone(), "https://n.example/", signer.clone());
        assert!(files.keep(b"acct", &[1; 32], b"sealed").unwrap());
        assert!(!files.keep(b"acct", &[1; 32], b"not this").unwrap());
        assert_eq!(fs::read(files.path(b"acct", &[1; 32])).unwrap(), b"sealed", "never changed under its id");
        assert_eq!(fs::read_dir(&files.spool).unwrap().count(), 0);
        assert_eq!(b.size(b"acct", &[1; 32]), Some(6));
        let url = b.link(b"acct", &[1; 32], false, u64::MAX);
        let token = url.strip_prefix("https://n.example/v1/blobs/").expect("this server's");
        let g = signer.check(token, 0).unwrap();
        assert_eq!((g.account.as_slice(), g.id, g.put), (b"acct".as_slice(), [1; 32], false));
        // Marked, kept a while, then gone; unmarked, it stays.
        assert!(files.keep(b"acct", &[2; 32], b"other").unwrap());
        b.mark(b"acct", &[1; 32], 1000);
        b.mark(b"acct", &[2; 32], 1000);
        b.mark(b"acct", &[1; 32], 5000);
        b.mark(b"acct", &[3; 32], 1000);
        assert!(!files.mark_path(b"acct", &[3; 32]).exists(), "not kept: nothing to mark");
        b.unmark(b"acct", &[2; 32]);
        assert_eq!(b.sweep(1000), 0, "marked at 1000: not before it");
        assert_eq!(b.sweep(1001), 1, "the first mark counts");
        assert_eq!((b.size(b"acct", &[1; 32]), b.size(b"acct", &[2; 32])), (None, Some(5)));
        assert!(!files.mark_path(b"acct", &[1; 32]).exists());
        b.forget(b"acct");
        assert_eq!(b.size(b"acct", &[1; 32]), None);
        let _ = fs::remove_dir_all(&data);
    }
}
