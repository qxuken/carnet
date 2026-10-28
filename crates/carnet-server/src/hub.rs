//! The server's side of sync, with no I/O in it: a connection's bytes in,
//! bytes for connections out. It logs devices in by their signatures, joins
//! them to rooms, keeps the records they send and passes them on, and
//! tells each device what changed while it was away. It reads headers
//! only: what is in a record it never knows (docs/SYNC.md §6, §7).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use carnet_sync::compat::{self, Client, Greeting, feature};
use carnet_sync::crypto::{Random, login_message, recovery_message, verify};
use carnet_sync::invite::{InviteCode, RUN_OUT, RUN_OUT_FIRST};
use carnet_sync::pair::NO_SLOT;
use carnet_sync::{passkey, password};
use carnet_sync::elo::{self, Header, Pieces};
use carnet_sync::wire::{
    self, AccountRow, BaseRow, BatchId, Carnet, CrdtType, Deny, DeviceRow, Frame, InviteRow, LogRow, PasskeyRow, Permission, RoomRow, UpdateStatusCode,
};
use loro_protocol::ProtocolMessage;

use crate::blobs::Job;
use crate::limits::{self, Limiter};
use crate::quota::{Admission, ByInvite, NO_ACCOUNT_EMAIL, NOTES_FULL, PasswordEmails, Quota, ROOM_FULL, Unlimited, password_email_is};
use crate::storage::{self, Live, Logged, Span, Storage, covers};

pub type ConnId = u64;

/// What the hub asks its driver to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Send(ConnId, Vec<u8>),
    Close(ConnId),
    /// File work it has allowed, for the worker (`blobs`): links given,
    /// and files put checked, off the hub's thread.
    Blob(Job),
}

/// The largest delta record kept; one past it is refused, not stored. A
/// snapshot may be bigger, up to [`elo::MAX_SNAPSHOT`]: it comes, and
/// goes, in fragments.
pub const MAX_RECORD: usize = 256 * 1024;

/// How much of a room's records goes to a socket that joined it before
/// the rest waits for it to be written ([`Hub::drained`]): a room's
/// records, its base among them, can be more than a socket may have
/// waiting.
pub const BACKFILL_PART: usize = 2 << 20;

/// The longest room id taken: a note's is 32 hex digits, the vault's
/// `vault`. A longer one is never joined, written, or repaired.
pub const MAX_ROOM_ID: usize = 64;

/// What one account's notes may hold (docs/SYNC.md §6, §11), against an
/// account that never compacts: far past what a note's history takes,
/// since compaction keeps a room to 200 records or 256 KiB of them. A
/// room past [`ROOM_RECORDS`] live records or [`ROOM_BYTES`] of them
/// takes no more until it's compacted; an account with [`MAX_ROOMS`]
/// rooms, no new one.
pub const ROOM_RECORDS: u64 = 10_000;
pub const ROOM_BYTES: u64 = 64 << 20;
pub const MAX_ROOMS: u64 = 100_000;

/// Rooms that hold nothing yet, joined by an account's sockets, all told
/// (§6): a socket may join any room its account has, but one that holds
/// nothing costs the account nothing, so the hub would keep any number of
/// made-up ids for it. A note being made is joined before it's written,
/// and a device's first sync joins every note it has before it writes
/// any, so an account may have this many such joins out, or as many as
/// it may still make rooms ([`MAX_ROOMS`] less what it has) if that's
/// more. A join past it is refused (`JoinError`); one written to, left,
/// or of a socket gone counts no more.
pub const NEW_ROOMS: u64 = 256;

/// How long an invite is good for.
pub const INVITE_DAYS: u64 = 7;

/// How long the first account's (the owner's) invite is good for: a day.
/// The server prints a new one on each start until that account is made,
/// so one run out is a restart away; a code in an old log lets no one in.
pub const OWNER_INVITE_DAYS: u64 = 1;

/// The most invites waiting at once: the owner's device can't fill the
/// directory.
const MAX_INVITES: usize = 100;

/// What a removed account's devices are told: "removed" in it, they
/// forget the account, as a removed device does.
/// What a server can do (`compat::feature`): the same on every server of
/// this build, but for passwords, which take its OPAQUE key, and signups,
/// which take a host that mails invites.
pub fn features(passwords: bool, signup: bool) -> Vec<String> {
    let mut f = vec![feature::PASSKEYS, feature::ACCOUNT_DELETE, feature::REMOVE, feature::USAGE];
    if passwords {
        f.push(feature::PASSWORDS);
    }
    if signup {
        f.push(feature::SIGNUP);
    }
    f.into_iter().map(str::to_string).collect()
}

const ACCOUNT_REMOVED: &str = "this account was removed from the server";
/// A login that names an account not here, or a device not in it: the
/// same either way, so a login tells nobody which accounts there are.
const NOT_HERE: &str = "this account isn't on this server (removed, or never here), or this device isn't in it";
/// An invite that can't make an account: used, or never made. One run
/// out is said as such (`invite::RUN_OUT`, `invite::RUN_OUT_FIRST`).
const BAD_INVITE: &str = "that invite is not good: used, run out, or never made";
const DEVICE_REMOVED: &str = "this device was removed from the account";

enum State {
    /// The challenge sent, the device's answer awaited.
    Challenged([u8; 32]),
    /// A new device in a pairing, not let in yet: its challenge kept, for
    /// the login that follows once it is.
    Joining { nonce: [u8; 32] },
    Ready { account: Vec<u8>, device: Vec<u8> },
}

struct Conn {
    state: State,
    rooms: HashSet<String>,
    /// Of `rooms`, those that held nothing when it joined them, and it
    /// hasn't written to since: counted for its account ([`NEW_ROOMS`]).
    new_rooms: HashSet<String>,
    /// Who it is from, as the driver tells (an address): what its tries
    /// are counted against. Empty: not limited.
    client: String,
    /// The protocol its app speaks (`compat`), which app it is and its
    /// version, as its address said.
    app: Client,
    /// Told why a write was refused as more than the server keeps
    /// (`ROOM_FULL`, `NOTES_FULL`): once a socket, however often it sends
    /// again.
    told_full: bool,
    /// Rooms it joined whose records are on their way to it, a part at a
    /// time, the first going now.
    backfills: VecDeque<Backfill>,
    /// A snapshot coming from it in fragments, until whole.
    incoming: Option<Pieces>,
}

/// A room's records on their way to a socket that joined it (§6): sent a
/// part at a time ([`BACKFILL_PART`]), the next once the socket has
/// written most of the last ([`Hub::drained`]).
struct Backfill {
    room: String,
    /// What the device has, by peer: as it joined, and the base's once
    /// that is sent.
    has: HashMap<u64, u64>,
    /// The room's records as read when it joined, not gone through yet;
    /// `None` for a join that waited behind another, read from the store
    /// a part at a time.
    first: Option<VecDeque<Span>>,
    /// A join that waited behind another: its base is read when its turn
    /// comes, not held meanwhile.
    base_due: bool,
    /// The last record's `seq` gone through.
    after: u64,
    /// Messages made and not sent yet: a base's fragments, a part's
    /// batches, and what another device writes to the room meanwhile.
    ready: VecDeque<Vec<u8>>,
}

impl Backfill {
    /// The room's base, first, if the device isn't past it: from then on
    /// it has what the base holds.
    fn base(&mut self, base: Option<&storage::Base>) {
        self.base_due = false;
        let Some(base) = base else { return };
        let has: Vec<(u64, u64)> = self.has.iter().map(|(&p, &c)| (p, c)).collect();
        if covers(&has, &base.vv) {
            return;
        }
        self.ready.extend(encoded(&self.room, std::slice::from_ref(&base.record)));
        for &(p, c) in &base.vv {
            let h = self.has.entry(p).or_insert(0);
            *h = (*h).max(c);
        }
    }
}

/// The numbers a pairing's code starts with: four digits, picked at
/// random (`Hub::free_nameplate`).
const NAMEPLATES: std::ops::RangeInclusive<u32> = 1000..=9999;

/// A pairing open (§7): the device in that opened it, and the new device
/// once one joins. The nameplate is spent on the first join.
struct Pairing {
    opener: ConnId,
    joiner: Option<ConnId>,
    approved: bool,
}

/// The most a wrapped key, or a device id, or a card, may be.
const MAX_WRAPPED: usize = 1024;
const MAX_DEVICE_ID: usize = 64;
const MAX_CARD: usize = 2048;

/// What a device asking for a password is told by a server that takes
/// none.
pub const NO_PASSWORDS: &str = "This server doesn't take passwords: come in by a code from another device, or the recovery words";

/// Compaction (§6): how long a lease lasts, how long a room must have had
/// no new record for a device not alone to be asked, and how long what a
/// base sets aside is kept (so it can be rolled back).
const LEASE: u64 = 600;
pub const QUIET: u64 = 300;
pub const KEEP: u64 = 30 * 86_400;

/// A device asked to send a room as a snapshot, until a time. A repair's
/// snapshot replaces the base whatever it holds.
struct Lease {
    conn: ConnId,
    until: u64,
    repair: bool,
}

/// What the hub has done since it started, for the server's health
/// (`/metrics`): counts only, never what anything held.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Logins let in, and turned away (a wrong signature, a removed device).
    pub logins: u64,
    pub logins_refused: u64,
    /// Sockets turned away for any reason, the logins above among them.
    pub denied: u64,
    /// Accounts made by invite, and invites refused.
    pub enrolled: u64,
    pub enroll_refused: u64,
    /// Pairings opened, and devices let in by the recovery words.
    pub pairings: u64,
    pub recoveries: u64,
    /// Records kept, their bytes, and batches refused (malformed, too big,
    /// for a room not joined).
    pub records: u64,
    pub record_bytes: u64,
    pub batches_refused: u64,
    /// Rooms compacted: a snapshot taken as a room's base.
    pub snapshots: u64,
    /// Writes to the storage that failed: the one sign the disk is in
    /// trouble.
    pub storage_failures: u64,
    /// Sockets turned away for trying too much from one address.
    pub limited: u64,
    /// Logins let in, by the app and version they came from: which
    /// builds are about. Past [`APPS`] of them, the rest under "other".
    pub logins_by_app: BTreeMap<(String, String), u64>,
}

/// How many apps and versions are counted apart, in the stats and the
/// sockets open: what anyone can claim stays a short list of labels.
pub const APPS: usize = 64;

/// `c`'s app and version, or ("other", "") once `seen` holds [`APPS`]
/// others.
fn app_key<V>(seen: &BTreeMap<(String, String), V>, c: &Client) -> (String, String) {
    let key = (if c.app.is_empty() { "unknown".to_string() } else { c.app.clone() }, c.version.clone());
    if seen.contains_key(&key) || seen.len() < APPS { key } else { ("other".into(), String::new()) }
}

