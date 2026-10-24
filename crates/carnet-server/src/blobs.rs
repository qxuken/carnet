//! Files (docs/SYNC.md §4, §6). Their bytes go over HTTP, to and from a
//! short-lived signed link (`links.rs`); the socket says only which, and
//! when one is kept. The hub decides who may (a socket logged in, its
//! account) and hands the rest here as a [`Job`]: the [`Worker`], on a
//! thread of its own, answers with links, and checks a file put is
//! there before it's said to be kept. Where the files are is behind
//! [`Blobs`]: files beside the databases for a server of one's own
//! ([`FileBlobs`], its links to this server's `/v1/blobs/…`), object
//! storage for a hosted one (its links pre-signed there), the same to a
//! device either way.
//!
//! A link to put says how many bytes, exactly: the device says how long
//! each file is sealed when it offers it, and a put of any other length
//! is refused (by this server, or by the bucket, whose link is signed with
//! the length). For an account with a limit, a link given and not yet
//! used counts against the room left, at that length, until the file
//! is kept or no put by the link can still be going on (its time, then
//! as long as a put may take: [`PUT_TAKES`]): an offer of many
//! files, or many offers, can't get more links than there's room for,
//! but one file may still take the account over (docs/SYNC.md §11).
//!
//! A file no note shows any more is dropped by a device's word
//! (`BlobDrop`): marked, and gone [`KEEP`] later, unless a device offers,
//! asks for or puts it meanwhile. The server can't read the notes to
//! know; the wait is for the device that still has a use for it.
//!
//! An account removed has its removal recorded where its files are
//! ([`Blobs::forget`]: `gone/<account>`), then its files go. A put by
//! a link given before (a bucket's can't be taken back) lands after, so
//! the hourly sweep takes the files of accounts recorded so, and no
//! others: files of an account the store doesn't know, and whose
//! removal isn't recorded, are kept. The store alone can't say an account
//! is no more: one restored from an older backup, or a new one, doesn't
//! know the accounts made since, and their files would all go.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use carnet_sync::crypto::{MAX_BLOB, sealed_len};
use carnet_sync::session::Transfer;
use carnet_sync::wire::{self, Carnet, Frame};

use crate::hub::{ConnId, KEEP, Out};
use crate::links::{Grant, LIFETIME, PUT_TAKES, Signer};
use crate::quota::{FULL, Limit, READ_ONLY};

/// The most a sealed file may be: the largest file, sealed.
pub const MAX: u64 = (MAX_BLOB + 64) as u64;

/// The most links to put an account with a limit may have out at once,
/// not yet used: past it, a file offered gets no answer, and the
/// device offers it again later (docs/SYNC.md §6).
pub const PENDING: usize = 1024;

/// What a device is told of a file it offered without saying how long
/// it is, or past what a file may be.
pub const UNSIZED: &str = "A file was offered without its size, or bigger than a file may be, so it wasn't taken: update Carnet to send it";

/// File work the hub has allowed, for the worker to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// Which of `ids` are kept (`BlobStored`), and where to put the rest
    /// (`BlobLink`), each of the length `sizes` says for it: while the
    /// account may write, and has `room` left for files (`None`: no
    /// limit), links out not yet used counted in.
    Offer { conn: ConnId, account: Vec<u8>, ids: Vec<[u8; 32]>, sizes: Vec<u64>, room: Option<u64>, writable: bool },
    /// What the account keeps: its `notes`' bytes, as the hub has them,
    /// and its files' (`UsageIs`).
    Usage { conn: ConnId, account: Vec<u8>, notes: u64, limit: Limit },
    /// Where to fetch each of `ids` (`BlobLink`), or that it isn't kept
    /// (`BlobNone`).
    Fetch { conn: ConnId, account: Vec<u8>, ids: Vec<[u8; 32]> },
    /// No note shows `ids` any more: each marked, to go [`KEEP`] from
    /// now (`BlobDropped`).
    Drop { conn: ConnId, account: Vec<u8>, ids: Vec<[u8; 32]> },
    /// An account removed: its files go.
    Gone { account: Vec<u8> },
}

