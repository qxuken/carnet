//! Storage on a database server: Postgres, every account in one schema
//! (docs/SYNC.md §6, §11). For a hosted server past what one machine's
//! SQLite files carry; a server of one's own stays on [`Sharded`]. The
//! `postgres` feature builds it.
//!
//! What [`Sharded`] keeps in a file per account is here a table per kind,
//! each row naming its account, and gone with it (`ON DELETE CASCADE`).
//! Every query says which account: that's what isolates them now. The
//! rows are the same, and so is every answer the [`Storage`] trait gives;
//! the storage tests run on both (`tests/common`, with
//! `CARNET_TEST_POSTGRES`).
//!
//! The hub calls from its one thread and waits for each answer: one
//! connection, its statements prepared once, made again if it drops.
//!
//! ```text
//! accounts   id, owner, created, seq, epoch           the directory and each file's meta
//! invites    hash, owner, expires
//! devices    account, id, public, revoked, created, card, seen
//! rekeys     account, device, epoch, sealed
//! records    account, seq, room, peer, span_start, span_end, bytes, shelved_by, shelved_at
//! bases      account, seq, room, vv, bytes, made_by, at, verified, undoable, shelved_by, shelved_at
//! removed    account, room, at
//! log        account, n, at, device, room, what
//! recovery   account, public, wrapped, created, card
//! passwords  account, id, record, public, wrapped, card
//! passkeys   account, public, wrapped, card, made, n
//! opened     at, what                                 who opened it to serve, and when
//! ```
//!
//! **Adding to it** (a new kind of row, say a passkey's): a new entry at
//! the end of [`SCHEMA`] (never an old one changed: a database that has
//! the old steps gets only the new), a `CREATE TABLE … (account BYTEA NOT
//! NULL REFERENCES accounts (id) ON DELETE CASCADE, …)` in it; the
//! trait's new methods below, after [`Sharded`]'s; and the rows in
//! [`AccountRows`], [`Postgres::put_rows`] and [`Postgres::rows`], so
//! `migrate` copies them.
//!
//! [`Sharded`]: crate::sqlite::Sharded

use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use postgres::{Client, Config, Statement, Transaction};
use tokio_postgres_rustls::MakeRustlsConnect;

use carnet_sync::wire::{decode_vv, encode_vv};

use crate::rows::{AccountRows, BaseRow, DeviceRow, InviteRow, PasskeyRow, PasswordRow, RecordRow, RecoveryRow, RekeyRow};
use crate::storage::{Base, Device, Failed, Logged, PasskeySlot, PasswordSlot, Shelf, Span, Storage, covers};

/// The schema, step by step: a database has the first so many (its
/// `carnet_schema.version`), and is given the rest when opened. Only ever
/// added to at the end.
pub const SCHEMA: &[&str] = &[
    // 1: what schema 7 of the SQLite files holds.
    "
    CREATE TABLE accounts (
        id BYTEA PRIMARY KEY,
        owner BOOLEAN NOT NULL DEFAULT FALSE,
        created BIGINT NOT NULL,
        seq BIGINT NOT NULL DEFAULT 0,
        epoch BIGINT NOT NULL DEFAULT 0
    );
    CREATE TABLE invites (
        hash BYTEA PRIMARY KEY,
        owner BOOLEAN NOT NULL,
        expires BIGINT NOT NULL
    );
    CREATE TABLE devices (
        account BYTEA NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
        id BYTEA NOT NULL,
        public BYTEA NOT NULL,
        revoked BOOLEAN NOT NULL DEFAULT FALSE,
        created BIGINT NOT NULL,
        card BYTEA NOT NULL DEFAULT '',
        seen BIGINT NOT NULL DEFAULT 0,
        PRIMARY KEY (account, id)
    );
    -- Each epoch's vault key, sealed to one device (an empty id: the
    -- recovery code).
    CREATE TABLE rekeys (
        account BYTEA NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
        device BYTEA NOT NULL,
        epoch BIGINT NOT NULL,
        sealed BYTEA NOT NULL,
        PRIMARY KEY (account, device, epoch)
    );
    -- Live when shelved_by is 0, else set aside by the base with that
    -- seq, at shelved_at; bases likewise. A seq is the account's, once.
    CREATE TABLE records (
        account BYTEA NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
        seq BIGINT NOT NULL,
        room TEXT NOT NULL,
        peer BIGINT NOT NULL,
        span_start BIGINT NOT NULL,
        span_end BIGINT NOT NULL,
        bytes BYTEA NOT NULL,
        shelved_by BIGINT NOT NULL DEFAULT 0,
        shelved_at BIGINT NOT NULL DEFAULT 0,
        PRIMARY KEY (account, seq)
    );
    CREATE INDEX records_room ON records (account, room, peer);
    CREATE INDEX records_shelved ON records (shelved_at) WHERE shelved_by > 0;
    CREATE TABLE bases (
        account BYTEA NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
        seq BIGINT NOT NULL,
        room TEXT NOT NULL,
        vv BYTEA NOT NULL,
        bytes BYTEA NOT NULL,
        made_by BYTEA NOT NULL,
        at BIGINT NOT NULL,
        verified BOOLEAN NOT NULL DEFAULT FALSE,
        undoable BOOLEAN NOT NULL DEFAULT TRUE,
        shelved_by BIGINT NOT NULL DEFAULT 0,
        shelved_at BIGINT NOT NULL DEFAULT 0,
        PRIMARY KEY (account, seq)
    );
    CREATE INDEX bases_room ON bases (account, room, shelved_by);
    CREATE INDEX bases_shelved ON bases (shelved_at) WHERE shelved_by > 0;
    CREATE TABLE removed (
        account BYTEA NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
        room TEXT NOT NULL,
        at BIGINT NOT NULL,
        PRIMARY KEY (account, room)
    );
    CREATE TABLE log (
        account BYTEA NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
        n BIGSERIAL,
        at BIGINT NOT NULL,
        device BYTEA NOT NULL,
        room TEXT NOT NULL,
        what TEXT NOT NULL,
        PRIMARY KEY (account, n)
    );
    -- One recovery code per account; its key finds the account.
    CREATE TABLE recovery (
        account BYTEA PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
        public BYTEA NOT NULL UNIQUE,
        wrapped BYTEA NOT NULL,
        created BIGINT NOT NULL,
        card BYTEA NOT NULL DEFAULT ''
    );
    -- One password per account: the email it signs in as and its slot's
    -- key each find the account.
    CREATE TABLE passwords (
        account BYTEA PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
        id TEXT NOT NULL UNIQUE,
        record BYTEA NOT NULL,
        public BYTEA NOT NULL UNIQUE,
        wrapped BYTEA NOT NULL,
        card BYTEA NOT NULL
    );
    -- Each time a server opened it to serve: `migrate` copies into a
    -- database none has, never over what one wrote.
    CREATE TABLE opened (
        at BIGINT NOT NULL,
        what TEXT NOT NULL
    );
    ",
    // 2: passkeys (schema 8 of the files): each a slot, named by its
    // signing key, which finds the account.
    "
    CREATE TABLE passkeys (
        account BYTEA NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
        public BYTEA PRIMARY KEY,
        wrapped BYTEA NOT NULL,
        card BYTEA NOT NULL,
        made BIGINT NOT NULL,
        n BIGSERIAL
    );
    CREATE INDEX passkeys_account ON passkeys (account, made, n);
    ",
];

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

