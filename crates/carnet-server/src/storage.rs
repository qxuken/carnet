//! What the server keeps: each account's devices, and each room's sealed
//! records, indexed by their plaintext headers (docs/SYNC.md §6). A trait,
//! so the hub runs on memory in the simulation and on SQLite in the
//! binary.

use std::collections::HashMap;

/// An enrolled device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub public: [u8; 32],
    pub revoked: bool,
    /// Its card, sealed (§7): what its owner calls it, and its box key.
    /// Empty until it sends one.
    pub card: Vec<u8>,
    /// Its last login, in seconds since 1970; 0 for never.
    pub seen: u64,
}

/// A delta record as stored: one peer's operations `start..end`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub peer: u64,
    pub start: u64,
    pub end: u64,
    /// When it came, in the account's order.
    pub seq: u64,
    pub record: Vec<u8>,
}

/// A room's base: a snapshot of its doc, history and all, sealed, that
/// stands for every record it covers (§6, compaction).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub seq: u64,
    /// What it holds, by peer, as its header says.
    pub vv: Vec<(u64, u64)>,
    pub record: Vec<u8>,
    /// The device that wrote it, and when (seconds since 1970).
    pub by: Vec<u8>,
    pub at: u64,
    /// Another device opened it and found it holds what it says.
    pub verified: bool,
    /// Everything it set aside is still kept: it can be rolled back.
    pub undoable: bool,
}

/// What a room has set aside: records a base stands for, kept for a while
/// so the base can be rolled back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shelf {
    pub count: u64,
    pub bytes: u64,
    /// When the oldest was set aside; 0 for none.
    pub since: u64,
}

/// What a room holds past its base, counted without reading a record:
/// whether it's due for compaction, or past what the server takes
/// (docs/SYNC.md §6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Live {
    /// Live records, and their bytes.
    pub records: u64,
    pub bytes: u64,
    /// It has a base.
    pub based: bool,
}

impl Live {
    /// Nothing kept: a room never written to, or removed.
    pub fn is_empty(&self) -> bool {
        self.records == 0 && !self.based
    }
}

/// Something done to an account's rooms that every device should be able
/// to see: a rollback, a repair, a snapshot that failed its check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Logged {
    pub at: u64,
    pub device: Vec<u8>,
    pub room: String,
    pub what: String,
}

/// Spans, as many as come to `bytes`, and at least one if there's any
/// ([`Storage::spans_after`]).
pub fn up_to(spans: impl IntoIterator<Item = Span>, bytes: usize) -> Vec<Span> {
    let mut out = Vec::new();
    let mut n = 0;
    for s in spans {
        if !out.is_empty() && n >= bytes {
            break;
        }
        n += s.record.len();
        out.push(s);
    }
    out
}

/// Whether `has` holds everything `vv` names.
pub fn covers(has: &[(u64, u64)], vv: &[(u64, u64)]) -> bool {
    let has: HashMap<u64, u64> = has.iter().copied().collect();
    vv.iter().all(|(p, c)| has.get(p).copied().unwrap_or(0) >= *c)
}

/// Storage that could not do what was asked (a disk full, a database
/// locked): the device is told, and sends it again later.
#[derive(Debug, PartialEq, Eq)]
pub struct Failed(pub String);

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "storage failed: {}", self.0)
    }
}

impl std::error::Error for Failed {}

/// Why a store refuses a device id the account has had.
pub const DEVICE_TAKEN: &str = "that device id is the account's already";
/// Why it refuses a recovery key another account's code has.
pub const RECOVERY_TAKEN: &str = "that recovery key is another account's";

pub trait Storage {
    fn device(&self, account: &[u8], device: &[u8]) -> Option<Device>;
    /// A new device in the account. An id the account has had already is
    /// refused (removed or not: a removed device stays removed), unless
    /// it's the same device again, its key the same and not removed.
    fn add_device(&mut self, account: &[u8], device: &[u8], public: [u8; 32]) -> Result<(), Failed>;
    fn revoke_device(&mut self, account: &[u8], device: &[u8]) -> Result<(), Failed>;
    /// Every device the account has had, removed ones too.
    fn devices(&self, account: &[u8]) -> Vec<(Vec<u8>, Device)>;
    fn set_card(&mut self, account: &[u8], device: &[u8], card: Vec<u8>) -> Result<(), Failed>;
    fn set_seen(&mut self, account: &[u8], device: &[u8], now: u64) -> Result<(), Failed>;

