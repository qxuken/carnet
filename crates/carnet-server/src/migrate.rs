//! `carnet-server migrate`: what a server keeps in its data folder (the
//! SQLite files, the pictures beside them) copied to Postgres and to an
//! S3 bucket, for a server that runs on those from then on (docs/SYNC.md
//! §6, §11; the README's runbook).
//!
//! - **Only with the server stopped.** The files are locked while they're
//!   read; one a server has open is refused, not waited for. (The
//!   pictures can go first, while it runs: below.)
//! - **The source is never written.** Nothing in the folder is changed or
//!   deleted: going back is starting the old server on it again.
//! - **Again, and it goes on.** Each account is copied whole in one
//!   transaction, or not at all; one already the same on the other side is
//!   skipped, one that differs is copied again. A picture is skipped when
//!   the bucket has it, the same size and MD5. A run that stopped half way
//!   is finished by the next.
//! - **Never over a server's work.** Once a server has run on the database
//!   or the bucket (each says so when one opens it), they hold what the
//!   folder doesn't, and nothing is copied over them.
//! - **Checked.** After copying, every account is read back from both
//!   sides and compared row by row (and by SHA-256 over its rows), the
//!   invites likewise, and every picture by size and MD5 (the ETag S3
//!   gives a plain put). Any difference fails the run.
//!
//! **Pictures first, while the server runs** (`pictures_first`,
//! `--pictures-first`): pictures are most of the bytes, and each is kept
//! under its id whole and never changed, so they can be copied before
//! the stop, and the stopped run after has only the records and the
//! pictures since to copy (it skips the rest, the same size and MD5).
//!
//! - **No lock, nothing written.** The folder is read as it is beside the
//!   server running on it. Only a file named as an id is taken: a
//!   picture is written in `spool/`, synced, and moved in under its id
//!   (`blobs.rs`), so one so named is whole. One still coming in isn't
//!   among them; the stopped run copies it.
//! - **Whose, by the folder.** `blobs/<account>/<id>` names its account,
//!   so SQLite isn't opened at all. Opening it read only beside the
//!   server would still write (a reader's place in the WAL's `-shm`), and
//!   tell nothing more: a running server has no pictures left in its
//!   databases (its start moves them out into files).
//! - **Gone meanwhile is fine.** A picture marked to go, or swept, or of
//!   an account removed, may be copied anyway: the stopped run takes from
//!   the bucket what the folder no longer has, and copies the marks.
//! - **Never over a server's work**, as above: a bucket (or database,
//!   given one) a server has run on is refused. Postgres isn't needed.
//!
//! The keys stay files in the data folder: `opaque.key` (the server's
//! OPAQUE key, which every password depends on) and `links.key` (for the
//! folder's own picture links, unused with a bucket). A database dump
//! holds no secret; the folder does.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
#[cfg(feature = "s3")]
use std::path::Path;

use crate::postgres::Postgres;
use crate::rows::Counts;
#[cfg(feature = "s3")]
use crate::s3::{Object, Picture, S3Blobs, S3Config};
use crate::sqlite::Locked;
use crate::storage::Failed;

/// What to copy, from where to where.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// The data folder (`--data` of the server that ran on it).
    pub from: PathBuf,
    /// The records to this database.
    pub postgres: Option<String>,
    /// The pictures to this bucket.
    #[cfg(feature = "s3")]
    pub s3: Option<S3Config>,
    /// Only compare what's there; copy nothing.
    pub verify_only: bool,
    /// Only the pictures, with the server still running: no lock, no
    /// records, no check (the stopped run after does those).
    pub pictures_first: bool,
}

/// What a run did, and found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Done {
    /// The rows in the folder: every account's, added up.
    pub rows: Counts,
    /// Accounts copied this time, and those the same already.
    pub copied: u64,
    pub skipped: u64,
    /// Accounts the database had and the folder doesn't: gone from it.
    pub dropped: u64,
    pub invites: u64,
    /// Pictures in the folder (and their bytes), those put this time, the
    /// same already, and marks to go.
    pub pictures: u64,
    pub picture_bytes: u64,
    pub put: u64,
    pub put_bytes: u64,
    pub pictures_skipped: u64,
    pub marks: u64,
    /// Pictures with no account in the folder: left where they are.
    pub strays: u64,
    /// Every account's SHA-256 over its rows, as both sides have them.
    pub sums: BTreeMap<Vec<u8>, [u8; 32]>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(feature = "s3")]
fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len().is_multiple_of(2) && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect())?
}

const SERVED_DB: &str =
    "a server has run on this database: it holds what the folder doesn't, and nothing is copied over it. Migrate into an empty database (or empty this one first)";

/// The run, saying what it does on `say`: copied (unless `verify_only`)
/// and checked, or why not.
pub fn run(plan: &Plan, say: &mut (dyn FnMut(&str) + Send)) -> Result<Done, Failed> {
    // The database's client blocks: off any async caller's thread.
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::scope(|s| s.spawn(|| run(plan, say)).join().unwrap_or_else(|p| std::panic::resume_unwind(p)));
    }
    #[cfg(feature = "s3")]
    let to_s3 = plan.s3.is_some();
    #[cfg(not(feature = "s3"))]
    let to_s3 = false;
    if plan.postgres.is_none() && !to_s3 {
        return Err(Failed("nothing to migrate to: give a database (--to-postgres), a bucket (--to-s3), or both".into()));
    }
    if plan.pictures_first {
        return pictures_first(plan, say);
    }
    let src = Locked::open(&plan.from)?;
    say(&format!("{}: locked for the run (no server can start on it meanwhile); read, never written", plan.from.display()));
    let mut done = Done::default();
    let accounts = src.accounts()?;

    if let Some(url) = &plan.postgres {
        let pg = Postgres::target(url)?;
        if pg.served()? && !plan.verify_only {
            return Err(Failed(SERVED_DB.into()));
        }
        records(&src, &pg, &accounts, plan.verify_only, say, &mut done)?;
    }

    #[cfg(feature = "s3")]
    if let Some(config) = &plan.s3 {
        let s3 = bucket(config, plan.verify_only)?;
        pictures(&src, &s3, &plan.from, &accounts, plan.verify_only, say, &mut done)?;
    }
    Ok(done)
}

/// Every account's rows, and the invites: copied where they differ,
/// then compared on both sides.
fn records(src: &Locked, pg: &Postgres, accounts: &[Vec<u8>], verify_only: bool, say: &mut dyn FnMut(&str), done: &mut Done) -> Result<(), Failed> {
    let total = accounts.len();
    if !verify_only {
        for (i, a) in accounts.iter().enumerate() {
            let rows = src.rows(a)?;
            let there = pg.rows(a)?;
            if there.as_ref() == Some(&rows) {
                done.skipped += 1;
                continue;
            }
            pg.put_rows(&rows)?;
            done.copied += 1;
            let c = rows.counts();
            say(&format!("account {}/{total} {}: {} devices, {} records, {} bases, copied", i + 1, hex(a), c.devices, c.records, c.bases));
        }
        let keep: BTreeSet<&Vec<u8>> = accounts.iter().collect();
        for a in pg.account_ids()? {
            if !keep.contains(&a) {
                pg.drop_rows(&a)?;
                done.dropped += 1;
                say(&format!("account {}: gone from the folder since the last run, gone from the database", hex(&a)));
            }
        }
        pg.put_invites(&src.invites()?)?;
    }

    // Checked: each account read back from both, compared whole.
    let mut wrong = Vec::new();
    for a in accounts {
        let rows = src.rows(a)?;
        let sum = rows.sum();
        match pg.rows(a)? {
            Some(there) if there == rows => {
                done.rows += rows.counts();
                done.sums.insert(a.clone(), sum);
            }
            Some(there) => wrong.push(format!("account {}: differs (sha256 {} in the folder, {} in the database)", hex(a), hex(&sum), hex(&there.sum()))),
            None => wrong.push(format!("account {}: not in the database", hex(a))),
        }
    }
    let extra: Vec<Vec<u8>> = pg.account_ids()?.into_iter().filter(|a| !accounts.contains(a)).collect();
    for a in &extra {
        wrong.push(format!("account {}: in the database, not in the folder", hex(a)));
    }
    let invites = src.invites()?;
    let mut theirs: Vec<_> = crate::storage::Storage::invites(pg);
    theirs.sort();
    let mut ours = invites.clone();
    ours.sort();
    if theirs != ours {
        wrong.push(format!("invites: {} in the folder, {} in the database, not the same", ours.len(), theirs.len()));
    }
    done.invites = invites.len() as u64;
    if !wrong.is_empty() {
        for w in &wrong {
            say(w);
        }
        return Err(Failed(format!("the database is not the same as the folder: {} differences", wrong.len())));
    }
    say(&format!("records checked: {} the same in the folder and the database, row by row; {} invites", done.rows, done.invites));
    Ok(())
}

