//! A device's side of sync, as a state machine with no I/O in it: bytes
//! come in by [`Session::receive`], bytes to send come out of
//! [`Session::take_out`]. A driver (the desktop's thread, the PWA's
//! socket, the simulation) moves them; this decides what they say. So the
//! same logic runs everywhere, and the simulation runs it exactly.
//!
//! What a device sends is never kept anywhere but in its notes. On
//! joining a room it learns what the server holds of it (a version
//! vector) and sends what its own copy has past that, sealed; a save
//! sends what is new since. A socket that drops loses nothing: the next
//! join works out again what the server lacks.

use std::collections::{BTreeSet, HashMap, HashSet};

use loro_protocol::ProtocolMessage;

use crate::compat::{Fit, Greeting};
use crate::crypto::{BlobId, Card, DeviceKey, MAX_BLOB, Random, RecoveryCode, Ring, VaultKey, blob_id, login_message, open_blob, seal_blob, sealed_len};
use crate::elo::{self, Header, Pieces};
use crate::note::NoteDoc;
use crate::pair::{Opener, OpenerOut};
use crate::rekey::{self, Signer};
use crate::wire::{self, AccountRow, BatchId, Carnet, CrdtType, Deny, DeviceRow, Frame, InviteRow, LogRow, PasskeyRow, RoomRow, UpdateStatusCode};

/// Where a session's notes are: one doc per room.
pub trait Docs {
    /// The rooms this device has a doc for.
    fn rooms(&self) -> Vec<String>;
    /// `room`'s doc, made (empty) if this device has none yet.
    fn doc(&mut self, room: &str) -> &NoteDoc;
}

/// Who a device is, and what it reads with.
#[derive(Clone, Debug)]
pub struct Identity {
    /// The server's origin, as the login signature names it.
    pub origin: String,
    pub account: Vec<u8>,
    pub device: Vec<u8>,
    pub key: DeviceKey,
    /// Every epoch's vault key it has; new records are sealed under the
    /// newest.
    pub ring: Ring,
    /// What the account's other devices call it.
    pub name: String,
}

/// A device of the account, as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: Vec<u8>,
    /// From its card; `None` before it has sent one, or if it did not
    /// open (not this vault's).
    pub name: Option<String>,
    /// Its last login, in seconds since 1970.
    pub seen: u64,
    /// This device.
    pub you: bool,
    /// Logged in now, as the server said when it listed them (one from
    /// before it said so: false).
    pub online: bool,
}

/// A passkey of the account, as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasskeyInfo {
    /// Its slot's signing key: what names it, to remove it by.
    pub id: [u8; 32],
    /// From its card; `None` if that did not open.
    pub name: Option<String>,
    /// Its credential id, as the browser knows it; empty if the card did
    /// not open.
    pub credential: Vec<u8>,
    /// When it was added, in seconds since 1970.
    pub made: u64,
}

/// What happened that the app should know of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    LoggedIn,
    /// Turned away, and why, in words.
    Denied(String),
    /// Turned away for good: this device taken out of its account, or
    /// (`account`) the account gone from the server. Its keys are no use.
    Removed { account: bool },
    /// The app and the server don't get on (`compat`, §14): either is too
    /// old for the other, and nothing is synced; or the server will stop
    /// taking this app soon (`Fit::UpdateSoon`), and it goes on.
    Outdated(Fit),
    /// `room` took in changes from elsewhere: its file is to be written.
    Changed(String),
    /// A record that did not open (another vault's, or tampered with):
    /// left out, and said.
    Unreadable(String),
    /// A pairing opened: the code to show the new device's owner.
    PairCode(String),
    /// The device with this id was let in.
    Paired(Vec<u8>),
    /// The pairing is over without a device let in, and why.
    PairFailed(String),
    /// The recovery code is set: the server keeps the vault key under it.
    RecoverySet,
    /// The account's devices, as asked for.
    Devices(Vec<DeviceInfo>),
    /// A new epoch's key came: the ring changed, and is to be kept.
    Rekeyed(u32),
    /// What was asked was not done, and why.
    Refused(String),
    /// A snapshot of `room` did not hold what its header said: not taken
    /// in, and the server told.
    BadSnapshot(String),
    /// The account's rooms as the server keeps them, and its log.
    Inspected(Vec<RoomRow>, Vec<LogRow>),
    /// A picture offered the server doesn't have: send it ([`Session::put_blob`]),
    /// which makes it a [`Transfer`].
    BlobWanted(BlobId),
    /// A picture the server has, whole: offered and already there, or put
    /// and answered that it's kept (on the server's disk).
    BlobStored(BlobId),
    /// A picture asked for, come and opened: the picture itself.
    BlobGot(BlobId, Vec<u8>),
    /// A picture asked for that the server doesn't have, or that didn't
    /// open as itself; or one being put that didn't go, or wasn't kept:
    /// offered or asked for again later.
    BlobFailed(BlobId),
    /// Whether this account is the server's owner's, and if it is, the
    /// invites waiting and every account (§6), as asked for.
    Invites { owner: bool, invites: Vec<InviteRow>, accounts: Vec<AccountRow> },
    /// The server has removed `room` (its note deleted forever): asked by
    /// [`Session::remove_room`], or found so on joining it.
    RoomRemoved(String),
    /// The server has these pictures marked to drop, as
    /// [`Session::drop_blobs`] asked.
    BlobsDropped(Vec<BlobId>),
    /// What the account keeps, and may, as [`Session::usage`] asked.
    Usage(Usage),
    /// The account can't write for now (its plan lapsed, or it's
    /// suspended): what's written here stays here until it can. Said once
    /// a login, or as soon as the server says so.
    ReadOnly,
    /// The account can write again, after [`Event::ReadOnly`]: what waited
    /// here is on its way. Said as soon as the server says so (a plan
    /// renewed, a suspension lifted), or at the next login that finds it.
    Writable,
    /// With the devices: whether the server takes passwords, the email
    /// this account's password signs in as, if it has one, and the only
    /// email one may sign in as here, if the server holds it to one (a
    /// hosted service: the email the account signed up with). The apps
    /// show that one, not a field to type another in.
    Password { allowed: bool, id: Option<String>, only: Option<String> },
    /// The password is set (or changed, or gone), as asked.
    PasswordSet,
    /// With the devices: the account's passkeys.
    Passkeys(Vec<PasskeyInfo>),
    /// A passkey is added (or taken away), as asked.
    PasskeySet,
}

/// What an account keeps and may keep (docs/SYNC.md §11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    /// Bytes kept, sealed: the notes, and the pictures.
    pub notes: u64,
    pub pictures: u64,
    /// The most it may keep; `None`: no limit (a server of one's own).
    pub limit: Option<u64>,
    pub writable: bool,
    /// The plan, and when it runs out (`None`: no end).
    pub plan: String,
    pub until: Option<u64>,
}

impl Usage {
    /// Whether the server takes a new picture now: the account can write,
    /// and is under its limit, if it has one. A picture held back (the
    /// server said [`FULL`] or [`READ_ONLY`]) is offered again once it is.
    pub fn has_room(&self) -> bool {
        self.writable && self.limit.is_none_or(|limit| self.notes + self.pictures < limit)
    }
}

/// What the server says (`Refused`) of pictures offered while the account
/// is full: they stay on the device until [`Usage::has_room`].
pub const FULL: &str = "This account is full, so new pictures stay on this device for now. Delete some, or move to a bigger plan.";