    /// The account's key epoch: 0, and one more each time a device is
    /// removed by another (§7).
    fn epoch(&self, account: &[u8]) -> u32;
    /// A device removed by another, all at once: it is revoked, the account
    /// is at `epoch`, each id in `keys` (an empty one the recovery code's,
    /// `password::SLOT` the password's, a `passkey::slot` a passkey's) has
    /// that epoch's key sealed to it, and each id in `cards` its card
    /// sealed anew.
    fn rotate(
        &mut self,
        account: &[u8],
        revoked: &[u8],
        epoch: u32,
        keys: Vec<(Vec<u8>, Vec<u8>)>,
        cards: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<(), Failed>;
    /// The keys sealed to `device` (empty: the recovery code), oldest
    /// epoch first.
    fn rekeys(&self, account: &[u8], device: &[u8]) -> Vec<Vec<u8>>;

    /// A delta record for `room`. Kept, with the account's next `seq`,
    /// unless a record of the same peer already covers its span; records
    /// it covers go. `None` when it was not kept (nothing new in it).
    /// Only a record kept is ever acknowledged as kept.
    fn put(&mut self, account: &[u8], room: &str, peer: u64, start: u64, end: u64, record: Vec<u8>) -> Result<Option<u64>, Failed>;

    /// `room`'s records, in the order they came: the live ones, not those
    /// a base has set aside.
    fn spans(&self, account: &[u8], room: &str) -> Vec<Span>;

    /// `room`'s live records that came after `seq`, in the order they
    /// came, as many as come to `bytes` (and at least one, if there's
    /// any): a room's records sent a part at a time (§6).
    fn spans_after(&self, account: &[u8], room: &str, seq: u64, bytes: usize) -> Vec<Span> {
        up_to(self.spans(account, room).into_iter().filter(|s| s.seq > seq), bytes)
    }

    /// How many live records `room` has and their bytes, and whether it
    /// has a base: what [`spans`](Storage::spans) and
    /// [`base`](Storage::base) would say, kept counted as they change, so
    /// asking costs the same however many the room has (the hub asks
    /// before and after every write). A store's own does; this one, for
    /// a store that keeps none, counts them.
    fn live(&self, account: &[u8], room: &str) -> Live {
        let spans = self.spans(account, room);
        Live { records: spans.len() as u64, bytes: spans.iter().map(|s| s.record.len() as u64).sum(), based: self.base(account, room).is_some() }
    }

    /// How many rooms have anything kept: [`rooms`](Storage::rooms),
    /// counted (a store's own keeps the count, as [`live`](Storage::live)).
    fn room_count(&self, account: &[u8]) -> u64 {
        self.rooms(account).len() as u64
    }

    /// The room's base, if it has one.
    fn base(&self, account: &[u8], room: &str) -> Option<Base>;
    /// A snapshot as the room's base, from `by`, at the account's next
    /// `seq`: the base before it, and every live record its `vv` covers,
    /// set aside (not gone). Its `seq` comes back.
    fn set_base(&mut self, account: &[u8], room: &str, vv: Vec<(u64, u64)>, record: Vec<u8>, by: &[u8], now: u64) -> Result<u64, Failed>;
    /// The base with this `seq`, if it is still the room's, checked good.
    fn verify_base(&mut self, account: &[u8], room: &str, seq: u64) -> Result<(), Failed>;
    /// The room's base dropped, and what it set aside back as it was.
    /// False, and nothing changed, if there is none or some of what it set
    /// aside is gone.
    fn roll_back(&mut self, account: &[u8], room: &str) -> Result<bool, Failed>;
    fn shelf(&self, account: &[u8], room: &str) -> Shelf;
    /// The rooms with anything kept: live records or a base.
    fn rooms(&self, account: &[u8]) -> Vec<String>;
    /// `room`'s note deleted forever: everything kept of it gone at once,
    /// and the room never taken again ([`is_removed`](Storage::is_removed)).
    fn remove_room(&mut self, account: &[u8], room: &str, now: u64) -> Result<(), Failed>;
    fn is_removed(&self, account: &[u8], room: &str) -> bool;
    /// The bytes kept for the account's notes: every record and base, set
    /// aside or not (they take the room until purged).
    fn usage(&self, account: &[u8]) -> u64;
    /// Set aside before `before`, in every account: gone for good, and the
    /// bases that set it aside can't be rolled back from then on.
    fn purge(&mut self, before: u64) -> Result<(), Failed>;
    fn log(&mut self, account: &[u8], entry: Logged) -> Result<(), Failed>;
    /// The newest first, at most 50.
    fn logged(&self, account: &[u8]) -> Vec<Logged>;

    /// Rooms with a record past `seq`, each with its newest.
    fn changed_since(&self, account: &[u8], seq: u64) -> Vec<(String, u64)>;

    /// The account's newest `seq`.
    fn seq(&self, account: &[u8]) -> u64;

    /// How many accounts there are: none, and the server prints an
    /// invite for the first (§6).
    fn accounts(&self) -> usize;
    fn add_account(&mut self, account: &[u8], owner: bool) -> Result<(), Failed>;
    /// An invite, by its code's hash, good until `expires` (seconds since
    /// 1970); `owner` for the first account's.
    fn add_invite(&mut self, hash: [u8; 32], owner: bool, expires: u64) -> Result<(), Failed>;
    /// An invite used: gone, whether it was good or not; `Some(owner)` if
    /// it was there and not past its time.
    fn take_invite(&mut self, hash: [u8; 32], now: u64) -> Result<Option<bool>, Failed>;
    /// An invite not used yet, by hash, run out or not: whether it's for
    /// the owner's account, and when it stops being good. What an
    /// enrollment that fails says by it.
    fn find_invite(&self, hash: [u8; 32]) -> Option<(bool, u64)> {
        self.invites().into_iter().find(|&(h, _, _)| h == hash).map(|(_, owner, expires)| (owner, expires))
    }
    /// Invites not used yet, by hash: whether each is for the owner's
    /// account, and when it stops being good (past ones among them: the
    /// caller knows the time).
    fn invites(&self) -> Vec<([u8; 32], bool, u64)>;
    /// An unused invite taken back: whether there was one.
    fn drop_invite(&mut self, hash: [u8; 32]) -> Result<bool, Failed>;
    /// Whether `account` is the owner's: the first, made by the invite the
    /// server printed. Its devices make the invites.
    fn is_owner(&self, account: &[u8]) -> bool;
    /// Every account: its id, whether it's the owner's, when it was made.
    fn account_list(&self) -> Vec<(Vec<u8>, bool, u64)>;
    /// Whether `account` is here: what a picture's put and the sweep of
    /// pictures no account owns ask. A store that can't say answers yes,
    /// so nothing is let go for a failure. (Every account has a device,
    /// unless a store says better.)
    fn has_account(&self, account: &[u8]) -> bool {
        !self.devices(account).is_empty()
    }
    /// An account gone, and everything it held with it: its devices, its
    /// records, its pictures, its recovery code.
    fn remove_account(&mut self, account: &[u8]) -> Result<(), Failed>;

    /// The account's recovery code (§7): its public key, the key ring
    /// wrapped under it, and its card. One per account: a new one replaces
    /// the last, and the keys sealed to the last go with it. A key another
    /// account's code has is refused, never taken from it.
    fn set_recovery(&mut self, account: &[u8], public: [u8; 32], wrapped: Vec<u8>, card: Vec<u8>) -> Result<(), Failed>;
    /// The account a recovery key is for, and its wrapped key ring.
    fn recovery(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)>;
    /// The recovery code's card; empty if there is no code.
    fn recovery_card(&self, account: &[u8]) -> Vec<u8>;

    /// The account's password (§7): what it signs in as, and its slot.
    /// Replaces any it had (and what was sealed to it); the caller has
    /// made sure no other account signs in as that `id`.
    fn set_password(&mut self, account: &[u8], slot: PasswordSlot) -> Result<(), Failed>;
    fn drop_password(&mut self, account: &[u8]) -> Result<(), Failed>;
    /// The account that signs in as `id`, and OPAQUE's record for it.
    fn password_of(&self, id: &str) -> Option<(Vec<u8>, Vec<u8>)>;
    /// The account whose password slot signs with `public`, and the key
    /// ring wrapped under it.
    fn password_slot(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)>;
    /// The password's card; empty if there is none.
    fn password_card(&self, account: &[u8]) -> Vec<u8>;

    /// A passkey for the account (§7): a slot, as the recovery words', named
    /// by its signing key. The same key again replaces it, and what was
    /// sealed to it goes; the caller has made sure no other account, nor
    /// any recovery code or password, has that key.
    fn add_passkey(&mut self, account: &[u8], slot: PasskeySlot) -> Result<(), Failed>;
    /// The passkey with this key gone, and what was sealed to it: whether
    /// there was one.
    fn drop_passkey(&mut self, account: &[u8], public: &[u8; 32]) -> Result<bool, Failed>;
    /// The account's passkeys, the oldest first.
    fn passkeys(&self, account: &[u8]) -> Vec<PasskeySlot>;
    /// The account whose passkey slot signs with `public`, and the key ring
    /// wrapped under it.
    fn passkey_slot(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)>;
}

/// A boxed storage is one: the simulation runs one hub on memory or
/// SQLite alike.
impl<T: Storage + ?Sized> Storage for Box<T> {
    fn device(&self, account: &[u8], device: &[u8]) -> Option<Device> {
        (**self).device(account, device)
    }
    fn add_device(&mut self, account: &[u8], device: &[u8], public: [u8; 32]) -> Result<(), Failed> {
        (**self).add_device(account, device, public)
    }
    fn revoke_device(&mut self, account: &[u8], device: &[u8]) -> Result<(), Failed> {
        (**self).revoke_device(account, device)
    }
    fn devices(&self, account: &[u8]) -> Vec<(Vec<u8>, Device)> {
        (**self).devices(account)
    }
    fn set_card(&mut self, account: &[u8], device: &[u8], card: Vec<u8>) -> Result<(), Failed> {
        (**self).set_card(account, device, card)
    }
    fn set_seen(&mut self, account: &[u8], device: &[u8], now: u64) -> Result<(), Failed> {
        (**self).set_seen(account, device, now)
    }
    fn epoch(&self, account: &[u8]) -> u32 {
        (**self).epoch(account)
    }
    fn rotate(
        &mut self,
        account: &[u8],
        revoked: &[u8],
        epoch: u32,
        keys: Vec<(Vec<u8>, Vec<u8>)>,
        cards: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<(), Failed> {
        (**self).rotate(account, revoked, epoch, keys, cards)
    }
    fn rekeys(&self, account: &[u8], device: &[u8]) -> Vec<Vec<u8>> {
        (**self).rekeys(account, device)
    }
    fn put(&mut self, account: &[u8], room: &str, peer: u64, start: u64, end: u64, record: Vec<u8>) -> Result<Option<u64>, Failed> {
        (**self).put(account, room, peer, start, end, record)
    }
    fn spans(&self, account: &[u8], room: &str) -> Vec<Span> {
        (**self).spans(account, room)
    }
    fn spans_after(&self, account: &[u8], room: &str, seq: u64, bytes: usize) -> Vec<Span> {
        (**self).spans_after(account, room, seq, bytes)
    }
    fn live(&self, account: &[u8], room: &str) -> Live {
        (**self).live(account, room)
    }
    fn room_count(&self, account: &[u8]) -> u64 {
        (**self).room_count(account)
    }
    fn base(&self, account: &[u8], room: &str) -> Option<Base> {
        (**self).base(account, room)
    }
    fn set_base(&mut self, account: &[u8], room: &str, vv: Vec<(u64, u64)>, record: Vec<u8>, by: &[u8], now: u64) -> Result<u64, Failed> {
        (**self).set_base(account, room, vv, record, by, now)
    }
    fn verify_base(&mut self, account: &[u8], room: &str, seq: u64) -> Result<(), Failed> {
        (**self).verify_base(account, room, seq)
    }
    fn roll_back(&mut self, account: &[u8], room: &str) -> Result<bool, Failed> {
        (**self).roll_back(account, room)
    }
    fn shelf(&self, account: &[u8], room: &str) -> Shelf {
        (**self).shelf(account, room)
    }
    fn rooms(&self, account: &[u8]) -> Vec<String> {
        (**self).rooms(account)
    }
    fn remove_room(&mut self, account: &[u8], room: &str, now: u64) -> Result<(), Failed> {
        (**self).remove_room(account, room, now)
    }
    fn is_removed(&self, account: &[u8], room: &str) -> bool {
        (**self).is_removed(account, room)
    }
    fn usage(&self, account: &[u8]) -> u64 {
        (**self).usage(account)
    }
    fn purge(&mut self, before: u64) -> Result<(), Failed> {
        (**self).purge(before)
    }
    fn log(&mut self, account: &[u8], entry: Logged) -> Result<(), Failed> {
        (**self).log(account, entry)
    }
    fn logged(&self, account: &[u8]) -> Vec<Logged> {
        (**self).logged(account)
    }
    fn changed_since(&self, account: &[u8], seq: u64) -> Vec<(String, u64)> {
        (**self).changed_since(account, seq)
    }
    fn seq(&self, account: &[u8]) -> u64 {
        (**self).seq(account)
    }
    fn accounts(&self) -> usize {
        (**self).accounts()
    }
    fn add_account(&mut self, account: &[u8], owner: bool) -> Result<(), Failed> {
        (**self).add_account(account, owner)
    }
    fn add_invite(&mut self, hash: [u8; 32], owner: bool, expires: u64) -> Result<(), Failed> {
        (**self).add_invite(hash, owner, expires)
    }
    fn take_invite(&mut self, hash: [u8; 32], now: u64) -> Result<Option<bool>, Failed> {
        (**self).take_invite(hash, now)
    }
    fn invites(&self) -> Vec<([u8; 32], bool, u64)> {
        (**self).invites()
    }
    fn find_invite(&self, hash: [u8; 32]) -> Option<(bool, u64)> {
        (**self).find_invite(hash)
    }
    fn drop_invite(&mut self, hash: [u8; 32]) -> Result<bool, Failed> {
        (**self).drop_invite(hash)
    }
    fn is_owner(&self, account: &[u8]) -> bool {
        (**self).is_owner(account)
    }
    fn has_account(&self, account: &[u8]) -> bool {
        (**self).has_account(account)
    }
    fn account_list(&self) -> Vec<(Vec<u8>, bool, u64)> {
        (**self).account_list()
    }
    fn remove_account(&mut self, account: &[u8]) -> Result<(), Failed> {
        (**self).remove_account(account)
    }
    fn set_recovery(&mut self, account: &[u8], public: [u8; 32], wrapped: Vec<u8>, card: Vec<u8>) -> Result<(), Failed> {
        (**self).set_recovery(account, public, wrapped, card)
    }
    fn recovery(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        (**self).recovery(public)
    }
    fn recovery_card(&self, account: &[u8]) -> Vec<u8> {
        (**self).recovery_card(account)
    }
    fn set_password(&mut self, account: &[u8], slot: PasswordSlot) -> Result<(), Failed> {
        (**self).set_password(account, slot)
    }
    fn drop_password(&mut self, account: &[u8]) -> Result<(), Failed> {
        (**self).drop_password(account)
    }
    fn password_of(&self, id: &str) -> Option<(Vec<u8>, Vec<u8>)> {
        (**self).password_of(id)
    }
    fn password_slot(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        (**self).password_slot(public)
    }
    fn password_card(&self, account: &[u8]) -> Vec<u8> {
        (**self).password_card(account)
    }
    fn add_passkey(&mut self, account: &[u8], slot: PasskeySlot) -> Result<(), Failed> {
        (**self).add_passkey(account, slot)
    }
    fn drop_passkey(&mut self, account: &[u8], public: &[u8; 32]) -> Result<bool, Failed> {
        (**self).drop_passkey(account, public)
    }
    fn passkeys(&self, account: &[u8]) -> Vec<PasskeySlot> {
        (**self).passkeys(account)
    }
    fn passkey_slot(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        (**self).passkey_slot(public)
    }
}

/// What a room holds, by peer: how far from the start each peer's spans
/// reach without a gap, from where its base (if any) leaves off. A device
/// sends a peer's operations past this.
pub fn held(base: Option<&Base>, spans: &[Span]) -> Vec<(u64, u64)> {
    let mut reach: HashMap<u64, u64> = base.map(|b| b.vv.iter().copied().collect()).unwrap_or_default();
    let mut by_peer: HashMap<u64, Vec<(u64, u64)>> = HashMap::new();
    for s in spans {
        by_peer.entry(s.peer).or_default().push((s.start, s.end));
    }
    for (peer, mut v) in by_peer {
        v.sort_unstable();
        let r = reach.entry(peer).or_insert(0);
        for (a, b) in v {
            if a <= *r {
                *r = (*r).max(b);
            }
        }
    }
    let mut out: Vec<(u64, u64)> = reach.into_iter().filter(|&(_, r)| r > 0).collect();
    out.sort_unstable();
    out
}

/// The records a device that holds `vv` lacks: the base, if it lacks any
/// of it, then every span reaching past what it has of its peer (or past
/// the base, which it will have). One that overlaps what it has is sent
/// whole: taking in an operation twice changes nothing. (The hub sends
/// the same, a part at a time: `hub::Backfill`.)
pub fn backfill(base: Option<&Base>, spans: &[Span], vv: &[(u64, u64)]) -> Vec<Vec<u8>> {
    let mut has: HashMap<u64, u64> = vv.iter().copied().collect();
    let mut out = Vec::new();
    if let Some(b) = base
        && !covers(vv, &b.vv)
    {
        out.push(b.record.clone());
        for &(p, c) in &b.vv {
            let h = has.entry(p).or_insert(0);
            *h = (*h).max(c);
        }
    }
    out.extend(spans.iter().filter(|s| s.end > has.get(&s.peer).copied().unwrap_or(0)).map(|s| s.record.clone()));
    out
}

#[derive(Clone)]
enum Shelved {
    Span(Span),
    Base(Base),
}

#[derive(Default)]
struct Account {
    devices: HashMap<Vec<u8>, Device>,
    seq: u64,
    rooms: HashMap<String, Vec<Span>>,
    bases: HashMap<String, Base>,
    /// Each room's live records and their bytes, kept counted as they
    /// come and go (as the databases keep theirs), and how many rooms
    /// hold anything live.
    live: HashMap<String, (u64, u64)>,
    kept: u64,
    /// Set aside: by which base's seq, when, and what (a span, or the base
    /// before it).
    shelved: Vec<(u64, u64, String, Shelved)>,
    log: Vec<Logged>,
    /// Rooms removed, and when.
    removed: HashMap<String, u64>,
    epoch: u32,
    /// Device (empty: the recovery code), epoch, the key sealed to it.
    rekeys: Vec<(Vec<u8>, u32, Vec<u8>)>,
    recovery_card: Vec<u8>,
    owner: bool,
    /// When it was made: the order accounts are listed in.
    made: u64,
}

impl Account {
    fn holds(&self, room: &str) -> bool {
        self.live.get(room).is_some_and(|&(n, _)| n > 0) || self.bases.contains_key(room)
    }