/// A picture in the folder: where its bytes are, or the bytes themselves
/// (one still kept in an account's database).
#[cfg(feature = "s3")]
enum Source {
    File(PathBuf),
    Kept(Vec<u8>),
}

#[cfg(feature = "s3")]
impl Source {
    fn read(&self) -> Result<Vec<u8>, Failed> {
        match self {
            Source::File(p) => std::fs::read(p).map_err(|e| Failed(format!("{}: {e}", p.display()))),
            Source::Kept(b) => Ok(b.clone()),
        }
    }
}

/// What the bucket should hold: the folder's pictures, and when those
/// marked to go were.
#[cfg(feature = "s3")]
struct Folder {
    pictures: BTreeMap<Picture, Source>,
    marks: BTreeMap<Picture, u64>,
}

/// Where each picture's file is, or its mark's.
#[cfg(feature = "s3")]
type Files = BTreeMap<Picture, PathBuf>;

/// The picture files under `data/blobs`, and the marks beside them, by
/// whose they are: `<account>/<id>` and `<account>/<id>.drop`. Only names
/// that are an id: a picture is moved in under its id whole, so one so
/// named is finished. A folder gone while it's read (an account removed
/// meanwhile) has nothing in it.
#[cfg(feature = "s3")]
fn walk(data: &Path) -> Result<(Files, Files), Failed> {
    let (mut pictures, mut marks) = (BTreeMap::new(), BTreeMap::new());
    for dir in std::fs::read_dir(data.join("blobs")).into_iter().flatten().flatten() {
        let Some(account) = dir.file_name().to_str().and_then(unhex) else { continue };
        let entries = match std::fs::read_dir(dir.path()) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Failed(format!("{}: {e}", dir.path().display()))),
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let (id, mark) = match name.strip_suffix(".drop") {
                Some(id) => (id, true),
                None => (name.as_str(), false),
            };
            let Some(id) = unhex(id).and_then(|i| <[u8; 32]>::try_from(i).ok()) else { continue };
            let into = if mark { &mut marks } else { &mut pictures };
            into.insert((account.clone(), id), e.path());
        }
    }
    Ok((pictures, marks))
}

/// The folder's pictures and their marks, by account. Pictures of no
/// account the folder has are counted, and left.
#[cfg(feature = "s3")]
fn folder(src: &Locked, data: &Path, accounts: &[Vec<u8>], done: &mut Done) -> Result<Folder, Failed> {
    let mut pictures = BTreeMap::new();
    for a in accounts {
        for (id, bytes) in src.kept_blobs(a)? {
            pictures.insert((a.clone(), id), Source::Kept(bytes));
        }
    }
    let known: BTreeSet<&Vec<u8>> = accounts.iter().collect();
    let (files, marked) = walk(data)?;
    for (p, path) in files {
        if known.contains(&p.0) {
            pictures.insert(p, Source::File(path));
        } else {
            done.strays += 1;
        }
    }
    // A mark with no picture (swept, half way) marks nothing.
    let marks = marked
        .into_iter()
        .filter(|(p, _)| pictures.contains_key(p))
        .map(|(p, path)| (p, std::fs::read_to_string(path).ok().and_then(|t| t.trim().parse::<u64>().ok()).unwrap_or(0)))
        .collect();
    Ok(Folder { pictures, marks })
}