/// Where files are kept: by account and id, sealed.
pub trait Blobs: Send {
    /// Its size, if kept.
    fn size(&mut self, account: &[u8], id: &[u8; 32]) -> Option<u64>;
    /// A link to `PUT` it, of exactly `put` bytes and no other number,
    /// or (`None`) to `GET` it, good until `expires`.
    fn link(&mut self, account: &[u8], id: &[u8; 32], put: Option<u64>, expires: u64) -> String;
    /// The account was removed: that recorded ([`Blobs::removed`]), and
    /// its files go.
    fn forget(&mut self, account: &[u8]);
    /// Marked to go, as of `now`, if kept and not marked already.
    fn mark(&mut self, account: &[u8], id: &[u8; 32], now: u64);
    /// Wanted after all: not marked.
    fn unmark(&mut self, account: &[u8], id: &[u8; 32]);
    /// Those marked before `before` gone: how many.
    fn sweep(&mut self, before: u64) -> u64;
    /// Every file the account keeps: its id, and its size.
    fn held(&mut self, account: &[u8]) -> Vec<([u8; 32], u64)>;
    /// The bytes the account's files take.
    fn usage(&mut self, account: &[u8]) -> u64 {
        self.held(account).iter().map(|(_, n)| n).sum()
    }
    /// Where its links point, if not this server: the origin the web app
    /// fetches files from, which its CSP allows (docs/SYNC.md §8).
    fn origin(&self) -> Option<String> {
        None
    }
    /// Every account with files here, whether it's still an account
    /// or not: what [`Worker::sweep_unowned`] asks the store about.
    fn owners(&mut self) -> Vec<Vec<u8>> {
        Vec::new()
    }
    /// Every account whose removal was recorded here ([`Blobs::forget`]):
    /// the only ones whose files the sweep takes without a device's
    /// word. None if it can't be read: nothing goes for a failure.
    fn removed(&mut self) -> HashSet<Vec<u8>> {
        HashSet::new()
    }
}