    /// `change` made to `room`, and the rooms holding anything counted
    /// again for it.
    fn changing<T>(&mut self, room: &str, change: impl FnOnce(&mut Account) -> T) -> T {
        let before = self.holds(room);
        let out = change(self);
        match (before, self.holds(room)) {
            (false, true) => self.kept += 1,
            (true, false) => self.kept -= 1,
            _ => {}
        }
        out
    }

    /// `room`'s live records counted `n` more (or fewer), `bytes` with them.
    fn count(&mut self, room: &str, n: i64, bytes: i64) {
        let c = self.live.entry(room.to_string()).or_default();
        c.0 = c.0.wrapping_add_signed(n);
        c.1 = c.1.wrapping_add_signed(bytes);
    }
}

/// How many spans, and their bytes.
fn weigh<'a>(spans: impl IntoIterator<Item = &'a Span>) -> (i64, i64) {
    spans.into_iter().fold((0, 0), |(n, b), s| (n + 1, b + s.record.len() as i64))
}

/// An account's password, as the server keeps it: the email it signs in
/// as, OPAQUE's record, and the slot it opens (§7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordSlot {
    pub id: String,
    pub record: Vec<u8>,
    pub public: [u8; 32],
    pub wrapped: Vec<u8>,
    pub card: Vec<u8>,
}