/// What the hub holds open now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Open {
    /// Sockets, and of them those logged in.
    pub sockets: u64,
    pub logged_in: u64,
    pub pairings: u64,
    /// Sockets logged in, by the protocol their app speaks: which apps
    /// are still about, before the oldest is let go (§14).
    pub protocols: BTreeMap<u32, u64>,
    /// And by app and version ([`APPS`] at most, the rest "other").
    pub apps: BTreeMap<(String, String), u64>,
}

pub struct Hub<S: Storage> {
    origin: String,
    store: S,
    conns: HashMap<ConnId, Conn>,
    pairings: HashMap<u32, Pairing>,
    /// The time, in seconds since 1970, as the driver last said: what
    /// invites are good until is measured against it.
    now: u64,
    /// By account and room.
    leases: HashMap<(Vec<u8>, String), Lease>,
    /// When a room last took a record, by account and room (this run's),
    /// for [`QUIET`] after: older says no more than none.
    last_write: HashMap<(Vec<u8>, String), u64>,
    last_purge: u64,
    /// A room is compacted past this many live records, or bytes of them.
    compact_count: usize,
    compact_bytes: usize,
    /// A room takes no more deltas past these, an account no new room
    /// past `max_rooms` ([`ROOM_RECORDS`], [`ROOM_BYTES`], [`MAX_ROOMS`]
    /// unless said).
    room_records: u64,
    room_bytes: u64,
    max_rooms: u64,
    /// What each account's notes take, as last counted and written since:
    /// what a write is held to [`Quota::notes_ceiling`] by, without the
    /// whole account counted each time. Never less than they take (a
    /// record a longer one covers isn't taken off); counted anew hourly,
    /// and when anything is removed.
    notes: HashMap<Vec<u8>, u64>,
    stats: Stats,
    /// Tries by client: a failed login, an invite, a pairing joined, a
    /// recovery.
    tries: Limiter,
    /// Password sign-ins by email, from any client
    /// ([`limits::PASSWORD_TRIES`]): emails a password signs in as, so as
    /// many as there are passwords, however many made-up ones come.
    password_tries: Limiter,
    /// And emails none does, counted alike, so a refusal never tells
    /// which an email is; kept apart, so made-up ones crowd out no real
    /// one, and let through once it's full rather than refused.
    unknown_tries: Limiter,
    /// How much each account may keep, and whether it may write.
    quota: std::sync::Arc<dyn Quota>,
    /// Told of each account an invite makes.
    admission: std::sync::Arc<dyn Admission>,
    /// Its OPAQUE key, if it takes passwords (§7); a server of one's own
    /// doesn't, unless asked.
    passwords: Option<password::Server>,
    /// Its host takes signups by email.
    signup: bool,
    /// Which email a password may sign in as.
    password_emails: PasswordEmails,
    /// How much of a room's records goes before the rest waits.
    backfill_part: usize,
    /// Sockets with more of a room's records to come, once they've
    /// written what they were sent: [`Hub::take_more`].
    more: BTreeSet<ConnId>,
    /// By account, its sockets' joins of rooms that held nothing
    /// ([`NEW_ROOMS`]).
    new_joins: HashMap<Vec<u8>, NewJoins>,
}

/// An account's sockets' joins of rooms that held nothing yet, and, once
/// they're past [`NEW_ROOMS`], how many it may have: counted once, not
/// at each join, until they're under it again.
#[derive(Default)]
struct NewJoins {
    joins: u64,
    may: Option<u64>,
}

impl<S: Storage> Hub<S> {
    /// A hub for the server at `origin` (`https://notes.example.com`): the
    /// name logins are signed for.
    pub fn new(origin: impl Into<String>, store: S) -> Self {
        Hub {
            origin: origin.into(),
            store,
            conns: HashMap::new(),
            pairings: HashMap::new(),
            now: 0,
            leases: HashMap::new(),
            last_write: HashMap::new(),
            last_purge: 0,
            compact_count: 200,
            compact_bytes: 256 * 1024,
            room_records: ROOM_RECORDS,
            room_bytes: ROOM_BYTES,
            max_rooms: MAX_ROOMS,
            notes: HashMap::new(),
            stats: Stats::default(),
            tries: Limiter::new(limits::TRIES),
            password_tries: Limiter::new(limits::PASSWORD_TRIES),
            unknown_tries: Limiter::new(limits::PASSWORD_TRIES),
            quota: std::sync::Arc::new(Unlimited),
            admission: std::sync::Arc::new(ByInvite),
            passwords: None,
            signup: false,
            password_emails: PasswordEmails::Any,
            backfill_part: BACKFILL_PART,
            more: BTreeSet::new(),
            new_joins: HashMap::new(),
        }
    }

    /// How much of a room's records goes to a socket before the rest waits
    /// for it to be written ([`BACKFILL_PART`] unless said): a driver
    /// holding sockets to a smaller budget says so.
    pub fn backfill_part(&mut self, bytes: usize) {
        self.backfill_part = bytes.max(1);
    }

    /// The sockets with more of a room's records to come since this was
    /// last asked: each is to be [`drained`](Hub::drained) once it has
    /// written most of what it was sent (at once, for a driver that
    /// doesn't hold sockets to a budget).
    pub fn take_more(&mut self) -> Vec<ConnId> {
        std::mem::take(&mut self.more).into_iter().collect()
    }

    /// `conn` has written most of what it was sent: the next part of the
    /// rooms it joined.
    pub fn drained(&mut self, conn: ConnId) -> Vec<Out> {
        self.more.remove(&conn);
        let Some(State::Ready { account, .. }) = self.conns.get(&conn).map(|c| &c.state) else { return Vec::new() };
        let account = account.clone();
        self.backfill(conn, &account)
    }

    /// Taking passwords, with this OPAQUE key (kept: a new one would make
    /// every password set so far fail).
    pub fn with_passwords(mut self, server: password::Server) -> Self {
        self.passwords = Some(server);
        self
    }

    pub fn set_passwords(&mut self, server: password::Server) {
        self.passwords = Some(server);
    }

    /// Holding passwords to `emails` (any, unless said).
    /// Its host takes signups by email (`feature::SIGNUP`).
    pub fn with_signup(mut self, signup: bool) -> Self {
        self.signup = signup;
        self
    }

    pub fn with_password_emails(mut self, emails: PasswordEmails) -> Self {
        self.password_emails = emails;
        self
    }

    pub fn set_password_emails(&mut self, emails: PasswordEmails) {
        self.password_emails = emails;
    }

    /// The one email `account`'s password may sign in as, if this server
    /// holds it to one: `Some("")` when it would and the host knows none.
    fn password_email(&self, account: &[u8]) -> Option<String> {
        match self.password_emails {
            PasswordEmails::Any => None,
            PasswordEmails::Account => Some(self.admission.email_of(account).map(|e| password::id(&e)).unwrap_or_default()),
        }
    }

    /// Telling `admission` of each account an invite makes.
    pub fn with_admission(mut self, admission: std::sync::Arc<dyn Admission>) -> Self {
        self.admission = admission;
        self
    }

    /// With limits on what each account keeps (a hosted service's plans).
    pub fn with_quota(mut self, quota: std::sync::Arc<dyn Quota>) -> Self {
        self.quota = quota;
        self
    }

    pub fn set_quota(&mut self, quota: std::sync::Arc<dyn Quota>) {
        self.quota = quota;
    }

    pub fn set_admission(&mut self, admission: std::sync::Arc<dyn Admission>) {
        self.admission = admission;
    }

    /// What the hub has done since it started.
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// What it keeps for compaction: rooms' last writes, and leases.
    pub fn compaction_kept(&self) -> (usize, usize) {
        (self.last_write.len(), self.leases.len())
    }

    /// `account`'s sockets' joins of rooms that held nothing, not yet
    /// written to ([`NEW_ROOMS`]).
    pub fn new_joins(&self, account: &[u8]) -> u64 {
        self.new_joins.get(account).map_or(0, |j| j.joins)
    }

    /// What it holds open now.
    pub fn open(&self) -> Open {
        Open {
            sockets: self.conns.len() as u64,
            logged_in: self.conns.values().filter(|c| matches!(c.state, State::Ready { .. })).count() as u64,
            pairings: self.pairings.len() as u64,
            protocols: self.conns.values().filter(|c| matches!(c.state, State::Ready { .. })).fold(BTreeMap::new(), |mut m, c| {
                *m.entry(c.app.protocol).or_default() += 1;
                m
            }),
            apps: self.conns.values().filter(|c| matches!(c.state, State::Ready { .. })).fold(BTreeMap::new(), |mut m, c| {
                *m.entry(app_key(&m, &c.app)).or_default() += 1;
                m
            }),
        }
    }

    /// A write to the storage, counted if it failed.
    fn kept<T>(&mut self, r: Result<T, storage::Failed>) -> Result<T, storage::Failed> {
        if r.is_err() {
            self.stats.storage_failures += 1;
        }
        r
    }

    /// A device let in by a pairing or a recovery, added: a failure to
    /// write it counted as [`kept`](Hub::kept) counts one, an id another
    /// device has already refused and not.
    fn device_added(&mut self, account: &[u8], device: &[u8], public: [u8; 32]) -> Result<(), storage::Failed> {
        let added = self.store.add_device(account, device, public);
        if added.as_ref().is_err_and(|e| e.0 != storage::DEVICE_TAKEN) {
            self.stats.storage_failures += 1;
        }
        added
    }

    /// When a room is compacted: past `count` live records, or `bytes` of
    /// them (200 and 256 KiB unless said).
    pub fn compact_when(&mut self, count: usize, bytes: usize) {
        self.compact_count = count;
        self.compact_bytes = bytes;
        // A room is always compacted well before it's refused.
        self.room_records = self.room_records.max(count as u64 * 10);
        self.room_bytes = self.room_bytes.max(bytes as u64 * 10);
    }

    /// What a room may hold before it takes no more deltas, and how many
    /// rooms an account may have ([`ROOM_RECORDS`], [`ROOM_BYTES`],
    /// [`MAX_ROOMS`] unless said).
    pub fn room_caps(&mut self, records: u64, bytes: u64, rooms: u64) {
        self.room_records = records;
        self.room_bytes = bytes;
        self.max_rooms = rooms;
    }

    /// What `account`'s notes take, as last counted and written since.
    fn notes_used(&mut self, account: &[u8]) -> u64 {
        if let Some(&n) = self.notes.get(account) {
            return n;
        }
        let n = self.store.usage(account);
        self.notes.insert(account.to_vec(), n);
        n
    }