/// The bucket's ETag is a plain put's: the bytes' MD5.
#[cfg(feature = "s3")]
fn is_md5(etag: &str) -> bool {
    etag.len() == 32 && etag.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What the bucket has there is these bytes: the same size, and the
/// same MD5 where its ETag says one.
#[cfg(feature = "s3")]
fn same(there: Option<&Object>, (len, md5): &(u64, String)) -> bool {
    there.is_some_and(|o| o.size == *len && (!is_md5(&o.etag) || o.etag == *md5))
}

/// Every picture the bucket's folder has, every account's, by key.
#[cfg(feature = "s3")]
fn in_bucket(s3: &S3Blobs) -> Result<BTreeMap<String, Object>, Failed> {
    Ok(s3.all_pictures().map_err(Failed)?.into_iter().map(|(_, o)| (o.key.clone(), o)).collect())
}

/// The bucket, there, its keys good, and not a server's.
#[cfg(feature = "s3")]
fn bucket(config: &S3Config, verify_only: bool) -> Result<S3Blobs, Failed> {
    let s3 = S3Blobs::new(config).map_err(Failed)?;
    s3.check().map_err(Failed)?;
    if s3.served().map_err(Failed)? && !verify_only {
        return Err(Failed("a server has run on this bucket: it holds what the folder doesn't, and nothing is copied over it. Migrate into an empty bucket (or folder in one)".into()));
    }
    Ok(s3)
}

/// The pictures put before the stop, the server running: each finished
/// file the bucket lacks, read as it is. Nothing locked or written,
/// nothing taken from the bucket, nothing checked: the stopped run after
/// does those.
#[cfg(feature = "s3")]
fn pictures_first(plan: &Plan, say: &mut dyn FnMut(&str)) -> Result<Done, Failed> {
    use crate::s3::md5_hex;
    let Some(config) = &plan.s3 else {
        return Err(Failed("--pictures-first copies the pictures: give a bucket (--to-s3)".into()));
    };
    if plan.verify_only {
        return Err(Failed("--pictures-first copies, --verify-only only checks: one or the other".into()));
    }
    if let Some(url) = &plan.postgres
        && Postgres::target(url)?.served()?
    {
        return Err(Failed(SERVED_DB.into()));
    }
    let s3 = bucket(config, false)?;
    say(&format!("{}: pictures read as they are, beside the server running on it; not locked, never written", plan.from.display()));
    let mut done = Done::default();
    let there = in_bucket(&s3)?;
    let (files, _) = walk(&plan.from)?;
    let (total, mut gone) = (files.len(), 0u64);
    for (i, ((a, id), path)) in files.iter().enumerate() {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            // Swept, or its account removed, since the listing.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                gone += 1;
                continue;
            }
            Err(e) => return Err(Failed(format!("{}: {e}", path.display()))),
        };
        let key = s3.key(a, id);
        let sum = (bytes.len() as u64, md5_hex(&bytes));
        done.pictures += 1;
        done.picture_bytes += sum.0;
        if same(there.get(&key), &sum) {
            done.pictures_skipped += 1;
            continue;
        }
        s3.put(&key, &bytes).map_err(Failed)?;
        done.put += 1;
        done.put_bytes += sum.0;
        if done.put.is_multiple_of(100) {
            say(&format!("pictures: {}/{total} looked at, {} put", i + 1, done.put));
        }
    }
    let meanwhile = if gone > 0 { format!("; {gone} gone from the folder while it was read") } else { String::new() };
    say(&format!(
        "pictures first: {} in the folder ({} bytes), {} put now ({} bytes), {} there already{meanwhile}. Next: stop the server, then migrate",
        done.pictures, done.picture_bytes, done.put, done.put_bytes, done.pictures_skipped
    ));
    Ok(done)
}

#[cfg(not(feature = "s3"))]
fn pictures_first(_: &Plan, _: &mut dyn FnMut(&str)) -> Result<Done, Failed> {
    Err(Failed("--pictures-first: this carnet-server was built without S3 (cargo build --features s3)".into()))
}