/// What the server says (`Refused`) of pictures offered while the account
/// can't write. The apps say read only once, as [`Event::ReadOnly`] has it,
/// so they don't show this beside it.
pub const READ_ONLY: &str = "This account is read only for now: changes stay on this device, and sync once it can write again.";

/// The most letters of the server's own words shown at once.
pub const SAID_MAX: usize = 200;

/// Words the server sent, made fit to show: on one line, no letter that
/// draws nothing or turns the text around, and at most [`SAID_MAX`]
/// letters. A server could say anything, at any length.
pub fn server_words(words: &str) -> String {
    let hidden = |c: char| c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}');
    let clean: String = words.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|&c| !hidden(c)).collect();
    let line = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(SAID_MAX) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line,
    }
}

/// Words the server sent, shown as its own and not Carnet's: "The server
/// says: …", made fit to show ([`server_words`]).
pub fn server_says(words: &str) -> String {
    format!("The server says: {}", server_words(words))
}

/// What this device says ([`Event::Refused`]) when the server asks for a
/// note compacted and its snapshot, history and all, is past what a
/// snapshot may weigh ([`elo::MAX_SNAPSHOT`]): it stays as it is on the
/// server, and once that's full its new changes wait here.
pub const TOO_BIG: &str = "A note is too big for the server to tidy up, so once it has as many changes as the server keeps, new ones stay on this device. Split it into smaller notes to sync them.";

/// A refusal ([`Event::Refused`]) as the apps show it: the server's words,
/// said as its own ([`server_says`]); but for [`FULL`] and [`READ_ONLY`],
/// which the apps know, and the words of this device's own making.
pub fn refusal_words(why: &str) -> String {
    if why == FULL || why == READ_ONLY || why == TOO_BIG || why.starts_with(PASSWORD_NOT_SET) { why.to_string() } else { server_says(why) }
}

/// How this device says a password it couldn't set (its own words, not
/// the server's).
const PASSWORD_NOT_SET: &str = "The password couldn't be set: ";

/// How much of a picture goes in one frame.
pub const BLOB_PIECE: usize = 192 * 1024;

/// The last list of devices, as read: each row with its card, if it
/// opened under this device's newest key and says it is that device.
struct Listed {
    epoch: u32,
    /// This device's newest epoch when the cards were read: only cards
    /// under it are believed, so a list read before a key came is read
    /// again before anything is sealed by it.
    at: u32,
    rows: Vec<(DeviceRow, Option<Card>)>,
    /// The recovery code's card: `Ok(None)` if there is no code, `Err` if
    /// there is one whose card did not open.
    recovery: Result<Option<Card>, ()>,
    /// The password's card, likewise; its name is the email it signs in as.
    password: Result<Option<Card>, ()>,
    /// Each passkey, with its card if it opened and says it is that
    /// passkey's slot.
    passkeys: Vec<(PasskeyRow, Option<Card>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Offline,
    /// Connected, the challenge awaited.
    Waiting,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Join {
    Asked,
    Joined,
}

pub struct Session {
    id: Identity,
    state: State,
    /// The last vault `seq` this device has a doc for every room of.
    cursor: u64,
    joined: HashMap<String, Join>,
    /// What the server holds of each room, by peer, as far as this
    /// device knows: told on joining, and grown by what is sent.
    held: HashMap<String, HashMap<u64, u64>>,
    /// Batches sent and not yet acknowledged, with their rooms.
    unacked: HashMap<BatchId, String>,
    /// Rooms with changes made here the server isn't known to keep yet:
    /// offline, read only, or on their way ([`Session::waiting`]).
    waiting: BTreeSet<String>,
    /// What the server was last known to keep of each room, when it had
    /// all this device had: a save that changes nothing waits for nothing.
    /// Kept across logins.
    kept: HashMap<String, HashMap<u64, u64>>,
    /// How many times the account's notes changed as this device saw:
    /// [`Session::moves`].
    moves: u64,
    next_batch: u64,
    out: Vec<Vec<u8>>,
    events: Vec<Event>,
    /// A pairing this device opened, while it is open.
    pairing: Option<Opener>,
    listed: Option<Listed>,
    /// Pictures the server wants, and where to put each.
    putting: HashMap<BlobId, String>,
    /// Pictures asked for, not come yet.
    fetching: HashSet<BlobId>,
    /// HTTP the driver is to do: [`Session::take_transfers`].
    transfers: Vec<Transfer>,
    /// Rooms to remove and pictures to drop, until the server says done:
    /// said again on each login.
    removing: BTreeSet<String>,
    dropping: BTreeSet<BlobId>,
    /// Rooms the server lets this device read, not write (the account's
    /// plan lapsed): nothing is sent to them until the next login.
    read_only: HashSet<String>,
    /// Rooms the server takes no more of for now (too much in them not
    /// yet compacted, or in the account: `PayloadTooLarge`): nothing more
    /// is sent to them until the server asks for one compacted, or the
    /// next login, rather than joined again to be refused again.
    full: HashSet<String>,
    /// Said [`Event::ReadOnly`], and not [`Event::Writable`] since.
    limited: bool,
    /// A password being set: the email, the password, OPAQUE's state,
    /// until the server answers.
    setting: Option<(String, zeroize::Zeroizing<String>, crate::password::Pending)>,
    /// The server's greeting, from its last challenge: what it can do.
    server: Option<Greeting>,
    /// New epochs' keys that follow one this device has no key for yet:
    /// taken once it has (`rekey.rs`).
    early: Vec<Vec<u8>>,
    /// Updates coming in fragments (a big snapshot), by room, until whole.
    incoming: HashMap<String, Pieces>,
    /// Rooms too big to compact ([`TOO_BIG`]), said once a session.
    too_big: HashSet<String>,
    /// The most a snapshot this device sends may weigh: [`elo::MAX_SNAPSHOT`].
    max_snapshot: usize,
}

/// A picture's bytes to move over HTTP, to or from a short-lived signed
/// URL the server gave (docs/SYNC.md §6): the driver does it, and says how
/// it went ([`Session::transferred`]). The socket only says which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// `PUT` these sealed bytes at `url`.
    Put { id: BlobId, url: String, sealed: Vec<u8> },
    /// `GET` `url`: the sealed bytes.
    Get { id: BlobId, url: String },
}

impl Transfer {
    pub fn id(&self) -> BlobId {
        match self {
            Transfer::Put { id, .. } | Transfer::Get { id, .. } => *id,
        }
    }
}

impl Session {
    pub fn new(id: Identity, cursor: u64) -> Self {
        Session {
            id,
            state: State::Offline,
            cursor,
            joined: HashMap::new(),
            held: HashMap::new(),
            unacked: HashMap::new(),
            waiting: BTreeSet::new(),
            kept: HashMap::new(),
            moves: 0,
            next_batch: 0,
            out: Vec::new(),
            events: Vec::new(),
            pairing: None,
            listed: None,
            putting: HashMap::new(),
            fetching: HashSet::new(),
            transfers: Vec::new(),
            removing: BTreeSet::new(),
            dropping: BTreeSet::new(),
            read_only: HashSet::new(),
            full: HashSet::new(),
            limited: false,
            setting: None,
            server: None,
            early: Vec::new(),
            incoming: HashMap::new(),
            too_big: HashSet::new(),
            max_snapshot: elo::MAX_SNAPSHOT,
        }
    }