    /// Why a write of `room` can't be taken now, if it can't: the room
    /// past what it holds until compacted, or the account's notes past
    /// their ceiling or as many as it may have. And what the room holds,
    /// counted without a record read.
    fn room_full(&mut self, account: &[u8], room: &str) -> (Option<&'static str>, Live) {
        let live = self.store.live(account, room);
        let why = if live.records >= self.room_records || live.bytes >= self.room_bytes {
            Some(ROOM_FULL)
        } else if self.notes_used(account) >= self.quota.notes_ceiling(account) || (live.is_empty() && self.store.room_count(account) >= self.max_rooms) {
            Some(NOTES_FULL)
        } else {
            None
        };
        (why, live)
    }

    /// The driver says what time it is (the hub has no clock of its own).
    /// Once an hour, what bases set aside more than [`KEEP`] ago goes, and
    /// what's kept for compaction once it can't matter (a room's last
    /// write past [`QUIET`], a lease past its time); what notes take is
    /// counted anew.
    pub fn tick(&mut self, now: u64) {
        self.now = now;
        if now >= self.last_purge + 3600 {
            self.last_purge = now;
            self.tries.prune(now as f64);
            self.unknown_tries.prune(now as f64);
            let purged = self.store.purge(now.saturating_sub(KEEP));
            let _ = self.kept(purged);
            self.last_write.retain(|_, &mut t| now < t + QUIET);
            self.leases.retain(|_, l| l.until > now);
            self.notes.clear();
        }
    }

    /// An invite redeemed: a new account, this device its first. Its id
    /// comes back.
    pub fn enroll(&mut self, invite: &InviteCode, device: &[u8], public: [u8; 32], rng: &mut impl Random) -> Result<Vec<u8>, String> {
        let made = self.make_account(invite, device, public, rng);
        match made {
            Ok(_) => self.stats.enrolled += 1,
            Err(_) => self.stats.enroll_refused += 1,
        }
        made
    }