thread_local! {
    /// Something failed since the connection was last used: it may be the
    /// connection (cut by the database, a restart, the network), which
    /// the client may not know yet. Checked before it's used again.
    static DOUBT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// What went wrong, with the database's own words when it said any.
fn failed(e: postgres::Error) -> Failed {
    DOUBT.set(true);
    match e.as_db_error() {
        Some(db) => Failed(format!("postgres: {}: {}", db.code().code(), db.message())),
        None => Failed(format!("postgres: {e}")),
    }
}

/// `f` where the client may block: the `postgres` client runs a runtime
/// of its own, which can't start on a thread already in one (an async
/// caller's). There, a thread of its own; elsewhere (the hub's thread, a
/// command), right here.
pub fn blocking<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|s| s.spawn(f).join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
    } else {
        f()
    }
}

/// Postgres keeps signed 64-bit integers; peers, counters and seqs are
/// unsigned. Stored as their bits, read back the same (as in SQLite).
fn int(n: u64) -> i64 {
    n as i64
}

fn uint(n: i64) -> u64 {
    n as u64
}

/// The connection, and its statements prepared: each once.
struct Conn {
    client: Client,
    prepared: HashMap<&'static str, Statement>,
}

impl Conn {
    fn st(&mut self, sql: &'static str) -> Result<Statement, Failed> {
        if let Some(s) = self.prepared.get(sql) {
            return Ok(s.clone());
        }
        let s = self.client.prepare(sql).map_err(failed)?;
        self.prepared.insert(sql, s.clone());
        Ok(s)
    }
}

pub struct Postgres {
    config: Config,
    tls: MakeRustlsConnect,
    conn: RefCell<Option<Conn>>,
    /// A schema of its own, dropped with it: the tests'.
    scratch: Option<String>,
}

/// TLS as a client: rustls, the web's roots. Used when the URL's
/// `sslmode` asks (`require`), or offered (`prefer`, the default).
fn tls() -> MakeRustlsConnect {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring has TLS 1.2 and 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    MakeRustlsConnect::new(config)
}

/// A client of the database at `url`, with TLS as the store has it: for
/// a host's own tables beside the server's. Off async
/// threads, as [`blocking`] puts it.
pub fn client(url: &str) -> Result<Client, Failed> {
    let config: Config = url.parse().map_err(failed)?;
    config.connect(tls()).map_err(failed)
}

impl Postgres {
    /// The database at `url` (`postgres://user:password@host/db`), its
    /// schema made or brought up to date, and this open said to be a
    /// server's ([`Postgres::target`] for `migrate`).
    pub fn open(url: &str) -> Result<Self, Failed> {
        blocking(|| {
            let p = Self::connect(url.parse().map_err(failed)?, None)?;
            p.opened("server")?;
            Ok(p)
        })
    }

    /// The database at `url` for `migrate` to copy into: not said to be a
    /// server's.
    pub fn target(url: &str) -> Result<Self, Failed> {
        blocking(|| Self::connect(url.parse().map_err(failed)?, None))
    }

    /// A schema of its own in the database at `url`, gone when this is
    /// dropped: the tests', which run side by side.
    pub fn scratch(url: &str) -> Result<Self, Failed> {
        blocking(|| Self::scratch_here(url))
    }

    fn scratch_here(url: &str) -> Result<Self, Failed> {
        let name = format!("carnet_test_{}_{:x}", std::process::id(), {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            (now() << 16) ^ NEXT.fetch_add(1, Ordering::Relaxed)
        });
        let mut config: Config = url.parse().map_err(failed)?;
        {
            let mut c = config.connect(tls()).map_err(failed)?;
            c.batch_execute(&format!("CREATE SCHEMA {name}")).map_err(failed)?;
        }
        config.options(&format!("-c search_path={name}"));
        let p = Self::connect(config, Some(name))?;
        p.opened("server")?;
        Ok(p)
    }

    fn connect(config: Config, scratch: Option<String>) -> Result<Self, Failed> {
        let p = Postgres { config, tls: tls(), conn: RefCell::new(None), scratch };
        p.prepare()?;
        Ok(p)
    }