/// A passkey's slot, as the server keeps it (§7): its signing key, the key
/// ring wrapped under what the passkey's PRF output comes to, its card,
/// and when it was added (seconds since 1970).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasskeySlot {
    pub public: [u8; 32],
    pub wrapped: Vec<u8>,
    pub card: Vec<u8>,
    pub made: u64,
}

/// Everything in memory: the simulation's, and the tests'.
#[derive(Default)]
pub struct Memory {
    accounts: HashMap<Vec<u8>, Account>,
    invites: HashMap<[u8; 32], (bool, u64)>,
    recovery: HashMap<[u8; 32], (Vec<u8>, Vec<u8>)>,
    passwords: HashMap<Vec<u8>, PasswordSlot>,
    /// By account, the oldest first.
    passkeys: HashMap<Vec<u8>, Vec<PasskeySlot>>,
    /// How many times a room's records were read whole.
    spans_read: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Memory {
    /// A count of the times a room's records are read whole
    /// ([`Storage::spans`]), kept however the store is boxed: a write
    /// mustn't (the tests').
    pub fn spans_read(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        self.spans_read.clone()
    }

    /// Every record kept, of every account: what a stolen disk would hold.
    pub fn all_records(&self) -> Vec<&[u8]> {
        self.accounts.values().flat_map(|a| a.rooms.values().flatten().map(|s| s.record.as_slice())).collect()
    }
}

impl Storage for Memory {
    fn device(&self, account: &[u8], device: &[u8]) -> Option<Device> {
        self.accounts.get(account)?.devices.get(device).cloned()
    }