    fn make_account(&mut self, invite: &InviteCode, device: &[u8], public: [u8; 32], rng: &mut impl Random) -> Result<Vec<u8>, String> {
        if device.is_empty() || device.len() > MAX_DEVICE_ID {
            return Err("a device id is 1 to 64 bytes".into());
        }
        // One the host won't let in is told as one that isn't good.
        if !self.admission.admit(&invite.hash()) {
            return Err(BAD_INVITE.into());
        }
        // Looked at first, as taking it drops it: one run out is told so,
        // with what gets a new one.
        let known = self.store.find_invite(invite.hash());
        let taken = self.store.take_invite(invite.hash(), self.now);
        let owner = self.kept(taken).map_err(|e| e.to_string())?.ok_or(match known {
            Some((true, expires)) if expires <= self.now => RUN_OUT_FIRST,
            Some((false, expires)) if expires <= self.now => RUN_OUT,
            _ => BAD_INVITE,
        })?;
        let account: [u8; 16] = rng.bytes();
        let added = self.store.add_account(&account, owner);
        self.kept(added).map_err(|e| e.to_string())?;
        let added = self.store.add_device(&account, device, public);
        self.kept(added).map_err(|e| e.to_string())?;
        self.admission.admitted(invite.hash(), &account);
        // The owner's in: the server's other invites for the first account
        // (one printed on each start until then) are good no more.
        if owner {
            for (hash, _, _) in self.store.invites().into_iter().filter(|&(_, o, _)| o) {
                let dropped = self.store.drop_invite(hash);
                let _ = self.kept(dropped);
            }
        }
        Ok(account.to_vec())
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    /// The storage back, every connection forgotten: a server going down.
    pub fn into_store(self) -> S {
        self.store
    }

    /// A socket opened, from a client the hub can't tell, of this build:
    /// never limited (the tests', the simulation's).
    pub fn connect(&mut self, conn: ConnId, rng: &mut impl Random) -> Vec<Out> {
        self.connect_from(conn, "", Client { protocol: compat::PROTOCOL, ..Client::default() }, rng)
    }

    /// A socket opened from `client` (an address), by `app` (its
    /// protocol, name and version): the challenge goes first, its nonce
    /// fresh, with this server's greeting. An app older than it takes is
    /// turned away after it, so the app reads why from the greeting too.
    pub fn connect_from(&mut self, conn: ConnId, client: &str, app: Client, rng: &mut impl Random) -> Vec<Out> {
        let nonce: [u8; 32] = rng.bytes();
        let protocol = app.protocol;
        self.conns.insert(conn, Conn {
            state: State::Challenged(nonce),
            rooms: HashSet::new(),
            new_rooms: HashSet::new(),
            client: client.to_string(),
            app,
            told_full: false,
            backfills: VecDeque::new(),
            incoming: None,
        });
        let mut out = vec![self.challenge(conn, nonce)];
        if compat::takes(protocol) == compat::Fit::AppTooOld {
            out.extend(self.deny_as(conn, compat::APP_TOO_OLD, Deny::Outdated));
        }
        out
    }

    /// Whether `conn` is a device logged in: past its challenge, for good.
    pub fn logged_in(&self, conn: ConnId) -> bool {
        self.conns.get(&conn).is_some_and(|c| matches!(c.state, State::Ready { .. }))
    }

    /// What this server can do, as its greeting lists it.
    pub fn features(&self) -> Vec<String> {
        features(self.passwords.is_some(), self.signup)
    }

    /// A socket forgotten, and its joins of rooms that held nothing with
    /// it.
    fn remove_conn(&mut self, conn: ConnId) -> Option<Conn> {
        let c = self.conns.remove(&conn)?;
        if let State::Ready { account, .. } = &c.state {
            self.new_rooms_gone(account, c.new_rooms.len() as u64);
        }
        Some(c)
    }

    /// `conn`'s join of `room`, if it held nothing when joined, counted
    /// no more: written to, left, or the room removed.
    fn new_room_gone(&mut self, conn: ConnId, account: &[u8], room: &str) {
        if self.conns.get_mut(&conn).is_some_and(|c| c.new_rooms.remove(room)) {
            self.new_rooms_gone(account, 1);
        }
    }

    fn new_rooms_gone(&mut self, account: &[u8], n: u64) {
        if n == 0 {
            return;
        }
        if let Some(j) = self.new_joins.get_mut(account) {
            j.joins = j.joins.saturating_sub(n);
            if j.joins < NEW_ROOMS {
                j.may = None;
            }
            if j.joins == 0 {
                self.new_joins.remove(account);
            }
        }
    }

    /// Whether `conn` may join `room`, which holds nothing: it has
    /// already, or its account's sockets have fewer such joins out than
    /// [`NEW_ROOMS`], or than the rooms it may still make if that's more.
    fn may_join_new(&mut self, conn: ConnId, account: &[u8], room: &str) -> bool {
        if self.conns.get(&conn).is_some_and(|c| c.new_rooms.contains(room)) {
            return true;
        }
        let joins = self.new_joins.get(account).map_or(0, |j| j.joins);
        if joins < NEW_ROOMS {
            return true;
        }
        let may = match self.new_joins.get(account).and_then(|j| j.may) {
            Some(may) => may,
            None => {
                let may = NEW_ROOMS.max(self.max_rooms.saturating_sub(self.store.room_count(account)));
                self.new_joins.entry(account.to_vec()).or_default().may = Some(may);
                may
            }
        };
        joins < may
    }

    fn challenge(&self, conn: ConnId, nonce: [u8; 32]) -> Out {
        carnet(conn, Carnet::Challenge { nonce, server: Greeting::ours(self.features()) })
    }

    /// One try from `conn`'s client, counted: whether it may.
    fn tried(&mut self, conn: ConnId) -> bool {
        let client = self.conns.get(&conn).map(|c| c.client.clone()).unwrap_or_default();
        let ok = self.tries.allow(&client, self.now as f64);
        if !ok {
            self.stats.limited += 1;
        }
        ok
    }

    /// A socket gone. A pairing it was in is over, and the other side told.
    /// A device logged in that went offline with it (no other socket of
    /// its own open) was last seen now, and the account's other devices
    /// online are sent their list again, so none says it's online (§7).
    pub fn disconnect(&mut self, conn: ConnId) -> Vec<Out> {
        self.more.remove(&conn);
        let gone = self.remove_conn(conn).and_then(|c| match c.state {
            State::Ready { account, device } => Some((account, device)),
            _ => None,
        });
        self.leases.retain(|_, l| l.conn != conn);
        let mut out = self.end_pairing_of(conn, "the other device went away");
        if let Some((account, _)) = &gone
            && self.online(account).is_empty()
        {
            self.notes.remove(account);
        }
        if let Some((account, device)) = gone
            && !self.online(&account).contains(&device)
        {
            let seen = self.store.set_seen(&account, &device, self.now);
            let _ = self.kept(seen);
            out.extend(self.lists_to_others(conn, &account));
        }
        out
    }

    pub fn receive(&mut self, conn: ConnId, bytes: &[u8], rng: &mut impl Random) -> Vec<Out> {
        let Some(c) = self.conns.get(&conn) else { return Vec::new() };
        let Ok(frame) = wire::decode(bytes) else {
            return self.deny(conn, "not a frame");
        };
        // A later app's frame this server doesn't know: skipped (§14).
        if let Frame::Carnet(Carnet::Unknown { .. }) = frame {
            return Vec::new();
        }
        let nonce = match c.state {
            State::Challenged(n) | State::Joining { nonce: n } => Some(n),
            State::Ready { .. } => None,
        };
        let (account, me) = match &c.state {
            State::Ready { account, device } => (Some(account.clone()), device.clone()),
            _ => (None, Vec::new()),
        };
        match (nonce, account, frame) {
            (Some(nonce), _, Frame::Carnet(Carnet::Hello { account, device, signature, since })) => {
                // The nonce is spent whatever comes of it; a pairing this
                // device was joining is done.
                let mut out = self.end_pairing_of(conn, "the new device is in");
                let known = self.store.device(&account, &device);
                let ok = known.as_ref().is_some_and(|d| {
                    !d.revoked && verify(&d.public, &login_message(&self.origin, &account, &device, &nonce), &signature)
                });
                if !ok {
                    self.stats.logins_refused += 1;
                    if !self.tried(conn) {
                        out.extend(self.deny(conn, limits::TOO_MANY));
                        return out;
                    }
                    // A device removed is told so: its app forgets the
                    // account. An account not here and a device not in
                    // one are told the same (an account deleted while its
                    // device was away is one): its app forgets it too. A
                    // device here whose signature is wrong (another
                    // server's origin, another key) is only turned away.
                    let (reason, why) = match known {
                        Some(d) if d.revoked => (DEVICE_REMOVED, Deny::DeviceRemoved),
                        None => (NOT_HERE, Deny::AccountRemoved),
                        Some(_) => ("not a device of this account", Deny::Other),
                    };
                    out.extend(self.deny_as(conn, reason, why));
                    return out;
                }
                let seq = self.store.seq(&account);
                let changed = self.store.changed_since(&account, since);
                self.stats.logins += 1;
                if let Some(c) = self.conns.get(&conn) {
                    let key = app_key(&self.stats.logins_by_app, &c.app);
                    *self.stats.logins_by_app.entry(key).or_default() += 1;
                }
                let seen = self.store.set_seen(&account, &device, self.now);
                let _ = self.kept(seen);
                let rekeys = self.store.rekeys(&account, &device);
                if let Some(c) = self.conns.get_mut(&conn) {
                    c.state = State::Ready { account, device };
                }
                out.push(carnet(conn, Carnet::Welcome { seq }));
                // Each epoch's key made while it was away (one it has
                // already, it leaves as is).
                for sealed in rekeys {
                    out.push(carnet(conn, Carnet::Rekey { sealed }));
                }
                if !changed.is_empty() {
                    out.push(carnet(conn, Carnet::Changed { rooms: changed }));
                }
                out
            }
            (None, Some(account), Frame::Room(m)) => self.room_message(conn, &account, &me, m),
            (None, Some(account), Frame::Carnet(c)) => self.ready_message(conn, account, me, c, rng),
            // Tries at something: counted, and past the limit refused.
            (Some(_), None, Frame::Carnet(Carnet::PairJoin { .. } | Carnet::Recover { .. } | Carnet::Enroll { .. } | Carnet::PasswordLogin { .. }))
                if !self.tried(conn) =>
            {
                self.deny(conn, limits::TOO_MANY)
            }
            (Some(nonce), None, Frame::Carnet(Carnet::PairJoin { nameplate, pake })) => self.join(conn, nonce, nameplate, pake),
            (Some(_), None, Frame::Carnet(Carnet::PairStep { data })) => self.relay(conn, Carnet::PairStep { data }),
            (Some(_), None, Frame::Carnet(Carnet::PairClose { reason })) => self.end_pairing_of(conn, &reason),
            // OPAQUE's answer, from the record (or made up, for an email
            // nobody signs in as): the device proves the password next, by
            // a recovery signed with what it comes to.
            (Some(_), None, Frame::Carnet(Carnet::PasswordLogin { id, request })) => {
                if self.passwords.is_none() {
                    return self.deny(conn, NO_PASSWORDS);
                }
                let id = password::id(&id);
                let record = self.store.password_of(&id).map(|(_, r)| r);
                // Counted by the email too, from wherever it's tried, and
                // one that signs in to nothing just the same: were it not,
                // a refusal would say which emails have an account.
                let now = self.now as f64;
                let counted = if record.is_some() { self.password_tries.allow(&id, now) } else { self.unknown_tries.allow_unless_full(&id, now) };
                if !counted {
                    self.stats.limited += 1;
                    return self.deny(conn, limits::TOO_MANY_FOR_EMAIL);
                }
                let Some(server) = &self.passwords else { return self.deny(conn, NO_PASSWORDS) };
                match server.login(&id, record.as_deref(), &request, rng) {
                    Ok(response) => vec![carnet(conn, Carnet::PasswordLoginReply { response })],
                    Err(_) => self.deny(conn, "not a sign-in"),
                }
            }
            (Some(nonce), None, Frame::Carnet(Carnet::Recover { public, signature, device, device_public })) => {
                self.recover(conn, nonce, public, signature, device, device_public, rng)
            }
            (Some(_), None, Frame::Carnet(Carnet::Enroll { invite, device, public })) => {
                let made = InviteCode::parse(&invite)
                    .ok_or_else(|| "that is not an invite code".to_string())
                    .and_then(|code| self.enroll(&code, &device, public, rng));
                match made {
                    Ok(account) => {
                        // A fresh challenge: the new device logs in as any.
                        let fresh: [u8; 32] = rng.bytes();
                        if let Some(c) = self.conns.get_mut(&conn) {
                            c.state = State::Challenged(fresh);
                        }
                        vec![carnet(conn, Carnet::Enrolled { account }), self.challenge(conn, fresh)]
                    }
                    Err(e) => self.deny(conn, &e),
                }
            }
            _ => self.deny(conn, "not now"),
        }
    }

    /// What a logged-in device asks beside its rooms: pairings, recovery.
    fn ready_message(&mut self, conn: ConnId, account: Vec<u8>, me: Vec<u8>, c: Carnet, rng: &mut impl Random) -> Vec<Out> {
        match c {
            Carnet::PairOpen => {
                let mut out = self.end_pairing_of(conn, "another pairing opened");
                let nameplate = self.free_nameplate(rng);
                self.pairings.insert(nameplate, Pairing { opener: conn, joiner: None, approved: false });
                self.stats.pairings += 1;
                out.push(carnet(conn, Carnet::PairOpened { nameplate }));
                out
            }
            Carnet::PairStep { data } => self.relay(conn, Carnet::PairStep { data }),
            Carnet::PairApprove { device, public } => {
                let Some((_, p)) = self.pairings.iter_mut().find(|(_, p)| p.opener == conn) else { return Vec::new() };
                if p.joiner.is_none() || p.approved || device.is_empty() || device.len() > MAX_DEVICE_ID {
                    return Vec::new();
                }
                p.approved = true;
                // The device in vouches for it; the next login is its own.
                if self.device_added(&account, &device, public).is_err() {
                    return self.end_pairing_of(conn, "the server could not keep the new device");
                }
                Vec::new()
            }
            Carnet::PairClose { reason } => self.end_pairing_of(conn, &reason),
            Carnet::PasswordSetStart { id, request } => {
                let Some(server) = &self.passwords else { return vec![carnet(conn, Carnet::Refused { reason: NO_PASSWORDS.into() })] };
                let id = password::id(&id);
                if let Some(reason) = self.password_taken(&account, &id) {
                    return vec![carnet(conn, Carnet::Refused { reason })];
                }
                match server.register(&id, &request) {
                    Ok(response) => vec![carnet(conn, Carnet::PasswordSetReply { response })],
                    Err(_) => vec![carnet(conn, Carnet::Refused { reason: "the password couldn't be set".into() })],
                }
            }
            Carnet::PasswordSetFinish { id, upload, public, wrapped, card } => {
                if self.passwords.is_none() {
                    return vec![carnet(conn, Carnet::Refused { reason: NO_PASSWORDS.into() })];
                }
                let id = password::id(&id);
                if let Some(reason) = self.password_taken(&account, &id) {
                    return vec![carnet(conn, Carnet::Refused { reason })];
                }
                if upload.len() > 4096 || wrapped.len() > MAX_WRAPPED * 8 || card.len() > MAX_CARD {
                    return self.deny(conn, "a password's record past its size");
                }
                if self.store.password_slot(&public).is_some_and(|(a, _)| a != account)
                    || self.store.recovery(&public).is_some()
                    || self.store.passkey_slot(&public).is_some()
                {
                    return vec![carnet(conn, Carnet::Refused { reason: "the password couldn't be set".into() })];
                }
                let Ok(record) = password::Server::record(&upload) else {
                    return vec![carnet(conn, Carnet::Refused { reason: "the password couldn't be set".into() })];
                };
                let set = self.store.set_password(&account, storage::PasswordSlot { id, record, public, wrapped, card });
                if self.kept(set).is_err() {
                    return vec![carnet(conn, Carnet::Refused { reason: "the server could not keep the password".into() })];
                }
                let _ = self.store.log(&account, Logged { at: self.now, device: me.clone(), room: String::new(), what: "set the password".into() });
                let mut out = vec![carnet(conn, Carnet::PasswordSet)];
                out.extend(self.lists_to_others(conn, &account));
                out
            }
            Carnet::PasswordDrop => {
                let dropped = self.store.drop_password(&account);
                if self.kept(dropped).is_err() {
                    return vec![carnet(conn, Carnet::Refused { reason: "the server could not take the password away".into() })];
                }
                let _ = self.store.log(&account, Logged { at: self.now, device: me.clone(), room: String::new(), what: "took the password away".into() });
                let mut out = vec![carnet(conn, Carnet::PasswordSet)];
                out.extend(self.lists_to_others(conn, &account));
                out
            }
            Carnet::PasskeyAdd { public, wrapped, card } => {
                if wrapped.len() > MAX_WRAPPED * 8 || card.len() > MAX_CARD {
                    return self.deny(conn, "a passkey's slot past its size");
                }
                let refuse = |reason: &str| vec![carnet(conn, Carnet::Refused { reason: reason.into() })];
                // A key another slot has, of any account: never two slots
                // one key, so a sign-in finds one.
                if self.store.passkey_slot(&public).is_some_and(|(a, _)| a != account)
                    || self.store.recovery(&public).is_some()
                    || self.store.password_slot(&public).is_some()
                {
                    return refuse("the passkey couldn't be added");
                }
                let mine = self.store.passkeys(&account);
                if !mine.iter().any(|p| p.public == public) && mine.len() >= passkey::MAX {
                    return refuse(&format!("{} passkeys at most: remove one first", passkey::MAX));
                }
                let added = self.store.add_passkey(&account, storage::PasskeySlot { public, wrapped, card, made: self.now });
                if self.kept(added).is_err() {
                    return refuse("the server could not keep the passkey");
                }
                let _ = self.store.log(&account, Logged { at: self.now, device: me.clone(), room: String::new(), what: "added a passkey".into() });
                let mut out = vec![carnet(conn, Carnet::PasskeySet)];
                out.extend(self.lists_to_others(conn, &account));
                out
            }
            Carnet::PasskeyDrop { public } => {
                let dropped = self.store.drop_passkey(&account, &public);
                let reason = match self.kept(dropped) {
                    Ok(true) => None,
                    Ok(false) => Some("that passkey isn't in the account"),
                    Err(_) => Some("the server could not take the passkey away"),
                };
                if let Some(reason) = reason {
                    return vec![carnet(conn, Carnet::Refused { reason: reason.into() })];
                }
                let _ = self.store.log(&account, Logged { at: self.now, device: me.clone(), room: String::new(), what: "took a passkey away".into() });
                let mut out = vec![carnet(conn, Carnet::PasskeySet)];
                out.extend(self.lists_to_others(conn, &account));
                out
            }
            Carnet::SetRecovery { public, wrapped, card } => {
                if wrapped.len() > MAX_WRAPPED * 8 || card.len() > MAX_CARD {
                    return self.deny(conn, "a wrapped key past its size");
                }
                let refuse = |reason: &str| vec![carnet(conn, Carnet::Refused { reason: reason.into() })];
                // A key another account's code has, or any password's or
                // passkey's: never two slots one key, so a recovery finds
                // the one it was made for, and no account takes over
                // another's.
                if self.store.recovery(&public).is_some_and(|(a, _)| a != account)
                    || self.store.password_slot(&public).is_some()
                    || self.store.passkey_slot(&public).is_some()
                {
                    return refuse("the recovery code couldn't be set");
                }
                let set = self.store.set_recovery(&account, public, wrapped, card);
                match self.kept(set) {
                    Ok(()) => vec![carnet(conn, Carnet::RecoverySet)],
                    Err(_) => refuse("the server could not keep the recovery code"),
                }
            }
            Carnet::SetCard { card } => {
                if card.len() > MAX_CARD {
                    return self.deny(conn, "a card past its size");
                }
                // Sealed under an epoch before the account's: from a device
                // that hasn't the newest key yet (it sends its card as it
                // logs in, before the keys made while it was away). The
                // card the removal sealed for it under the newest stays,
                // which is the only one the others believe (§7).
                if card.get(..4).map(|e| u32::from_be_bytes(e.try_into().expect("4"))) < Some(self.store.epoch(&account)) {
                    return Vec::new();
                }
                if self.store.set_card(&account, &me, card).is_err() {
                    return Vec::new();
                }
                // Its name, or its first: the others' lists say so now.
                self.lists_to_others(conn, &account)
            }
            Carnet::ListDevices => vec![self.devices_frame(conn, &account, &me)],
            Carnet::Revoke { device, epoch, keys, cards } => self.revoke(conn, &account, &me, device, epoch, keys, cards),
            Carnet::Checked { room, vv, ok } => self.checked(&account, &me, &room, &vv, ok),
            Carnet::Inspect => vec![self.inspected(conn, &account)],
            Carnet::RollBack { room } => match self.store.roll_back(&account, &room) {
                Ok(true) => {
                    self.notes.remove(&account);
                    let _ = self.store.log(&account, Logged { at: self.now, device: me, room, what: "rolled back".into() });
                    vec![self.inspected(conn, &account)]
                }
                _ => vec![carnet(conn, Carnet::Refused {
                    reason: "that room's base can't be rolled back: what it stands for is no longer kept".into(),
                })],
            },
            // Files: allowed here (logged in, this account); the worker
            // gives the links, and checks what was put. No more than
            // `MAX_IDS` of a frame are answered: a device sends no more.
            Carnet::BlobOffer { mut ids, mut sizes } => {
                ids.truncate(wire::MAX_IDS);
                sizes.truncate(ids.len());
                // Room left for files: the limit, less the notes.
                let limit = self.quota.limit(&account);
                let room = limit.bytes.map(|b| b.saturating_sub(self.store.usage(&account)));
                vec![Out::Blob(Job::Offer { conn, account, ids, sizes, room, writable: limit.writable })]
            }
            Carnet::Usage => {
                let notes = self.store.usage(&account);
                vec![Out::Blob(Job::Usage { conn, limit: self.quota.limit(&account), account, notes })]
            }
            Carnet::BlobFetch { mut ids } => {
                ids.truncate(wire::MAX_IDS);
                vec![Out::Blob(Job::Fetch { conn, account, ids })]
            }
            Carnet::BlobDrop { mut ids } => {
                ids.truncate(wire::MAX_IDS);
                vec![Out::Blob(Job::Drop { conn, account, ids })]
            }
            Carnet::RoomRemove { room } => self.remove_room(conn, &account, &me, room),
            // Deleted from one of its own devices. The server's owner's
            // can't be: it's what lets everyone else on.
            Carnet::AccountDelete if self.store.is_owner(&account) => {
                vec![carnet(conn, Carnet::Refused { reason: "This is the server's owner's account: it can't be deleted from here".into() })]
            }
            Carnet::AccountDelete => match self.remove_account(&account) {
                Ok(out) => out,
                Err(_) => vec![carnet(conn, Carnet::Refused { reason: "the server could not delete the account".into() })],
            },
            Carnet::Invites => vec![self.invite_list(conn, &account)],
            Carnet::InviteAdd { .. } | Carnet::InviteDrop { .. } | Carnet::AccountRemove { .. } if !self.store.is_owner(&account) => {
                vec![carnet(conn, Carnet::Refused { reason: "only the server's owner can do that".into() })]
            }
            Carnet::InviteAdd { hash } => {
                let waiting = self.store.invites().iter().filter(|(_, _, until)| *until > self.now).count();
                if waiting >= MAX_INVITES {
                    return vec![carnet(conn, Carnet::Refused { reason: format!("{MAX_INVITES} invites are waiting already: take some back first") })];
                }
                let added = self.store.add_invite(hash, false, self.now + INVITE_DAYS * 86_400);
                let _ = self.kept(added);
                vec![self.invite_list(conn, &account)]
            }
            Carnet::InviteDrop { hash } => {
                let dropped = self.store.drop_invite(hash);
                let _ = self.kept(dropped);
                vec![self.invite_list(conn, &account)]
            }
            Carnet::AccountRemove { account: gone } => {
                if gone == account || self.store.is_owner(&gone) {
                    return vec![carnet(conn, Carnet::Refused { reason: "the owner's own account can't be removed".into() })];
                }
                let Ok(mut out) = self.remove_account(&gone) else {
                    return vec![carnet(conn, Carnet::Refused { reason: "the server could not remove that account".into() })];
                };
                out.push(self.invite_list(conn, &account));
                out
            }
            Carnet::Repair { room } => {
                // Only a room the server keeps: a lease is held for each.
                if !fair_room(&room) || self.store.live(&account, &room).is_empty() {
                    return vec![carnet(conn, Carnet::Refused { reason: "that note isn't on the server to repair".into() })];
                }
                // Its own copy, as the room's base: asked of it now, whoever
                // else holds a lease, and whatever the base is.
                self.leases.insert((account, room.clone()), Lease { conn, until: self.now + LEASE, repair: true });
                vec![carnet(conn, Carnet::Compact { room })]
            }
            _ => self.deny(conn, "not now"),
        }
    }

    /// Why `account` can't sign in as `id`: not an email, or another
    /// account's.
    fn password_taken(&self, account: &[u8], id: &str) -> Option<String> {
        if !id.contains('@') || id.len() > 254 {
            return Some("That doesn't look like an email address".into());
        }
        match self.password_email(account) {
            Some(only) if only.is_empty() => return Some(NO_ACCOUNT_EMAIL.into()),
            Some(only) if only != id => return Some(password_email_is(&only)),
            _ => {}
        }
        match self.store.password_of(id) {
            Some((owner, _)) if owner != account => Some("That email signs in to another account already".into()),
            _ => None,
        }
    }

    /// The account gone, and everything it kept, its files too; its
    /// devices online told, and let go. What to send comes back.
    pub fn remove_account(&mut self, gone: &[u8]) -> Result<Vec<Out>, storage::Failed> {
        let removed = self.store.remove_account(gone);
        self.kept(removed)?;
        let mut ids: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(&c.state, State::Ready { account: a, .. } if *a == gone))
            .map(|(&id, _)| id)
            .collect();
        ids.sort_unstable();
        let mut out = Vec::new();
        for id in ids {
            out.extend(self.deny_as(id, ACCOUNT_REMOVED, Deny::AccountRemoved));
            out.extend(self.disconnect(id));
        }
        self.leases.retain(|(a, _), _| a != gone);
        self.last_write.retain(|(a, _), _| a != gone);
        self.notes.remove(gone);
        out.push(Out::Blob(Job::Gone { account: gone.to_vec() }));
        self.admission.removed(gone);
        Ok(out)
    }