/// Boxed, still one: the server runs files or a bucket alike.
impl<T: Blobs + ?Sized> Blobs for Box<T> {
    fn size(&mut self, account: &[u8], id: &[u8; 32]) -> Option<u64> {
        (**self).size(account, id)
    }
    fn link(&mut self, account: &[u8], id: &[u8; 32], put: Option<u64>, expires: u64) -> String {
        (**self).link(account, id, put, expires)
    }
    fn held(&mut self, account: &[u8]) -> Vec<([u8; 32], u64)> {
        (**self).held(account)
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
    fn origin(&self) -> Option<String> {
        (**self).origin()
    }
    fn owners(&mut self) -> Vec<Vec<u8>> {
        (**self).owners()
    }
    fn removed(&mut self) -> HashSet<Vec<u8>> {
        (**self).removed()
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Files on disk: `data/blobs/<account>/<id>`, each written whole
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
        crate::private::dir(&files.root)?;
        crate::private::dir(&files.spool)?;
        for e in fs::read_dir(&files.spool)?.flatten() {
            let _ = fs::remove_file(e.path());
        }
        Ok(files)
    }

    pub fn path(&self, account: &[u8], id: &[u8; 32]) -> PathBuf {
        self.root.join(hex(account)).join(hex(id))
    }

    /// An account's removal, recorded: `gone/<account>`, when, in seconds
    /// since 1970 (`gone` names no account: not hex).
    fn removal_path(&self, account: &[u8]) -> PathBuf {
        self.root.join("gone").join(hex(account))
    }

    /// The account's removal recorded, as of `now`, on the disk when this
    /// returns: before its files go, so a crash between leaves the
    /// sweep to finish.
    pub fn record_removal(&self, account: &[u8], now: u64) -> io::Result<()> {
        let dir = self.root.join("gone");
        if !dir.exists() {
            crate::private::dir(&dir)?;
            sync_dir(&self.root)?;
        }
        let mut file = crate::private::create().open(self.removal_path(account))?;
        io::Write::write_all(&mut file, now.to_string().as_bytes())?;
        file.sync_all()?;
        sync_dir(&dir)
    }

    /// Every account whose removal is recorded.
    pub fn removed(&self) -> HashSet<Vec<u8>> {
        fs::read_dir(self.root.join("gone")).into_iter().flatten().flatten().filter_map(|e| unhex(&e.file_name().to_string_lossy()).filter(|a| !a.is_empty())).collect()
    }

    /// Beside a file marked to go: when it was, in seconds since 1970.
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

    /// Every file marked before `before`, gone: how many.
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

    /// `sealed` kept as `id`, unless it is already (a file never
    /// changes under its id). True if it wasn't.
    pub fn keep(&self, account: &[u8], id: &[u8; 32], sealed: &[u8]) -> io::Result<bool> {
        use std::io::Write;
        let tmp = self.spool.join(format!("{}-{}", hex(account), hex(id)));
        let mut file = crate::private::create().open(&tmp)?;
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
            crate::private::dir(dir)?;
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

/// A server's own files: on disk, and links to its own `/v1/blobs/…`.
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

    fn link(&mut self, account: &[u8], id: &[u8; 32], put: Option<u64>, expires: u64) -> String {
        let token = self.signer.sign(&Grant { account: account.to_vec(), id: *id, put: put.is_some(), max: put.unwrap_or(0), expires });
        format!("{}/v1/blobs/{token}", self.origin)
    }

    fn forget(&mut self, account: &[u8]) {
        // Not recorded, not deleted: the sweep would never finish it.
        if let Err(e) = self.files.record_removal(account, unix_now()) {
            eprintln!("carnet-server: files: recording an account's removal: {e}");
            return;
        }
        let _ = fs::remove_dir_all(self.files.root.join(hex(account)));
    }

    fn removed(&mut self) -> HashSet<Vec<u8>> {
        self.files.removed()
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

    fn owners(&mut self) -> Vec<Vec<u8>> {
        fs::read_dir(&self.files.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| unhex(&e.file_name().to_string_lossy()).filter(|a| !a.is_empty()))
            .collect()
    }

    fn held(&mut self, account: &[u8]) -> Vec<([u8; 32], u64)> {
        fs::read_dir(self.files.root.join(hex(account)))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let id = <[u8; 32]>::try_from(unhex(&e.file_name().to_string_lossy())?).ok()?;
                Some((id, e.metadata().ok()?.len()))
            })
            .collect()
    }
}

/// Files in memory, and links that are only names, as an object
/// store's on a public host would read ([`MEMORY_LINKS`]; a device follows
/// no other kind): the tests', whose harnesses do the transfers with
/// [`MemoryBlobs::serve`].
#[derive(Default)]
pub struct MemoryBlobs {
    held: HashMap<(Vec<u8>, [u8; 32]), Vec<u8>>,
    marked: HashMap<(Vec<u8>, [u8; 32]), u64>,
    removed: HashSet<Vec<u8>>,
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
        // A put's link says its length, and takes no other.
        let size = parts.next().and_then(|n| n.parse::<u64>().ok());
        match (way, sealed) {
            (Some("put"), Some(sealed)) if Some(sealed.len() as u64) == size => {
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
    fn link(&mut self, account: &[u8], id: &[u8; 32], put: Option<u64>, _expires: u64) -> String {
        match put {
            Some(n) => format!("{MEMORY_LINKS}put/{}/{}/{n}", hex(account), hex(id)),
            None => format!("{MEMORY_LINKS}get/{}/{}", hex(account), hex(id)),
        }
    }
    fn forget(&mut self, account: &[u8]) {
        self.removed.insert(account.to_vec());
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

    fn held(&mut self, account: &[u8]) -> Vec<([u8; 32], u64)> {
        self.held.iter().filter(|((a, _), _)| a == account).map(|((_, id), b)| (*id, b.len() as u64)).collect()
    }

    fn owners(&mut self) -> Vec<Vec<u8>> {
        let all: HashSet<Vec<u8>> = self.held.keys().map(|(a, _)| a.clone()).collect();
        all.into_iter().collect()
    }

    fn removed(&mut self) -> HashSet<Vec<u8>> {
        self.removed.clone()
    }
}

/// What [`Worker::sweep_unowned`] found: how many accounts the store
/// doesn't have hold files here, removed ones' gone, the rest kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unowned {
    /// Removed (recorded so): their files gone.
    pub gone: usize,
    /// Never known to the store, or not any more, removal not recorded:
    /// their files kept.
    pub kept: usize,
}

/// Accounts with files here that the store doesn't have, and whose
/// removal isn't recorded here: what a server refuses to start beside
/// with no account of its own (`server::Start`), its records likely not
/// put back. `has` is the store's word on each.
pub fn strangers(blobs: &mut dyn Blobs, has: impl Fn(&[u8]) -> bool) -> Vec<Vec<u8>> {
    let owners = blobs.owners();
    if owners.is_empty() {
        return Vec::new();
    }
    let removed = blobs.removed();
    owners.into_iter().filter(|a| !removed.contains(a) && !has(a)).collect()
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What the worker has done, for `/metrics`: read from any thread.
#[derive(Debug, Default)]
pub struct Counts {
    pub links: AtomicU64,
    /// Marked to go by a device's word, and gone after.
    pub dropped: AtomicU64,
    pub swept: AtomicU64,
}

/// An account's links to put out, by file: its length, and when no
/// put by its link can still be going on (its link's time, and
/// [`PUT_TAKES`] after: a put begun as the link runs out may go on as
/// long).
type Pending = HashMap<[u8; 32], (u64, u64)>;

pub struct Worker<B: Blobs> {
    pub blobs: B,
    pub counts: Arc<Counts>,
    /// Links to put given to accounts with a limit, not yet seen used: by
    /// account, each file's length and until when a put by its link
    /// may go on. Counted against the room left until the file is kept,
    /// or no put by the link can be going on: a link run out with puts
    /// begun before still coming frees no room for another round of them.
    pending: HashMap<Vec<u8>, Pending>,
}

impl<B: Blobs> Worker<B> {
    pub fn new(blobs: B) -> Self {
        Worker { blobs, counts: Arc::default(), pending: HashMap::new() }
    }

    /// Links to put out for `account`, not yet used: how many, and the
    /// bytes they may put.
    pub fn pending(&self, account: &[u8]) -> (usize, u64) {
        self.pending.get(account).map_or((0, 0), |p| (p.len(), p.values().map(|(n, _)| n).sum()))
    }

    /// Does `job`, saying what it sends in `out`.
    pub fn run(&mut self, job: Job, out: &mut dyn FnMut(Out)) {
        self.run_at(job, unix_now(), out);
    }

    /// Does `job` as of `now`, in seconds since 1970.
    pub fn run_at(&mut self, job: Job, now: u64, out: &mut dyn FnMut(Out)) {
        let expires = now + LIFETIME;
        match job {
            Job::Offer { conn, account, ids, sizes, room, writable } => {
                // With a limit, what's kept is listed once (it's counted
                // anyway); without, each file is asked after.
                let held: Option<HashMap<[u8; 32], u64>> = room.map(|_| self.blobs.held(&account).into_iter().collect());
                let pending = self.pending.entry(account.clone()).or_default();
                pending.retain(|id, (_, until)| *until > now && !held.as_ref().is_some_and(|h| h.contains_key(id)));
                let mut used = held.as_ref().map_or(0, |h| h.values().sum::<u64>()) + pending.values().map(|(n, _)| n).sum::<u64>();
                let mut told = HashSet::new();
                for (i, id) in ids.into_iter().enumerate() {
                    let kept = match &held {
                        Some(h) => h.contains_key(&id),
                        None => self.blobs.size(&account, &id).is_some(),
                    };
                    if kept {
                        self.blobs.unmark(&account, &id);
                        out(send(conn, Carnet::BlobStored { id }));
                        continue;
                    }
                    // New files wait while the account can't write, or
                    // is full, links out counted in: one may take it over,
                    // never two. One offered again has its link counted
                    // once.
                    let size = sizes.get(i).copied().filter(|n| (sealed_len(0)..=MAX).contains(n));
                    let again = pending.remove(&id);
                    used = used.saturating_sub(again.map_or(0, |(n, _)| n));
                    let refused = match (room, size) {
                        _ if !writable => Some(READ_ONLY),
                        (_, None) => Some(UNSIZED),
                        (Some(left), _) if used >= left => Some(FULL),
                        _ => None,
                    };
                    if let Some(why) = refused {
                        // Its link out still counts until it's past.
                        if let Some((n, until)) = again {
                            pending.insert(id, (n, until));
                            used += n;
                        }
                        if told.insert(why) {
                            out(send(conn, Carnet::Refused { reason: why.into() }));
                        }
                        continue;
                    }
                    let size = size.expect("sized");
                    if room.is_some() {
                        // As many as that out already: no answer now, and
                        // the device offers it again later.
                        if pending.len() >= PENDING {
                            continue;
                        }
                        pending.insert(id, (size, expires + PUT_TAKES));
                        used += size;
                    }
                    let url = self.blobs.link(&account, &id, Some(size), expires);
                    self.counts.links.fetch_add(1, Ordering::Relaxed);
                    out(send(conn, Carnet::BlobLink { id, put: true, url, expires }));
                }
                if self.pending.get(&account).is_some_and(HashMap::is_empty) {
                    self.pending.remove(&account);
                }
            }
            Job::Fetch { conn, account, ids } => {
                for id in ids {
                    if self.blobs.size(&account, &id).is_some() {
                        self.blobs.unmark(&account, &id);
                        let url = self.blobs.link(&account, &id, None, expires);
                        self.counts.links.fetch_add(1, Ordering::Relaxed);
                        out(send(conn, Carnet::BlobLink { id, put: false, url, expires }));
                    } else {
                        out(send(conn, Carnet::BlobNone { id }));
                    }
                }
            }
            Job::Drop { conn, account, ids } => {
                for id in &ids {
                    self.blobs.mark(&account, id, now);
                }
                self.counts.dropped.fetch_add(ids.len() as u64, Ordering::Relaxed);
                out(send(conn, Carnet::BlobDropped { ids }));
            }
            Job::Usage { conn, account, notes, limit } => {
                let files = self.blobs.usage(&account);
                out(send(conn, Carnet::UsageIs {
                    notes,
                    files,
                    limit: limit.bytes.unwrap_or(0),
                    writable: limit.writable,
                    plan: limit.plan,
                    until: limit.until,
                }));
            }
            Job::Gone { account } => {
                self.pending.remove(&account);
                self.blobs.forget(&account);
            }
        }
    }

    /// Files marked to go more than [`KEEP`] before `now`, gone; links
    /// out whose puts can't be going on any more, forgotten.
    pub fn sweep(&mut self, now: u64) -> u64 {
        for p in self.pending.values_mut() {
            p.retain(|_, (_, until)| *until > now);
        }
        self.pending.retain(|_, p| !p.is_empty());
        let n = self.blobs.sweep(now.saturating_sub(KEEP));
        self.counts.swept.fetch_add(n, Ordering::Relaxed);
        n
    }

    /// Files of accounts removed, gone: a put by a link given before
    /// its account was deleted, or one the deletion missed. `unknown`
    /// says which of the accounts with files here the store doesn't
    /// have now (asked after the files are listed, so an account made
    /// meanwhile isn't among them); of those, only the ones whose removal
    /// is recorded here go ([`Blobs::removed`]). The rest are kept: the
    /// store may be older than they are (restored from a backup), or new.
    pub fn sweep_unowned(&mut self, unknown: impl FnOnce(Vec<Vec<u8>>) -> Vec<Vec<u8>>) -> Unowned {
        let owners = self.blobs.owners();
        if owners.is_empty() {
            return Unowned::default();
        }
        let unknown = unknown(owners);
        if unknown.is_empty() {
            return Unowned::default();
        }
        let removed = self.blobs.removed();
        let (gone, kept): (Vec<_>, Vec<_>) = unknown.into_iter().partition(|a| removed.contains(a));
        for account in &gone {
            self.pending.remove(account);
            self.blobs.forget(account);
        }
        Unowned { gone: gone.len(), kept: kept.len() }
    }

    /// `outs` with the file work in them done: what the hub sent and
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
    /// link, or none. An account removed takes its files.
    #[test]
    fn links_to_put_and_to_fetch_and_kept_only_once_there() {
        let mut w = Worker::memory();
        let acct = b"acct".to_vec();
        let sealed = vec![5u8; 40];
        let offer = |ids: Vec<[u8; 32]>| Job::Offer { conn: 1, account: b"acct".to_vec(), sizes: vec![40; ids.len()], ids, room: None, writable: true };
        let said_offer = said(&mut w, offer(vec![[1; 32]]));
        let [Carnet::BlobLink { id, put: true, url, .. }] = &said_offer[..] else { panic!("{said_offer:?}") };
        assert_eq!(*id, [1; 32]);
        // Only of the length it said.
        assert!(w.blobs.serve(&Transfer::Put { id: [1; 32], url: url.clone(), sealed: vec![5u8; 41] }).is_err());
        w.blobs.serve(&Transfer::Put { id: [1; 32], url: url.clone(), sealed: sealed.clone() }).unwrap();
        assert_eq!(said(&mut w, offer(vec![[1; 32]])), [Carnet::BlobStored { id: [1; 32] }]);
        let fetched = said(&mut w, Job::Fetch { conn: 1, account: acct.clone(), ids: vec![[1; 32], [2; 32]] });
        let [Carnet::BlobLink { put: false, url, .. }, Carnet::BlobNone { id: missing }] = &fetched[..] else { panic!("{fetched:?}") };
        assert_eq!(*missing, [2; 32]);
        assert_eq!(w.blobs.serve(&Transfer::Get { id: [1; 32], url: url.clone() }).unwrap(), sealed);
        // A get link doesn't put.
        assert!(w.blobs.serve(&Transfer::Put { id: [1; 32], url: url.clone(), sealed: b"other".to_vec() }).is_err());
        w.run(Job::Gone { account: acct.clone() }, &mut |_| {});
        assert_eq!(w.blobs.size(&acct, &[1; 32]), None);
        assert_eq!(w.counts.links.load(Ordering::Relaxed), 2, "one to put, one to fetch");
    }

    /// An offer of thousands of files, or many offers, gets no more
    /// links than there's room for: each link out counts at the length
    /// the device said, until its file is kept or it runs out. One
    /// file may take the account over, never two; one offered again
    /// is counted once; one without its size gets none.
    #[test]
    fn links_out_count_against_the_room_left() {
        let mut w = Worker::memory();
        let acct = b"acct".to_vec();
        let id = |n: u32| -> [u8; 32] {
            let mut id = [0; 32];
            id[..4].copy_from_slice(&n.to_be_bytes());
            id
        };
        let offer = |ids: Vec<[u8; 32]>, size: u64, room: u64| Job::Offer { conn: 1, account: b"acct".to_vec(), sizes: vec![size; ids.len()], ids, room: Some(room), writable: true };
        let links = |said: &[Carnet]| said.iter().filter(|c| matches!(c, Carnet::BlobLink { put: true, .. })).count();
        let full = |said: &[Carnet]| said.iter().filter(|c| **c == Carnet::Refused { reason: FULL.into() }).count();
        let mut out = Vec::new();
        let mut at = |w: &mut Worker<MemoryBlobs>, job: Job, now: u64| {
            out.clear();
            w.run_at(job, now, &mut |o| out.push(o));
            out.iter().filter_map(|o| if let Out::Send(_, b) = o { if let Ok(Frame::Carnet(c)) = wire::decode(b) { Some(c) } else { None } } else { None }).collect::<Vec<_>>()
        };
        let t = 1_000_000;

        // 5,000 files of 1,000 bytes, 10,000 left: ten links, and the
        // rest refused, said once.
        let said = at(&mut w, offer((0..5000).map(id).collect(), 1000, 10_000), t);
        assert_eq!((links(&said), full(&said)), (10, 1));
        assert_eq!(w.pending(&acct), (10, 10_000));
        // Offered again before any is put: nothing more.
        let said = at(&mut w, offer((5000..5100).map(id).collect(), 1000, 10_000), t + 1);
        assert_eq!((links(&said), full(&said)), (0, 1));
        // One of the ten again: a new link, counted once.
        let said = at(&mut w, offer(vec![id(3)], 1000, 10_000), t + 2);
        assert_eq!(links(&said), 1);
        assert_eq!(w.pending(&acct), (10, 10_000));
        // Put: kept, its link no longer counted, the file is; still full.
        let Some(Carnet::BlobLink { url, .. }) = said.into_iter().next() else { panic!() };
        w.blobs.serve(&Transfer::Put { id: id(3), url, sealed: vec![1; 1000] }).unwrap();
        let said = at(&mut w, offer(vec![id(3), id(6000)], 1000, 10_000), t + 3);
        assert_eq!(said[0], Carnet::BlobStored { id: id(3) });
        assert_eq!((links(&said), full(&said)), (0, 1));
        assert_eq!(w.pending(&acct), (9, 9000));
        // The links run out unused, and no put by one can still be going
        // on: room again, for what's not kept.
        let said = at(&mut w, offer((7000..7020).map(id).collect(), 1000, 10_000), t + 2 + LIFETIME + PUT_TAKES);
        assert_eq!((links(&said), full(&said)), (9, 1));
        assert_eq!(w.sweep(t + 10 * (LIFETIME + PUT_TAKES)), 0);
        assert_eq!(w.pending(&acct), (0, 0), "forgotten once past");

        // One file may take it over, never two.
        let mut w = Worker::memory();
        let said = at(&mut w, offer(vec![id(1), id(2)], 50_000, 10_000), t);
        assert_eq!((links(&said), full(&said)), (1, 1));
        // Without a size, or past what a file may be: no link.
        let said = at(&mut w, Job::Offer { conn: 1, account: acct.clone(), ids: vec![id(8), id(9)], sizes: vec![MAX + 1], room: None, writable: true }, t);
        assert_eq!(said, [Carnet::Refused { reason: UNSIZED.into() }]);
        // Only so many links out at once, however much room.
        let said = at(&mut w, offer((0..PENDING as u32 + 50).map(|n| id(n + 10)).collect(), 36, u64::MAX), t);
        assert_eq!((links(&said), full(&said)), (PENDING - 1, 0), "one is out already");
        // Without a limit, nothing is counted.
        let mut w = Worker::memory();
        let said = at(&mut w, Job::Offer { conn: 1, account: acct.clone(), ids: (0..3000).map(id).collect(), sizes: vec![MAX; 3000], room: None, writable: true }, t);
        assert_eq!(links(&said), 3000);
        assert_eq!(w.pending(&acct), (0, 0));
    }

    /// A link that has run out may have a put begun before still coming,
    /// for as long as a put may take: its room stays taken till then, so
    /// slow puts begun on one round of links, and a second round asked
    /// for once those run out, can't take an account to twice its limit.
    #[test]
    fn a_link_run_out_holds_its_room_while_its_put_may_still_come() {
        let mut w = Worker::memory();
        let acct = b"acct".to_vec();
        let id = |n: u8| [n; 32];
        let offer = |ids: Vec<[u8; 32]>| Job::Offer { conn: 1, account: b"acct".to_vec(), sizes: vec![1000; ids.len()], ids, room: Some(10_000), writable: true };
        let links_at = |w: &mut Worker<MemoryBlobs>, job: Job, now: u64| {
            let mut out = Vec::new();
            w.run_at(job, now, &mut |o| out.push(o));
            out.iter().filter(|o| matches!(o, Out::Send(_, b) if matches!(wire::decode(b), Ok(Frame::Carnet(Carnet::BlobLink { put: true, .. }))))).count()
        };
        let t = 1_000_000;
        assert_eq!(links_at(&mut w, offer((0..10).map(id).collect()), t), 10, "the room's worth");
        // The links have run out, but puts begun on them may still come.
        for later in [t + LIFETIME, t + LIFETIME + 1, t + LIFETIME + PUT_TAKES - 1] {
            assert_eq!(links_at(&mut w, offer((10..20).map(id).collect()), later), 0, "at {}", later - t);
            assert_eq!(w.sweep(later), 0);
            assert_eq!(w.pending(&acct), (10, 10_000), "still counted at {}", later - t);
        }
        // None can be now: the room is free again.
        assert_eq!(links_at(&mut w, offer((10..20).map(id).collect()), t + LIFETIME + PUT_TAKES), 10);
        // And this server ends a put by then (`server::BLOB_TIMEOUT`).
        assert_eq!(crate::server::BLOB_TIMEOUT.as_secs(), PUT_TAKES);
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
        let url = b.link(b"acct", &[1; 32], None, u64::MAX);
        let token = url.strip_prefix("https://n.example/v1/blobs/").expect("this server's");
        let g = signer.check(token, 0).unwrap();
        assert_eq!((g.account.as_slice(), g.id, g.put), (b"acct".as_slice(), [1; 32], false));
        // A put's link says the length the device said, and the PUT is
        // held to it (`server.rs`).
        let url = b.link(b"acct", &[2; 32], Some(5036), u64::MAX);
        let g = signer.check(url.strip_prefix("https://n.example/v1/blobs/").unwrap(), 0).unwrap();
        assert_eq!((g.put, g.max), (true, 5036));
        assert_eq!(b.held(b"acct"), [([1; 32], 6)], "what's kept, marks aside");
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
        assert_eq!(b.removed(), HashSet::from([b"acct".to_vec()]), "its removal recorded");
        assert!(b.owners().is_empty(), "the record names no account folder");
        let _ = fs::remove_dir_all(&data);
    }

    /// Of the accounts with files the store doesn't have, only those
    /// removed here lose them; the others are kept, and are what a store
    /// with no account refuses to start beside.
    #[test]
    fn the_sweep_takes_only_removed_accounts_files() {
        let mut w = Worker::memory();
        for a in [&b"here"[..], b"removed", b"stranger"] {
            w.blobs.put(a, &[1; 32], b"sealed");
        }
        w.run_at(Job::Gone { account: b"removed".to_vec() }, 0, &mut |_| {});
        // A put by a link given before, after.
        w.blobs.put(b"removed", &[2; 32], b"late");
        let store = |a: &[u8]| a == b"here";
        assert_eq!(strangers(&mut w.blobs, store), [b"stranger".to_vec()]);
        let swept = w.sweep_unowned(|owners| owners.into_iter().filter(|a| !store(a)).collect());
        assert_eq!(swept, Unowned { gone: 1, kept: 1 });
        assert_eq!(w.blobs.get(b"removed", &[2; 32]), None);
        assert!(w.blobs.get(b"stranger", &[1; 32]).is_some() && w.blobs.get(b"here", &[1; 32]).is_some());
        // The store that doesn't know "here" either (an empty one): kept.
        assert_eq!(w.sweep_unowned(|owners| owners), Unowned { gone: 0, kept: 2 });
        assert!(w.blobs.get(b"here", &[1; 32]).is_some());
    }
}
