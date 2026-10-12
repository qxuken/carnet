//! Storage on disk: SQLite, a database per account (docs/SYNC.md §6).
//!
//! ```text
//! data/carnet.db                the directory: accounts, invites
//! data/accounts/<account>.db    one account's devices, records, seq
//! ```
//!
//! Accounts never meet, so neither do their files. A query in one
//! account's database cannot reach another's, whatever it forgets to say:
//! isolation by which file is open, not by every query naming an account.
//! It also means:
//! - one account's writes never wait on another's (SQLite has one writer
//!   per file);
//! - an account deleted is a file deleted;
//! - an account handed over or moved is a file copied;
//! - a file damaged is one account's trouble.
//!
//! Reads never make files. A login naming an account that is not there
//! finds nothing and leaves nothing behind; only an account made in the
//! directory gets its file.
//!
//! Open account databases are kept in a cache, the least recently used
//! closed past [`OPEN`]. Each file says its schema's version
//! (`PRAGMA user_version`); a file from a newer server is refused, not
//! guessed at. Everything here is public (keys, hashes) or sealed: a copy
//! of the folder is a backup that can go anywhere.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use carnet_sync::wire::{decode_vv, encode_vv};

use crate::rows::{AccountRows, BaseRow, DeviceRow, InviteRow, PasskeyRow, PasswordRow, RecordRow, RecoveryRow, RekeyRow};
use crate::storage::{Base, Device, Failed, Live, Logged, PasskeySlot, PasswordSlot, Shelf, Span, Storage, covers};

/// A picture as a database kept it: its id, its sealed bytes.
pub type Kept = ([u8; 32], Vec<u8>);

/// Account databases kept open at once.
pub const OPEN: usize = 256;

/// The schemas' version, in every file's `user_version`. 2: recovery.
/// 3: device cards, last seen, key epochs and the keys sealed for them.
/// 4: bases (compaction), records set aside, the log. 5: pictures.
/// 6: rooms removed. 7: passwords. 8: passkeys.
const VERSION: i64 = 8;

const PRAGMAS: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
";