    /// A note deleted forever: its room's records gone, and the room never
    /// taken again. The vault's room is every note's, never removed. The
    /// devices in it are told, and leave it.
    fn remove_room(&mut self, conn: ConnId, account: &[u8], me: &[u8], room: String) -> Vec<Out> {
        if room == carnet_sync::vault::ROOM || !fair_room(&room) {
            return vec![carnet(conn, Carnet::Refused { reason: "that can't be removed".into() })];
        }
        if !self.store.is_removed(account, &room) {
            let removed = self.store.remove_room(account, &room, self.now);
            if self.kept(removed).is_err() {
                return vec![carnet(conn, Carnet::Refused { reason: "the server could not remove that note".into() })];
            }
            let _ = self.store.log(account, Logged { at: self.now, device: me.to_vec(), room: room.clone(), what: "deleted forever".into() });
            let key = (account.to_vec(), room.clone());
            self.leases.remove(&key);
            self.last_write.remove(&key);
            self.notes.remove(account);
        }
        let mut ids: Vec<ConnId> = self.conns.keys().copied().collect();
        ids.sort_unstable();
        let mut out = Vec::new();
        for id in ids {
            let c = self.conns.get_mut(&id).expect("listed");
            let mine = matches!(&c.state, State::Ready { account: a, .. } if a == account);
            if mine && (c.rooms.remove(&room) || id == conn) {
                out.push(carnet(id, Carnet::RoomRemoved { room: room.clone() }));
            }
            if mine {
                self.new_room_gone(id, account, &room);
            }
        }
        out
    }

    /// A device's word on a base it opened: good, and it's verified; not,
    /// and it's rolled back if it can be, and said in the log either way.
    fn checked(&mut self, account: &[u8], me: &[u8], room: &str, vv: &[u8], ok: bool) -> Vec<Out> {
        let Some(base) = self.store.base(account, room) else { return Vec::new() };
        if base.verified || wire::decode_vv(vv).ok().as_ref() != Some(&base.vv) {
            return Vec::new();
        }
        if ok {
            let _ = self.store.verify_base(account, room, base.seq);
        } else {
            let back = self.store.roll_back(account, room).unwrap_or(false);
            self.notes.remove(account);
            let what = format!(
                "a snapshot by device {} did not hold what it said{}",
                hex(&base.by),
                if back { ": rolled back" } else { ", and could not be rolled back" }
            );
            let _ = self.store.log(account, Logged { at: self.now, device: me.to_vec(), room: room.into(), what });
        }
        Vec::new()
    }

    /// The account's rooms as the server keeps them, and its log.
    fn inspected(&self, conn: ConnId, account: &[u8]) -> Out {
        let (rooms, log) = self.inspect(account);
        carnet(conn, Carnet::Inspected { rooms, log })
    }

    /// How the server keeps an account's notes, room by room, and its log:
    /// what a device's inspector shows, and a host's helpdesk. Sizes and
    /// times only; every record is sealed.
    pub fn inspect(&self, account: &[u8]) -> (Vec<RoomRow>, Vec<LogRow>) {
        let rooms = self
            .store
            .rooms(account)
            .into_iter()
            .map(|room| {
                let base = self.store.base(account, &room);
                let spans = self.store.spans(account, &room);
                let shelf = self.store.shelf(account, &room);
                RoomRow {
                    held: storage::held(base.as_ref(), &spans),
                    live: spans.len() as u64,
                    live_bytes: spans.iter().map(|s| s.record.len() as u64).sum(),
                    base: base.map(|b| BaseRow { by: b.by, at: b.at, verified: b.verified, undoable: b.undoable, bytes: b.record.len() as u64, vv: b.vv }),
                    shelved: shelf.count,
                    shelved_bytes: shelf.bytes,
                    shelved_since: shelf.since,
                    room,
                }
            })
            .collect();
        let log = self.store.logged(account).into_iter().map(|l| LogRow { at: l.at, device: l.device, room: l.room, what: l.what }).collect();
        (rooms, log)
    }