    /// The connection, made again if it dropped (the database restarted,
    /// the network went): what failed before is failed, what comes after
    /// goes to the new one.
    fn conn(&self) -> Result<RefMut<'_, Conn>, Failed> {
        let mut c = self.conn.borrow_mut();
        let gone = match c.as_mut() {
            None => true,
            Some(c) if c.client.is_closed() => true,
            Some(c) if DOUBT.replace(false) => c.client.is_valid(std::time::Duration::from_secs(5)).is_err(),
            Some(_) => false,
        };
        if gone {
            let client = self.config.connect(self.tls.clone()).map_err(failed)?;
            *c = Some(Conn { client, prepared: HashMap::new() });
        }
        Ok(RefMut::map(c, |c| c.as_mut().expect("just made")))
    }

    /// The schema made, or the steps it lacks taken, under a lock: two
    /// servers starting at once take turns. One from a newer server is
    /// refused, not guessed at.
    fn prepare(&self) -> Result<(), Failed> {
        let mut c = self.conn()?;
        let mut tx = c.client.transaction().map_err(failed)?;
        tx.batch_execute(
            "SELECT pg_advisory_xact_lock(4381001);
             CREATE TABLE IF NOT EXISTS carnet_schema (one INT PRIMARY KEY CHECK (one = 1), version INT NOT NULL);
             INSERT INTO carnet_schema (one, version) VALUES (1, 0) ON CONFLICT (one) DO NOTHING;",
        )
        .map_err(failed)?;
        let v: i32 = tx.query_one("SELECT version FROM carnet_schema WHERE one = 1", &[]).map_err(failed)?.get(0);
        let known = SCHEMA.len() as i32;
        if v > known {
            return Err(Failed(format!("a database from a newer carnet-server (schema {v}, this reads {known})")));
        }
        for step in &SCHEMA[v as usize..] {
            tx.batch_execute(step).map_err(failed)?;
        }
        tx.execute("UPDATE carnet_schema SET version = $1 WHERE one = 1", &[&known]).map_err(failed)?;
        tx.commit().map_err(failed)
    }

    fn opened(&self, what: &str) -> Result<(), Failed> {
        let mut c = self.conn()?;
        c.client.execute("INSERT INTO opened (at, what) VALUES ($1, $2)", &[&int(now()), &what]).map(|_| ()).map_err(failed)
    }

    /// Whether a server has opened it to serve: what it holds is newer
    /// than any copy.
    pub fn served(&self) -> Result<bool, Failed> {
        let mut c = self.conn()?;
        c.client.query_one("SELECT EXISTS (SELECT 1 FROM opened WHERE what = 'server')", &[]).map(|r| r.get(0)).map_err(failed)
    }

    /// `f` on the connection; on any failure, `or`.
    fn read<T>(&self, or: T, f: impl FnOnce(&mut Conn) -> Result<T, Failed>) -> T {
        match self.conn() {
            Ok(mut c) => f(&mut c).unwrap_or(or),
            Err(_) => or,
        }
    }

    /// `f` in a transaction, committed if it says so.
    fn write<T>(&self, statements: &[&'static str], f: impl FnOnce(&mut Transaction<'_>, &[Statement]) -> Result<T, Failed>) -> Result<T, Failed> {
        let mut c = self.conn()?;
        let st = statements.iter().map(|s| c.st(s)).collect::<Result<Vec<_>, _>>()?;
        let mut tx = c.client.transaction().map_err(failed)?;
        let out = f(&mut tx, &st)?;
        tx.commit().map_err(failed)?;
        Ok(out)
    }

    /// One statement, its rows changed.
    fn exec(&self, sql: &'static str, params: &[&(dyn postgres::types::ToSql + Sync)]) -> Result<u64, Failed> {
        let mut c = self.conn()?;
        let st = c.st(sql)?;
        c.client.execute(&st, params).map_err(failed)
    }

    /// Every account's id, the oldest first.
    pub fn account_ids(&self) -> Result<Vec<Vec<u8>>, Failed> {
        let mut c = self.conn()?;
        Ok(c.client.query("SELECT id FROM accounts ORDER BY created, id", &[]).map_err(failed)?.iter().map(|r| r.get(0)).collect())
    }

    /// Everything the account keeps, as rows: `None` if it isn't here.
    pub fn rows(&self, account: &[u8]) -> Result<Option<AccountRows>, Failed> {
        let mut c = self.conn()?;
        let mut tx = c.client.build_transaction().read_only(true).isolation_level(postgres::IsolationLevel::RepeatableRead).start().map_err(failed)?;
        let Some(a) = tx.query_opt("SELECT owner, created, seq, epoch FROM accounts WHERE id = $1", &[&account]).map_err(failed)? else {
            return Ok(None);
        };
        let devices = tx
            .query("SELECT id, public, revoked, created, card, seen FROM devices WHERE account = $1 ORDER BY id", &[&account])
            .map_err(failed)?
            .iter()
            .map(|r| DeviceRow { id: r.get(0), public: r.get(1), revoked: r.get(2), created: uint(r.get(3)), card: r.get(4), seen: uint(r.get(5)) })
            .collect();
        let rekeys = tx
            .query("SELECT device, epoch, sealed FROM rekeys WHERE account = $1 ORDER BY device, epoch", &[&account])
            .map_err(failed)?
            .iter()
            .map(|r| RekeyRow { device: r.get(0), epoch: r.get::<_, i64>(1) as u32, sealed: r.get(2) })
            .collect();
        let records = tx
            .query(
                "SELECT seq, room, peer, span_start, span_end, bytes, shelved_by, shelved_at FROM records WHERE account = $1 ORDER BY seq",
                &[&account],
            )
            .map_err(failed)?
            .iter()
            .map(|r| RecordRow {
                seq: uint(r.get(0)),
                room: r.get(1),
                peer: uint(r.get(2)),
                start: uint(r.get(3)),
                end: uint(r.get(4)),
                bytes: r.get(5),
                shelved_by: uint(r.get(6)),
                shelved_at: uint(r.get(7)),
            })
            .collect();
        let bases = tx
            .query(
                "SELECT seq, room, vv, bytes, made_by, at, verified, undoable, shelved_by, shelved_at FROM bases WHERE account = $1 ORDER BY seq",
                &[&account],
            )
            .map_err(failed)?
            .iter()
            .map(|r| BaseRow {
                seq: uint(r.get(0)),
                room: r.get(1),
                vv: r.get(2),
                bytes: r.get(3),
                by: r.get(4),
                at: uint(r.get(5)),
                verified: r.get(6),
                undoable: r.get(7),
                shelved_by: uint(r.get(8)),
                shelved_at: uint(r.get(9)),
            })
            .collect();
        let removed = tx
            .query("SELECT room, at FROM removed WHERE account = $1 ORDER BY room", &[&account])
            .map_err(failed)?
            .iter()
            .map(|r| (r.get(0), uint(r.get(1))))
            .collect();
        let log = tx
            .query("SELECT at, device, room, what FROM log WHERE account = $1 ORDER BY n", &[&account])
            .map_err(failed)?
            .iter()
            .map(|r| Logged { at: uint(r.get(0)), device: r.get(1), room: r.get(2), what: r.get(3) })
            .collect();
        let recovery = tx
            .query_opt("SELECT public, wrapped, created, card FROM recovery WHERE account = $1", &[&account])
            .map_err(failed)?
            .map(|r| RecoveryRow { public: r.get(0), wrapped: r.get(1), created: uint(r.get(2)), card: r.get(3) });
        let password = tx
            .query_opt("SELECT id, record, public, wrapped, card FROM passwords WHERE account = $1", &[&account])
            .map_err(failed)?
            .map(|r| PasswordRow { id: r.get(0), record: r.get(1), public: r.get(2), wrapped: r.get(3), card: r.get(4) });
        let passkeys = tx
            .query("SELECT public, wrapped, card, made FROM passkeys WHERE account = $1 ORDER BY made, n", &[&account])
            .map_err(failed)?
            .iter()
            .map(|r| PasskeyRow { public: r.get(0), wrapped: r.get(1), card: r.get(2), made: uint(r.get(3)) })
            .collect();
        Ok(Some(AccountRows {
            id: account.to_vec(),
            owner: a.get(0),
            created: uint(a.get(1)),
            seq: uint(a.get(2)),
            epoch: a.get::<_, i64>(3) as u32,
            devices,
            rekeys,
            records,
            bases,
            removed,
            log,
            recovery,
            password,
            passkeys,
        }))
    }

    /// The account as `rows` has it, in place of whatever was here of it:
    /// all of it, or (failing) nothing changed.
    pub fn put_rows(&self, rows: &AccountRows) -> Result<(), Failed> {
        let mut c = self.conn()?;
        let mut tx = c.client.transaction().map_err(failed)?;
        let a = &rows.id;
        tx.execute("DELETE FROM accounts WHERE id = $1", &[a]).map_err(failed)?;
        tx.execute(
            "INSERT INTO accounts (id, owner, created, seq, epoch) VALUES ($1, $2, $3, $4, $5)",
            &[a, &rows.owner, &int(rows.created), &int(rows.seq), &i64::from(rows.epoch)],
        )
        .map_err(failed)?;
        let st = tx.prepare("INSERT INTO devices (account, id, public, revoked, created, card, seen) VALUES ($1, $2, $3, $4, $5, $6, $7)").map_err(failed)?;
        for d in &rows.devices {
            tx.execute(&st, &[a, &d.id, &d.public, &d.revoked, &int(d.created), &d.card, &int(d.seen)]).map_err(failed)?;
        }
        let st = tx.prepare("INSERT INTO rekeys (account, device, epoch, sealed) VALUES ($1, $2, $3, $4)").map_err(failed)?;
        for k in &rows.rekeys {
            tx.execute(&st, &[a, &k.device, &i64::from(k.epoch), &k.sealed]).map_err(failed)?;
        }
        let st = tx
            .prepare("INSERT INTO records (account, seq, room, peer, span_start, span_end, bytes, shelved_by, shelved_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
            .map_err(failed)?;
        for r in &rows.records {
            tx.execute(&st, &[a, &int(r.seq), &r.room, &int(r.peer), &int(r.start), &int(r.end), &r.bytes, &int(r.shelved_by), &int(r.shelved_at)]).map_err(failed)?;
        }
        let st = tx
            .prepare(
                "INSERT INTO bases (account, seq, room, vv, bytes, made_by, at, verified, undoable, shelved_by, shelved_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            )
            .map_err(failed)?;
        for b in &rows.bases {
            tx.execute(&st, &[a, &int(b.seq), &b.room, &b.vv, &b.bytes, &b.by, &int(b.at), &b.verified, &b.undoable, &int(b.shelved_by), &int(b.shelved_at)])
                .map_err(failed)?;
        }
        let st = tx.prepare("INSERT INTO removed (account, room, at) VALUES ($1, $2, $3)").map_err(failed)?;
        for (room, at) in &rows.removed {
            tx.execute(&st, &[a, room, &int(*at)]).map_err(failed)?;
        }
        let st = tx.prepare("INSERT INTO log (account, at, device, room, what) VALUES ($1, $2, $3, $4, $5)").map_err(failed)?;
        for l in &rows.log {
            tx.execute(&st, &[a, &int(l.at), &l.device, &l.room, &l.what]).map_err(failed)?;
        }
        if let Some(r) = &rows.recovery {
            // A key another account had (only ever a copy's leftover): this
            // one's now, as in the files' directory.
            tx.execute("DELETE FROM recovery WHERE public = $1", &[&r.public]).map_err(failed)?;
            tx.execute("INSERT INTO recovery (account, public, wrapped, created, card) VALUES ($1, $2, $3, $4, $5)", &[a, &r.public, &r.wrapped, &int(r.created), &r.card])
                .map_err(failed)?;
        }
        if let Some(p) = &rows.password {
            tx.execute("INSERT INTO passwords (account, id, record, public, wrapped, card) VALUES ($1, $2, $3, $4, $5, $6)", &[a, &p.id, &p.record, &p.public, &p.wrapped, &p.card])
                .map_err(failed)?;
        }
        for k in &rows.passkeys {
            tx.execute("DELETE FROM passkeys WHERE public = $1", &[&k.public]).map_err(failed)?;
            tx.execute("INSERT INTO passkeys (account, public, wrapped, card, made) VALUES ($1, $2, $3, $4, $5)", &[a, &k.public, &k.wrapped, &k.card, &int(k.made)])
                .map_err(failed)?;
        }
        tx.commit().map_err(failed)
    }

    /// The account gone, if it's here.
    pub fn drop_rows(&self, account: &[u8]) -> Result<(), Failed> {
        self.exec("DELETE FROM accounts WHERE id = $1", &[&account]).map(|_| ())
    }

    /// The invites as `all` has them, and no others.
    pub fn put_invites(&self, all: &[InviteRow]) -> Result<(), Failed> {
        let mut c = self.conn()?;
        let mut tx = c.client.transaction().map_err(failed)?;
        tx.execute("DELETE FROM invites", &[]).map_err(failed)?;
        for (hash, owner, expires) in all {
            tx.execute("INSERT INTO invites (hash, owner, expires) VALUES ($1, $2, $3)", &[&hash.as_slice(), owner, &int(*expires)]).map_err(failed)?;
        }
        tx.commit().map_err(failed)
    }

    fn account_known(tx: &mut Transaction<'_>, st: &Statement, account: &[u8]) -> Result<(), Failed> {
        match tx.query_opt(st, &[&account]).map_err(failed)? {
            Some(_) => Ok(()),
            None => Err(Failed("no such account".into())),
        }
    }
}

impl Drop for Postgres {
    fn drop(&mut self) {
        let conn = self.conn.get_mut().take();
        let scratch = self.scratch.take();
        let (config, tls) = (&self.config, &self.tls);
        blocking(move || {
            let Some(name) = scratch else {
                drop(conn);
                return;
            };
            let client = match conn {
                Some(c) if !c.client.is_closed() => Some(c.client),
                _ => config.connect(tls.clone()).ok(),
            };
            if let Some(mut client) = client {
                let _ = client.batch_execute(&format!("DROP SCHEMA {name} CASCADE"));
            }
        });
    }
}

/// The account's `seq` one more, in `tx`: the new one.
const NEXT_SEQ: &str = "UPDATE accounts SET seq = seq + 1 WHERE id = $1 RETURNING seq";
const ACCOUNT: &str = "SELECT 1 FROM accounts WHERE id = $1 FOR UPDATE";

fn next_seq(tx: &mut Transaction<'_>, st: &Statement, account: &[u8]) -> Result<i64, Failed> {
    tx.query_opt(st, &[&account]).map_err(failed)?.map(|r| r.get(0)).ok_or_else(|| Failed("no such account".into()))
}

impl Storage for Postgres {
    fn device(&self, account: &[u8], device: &[u8]) -> Option<Device> {
        self.read(None, |c| {
            let st = c.st("SELECT public, revoked, card, seen FROM devices WHERE account = $1 AND id = $2")?;
            let row = c.client.query_opt(&st, &[&account, &device]).map_err(failed)?;
            Ok(row.and_then(|r| {
                let public: Vec<u8> = r.get(0);
                Some(Device { public: public.try_into().ok()?, revoked: r.get(1), card: r.get(2), seen: uint(r.get(3)) })
            }))
        })
    }

    fn devices(&self, account: &[u8]) -> Vec<(Vec<u8>, Device)> {
        self.read(Vec::new(), |c| {
            let st = c.st("SELECT id, public, revoked, card, seen FROM devices WHERE account = $1 ORDER BY id")?;
            let rows = c.client.query(&st, &[&account]).map_err(failed)?;
            Ok(rows
                .iter()
                .filter_map(|r| {
                    let public: Vec<u8> = r.get(1);
                    Some((r.get(0), Device { public: public.try_into().ok()?, revoked: r.get(2), card: r.get(3), seen: uint(r.get(4)) }))
                })
                .collect())
        })
    }

    fn set_card(&mut self, account: &[u8], device: &[u8], card: Vec<u8>) -> Result<(), Failed> {
        self.exec("UPDATE devices SET card = $3 WHERE account = $1 AND id = $2", &[&account, &device, &card]).map(|_| ())
    }

    fn set_seen(&mut self, account: &[u8], device: &[u8], now: u64) -> Result<(), Failed> {
        self.exec("UPDATE devices SET seen = $3 WHERE account = $1 AND id = $2", &[&account, &device, &int(now)]).map(|_| ())
    }

    fn epoch(&self, account: &[u8]) -> u32 {
        self.read(0, |c| {
            let st = c.st("SELECT epoch FROM accounts WHERE id = $1")?;
            Ok(c.client.query_opt(&st, &[&account]).map_err(failed)?.map_or(0, |r| r.get::<_, i64>(0) as u32))
        })
    }

    fn rotate(
        &mut self,
        account: &[u8],
        revoked: &[u8],
        epoch: u32,
        keys: Vec<(Vec<u8>, Vec<u8>)>,
        cards: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<(), Failed> {
        // All of it, or none: a device half removed is not one.
        self.write(
            &[
                ACCOUNT,
                "UPDATE devices SET revoked = TRUE WHERE account = $1 AND id = $2",
                "UPDATE accounts SET epoch = $2 WHERE id = $1",
                "INSERT INTO rekeys (account, device, epoch, sealed) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (account, device, epoch) DO UPDATE SET sealed = excluded.sealed",
                "UPDATE recovery SET card = $2 WHERE account = $1",
                "UPDATE passwords SET card = $2 WHERE account = $1",
                "UPDATE devices SET card = $3 WHERE account = $1 AND id = $2",
                "UPDATE passkeys SET card = $3 WHERE account = $1 AND public = $2",
            ],
            |tx, st| {
                Self::account_known(tx, &st[0], account)?;
                tx.execute(&st[1], &[&account, &revoked]).map_err(failed)?;
                tx.execute(&st[2], &[&account, &i64::from(epoch)]).map_err(failed)?;
                for (id, sealed) in &keys {
                    tx.execute(&st[3], &[&account, id, &i64::from(epoch), sealed]).map_err(failed)?;
                }
                for (id, card) in &cards {
                    if id.is_empty() {
                        tx.execute(&st[4], &[&account, card]).map_err(failed)?;
                    } else if id == carnet_sync::password::SLOT {
                        tx.execute(&st[5], &[&account, card]).map_err(failed)?;
                    } else if let Some(public) = carnet_sync::passkey::of_slot(id) {
                        tx.execute(&st[7], &[&account, &public.as_slice(), card]).map_err(failed)?;
                    } else {
                        tx.execute(&st[6], &[&account, id, card]).map_err(failed)?;
                    }
                }
                Ok(())
            },
        )
    }

    fn rekeys(&self, account: &[u8], device: &[u8]) -> Vec<Vec<u8>> {
        self.read(Vec::new(), |c| {
            let st = c.st("SELECT sealed FROM rekeys WHERE account = $1 AND device = $2 ORDER BY epoch")?;
            Ok(c.client.query(&st, &[&account, &device]).map_err(failed)?.iter().map(|r| r.get(0)).collect())
        })
    }

    fn add_device(&mut self, account: &[u8], device: &[u8], public: [u8; 32]) -> Result<(), Failed> {
        self.write(
            &[
                ACCOUNT,
                "INSERT INTO devices (account, id, public, created) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (account, id) DO UPDATE SET public = excluded.public, revoked = FALSE",
            ],
            |tx, st| {
                Self::account_known(tx, &st[0], account)?;
                tx.execute(&st[1], &[&account, &device, &public.as_slice(), &int(now())]).map(|_| ()).map_err(failed)
            },
        )
    }

    fn revoke_device(&mut self, account: &[u8], device: &[u8]) -> Result<(), Failed> {
        self.exec("UPDATE devices SET revoked = TRUE WHERE account = $1 AND id = $2", &[&account, &device]).map(|_| ())
    }

    fn put(&mut self, account: &[u8], room: &str, peer: u64, start: u64, end: u64, record: Vec<u8>) -> Result<Option<u64>, Failed> {
        if self.base(account, room).is_some_and(|b| covers(&b.vv, &[(peer, end)])) {
            return Ok(None);
        }
        self.write(
            &[
                "SELECT EXISTS (SELECT 1 FROM records WHERE account = $1 AND room = $2 AND peer = $3
                     AND span_start <= $4 AND $5 <= span_end AND shelved_by = 0)",
                "DELETE FROM records WHERE account = $1 AND room = $2 AND peer = $3
                     AND $4 <= span_start AND span_end <= $5 AND shelved_by = 0",
                NEXT_SEQ,
                "INSERT INTO records (account, seq, room, peer, span_start, span_end, bytes) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            ],
            |tx, st| {
                let (peer, start, end) = (int(peer), int(start), int(end));
                let covered: bool = tx.query_one(&st[0], &[&account, &room, &peer, &start, &end]).map_err(failed)?.get(0);
                if covered {
                    return Ok(None);
                }
                tx.execute(&st[1], &[&account, &room, &peer, &start, &end]).map_err(failed)?;
                let seq = next_seq(tx, &st[2], account)?;
                tx.execute(&st[3], &[&account, &seq, &room, &peer, &start, &end, &record]).map_err(failed)?;
                Ok(Some(uint(seq)))
            },
        )
    }

    fn spans(&self, account: &[u8], room: &str) -> Vec<Span> {
        self.read(Vec::new(), |c| {
            let st = c.st("SELECT peer, span_start, span_end, seq, bytes FROM records WHERE account = $1 AND room = $2 AND shelved_by = 0 ORDER BY seq")?;
            Ok(c.client
                .query(&st, &[&account, &room])
                .map_err(failed)?
                .iter()
                .map(|r| Span { peer: uint(r.get(0)), start: uint(r.get(1)), end: uint(r.get(2)), seq: uint(r.get(3)), record: r.get(4) })
                .collect())
        })
    }

    fn changed_since(&self, account: &[u8], seq: u64) -> Vec<(String, u64)> {
        self.read(Vec::new(), |c| {
            let st = c.st(
                "SELECT room, MAX(seq) AS newest FROM (
                     SELECT room, seq FROM records WHERE account = $1 AND shelved_by = 0
                     UNION ALL SELECT room, seq FROM bases WHERE account = $1 AND shelved_by = 0) AS live
                 GROUP BY room HAVING MAX(seq) > $2 ORDER BY newest, room",
            )?;
            Ok(c.client.query(&st, &[&account, &int(seq)]).map_err(failed)?.iter().map(|r| (r.get(0), uint(r.get(1)))).collect())
        })
    }

    fn base(&self, account: &[u8], room: &str) -> Option<Base> {
        self.read(None, |c| {
            let st = c.st("SELECT seq, vv, bytes, made_by, at, verified, undoable FROM bases WHERE account = $1 AND room = $2 AND shelved_by = 0")?;
            Ok(c.client.query_opt(&st, &[&account, &room]).map_err(failed)?.map(|r| Base {
                seq: uint(r.get(0)),
                vv: decode_vv(&r.get::<_, Vec<u8>>(1)).unwrap_or_default(),
                record: r.get(2),
                by: r.get(3),
                at: uint(r.get(4)),
                verified: r.get(5),
                undoable: r.get(6),
            }))
        })
    }

    fn set_base(&mut self, account: &[u8], room: &str, vv: Vec<(u64, u64)>, record: Vec<u8>, by: &[u8], now: u64) -> Result<u64, Failed> {
        let live = self.spans(account, room);
        self.write(
            &[
                NEXT_SEQ,
                "UPDATE bases SET shelved_by = $3, shelved_at = $4 WHERE account = $1 AND room = $2 AND shelved_by = 0",
                "UPDATE records SET shelved_by = $3, shelved_at = $4 WHERE account = $1 AND seq = $2",
                "INSERT INTO bases (account, seq, room, vv, bytes, made_by, at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            ],
            |tx, st| {
                let seq = next_seq(tx, &st[0], account)?;
                tx.execute(&st[1], &[&account, &room, &seq, &int(now)]).map_err(failed)?;
                for s in live.iter().filter(|s| covers(&vv, &[(s.peer, s.end)])) {
                    tx.execute(&st[2], &[&account, &int(s.seq), &seq, &int(now)]).map_err(failed)?;
                }
                tx.execute(&st[3], &[&account, &seq, &room, &encode_vv(&vv), &record, &by, &int(now)]).map_err(failed)?;
                Ok(uint(seq))
            },
        )
    }

    fn verify_base(&mut self, account: &[u8], room: &str, seq: u64) -> Result<(), Failed> {
        self.exec("UPDATE bases SET verified = TRUE WHERE account = $1 AND seq = $2 AND room = $3 AND shelved_by = 0", &[&account, &int(seq), &room]).map(|_| ())
    }

    fn roll_back(&mut self, account: &[u8], room: &str) -> Result<bool, Failed> {
        let Some(b) = self.base(account, room).filter(|b| b.undoable) else { return Ok(false) };
        self.write(
            &[
                "DELETE FROM bases WHERE account = $1 AND seq = $2",
                "UPDATE bases SET shelved_by = 0, shelved_at = 0 WHERE account = $1 AND shelved_by = $2 AND room = $3",
                "UPDATE records SET shelved_by = 0, shelved_at = 0 WHERE account = $1 AND shelved_by = $2 AND room = $3",
            ],
            |tx, st| {
                let seq = int(b.seq);
                tx.execute(&st[0], &[&account, &seq]).map_err(failed)?;
                tx.execute(&st[1], &[&account, &seq, &room]).map_err(failed)?;
                tx.execute(&st[2], &[&account, &seq, &room]).map_err(failed)?;
                Ok(true)
            },
        )
    }

    fn shelf(&self, account: &[u8], room: &str) -> Shelf {
        self.read(Shelf::default(), |c| {
            let st = c.st(
                "SELECT COUNT(*), COALESCE(SUM(LENGTH(bytes)), 0)::BIGINT, COALESCE(MIN(shelved_at), 0) FROM (
                     SELECT bytes, shelved_at FROM records WHERE account = $1 AND room = $2 AND shelved_by > 0
                     UNION ALL SELECT bytes, shelved_at FROM bases WHERE account = $1 AND room = $2 AND shelved_by > 0) AS aside",
            )?;
            let r = c.client.query_one(&st, &[&account, &room]).map_err(failed)?;
            Ok(Shelf { count: uint(r.get(0)), bytes: uint(r.get(1)), since: uint(r.get(2)) })
        })
    }

    fn rooms(&self, account: &[u8]) -> Vec<String> {
        self.read(Vec::new(), |c| {
            let st = c.st(
                "SELECT room FROM records WHERE account = $1 AND shelved_by = 0
                 UNION SELECT room FROM bases WHERE account = $1 AND shelved_by = 0 ORDER BY room",
            )?;
            Ok(c.client.query(&st, &[&account]).map_err(failed)?.iter().map(|r| r.get(0)).collect())
        })
    }

    fn remove_room(&mut self, account: &[u8], room: &str, now: u64) -> Result<(), Failed> {
        self.write(
            &[
                ACCOUNT,
                "DELETE FROM records WHERE account = $1 AND room = $2",
                "DELETE FROM bases WHERE account = $1 AND room = $2",
                "INSERT INTO removed (account, room, at) VALUES ($1, $2, $3) ON CONFLICT (account, room) DO NOTHING",
            ],
            |tx, st| {
                Self::account_known(tx, &st[0], account)?;
                tx.execute(&st[1], &[&account, &room]).map_err(failed)?;
                tx.execute(&st[2], &[&account, &room]).map_err(failed)?;
                tx.execute(&st[3], &[&account, &room, &int(now)]).map(|_| ()).map_err(failed)
            },
        )
    }

    fn usage(&self, account: &[u8]) -> u64 {
        self.read(0, |c| {
            let st = c.st(
                "SELECT ((SELECT COALESCE(SUM(LENGTH(bytes)), 0) FROM records WHERE account = $1)
                      + (SELECT COALESCE(SUM(LENGTH(bytes)), 0) FROM bases WHERE account = $1))::BIGINT",
            )?;
            Ok(uint(c.client.query_one(&st, &[&account]).map_err(failed)?.get(0)))
        })
    }

    fn is_removed(&self, account: &[u8], room: &str) -> bool {
        self.read(false, |c| {
            let st = c.st("SELECT EXISTS (SELECT 1 FROM removed WHERE account = $1 AND room = $2)")?;
            Ok(c.client.query_one(&st, &[&account, &room]).map_err(failed)?.get(0))
        })
    }

    fn purge(&mut self, before: u64) -> Result<(), Failed> {
        self.write(
            &[
                // A base whose set-aside goes can't be rolled back after.
                "UPDATE bases SET undoable = FALSE WHERE (account, seq) IN (
                     SELECT account, shelved_by FROM records WHERE shelved_by > 0 AND shelved_at < $1
                     UNION SELECT account, shelved_by FROM bases WHERE shelved_by > 0 AND shelved_at < $1)",
                "DELETE FROM records WHERE shelved_by > 0 AND shelved_at < $1",
                "DELETE FROM bases WHERE shelved_by > 0 AND shelved_at < $1",
            ],
            |tx, st| {
                for s in st {
                    tx.execute(s, &[&int(before)]).map_err(failed)?;
                }
                Ok(())
            },
        )
    }

    fn log(&mut self, account: &[u8], entry: Logged) -> Result<(), Failed> {
        self.exec("INSERT INTO log (account, at, device, room, what) VALUES ($1, $2, $3, $4, $5)", &[&account, &int(entry.at), &entry.device, &entry.room, &entry.what])
            .map(|_| ())
    }

    fn logged(&self, account: &[u8]) -> Vec<Logged> {
        self.read(Vec::new(), |c| {
            let st = c.st("SELECT at, device, room, what FROM log WHERE account = $1 ORDER BY n DESC LIMIT 50")?;
            Ok(c.client
                .query(&st, &[&account])
                .map_err(failed)?
                .iter()
                .map(|r| Logged { at: uint(r.get(0)), device: r.get(1), room: r.get(2), what: r.get(3) })
                .collect())
        })
    }

    fn seq(&self, account: &[u8]) -> u64 {
        self.read(0, |c| {
            let st = c.st("SELECT seq FROM accounts WHERE id = $1")?;
            Ok(c.client.query_opt(&st, &[&account]).map_err(failed)?.map_or(0, |r| uint(r.get(0))))
        })
    }

    fn accounts(&self) -> usize {
        self.read(0, |c| {
            let st = c.st("SELECT COUNT(*) FROM accounts")?;
            Ok(c.client.query_one(&st, &[]).map_err(failed)?.get::<_, i64>(0) as usize)
        })
    }

    fn add_account(&mut self, account: &[u8], owner: bool) -> Result<(), Failed> {
        if account.is_empty() || account.len() > 64 {
            return Err(Failed("an account id is 1 to 64 bytes".into()));
        }
        self.exec("INSERT INTO accounts (id, owner, created) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING", &[&account, &owner, &int(now())]).map(|_| ())
    }

    fn add_invite(&mut self, hash: [u8; 32], owner: bool, expires: u64) -> Result<(), Failed> {
        self.exec(
            "INSERT INTO invites (hash, owner, expires) VALUES ($1, $2, $3) ON CONFLICT (hash) DO UPDATE SET owner = excluded.owner, expires = excluded.expires",
            &[&hash.as_slice(), &owner, &int(expires)],
        )
        .map(|_| ())
    }

    fn take_invite(&mut self, hash: [u8; 32], now: u64) -> Result<Option<bool>, Failed> {
        let mut c = self.conn()?;
        let st = c.st("DELETE FROM invites WHERE hash = $1 RETURNING owner, expires")?;
        let row = c.client.query_opt(&st, &[&hash.as_slice()]).map_err(failed)?;
        Ok(row.map(|r| (r.get::<_, bool>(0), uint(r.get(1)))).filter(|&(_, expires)| now < expires).map(|(owner, _)| owner))
    }

    fn invites(&self) -> Vec<([u8; 32], bool, u64)> {
        self.read(Vec::new(), |c| {
            let st = c.st("SELECT hash, owner, expires FROM invites ORDER BY expires, hash")?;
            Ok(c.client
                .query(&st, &[])
                .map_err(failed)?
                .iter()
                .filter_map(|r| Some((<[u8; 32]>::try_from(r.get::<_, Vec<u8>>(0)).ok()?, r.get(1), uint(r.get(2)))))
                .collect())
        })
    }

    fn drop_invite(&mut self, hash: [u8; 32]) -> Result<bool, Failed> {
        self.exec("DELETE FROM invites WHERE hash = $1", &[&hash.as_slice()]).map(|n| n > 0)
    }

    fn is_owner(&self, account: &[u8]) -> bool {
        self.read(false, |c| {
            let st = c.st("SELECT owner FROM accounts WHERE id = $1")?;
            Ok(c.client.query_opt(&st, &[&account]).map_err(failed)?.is_some_and(|r| r.get(0)))
        })
    }

    fn account_list(&self) -> Vec<(Vec<u8>, bool, u64)> {
        self.read(Vec::new(), |c| {
            let st = c.st("SELECT id, owner, created FROM accounts ORDER BY created, id")?;
            Ok(c.client.query(&st, &[]).map_err(failed)?.iter().map(|r| (r.get(0), r.get(1), uint(r.get(2)))).collect())
        })
    }

    fn remove_account(&mut self, account: &[u8]) -> Result<(), Failed> {
        // Its devices, records, codes and the rest go with it.
        self.exec("DELETE FROM accounts WHERE id = $1", &[&account]).map(|_| ())
    }

    fn set_recovery(&mut self, account: &[u8], public: [u8; 32], wrapped: Vec<u8>, card: Vec<u8>) -> Result<(), Failed> {
        self.write(
            &[
                ACCOUNT,
                "DELETE FROM recovery WHERE public = $2 AND account <> $1",
                "INSERT INTO recovery (account, public, wrapped, created, card) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (account) DO UPDATE SET public = excluded.public, wrapped = excluded.wrapped,
                     card = excluded.card, created = excluded.created",
                // The new code's ring has every epoch: what was sealed to the
                // last code goes.
                "DELETE FROM rekeys WHERE account = $1 AND device = ''",
            ],
            |tx, st| {
                Self::account_known(tx, &st[0], account)?;
                tx.execute(&st[1], &[&account, &public.as_slice()]).map_err(failed)?;
                tx.execute(&st[2], &[&account, &public.as_slice(), &wrapped, &int(now()), &card]).map_err(failed)?;
                tx.execute(&st[3], &[&account]).map(|_| ()).map_err(failed)
            },
        )
    }

    fn recovery(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        self.read(None, |c| {
            let st = c.st("SELECT account, wrapped FROM recovery WHERE public = $1")?;
            Ok(c.client.query_opt(&st, &[&public.as_slice()]).map_err(failed)?.map(|r| (r.get(0), r.get(1))))
        })
    }

    fn set_password(&mut self, account: &[u8], slot: PasswordSlot) -> Result<(), Failed> {
        self.write(
            &[
                ACCOUNT,
                "INSERT INTO passwords (account, id, record, public, wrapped, card) VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (account) DO UPDATE SET id = excluded.id, record = excluded.record, public = excluded.public,
                     wrapped = excluded.wrapped, card = excluded.card",
                // The new slot's ring has every epoch: what was sealed to the
                // last one goes.
                "DELETE FROM rekeys WHERE account = $1 AND device = $2",
            ],
            |tx, st| {
                Self::account_known(tx, &st[0], account)?;
                tx.execute(&st[1], &[&account, &slot.id, &slot.record, &slot.public.as_slice(), &slot.wrapped, &slot.card]).map_err(failed)?;
                tx.execute(&st[2], &[&account, &carnet_sync::password::SLOT]).map(|_| ()).map_err(failed)
            },
        )
    }

    fn drop_password(&mut self, account: &[u8]) -> Result<(), Failed> {
        self.write(&["DELETE FROM passwords WHERE account = $1", "DELETE FROM rekeys WHERE account = $1 AND device = $2"], |tx, st| {
            tx.execute(&st[0], &[&account]).map_err(failed)?;
            tx.execute(&st[1], &[&account, &carnet_sync::password::SLOT]).map(|_| ()).map_err(failed)
        })
    }

    fn password_of(&self, id: &str) -> Option<(Vec<u8>, Vec<u8>)> {
        self.read(None, |c| {
            let st = c.st("SELECT account, record FROM passwords WHERE id = $1")?;
            Ok(c.client.query_opt(&st, &[&id]).map_err(failed)?.map(|r| (r.get(0), r.get(1))))
        })
    }

    fn password_slot(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        self.read(None, |c| {
            let st = c.st("SELECT account, wrapped FROM passwords WHERE public = $1")?;
            Ok(c.client.query_opt(&st, &[&public.as_slice()]).map_err(failed)?.map(|r| (r.get(0), r.get(1))))
        })
    }

    fn password_card(&self, account: &[u8]) -> Vec<u8> {
        self.read(Vec::new(), |c| {
            let st = c.st("SELECT card FROM passwords WHERE account = $1")?;
            Ok(c.client.query_opt(&st, &[&account]).map_err(failed)?.map(|r| r.get(0)).unwrap_or_default())
        })
    }

    fn recovery_card(&self, account: &[u8]) -> Vec<u8> {
        self.read(Vec::new(), |c| {
            let st = c.st("SELECT card FROM recovery WHERE account = $1")?;
            Ok(c.client.query_opt(&st, &[&account]).map_err(failed)?.map(|r| r.get(0)).unwrap_or_default())
        })
    }

    fn add_passkey(&mut self, account: &[u8], slot: PasskeySlot) -> Result<(), Failed> {
        self.write(
            &[
                ACCOUNT,
                // A key another account had (only ever a copy's leftover):
                // this one's now, as in the files' directory.
                "DELETE FROM passkeys WHERE public = $2 AND account <> $1",
                // Added again: when it first was stays.
                "INSERT INTO passkeys (account, public, wrapped, card, made) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (public) DO UPDATE SET wrapped = excluded.wrapped, card = excluded.card",
                // The slot's ring has every epoch: what was sealed to it
                // before goes.
                "DELETE FROM rekeys WHERE account = $1 AND device = $2",
            ],
            |tx, st| {
                Self::account_known(tx, &st[0], account)?;
                tx.execute(&st[1], &[&account, &slot.public.as_slice()]).map_err(failed)?;
                tx.execute(&st[2], &[&account, &slot.public.as_slice(), &slot.wrapped, &slot.card, &int(slot.made)]).map_err(failed)?;
                tx.execute(&st[3], &[&account, &carnet_sync::passkey::slot(&slot.public)]).map(|_| ()).map_err(failed)
            },
        )
    }

    fn drop_passkey(&mut self, account: &[u8], public: &[u8; 32]) -> Result<bool, Failed> {
        self.write(&["DELETE FROM passkeys WHERE account = $1 AND public = $2", "DELETE FROM rekeys WHERE account = $1 AND device = $2"], |tx, st| {
            let n = tx.execute(&st[0], &[&account, &public.as_slice()]).map_err(failed)?;
            tx.execute(&st[1], &[&account, &carnet_sync::passkey::slot(public)]).map_err(failed)?;
            Ok(n > 0)
        })
    }

    fn passkeys(&self, account: &[u8]) -> Vec<PasskeySlot> {
        self.read(Vec::new(), |c| {
            let st = c.st("SELECT public, wrapped, card, made FROM passkeys WHERE account = $1 ORDER BY made, n")?;
            Ok(c.client
                .query(&st, &[&account])
                .map_err(failed)?
                .iter()
                .filter_map(|r| Some(PasskeySlot { public: r.get::<_, Vec<u8>>(0).try_into().ok()?, wrapped: r.get(1), card: r.get(2), made: uint(r.get(3)) }))
                .collect())
        })
    }

    fn passkey_slot(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        self.read(None, |c| {
            let st = c.st("SELECT account, wrapped FROM passkeys WHERE public = $1")?;
            Ok(c.client.query_opt(&st, &[&public.as_slice()]).map_err(failed)?.map(|r| (r.get(0), r.get(1))))
        })
    }
}