    /// A smaller most a snapshot may weigh than [`elo::MAX_SNAPSHOT`], for
    /// a test to reach it without a note of that size.
    #[doc(hidden)]
    pub fn set_max_snapshot(&mut self, bytes: usize) {
        self.max_snapshot = bytes.min(elo::MAX_SNAPSHOT);
    }

    /// Sets (or changes) the account's password, signing in as `email`
    /// (§7): [`Event::PasswordSet`], or [`Event::Refused`] (an email
    /// another account has, a server that takes no passwords).
    pub fn set_password(&mut self, email: &str, password: &str, rng: &mut impl Random) -> Result<(), String> {
        if self.state != State::Ready {
            return Err("not connected".into());
        }
        let id = crate::password::id(email);
        if !id.contains('@') || id.len() > 254 {
            return Err("That doesn't look like an email address".into());
        }
        if password.chars().count() < crate::password::MIN_LEN {
            return Err(format!("A password of {} letters at least", crate::password::MIN_LEN));
        }
        let (request, state) = crate::password::register_start(password, rng)?;
        self.setting = Some((id.clone(), zeroize::Zeroizing::new(password.to_string()), crate::password::Pending(state)));
        self.send(Frame::Carnet(Carnet::PasswordSetStart { id, request }));
        Ok(())
    }