    /// The account's devices online now, by id.
    /// `account`'s devices logged in now, each with the app it is on (one
    /// row a socket): what the helpdesk asks first.
    pub fn online_apps(&self, account: &[u8]) -> Vec<(Vec<u8>, Client)> {
        let mut rows: Vec<(Vec<u8>, Client)> = self
            .conns
            .values()
            .filter_map(|c| match &c.state {
                State::Ready { account: a, device } if a == account => Some((device.clone(), c.app.clone())),
                _ => None,
            })
            .collect();
        rows.sort();
        rows.dedup();
        rows
    }

    /// `account`'s limit changed (a plan set or ended, a suspension put on
    /// or lifted: docs/SYNC.md §11): each of its sockets logged in is told
    /// now, unasked, as a `UsageIs` (by way of the file worker, which
    /// counts the files). A device read only until now joins its rooms
    /// again, writable, and sends what waited; one read only from now on
    /// stops sending. The host calls it once the quota answers anew.
    pub fn limit_changed(&mut self, account: &[u8]) -> Vec<Out> {
        let mut conns: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(&c.state, State::Ready { account: a, .. } if a == account))
            .map(|(id, _)| *id)
            .collect();
        if conns.is_empty() {
            return Vec::new();
        }
        conns.sort_unstable();
        let limit = self.quota.limit(account);
        let notes = self.store.usage(account);
        conns.into_iter().map(|conn| Out::Blob(Job::Usage { conn, account: account.to_vec(), notes, limit: limit.clone() })).collect()
    }

    pub fn online(&self, account: &[u8]) -> Vec<Vec<u8>> {
        let mut ids: Vec<Vec<u8>> = self
            .conns
            .values()
            .filter_map(|c| match &c.state {
                State::Ready { account: a, device } if a == account => Some(device.clone()),
                _ => None,
            })
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Whether to ask `conn` for `room` as a snapshot now: the room is past
    /// its limits, nobody holds a lease on it, and nobody else is writing
    /// (this device is the account's only one online, or the room has
    /// taken nothing for a while), or the room is halfway to what it
    /// keeps uncompacted (a device writing on and on while another is
    /// online: an import, its vault a record a note). The device has had
    /// everything the server holds before this reaches it: one socket
    /// keeps its order.
    fn compact_now(&mut self, conn: ConnId, account: &[u8], room: &str, live: Live) -> Option<Out> {
        if live.records < self.compact_count as u64 && live.bytes < self.compact_bytes as u64 {
            return None;
        }
        let key = (account.to_vec(), room.to_string());
        if self.leases.get(&key).is_some_and(|l| l.until > self.now) {
            return None;
        }
        let alone = !self
            .conns
            .iter()
            .any(|(&id, c)| id != conn && matches!(&c.state, State::Ready { account: a, .. } if a == account));
        let quiet = self.last_write.get(&key).is_none_or(|&t| self.now >= t + QUIET);
        let filling = live.records >= self.room_records / 2 || live.bytes >= self.room_bytes / 2;
        if !alone && !quiet && !filling {
            return None;
        }
        self.leases.insert(key, Lease { conn, until: self.now + LEASE, repair: false });
        Some(carnet(conn, Carnet::Compact { room: room.to_string() }))
    }

    /// A snapshot from the device holding `room`'s lease: the room's new
    /// base, if it covers the base before it (a repair's needn't). Then the
    /// account's other devices in the room are sent it, so one opens it and
    /// says whether it holds what it says.
    #[allow(clippy::too_many_arguments)]
    fn take_snapshot(&mut self, conn: ConnId, account: &[u8], me: &[u8], room: &str, vv: Vec<(u64, u64)>, record: Vec<u8>, batch_id: BatchId) -> Vec<Out> {
        let key = (account.to_vec(), room.to_string());
        let Some(lease) = self.leases.get(&key).filter(|l| l.conn == conn && l.until > self.now) else {
            return self.refuse(conn, room, batch_id, UpdateStatusCode::PermissionDenied);
        };
        let repair = lease.repair;
        if !repair && self.store.base(account, room).is_some_and(|b| !covers(&vv, &b.vv)) {
            return self.refuse(conn, room, batch_id, UpdateStatusCode::InvalidUpdate);
        }
        let set = self.store.set_base(account, room, vv, record.clone(), me, self.now);
        if self.kept(set).is_err() {
            return vec![send(conn, ack(room, batch_id, UpdateStatusCode::Unknown))];
        }
        self.stats.snapshots += 1;
        self.leases.remove(&key);
        if let Some(n) = self.notes.get_mut(account) {
            *n += record.len() as u64;
        }
        if repair {
            let _ = self.store.log(account, Logged { at: self.now, device: me.to_vec(), room: room.into(), what: "replaced the server's copy with its own".into() });
        }
        let mut out = vec![send(conn, ack(room, batch_id, UpdateStatusCode::Ok))];
        // A repair was asked from the inspector: it shows the outcome.
        if repair {
            out.push(self.inspected(conn, account));
        }
        let mut ids: Vec<ConnId> = self.conns.keys().copied().filter(|&id| id != conn).collect();
        ids.sort_unstable();
        for id in ids {
            let c = &self.conns[&id];
            if matches!(&c.state, State::Ready { account: a, .. } if a == account) && c.rooms.contains(room) {
                out.extend(self.pass_on(id, room, std::slice::from_ref(&record)));
            }
        }
        out
    }

    /// Records of `room` passed on to `conn`, which joined it: now, or,
    /// while some of what it lacked of the room is made and not sent yet
    /// (a base's fragments), after that, so an update in fragments never
    /// meets another of its room.
    fn pass_on(&mut self, conn: ConnId, room: &str, records: &[Vec<u8>]) -> Vec<Out> {
        let messages = elo::batches(records.to_vec()).into_iter().flat_map(|batch| encoded(room, &batch));
        if let Some(b) = self.conns.get_mut(&conn).and_then(|c| c.backfills.front_mut()).filter(|b| b.room == room && !b.ready.is_empty()) {
            b.ready.extend(messages);
            return Vec::new();
        }
        messages.map(|m| Out::Send(conn, m)).collect()
    }

    /// The next part of what `conn` lacks of the rooms it joined, up to
    /// [`BACKFILL_PART`] bytes: the first room's, then the next's. A room
    /// all sent, the device may be asked to compact it (it has everything
    /// the server holds of it now: one socket keeps its order). More to
    /// come, it's said ([`Hub::take_more`]).
    fn backfill(&mut self, conn: ConnId, account: &[u8]) -> Vec<Out> {
        let part = self.backfill_part;
        let mut out = Vec::new();
        let mut sent = 0;
        loop {
            let Some(c) = self.conns.get_mut(&conn) else { return out };
            let Some(b) = c.backfills.front_mut() else { return out };
            if sent >= part {
                self.more.insert(conn);
                return out;
            }
            if b.base_due {
                b.base(self.store.base(account, &b.room).as_ref());
            }
            if let Some(m) = b.ready.pop_front() {
                sent += m.len();
                out.push(Out::Send(conn, m));
                continue;
            }
            let read = match &mut b.first {
                Some(first) => {
                    let n = first.iter().scan(0, |n, s| {
                        let before = *n;
                        *n += s.record.len();
                        Some(before)
                    });
                    let take = n.take_while(|&before| before < part).count().max(1).min(first.len());
                    first.drain(..take).collect()
                }
                None => self.store.spans_after(account, &b.room, b.after, part),
            };
            if read.is_empty() {
                let room = b.room.clone();
                c.backfills.pop_front();
                let live = self.store.live(account, &room);
                out.extend(self.compact_now(conn, account, &room, live));
                continue;
            }
            b.after = read.last().map_or(b.after, |s| s.seq);
            let records: Vec<Vec<u8>> = read.into_iter().filter(|s| s.end > b.has.get(&s.peer).copied().unwrap_or(0)).map(|s| s.record).collect();
            for batch in elo::batches_under(records, elo::MAX_BATCH.min(part)) {
                b.ready.extend(encoded(&b.room, &batch));
            }
        }
    }

    /// The invites waiting and every account, for the owner's device; for
    /// another's, only that it isn't the owner's.
    fn invite_list(&self, conn: ConnId, account: &[u8]) -> Out {
        if !self.store.is_owner(account) {
            return carnet(conn, Carnet::InviteList { owner: false, invites: Vec::new(), accounts: Vec::new() });
        }
        let invites = self.store.invites().into_iter().filter(|&(_, _, until)| until > self.now).map(|(hash, _, until)| InviteRow { hash, until }).collect();
        let accounts = self
            .store
            .account_list()
            .into_iter()
            .map(|(id, owner, made)| {
                let devices: Vec<_> = self.store.devices(&id).into_iter().filter(|(_, d)| !d.revoked).collect();
                AccountRow {
                    owner,
                    made,
                    seen: devices.iter().map(|(_, d)| d.seen).max().unwrap_or(0),
                    devices: devices.len() as u64,
                    you: id == account,
                    id,
                }
            })
            .collect();
        carnet(conn, Carnet::InviteList { owner: true, invites, accounts })
    }

    /// The account's devices not removed, for `me` to read.
    fn devices_frame(&self, conn: ConnId, account: &[u8], me: &[u8]) -> Out {
        // Who's logged in now: said with each, so a device online isn't
        // shown as last seen when its session began.
        let online = self.online(account);
        let devices = self
            .store
            .devices(account)
            .into_iter()
            .filter(|(_, d)| !d.revoked)
            .map(|(id, d)| DeviceRow { you: id == me, online: online.contains(&id), id, public: d.public, card: d.card, seen: d.seen })
            .collect();
        carnet(conn, Carnet::Devices {
            epoch: self.store.epoch(account),
            devices,
            recovery: self.store.recovery_card(account),
            password: self.store.password_card(account),
            passwords: self.passwords.is_some(),
            passkeys: self.store.passkeys(account).into_iter().map(|p| PasskeyRow { public: p.public, card: p.card, made: p.made }).collect(),
            // Its own email, to its own devices only.
            password_email: if self.passwords.is_some() { self.password_email(account).unwrap_or_default() } else { String::new() },
        })
    }

    /// A device taken out (§7). By itself: it leaves. By another: only with
    /// the next epoch's key sealed to every device kept and to each slot
    /// (the recovery code, the password, each passkey), no more and no
    /// fewer, so nothing is left without it; then the one taken out is shut
    /// out at once, and the others given their keys.
    #[allow(clippy::too_many_arguments)]
    fn revoke(
        &mut self,
        conn: ConnId,
        account: &[u8],
        me: &[u8],
        device: Vec<u8>,
        epoch: u32,
        keys: Vec<(Vec<u8>, Vec<u8>)>,
        cards: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Vec<Out> {
        let refuse = |reason: &str| vec![carnet(conn, Carnet::Refused { reason: reason.into() })];
        if device == me {
            if self.store.revoke_device(account, me).is_err() {
                return refuse("the server could not take this device out");
            }
            let mut out = self.shut_out(account, me, "this device left the account", Deny::Other);
            out.extend(self.lists_to_others(conn, account));
            return out;
        }
        let devices = self.store.devices(account);
        if !devices.iter().any(|(id, d)| *id == device && !d.revoked) {
            return refuse("that device is not in the account");
        }
        if self.store.epoch(account).checked_add(1) != Some(epoch) {
            return refuse("the account's keys changed meanwhile: try again");
        }
        let mut want: BTreeSet<Vec<u8>> = devices.iter().filter(|(id, d)| !d.revoked && *id != device).map(|(id, _)| id.clone()).collect();
        if !self.store.recovery_card(account).is_empty() {
            want.insert(Vec::new());
        }
        if !self.store.password_card(account).is_empty() {
            want.insert(password::SLOT.to_vec());
        }
        for p in self.store.passkeys(account) {
            want.insert(passkey::slot(&p.public));
        }
        let got: BTreeSet<Vec<u8>> = keys.iter().map(|(id, _)| id.clone()).collect();
        if got != want || keys.len() != want.len() {
            return refuse("a device came in or went meanwhile: try again");
        }
        if keys.iter().any(|(_, k)| k.len() > MAX_WRAPPED) || cards.iter().any(|(id, c)| c.len() > MAX_CARD || !want.contains(id)) {
            return refuse("a key or card past its size, or for no device kept");
        }
        let to_send: HashMap<Vec<u8>, Vec<u8>> = keys.iter().cloned().collect();
        if self.store.rotate(account, &device, epoch, keys, cards).is_err() {
            return refuse("the server could not keep the new keys");
        }
        let mut out = self.shut_out(account, &device, DEVICE_REMOVED, Deny::DeviceRemoved);
        let mut ids: Vec<ConnId> = self.conns.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            if let State::Ready { account: a, device: d } = &self.conns[&id].state
                && a == account
                && let Some(sealed) = to_send.get(d)
            {
                out.push(carnet(id, Carnet::Rekey { sealed: sealed.clone() }));
            }
        }
        out.extend(self.lists_to_others(ConnId::MAX, account));
        out
    }

    /// The account's list, fresh, to each of its devices online but
    /// `except`: a device came, went, or named itself.
    fn lists_to_others(&self, except: ConnId, account: &[u8]) -> Vec<Out> {
        let mut ids: Vec<ConnId> = self.conns.keys().copied().filter(|&id| id != except).collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| match &self.conns[&id].state {
                State::Ready { account: a, device } if a == account => Some(self.devices_frame(id, account, device)),
                _ => None,
            })
            .collect()
    }

    /// Every socket `device` has open, told why and closed.
    fn shut_out(&mut self, account: &[u8], device: &[u8], reason: &str, why: Deny) -> Vec<Out> {
        let theirs: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(&c.state, State::Ready { account: a, device: d } if a == account && d == device))
            .map(|(id, _)| *id)
            .collect();
        let mut out = Vec::new();
        for id in theirs {
            out.extend(self.deny_as(id, reason, why));
        }
        out
    }

    /// A nameplate no pairing open has: four digits, at random, so a
    /// stranger joining numbers it guesses (which spends a pairing, right
    /// code or not) seldom finds one open; the smallest free past them in
    /// the unlikely case that tries come up taken.
    fn free_nameplate(&self, rng: &mut impl Random) -> u32 {
        for _ in 0..32 {
            let [a, b, c, d]: [u8; 4] = rng.bytes();
            let n = NAMEPLATES.start() + u32::from_be_bytes([a, b, c, d]) % (NAMEPLATES.end() - NAMEPLATES.start() + 1);
            if !self.pairings.contains_key(&n) {
                return n;
            }
        }
        (NAMEPLATES.end() + 1..).find(|n| !self.pairings.contains_key(n)).expect("a free number")
    }

    /// A new device joins the pairing with this nameplate. One join per
    /// nameplate, right code or not: a guess spends it.
    fn join(&mut self, conn: ConnId, nonce: [u8; 32], nameplate: u32, pake: Vec<u8>) -> Vec<Out> {
        let Some(p) = self.pairings.get_mut(&nameplate).filter(|p| p.joiner.is_none()) else {
            return vec![carnet(conn, Carnet::PairClose { reason: "no pairing open with that number".into() })];
        };
        p.joiner = Some(conn);
        let opener = p.opener;
        if let Some(c) = self.conns.get_mut(&conn) {
            c.state = State::Joining { nonce };
        }
        vec![carnet(opener, Carnet::PairJoined { pake })]
    }

    /// A pairing's message, passed to the other side as it is.
    fn relay(&mut self, conn: ConnId, c: Carnet) -> Vec<Out> {
        let other = self.pairings.values().find_map(|p| match p.joiner {
            Some(j) if j == conn => Some(p.opener),
            Some(j) if p.opener == conn => Some(j),
            _ => None,
        });
        other.map_or_else(Vec::new, |o| vec![carnet(o, c)])
    }

    /// The pairing `conn` is in, over: the other side told why, and a
    /// joiner not let in is back to its challenge.
    fn end_pairing_of(&mut self, conn: ConnId, reason: &str) -> Vec<Out> {
        let Some(n) = self.pairings.iter().find(|(_, p)| p.opener == conn || p.joiner == Some(conn)).map(|(n, _)| *n) else {
            return Vec::new();
        };
        let p = self.pairings.remove(&n).expect("found");
        let mut out = Vec::new();
        for side in [Some(p.opener), p.joiner].into_iter().flatten().filter(|&s| s != conn) {
            if self.conns.contains_key(&side) {
                out.push(carnet(side, Carnet::PairClose { reason: reason.into() }));
            }
        }
        if let Some(j) = p.joiner
            && let Some(c) = self.conns.get_mut(&j)
            && let State::Joining { nonce } = c.state
        {
            c.state = State::Challenged(nonce);
        }
        out
    }

    /// The recovery key signs a new device in: let it in, hand back the
    /// vault key wrapped, and challenge it again, for its own login.
    #[allow(clippy::too_many_arguments)]
    fn recover(
        &mut self,
        conn: ConnId,
        nonce: [u8; 32],
        public: [u8; 32],
        signature: [u8; 64],
        device: Vec<u8>,
        device_public: [u8; 32],
        rng: &mut impl Random,
    ) -> Vec<Out> {
        // The recovery words' slot, the password's, or a passkey's.
        let found = self
            .store
            .recovery(&public)
            .map(|(a, w)| (a, w, Vec::new()))
            .or_else(|| self.store.password_slot(&public).map(|(a, w)| (a, w, password::SLOT.to_vec())))
            .or_else(|| self.store.passkey_slot(&public).map(|(a, w)| (a, w, passkey::slot(&public))));
        let Some((account, wrapped, slot)) = found else {
            return self.deny(conn, NO_SLOT);
        };
        let m = recovery_message(&self.origin, &public, &device, &device_public, &nonce);
        if device.is_empty() || device.len() > MAX_DEVICE_ID || !verify(&public, &m, &signature) {
            return self.deny(conn, "not that recovery code's signature");
        }
        if self.device_added(&account, &device, device_public).is_err() {
            return self.deny(conn, "the server could not keep the new device");
        }
        self.stats.recoveries += 1;
        let fresh: [u8; 32] = rng.bytes();
        if let Some(c) = self.conns.get_mut(&conn) {
            c.state = State::Challenged(fresh);
        }
        let rekeys = self.store.rekeys(&account, &slot);
        vec![carnet(conn, Carnet::Recovered { account, wrapped, rekeys }), self.challenge(conn, fresh)]
    }

    /// A batch not kept, said to the device that sent it.
    fn refuse(&mut self, conn: ConnId, room: &str, batch_id: BatchId, status: UpdateStatusCode) -> Vec<Out> {
        self.stats.batches_refused += 1;
        vec![send(conn, ack(room, batch_id, status))]
    }

    fn deny(&mut self, conn: ConnId, reason: &str) -> Vec<Out> {
        self.deny_as(conn, reason, Deny::Other)
    }

    /// Turned away for `why`, which the app acts on whatever `reason`
    /// says.
    fn deny_as(&mut self, conn: ConnId, reason: &str, why: Deny) -> Vec<Out> {
        self.stats.denied += 1;
        let mut out = self.end_pairing_of(conn, "the other device was turned away");
        self.remove_conn(conn);
        out.push(carnet(conn, Carnet::Denied { reason: reason.into(), why }));
        out.push(Out::Close(conn));
        out
    }

    fn room_message(&mut self, conn: ConnId, account: &[u8], me: &[u8], m: ProtocolMessage) -> Vec<Out> {
        match m {
            // A note deleted forever: not taken again, whoever had it.
            ProtocolMessage::JoinRequest { crdt: CrdtType::Elo, room_id, .. } if self.store.is_removed(account, &room_id) => {
                vec![carnet(conn, Carnet::RoomRemoved { room: room_id })]
            }
            // A room id no device makes: not joined.
            ProtocolMessage::JoinRequest { crdt: CrdtType::Elo, room_id, .. } if !fair_room(&room_id) => {
                vec![send(conn, ProtocolMessage::JoinError {
                    crdt: CrdtType::Elo,
                    room_id,
                    code: loro_protocol::JoinErrorCode::Unknown,
                    message: format!("a room id is 1 to {MAX_ROOM_ID} bytes"),
                    receiver_version: None,
                    app_code: None,
                })]
            }
            // Joined: what the server holds of the room said, then what the
            // device lacks of it (the base, if it isn't past it, and every
            // delta its version vector doesn't cover), a part at a time,
            // after any room joined before whose records are still on their
            // way. Once it's all gone, it may be asked to compact. A room
            // that holds nothing is joined only while the account's
            // sockets haven't too many such joins out ([`NEW_ROOMS`]).
            ProtocolMessage::JoinRequest { crdt: CrdtType::Elo, room_id, version, .. } => {
                let base = self.store.base(account, &room_id);
                let spans = self.store.spans(account, &room_id);
                let new = base.is_none() && spans.is_empty();
                if new && !self.may_join_new(conn, account, &room_id) {
                    return vec![send(conn, ProtocolMessage::JoinError {
                        crdt: CrdtType::Elo,
                        room_id,
                        code: loro_protocol::JoinErrorCode::Unknown,
                        message: "too many new notes joined at once: they're joined again as those before them are written".into(),
                        receiver_version: None,
                        app_code: None,
                    })];
                }
                if new {
                    if self.conns.get_mut(&conn).is_some_and(|c| c.new_rooms.insert(room_id.clone())) {
                        self.new_joins.entry(account.to_vec()).or_default().joins += 1;
                    }
                } else {
                    self.new_room_gone(conn, account, &room_id);
                }
                let has = wire::decode_vv(&version).unwrap_or_default();
                let held = storage::held(base.as_ref(), &spans);
                let mut b = Backfill { room: room_id.clone(), has: has.iter().copied().collect(), first: None, base_due: true, after: 0, ready: VecDeque::new() };
                let Some(c) = self.conns.get_mut(&conn) else { return Vec::new() };
                c.rooms.insert(room_id.clone());
                // Joined again: what was on its way of the room starts over.
                c.backfills.retain(|b| b.room != room_id);
                let first = c.backfills.is_empty();
                if first {
                    b.base(base.as_ref());
                    b.first = Some(spans.into());
                }
                c.backfills.push_back(b);
                let mut out = vec![send(conn, ProtocolMessage::JoinResponseOk {
                    crdt: CrdtType::Elo,
                    room_id,
                    permission: if self.quota.limit(account).writable { Permission::Write } else { Permission::Read },
                    version: wire::encode_vv(&held),
                    extra: None,
                })];
                if first {
                    out.extend(self.backfill(conn, account));
                }
                out
            }
            ProtocolMessage::Leave { room_id, .. } => {
                self.new_room_gone(conn, account, &room_id);
                if let Some(c) = self.conns.get_mut(&conn) {
                    c.rooms.remove(&room_id);
                    c.backfills.retain(|b| b.room != room_id);
                }
                Vec::new()
            }
            // Only a snapshot comes in fragments (one past a message): from
            // the lease holder, of a room it joined, no bigger than a
            // snapshot may be, one at a time. Nothing is set aside for it
            // unless it's all of that; then it's taken as if it came whole.
            ProtocolMessage::DocUpdateFragmentHeader { crdt: CrdtType::Elo, room_id, batch_id, fragment_count, total_size_bytes } => {
                let joined = self.conns.get(&conn).is_some_and(|c| c.rooms.contains(&room_id));
                let leased = self.leases.get(&(account.to_vec(), room_id.clone())).is_some_and(|l| l.conn == conn && l.until > self.now);
                if !joined || !leased || !self.quota.limit(account).writable {
                    return self.refuse(conn, &room_id, batch_id, UpdateStatusCode::PermissionDenied);
                }
                let Some(p) = Pieces::start(&room_id, batch_id, fragment_count, total_size_bytes) else {
                    return self.refuse(conn, &room_id, batch_id, UpdateStatusCode::PayloadTooLarge);
                };
                if let Some(c) = self.conns.get_mut(&conn) {
                    c.incoming = Some(p);
                }
                Vec::new()
            }
            ProtocolMessage::DocUpdateFragment { crdt: CrdtType::Elo, room_id, batch_id, index, fragment } => {
                let Some(p) = self.conns.get_mut(&conn).and_then(|c| c.incoming.as_mut()) else { return Vec::new() };
                match p.add(&room_id, batch_id, index, &fragment) {
                    Ok(None) => Vec::new(),
                    Ok(Some(update)) => {
                        if let Some(c) = self.conns.get_mut(&conn) {
                            c.incoming = None;
                        }
                        self.room_message(conn, account, me, ProtocolMessage::DocUpdate { crdt: CrdtType::Elo, room_id, updates: vec![update], batch_id })
                    }
                    Err(_) => {
                        let batch_id = p.batch_id;
                        let room = p.room.clone();
                        if let Some(c) = self.conns.get_mut(&conn) {
                            c.incoming = None;
                        }
                        self.refuse(conn, &room, batch_id, UpdateStatusCode::InvalidUpdate)
                    }
                }
            }
            ProtocolMessage::DocUpdate { crdt: CrdtType::Elo, room_id, updates, batch_id } => {
                let joined = self.conns.get(&conn).is_some_and(|c| c.rooms.contains(&room_id));
                if !joined || !self.quota.limit(account).writable {
                    return self.refuse(conn, &room_id, batch_id, UpdateStatusCode::PermissionDenied);
                }
                // Every record read before any is kept: a batch is taken
                // whole or not at all.
                let mut deltas = Vec::new();
                for container in &updates {
                    let Ok(records) = elo::records(container) else {
                        return self.refuse(conn, &room_id, batch_id, UpdateStatusCode::InvalidUpdate);
                    };
                    for r in records {
                        match elo::header(r) {
                            Ok(Header::Snapshot { .. }) if r.len() > elo::MAX_SNAPSHOT => {
                                return self.refuse(conn, &room_id, batch_id, UpdateStatusCode::PayloadTooLarge);
                            }
                            Ok(Header::Delta { .. }) | Err(_) if r.len() > MAX_RECORD => {
                                return self.refuse(conn, &room_id, batch_id, UpdateStatusCode::PayloadTooLarge);
                            }
                            Ok(Header::Delta { peer, start, end, .. }) if start < end => deltas.push((peer, start, end, r.to_vec())),
                            // A snapshot comes alone, from the lease holder.
                            Ok(Header::Snapshot { vv, .. }) if updates.len() == 1 && deltas.is_empty() => {
                                let one = elo::records(&updates[0]).map(|rs| rs.len() == 1).unwrap_or(false);
                                if !one {
                                    return self.refuse(conn, &room_id, batch_id, UpdateStatusCode::InvalidUpdate);
                                }
                                return self.take_snapshot(conn, account, me, &room_id, vv, r.to_vec(), batch_id);
                            }
                            _ => return self.refuse(conn, &room_id, batch_id, UpdateStatusCode::InvalidUpdate),
                        }
                    }
                }
                // Past what the room or the account keeps: not taken, the
                // device told once, and asked to compact if it may.
                if let (Some(why), live) = self.room_full(account, &room_id) {
                    let mut out = self.refuse(conn, &room_id, batch_id, UpdateStatusCode::PayloadTooLarge);
                    if let Some(c) = self.conns.get_mut(&conn)
                        && !std::mem::replace(&mut c.told_full, true)
                    {
                        out.push(carnet(conn, Carnet::Refused { reason: why.into() }));
                    }
                    out.extend(self.compact_now(conn, account, &room_id, live));
                    return out;
                }
                let mut kept = Vec::new();
                let mut newest = None;
                let mut failed = false;
                for (peer, start, end, record) in deltas {
                    let put = self.store.put(account, &room_id, peer, start, end, record.clone());
                    match self.kept(put) {
                        Ok(Some(seq)) => {
                            self.stats.records += 1;
                            self.stats.record_bytes += record.len() as u64;
                            if let Some(n) = self.notes.get_mut(account) {
                                *n += record.len() as u64;
                            }
                            kept.push(record);
                            newest = Some(seq);
                        }
                        Ok(None) => {}
                        Err(_) => failed = true,
                    }
                }
                // Not kept is never acknowledged as kept: the device joins
                // again and sends what is still missing.
                let status = if failed { UpdateStatusCode::Unknown } else { UpdateStatusCode::Ok };
                let mut out = vec![send(conn, ack(&room_id, batch_id, status))];
                if let Some(seq) = newest {
                    self.new_room_gone(conn, account, &room_id);
                    self.last_write.insert((account.to_vec(), room_id.clone()), self.now);
                    out.extend(self.tell_others(conn, account, &room_id, &kept, seq));
                    // A device writing on alone: the room may be due now,
                    // as counted where the records are, none read.
                    let live = self.store.live(account, &room_id);
                    out.extend(self.compact_now(conn, account, &room_id, live));
                }
                out
            }
            ProtocolMessage::JoinRequest { room_id, crdt, .. } => vec![send(conn, ProtocolMessage::JoinError {
                crdt,
                room_id,
                code: loro_protocol::JoinErrorCode::Unknown,
                message: "Carnet rooms are %ELO".into(),
                receiver_version: None,
                app_code: None,
            })],
            _ => Vec::new(),
        }
    }

    /// The account's other devices: the records, to those in the room;
    /// that the room changed, to the rest.
    fn tell_others(&mut self, from: ConnId, account: &[u8], room: &str, records: &[Vec<u8>], seq: u64) -> Vec<Out> {
        let mut out = Vec::new();
        let mut ids: Vec<ConnId> = self.conns.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let c = &self.conns[&id];
            let mine = matches!(&c.state, State::Ready { account: a, .. } if a == account);
            if id == from || !mine {
                continue;
            }
            if c.rooms.contains(room) {
                out.extend(self.pass_on(id, room, records));
            } else {
                out.push(Out::Send(id, wire::encode(&Frame::Carnet(Carnet::Changed { rooms: vec![(room.to_string(), seq)] }))));
            }
        }
        out
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A room id a device makes: 1 to [`MAX_ROOM_ID`] bytes.
fn fair_room(room: &str) -> bool {
    !room.is_empty() && room.len() <= MAX_ROOM_ID
}