    fn add_device(&mut self, account: &[u8], device: &[u8], public: [u8; 32]) -> Result<(), Failed> {
        let a = self.accounts.entry(account.to_vec()).or_default();
        match a.devices.get(device) {
            Some(d) if d.public == public && !d.revoked => Ok(()),
            Some(_) => Err(Failed(DEVICE_TAKEN.into())),
            None => {
                a.devices.insert(device.to_vec(), Device { public, revoked: false, card: Vec::new(), seen: 0 });
                Ok(())
            }
        }
    }

    fn devices(&self, account: &[u8]) -> Vec<(Vec<u8>, Device)> {
        let mut out: Vec<_> = self.accounts.get(account).map(|a| a.devices.clone().into_iter().collect()).unwrap_or_default();
        out.sort_by(|a: &(Vec<u8>, Device), b| a.0.cmp(&b.0));
        out
    }

    fn set_card(&mut self, account: &[u8], device: &[u8], card: Vec<u8>) -> Result<(), Failed> {
        if let Some(d) = self.accounts.get_mut(account).and_then(|a| a.devices.get_mut(device)) {
            d.card = card;
        }
        Ok(())
    }

    fn set_seen(&mut self, account: &[u8], device: &[u8], now: u64) -> Result<(), Failed> {
        if let Some(d) = self.accounts.get_mut(account).and_then(|a| a.devices.get_mut(device)) {
            d.seen = now;
        }
        Ok(())
    }

    fn epoch(&self, account: &[u8]) -> u32 {
        self.accounts.get(account).map_or(0, |a| a.epoch)
    }