/// Every picture, and every mark, put where the bucket lacks it; what
/// the bucket has past the folder's, gone; then each compared.
#[cfg(feature = "s3")]
fn pictures(src: &Locked, s3: &S3Blobs, data: &Path, accounts: &[Vec<u8>], verify_only: bool, say: &mut dyn FnMut(&str), done: &mut Done) -> Result<(), Failed> {
    use crate::s3::md5_hex;
    let Folder { pictures, marks } = folder(src, data, accounts, done)?;
    // Each picture's size and MD5, read once.
    let mut sums: BTreeMap<String, (u64, String)> = BTreeMap::new();
    if !verify_only {
        // Every account's, so what isn't the folder's is found too.
        let there = in_bucket(s3)?;
        let total = pictures.len();
        for (i, ((a, id), source)) in pictures.iter().enumerate() {
            let key = s3.key(a, id);
            let bytes = source.read()?;
            let sum = (bytes.len() as u64, md5_hex(&bytes));
            let same = same(there.get(&key), &sum);
            sums.insert(key.clone(), sum);
            if same {
                done.pictures_skipped += 1;
                continue;
            }
            s3.put(&key, &bytes).map_err(Failed)?;
            done.put += 1;
            done.put_bytes += bytes.len() as u64;
            if done.put.is_multiple_of(100) {
                say(&format!("pictures: {}/{total} looked at, {} put", i + 1, done.put));
            }
        }
        // What the bucket has past the folder's: a picture swept since a
        // run before (or since the pictures went first), one of an account
        // removed, a mark taken back.
        let wanted: BTreeSet<String> = pictures.keys().map(|(a, id)| s3.key(a, id)).collect();
        let extra: Vec<String> = there.keys().filter(|k| !wanted.contains(*k)).cloned().collect();
        s3.delete_all(&extra).map_err(Failed)?;
        let theirs: BTreeMap<Picture, u64> = s3.marks().map_err(Failed)?.into_iter().collect();
        for ((a, id), at) in &marks {
            if theirs.get(&(a.clone(), *id)) != Some(at) {
                s3.put_mark(a, id, *at).map_err(Failed)?;
            }
        }
        for (a, id) in theirs.keys().filter(|k| !marks.contains_key(*k)) {
            s3.delete_mark(a, id).map_err(Failed)?;
        }
    }

    // Checked: each picture there, its size and MD5; the marks the same.
    let there = in_bucket(s3)?;
    let mut wrong = Vec::new();
    let mut unsure = 0u64;
    for ((a, id), source) in &pictures {
        let key = s3.key(a, id);
        let (len, md5) = match sums.remove(&key) {
            Some(sum) => sum,
            None => {
                let bytes = source.read()?;
                (bytes.len() as u64, md5_hex(&bytes))
            }
        };
        match there.get(&key) {
            None => wrong.push(format!("{key}: not in the bucket")),
            Some(o) if o.size != len => wrong.push(format!("{key}: {} bytes in the bucket, {len} in the folder", o.size)),
            Some(o) if is_md5(&o.etag) && o.etag != md5 => wrong.push(format!("{key}: its MD5 differs")),
            Some(o) => unsure += u64::from(!is_md5(&o.etag)),
        }
        done.pictures += 1;
        done.picture_bytes += len;
    }
    let wanted: BTreeSet<String> = pictures.keys().map(|(a, id)| s3.key(a, id)).collect();
    for key in there.keys().filter(|k| !wanted.contains(*k)) {
        wrong.push(format!("{key}: in the bucket, not in the folder"));
    }
    let theirs: BTreeMap<Picture, u64> = s3.marks().map_err(Failed)?.into_iter().collect();
    if theirs != marks {
        wrong.push(format!("marks to go: {} in the folder, {} in the bucket, not the same", marks.len(), theirs.len()));
    }
    done.marks = marks.len() as u64;
    if !wrong.is_empty() {
        for w in &wrong {
            say(w);
        }
        return Err(Failed(format!("the bucket is not the same as the folder: {} differences", wrong.len())));
    }
    let how = if unsure > 0 { format!(" ({unsure} by size only: the bucket's ETag isn't their MD5)") } else { String::new() };
    say(&format!(
        "pictures checked: {} ({} bytes) the same in the folder and the bucket, by size and MD5{how}; {} marked to go",
        done.pictures, done.picture_bytes, done.marks
    ));
    if done.strays > 0 {
        say(&format!("{} pictures of no account in the folder were left as they are", done.strays));
    }
    Ok(())
}