const DIRECTORY: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    id BLOB PRIMARY KEY,
    owner INTEGER NOT NULL DEFAULT 0,
    created INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE IF NOT EXISTS invites (
    hash BLOB PRIMARY KEY,
    owner INTEGER NOT NULL,
    expires INTEGER NOT NULL
);
-- Which account a recovery key is for: the one thing the words alone
-- must find. The wrapped key itself is in the account's own database.
CREATE TABLE IF NOT EXISTS recovery (
    public BLOB PRIMARY KEY,
    account BLOB NOT NULL UNIQUE
);
-- Which account signs in as an email, and which its password's slot
-- key is: what a sign-in must find. The rest is the account's own.
CREATE TABLE IF NOT EXISTS passwords (
    id TEXT PRIMARY KEY,
    account BLOB NOT NULL UNIQUE,
    public BLOB NOT NULL UNIQUE
);
-- Which account a passkey's slot key is for: what signing in with the
-- passkey must find. The slot itself is the account's own.
CREATE TABLE IF NOT EXISTS passkeys (
    public BLOB PRIMARY KEY,
    account BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS passkeys_account ON passkeys (account);
";

const ACCOUNT: &str = "
CREATE TABLE IF NOT EXISTS meta (
    one INTEGER PRIMARY KEY CHECK (one = 1),
    seq INTEGER NOT NULL DEFAULT 0,
    epoch INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO meta (one) VALUES (1);
CREATE TABLE IF NOT EXISTS devices (
    id BLOB PRIMARY KEY,
    public BLOB NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0,
    created INTEGER NOT NULL DEFAULT (unixepoch()),
    card BLOB NOT NULL DEFAULT x'',
    seen INTEGER NOT NULL DEFAULT 0
);
-- Each epoch's vault key, sealed to one device (an empty id: the
-- recovery code). Only that device opens it.
CREATE TABLE IF NOT EXISTS rekeys (
    device BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    sealed BLOB NOT NULL,
    PRIMARY KEY (device, epoch)
);
CREATE TABLE IF NOT EXISTS records (
    room TEXT NOT NULL,
    peer INTEGER NOT NULL,
    span_start INTEGER NOT NULL,
    span_end INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    bytes BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS records_room ON records (room, peer);
CREATE INDEX IF NOT EXISTS records_seq ON records (seq);
-- Each room's base (compaction, §6): live when shelved_by is 0, else set
-- aside by the base with that seq, at shelved_at. Records likewise.
CREATE TABLE IF NOT EXISTS bases (
    seq INTEGER PRIMARY KEY,
    room TEXT NOT NULL,
    vv BLOB NOT NULL,
    bytes BLOB NOT NULL,
    by BLOB NOT NULL,
    at INTEGER NOT NULL,
    verified INTEGER NOT NULL DEFAULT 0,
    undoable INTEGER NOT NULL DEFAULT 1,
    shelved_by INTEGER NOT NULL DEFAULT 0,
    shelved_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS bases_room ON bases (room, shelved_by);
-- Pictures, sealed, by id: kept once however many notes show them.
CREATE TABLE IF NOT EXISTS blobs (
    id BLOB PRIMARY KEY,
    bytes BLOB NOT NULL,
    at INTEGER NOT NULL DEFAULT (unixepoch())
);
-- Rooms whose notes were deleted forever: never taken again.
CREATE TABLE IF NOT EXISTS removed (
    room TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
-- The password (§7): OPAQUE's record, the slot it opens.
CREATE TABLE IF NOT EXISTS password (
    one INTEGER PRIMARY KEY CHECK (one = 1),
    id TEXT NOT NULL,
    record BLOB NOT NULL,
    public BLOB NOT NULL,
    wrapped BLOB NOT NULL,
    card BLOB NOT NULL
);
-- Passkeys (§7): each a slot, named by its signing key.
CREATE TABLE IF NOT EXISTS passkeys (
    public BLOB PRIMARY KEY,
    wrapped BLOB NOT NULL,
    card BLOB NOT NULL,
    made INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS log (
    at INTEGER NOT NULL,
    device BLOB NOT NULL,
    room TEXT NOT NULL,
    what TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS recovery (
    one INTEGER PRIMARY KEY CHECK (one = 1),
    public BLOB NOT NULL,
    wrapped BLOB NOT NULL,
    created INTEGER NOT NULL DEFAULT (unixepoch()),
    card BLOB NOT NULL DEFAULT x''
);
";

/// Columns an account file from an older server lacks, added in place:
/// table, column, its declaration.
const ACCOUNT_COLUMNS: &[(&str, &str, &str)] = &[
    ("meta", "epoch", "INTEGER NOT NULL DEFAULT 0"),
    ("devices", "card", "BLOB NOT NULL DEFAULT x''"),
    ("devices", "seen", "INTEGER NOT NULL DEFAULT 0"),
    ("recovery", "card", "BLOB NOT NULL DEFAULT x''"),
    ("records", "shelved_by", "INTEGER NOT NULL DEFAULT 0"),
    ("records", "shelved_at", "INTEGER NOT NULL DEFAULT 0"),
];

/// Each column a file lacks, added: whatever older version it is, it ends
/// as this one's.
fn add_columns(db: &Connection, columns: &[(&str, &str, &str)]) -> Result<(), Failed> {
    for (table, column, decl) in columns {
        let has: bool = db
            .query_row(&format!("SELECT EXISTS (SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1)"), params![column], |r| r.get(0))
            .map_err(failed)?;
        if !has {
            db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}")).map_err(failed)?;
        }
    }
    Ok(())
}

fn failed(e: rusqlite::Error) -> Failed {
    Failed(e.to_string())
}

/// SQLite keeps signed 64-bit integers; peers, counters and seqs are
/// unsigned. Stored as their bits, read back the same.
fn int(n: u64) -> i64 {
    n as i64
}

fn uint(n: i64) -> u64 {
    n as u64
}

/// The schema made, or checked: a file from a newer server is not read.
fn prepare(db: &Connection, schema: &str) -> Result<(), Failed> {
    db.execute_batch(PRAGMAS).map_err(failed)?;
    let v: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(failed)?;
    if v > VERSION {
        return Err(Failed(format!("a database from a newer carnet-server (schema {v}, this reads {VERSION})")));
    }
    db.execute_batch(schema).map_err(failed)?;
    db.execute_batch(&format!("PRAGMA user_version = {VERSION}")).map_err(failed)
}

/// An account's file name: its id in hex, so no id names a path.
fn file_name(account: &[u8]) -> Option<String> {
    (!account.is_empty() && account.len() <= 64).then(|| account.iter().map(|b| format!("{b:02x}")).collect::<String>() + ".db")
}

/// Where the databases are: a folder, or memory (the tests').
enum Place {
    Disk(PathBuf),
    Memory,
}

pub struct Sharded {
    place: Place,
    directory: Connection,
    /// Open account databases, each with when it was last used. Behind a
    /// `RefCell`, so a read (`&self`) can open what it reads.
    open: RefCell<HashMap<Vec<u8>, (Connection, u64)>>,
    clock: Cell<u64>,
}

impl Sharded {
    /// The databases in `data`: the directory made if it is not there.
    pub fn open(data: &Path) -> Result<Self, Failed> {
        // Its own user's alone (`private.rs`): the folders, and each
        // database before its WAL is made.
        crate::private::data_dir(data).map_err(|e| Failed(e.to_string()))?;
        crate::private::dir(&data.join("accounts")).map_err(|e| Failed(e.to_string()))?;
        let path = Self::directory_path(data);
        let directory = Connection::open(&path).map_err(failed)?;
        crate::private::file(&path).map_err(|e| Failed(e.to_string()))?;
        prepare(&directory, DIRECTORY)?;
        Ok(Sharded { place: Place::Disk(data.to_path_buf()), directory, open: RefCell::default(), clock: Cell::new(0) })
    }

    /// Every database in memory. None is ever closed: one closed in
    /// memory is gone.
    pub fn memory() -> Result<Self, Failed> {
        let directory = Connection::open_in_memory().map_err(failed)?;
        prepare(&directory, DIRECTORY)?;
        Ok(Sharded { place: Place::Memory, directory, open: RefCell::default(), clock: Cell::new(0) })
    }

    /// The pictures still kept in `account`'s database, taken out: they
    /// live as files now (`blobs::BlobFiles`), and are moved there once.
    pub fn take_blobs(&self, account: &[u8]) -> Result<Vec<Kept>, Failed> {
        self.with(account, |db| -> Result<Vec<Kept>, Failed> {
            let mut q = db.prepare("SELECT id, bytes FROM blobs").map_err(failed)?;
            let rows = q
                .query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)))
                .map_err(failed)?
                .flatten()
                .filter_map(|(id, b)| Some((<[u8; 32]>::try_from(id).ok()?, b)))
                .collect();
            Ok(rows)
        })?
        .unwrap_or(Ok(Vec::new()))
    }

    /// Those pictures gone from the database, once they're files.
    pub fn drop_blobs(&self, account: &[u8]) -> Result<(), Failed> {
        self.with(account, |db| db.execute("DELETE FROM blobs", []).map(|_| ()).map_err(failed))?.unwrap_or(Ok(()))
    }

    pub fn directory_path(data: &Path) -> PathBuf {
        data.join("carnet.db")
    }

    /// Where an account's database is, on disk.
    pub fn account_path(data: &Path, account: &[u8]) -> Option<PathBuf> {
        Some(data.join("accounts").join(file_name(account)?))
    }

    fn known(&self, account: &[u8]) -> bool {
        self.directory
            .query_row("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = ?1)", params![account], |r| r.get(0))
            .unwrap_or(false)
    }

    /// `f` on `account`'s database, opened (and cached) if it is not.
    /// `None` for an account the directory does not have: then nothing is
    /// opened, and nothing made.
    fn with<T>(&self, account: &[u8], f: impl FnOnce(&Connection) -> T) -> Result<Option<T>, Failed> {
        let now = self.clock.get() + 1;
        self.clock.set(now);
        let mut open = self.open.borrow_mut();
        if let Some(entry) = open.get_mut(account) {
            entry.1 = now;
            return Ok(Some(f(&entry.0)));
        }
        if !self.known(account) {
            return Ok(None);
        }
        let db = match &self.place {
            Place::Disk(data) => {
                let path = Self::account_path(data, account).ok_or_else(|| Failed("an account id past 64 bytes".into()))?;
                let db = Connection::open(&path).map_err(failed)?;
                crate::private::file(&path).map_err(|e| Failed(e.to_string()))?;
                db
            }
            Place::Memory => Connection::open_in_memory().map_err(failed)?,
        };
        prepare(&db, ACCOUNT)?;
        add_columns(&db, ACCOUNT_COLUMNS)?;
        if matches!(self.place, Place::Disk(_)) && open.len() >= OPEN {
            let oldest = open.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                open.remove(&k);
            }
        }
        let entry = open.entry(account.to_vec()).or_insert((db, now));
        Ok(Some(f(&entry.0)))
    }

    /// How many account databases are open now.
    pub fn open_count(&self) -> usize {
        self.open.borrow().len()
    }

    /// The account and everything of it gone: its row, its file.
    pub fn delete_account(&mut self, account: &[u8]) -> Result<(), Failed> {
        self.open.borrow_mut().remove(account);
        self.directory.execute("DELETE FROM accounts WHERE id = ?1", params![account]).map_err(failed)?;
        self.directory.execute("DELETE FROM recovery WHERE account = ?1", params![account]).map_err(failed)?;
        self.directory.execute("DELETE FROM passwords WHERE account = ?1", params![account]).map_err(failed)?;
        self.directory.execute("DELETE FROM passkeys WHERE account = ?1", params![account]).map_err(failed)?;
        if let Place::Disk(data) = &self.place
            && let Some(path) = Self::account_path(data, account)
        {
            for suffix in ["", "-wal", "-shm"] {
                let mut p = path.clone().into_os_string();
                p.push(suffix);
                match std::fs::remove_file(&p) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(Failed(e.to_string())),
                }
            }
        }
        Ok(())
    }

    /// Every database copied into the folder `to`, laid out as the data
    /// folder is (`VACUUM INTO`: consistent while the server runs).
    /// Returns how many accounts went.
    pub fn backup(&self, to: &Path) -> Result<usize, Failed> {
        let Place::Disk(data) = &self.place else { return Err(Failed("nothing on disk to back up".into())) };
        std::fs::create_dir_all(to.join("accounts")).map_err(|e| Failed(e.to_string()))?;
        let into = |db: &Connection, path: PathBuf| -> Result<(), Failed> {
            if path.exists() {
                return Err(Failed(format!("{} is there already", path.display())));
            }
            let path = path.to_str().ok_or_else(|| Failed("a backup path that is not UTF-8".into()))?.to_string();
            db.execute("VACUUM INTO ?1", params![path]).map(|_| ()).map_err(failed)
        };
        into(&self.directory, Self::directory_path(to))?;
        let ids: Vec<Vec<u8>> = {
            let mut q = self.directory.prepare("SELECT id FROM accounts").map_err(failed)?;
            q.query_map([], |r| r.get(0)).map_err(failed)?.flatten().collect()
        };
        for id in &ids {
            let (Some(from), Some(dest)) = (Self::account_path(data, id), Self::account_path(to, id)) else { continue };
            if from.exists() {
                let db = Connection::open_with_flags(&from, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(failed)?;
                into(&db, dest)?;
            }
        }
        // The pictures, files beside the databases: each written whole
        // and never changed, so a copy while the server runs is whole too.
        copy_tree(&data.join("blobs"), &to.join("blobs")).map_err(|e| Failed(e.to_string()))?;
        Ok(ids.len())
    }
}

/// The files as `migrate` reads them, with no server on them: each locked
/// whole while it's read (the directory for as long as this lives, so no
/// server starts on them meanwhile), and nothing written. A file in use
/// is refused, not waited for.
pub struct Locked {
    data: PathBuf,
    directory: Connection,
}

/// `path` opened alone: refused if anything else has it open (a server),
/// or it's from another version of this server.
fn lock(path: &Path) -> Result<Connection, Failed> {
    if !path.exists() {
        return Err(Failed(format!("{}: not there", path.display())));
    }
    let in_use = || Failed(format!("{} is in use: stop the server that has it open, then migrate", path.display()));
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(failed)?;
    db.busy_timeout(std::time::Duration::ZERO).map_err(failed)?;
    // Alone, or not at all: a server holds its files open, WAL and all,
    // for as long as it runs.
    db.query_row("PRAGMA locking_mode = EXCLUSIVE", [], |_| Ok(())).map_err(failed)?;
    db.execute_batch("BEGIN EXCLUSIVE").map_err(|_| in_use())?;
    let v: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(failed)?;
    if v != VERSION {
        let why = if v > VERSION { "a newer carnet-server: migrate with that one" } else { "an older carnet-server: start this one on the data once, stop it, then migrate" };
        return Err(Failed(format!("{} is from {why} (schema {v}, this reads {VERSION})", path.display())));
    }
    Ok(db)
}

impl Locked {
    pub fn open(data: &Path) -> Result<Self, Failed> {
        let directory = lock(&Sharded::directory_path(data))?;
        Ok(Locked { data: data.to_path_buf(), directory })
    }

    /// Every account, the oldest first.
    pub fn accounts(&self) -> Result<Vec<Vec<u8>>, Failed> {
        let mut q = self.directory.prepare("SELECT id FROM accounts ORDER BY created, id").map_err(failed)?;
        let rows = q.query_map([], |r| r.get(0)).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)?;
        Ok(rows)
    }

    pub fn invites(&self) -> Result<Vec<InviteRow>, Failed> {
        let mut q = self.directory.prepare("SELECT hash, owner, expires FROM invites ORDER BY hash").map_err(failed)?;
        let rows: Vec<(Vec<u8>, bool, i64)> = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)?;
        rows.into_iter()
            .map(|(h, owner, expires)| Ok((<[u8; 32]>::try_from(h).map_err(|_| Failed("an invite's hash that isn't 32 bytes".into()))?, owner, uint(expires))))
            .collect()
    }

    /// `f` on the account's own file, locked while it runs.
    fn with_account<T>(&self, account: &[u8], f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, Failed> {
        let path = Sharded::account_path(&self.data, account).ok_or_else(|| Failed("an account id past 64 bytes".into()))?;
        let db = lock(&path)?;
        let out = f(&db).map_err(failed);
        let _ = db.execute_batch("ROLLBACK");
        out
    }

    /// Everything the account keeps, as rows.
    pub fn rows(&self, account: &[u8]) -> Result<AccountRows, Failed> {
        let (owner, created): (bool, i64) =
            self.directory.query_row("SELECT owner, created FROM accounts WHERE id = ?1", params![account], |r| Ok((r.get(0)?, r.get(1)?))).map_err(failed)?;
        self.with_account(account, |db| {
            let (seq, epoch): (i64, u32) = db.query_row("SELECT seq, epoch FROM meta WHERE one = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let all = |sql: &str| -> rusqlite::Result<rusqlite::Statement<'_>> { db.prepare(sql) };
            let devices = all("SELECT id, public, revoked, created, card, seen FROM devices ORDER BY id")?
                .query_map([], |r| Ok(DeviceRow { id: r.get(0)?, public: r.get(1)?, revoked: r.get(2)?, created: uint(r.get(3)?), card: r.get(4)?, seen: uint(r.get(5)?) }))?
                .collect::<Result<_, _>>()?;
            let rekeys = all("SELECT device, epoch, sealed FROM rekeys ORDER BY device, epoch")?
                .query_map([], |r| Ok(RekeyRow { device: r.get(0)?, epoch: r.get(1)?, sealed: r.get(2)? }))?
                .collect::<Result<_, _>>()?;
            let records = all("SELECT seq, room, peer, span_start, span_end, bytes, shelved_by, shelved_at FROM records ORDER BY seq")?
                .query_map([], |r| {
                    Ok(RecordRow {
                        seq: uint(r.get(0)?),
                        room: r.get(1)?,
                        peer: uint(r.get(2)?),
                        start: uint(r.get(3)?),
                        end: uint(r.get(4)?),
                        bytes: r.get(5)?,
                        shelved_by: uint(r.get(6)?),
                        shelved_at: uint(r.get(7)?),
                    })
                })?
                .collect::<Result<_, _>>()?;
            let bases = all("SELECT seq, room, vv, bytes, by, at, verified, undoable, shelved_by, shelved_at FROM bases ORDER BY seq")?
                .query_map([], |r| {
                    Ok(BaseRow {
                        seq: uint(r.get(0)?),
                        room: r.get(1)?,
                        vv: r.get(2)?,
                        bytes: r.get(3)?,
                        by: r.get(4)?,
                        at: uint(r.get(5)?),
                        verified: r.get(6)?,
                        undoable: r.get(7)?,
                        shelved_by: uint(r.get(8)?),
                        shelved_at: uint(r.get(9)?),
                    })
                })?
                .collect::<Result<_, _>>()?;
            let removed = all("SELECT room, at FROM removed ORDER BY room")?.query_map([], |r| Ok((r.get(0)?, uint(r.get(1)?))))?.collect::<Result<_, _>>()?;
            let log = all("SELECT at, device, room, what FROM log ORDER BY rowid")?
                .query_map([], |r| Ok(Logged { at: uint(r.get(0)?), device: r.get(1)?, room: r.get(2)?, what: r.get(3)? }))?
                .collect::<Result<_, _>>()?;
            let recovery = db
                .query_row("SELECT public, wrapped, created, card FROM recovery WHERE one = 1", [], |r| {
                    Ok(RecoveryRow { public: r.get(0)?, wrapped: r.get(1)?, created: uint(r.get(2)?), card: r.get(3)? })
                })
                .optional()?;
            let password = db
                .query_row("SELECT id, record, public, wrapped, card FROM password WHERE one = 1", [], |r| {
                    Ok(PasswordRow { id: r.get(0)?, record: r.get(1)?, public: r.get(2)?, wrapped: r.get(3)?, card: r.get(4)? })
                })
                .optional()?;
            let passkeys = all("SELECT public, wrapped, card, made FROM passkeys ORDER BY made, rowid")?
                .query_map([], |r| Ok(PasskeyRow { public: r.get(0)?, wrapped: r.get(1)?, card: r.get(2)?, made: uint(r.get(3)?) }))?
                .collect::<Result<_, _>>()?;
            Ok(AccountRows {
                id: account.to_vec(),
                owner,
                created: uint(created),
                seq: uint(seq),
                epoch,
                devices,
                rekeys,
                records,
                bases,
                removed,
                log,
                recovery,
                password,
                passkeys,
            })
        })
    }

    /// Pictures a server before kept in the account's own file (a
    /// server's start moves them out; one that never started since still
    /// has them).
    pub fn kept_blobs(&self, account: &[u8]) -> Result<Vec<Kept>, Failed> {
        self.with_account(account, |db| {
            let mut q = db.prepare("SELECT id, bytes FROM blobs ORDER BY id")?;
            let rows = q.query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().filter_map(|(id, b)| Some((<[u8; 32]>::try_from(id).ok()?, b))).collect())
        })
    }
}