    /// The account deleted, with everything the server keeps of it: every
    /// device is let go, this one too (`Denied`). The notes on the devices
    /// stay as they are.
    pub fn delete_account(&mut self) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::AccountDelete));
        }
    }

    /// Adds a passkey (§7, `passkey.rs`): its slot keeps the key ring
    /// wrapped under what its PRF output comes to, and its card, with
    /// `name` and the browser's `credential` id. [`Event::PasskeySet`], or
    /// [`Event::Refused`] (one too many, say).
    pub fn add_passkey(&mut self, name: &str, credential: &[u8], prf: &[u8], rng: &mut impl Random) -> Result<(), String> {
        if self.state != State::Ready {
            return Err("not connected".into());
        }
        if prf.len() != crate::passkey::PRF_LEN {
            return Err("That passkey didn't give what Carnet needs".into());
        }
        if credential.is_empty() || credential.len() > 1023 {
            return Err("That passkey's id isn't one".into());
        }
        if self.listed.as_ref().is_some_and(|l| l.passkeys.len() >= crate::passkey::MAX) {
            return Err(format!("{} passkeys at most: remove one first", crate::passkey::MAX));
        }
        let secret = crate::passkey::secret(prf);
        let wrapped = self.id.ring.wrap(&secret.wrapping_key(), rng);
        let public = secret.signing_key().public();
        let card = Card {
            device: crate::passkey::slot(&public),
            public,
            boxed: secret.box_secret().public(),
            name: crate::passkey::name(name),
            credential: credential.to_vec(),
        }
        .seal(&self.id.ring, rng);
        self.send(Frame::Carnet(Carnet::PasskeyAdd { public, wrapped, card }));
        Ok(())
    }

    /// The passkey `id` names can't let a device in any more.
    pub fn remove_passkey(&mut self, id: &[u8; 32]) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::PasskeyDrop { public: *id }));
        }
    }

    /// No password any more: only codes and the recovery words let a
    /// device in.
    pub fn drop_password(&mut self) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::PasswordDrop));
        }
    }

    /// The account this device is in.
    pub fn account(&self) -> &[u8] {
        &self.id.account
    }

    /// Asks what the account keeps, and may: it comes as [`Event::Usage`].
    pub fn usage(&mut self) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::Usage));
        }
    }

    /// `room`'s note is deleted forever: the server is to remove it, now
    /// or on the next login, until it says so ([`Event::RoomRemoved`]).
    /// This device leaves it: the app forgets its doc.
    pub fn remove_room(&mut self, room: &str) {
        self.joined.remove(room);
        self.held.remove(room);
        self.waiting.remove(room);
        self.kept.remove(room);
        if self.removing.insert(room.to_string()) && self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::RoomRemove { room: room.to_string() }));
        }
    }

    /// The rooms to remove and pictures to drop the server hasn't said
    /// done yet: what a device keeps across a restart, to say again.
    pub fn pending_removals(&self) -> (Vec<String>, Vec<BlobId>) {
        (self.removing.iter().cloned().collect(), self.dropping.iter().copied().collect())
    }

    /// No note shows these pictures any more: the server may drop them,
    /// told now or on the next login, until it says so
    /// ([`Event::BlobsDropped`]).
    pub fn drop_blobs(&mut self, ids: &[BlobId]) {
        let new: Vec<BlobId> = ids.iter().copied().filter(|id| self.dropping.insert(*id)).collect();
        if self.state == State::Ready {
            self.send_drops(&new);
        }
    }

    /// `BlobDrop`s for `ids`, as many as they take.
    fn send_drops(&mut self, ids: &[BlobId]) {
        for chunk in ids.chunks(wire::MAX_IDS) {
            self.send(Frame::Carnet(Carnet::BlobDrop { ids: chunk.to_vec() }));
        }
    }

    /// Asks whether the server has these pictures, each with its length
    /// (as it is here, not sealed): each comes back as
    /// [`Event::BlobWanted`] or [`Event::BlobStored`]. Sent
    /// [`wire::MAX_IDS`] to a frame, each with how long it will be
    /// sealed: the server holds the put to that.
    pub fn offer_blobs(&mut self, pictures: Vec<(BlobId, usize)>) {
        if self.state != State::Ready {
            return;
        }
        for chunk in pictures.chunks(wire::MAX_IDS) {
            let ids = chunk.iter().map(|(id, _)| *id).collect();
            let sizes = chunk.iter().map(|(_, n)| sealed_len(*n)).collect();
            self.send(Frame::Carnet(Carnet::BlobOffer { ids, sizes }));
        }
    }

    /// A picture the server wanted, sealed, to be put at the link it gave:
    /// a [`Transfer`], then [`Event::BlobStored`] once the put is answered
    /// (the server's word that it's on its disk). `false` for one too big, or not wanted.
    pub fn put_blob(&mut self, plain: &[u8], rng: &mut impl Random) -> bool {
        if self.state != State::Ready || plain.len() > MAX_BLOB {
            return false;
        }
        let id = blob_id(&self.id.ring, plain);
        let Some(url) = self.putting.remove(&id) else { return false };
        let sealed = seal_blob(&self.id.ring, &id, plain, rng);
        self.transfers.push(Transfer::Put { id, url, sealed });
        true
    }

    /// Asks for a picture: a [`Transfer`], then [`Event::BlobGot`] or
    /// [`Event::BlobFailed`]. Asked while logging in, it's asked once in.
    pub fn get_blob(&mut self, id: BlobId) {
        if self.fetching.insert(id) && self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::BlobFetch { ids: vec![id] }));
        }
    }

    /// The HTTP to do: pictures to put and to fetch.
    pub fn take_transfers(&mut self) -> Vec<Transfer> {
        std::mem::take(&mut self.transfers)
    }

    /// How a [`Transfer`] went: for a fetch, the sealed bytes. A put that
    /// went is kept: the server answers it only once the picture is on its
    /// disk, connected or not. One fetched is opened (it must be the
    /// picture its id names).
    pub fn transferred(&mut self, id: BlobId, put: bool, result: Result<Vec<u8>, String>) {
        match (put, result) {
            (true, Ok(_)) => self.events.push(Event::BlobStored(id)),
            (true, Err(_)) => self.events.push(Event::BlobFailed(id)),
            (false, result) => {
                if !self.fetching.remove(&id) {
                    return;
                }
                match result.ok().map(|sealed| open_blob(&self.id.ring, &id, &sealed)) {
                    Some(Ok(plain)) => self.events.push(Event::BlobGot(id, plain)),
                    _ => self.events.push(Event::BlobFailed(id)),
                }
            }
        }
    }

    /// This device's id for a picture.
    pub fn blob_id(&self, plain: &[u8]) -> BlobId {
        blob_id(&self.id.ring, plain)
    }

    /// Every epoch's key this device has: what the app keeps, after an
    /// [`Event::Rekeyed`].
    pub fn ring(&self) -> &Ring {
        &self.id.ring
    }

    /// Asks for the account's devices: they come as [`Event::Devices`].
    pub fn list_devices(&mut self) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::ListDevices));
        }
    }

    /// Takes `device` out of the account (§7), by the last list asked for.
    /// Another device: a new vault key, the next epoch's, goes sealed to
    /// each device kept and to each slot (the recovery code, the password,
    /// each passkey) and signed by this one (`rekey.rs`), so the one taken
    /// out reads nothing written from now on; this device takes it in when
    /// the server confirms. This device itself: it leaves, and nothing else
    /// changes (it is not taken to be lost or stolen).
    pub fn revoke(&mut self, device: &[u8], rng: &mut impl Random) -> Result<(), String> {
        if self.state != State::Ready {
            return Err("not connected".into());
        }
        if device == self.id.device.as_slice() {
            self.send(Frame::Carnet(Carnet::Revoke { device: device.to_vec(), epoch: 0, keys: Vec::new(), cards: Vec::new() }));
            return Ok(());
        }
        // The epoch after the newest this device has (§7): the server's
        // number is only checked against it, never added to.
        let newest = self.id.ring.current().0;
        let Some(listed) = self.listed.as_ref() else {
            return Err("the list of devices has not come yet".into());
        };
        // Read before a key came: its cards are the last epoch's, which a
        // device removed since knows too. Read again.
        if listed.at != newest {
            self.list_devices();
            return Err("the list of devices is out of date: wait a moment, then try again".into());
        }
        if !listed.rows.iter().any(|(r, _)| r.id == device) {
            return Err("that device is not in the account".into());
        }
        let Ok(recovery) = &listed.recovery else {
            return Err("the recovery code's card did not open: make a new recovery code first".into());
        };
        let Ok(password) = &listed.password else {
            return Err("the password's card did not open: set the password again first".into());
        };
        if listed.epoch > newest {
            return Err("this device hasn't the account's newest key yet: wait a moment, then try again".into());
        }
        let epoch = newest.checked_add(1).ok_or("this account's keys can't change any more")?;
        let new = VaultKey::generate(rng);
        let signer = Signer::new(&self.id.account, &self.id.device, &self.id.key, &self.id.ring, rng);
        let mut next = self.id.ring.clone();
        next.insert(epoch, new.clone());
        let (mut keys, mut cards) = (Vec::new(), Vec::new());
        for (row, card) in listed.rows.iter().filter(|(r, _)| r.id != device) {
            let Some(card) = card else {
                return Err("a device has not said who it is yet: open Carnet on each of your devices once, then try again".into());
            };
            keys.push((row.id.clone(), signer.seal(&card.boxed, epoch, &new, rng)));
            cards.push((row.id.clone(), card.seal(&next, rng)));
        }
        if let Some(card) = recovery {
            keys.push((Vec::new(), signer.seal(&card.boxed, epoch, &new, rng)));
            cards.push((Vec::new(), card.seal(&next, rng)));
        }
        if let Some(card) = password {
            keys.push((card.device.clone(), signer.seal(&card.boxed, epoch, &new, rng)));
            cards.push((card.device.clone(), card.seal(&next, rng)));
        }
        for (_, card) in &listed.passkeys {
            let Some(card) = card else {
                return Err("a passkey's card did not open: remove that passkey, then try again".into());
            };
            keys.push((card.device.clone(), signer.seal(&card.boxed, epoch, &new, rng)));
            cards.push((card.device.clone(), card.seal(&next, rng)));
        }
        self.send(Frame::Carnet(Carnet::Revoke { device: device.to_vec(), epoch, keys, cards }));
        Ok(())
    }

    /// Asks how the server keeps the account's rooms: the answer comes as
    /// [`Event::Inspected`].
    pub fn inspect(&mut self) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::Inspect));
        }
    }

    /// Whether this account is the server's owner's, and if so its invites
    /// and the accounts: [`Event::Invites`].
    pub fn list_invites(&mut self) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::Invites));
        }
    }

    /// A new invite, for someone to make an account of their own: the
    /// code, made here, to give them; the server keeps only its hash, good
    /// for a week. `None` while not logged in. Only the owner's devices
    /// may: another's is refused. The app keeps the code in the vault
    /// ([`NoteDoc::keep_invite`](crate::NoteDoc::keep_invite)) once this
    /// has gone, so the account's devices can show it again.
    pub fn invite(&mut self, rng: &mut impl Random) -> Option<crate::invite::InviteCode> {
        if self.state != State::Ready {
            return None;
        }
        let code = crate::invite::InviteCode::generate(rng);
        self.send(Frame::Carnet(Carnet::InviteAdd { hash: code.hash() }));
        Some(code)
    }

    /// An invite not used yet, taken back.
    pub fn drop_invite(&mut self, hash: [u8; 32]) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::InviteDrop { hash }));
        }
    }

    /// Another person's account removed from the server, with everything
    /// it holds.
    pub fn remove_account(&mut self, account: &[u8]) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::AccountRemove { account: account.to_vec() }));
        }
    }

    /// Drops `room`'s base on the server, and puts back what it stood for
    /// (§9).
    pub fn roll_back(&mut self, room: &str) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::RollBack { room: room.to_string() }));
        }
    }

    /// Makes this device's copy of `room` the server's base, whatever the
    /// base is now (§9): the server asks for it as a snapshot.
    pub fn repair(&mut self, room: &str) {
        if self.state == State::Ready {
            self.send(Frame::Carnet(Carnet::Repair { room: room.to_string() }));
        }
    }

    /// `room` as one snapshot record, history and all, sealed under the
    /// newest key: what the server asked for (§6). One too big for a
    /// message goes in fragments. One past what a snapshot may weigh
    /// ([`elo::MAX_SNAPSHOT`]) isn't sent, and that's said ([`TOO_BIG`],
    /// once a session); the lease runs out, and it stays as it is.
    fn send_snapshot(&mut self, room: &str, docs: &mut impl Docs, rng: &mut impl Random) {
        if self.joined.get(room) != Some(&Join::Joined) {
            return;
        }
        let doc = docs.doc(room);
        let plain = doc.to_bytes();
        let (epoch, vault) = self.id.ring.current();
        let header = Header::Snapshot { vv: doc.version_vector(), epoch };
        let max = self.max_snapshot;
        let record = (plain.len() <= max).then(|| elo::seal(vault, room, &header, &plain, rng)).filter(|r| r.len() <= max);
        let Some(record) = record else {
            if self.too_big.insert(room.to_string()) {
                self.events.push(Event::Refused(TOO_BIG.into()));
            }
            return;
        };
        self.next_batch += 1;
        let id = BatchId(self.next_batch.to_be_bytes());
        self.unacked.insert(id, room.to_string());
        for m in elo::messages(room, elo::container(&[record]), id) {
            self.send(Frame::Room(m));
        }
    }

    /// What this device is called in the others' lists, changed: its card
    /// goes again now (if logged in, else on the next login), and the list
    /// is asked for again, to show it.
    pub fn rename(&mut self, name: &str, rng: &mut impl Random) {
        self.id.name = name.to_string();
        if self.state == State::Ready {
            let card = self.card(rng);
            self.send(Frame::Carnet(Carnet::SetCard { card }));
            self.list_devices();
        }
    }

    /// This device's card, sealed under the newest key.
    fn card(&self, rng: &mut impl Random) -> Vec<u8> {
        Card {
            device: self.id.device.clone(),
            public: self.id.key.public(),
            boxed: self.id.key.box_secret().public(),
            name: self.id.name.clone(),
            credential: Vec::new(),
        }
        .seal(&self.id.ring, rng)
    }

    /// Each row's card, opened, and kept only if it says it is that row's
    /// device: a server cannot pass one device's card off as another's.
    /// And only under this device's newest key: every card is sealed anew
    /// under each new epoch (the removal's, then each device's own), and
    /// one under an older epoch could be any device's that was in the
    /// account then, the one since removed too. A server handing that one
    /// back, a slot's or a device's, would have the next key sealed to it
    /// (§7). A row whose card isn't under the newest key is one that
    /// hasn't said who it is yet; a slot's, one whose card didn't open.
    fn read_list(&self, epoch: u32, devices: Vec<DeviceRow>, recovery: &[u8], password: &[u8], passkeys: Vec<PasskeyRow>) -> Listed {
        let (at, _) = self.id.ring.current();
        let open = |card: &[u8]| Card::open_at(card, &self.id.ring, at).ok();
        let rows = devices
            .into_iter()
            .map(|row| {
                let card = open(&row.card).filter(|c| c.device == row.id && c.public == row.public);
                (row, card)
            })
            .collect();
        let recovery = if recovery.is_empty() {
            Ok(None)
        } else {
            open(recovery).filter(|c| c.device.is_empty()).map(Some).ok_or(())
        };
        let password = if password.is_empty() {
            Ok(None)
        } else {
            open(password).filter(|c| c.device == crate::password::SLOT).map(Some).ok_or(())
        };
        let passkeys = passkeys
            .into_iter()
            .map(|row| {
                let card = open(&row.card).filter(|c| c.public == row.public && c.device == crate::passkey::slot(&row.public));
                (row, card)
            })
            .collect();
        Listed { epoch, at, rows, recovery, password, passkeys }
    }

    /// Opens a pairing, to let a new device in (§7): the code to show it
    /// comes as [`Event::PairCode`]. One at a time; opening another closes
    /// the last.
    pub fn pair_open(&mut self, rng: &mut impl Random) {
        if self.state != State::Ready {
            return;
        }
        self.pairing = Some(Opener::new(rng));
        self.send(Frame::Carnet(Carnet::PairOpen));
    }

    /// Closes the pairing open, if one is.
    pub fn pair_cancel(&mut self) {
        if self.pairing.take().is_some() {
            self.send(Frame::Carnet(Carnet::PairClose { reason: "cancelled".into() }));
        }
    }

    /// Sets the recovery code (§7): the server keeps the vault key wrapped
    /// under it, and the code's public key, which recovering signs with.
    /// One code at a time: a new one replaces the last.
    pub fn set_recovery(&mut self, code: &RecoveryCode, rng: &mut impl Random) {
        if self.state != State::Ready {
            return;
        }
        let wrapped = self.id.ring.wrap(&code.wrapping_key(), rng);
        let public = code.signing_key().public();
        let card = Card { device: Vec::new(), public, boxed: code.box_secret().public(), name: String::new(), credential: Vec::new() }.seal(&self.id.ring, rng);
        self.send(Frame::Carnet(Carnet::SetRecovery { public, wrapped, card }));
    }

    /// The rooms with changes made here the server isn't known to keep
    /// yet: those [`Session::waiting`] counts.
    pub fn waiting_rooms(&self) -> impl Iterator<Item = &str> {
        self.waiting.iter().map(String::as_str)
    }

    /// Whether the server has said it keeps `room` as `doc` has it now.
    /// Not known (not yet joined since this session began): no.
    pub fn is_kept(&self, room: &str, doc: &NoteDoc) -> bool {
        self.kept.get(room).is_some_and(|k| covers(k, &doc.version_vector()))
    }

    /// Whether the account has a recovery code, by the last list of
    /// devices: `None` before one has come.
    pub fn has_recovery(&self) -> Option<bool> {
        self.listed.as_ref().map(|l| !matches!(l.recovery, Ok(None)))
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// A count that grows each time the account's notes change on the
    /// server as far as this device sees: a write of its own kept, one
    /// from another device taken in, a room removed. What the server says
    /// of them (the inspector, §9) is out of date once it has grown.
    pub fn moves(&self) -> u64 {
        self.moves
    }

    pub fn is_ready(&self) -> bool {
        self.state == State::Ready
    }

    /// Whether `room` is joined: what the server holds of it has come.
    pub fn has_joined(&self, room: &str) -> bool {
        self.joined.get(room) == Some(&Join::Joined)
    }

    /// Logged in, every room joined, everything sent acknowledged.
    pub fn is_idle(&self) -> bool {
        self.state == State::Ready && self.unacked.is_empty() && self.joined.values().all(|j| *j == Join::Joined)
    }

    /// How many rooms (notes, and the library's own) have changes made
    /// here the server isn't known to keep yet: offline, read only, or on
    /// their way. What "Waiting to send" counts; 0 when it all is kept.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    pub fn take_out(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.out)
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// A socket is open: the server speaks first.
    pub fn connected(&mut self) {
        self.state = State::Waiting;
    }

    /// Whether the server said it can do `feature` (`compat::feature`),
    /// as it greeted this device last; before any greeting, no.
    pub fn server_has(&self, feature: &str) -> bool {
        self.server.as_ref().is_some_and(|g| g.has(feature))
    }

    /// The socket is gone, and with it everything in flight. Nothing is
    /// lost: what the server lacks is worked out again on the next join.
    pub fn disconnected(&mut self) {
        self.state = State::Offline;
        if self.pairing.take().is_some() {
            self.events.push(Event::PairFailed("the connection dropped".into()));
        }
        self.joined.clear();
        self.held.clear();
        self.unacked.clear();
        self.incoming.clear();
        // Asked or wanted, and not done: failed, to be tried again. (A put
        // under way still finishes: its answer says it's kept.)
        let mut undone: Vec<BlobId> = self.fetching.drain().collect();
        undone.extend(self.putting.drain().map(|(id, _)| id));
        undone.sort_unstable();
        undone.dedup();
        for id in undone {
            self.events.push(Event::BlobFailed(id));
        }
    }

    /// `room`'s doc changed here: what it has that the server does not
    /// goes out now, if the room is joined (else on joining).
    pub fn saved(&mut self, room: &str, docs: &mut impl Docs, rng: &mut impl Random) {
        if self.removing.contains(room) {
            return;
        }
        // Something the server isn't known to keep: waiting, until it is.
        let vv = docs.doc(room).version_vector();
        if !self.kept.get(room).is_some_and(|k| covers(k, &vv)) {
            self.waiting.insert(room.to_string());
        }
        if self.read_only.contains(room) || self.full.contains(room) {
            return;
        }
        match self.joined.get(room) {
            Some(Join::Joined) => self.send_missing(room, docs, rng),
            Some(Join::Asked) => {}
            None if self.state == State::Ready => self.join(room, docs),
            None => {}
        }
    }

    pub fn receive(&mut self, bytes: &[u8], docs: &mut impl Docs, rng: &mut impl Random) {
        let Ok(frame) = wire::decode(bytes) else { return };
        match frame {
            Frame::Carnet(Carnet::Challenge { nonce, server }) if self.state == State::Waiting => {
                let fit = server.fit();
                self.server = Some(server);
                match fit {
                    Fit::ServerTooOld | Fit::AppTooOld => {
                        self.state = State::Offline;
                        self.events.push(Event::Outdated(fit));
                        return;
                    }
                    Fit::UpdateSoon => self.events.push(Event::Outdated(fit)),
                    Fit::Fine => {}
                }
                let m = login_message(&self.id.origin, &self.id.account, &self.id.device, &nonce);
                self.send(Frame::Carnet(Carnet::Hello {
                    account: self.id.account.clone(),
                    device: self.id.device.clone(),
                    signature: self.id.key.sign(&m),
                    since: self.cursor,
                }));
            }
            Frame::Carnet(Carnet::Welcome { .. }) if self.state == State::Waiting => {
                self.state = State::Ready;
                self.read_only.clear();
                self.full.clear();
                self.events.push(Event::LoggedIn);
                // Who this device is, for the others' lists: its name may
                // have changed, and a new epoch wants it sealed anew.
                let card = self.card(rng);
                self.send(Frame::Carnet(Carnet::SetCard { card }));
                // What was deleted while away, told before the rest.
                for room in self.removing.clone() {
                    self.send(Frame::Carnet(Carnet::RoomRemove { room }));
                }
                let dropping: Vec<BlobId> = self.dropping.iter().copied().collect();
                self.send_drops(&dropping);
                for room in docs.rooms() {
                    if !self.removing.contains(&room) {
                        self.join(&room, docs);
                    }
                }
                // Pictures asked for while logging in.
                let mut ids: Vec<BlobId> = self.fetching.iter().copied().collect();
                ids.sort_unstable();
                for chunk in ids.chunks(wire::MAX_IDS) {
                    self.send(Frame::Carnet(Carnet::BlobFetch { ids: chunk.to_vec() }));
                }
            }
            Frame::Carnet(Carnet::Denied { reason, why }) => {
                self.state = State::Offline;
                self.events.push(match why {
                    Deny::DeviceRemoved => Event::Removed { account: false },
                    Deny::AccountRemoved => Event::Removed { account: true },
                    Deny::Outdated => Event::Outdated(Fit::AppTooOld),
                    Deny::Other => Event::Denied(server_words(&reason)),
                });
            }
            Frame::Carnet(Carnet::Changed { rooms }) if self.state == State::Ready => {
                let mut newest = self.cursor;
                for (room, seq) in rooms {
                    newest = newest.max(seq);
                    if self.removing.contains(&room) {
                        continue;
                    }
                    // A doc made for it now is a room this device will join
                    // on every login from here on: the cursor can pass it.
                    let _ = docs.doc(&room);
                    if !self.joined.contains_key(&room) {
                        self.join(&room, docs);
                    }
                }
                self.cursor = newest;
            }
            Frame::Room(m) => self.room_message(m, docs, rng),
            Frame::Carnet(Carnet::RecoverySet) => {
                self.events.push(Event::RecoverySet);
                // A removal seals the next key to the recovery code too, by
                // the last list: read again, or the server, finding a code
                // the list didn't have, refuses the next one.
                self.list_devices();
            }
            Frame::Carnet(Carnet::PasswordSetReply { response }) => {
                let Some((id, password, state)) = self.setting.take() else { return };
                match crate::password::register_finish(state.0, &id, &password, &response, rng) {
                    Ok((upload, secret)) => {
                        let wrapped = self.id.ring.wrap(&secret.wrapping_key(), rng);
                        let public = secret.signing_key().public();
                        let card = Card { device: crate::password::SLOT.to_vec(), public, boxed: secret.box_secret().public(), name: id.clone(), credential: Vec::new() }
                            .seal(&self.id.ring, rng);
                        self.send(Frame::Carnet(Carnet::PasswordSetFinish { id, upload, public, wrapped, card }));
                    }
                    Err(e) => self.events.push(Event::Refused(format!("{PASSWORD_NOT_SET}{e}"))),
                }
            }
            Frame::Carnet(Carnet::PasswordSet) => {
                self.events.push(Event::PasswordSet);
                // A removal seals the next key to the password too, by the
                // last list: read again.
                self.list_devices();
            }
            Frame::Carnet(Carnet::PasskeySet) => {
                self.events.push(Event::PasskeySet);
                // A removal seals the next key to each passkey too, by the
                // last list: read again.
                self.list_devices();
            }
            Frame::Carnet(Carnet::Devices { epoch, devices, recovery, password, passwords, passkeys, password_email }) if self.state == State::Ready => {
                let listed = self.read_list(epoch, devices, &recovery, &password, passkeys);
                let id = listed.password.as_ref().ok().and_then(|c| c.as_ref()).map(|c| c.name.clone());
                let only = Some(password_email).filter(|e| !e.is_empty());
                self.events.push(Event::Password { allowed: passwords, id, only });
                let keys = listed
                    .passkeys
                    .iter()
                    .map(|(r, c)| PasskeyInfo {
                        id: r.public,
                        name: c.as_ref().map(|c| c.name.clone()),
                        credential: c.as_ref().map(|c| c.credential.clone()).unwrap_or_default(),
                        made: r.made,
                    })
                    .collect();
                self.events.push(Event::Passkeys(keys));
                let infos = listed
                    .rows
                    .iter()
                    .map(|(r, c)| DeviceInfo { id: r.id.clone(), name: c.as_ref().map(|c| c.name.clone()), seen: r.seen, you: r.you, online: r.online })
                    .collect();
                self.listed = Some(listed);
                self.events.push(Event::Devices(infos));
            }
            // A new epoch's key: taken only from a device in the account at
            // this one's newest epoch, for the epoch right after it, never in
            // place of a key it has (`rekey.rs`). One that follows a key not come yet is held
            // until it has: the server sends them in order, but a device
            // away across two removals may be sent them otherwise.
            Frame::Carnet(Carnet::Rekey { sealed }) => {
                if !self.early.contains(&sealed) {
                    self.early.push(sealed);
                }
                let taken = rekey::take_all(&mut self.id.ring, &self.id.account, &self.id.key.box_secret(), &mut self.early);
                if let Some(over) = self.early.len().checked_sub(rekey::MAX_HELD) {
                    self.early.drain(..over);
                }
                for &epoch in &taken {
                    self.events.push(Event::Rekeyed(epoch));
                }
                if !taken.is_empty() && self.state == State::Ready {
                    let card = self.card(rng);
                    self.send(Frame::Carnet(Carnet::SetCard { card }));
                }
            }
            Frame::Carnet(Carnet::Refused { reason }) => self.events.push(Event::Refused(reason)),
            Frame::Carnet(Carnet::Compact { room }) if self.state == State::Ready => {
                self.send_snapshot(&room, docs, rng);
                // Compacted, a room the server took no more of takes more
                // again: joined anew, to send what waited.
                if self.full.remove(&room) {
                    self.join(&room, docs);
                }
            }
            Frame::Carnet(Carnet::Inspected { rooms, log }) => self.events.push(Event::Inspected(rooms, log)),
            Frame::Carnet(Carnet::InviteList { owner, invites, accounts }) => self.events.push(Event::Invites { owner, invites, accounts }),
            // Pictures: where to put one the server wants, or fetch one
            // asked for; kept; or not there. A link not plainly the web's
            // (`link::fair`) is never followed: a put is offered again
            // later, a fetch fails.
            Frame::Carnet(Carnet::BlobLink { id, put, url, .. }) if !crate::link::fair(&self.id.origin, &url) => {
                if !put && self.fetching.remove(&id) {
                    self.events.push(Event::BlobFailed(id));
                }
            }
            Frame::Carnet(Carnet::BlobLink { id, put: true, url, .. }) => {
                self.putting.insert(id, url);
                self.events.push(Event::BlobWanted(id));
            }
            Frame::Carnet(Carnet::BlobLink { id, put: false, url, .. }) => {
                if self.fetching.contains(&id) {
                    self.transfers.push(Transfer::Get { id, url });
                }
            }
            Frame::Carnet(Carnet::BlobStored { id }) => self.events.push(Event::BlobStored(id)),
            Frame::Carnet(Carnet::UsageIs { notes, pictures, limit, writable, plan, until }) => {
                // Asked, or told unasked when the account's limit changed
                // (§11): read only now, or writable again.
                if self.state == State::Ready {
                    if writable {
                        // The rooms joined to read are joined again: the
                        // server lets them be written now, and what waited
                        // here goes (`send_missing` on the answer).
                        let mut rooms: Vec<String> = self.read_only.drain().collect();
                        rooms.sort_unstable();
                        for room in rooms {
                            if self.joined.contains_key(&room) && !self.removing.contains(&room) {
                                self.join(&room, docs);
                            }
                        }
                        if self.limited {
                            self.limited = false;
                            self.events.push(Event::Writable);
                        }
                    } else {
                        // Every room joined is read only from now on: no
                        // write is sent to be refused.
                        let joined: Vec<String> = self.joined.keys().cloned().collect();
                        let new = !self.limited && self.read_only.is_empty();
                        self.read_only.extend(joined);
                        if new {
                            self.limited = true;
                            self.events.push(Event::ReadOnly);
                        }
                    }
                }
                self.events.push(Event::Usage(Usage {
                    notes,
                    pictures,
                    limit: (limit > 0).then_some(limit),
                    writable,
                    plan,
                    until: (until > 0).then_some(until),
                }));
            }
            Frame::Carnet(Carnet::RoomRemoved { room }) => {
                self.moves += 1;
                self.removing.remove(&room);
                self.joined.remove(&room);
                self.held.remove(&room);
                self.waiting.remove(&room);
                self.kept.remove(&room);
                self.events.push(Event::RoomRemoved(room));
            }
            Frame::Carnet(Carnet::BlobDropped { ids }) => {
                for id in &ids {
                    self.dropping.remove(id);
                }
                self.events.push(Event::BlobsDropped(ids));
            }
            Frame::Carnet(Carnet::BlobNone { id }) => {
                if self.fetching.remove(&id) {
                    self.events.push(Event::BlobFailed(id));
                }
            }
            Frame::Carnet(
                c @ (Carnet::PairOpened { .. } | Carnet::PairJoined { .. } | Carnet::PairStep { .. } | Carnet::PairClose { .. }),
            ) => {
                let Some(opener) = self.pairing.as_mut() else { return };
                match opener.receive(&c, &self.id.ring, &self.id.account, rng) {
                    OpenerOut::Send(frames) => self.out.extend(frames),
                    OpenerOut::Code(code) => self.events.push(Event::PairCode(code.text())),
                    OpenerOut::Paired(frames, device) => {
                        self.out.extend(frames);
                        self.pairing = None;
                        self.events.push(Event::Paired(device));
                    }
                    OpenerOut::Failed(reason, frames) => {
                        self.out.extend(frames);
                        self.pairing = None;
                        self.events.push(Event::PairFailed(reason));
                    }
                    OpenerOut::Nothing => {}
                }
            }
            _ => {}
        }
    }

    fn room_message(&mut self, m: ProtocolMessage, docs: &mut impl Docs, rng: &mut impl Random) {
        match m {
            ProtocolMessage::JoinResponseOk { room_id, version, permission, .. } => {
                let held = wire::decode_vv(&version).unwrap_or_default();
                self.held.insert(room_id.clone(), held.into_iter().collect());
                self.joined.insert(room_id.clone(), Join::Joined);
                // Read only (a plan lapsed): what's here waits here.
                if permission == loro_protocol::Permission::Read {
                    // What's here and not there waits; what's there is kept.
                    let vv = docs.doc(&room_id).version_vector();
                    if self.held.get(&room_id).is_some_and(|h| covers(h, &vv)) {
                        self.settle(&room_id, docs);
                    } else {
                        self.waiting.insert(room_id.clone());
                    }
                    if self.read_only.is_empty() {
                        self.limited = true;
                        self.events.push(Event::ReadOnly);
                    }
                    self.read_only.insert(room_id);
                    return;
                }
                self.read_only.remove(&room_id);
                if self.limited && self.read_only.is_empty() {
                    self.limited = false;
                    self.events.push(Event::Writable);
                }
                self.send_missing(&room_id, docs, rng);
            }
            ProtocolMessage::JoinError { room_id, .. } => {
                self.joined.remove(&room_id);
            }
            ProtocolMessage::DocUpdate { room_id, updates, .. } => {
                let mut took = false;
                let mut again = false;
                for container in &updates {
                    let Ok(records) = elo::records(container) else { continue };
                    for record in records {
                        match elo::open(&self.id.ring, &room_id, record) {
                            // A snapshot is opened on its own first (§9,
                            // fork first): taken in only if it holds what
                            // its header says, and the server told either
                            // way.
                            Ok((Header::Snapshot { vv, .. }, plain)) => {
                                let ok = NoteDoc::load(&plain, 0).is_ok_and(|fork| fork.version_vector() == vv);
                                if ok && docs.doc(&room_id).import(&plain).is_ok() {
                                    took = true;
                                    let h = self.held.entry(room_id.clone()).or_default();
                                    for &(peer, c) in &vv {
                                        let e = h.entry(peer).or_default();
                                        *e = (*e).max(c);
                                    }
                                } else {
                                    self.events.push(Event::BadSnapshot(room_id.clone()));
                                    again = true;
                                }
                                self.send(Frame::Carnet(Carnet::Checked { room: room_id.clone(), vv: wire::encode_vv(&vv), ok }));
                            }
                            Ok((header, plain)) => {
                                let doc = docs.doc(&room_id);
                                if doc.import(&plain).is_ok() {
                                    took = true;
                                }
                                // What came from the server, the server holds.
                                if let Header::Delta { peer, end, .. } = header {
                                    let h = self.held.entry(room_id.clone()).or_default().entry(peer).or_default();
                                    *h = (*h).max(end);
                                }
                            }
                            Err(_) => self.events.push(Event::Unreadable(room_id.clone())),
                        }
                    }
                }
                // A bad snapshot told, the room is asked for again: the server
                // has rolled it back by then, and sends what it stood for.
                if again {
                    self.join(&room_id, docs);
                }
                if took {
                    self.moves += 1;
                    self.settle(&room_id, docs);
                    self.events.push(Event::Changed(room_id));
                }
            }
            ProtocolMessage::Ack { ref_id, status, .. } => {
                let room = self.unacked.remove(&ref_id);
                if let Some(room) = &room
                    && status == UpdateStatusCode::Ok
                {
                    self.moves += 1;
                    // Kept: no longer waiting, once all of it is.
                    self.settle(room, docs);
                    // Taken in a room the server took no more of (its
                    // snapshot): it takes more again; what it holds asked.
                    if self.full.remove(room) {
                        self.join(room, docs);
                    }
                }
                if status == UpdateStatusCode::PayloadTooLarge
                    && let Some(room) = &room
                {
                    // More than the server keeps for now: what it holds
                    // is asked again once it takes more.
                    self.held.remove(room);
                    self.full.insert(room.clone());
                } else if status != UpdateStatusCode::Ok
                    && let Some(room) = room
                    && !self.read_only.contains(&room)
                {
                    // Refused: the server's idea of the room is not ours.
                    // Join again, and send what it says it lacks.
                    self.held.remove(&room);
                    self.join(&room, docs);
                }
            }
            ProtocolMessage::RoomError { room_id, .. } => {
                self.joined.remove(&room_id);
                self.join(&room_id, docs);
            }
            // An update in fragments (a big snapshot): put back together,
            // then taken as if it had come whole. One not as said: the room
            // is asked for again. Another begun before it ended is the
            // server sending the room over (joined again): it takes the
            // place of the first, which it sends again.
            ProtocolMessage::DocUpdateFragmentHeader { room_id, batch_id, fragment_count, total_size_bytes, .. } => {
                self.incoming.remove(&room_id);
                match Pieces::start(&room_id, batch_id, fragment_count, total_size_bytes) {
                    Some(p) => {
                        self.incoming.insert(room_id, p);
                    }
                    None => self.join(&room_id, docs),
                }
            }
            ProtocolMessage::DocUpdateFragment { room_id, batch_id, index, fragment, .. } => {
                let Some(p) = self.incoming.get_mut(&room_id) else { return };
                match p.add(&room_id, batch_id, index, &fragment) {
                    Ok(None) => {}
                    Ok(Some(update)) => {
                        self.incoming.remove(&room_id);
                        let m = ProtocolMessage::DocUpdate { crdt: CrdtType::Elo, room_id, updates: vec![update], batch_id };
                        self.room_message(m, docs, rng);
                    }
                    Err(_) => {
                        self.incoming.remove(&room_id);
                        self.join(&room_id, docs);
                    }
                }
            }
            _ => {}
        }
    }

    fn join(&mut self, room: &str, docs: &mut impl Docs) {
        let vv = docs.doc(room).version_vector();
        self.joined.insert(room.to_string(), Join::Asked);
        self.send(Frame::Room(ProtocolMessage::JoinRequest {
            crdt: CrdtType::Elo,
            room_id: room.to_string(),
            auth: Vec::new(),
            version: wire::encode_vv(&vv),
        }));
    }

    /// Every peer's operations this doc has past what the server holds,
    /// sealed, in as many updates as their size takes: a span too big for
    /// one record is cut in halves until each piece fits.
    fn send_missing(&mut self, room: &str, docs: &mut impl Docs, rng: &mut impl Random) {
        let doc = docs.doc(room);
        let (epoch, vault) = self.id.ring.current();
        let held = self.held.entry(room.to_string()).or_default();
        let mut records = Vec::new();
        for (peer, end) in doc.version_vector() {
            let start = held.get(&peer).copied().unwrap_or(0);
            if start < end {
                for (a, b, plain) in pieces(doc, peer, start, end) {
                    let header = Header::Delta { peer, start: a, end: b, epoch };
                    records.push(elo::seal(vault, room, &header, &plain, rng));
                }
                held.insert(peer, end);
            }
        }
        // Sent: waiting until it's said kept. Nothing to send: kept.
        if records.is_empty() {
            self.settle(room, docs);
        } else {
            self.waiting.insert(room.to_string());
        }
        for batch in elo::batches(records) {
            self.next_batch += 1;
            let id = BatchId(self.next_batch.to_be_bytes());
            self.unacked.insert(id, room.to_string());
            self.send(Frame::Room(ProtocolMessage::DocUpdate {
                crdt: CrdtType::Elo,
                room_id: room.to_string(),
                updates: vec![elo::container(&batch)],
                batch_id: id,
            }));
        }
    }

    /// If the server keeps all `room` has here (nothing of it on its way,
    /// and what it holds covers the doc), it's no longer waiting, and
    /// that's remembered.
    fn settle(&mut self, room: &str, docs: &mut impl Docs) {
        if self.unacked.values().any(|r| r == room) {
            return;
        }
        let Some(held) = self.held.get(room) else { return };
        let vv = docs.doc(room).version_vector();
        if covers(held, &vv) {
            self.kept.insert(room.to_string(), vv.into_iter().collect());
            self.waiting.remove(room);
        }
    }

    fn send(&mut self, f: Frame) {
        self.out.push(wire::encode(&f));
    }
}