    fn rotate(
        &mut self,
        account: &[u8],
        revoked: &[u8],
        epoch: u32,
        keys: Vec<(Vec<u8>, Vec<u8>)>,
        cards: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<(), Failed> {
        let a = self.accounts.entry(account.to_vec()).or_default();
        if let Some(d) = a.devices.get_mut(revoked) {
            d.revoked = true;
        }
        a.epoch = epoch;
        a.rekeys.extend(keys.into_iter().map(|(id, k)| (id, epoch, k)));
        for (id, card) in cards {
            if id.is_empty() {
                a.recovery_card = card;
            } else if id == carnet_sync::password::SLOT {
                if let Some(p) = self.passwords.get_mut(account) {
                    p.card = card;
                }
            } else if let Some(public) = carnet_sync::passkey::of_slot(&id) {
                if let Some(p) = self.passkeys.get_mut(account).and_then(|all| all.iter_mut().find(|p| p.public == public)) {
                    p.card = card;
                }
            } else if let Some(d) = a.devices.get_mut(&id) {
                d.card = card;
            }
        }
        Ok(())
    }

    fn rekeys(&self, account: &[u8], device: &[u8]) -> Vec<Vec<u8>> {
        let Some(a) = self.accounts.get(account) else { return Vec::new() };
        let mut mine: Vec<_> = a.rekeys.iter().filter(|(id, _, _)| id == device).collect();
        mine.sort_by_key(|(_, e, _)| *e);
        mine.into_iter().map(|(_, _, k)| k.clone()).collect()
    }

    fn revoke_device(&mut self, account: &[u8], device: &[u8]) -> Result<(), Failed> {
        if let Some(d) = self.accounts.get_mut(account).and_then(|a| a.devices.get_mut(device)) {
            d.revoked = true;
        }
        Ok(())
    }

    fn put(&mut self, account: &[u8], room: &str, peer: u64, start: u64, end: u64, record: Vec<u8>) -> Result<Option<u64>, Failed> {
        let a = self.accounts.entry(account.to_vec()).or_default();
        if a.bases.get(room).is_some_and(|b| covers(&b.vv, &[(peer, end)])) {
            return Ok(None);
        }
        if a.rooms.get(room).is_some_and(|spans| spans.iter().any(|s| s.peer == peer && s.start <= start && end <= s.end)) {
            return Ok(None);
        }
        a.changing(room, |a| {
            let spans = a.rooms.entry(room.to_string()).or_default();
            let (gone, kept): (Vec<Span>, Vec<Span>) = spans.drain(..).partition(|s| s.peer == peer && start <= s.start && s.end <= end);
            *spans = kept;
            a.seq += 1;
            let (n, b) = weigh(&gone);
            a.count(room, 1 - n, record.len() as i64 - b);
            a.rooms.entry(room.to_string()).or_default().push(Span { peer, start, end, seq: a.seq, record });
            Ok(Some(a.seq))
        })
    }

    fn spans(&self, account: &[u8], room: &str) -> Vec<Span> {
        self.spans_read.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.accounts.get(account).and_then(|a| a.rooms.get(room)).cloned().unwrap_or_default()
    }

    fn live(&self, account: &[u8], room: &str) -> Live {
        let Some(a) = self.accounts.get(account) else { return Live::default() };
        let (records, bytes) = a.live.get(room).copied().unwrap_or_default();
        Live { records, bytes, based: a.bases.contains_key(room) }
    }

    fn room_count(&self, account: &[u8]) -> u64 {
        self.accounts.get(account).map_or(0, |a| a.kept)
    }

    fn changed_since(&self, account: &[u8], seq: u64) -> Vec<(String, u64)> {
        let Some(a) = self.accounts.get(account) else { return Vec::new() };
        let mut newest: HashMap<&String, u64> = HashMap::new();
        for (room, spans) in &a.rooms {
            if let Some(n) = spans.iter().map(|s| s.seq).max() {
                newest.insert(room, n);
            }
        }
        for (room, b) in &a.bases {
            let n = newest.entry(room).or_insert(0);
            *n = (*n).max(b.seq);
        }
        let mut out: Vec<(String, u64)> = newest.into_iter().filter(|&(_, n)| n > seq).map(|(r, n)| (r.clone(), n)).collect();
        out.sort_by_key(|(_, n)| *n);
        out
    }

    fn base(&self, account: &[u8], room: &str) -> Option<Base> {
        self.accounts.get(account)?.bases.get(room).cloned()
    }

    fn set_base(&mut self, account: &[u8], room: &str, vv: Vec<(u64, u64)>, record: Vec<u8>, by: &[u8], now: u64) -> Result<u64, Failed> {
        let a = self.accounts.entry(account.to_vec()).or_default();
        a.changing(room, |a| {
            a.seq += 1;
            let seq = a.seq;
            if let Some(old) = a.bases.remove(room) {
                a.shelved.push((seq, now, room.to_string(), Shelved::Base(old)));
            }
            let spans = a.rooms.entry(room.to_string()).or_default();
            let (covered, live): (Vec<Span>, Vec<Span>) = spans.drain(..).partition(|s| covers(&vv, &[(s.peer, s.end)]));
            *spans = live;
            let (n, b) = weigh(&covered);
            a.count(room, -n, -b);
            a.shelved.extend(covered.into_iter().map(|s| (seq, now, room.to_string(), Shelved::Span(s))));
            a.bases.insert(room.to_string(), Base { seq, vv, record, by: by.to_vec(), at: now, verified: false, undoable: true });
            Ok(seq)
        })
    }

    fn verify_base(&mut self, account: &[u8], room: &str, seq: u64) -> Result<(), Failed> {
        if let Some(b) = self.accounts.get_mut(account).and_then(|a| a.bases.get_mut(room)).filter(|b| b.seq == seq) {
            b.verified = true;
        }
        Ok(())
    }

    fn roll_back(&mut self, account: &[u8], room: &str) -> Result<bool, Failed> {
        let Some(a) = self.accounts.get_mut(account) else { return Ok(false) };
        let Some(b) = a.bases.get(room).filter(|b| b.undoable) else { return Ok(false) };
        let seq = b.seq;
        a.changing(room, |a| {
            a.bases.remove(room);
            let (back, kept): (Vec<_>, Vec<_>) = a.shelved.drain(..).partition(|(by, _, r, _)| *by == seq && r == room);
            a.shelved = kept;
            for (_, _, _, s) in back {
                match s {
                    Shelved::Span(s) => {
                        a.count(room, 1, s.record.len() as i64);
                        a.rooms.entry(room.to_string()).or_default().push(s);
                    }
                    Shelved::Base(old) => {
                        a.bases.insert(room.to_string(), old);
                    }
                }
            }
            if let Some(spans) = a.rooms.get_mut(room) {
                spans.sort_by_key(|s| s.seq);
            }
        });
        Ok(true)
    }

    fn shelf(&self, account: &[u8], room: &str) -> Shelf {
        let Some(a) = self.accounts.get(account) else { return Shelf::default() };
        let mine = a.shelved.iter().filter(|(_, _, r, _)| r == room);
        mine.fold(Shelf::default(), |s, (_, at, _, x)| Shelf {
            count: s.count + 1,
            bytes: s.bytes + match x {
                Shelved::Span(sp) => sp.record.len() as u64,
                Shelved::Base(b) => b.record.len() as u64,
            },
            since: if s.since == 0 { *at } else { s.since.min(*at) },
        })
    }

    fn rooms(&self, account: &[u8]) -> Vec<String> {
        let Some(a) = self.accounts.get(account) else { return Vec::new() };
        let mut out: Vec<String> = a.rooms.iter().filter(|(_, s)| !s.is_empty()).map(|(r, _)| r.clone()).chain(a.bases.keys().cloned()).collect();
        out.sort();
        out.dedup();
        out
    }

    fn remove_room(&mut self, account: &[u8], room: &str, now: u64) -> Result<(), Failed> {
        let Some(a) = self.accounts.get_mut(account) else { return Err(Failed("no such account".into())) };
        a.changing(room, |a| {
            a.rooms.remove(room);
            a.bases.remove(room);
            a.live.remove(room);
        });
        a.shelved.retain(|(_, _, r, _)| r != room);
        a.removed.entry(room.to_string()).or_insert(now);
        Ok(())
    }

    fn is_removed(&self, account: &[u8], room: &str) -> bool {
        self.accounts.get(account).is_some_and(|a| a.removed.contains_key(room))
    }

    fn usage(&self, account: &[u8]) -> u64 {
        let Some(a) = self.accounts.get(account) else { return 0 };
        let spans: u64 = a.rooms.values().flatten().map(|s| s.record.len() as u64).sum();
        let bases: u64 = a.bases.values().map(|b| b.record.len() as u64).sum();
        let shelved: u64 = a
            .shelved
            .iter()
            .map(|(_, _, _, s)| match s {
                Shelved::Span(s) => s.record.len() as u64,
                Shelved::Base(b) => b.record.len() as u64,
            })
            .sum();
        spans + bases + shelved
    }

    fn purge(&mut self, before: u64) -> Result<(), Failed> {
        for a in self.accounts.values_mut() {
            let gone: Vec<(u64, String)> = a.shelved.iter().filter(|(_, at, _, _)| *at < before).map(|(by, _, r, _)| (*by, r.clone())).collect();
            a.shelved.retain(|(_, at, _, _)| *at >= before);
            for (by, room) in gone {
                if let Some(b) = a.bases.get_mut(&room).filter(|b| b.seq == by) {
                    b.undoable = false;
                }
                // A base set aside with what it set aside gone stays so.
                for (_, _, _, s) in a.shelved.iter_mut() {
                    if let Shelved::Base(b) = s
                        && b.seq == by
                    {
                        b.undoable = false;
                    }
                }
            }
        }
        Ok(())
    }

    fn log(&mut self, account: &[u8], entry: Logged) -> Result<(), Failed> {
        let a = self.accounts.entry(account.to_vec()).or_default();
        a.log.push(entry);
        Ok(())
    }

    fn logged(&self, account: &[u8]) -> Vec<Logged> {
        self.accounts.get(account).map(|a| a.log.iter().rev().take(50).cloned().collect()).unwrap_or_default()
    }

    fn seq(&self, account: &[u8]) -> u64 {
        self.accounts.get(account).map_or(0, |a| a.seq)
    }

    fn accounts(&self) -> usize {
        self.accounts.len()
    }

    fn add_account(&mut self, account: &[u8], owner: bool) -> Result<(), Failed> {
        let made = self.accounts.len() as u64;
        let a = self.accounts.entry(account.to_vec()).or_default();
        a.owner |= owner;
        a.made = made;
        Ok(())
    }

    fn add_invite(&mut self, hash: [u8; 32], owner: bool, expires: u64) -> Result<(), Failed> {
        self.invites.insert(hash, (owner, expires));
        Ok(())
    }

    fn take_invite(&mut self, hash: [u8; 32], now: u64) -> Result<Option<bool>, Failed> {
        Ok(self.invites.remove(&hash).filter(|&(_, expires)| now < expires).map(|(owner, _)| owner))
    }

    fn invites(&self) -> Vec<([u8; 32], bool, u64)> {
        let mut all: Vec<_> = self.invites.iter().map(|(h, &(owner, expires))| (*h, owner, expires)).collect();
        all.sort_by_key(|&(h, _, expires)| (expires, h));
        all
    }

    fn drop_invite(&mut self, hash: [u8; 32]) -> Result<bool, Failed> {
        Ok(self.invites.remove(&hash).is_some())
    }

    fn is_owner(&self, account: &[u8]) -> bool {
        self.accounts.get(account).is_some_and(|a| a.owner)
    }

    fn has_account(&self, account: &[u8]) -> bool {
        self.accounts.contains_key(account)
    }

    fn account_list(&self) -> Vec<(Vec<u8>, bool, u64)> {
        let mut all: Vec<_> = self.accounts.iter().map(|(id, a)| (id.clone(), a.owner, a.made)).collect();
        all.sort_by(|a, b| (a.2, &a.0).cmp(&(b.2, &b.0)));
        all
    }

    fn remove_account(&mut self, account: &[u8]) -> Result<(), Failed> {
        self.accounts.remove(account);
        self.recovery.retain(|_, (a, _)| a != account);
        self.passwords.remove(account);
        self.passkeys.remove(account);
        Ok(())
    }

    fn set_recovery(&mut self, account: &[u8], public: [u8; 32], wrapped: Vec<u8>, card: Vec<u8>) -> Result<(), Failed> {
        if self.recovery.get(&public).is_some_and(|(a, _)| a != account) {
            return Err(Failed(RECOVERY_TAKEN.into()));
        }
        self.recovery.retain(|_, (a, _)| a != account);
        self.recovery.insert(public, (account.to_vec(), wrapped));
        let a = self.accounts.entry(account.to_vec()).or_default();
        a.recovery_card = card;
        a.rekeys.retain(|(id, _, _)| !id.is_empty());
        Ok(())
    }

    fn recovery(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        self.recovery.get(public).cloned()
    }

    fn recovery_card(&self, account: &[u8]) -> Vec<u8> {
        self.accounts.get(account).map(|a| a.recovery_card.clone()).unwrap_or_default()
    }

    fn set_password(&mut self, account: &[u8], slot: PasswordSlot) -> Result<(), Failed> {
        self.passwords.insert(account.to_vec(), slot);
        if let Some(a) = self.accounts.get_mut(account) {
            a.rekeys.retain(|(id, _, _)| id != carnet_sync::password::SLOT);
        }
        Ok(())
    }

    fn drop_password(&mut self, account: &[u8]) -> Result<(), Failed> {
        self.passwords.remove(account);
        if let Some(a) = self.accounts.get_mut(account) {
            a.rekeys.retain(|(id, _, _)| id != carnet_sync::password::SLOT);
        }
        Ok(())
    }

    fn password_of(&self, id: &str) -> Option<(Vec<u8>, Vec<u8>)> {
        self.passwords.iter().find(|(_, p)| p.id == id).map(|(a, p)| (a.clone(), p.record.clone()))
    }

    fn password_slot(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        self.passwords.iter().find(|(_, p)| p.public == *public).map(|(a, p)| (a.clone(), p.wrapped.clone()))
    }

    fn password_card(&self, account: &[u8]) -> Vec<u8> {
        self.passwords.get(account).map(|p| p.card.clone()).unwrap_or_default()
    }

    fn add_passkey(&mut self, account: &[u8], slot: PasskeySlot) -> Result<(), Failed> {
        let id = carnet_sync::passkey::slot(&slot.public);
        if let Some(a) = self.accounts.get_mut(account) {
            a.rekeys.retain(|(d, _, _)| *d != id);
        }
        let all = self.passkeys.entry(account.to_vec()).or_default();
        match all.iter_mut().find(|p| p.public == slot.public) {
            // Added again: when it first was stays.
            Some(p) => {
                p.wrapped = slot.wrapped;
                p.card = slot.card;
            }
            None => all.push(slot),
        }
        Ok(())
    }

    fn drop_passkey(&mut self, account: &[u8], public: &[u8; 32]) -> Result<bool, Failed> {
        let id = carnet_sync::passkey::slot(public);
        if let Some(a) = self.accounts.get_mut(account) {
            a.rekeys.retain(|(d, _, _)| *d != id);
        }
        let Some(all) = self.passkeys.get_mut(account) else { return Ok(false) };
        let had = all.len();
        all.retain(|p| p.public != *public);
        Ok(all.len() < had)
    }

    fn passkeys(&self, account: &[u8]) -> Vec<PasskeySlot> {
        self.passkeys.get(account).cloned().unwrap_or_default()
    }

    fn passkey_slot(&self, public: &[u8; 32]) -> Option<(Vec<u8>, Vec<u8>)> {
        self.passkeys.iter().find_map(|(a, all)| all.iter().find(|p| p.public == *public).map(|p| (a.clone(), p.wrapped.clone())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_stands_for_what_it_covers_and_rolls_back() {
        let mut m = Memory::default();
        let a = b"acct";
        m.add_account(a, true).unwrap();
        m.put(a, "r", 1, 0, 5, b"a".to_vec()).unwrap();
        m.put(a, "r", 2, 0, 3, b"b".to_vec()).unwrap();
        m.put(a, "r", 1, 5, 8, b"c".to_vec()).unwrap();
        // A snapshot holding peer 1 to 5 and peer 2 to 3: those two go
        // aside; peer 1's 5..8 stays live.
        let seq = m.set_base(a, "r", vec![(1, 5), (2, 3)], b"S".to_vec(), b"dev", 100).unwrap();
        assert_eq!(seq, 4);
        assert_eq!(m.spans(a, "r").iter().map(|s| s.record.clone()).collect::<Vec<_>>(), [b"c".to_vec()]);
        let base = m.base(a, "r").unwrap();
        assert_eq!(held(Some(&base), &m.spans(a, "r")), [(1, 8), (2, 3)]);
        // A device with nothing gets the base and what's past it; one with
        // all of the base, only what's past what it has.
        assert_eq!(backfill(Some(&base), &m.spans(a, "r"), &[]), [b"S".to_vec(), b"c".to_vec()]);
        assert_eq!(backfill(Some(&base), &m.spans(a, "r"), &[(1, 6), (2, 3)]), [b"c".to_vec()]);
        assert_eq!(m.put(a, "r", 2, 1, 3, b"late".to_vec()), Ok(None), "covered by the base");
        assert_eq!(m.shelf(a, "r"), Shelf { count: 2, bytes: 2, since: 100 });
        assert_eq!(m.changed_since(a, 3), [("r".to_string(), 4)]);

        assert!(m.roll_back(a, "r").unwrap());
        assert_eq!(m.base(a, "r"), None);
        assert_eq!(m.spans(a, "r").len(), 3, "all three live again");
        assert_eq!(m.shelf(a, "r").count, 0);

        // Set aside past its time: gone, and the base can't be rolled back.
        m.set_base(a, "r", vec![(1, 8), (2, 3)], b"S2".to_vec(), b"dev", 200).unwrap();
        m.purge(201).unwrap();
        assert!(!m.base(a, "r").unwrap().undoable);
        assert!(!m.roll_back(a, "r").unwrap());
        assert_eq!(m.base(a, "r").unwrap().record, b"S2");
    }

    #[test]
    fn spans_cover_replace_and_reach() {
        let mut m = Memory::default();
        let a = b"acct";
        assert_eq!(m.put(a, "r", 1, 0, 5, b"a".to_vec()), Ok(Some(1)));
        assert_eq!(m.put(a, "r", 1, 2, 4, b"b".to_vec()), Ok(None), "covered");
        assert_eq!(m.put(a, "r", 1, 0, 9, b"c".to_vec()), Ok(Some(2)), "covers, replaces");
        assert_eq!(m.put(a, "r", 2, 3, 6, b"d".to_vec()), Ok(Some(3)));
        let spans = m.spans(a, "r");
        assert_eq!(spans.len(), 2);
        // Peer 2 starts at 3: nothing held from its start.
        assert_eq!(held(None, &spans), [(1, 9)]);
        assert_eq!(backfill(None, &spans, &[(1, 9)]), [b"d".to_vec()]);
        assert_eq!(m.changed_since(a, 2), [("r".to_string(), 3)]);
    }
}