fn carnet(conn: ConnId, c: Carnet) -> Out {
    Out::Send(conn, wire::encode(&Frame::Carnet(c)))
}

fn send(conn: ConnId, m: ProtocolMessage) -> Out {
    Out::Send(conn, wire::encode(&Frame::Room(m)))
}

/// Records for `room` as one update, encoded: one message, or, past what
/// a message holds (a big base), its fragments.
fn encoded(room: &str, records: &[Vec<u8>]) -> Vec<Vec<u8>> {
    elo::messages(room, elo::container(records), BatchId([0; 8])).into_iter().map(|m| wire::encode(&Frame::Room(m))).collect()
}

fn ack(room: &str, ref_id: BatchId, status: UpdateStatusCode) -> ProtocolMessage {
    ProtocolMessage::Ack { crdt: CrdtType::Elo, room_id: room.to_string(), ref_id, status }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever apps claim to be, the counts stay a short list: past
    /// [`APPS`] of them, a new one is "other"; one counted already stays.
    #[test]
    fn apps_and_versions_are_counted_apart_up_to_a_limit() {
        let mut seen: BTreeMap<(String, String), u64> = BTreeMap::new();
        let app = |v: usize| Client { protocol: 1, app: "desktop-macos".into(), version: format!("0.{v}") };
        for v in 0..APPS + 5 {
            *seen.entry(app_key(&seen, &app(v))).or_default() += 1;
        }
        assert_eq!(seen.len(), APPS + 1);
        assert_eq!(seen[&("other".to_string(), String::new())], 5);
        assert_eq!(app_key(&seen, &app(3)), ("desktop-macos".into(), "0.3".into()));
        assert_eq!(app_key(&BTreeMap::<_, u64>::new(), &Client::default()), ("unknown".into(), String::new()));
    }
}