/// Whether `have` holds every peer's operations `vv` says there are.
fn covers(have: &HashMap<u64, u64>, vv: &[(u64, u64)]) -> bool {
    vv.iter().all(|(peer, end)| have.get(peer).copied().unwrap_or(0) >= *end)
}

/// `peer`'s operations `start..end` in spans whose changes each fit a
/// record ([`elo::MAX_PLAIN`]). An operation is a letter or so, so halving
/// always ends.
fn pieces(doc: &NoteDoc, peer: u64, start: u64, end: u64) -> Vec<(u64, u64, Vec<u8>)> {
    let plain = doc.span(peer, start, end);
    if plain.len() <= elo::MAX_PLAIN || end - start < 2 {
        return vec![(start, end, plain)];
    }
    let mid = start + (end - start) / 2;
    let mut out = pieces(doc, peer, start, mid);
    out.extend(pieces(doc, peer, mid, end));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a server says is shown as its own, on one line, cut short,
    /// with nothing in it that hides or turns letters around.
    #[test]
    fn the_servers_words_are_shown_as_its_own() {
        assert_eq!(server_says("only the server's owner can do that"), "The server says: only the server's owner can do that");
        assert_eq!(server_words("two\nlines\t and\u{202e}evil\u{200b} "), "two lines andevil");
        let long = server_words(&"é".repeat(1000));
        assert_eq!(long.chars().count(), SAID_MAX + 1);
        assert!(long.ends_with('…'));
        assert_eq!(server_words(""), "");
        // A refusal: the server's own, marked; the ones the apps know, and
        // this device's own words, as they are.
        assert_eq!(refusal_words("only the owner can"), "The server says: only the owner can");
        assert_eq!(refusal_words(FULL), FULL);
        assert_eq!(refusal_words("The password couldn't be set: no"), "The password couldn't be set: no");
    }
}