/// Every file under `from`, copied to the same place under `to`.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let Ok(entries) = std::fs::read_dir(from) else { return Ok(()) };
    std::fs::create_dir_all(to)?;
    for e in entries {
        let e = e?;
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        } else {
            std::fs::copy(e.path(), to.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// A device's row: public, revoked, card, seen (the first four columns);
/// `None` for a key that is not 32 bytes.
fn device_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Device>> {
    let public: Vec<u8> = r.get(0)?;
    let (revoked, card, seen): (bool, Vec<u8>, i64) = (r.get(1)?, r.get(2)?, r.get(3)?);
    Ok(public.try_into().ok().map(|public| Device { public, revoked, card, seen: uint(seen) }))
}

impl Storage for Sharded {
    fn device(&self, account: &[u8], device: &[u8]) -> Option<Device> {
        let row = self
            .with(account, |db| {
                db.query_row("SELECT public, revoked, card, seen FROM devices WHERE id = ?1", params![device], device_row)
                    .optional()
                    .ok()
                    .flatten()
            })
            .ok()
            .flatten()
            .flatten();
        row.flatten()
    }

    fn devices(&self, account: &[u8]) -> Vec<(Vec<u8>, Device)> {
        self.with(account, |db| -> rusqlite::Result<Vec<(Vec<u8>, Device)>> {
            let mut q = db.prepare("SELECT public, revoked, card, seen, id FROM devices ORDER BY id")?;
            let rows = q.query_map([], |r| Ok((r.get::<_, Vec<u8>>(4)?, device_row(r)?)))?;
            Ok(rows.flatten().filter_map(|(id, d)| Some((id, d?))).collect())
        })
        .ok()
        .flatten()
        .and_then(Result::ok)
        .unwrap_or_default()
    }

    fn set_card(&mut self, account: &[u8], device: &[u8], card: Vec<u8>) -> Result<(), Failed> {
        self.with(account, |db| db.execute("UPDATE devices SET card = ?2 WHERE id = ?1", params![device, card]).map(|_| ()).map_err(failed))?
            .unwrap_or(Ok(()))
    }

    fn set_seen(&mut self, account: &[u8], device: &[u8], now: u64) -> Result<(), Failed> {
        self.with(account, |db| db.execute("UPDATE devices SET seen = ?2 WHERE id = ?1", params![device, int(now)]).map(|_| ()).map_err(failed))?
            .unwrap_or(Ok(()))
    }

    fn epoch(&self, account: &[u8]) -> u32 {
        self.with(account, |db| db.query_row("SELECT epoch FROM meta WHERE one = 1", [], |r| r.get::<_, u32>(0)).unwrap_or(0))
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    fn rotate(
        &mut self,
        account: &[u8],
        revoked: &[u8],
        epoch: u32,
        keys: Vec<(Vec<u8>, Vec<u8>)>,
        cards: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<(), Failed> {
        self.with(account, |db| -> Result<(), Failed> {
            // All of it, or none: a device half removed is not one.
            let tx = db.unchecked_transaction().map_err(failed)?;
            tx.execute("UPDATE devices SET revoked = 1 WHERE id = ?1", params![revoked]).map_err(failed)?;
            tx.execute("UPDATE meta SET epoch = ?1 WHERE one = 1", params![epoch]).map_err(failed)?;
            for (id, sealed) in keys {
                tx.execute("INSERT OR REPLACE INTO rekeys (device, epoch, sealed) VALUES (?1, ?2, ?3)", params![id, epoch, sealed])
                    .map_err(failed)?;
            }
            for (id, card) in cards {
                if id.is_empty() {
                    tx.execute("UPDATE recovery SET card = ?1 WHERE one = 1", params![card]).map_err(failed)?;
                } else if id == carnet_sync::password::SLOT {
                    tx.execute("UPDATE password SET card = ?1 WHERE one = 1", params![card]).map_err(failed)?;
                } else if let Some(public) = carnet_sync::passkey::of_slot(&id) {
                    tx.execute("UPDATE passkeys SET card = ?2 WHERE public = ?1", params![public.as_slice(), card]).map_err(failed)?;
                } else {
                    tx.execute("UPDATE devices SET card = ?2 WHERE id = ?1", params![id, card]).map_err(failed)?;
                }
            }
            tx.commit().map_err(failed)
        })?
        .ok_or_else(|| Failed("no such account".into()))?
    }

    fn rekeys(&self, account: &[u8], device: &[u8]) -> Vec<Vec<u8>> {
        self.with(account, |db| -> rusqlite::Result<Vec<Vec<u8>>> {
            let mut q = db.prepare("SELECT sealed FROM rekeys WHERE device = ?1 ORDER BY epoch")?;
            let rows = q.query_map(params![device], |r| r.get(0))?;
            Ok(rows.flatten().collect())
        })
        .ok()
        .flatten()
        .and_then(Result::ok)
        .unwrap_or_default()
    }

    fn add_device(&mut self, account: &[u8], device: &[u8], public: [u8; 32]) -> Result<(), Failed> {
        self.with(account, |db| {
            let added = db
                .execute("INSERT INTO devices (id, public) VALUES (?1, ?2) ON CONFLICT (id) DO NOTHING", params![device, public.as_slice()])
                .map_err(failed)?;
            if added == 1 {
                return Ok(());
            }
            // The id was there: the same device again is no change; any
            // other (a removed one, another's key) is refused.
            let (had, revoked): (Vec<u8>, bool) = db
                .query_row("SELECT public, revoked FROM devices WHERE id = ?1", params![device], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(failed)?;
            if had == public && !revoked { Ok(()) } else { Err(Failed(crate::storage::DEVICE_TAKEN.into())) }
        })?
        .ok_or_else(|| Failed("no such account".into()))?
    }

    fn revoke_device(&mut self, account: &[u8], device: &[u8]) -> Result<(), Failed> {
        self.with(account, |db| {
            db.execute("UPDATE devices SET revoked = 1 WHERE id = ?1", params![device]).map(|_| ()).map_err(failed)
        })?
        .unwrap_or(Ok(()))
    }

    fn put(&mut self, account: &[u8], room: &str, peer: u64, start: u64, end: u64, record: Vec<u8>) -> Result<Option<u64>, Failed> {
        if self.base(account, room).is_some_and(|b| covers(&b.vv, &[(peer, end)])) {
            return Ok(None);
        }
        self.with(account, |db| -> Result<Option<u64>, Failed> {
            let tx = db.unchecked_transaction().map_err(failed)?;
            let covered: bool = tx
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM records WHERE room = ?1 AND peer = ?2 AND span_start <= ?3 AND ?4 <= span_end
                         AND shelved_by = 0)",
                    params![room, int(peer), int(start), int(end)],
                    |r| r.get(0),
                )
                .map_err(failed)?;
            if covered {
                return Ok(None);
            }
            tx.execute(
                "DELETE FROM records WHERE room = ?1 AND peer = ?2 AND ?3 <= span_start AND span_end <= ?4 AND shelved_by = 0",
                params![room, int(peer), int(start), int(end)],
            )
            .map_err(failed)?;
            let seq: i64 = tx.query_row("UPDATE meta SET seq = seq + 1 RETURNING seq", [], |r| r.get(0)).map_err(failed)?;
            tx.execute(
                "INSERT INTO records (room, peer, span_start, span_end, seq, bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![room, int(peer), int(start), int(end), seq, record],
            )
            .map_err(failed)?;
            tx.commit().map_err(failed)?;
            Ok(Some(uint(seq)))
        })?
        .ok_or_else(|| Failed("no such account".into()))?
    }

    fn spans(&self, account: &[u8], room: &str) -> Vec<Span> {
        self.with(account, |db| {
            let Ok(mut q) = db.prepare_cached("SELECT peer, span_start, span_end, seq, bytes FROM records WHERE room = ?1 AND shelved_by = 0 ORDER BY seq")
            else {
                return Vec::new();
            };
            q.query_map(params![room], |r| {
                Ok(Span { peer: uint(r.get(0)?), start: uint(r.get(1)?), end: uint(r.get(2)?), seq: uint(r.get(3)?), record: r.get(4)? })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
        })
        .ok()
        .flatten()
        .unwrap_or_default()
    }

    fn live(&self, account: &[u8], room: &str) -> Live {
        self.with(account, |db| {
            db.query_row(
                "SELECT (SELECT COUNT(*) FROM records WHERE room = ?1 AND shelved_by = 0),
                        (SELECT COALESCE(SUM(LENGTH(bytes)), 0) FROM records WHERE room = ?1 AND shelved_by = 0),
                        EXISTS (SELECT 1 FROM bases WHERE room = ?1 AND shelved_by = 0)",
                params![room],
                |r| Ok(Live { records: uint(r.get(0)?), bytes: uint(r.get(1)?), based: r.get(2)? }),
            )
            .unwrap_or_default()
        })
        .ok()
        .flatten()
        .unwrap_or_default()
    }

    fn room_count(&self, account: &[u8]) -> u64 {
        self.with(account, |db| {
            db.query_row(
                "SELECT COUNT(*) FROM (SELECT room FROM records WHERE shelved_by = 0 UNION SELECT room FROM bases WHERE shelved_by = 0)",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(uint)
            .unwrap_or(0)
        })
        .ok()
        .flatten()
        .unwrap_or(0)
    }

    fn changed_since(&self, account: &[u8], seq: u64) -> Vec<(String, u64)> {
        self.with(account, |db| {
            let Ok(mut q) =
                db.prepare_cached(
                    "SELECT room, MAX(seq) AS newest FROM (
                         SELECT room, seq FROM records WHERE shelved_by = 0
                         UNION ALL SELECT room, seq FROM bases WHERE shelved_by = 0)
                     GROUP BY room HAVING newest > ?1 ORDER BY newest",
                )
            else {
                return Vec::new();
            };
            q.query_map(params![int(seq)], |r| Ok((r.get::<_, String>(0)?, uint(r.get(1)?))))
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default()
        })
        .ok()
        .flatten()
        .unwrap_or_default()
    }

    fn base(&self, account: &[u8], room: &str) -> Option<Base> {
        self.with(account, |db| {
            db.query_row(
                "SELECT seq, vv, bytes, by, at, verified, undoable FROM bases WHERE room = ?1 AND shelved_by = 0",
                params![room],
                |r| {
                    Ok(Base {
                        seq: uint(r.get(0)?),
                        vv: decode_vv(&r.get::<_, Vec<u8>>(1)?).unwrap_or_default(),
                        record: r.get(2)?,
                        by: r.get(3)?,
                        at: uint(r.get(4)?),
                        verified: r.get(5)?,
                        undoable: r.get(6)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
        })
        .ok()
        .flatten()
        .flatten()
    }

    fn set_base(&mut self, account: &[u8], room: &str, vv: Vec<(u64, u64)>, record: Vec<u8>, by: &[u8], now: u64) -> Result<u64, Failed> {
        let live = self.spans(account, room);
        self.with(account, |db| -> Result<u64, Failed> {
            let tx = db.unchecked_transaction().map_err(failed)?;
            let seq: i64 = tx.query_row("UPDATE meta SET seq = seq + 1 RETURNING seq", [], |r| r.get(0)).map_err(failed)?;
            tx.execute("UPDATE bases SET shelved_by = ?2, shelved_at = ?3 WHERE room = ?1 AND shelved_by = 0", params![room, seq, int(now)])
                .map_err(failed)?;
            for s in live.iter().filter(|s| covers(&vv, &[(s.peer, s.end)])) {
                tx.execute(
                    "UPDATE records SET shelved_by = ?2, shelved_at = ?3 WHERE seq = ?1",
                    params![int(s.seq), seq, int(now)],
                )
                .map_err(failed)?;
            }
            tx.execute(
                "INSERT INTO bases (seq, room, vv, bytes, by, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![seq, room, encode_vv(&vv), record, by, int(now)],
            )
            .map_err(failed)?;
            tx.commit().map_err(failed)?;
            Ok(uint(seq))
        })?
        .ok_or_else(|| Failed("no such account".into()))?
    }

    fn verify_base(&mut self, account: &[u8], room: &str, seq: u64) -> Result<(), Failed> {
        self.with(account, |db| {
            db.execute("UPDATE bases SET verified = 1 WHERE seq = ?1 AND room = ?2 AND shelved_by = 0", params![int(seq), room])
                .map(|_| ())
                .map_err(failed)
        })?
        .unwrap_or(Ok(()))
    }

    fn roll_back(&mut self, account: &[u8], room: &str) -> Result<bool, Failed> {
        let Some(b) = self.base(account, room).filter(|b| b.undoable) else { return Ok(false) };
        self.with(account, |db| -> Result<bool, Failed> {
            let tx = db.unchecked_transaction().map_err(failed)?;
            tx.execute("DELETE FROM bases WHERE seq = ?1", params![int(b.seq)]).map_err(failed)?;
            for table in ["bases", "records"] {
                tx.execute(
                    &format!("UPDATE {table} SET shelved_by = 0, shelved_at = 0 WHERE shelved_by = ?1 AND room = ?2"),
                    params![int(b.seq), room],
                )
                .map_err(failed)?;
            }
            tx.commit().map_err(failed)?;
            Ok(true)
        })?
        .ok_or_else(|| Failed("no such account".into()))?
    }

    fn shelf(&self, account: &[u8], room: &str) -> Shelf {
        self.with(account, |db| {
            db.query_row(
                "SELECT COUNT(*), COALESCE(SUM(LENGTH(bytes)), 0), COALESCE(MIN(shelved_at), 0) FROM (
                     SELECT bytes, shelved_at FROM records WHERE room = ?1 AND shelved_by > 0
                     UNION ALL SELECT bytes, shelved_at FROM bases WHERE room = ?1 AND shelved_by > 0)",
                params![room],
                |r| Ok(Shelf { count: uint(r.get(0)?), bytes: uint(r.get(1)?), since: uint(r.get(2)?) }),
            )
            .unwrap_or_default()
        })
        .ok()
        .flatten()
        .unwrap_or_default()
    }

    fn rooms(&self, account: &[u8]) -> Vec<String> {
        self.with(account, |db| -> rusqlite::Result<Vec<String>> {
            let mut q = db.prepare(
                "SELECT room FROM records WHERE shelved_by = 0 UNION SELECT room FROM bases WHERE shelved_by = 0 ORDER BY room",
            )?;
            let rows = q.query_map([], |r| r.get(0))?;
            Ok(rows.flatten().collect())
        })
        .ok()
        .flatten()
        .and_then(Result::ok)
        .unwrap_or_default()
    }

    fn remove_room(&mut self, account: &[u8], room: &str, now: u64) -> Result<(), Failed> {
        self.with(account, |db| -> Result<(), Failed> {
            let tx = db.unchecked_transaction().map_err(failed)?;
            for table in ["records", "bases"] {
                tx.execute(&format!("DELETE FROM {table} WHERE room = ?1"), params![room]).map_err(failed)?;
            }
            tx.execute("INSERT OR IGNORE INTO removed (room, at) VALUES (?1, ?2)", params![room, int(now)]).map_err(failed)?;
            tx.commit().map_err(failed)
        })?
        .ok_or_else(|| Failed("no such account".into()))?
    }

    fn usage(&self, account: &[u8]) -> u64 {
        self.with(account, |db| {
            db.query_row(
                "SELECT (SELECT COALESCE(SUM(LENGTH(bytes)), 0) FROM records) + (SELECT COALESCE(SUM(LENGTH(bytes)), 0) FROM bases)",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(uint)
            .unwrap_or(0)
        })
        .ok()
        .flatten()
        .unwrap_or(0)
    }

    fn is_removed(&self, account: &[u8], room: &str) -> bool {
        self.with(account, |db| db.query_row("SELECT EXISTS (SELECT 1 FROM removed WHERE room = ?1)", params![room], |r| r.get(0)).unwrap_or(false))
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    fn purge(&mut self, before: u64) -> Result<(), Failed> {
        let ids: Vec<Vec<u8>> = {
            let mut q = self.directory.prepare("SELECT id FROM accounts").map_err(failed)?;
            q.query_map([], |r| r.get(0)).map_err(failed)?.flatten().collect()
        };
        for id in ids {
            self.with(&id, |db| -> Result<(), Failed> {
                let tx = db.unchecked_transaction().map_err(failed)?;
                // A base whose set-aside goes can't be rolled back after.
                tx.execute(
                    "UPDATE bases SET undoable = 0 WHERE seq IN (
                         SELECT shelved_by FROM records WHERE shelved_by > 0 AND shelved_at < ?1
                         UNION SELECT shelved_by FROM bases WHERE shelved_by > 0 AND shelved_at < ?1)",
                    params![int(before)],
                )
                .map_err(failed)?;
                for table in ["records", "bases"] {
                    tx.execute(&format!("DELETE FROM {table} WHERE shelved_by > 0 AND shelved_at < ?1"), params![int(before)])
                        .map_err(failed)?;
                }
                tx.commit().map_err(failed)
            })?
            .unwrap_or(Ok(()))?;
        }
        Ok(())
    }

    fn log(&mut self, account: &[u8], entry: Logged) -> Result<(), Failed> {
        self.with(account, |db| {
            db.execute(
                "INSERT INTO log (at, device, room, what) VALUES (?1, ?2, ?3, ?4)",
                params![int(entry.at), entry.device, entry.room, entry.what],
            )
            .map(|_| ())
            .map_err(failed)
        })?
        .ok_or_else(|| Failed("no such account".into()))?
    }

    fn logged(&self, account: &[u8]) -> Vec<Logged> {
        self.with(account, |db| -> rusqlite::Result<Vec<Logged>> {
            let mut q = db.prepare("SELECT at, device, room, what FROM log ORDER BY rowid DESC LIMIT 50")?;
            let rows = q.query_map([], |r| Ok(Logged { at: uint(r.get(0)?), device: r.get(1)?, room: r.get(2)?, what: r.get(3)? }))?;
            Ok(rows.flatten().collect())
        })
        .ok()
        .flatten()
        .and_then(Result::ok)
        .unwrap_or_default()
    }

    fn seq(&self, account: &[u8]) -> u64 {
        self.with(account, |db| db.query_row("SELECT seq FROM meta", [], |r| r.get::<_, i64>(0)).ok())
            .ok()
            .flatten()
            .flatten()
            .map_or(0, uint)
    }

    fn accounts(&self) -> usize {
        self.directory.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get::<_, i64>(0)).map_or(0, |n| n as usize)
    }

    fn add_account(&mut self, account: &[u8], owner: bool) -> Result<(), Failed> {
        if file_name(account).is_none() {
            return Err(Failed("an account id is 1 to 64 bytes".into()));
        }
        self.directory
            .execute("INSERT INTO accounts (id, owner) VALUES (?1, ?2) ON CONFLICT (id) DO NOTHING", params![account, owner])
            .map_err(failed)?;
        // Its file made now, so the first read finds it.
        self.with(account, |_| ()).map(|_| ())
    }

    fn add_invite(&mut self, hash: [u8; 32], owner: bool, expires: u64) -> Result<(), Failed> {
        self.directory
            .execute("INSERT OR REPLACE INTO invites (hash, owner, expires) VALUES (?1, ?2, ?3)", params![hash.as_slice(), owner, int(expires)])
            .map(|_| ())
            .map_err(failed)
    }

    fn take_invite(&mut self, hash: [u8; 32], now: u64) -> Result<Option<bool>, Failed> {
        let row: Option<(bool, i64)> = self
            .directory
            .query_row("DELETE FROM invites WHERE hash = ?1 RETURNING owner, expires", params![hash.as_slice()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(failed)?;
        Ok(row.filter(|&(_, expires)| now < uint(expires)).map(|(owner, _)| owner))
    }

    fn invites(&self) -> Vec<([u8; 32], bool, u64)> {
        let Ok(mut q) = self.directory.prepare("SELECT hash, owner, expires FROM invites ORDER BY expires, hash") else { return Vec::new() };
        q.query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, bool>(1)?, r.get::<_, i64>(2)?)))
            .map(|rows| rows.flatten().filter_map(|(h, o, e)| Some((<[u8; 32]>::try_from(h).ok()?, o, uint(e)))).collect())
            .unwrap_or_default()
    }

    fn drop_invite(&mut self, hash: [u8; 32]) -> Result<bool, Failed> {
        self.directory.execute("DELETE FROM invites WHERE hash = ?1", params![hash.as_slice()]).map(|n| n > 0).map_err(failed)
    }

    fn is_owner(&self, account: &[u8]) -> bool {
        self.directory.query_row("SELECT owner FROM accounts WHERE id = ?1", params![account], |r| r.get(0)).unwrap_or(false)
    }

    fn has_account(&self, account: &[u8]) -> bool {
        self.directory.query_row("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = ?1)", params![account], |r| r.get(0)).unwrap_or(true)
    }

    fn account_list(&self) -> Vec<(Vec<u8>, bool, u64)> {
        let Ok(mut q) = self.directory.prepare("SELECT id, owner, created FROM accounts ORDER BY created, id") else { return Vec::new() };
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, uint(r.get(2)?)))).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    fn remove_account(&mut self, account: &[u8]) -> Result<(), Failed> {
        self.delete_account(account)
    }

    fn set_recovery(&mut self, account: &[u8], public: [u8; 32], wrapped: Vec<u8>, card: Vec<u8>) -> Result<(), Failed> {
        // Another account's key: refused before anything is written.
        let had: Option<Vec<u8>> = self
            .directory
            .query_row("SELECT account FROM recovery WHERE public = ?1", params![public.as_slice()], |r| r.get(0))
            .optional()
            .map_err(failed)?;
        if had.is_some_and(|a| a != account) {
            return Err(Failed(crate::storage::RECOVERY_TAKEN.into()));
        }
        self.with(account, |db| -> Result<(), Failed> {
            let tx = db.unchecked_transaction().map_err(failed)?;
            tx.execute(
                "INSERT INTO recovery (one, public, wrapped, card) VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT (one) DO UPDATE SET public = excluded.public, wrapped = excluded.wrapped,
                     card = excluded.card, created = unixepoch()",
                params![public.as_slice(), wrapped, card],
            )
            .map_err(failed)?;
            // The new code's ring has every epoch: what was sealed to the
            // last code goes.
            tx.execute("DELETE FROM rekeys WHERE device = x''", []).map_err(failed)?;
            tx.commit().map_err(failed)
        })?
        .ok_or_else(|| Failed("no such account".into()))??;
        let tx = self.directory.unchecked_transaction().map_err(failed)?;
        // Its own last key goes; a plain insert, so a key another
        // account's row has fails rather than moves.
        tx.execute("DELETE FROM recovery WHERE account = ?1", params![account]).map_err(failed)?;
        tx.execute("INSERT INTO recovery (public, account) VALUES (?1, ?2)", params![public.as_slice(), account]).map_err(failed)?;
        tx.commit().map_err(failed)
    }

    fn recovery(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        let account: Vec<u8> = self
            .directory
            .query_row("SELECT account FROM recovery WHERE public = ?1", params![public.as_slice()], |r| r.get(0))
            .optional()
            .ok()
            .flatten()?;
        // The account's own database says what its code is now: an index
        // row left over from a code replaced finds nothing.
        let wrapped = self
            .with(&account, |db| {
                db.query_row("SELECT wrapped FROM recovery WHERE public = ?1", params![public.as_slice()], |r| r.get::<_, Vec<u8>>(0))
                    .optional()
                    .ok()
                    .flatten()
            })
            .ok()
            .flatten()
            .flatten()?;
        Some((account, wrapped))
    }

    fn set_password(&mut self, account: &[u8], slot: PasswordSlot) -> Result<(), Failed> {
        self.with(account, |db| -> Result<(), Failed> {
            let tx = db.unchecked_transaction().map_err(failed)?;
            tx.execute(
                "INSERT OR REPLACE INTO password (one, id, record, public, wrapped, card) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
                params![slot.id, slot.record, slot.public.as_slice(), slot.wrapped, slot.card],
            )
            .map_err(failed)?;
            // The new slot's ring has every epoch: what was sealed to the
            // last one goes.
            tx.execute("DELETE FROM rekeys WHERE device = ?1", params![carnet_sync::password::SLOT]).map_err(failed)?;
            tx.commit().map_err(failed)
        })?
        .ok_or_else(|| Failed("no such account".into()))??;
        let tx = self.directory.unchecked_transaction().map_err(failed)?;
        tx.execute("DELETE FROM passwords WHERE account = ?1", params![account]).map_err(failed)?;
        tx.execute("INSERT INTO passwords (id, account, public) VALUES (?1, ?2, ?3)", params![slot.id, account, slot.public.as_slice()])
            .map_err(failed)?;
        tx.commit().map_err(failed)
    }

    fn drop_password(&mut self, account: &[u8]) -> Result<(), Failed> {
        self.with(account, |db| -> Result<(), Failed> {
            db.execute("DELETE FROM password", []).map_err(failed)?;
            db.execute("DELETE FROM rekeys WHERE device = ?1", params![carnet_sync::password::SLOT]).map_err(failed)?;
            Ok(())
        })?
        .unwrap_or(Ok(()))?;
        self.directory.execute("DELETE FROM passwords WHERE account = ?1", params![account]).map_err(failed)?;
        Ok(())
    }

    fn password_of(&self, id: &str) -> Option<(Vec<u8>, Vec<u8>)> {
        let account: Vec<u8> =
            self.directory.query_row("SELECT account FROM passwords WHERE id = ?1", params![id], |r| r.get(0)).optional().ok().flatten()?;
        let record = self
            .with(&account, |db| db.query_row("SELECT record FROM password WHERE id = ?1", params![id], |r| r.get::<_, Vec<u8>>(0)).optional().ok().flatten())
            .ok()
            .flatten()
            .flatten()?;
        Some((account, record))
    }

    fn password_slot(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        let account: Vec<u8> = self
            .directory
            .query_row("SELECT account FROM passwords WHERE public = ?1", params![public.as_slice()], |r| r.get(0))
            .optional()
            .ok()
            .flatten()?;
        let wrapped = self
            .with(&account, |db| {
                db.query_row("SELECT wrapped FROM password WHERE public = ?1", params![public.as_slice()], |r| r.get::<_, Vec<u8>>(0)).optional().ok().flatten()
            })
            .ok()
            .flatten()
            .flatten()?;
        Some((account, wrapped))
    }

    fn password_card(&self, account: &[u8]) -> Vec<u8> {
        self.with(account, |db| db.query_row("SELECT card FROM password WHERE one = 1", [], |r| r.get::<_, Vec<u8>>(0)).unwrap_or_default())
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn recovery_card(&self, account: &[u8]) -> Vec<u8> {
        self.with(account, |db| db.query_row("SELECT card FROM recovery WHERE one = 1", [], |r| r.get::<_, Vec<u8>>(0)).unwrap_or_default())
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn add_passkey(&mut self, account: &[u8], slot: PasskeySlot) -> Result<(), Failed> {
        self.with(account, |db| -> Result<(), Failed> {
            let tx = db.unchecked_transaction().map_err(failed)?;
            tx.execute(
                "INSERT INTO passkeys (public, wrapped, card, made) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (public) DO UPDATE SET wrapped = excluded.wrapped, card = excluded.card",
                params![slot.public.as_slice(), slot.wrapped, slot.card, int(slot.made)],
            )
            .map_err(failed)?;
            // The slot's ring has every epoch: what was sealed to it before
            // goes.
            tx.execute("DELETE FROM rekeys WHERE device = ?1", params![carnet_sync::passkey::slot(&slot.public)]).map_err(failed)?;
            tx.commit().map_err(failed)
        })?
        .ok_or_else(|| Failed("no such account".into()))??;
        self.directory
            .execute("INSERT OR REPLACE INTO passkeys (public, account) VALUES (?1, ?2)", params![slot.public.as_slice(), account])
            .map(|_| ())
            .map_err(failed)
    }

    fn drop_passkey(&mut self, account: &[u8], public: &[u8; 32]) -> Result<bool, Failed> {
        let had = self
            .with(account, |db| -> Result<bool, Failed> {
                let tx = db.unchecked_transaction().map_err(failed)?;
                let n = tx.execute("DELETE FROM passkeys WHERE public = ?1", params![public.as_slice()]).map_err(failed)?;
                tx.execute("DELETE FROM rekeys WHERE device = ?1", params![carnet_sync::passkey::slot(public)]).map_err(failed)?;
                tx.commit().map_err(failed)?;
                Ok(n > 0)
            })?
            .unwrap_or(Ok(false))?;
        self.directory
            .execute("DELETE FROM passkeys WHERE public = ?1 AND account = ?2", params![public.as_slice(), account])
            .map_err(failed)?;
        Ok(had)
    }

    fn passkeys(&self, account: &[u8]) -> Vec<PasskeySlot> {
        self.with(account, |db| -> rusqlite::Result<Vec<PasskeySlot>> {
            let mut q = db.prepare("SELECT public, wrapped, card, made FROM passkeys ORDER BY made, rowid")?;
            let rows = q.query_map([], |r| {
                let public: Vec<u8> = r.get(0)?;
                Ok((public, r.get(1)?, r.get(2)?, uint(r.get(3)?)))
            })?;
            Ok(rows
                .flatten()
                .filter_map(|(public, wrapped, card, made)| Some(PasskeySlot { public: public.try_into().ok()?, wrapped, card, made }))
                .collect())
        })
        .ok()
        .flatten()
        .and_then(Result::ok)
        .unwrap_or_default()
    }

    fn passkey_slot(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        let account: Vec<u8> = self
            .directory
            .query_row("SELECT account FROM passkeys WHERE public = ?1", params![public.as_slice()], |r| r.get(0))
            .optional()
            .ok()
            .flatten()?;
        // The account's own database says what its passkeys are now.
        let wrapped = self
            .with(&account, |db| {
                db.query_row("SELECT wrapped FROM passkeys WHERE public = ?1", params![public.as_slice()], |r| r.get::<_, Vec<u8>>(0)).optional().ok().flatten()
            })
            .ok()
            .flatten()
            .flatten()?;
        Some((account, wrapped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("carnet-sharded-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn accounts_live_in_files_of_their_own() {
        let data = temp("own");
        let mut s = Sharded::open(&data).unwrap();
        s.add_account(b"alice", true).unwrap();
        s.add_account(b"bob", false).unwrap();
        s.put(b"alice", "r", 1, 0, 3, b"ALICE-SEALED".to_vec()).unwrap();
        s.put(b"bob", "r", 1, 0, 3, b"BOB-SEALED".to_vec()).unwrap();
        assert_eq!(s.spans(b"alice", "r").len(), 1);
        assert_eq!(s.spans(b"bob", "r")[0].record, b"BOB-SEALED");
        assert_eq!(s.seq(b"bob"), 1, "each account counts its own");
        drop(s);
        let alice = std::fs::read(Sharded::account_path(&data, b"alice").unwrap()).unwrap_or_default();
        let wal = std::fs::read(format!("{}-wal", Sharded::account_path(&data, b"alice").unwrap().display())).unwrap_or_default();
        let both = [alice, wal].concat();
        assert!(both.windows(12).any(|w| w == b"ALICE-SEALED"));
        assert!(!both.windows(10).any(|w| w == b"BOB-SEALED"), "bob's record is not in alice's file");
        let _ = std::fs::remove_dir_all(&data);
    }

    /// An account removed is its file gone, its WAL with it; the others
    /// stay, and the owner is known.
    #[test]
    fn an_account_removed_is_its_file_deleted() {
        let data = temp("removed");
        let mut s = Sharded::open(&data).unwrap();
        s.add_account(b"alice", true).unwrap();
        s.add_account(b"bob", false).unwrap();
        s.add_device(b"bob", b"dev", [7; 32]).unwrap();
        s.set_recovery(b"bob", [5; 32], vec![1], vec![2]).unwrap();
        s.put(b"bob", "r", 1, 0, 3, b"BOB-SEALED".to_vec()).unwrap();
        assert!(s.is_owner(b"alice") && !s.is_owner(b"bob"));
        assert_eq!(s.account_list().iter().map(|(id, owner, _)| (id.clone(), *owner)).collect::<Vec<_>>(), [(b"alice".to_vec(), true), (b"bob".to_vec(), false)]);
        let bob = Sharded::account_path(&data, b"bob").unwrap();
        assert!(bob.exists());
        s.remove_account(b"bob").unwrap();
        assert!(!bob.exists() && !PathBuf::from(format!("{}-wal", bob.display())).exists());
        assert!(s.devices(b"bob").is_empty() && s.spans(b"bob", "r").is_empty() && s.recovery(&[5; 32]).is_none());
        assert_eq!(s.accounts(), 1);
        assert!(Sharded::account_path(&data, b"alice").unwrap().exists());
        // Invites: listed, taken back.
        s.add_invite([1; 32], false, 100).unwrap();
        assert_eq!(s.invites(), [([1; 32], false, 100)]);
        assert!(s.drop_invite([1; 32]).unwrap() && !s.drop_invite([1; 32]).unwrap());
        let _ = std::fs::remove_dir_all(&data);
    }

    /// An account file from the server before devices had cards (schema
    /// 2) opens, gains what it lacks, and keeps what it had.
    #[test]
    fn an_older_account_file_is_brought_up_to_date() {
        let data = temp("older");
        let mut s = Sharded::open(&data).unwrap();
        s.add_account(b"alice", true).unwrap();
        s.add_device(b"alice", b"dev", [7; 32]).unwrap();
        s.put(b"alice", "r", 1, 0, 3, b"SEALED".to_vec()).unwrap();
        drop(s);
        // Back to how schema 2 was.
        let db = Connection::open(Sharded::account_path(&data, b"alice").unwrap()).unwrap();
        db.execute_batch(
            "ALTER TABLE devices DROP COLUMN card; ALTER TABLE devices DROP COLUMN seen;
             ALTER TABLE meta DROP COLUMN epoch; ALTER TABLE recovery DROP COLUMN card;
             DROP TABLE rekeys; DROP TABLE passkeys; PRAGMA user_version = 2;",
        )
        .unwrap();
        drop(db);

        let mut s = Sharded::open(&data).unwrap();
        let d = s.device(b"alice", b"dev").unwrap();
        assert_eq!((d.public, d.revoked, d.card.len(), d.seen), ([7; 32], false, 0, 0));
        assert_eq!(s.spans(b"alice", "r")[0].record, b"SEALED");
        assert_eq!(s.epoch(b"alice"), 0);
        s.set_card(b"alice", b"dev", vec![1, 2]).unwrap();
        s.set_seen(b"alice", b"dev", 99).unwrap();
        s.add_device(b"alice", b"lost", [8; 32]).unwrap();
        s.rotate(b"alice", b"lost", 1, vec![(b"dev".to_vec(), vec![5])], vec![(b"dev".to_vec(), vec![3])]).unwrap();
        assert_eq!(s.epoch(b"alice"), 1);
        assert_eq!(s.rekeys(b"alice", b"dev"), [vec![5]]);
        assert_eq!(s.device(b"alice", b"dev").unwrap().card, [3]);
        assert!(s.device(b"alice", b"lost").unwrap().revoked);
        assert_eq!(s.devices(b"alice").len(), 2);
        // Passkeys, which it never had.
        let slot = PasskeySlot { public: [4; 32], wrapped: vec![1], card: vec![2], made: 5 };
        s.add_passkey(b"alice", slot.clone()).unwrap();
        assert_eq!(s.passkeys(b"alice"), [slot]);
        assert_eq!(s.passkey_slot(&[4; 32]), Some((b"alice".to_vec(), vec![1])));
        let v: i64 = s.with(b"alice", |db| db.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()).unwrap().unwrap();
        assert_eq!(v, VERSION);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_read_for_an_unknown_account_makes_nothing() {
        let data = temp("unknown");
        let mut s = Sharded::open(&data).unwrap();
        assert_eq!(s.device(b"nobody", b"dev"), None);
        assert_eq!(s.seq(b"nobody"), 0);
        assert!(s.spans(b"nobody", "r").is_empty());
        assert!(s.put(b"nobody", "r", 1, 0, 1, vec![1]).is_err(), "no account, no records");
        assert!(s.add_device(b"nobody", b"dev", [0; 32]).is_err());
        assert_eq!(std::fs::read_dir(data.join("accounts")).unwrap().count(), 0, "no file made");
        assert_eq!(s.open_count(), 0);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn an_account_deleted_is_a_file_gone_and_a_backup_is_a_folder() {
        let data = temp("delete");
        let mut s = Sharded::open(&data).unwrap();
        for a in [b"a1", b"a2"] {
            s.add_account(a, false).unwrap();
            s.add_device(a, b"d", [7; 32]).unwrap();
            s.put(a, "r", 1, 0, 2, vec![9; 8]).unwrap();
        }
        // A picture, a file beside the databases (blobs::BlobFiles).
        std::fs::create_dir_all(data.join("blobs").join("6132")).unwrap();
        std::fs::write(data.join("blobs").join("6132").join("07"), b"SEALED").unwrap();
        let copy = temp("delete-copy");
        assert_eq!(s.backup(&copy).unwrap(), 2);
        assert_eq!(std::fs::read(copy.join("blobs").join("6132").join("07")).unwrap(), b"SEALED", "pictures too");
        let restored = Sharded::open(&copy).unwrap();
        assert_eq!(restored.spans(b"a2", "r").len(), 1);
        assert_eq!(restored.device(b"a1", b"d").map(|d| d.public), Some([7; 32]));
        s.delete_account(b"a1").unwrap();
        assert!(!Sharded::account_path(&data, b"a1").unwrap().exists());
        assert_eq!(s.device(b"a1", b"d"), None);
        assert_eq!(s.accounts(), 1);
        assert_eq!(s.spans(b"a2", "r").len(), 1, "the other account is untouched");
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&copy);
    }

    #[test]
    fn past_the_cache_accounts_close_and_open_again() {
        let data = temp("cache");
        let mut s = Sharded::open(&data).unwrap();
        let ids: Vec<Vec<u8>> = (0..OPEN + 10).map(|i| format!("acct-{i}").into_bytes()).collect();
        for id in &ids {
            s.add_account(id, false).unwrap();
            s.put(id, "r", 1, 0, 1, id.clone()).unwrap();
        }
        assert!(s.open_count() <= OPEN);
        for id in &ids {
            assert_eq!(s.spans(id, "r")[0].record, *id, "reopened and read");
        }
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_file_from_a_newer_server_is_not_read() {
        let data = temp("newer");
        let mut s = Sharded::open(&data).unwrap();
        s.add_account(b"a", false).unwrap();
        drop(s);
        let db = Connection::open(Sharded::account_path(&data, b"a").unwrap()).unwrap();
        db.execute_batch("PRAGMA user_version = 99").unwrap();
        drop(db);
        let s = Sharded::open(&data).unwrap();
        assert_eq!(s.device(b"a", b"d"), None);
        assert!(s.with(b"a", |_| ()).is_err());
        let _ = std::fs::remove_dir_all(&data);
    }
}
