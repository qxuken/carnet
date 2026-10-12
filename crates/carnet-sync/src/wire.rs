//! What goes over a device's one WebSocket (docs/SYNC.md §6): the Loro
//! Protocol's room messages, for each doc, and Carnet's own frames, for
//! logging in and for the vault's feed of what changed. A Loro Protocol
//! message starts with its CRDT's magic (`%ELO`); a Carnet frame with
//! `%CRN`, so one socket carries both and either side tells them apart
//! by their first four bytes.

use loro_protocol::{BytesReader, BytesWriter, ProtocolMessage};

use crate::compat::Greeting;

pub use loro_protocol::{BatchId, CrdtType, Permission, UpdateStatusCode};

const CARNET: &[u8; 4] = b"%CRN";

/// Why a device was turned away, for the app to act on whatever the
/// words say. A later protocol's reason reads as `Other`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Deny {
    #[default]
    Other,
    /// This device was taken out of its account.
    DeviceRemoved,
    /// The account is gone from the server.
    AccountRemoved,
    /// The app is older than the server takes (`compat::OLDEST`).
    Outdated,
}

impl Deny {
    fn code(self) -> u8 {
        match self {
            Deny::Other => 0,
            Deny::DeviceRemoved => 1,
            Deny::AccountRemoved => 2,
            Deny::Outdated => 3,
        }
    }

    fn of(code: u8) -> Self {
        match code {
            1 => Deny::DeviceRemoved,
            2 => Deny::AccountRemoved,
            3 => Deny::Outdated,
            _ => Deny::Other,
        }
    }
}

/// A device's id and its sealed bytes (a key, or a card).
type Pair = (Vec<u8>, Vec<u8>);

/// Carnet's own frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Carnet {
    /// Server → device, first thing: sign this to log in (§7); and the
    /// server's protocol and features, for the app to see whether they
    /// talk (`compat`, §14).
    Challenge { nonce: [u8; 32], server: Greeting },
    /// Device → server: who it is, its signature over the challenge, and
    /// the last vault `seq` it has applied.
    Hello { account: Vec<u8>, device: Vec<u8>, signature: [u8; 64], since: u64 },
    /// Server → device: logged in; the vault's newest `seq`.
    Welcome { seq: u64 },
    /// Server → device: not logged in, and why: in words, and as a
    /// [`Deny`] the app acts on. The socket closes.
    Denied { reason: String, why: Deny },
    /// Server → device: rooms with records past the device's `since` (or
    /// one just written by another device), each with its newest `seq`.
    Changed { rooms: Vec<(String, u64)> },

    // Pairing (§7): a device already in lets a new one in. The server
    // relays; what it relays it cannot read or change unnoticed.
    /// Device in → server: open a pairing.
    PairOpen,
    /// Server → device in: its nameplate, the number the code starts with.
    PairOpened { nameplate: u32 },
    /// New device → server (logged in it is not): join the pairing with
    /// this nameplate, with the new device's PAKE message.
    PairJoin { nameplate: u32, pake: Vec<u8> },
    /// Server → device in: a device joined, with its PAKE message.
    PairJoined { pake: Vec<u8> },
    /// Either side → server → the other: the pairing's next message, which
    /// the server passes on as it is.
    PairStep { data: Vec<u8> },
    /// Device in → server: let this device into the account.
    PairApprove { device: Vec<u8>, public: [u8; 32] },
    /// Either side, or the server: the pairing is over, and why.
    PairClose { reason: String },

    // Recovery (§7): the 12 words, when no device is left.
    /// Device in → server: the key ring wrapped under the recovery code,
    /// the code's public key, which recovering signs with, and the code's
    /// card (its box key, sealed), what a new epoch's key is sealed to.
    SetRecovery { public: [u8; 32], wrapped: Vec<u8>, card: Vec<u8> },
    /// Server → device in: kept.
    RecoverySet,
    /// New device → server (logged in it is not): the recovery key's
    /// signature over the challenge and this device, which it lets in.
    Recover { public: [u8; 32], signature: [u8; 64], device: Vec<u8>, device_public: [u8; 32] },
    /// Server → new device: its account, the key ring wrapped, and each
    /// epoch's key made since, sealed to the code's box key. A fresh
    /// challenge follows, to log in with.
    Recovered { account: Vec<u8>, wrapped: Vec<u8>, rekeys: Vec<Vec<u8>> },

    // Enrolling (§6): an invite redeemed over the socket, so a device
    // needs no HTTP client to begin.
    /// New device → server (logged in it is not): this invite, for a new
    /// account whose first device this is.
    Enroll { invite: String, device: Vec<u8>, public: [u8; 32] },
    /// Server → new device: the account made. A fresh challenge follows.
    Enrolled { account: Vec<u8> },

    // Devices (§7): what is in the account, and taking one out.
    /// Device → server: its card (name and keys, sealed under the vault
    /// key), sent on each login.
    SetCard { card: Vec<u8> },
    /// Device → server: the account's devices, please.
    ListDevices,
    /// Server → device: the devices not removed, the account's epoch, the
    /// recovery code's card and the password's (each empty if none),
    /// whether this server takes passwords at all, the passkeys, and the
    /// one email a password may sign in as here (`password_email`: the
    /// email the account signed up with, on a hosted service whose
    /// passwords sign in by it alone; empty: any). Told only to the
    /// account's own devices.
    Devices { epoch: u32, devices: Vec<DeviceRow>, recovery: Vec<u8>, password: Vec<u8>, passwords: bool, passkeys: Vec<PasskeyRow>, password_email: String },
    /// Device → server: take `device` out. Another device: `epoch` is the
    /// next, and `keys` its vault key sealed to each device kept (and to
    /// the recovery code, under an empty id), `cards` theirs sealed under
    /// it. This device itself: it leaves, and nothing else changes.
    Revoke { device: Vec<u8>, epoch: u32, keys: Vec<(Vec<u8>, Vec<u8>)>, cards: Vec<(Vec<u8>, Vec<u8>)> },
    /// Server → device: an epoch's vault key, sealed to its box key.
    Rekey { sealed: Vec<u8> },
    /// Server → device: what it asked was not done, and why. Unlike
    /// `Denied`, the socket stays open.
    Refused { reason: String },

    // Compaction and repair (§6, §9).
    /// Server → device: send `room` as one snapshot, to stand for its
    /// records: this device holds the lease on it for a while.
    Compact { room: String },
    /// Device → server: a snapshot of `room` it was sent, whose header
    /// said `vv`, opened (`ok`) or found not to hold what it says.
    Checked { room: String, vv: Vec<u8>, ok: bool },
    /// Device → server: what the account's rooms hold, please.
    Inspect,
    /// Server → device: each room as the server keeps it, and what was
    /// done to them lately.
    Inspected { rooms: Vec<RoomRow>, log: Vec<LogRow> },
    /// Device → server: drop `room`'s base and put back what it stood for.
    RollBack { room: String },
    /// Device → server: this device's copy of `room` is good; take a
    /// snapshot of it as the base, whatever the base is now.
    Repair { room: String },

    // Pictures (§4, §7): sealed blobs, by id. The bytes go over HTTP, to
    /// and from a short-lived signed URL the server gives here; the socket
    /// says only which, and when they're kept.
    /// Device → server: does it have these? Each comes back as
    /// `BlobStored`, or a `BlobLink` to put it at.
    BlobOffer { ids: Vec<[u8; 32]> },
    /// Device → server: links to fetch these. Each comes back as a
    /// `BlobLink`, or `BlobNone`.
    BlobFetch { ids: Vec<[u8; 32]> },
    /// Server → device: where to `PUT` (`put`) or `GET` a blob, good until
    /// `expires` (seconds since 1970).
    BlobLink { id: [u8; 32], put: bool, url: String, expires: u64 },
    /// Server → device: the blob is kept, whole.
    BlobStored { id: [u8; 32] },
    /// Server → device: it doesn't have that blob.
    BlobNone { id: [u8; 32] },
    /// Device → server: no note shows these any more. The server keeps
    /// each a while longer (30 days), in case a device still wants it:
    /// offered, asked for or put in that time, it stays. `BlobDropped`
    /// back.
    BlobDrop { ids: Vec<[u8; 32]> },
    /// Server → device: those `BlobDrop` named, marked.
    BlobDropped { ids: Vec<[u8; 32]> },

    // What the account keeps, and may (§11).
    /// Device → server: how much is kept, and the limit. `UsageIs` back.
    Usage,
    /// Server → device: bytes kept for notes and for pictures (sealed);
    /// the limit (0: none); whether it may write; its plan and until when
    /// (0: no end).
    UsageIs { notes: u64, pictures: u64, limit: u64, writable: bool, plan: String, until: u64 },

    // Email and password (§7, password.rs): OPAQUE's messages relayed.
    /// Device → server: set (or change) this account's password, signing
    /// in as `id` (an email). `PasswordSetReply` back, or `Refused`.
    PasswordSetStart { id: String, request: Vec<u8> },
    PasswordSetReply { response: Vec<u8> },
    /// Device → server: OPAQUE's record to keep, and the slot the password
    /// opens: its signing key, the vault key wrapped under it, its card.
    /// `PasswordSet` back.
    PasswordSetFinish { id: String, upload: Vec<u8>, public: [u8; 32], wrapped: Vec<u8>, card: Vec<u8> },
    PasswordSet,
    /// Device → server: no password any more. `PasswordSet` back.
    PasswordDrop,
    /// A new device → server, before any login: signing in as `id`.
    /// `PasswordLoginReply` back (made up for an unknown `id`, and alike);
    /// then a `Recover` signed with what the password comes to.
    PasswordLogin { id: String, request: Vec<u8> },
    PasswordLoginReply { response: Vec<u8> },

    /// Device → server: delete this account, everything the server keeps
    /// of it, now. Its devices are let go (`Denied`), this one too.
    AccountDelete,

    /// A frame of a later protocol, by its tag: skipped by whoever gets
    /// it (§14). Only ever decoded.
    Unknown { tag: u8 },
    // Passkeys (§7, passkey.rs): each a slot, as the recovery words are.
    /// Device → server: a passkey's slot: its signing key, the key ring
    /// wrapped under it, its card. `PasskeySet` back, or `Refused`.
    PasskeyAdd { public: [u8; 32], wrapped: Vec<u8>, card: Vec<u8> },
    /// Device → server: the passkey whose slot signs with `public` taken
    /// away. `PasskeySet` back.
    PasskeyDrop { public: [u8; 32] },
    PasskeySet,

    // Notes deleted forever (§4).
    /// Device → server: this room's note is deleted forever: its records
    /// go, and the room is never taken again. `RoomRemoved` back.
    RoomRemove { room: String },
    /// Server → device: the room is gone: removed now, or asked for (to
    /// join, or by `RoomRemove`) after it was.
    RoomRemoved { room: String },

    // Invites (§6): the owner's devices let new people onto the server.
    /// Device → server: the invites not used yet, and the accounts.
    Invites,
    /// Server → device: whether its account is the owner's; if it is, the
    /// invites not used yet and every account. Another's gets neither.
    InviteList { owner: bool, invites: Vec<InviteRow>, accounts: Vec<AccountRow> },
    /// Owner's device → server: keep this invite, by its code's hash, good
    /// for a week. The device made the code; the server never sees it.
    InviteAdd { hash: [u8; 32] },
    /// Owner's device → server: take back this invite, not used yet.
    InviteDrop { hash: [u8; 32] },
    /// Owner's device → server: remove this account, and all it holds.
    AccountRemove { account: Vec<u8> },
}

/// An invite in an [`Carnet::InviteList`]: its hash, and when it stops
/// being good, in seconds since 1970.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InviteRow {
    pub hash: [u8; 32],
    pub until: u64,
}

/// An account in an [`Carnet::InviteList`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountRow {
    pub id: Vec<u8>,
    pub owner: bool,
    /// When it was made, and when one of its devices last logged in.
    pub made: u64,
    pub seen: u64,
    /// Its devices not removed.
    pub devices: u64,
    /// The account asking.
    pub you: bool,
}

/// A room in an [`Carnet::Inspected`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomRow {
    pub room: String,
    /// What the server holds of it, by peer, as a join would say.
    pub held: Vec<(u64, u64)>,
    /// Records past the base, and their bytes.
    pub live: u64,
    pub live_bytes: u64,
    pub base: Option<BaseRow>,
    /// Records set aside by a base, their bytes, and since when.
    pub shelved: u64,
    pub shelved_bytes: u64,
    pub shelved_since: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseRow {
    pub by: Vec<u8>,
    pub at: u64,
    pub verified: bool,
    pub undoable: bool,
    pub vv: Vec<(u64, u64)>,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRow {
    pub at: u64,
    pub device: Vec<u8>,
    pub room: String,
    pub what: String,
}

/// A passkey in a [`Carnet::Devices`] list: its slot's signing key, which
/// names it; its card, sealed (its name in it); when it was added, in
/// seconds since 1970.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasskeyRow {
    pub public: [u8; 32],
    pub card: Vec<u8>,
    pub made: u64,
}

/// A device in a [`Carnet::Devices`] list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: Vec<u8>,
    pub public: [u8; 32],
    /// Its card, sealed; empty until it has logged in once to send one.
    pub card: Vec<u8>,
    /// When it last logged in, in seconds since 1970.
    pub seen: u64,
    /// The device asking.
    pub you: bool,
    /// Logged in now (on another socket, or this one). Said at the end of
    /// the list (§14): from a server from before, false.
    pub online: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Room(ProtocolMessage),
    Carnet(Carnet),
}

pub fn encode(frame: &Frame) -> Vec<u8> {
    match frame {
        Frame::Room(m) => loro_protocol::encode(m).expect("a message this side made encodes"),
        Frame::Carnet(c) => {
            let mut w = BytesWriter::new();
            w.push_bytes(CARNET);
            match c {
                Carnet::Challenge { nonce, server } => {
                    w.push_byte(1);
                    w.push_bytes(nonce);
                    w.push_uleb128(server.protocol.into());
                    w.push_uleb128(server.oldest.into());
                    w.push_uleb128(server.deprecated.into());
                    w.push_uleb128(server.features.len() as u64);
                    for f in &server.features {
                        w.push_var_string(f);
                    }
                }
                Carnet::Hello { account, device, signature, since } => {
                    w.push_byte(2);
                    w.push_var_bytes(account);
                    w.push_var_bytes(device);
                    w.push_bytes(signature);
                    w.push_uleb128(*since);
                }
                Carnet::Welcome { seq } => {
                    w.push_byte(3);
                    w.push_uleb128(*seq);
                }
                Carnet::Denied { reason, why } => {
                    w.push_byte(4);
                    w.push_var_string(reason);
                    w.push_byte(why.code());
                }
                Carnet::Changed { rooms } => {
                    w.push_byte(5);
                    w.push_uleb128(rooms.len() as u64);
                    for (room, seq) in rooms {
                        w.push_var_string(room);
                        w.push_uleb128(*seq);
                    }
                }
                Carnet::PairOpen => w.push_byte(6),
                Carnet::PairOpened { nameplate } => {
                    w.push_byte(7);
                    w.push_uleb128(u64::from(*nameplate));
                }
                Carnet::PairJoin { nameplate, pake } => {
                    w.push_byte(8);
                    w.push_uleb128(u64::from(*nameplate));
                    w.push_var_bytes(pake);
                }
                Carnet::PairJoined { pake } => {
                    w.push_byte(9);
                    w.push_var_bytes(pake);
                }
                Carnet::PairStep { data } => {
                    w.push_byte(10);
                    w.push_var_bytes(data);
                }
                Carnet::PairApprove { device, public } => {
                    w.push_byte(11);
                    w.push_var_bytes(device);
                    w.push_bytes(public);
                }
                Carnet::PairClose { reason } => {
                    w.push_byte(12);
                    w.push_var_string(reason);
                }
                Carnet::SetRecovery { public, wrapped, card } => {
                    w.push_byte(13);
                    w.push_bytes(public);
                    w.push_var_bytes(wrapped);
                    w.push_var_bytes(card);
                }
                Carnet::RecoverySet => w.push_byte(14),
                Carnet::Recover { public, signature, device, device_public } => {
                    w.push_byte(15);
                    w.push_bytes(public);
                    w.push_bytes(signature);
                    w.push_var_bytes(device);
                    w.push_bytes(device_public);
                }
                Carnet::Recovered { account, wrapped, rekeys } => {
                    w.push_byte(16);
                    w.push_var_bytes(account);
                    w.push_var_bytes(wrapped);
                    w.push_uleb128(rekeys.len() as u64);
                    for r in rekeys {
                        w.push_var_bytes(r);
                    }
                }
                Carnet::Enroll { invite, device, public } => {
                    w.push_byte(17);
                    w.push_var_string(invite);
                    w.push_var_bytes(device);
                    w.push_bytes(public);
                }
                Carnet::Enrolled { account } => {
                    w.push_byte(18);
                    w.push_var_bytes(account);
                }
                Carnet::SetCard { card } => {
                    w.push_byte(19);
                    w.push_var_bytes(card);
                }
                Carnet::ListDevices => w.push_byte(20),
                Carnet::Devices { epoch, devices, recovery, password, passwords, passkeys, password_email } => {
                    w.push_byte(21);
                    w.push_uleb128(u64::from(*epoch));
                    w.push_uleb128(devices.len() as u64);
                    for d in devices {
                        w.push_var_bytes(&d.id);
                        w.push_bytes(&d.public);
                        w.push_var_bytes(&d.card);
                        w.push_uleb128(d.seen);
                        w.push_byte(u8::from(d.you));
                    }
                    w.push_var_bytes(recovery);
                    w.push_var_bytes(password);
                    w.push_byte(u8::from(*passwords));
                    w.push_uleb128(passkeys.len() as u64);
                    for p in passkeys {
                        w.push_bytes(&p.public);
                        w.push_var_bytes(&p.card);
                        w.push_uleb128(p.made);
                    }
                    // Added at the end (§14), and only when there's one: a
                    // list without it is written as before it was. The
                    // devices online now after it, likewise (by their place
                    // in the list), the email written, if empty, before them.
                    let online: Vec<u64> = devices.iter().enumerate().filter(|(_, d)| d.online).map(|(i, _)| i as u64).collect();
                    if !password_email.is_empty() || !online.is_empty() {
                        w.push_var_string(password_email);
                    }
                    if !online.is_empty() {
                        w.push_uleb128(online.len() as u64);
                        for i in online {
                            w.push_uleb128(i);
                        }
                    }
                }
                Carnet::Revoke { device, epoch, keys, cards } => {
                    w.push_byte(22);
                    w.push_var_bytes(device);
                    w.push_uleb128(u64::from(*epoch));
                    for pairs in [keys, cards] {
                        w.push_uleb128(pairs.len() as u64);
                        for (id, b) in pairs {
                            w.push_var_bytes(id);
                            w.push_var_bytes(b);
                        }
                    }
                }
                Carnet::Rekey { sealed } => {
                    w.push_byte(23);
                    w.push_var_bytes(sealed);
                }
                Carnet::Refused { reason } => {
                    w.push_byte(24);
                    w.push_var_string(reason);
                }
                Carnet::Compact { room } => {
                    w.push_byte(25);
                    w.push_var_string(room);
                }
                Carnet::Checked { room, vv, ok } => {
                    w.push_byte(26);
                    w.push_var_string(room);
                    w.push_var_bytes(vv);
                    w.push_byte(u8::from(*ok));
                }
                Carnet::Inspect => w.push_byte(27),
                Carnet::Inspected { rooms, log } => {
                    w.push_byte(28);
                    w.push_uleb128(rooms.len() as u64);
                    for r in rooms {
                        w.push_var_string(&r.room);
                        w.push_var_bytes(&encode_vv(&r.held));
                        for n in [r.live, r.live_bytes, r.shelved, r.shelved_bytes, r.shelved_since] {
                            w.push_uleb128(n);
                        }
                        match &r.base {
                            None => w.push_byte(0),
                            Some(b) => {
                                w.push_byte(1);
                                w.push_var_bytes(&b.by);
                                w.push_uleb128(b.at);
                                w.push_byte(u8::from(b.verified) | u8::from(b.undoable) << 1);
                                w.push_var_bytes(&encode_vv(&b.vv));
                                w.push_uleb128(b.bytes);
                            }
                        }
                    }
                    w.push_uleb128(log.len() as u64);
                    for l in log {
                        w.push_uleb128(l.at);
                        w.push_var_bytes(&l.device);
                        w.push_var_string(&l.room);
                        w.push_var_string(&l.what);
                    }
                }
                Carnet::RollBack { room } => {
                    w.push_byte(29);
                    w.push_var_string(room);
                }
                Carnet::Repair { room } => {
                    w.push_byte(30);
                    w.push_var_string(room);
                }
                Carnet::BlobOffer { ids } | Carnet::BlobFetch { ids } | Carnet::BlobDrop { ids } | Carnet::BlobDropped { ids } => {
                    w.push_byte(match c {
                        Carnet::BlobOffer { .. } => 42,
                        Carnet::BlobFetch { .. } => 43,
                        Carnet::BlobDrop { .. } => 46,
                        _ => 47,
                    });
                    w.push_uleb128(ids.len() as u64);
                    for id in ids {
                        w.push_bytes(id);
                    }
                }
                Carnet::BlobLink { id, put, url, expires } => {
                    w.push_byte(44);
                    w.push_bytes(id);
                    w.push_byte(u8::from(*put));
                    w.push_var_string(url);
                    w.push_uleb128(*expires);
                }
                Carnet::BlobStored { id } | Carnet::BlobNone { id } => {
                    w.push_byte(match c {
                        Carnet::BlobStored { .. } => 34,
                        Carnet::BlobNone { .. } => 36,
                        _ => 45,
                    });
                    w.push_bytes(id);
                }
                Carnet::RoomRemove { room } | Carnet::RoomRemoved { room } => {
                    w.push_byte(if matches!(c, Carnet::RoomRemove { .. }) { 48 } else { 49 });
                    w.push_var_string(room);
                }
                Carnet::Usage => w.push_byte(50),
                Carnet::AccountDelete => w.push_byte(70),
                Carnet::Unknown { tag } => w.push_byte(*tag),
                Carnet::PasswordSetStart { id, request } | Carnet::PasswordLogin { id, request } => {
                    w.push_byte(if matches!(c, Carnet::PasswordSetStart { .. }) { 52 } else { 57 });
                    w.push_var_string(id);
                    w.push_var_bytes(request);
                }
                Carnet::PasswordSetReply { response } | Carnet::PasswordLoginReply { response } => {
                    w.push_byte(if matches!(c, Carnet::PasswordSetReply { .. }) { 53 } else { 58 });
                    w.push_var_bytes(response);
                }
                Carnet::PasswordSetFinish { id, upload, public, wrapped, card } => {
                    w.push_byte(54);
                    w.push_var_string(id);
                    w.push_var_bytes(upload);
                    w.push_bytes(public);
                    w.push_var_bytes(wrapped);
                    w.push_var_bytes(card);
                }
                Carnet::PasswordSet => w.push_byte(55),
                Carnet::PasskeyAdd { public, wrapped, card } => {
                    w.push_byte(59);
                    w.push_bytes(public);
                    w.push_var_bytes(wrapped);
                    w.push_var_bytes(card);
                }
                Carnet::PasskeyDrop { public } => {
                    w.push_byte(60);
                    w.push_bytes(public);
                }
                Carnet::PasskeySet => w.push_byte(61),
                Carnet::PasswordDrop => w.push_byte(56),
                Carnet::UsageIs { notes, pictures, limit, writable, plan, until } => {
                    w.push_byte(51);
                    w.push_uleb128(*notes);
                    w.push_uleb128(*pictures);
                    w.push_uleb128(*limit);
                    w.push_byte(u8::from(*writable));
                    w.push_var_string(plan);
                    w.push_uleb128(*until);
                }
                Carnet::Invites => w.push_byte(37),
                Carnet::InviteList { owner, invites, accounts } => {
                    w.push_byte(38);
                    w.push_byte(u8::from(*owner));
                    w.push_uleb128(invites.len() as u64);
                    for i in invites {
                        w.push_bytes(&i.hash);
                        w.push_uleb128(i.until);
                    }
                    w.push_uleb128(accounts.len() as u64);
                    for a in accounts {
                        w.push_var_bytes(&a.id);
                        w.push_byte(u8::from(a.owner) | (u8::from(a.you) << 1));
                        w.push_uleb128(a.made);
                        w.push_uleb128(a.seen);
                        w.push_uleb128(a.devices);
                    }
                }
                Carnet::InviteAdd { hash } | Carnet::InviteDrop { hash } => {
                    w.push_byte(if matches!(c, Carnet::InviteAdd { .. }) { 39 } else { 40 });
                    w.push_bytes(hash);
                }
                Carnet::AccountRemove { account } => {
                    w.push_byte(41);
                    w.push_var_bytes(account);
                }
            }
            w.finalize()
        }
    }
}

/// A count that the bytes left could hold, at `each` bytes an item at
/// least. The Loro Protocol's decoders make room for whatever count they
/// read before reading the items: four bytes claiming four billion would
/// ask for gigabytes (found by fuzzing). Every count from a socket is
/// held to what follows it first.
pub(crate) fn plausible(r: &mut BytesReader<'_>, each: usize) -> Result<(), String> {
    let n = r.read_uleb128()?;
    if n > (r.remaining() / each.max(1)) as u64 {
        return Err("a count past what follows it".into());
    }
    Ok(())
}

/// A Loro Protocol message whose counts are plausible: the one with a
/// count (`DocUpdate`, type 3) has no more updates than bytes for them.
fn plausible_message(bytes: &[u8]) -> Result<(), String> {
    let mut r = BytesReader::new(bytes);
    r.read_bytes(4)?;
    r.read_var_bytes()?;
    if r.read_byte()? == 3 {
        plausible(&mut r, 1)?;
    }
    Ok(())
}

pub fn decode(bytes: &[u8]) -> Result<Frame, String> {
    if !bytes.starts_with(CARNET) {
        plausible_message(bytes)?;
        return loro_protocol::decode(bytes).map(Frame::Room);
    }
    let mut r = BytesReader::new(&bytes[4..]);
    let fixed = |r: &mut BytesReader<'_>, n: usize| r.read_bytes(n).map(<[u8]>::to_vec);
    let c = match r.read_byte()? {
        1 => Carnet::Challenge {
            nonce: fixed(&mut r, 32)?.try_into().expect("32"),
            // A server before greetings sent the nonce alone.
            server: if r.remaining() == 0 { Greeting::default() } else { greeting(&mut r)? },
        },
        2 => Carnet::Hello {
            account: r.read_var_bytes()?.to_vec(),
            device: r.read_var_bytes()?.to_vec(),
            signature: fixed(&mut r, 64)?.try_into().expect("64"),
            since: r.read_uleb128()?,
        },
        3 => Carnet::Welcome { seq: r.read_uleb128()? },
        4 => Carnet::Denied {
            reason: r.read_var_string()?,
            why: if r.remaining() == 0 { Deny::Other } else { Deny::of(r.read_byte()?) },
        },
        5 => {
            let n = r.read_uleb128()?;
            // Each room at least two bytes: a count claiming more is lying.
            if n > r.remaining() as u64 / 2 {
                return Err("room count past the frame's end".into());
            }
            let mut rooms = Vec::with_capacity(n as usize);
            for _ in 0..n {
                rooms.push((r.read_var_string()?, r.read_uleb128()?));
            }
            Carnet::Changed { rooms }
        }
        6 => Carnet::PairOpen,
        7 => Carnet::PairOpened { nameplate: nameplate(&mut r)? },
        8 => Carnet::PairJoin { nameplate: nameplate(&mut r)?, pake: r.read_var_bytes()?.to_vec() },
        9 => Carnet::PairJoined { pake: r.read_var_bytes()?.to_vec() },
        10 => Carnet::PairStep { data: r.read_var_bytes()?.to_vec() },
        11 => Carnet::PairApprove { device: r.read_var_bytes()?.to_vec(), public: fixed(&mut r, 32)?.try_into().expect("32") },
        12 => Carnet::PairClose { reason: r.read_var_string()? },
        13 => Carnet::SetRecovery {
            public: fixed(&mut r, 32)?.try_into().expect("32"),
            wrapped: r.read_var_bytes()?.to_vec(),
            card: r.read_var_bytes()?.to_vec(),
        },
        14 => Carnet::RecoverySet,
        15 => Carnet::Recover {
            public: fixed(&mut r, 32)?.try_into().expect("32"),
            signature: fixed(&mut r, 64)?.try_into().expect("64"),
            device: r.read_var_bytes()?.to_vec(),
            device_public: fixed(&mut r, 32)?.try_into().expect("32"),
        },
        16 => {
            let account = r.read_var_bytes()?.to_vec();
            let wrapped = r.read_var_bytes()?.to_vec();
            let n = count(&mut r, 1)?;
            let rekeys = (0..n).map(|_| r.read_var_bytes().map(<[u8]>::to_vec)).collect::<Result<_, _>>()?;
            Carnet::Recovered { account, wrapped, rekeys }
        }
        17 => Carnet::Enroll {
            invite: r.read_var_string()?,
            device: r.read_var_bytes()?.to_vec(),
            public: fixed(&mut r, 32)?.try_into().expect("32"),
        },
        18 => Carnet::Enrolled { account: r.read_var_bytes()?.to_vec() },
        19 => Carnet::SetCard { card: r.read_var_bytes()?.to_vec() },
        20 => Carnet::ListDevices,
        21 => {
            let epoch = epoch(&mut r)?;
            // A row is 36 bytes at least: an id's length, a key, a card's
            // length, a time, a flag.
            let n = count(&mut r, 36)?;
            let mut devices = Vec::with_capacity(n);
            for _ in 0..n {
                devices.push(DeviceRow {
                    id: r.read_var_bytes()?.to_vec(),
                    public: fixed(&mut r, 32)?.try_into().expect("32"),
                    card: r.read_var_bytes()?.to_vec(),
                    seen: r.read_uleb128()?,
                    you: r.read_byte()? != 0,
                    online: false,
                });
            }
            let recovery = r.read_var_bytes()?.to_vec();
            let password = r.read_var_bytes()?.to_vec();
            let passwords = r.read_byte()? != 0;
            // A server from before passkeys says none.
            let mut passkeys = Vec::new();
            if r.remaining() > 0 {
                // A row is 34 bytes at least: a key, a card's length, a time.
                let n = count(&mut r, 34)?;
                for _ in 0..n {
                    passkeys.push(PasskeyRow { public: fixed(&mut r, 32)?.try_into().expect("32"), card: r.read_var_bytes()?.to_vec(), made: r.read_uleb128()? });
                }
            }
            // A server from before password emails: any.
            let password_email = if r.remaining() > 0 { r.read_var_string()? } else { String::new() };
            // A server from before it said who's online: nobody said.
            if r.remaining() > 0 {
                let k = count(&mut r, 1)?;
                for _ in 0..k {
                    let i = r.read_uleb128()?;
                    if let Some(d) = usize::try_from(i).ok().and_then(|i| devices.get_mut(i)) {
                        d.online = true;
                    }
                }
            }
            Carnet::Devices { epoch, devices, recovery, password, passwords, passkeys, password_email }
        }
        22 => {
            let device = r.read_var_bytes()?.to_vec();
            let epoch = epoch(&mut r)?;
            let mut pairs = || -> Result<Vec<Pair>, String> {
                let n = count(&mut r, 2)?;
                (0..n).map(|_| Ok((r.read_var_bytes()?.to_vec(), r.read_var_bytes()?.to_vec()))).collect()
            };
            let keys = pairs()?;
            let cards = pairs()?;
            Carnet::Revoke { device, epoch, keys, cards }
        }
        23 => Carnet::Rekey { sealed: r.read_var_bytes()?.to_vec() },
        24 => Carnet::Refused { reason: r.read_var_string()? },
        25 => Carnet::Compact { room: r.read_var_string()? },
        26 => Carnet::Checked { room: r.read_var_string()?, vv: r.read_var_bytes()?.to_vec(), ok: r.read_byte()? != 0 },
        27 => Carnet::Inspect,
        28 => {
            // A room is 8 bytes at least; a log line 4.
            let n = count(&mut r, 8)?;
            let mut rooms = Vec::with_capacity(n);
            for _ in 0..n {
                let room = r.read_var_string()?;
                let held = decode_vv(r.read_var_bytes()?)?;
                let mut nums = [0u64; 5];
                for x in &mut nums {
                    *x = r.read_uleb128()?;
                }
                let base = match r.read_byte()? {
                    0 => None,
                    _ => {
                        let by = r.read_var_bytes()?.to_vec();
                        let at = r.read_uleb128()?;
                        let flags = r.read_byte()?;
                        let vv = decode_vv(r.read_var_bytes()?)?;
                        Some(BaseRow { by, at, verified: flags & 1 != 0, undoable: flags & 2 != 0, vv, bytes: r.read_uleb128()? })
                    }
                };
                let [live, live_bytes, shelved, shelved_bytes, shelved_since] = nums;
                rooms.push(RoomRow { room, held, live, live_bytes, base, shelved, shelved_bytes, shelved_since });
            }
            let n = count(&mut r, 4)?;
            let mut log = Vec::with_capacity(n);
            for _ in 0..n {
                log.push(LogRow { at: r.read_uleb128()?, device: r.read_var_bytes()?.to_vec(), room: r.read_var_string()?, what: r.read_var_string()? });
            }
            Carnet::Inspected { rooms, log }
        }
        29 => Carnet::RollBack { room: r.read_var_string()? },
        30 => Carnet::Repair { room: r.read_var_string()? },
        34 => Carnet::BlobStored { id: fixed(&mut r, 32)?.try_into().expect("32") },
        36 => Carnet::BlobNone { id: fixed(&mut r, 32)?.try_into().expect("32") },
        37 => Carnet::Invites,
        38 => {
            let owner = r.read_byte()? != 0;
            let n = count(&mut r, 33)?;
            let invites = (0..n)
                .map(|_| Ok(InviteRow { hash: fixed(&mut r, 32)?.try_into().expect("32"), until: r.read_uleb128()? }))
                .collect::<Result<_, String>>()?;
            // A row is 5 bytes at least: an id's length, flags, three times.
            let n = count(&mut r, 5)?;
            let accounts = (0..n)
                .map(|_| {
                    let id = r.read_var_bytes()?.to_vec();
                    let flags = r.read_byte()?;
                    Ok(AccountRow { id, owner: flags & 1 != 0, you: flags & 2 != 0, made: r.read_uleb128()?, seen: r.read_uleb128()?, devices: r.read_uleb128()? })
                })
                .collect::<Result<_, String>>()?;
            Carnet::InviteList { owner, invites, accounts }
        }
        39 => Carnet::InviteAdd { hash: fixed(&mut r, 32)?.try_into().expect("32") },
        40 => Carnet::InviteDrop { hash: fixed(&mut r, 32)?.try_into().expect("32") },
        41 => Carnet::AccountRemove { account: r.read_var_bytes()?.to_vec() },
        t @ (42 | 43 | 46 | 47) => {
            let n = count(&mut r, 32)?;
            let ids = (0..n).map(|_| fixed(&mut r, 32).map(|b| b.try_into().expect("32"))).collect::<Result<_, _>>()?;
            match t {
                42 => Carnet::BlobOffer { ids },
                43 => Carnet::BlobFetch { ids },
                46 => Carnet::BlobDrop { ids },
                _ => Carnet::BlobDropped { ids },
            }
        }
        48 => Carnet::RoomRemove { room: r.read_var_string()? },
        50 => Carnet::Usage,
        70 => Carnet::AccountDelete,
        52 => Carnet::PasswordSetStart { id: r.read_var_string()?, request: r.read_var_bytes()?.to_vec() },
        53 => Carnet::PasswordSetReply { response: r.read_var_bytes()?.to_vec() },
        54 => Carnet::PasswordSetFinish {
            id: r.read_var_string()?,
            upload: r.read_var_bytes()?.to_vec(),
            public: fixed(&mut r, 32)?.try_into().expect("32"),
            wrapped: r.read_var_bytes()?.to_vec(),
            card: r.read_var_bytes()?.to_vec(),
        },
        55 => Carnet::PasswordSet,
        56 => Carnet::PasswordDrop,
        57 => Carnet::PasswordLogin { id: r.read_var_string()?, request: r.read_var_bytes()?.to_vec() },
        58 => Carnet::PasswordLoginReply { response: r.read_var_bytes()?.to_vec() },
        59 => Carnet::PasskeyAdd {
            public: fixed(&mut r, 32)?.try_into().expect("32"),
            wrapped: r.read_var_bytes()?.to_vec(),
            card: r.read_var_bytes()?.to_vec(),
        },
        60 => Carnet::PasskeyDrop { public: fixed(&mut r, 32)?.try_into().expect("32") },
        61 => Carnet::PasskeySet,
        51 => Carnet::UsageIs {
            notes: r.read_uleb128()?,
            pictures: r.read_uleb128()?,
            limit: r.read_uleb128()?,
            writable: r.read_byte()? != 0,
            plan: r.read_var_string()?,
            until: r.read_uleb128()?,
        },
        49 => Carnet::RoomRemoved { room: r.read_var_string()? },
        44 => Carnet::BlobLink {
            id: fixed(&mut r, 32)?.try_into().expect("32"),
            put: r.read_byte()? != 0,
            url: r.read_var_string()?,
            expires: r.read_uleb128()?,
        },
        // A later protocol's frame: skipped, not a reason to hang up.
        tag => Carnet::Unknown { tag },
    };
    // What follows the fields this build knows is a later protocol's,
    // added at the end: left unread (§14).
    Ok(Frame::Carnet(c))
}

fn greeting(r: &mut BytesReader<'_>) -> Result<Greeting, String> {
    let n = |r: &mut BytesReader<'_>| u32::try_from(r.read_uleb128()?).map_err(|_| "a protocol past u32".to_string());
    let (protocol, oldest, deprecated) = (n(r)?, n(r)?, n(r)?);
    let count = count(r, 1)?;
    let mut features = Vec::with_capacity(count);
    for _ in 0..count {
        features.push(r.read_var_string()?);
    }
    Ok(Greeting { protocol, oldest, deprecated, features })
}

/// A count of items at least `each` bytes long, held to the bytes left.
fn count(r: &mut BytesReader<'_>, each: usize) -> Result<usize, String> {
    let n = r.read_uleb128()?;
    if n > (r.remaining() / each.max(1)) as u64 {
        return Err("a count past what follows it".into());
    }
    Ok(n as usize)
}

fn epoch(r: &mut BytesReader<'_>) -> Result<u32, String> {
    u32::try_from(r.read_uleb128()?).map_err(|_| "an epoch past u32".to_string())
}

fn nameplate(r: &mut BytesReader<'_>) -> Result<u32, String> {
    u32::try_from(r.read_uleb128()?).map_err(|_| "a nameplate past u32".to_string())
}

/// A version vector as a join carries it: a count, then each peer (its
/// decimal digits, as the other Loro Protocol clients write it) and its
/// counter.
pub fn encode_vv(vv: &[(u64, u64)]) -> Vec<u8> {
    let mut w = BytesWriter::new();
    w.push_uleb128(vv.len() as u64);
    for (peer, counter) in vv {
        w.push_var_bytes(peer.to_string().as_bytes());
        w.push_uleb128(*counter);
    }
    w.finalize()
}

pub fn decode_vv(bytes: &[u8]) -> Result<Vec<(u64, u64)>, String> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut r = BytesReader::new(bytes);
    let n = r.read_uleb128()?;
    if n > r.remaining() as u64 / 2 {
        return Err("peer count past the end".into());
    }
    let mut vv = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let peer = std::str::from_utf8(r.read_var_bytes()?)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or("a peer that is not a number")?;
        vv.push((peer, r.read_uleb128()?));
    }
    Ok(vv)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of every frame, each field set: what the fixtures hold.
    fn samples() -> Vec<Frame> {
        vec![
            Frame::Carnet(Carnet::Challenge { nonce: [7; 32], server: Greeting::ours(vec!["passkeys".into(), "remove".into()]) }),
            Frame::Carnet(Carnet::Hello { account: b"a".to_vec(), device: b"d".to_vec(), signature: [9; 64], since: 300 }),
            Frame::Carnet(Carnet::Welcome { seq: 1 << 40 }),
            Frame::Carnet(Carnet::Denied { reason: "this device was removed from the account".into(), why: Deny::DeviceRemoved }),
            Frame::Carnet(Carnet::Changed { rooms: vec![("vault".into(), 3), ("ab12".into(), 9)] }),
            Frame::Carnet(Carnet::PairOpen),
            Frame::Carnet(Carnet::PairOpened { nameplate: 47 }),
            Frame::Carnet(Carnet::PairJoin { nameplate: 47, pake: vec![1, 2, 3] }),
            Frame::Carnet(Carnet::PairJoined { pake: vec![4] }),
            Frame::Carnet(Carnet::PairStep { data: vec![5, 6] }),
            Frame::Carnet(Carnet::PairApprove { device: b"d".to_vec(), public: [3; 32] }),
            Frame::Carnet(Carnet::PairClose { reason: "wrong code".into() }),
            Frame::Carnet(Carnet::SetRecovery { public: [1; 32], wrapped: vec![9; 60], card: vec![8; 90] }),
            Frame::Carnet(Carnet::RecoverySet),
            Frame::Carnet(Carnet::Recover { public: [1; 32], signature: [2; 64], device: b"n".to_vec(), device_public: [3; 32] }),
            Frame::Carnet(Carnet::Recovered { account: b"acct".to_vec(), wrapped: vec![7; 60], rekeys: vec![vec![1; 80], vec![2; 80]] }),
            Frame::Carnet(Carnet::Enroll { invite: "K7QF-2M9D-XW4H-P3TA".into(), device: b"d".to_vec(), public: [4; 32] }),
            Frame::Carnet(Carnet::Enrolled { account: b"acct".to_vec() }),
            Frame::Carnet(Carnet::SetCard { card: vec![3; 100] }),
            Frame::Carnet(Carnet::ListDevices),
            Frame::Carnet(Carnet::Devices {
                epoch: 2,
                devices: vec![
                    DeviceRow { id: b"a".to_vec(), public: [1; 32], card: vec![4; 90], seen: 1_760_000_000, you: true, online: false },
                    DeviceRow { id: b"b".to_vec(), public: [2; 32], card: Vec::new(), seen: 0, you: false, online: false },
                ],
                recovery: vec![5; 90],
                password: vec![6; 90],
                passwords: true,
                passkeys: vec![PasskeyRow { public: [7; 32], card: vec![8; 120], made: 1_760_000_000 }, PasskeyRow { public: [9; 32], card: vec![1; 120], made: 0 }],
                password_email: String::new(),
            }),
            // Two devices online now (this one, and another), with no
            // password's email: written empty before them.
            Frame::Carnet(Carnet::Devices {
                epoch: 2,
                devices: vec![
                    DeviceRow { id: b"a".to_vec(), public: [1; 32], card: vec![4; 90], seen: 1_760_000_000, you: true, online: true },
                    DeviceRow { id: b"b".to_vec(), public: [2; 32], card: Vec::new(), seen: 1_760_000_100, you: false, online: false },
                    DeviceRow { id: b"c".to_vec(), public: [3; 32], card: vec![5; 90], seen: 1_760_000_200, you: false, online: true },
                ],
                recovery: Vec::new(),
                password: Vec::new(),
                passwords: false,
                passkeys: Vec::new(),
                password_email: String::new(),
            }),
            // On a server holding passwords to the account's email.
            Frame::Carnet(Carnet::Devices { epoch: 1, devices: Vec::new(), recovery: Vec::new(), password: Vec::new(), passwords: true, passkeys: Vec::new(), password_email: "ann@example.com".into() }),
            Frame::Carnet(Carnet::Devices { epoch: 0, devices: Vec::new(), recovery: Vec::new(), password: Vec::new(), passwords: false, passkeys: Vec::new(), password_email: String::new() }),
            Frame::Carnet(Carnet::PasskeyAdd { public: [4; 32], wrapped: vec![5; 60], card: vec![6; 120] }),
            Frame::Carnet(Carnet::PasskeyDrop { public: [4; 32] }),
            Frame::Carnet(Carnet::PasskeySet),
            Frame::Carnet(Carnet::Revoke {
                device: b"b".to_vec(),
                epoch: 3,
                keys: vec![(b"a".to_vec(), vec![6; 80]), (Vec::new(), vec![7; 80])],
                cards: vec![(b"a".to_vec(), vec![8; 90])],
            }),
            Frame::Carnet(Carnet::Rekey { sealed: vec![9; 80] }),
            Frame::Carnet(Carnet::Refused { reason: "a device came in meanwhile".into() }),
            Frame::Carnet(Carnet::Compact { room: "ab12".into() }),
            Frame::Carnet(Carnet::Checked { room: "ab12".into(), vv: encode_vv(&[(1, 4)]), ok: false }),
            Frame::Carnet(Carnet::Inspect),
            Frame::Carnet(Carnet::Inspected {
                rooms: vec![
                    RoomRow { room: "ab12".into(), held: vec![(1, 9)], live: 3, live_bytes: 900, base: None, shelved: 0, shelved_bytes: 0, shelved_since: 0 },
                    RoomRow {
                        room: "cd34".into(),
                        held: vec![(1, 40), (7, 2)],
                        live: 1,
                        live_bytes: 80,
                        base: Some(BaseRow { by: b"dev".to_vec(), at: 1_760_000_000, verified: false, undoable: true, vv: vec![(1, 39)], bytes: 4000 }),
                        shelved: 200,
                        shelved_bytes: 30_000,
                        shelved_since: 1_760_000_000,
                    },
                ],
                log: vec![LogRow { at: 1_760_000_100, device: b"dev".to_vec(), room: "cd34".into(), what: "rolled back".into() }],
            }),
            Frame::Carnet(Carnet::RollBack { room: "cd34".into() }),
            Frame::Carnet(Carnet::Repair { room: "cd34".into() }),
            Frame::Carnet(Carnet::BlobOffer { ids: vec![[1; 32], [2; 32]] }),
            Frame::Carnet(Carnet::BlobFetch { ids: vec![[2; 32]] }),
            Frame::Carnet(Carnet::BlobDrop { ids: vec![[7; 32], [8; 32]] }),
            Frame::Carnet(Carnet::BlobDropped { ids: vec![[7; 32]] }),
            Frame::Carnet(Carnet::RoomRemove { room: "ab12".into() }),
            Frame::Carnet(Carnet::Usage),
            Frame::Carnet(Carnet::AccountDelete),
            Frame::Carnet(Carnet::PasswordSetStart { id: "ann@example.com".into(), request: vec![1; 32] }),
            Frame::Carnet(Carnet::PasswordSetReply { response: vec![2; 64] }),
            Frame::Carnet(Carnet::PasswordSetFinish { id: "ann@example.com".into(), upload: vec![3; 192], public: [4; 32], wrapped: vec![5; 60], card: vec![6; 90] }),
            Frame::Carnet(Carnet::PasswordSet),
            Frame::Carnet(Carnet::PasswordDrop),
            Frame::Carnet(Carnet::PasswordLogin { id: "ann@example.com".into(), request: vec![7; 96] }),
            Frame::Carnet(Carnet::PasswordLoginReply { response: vec![8; 320] }),
            Frame::Carnet(Carnet::UsageIs { notes: 12_345, pictures: 6_000_000_000, limit: 5 << 30, writable: false, plan: "Personal".into(), until: 1_790_000_000 }),
            Frame::Carnet(Carnet::RoomRemoved { room: "ab12".into() }),
            Frame::Carnet(Carnet::BlobLink { id: [3; 32], put: true, url: "https://n.example/v1/blobs/ab12".into(), expires: 1_760_000_300 }),
            Frame::Carnet(Carnet::BlobLink { id: [3; 32], put: false, url: "https://bucket.example/x?sig=1".into(), expires: 0 }),
            Frame::Carnet(Carnet::BlobStored { id: [4; 32] }),
            Frame::Carnet(Carnet::BlobNone { id: [6; 32] }),
            Frame::Carnet(Carnet::Invites),
            Frame::Carnet(Carnet::InviteList {
                owner: true,
                invites: vec![InviteRow { hash: [7; 32], until: 1_760_600_000 }],
                accounts: vec![
                    AccountRow { id: b"own".to_vec(), owner: true, made: 1_760_000_000, seen: 1_760_500_000, devices: 3, you: true },
                    AccountRow { id: b"other".to_vec(), owner: false, made: 1_760_100_000, seen: 0, devices: 1, you: false },
                ],
            }),
            Frame::Carnet(Carnet::InviteList { owner: false, invites: Vec::new(), accounts: Vec::new() }),
            Frame::Carnet(Carnet::InviteAdd { hash: [8; 32] }),
            Frame::Carnet(Carnet::InviteDrop { hash: [9; 32] }),
            Frame::Carnet(Carnet::AccountRemove { account: b"other".to_vec() }),
            Frame::Room(ProtocolMessage::JoinRequest {
                crdt: CrdtType::Elo,
                room_id: "ab12".into(),
                auth: Vec::new(),
                version: encode_vv(&[(42, 7)]),
            }),
        ]
    }

    #[test]
    fn frames_go_and_come_back() {
        for f in samples() {
            assert_eq!(decode(&encode(&f)).unwrap(), f);
        }
        assert_eq!(decode_vv(&encode_vv(&[(1, 2), (u64::MAX, 0)])).unwrap(), [(1, 2), (u64::MAX, 0)]);
        assert!(decode(b"%CRN\x05\xff\xff\xff\x0f").is_err(), "a count past the end");
        // A server from before passkeys: its list, without them (nor a
        // password's email: any).
        let old = encode(&Frame::Carnet(Carnet::Devices { epoch: 1, devices: Vec::new(), recovery: Vec::new(), password: Vec::new(), passwords: true, passkeys: Vec::new(), password_email: String::new() }));
        let old = &old[..old.len() - 1];
        assert!(matches!(decode(old), Ok(Frame::Carnet(Carnet::Devices { epoch: 1, passwords: true, passkeys, password_email, .. })) if passkeys.is_empty() && password_email.is_empty()));
        // A server from before it said who's online: the same list, nobody
        // said online (the email and the places, 1 + 3 bytes, cut).
        let row = |id: &[u8], online| DeviceRow { id: id.to_vec(), public: [1; 32], card: Vec::new(), seen: 9, you: false, online };
        let now = encode(&Frame::Carnet(Carnet::Devices {
            epoch: 1,
            devices: vec![row(b"a", true), row(b"b", false), row(b"c", true)],
            recovery: Vec::new(),
            password: Vec::new(),
            passwords: false,
            passkeys: Vec::new(),
            password_email: String::new(),
        }));
        let Ok(Frame::Carnet(Carnet::Devices { devices, .. })) = decode(&now[..now.len() - 4]) else { panic!() };
        assert_eq!(devices, vec![row(b"a", false), row(b"b", false), row(b"c", false)]);
        let Ok(Frame::Carnet(Carnet::Devices { devices, .. })) = decode(&now) else { panic!() };
        assert_eq!(devices.iter().map(|d| d.online).collect::<Vec<_>>(), [true, false, true]);
        // A DocUpdate claiming billions of updates in a few bytes: refused
        // before anything is made room for.
        let mut m = b"%ELO\x01r\x03".to_vec();
        m.extend([0xff, 0xff, 0xff, 0x7a]);
        assert!(decode(&m).is_err());
    }

    /// Tags no frame uses any more: never given to another.
    const RETIRED: &[u8] = &[];

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("protocol")
    }

    /// A frame's name and its bytes.
    type Named = (String, Vec<u8>);

    /// Each protocol's frames as its builds wrote them (`protocol/N.txt`),
    /// one per line: a name, then its bytes in hex.
    fn fixtures() -> Vec<(u32, Vec<Named>)> {
        let mut all = Vec::new();
        for e in std::fs::read_dir(fixtures_dir()).unwrap().flatten() {
            let path = e.path();
            let Some(n) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) else { continue };
            let lines = std::fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| {
                    let (name, bytes) = l.split_once(' ').unwrap();
                    (name.to_string(), unhex(bytes.trim()))
                })
                .collect();
            all.push((n, lines));
        }
        all.sort_by_key(|(n, _)| *n);
        all
    }

    fn name(f: &Frame) -> String {
        match f {
            Frame::Room(_) => "room".into(),
            Frame::Carnet(c) => format!("{c:?}").split([' ', '{', '(']).next().unwrap().to_string(),
        }
    }

    /// Every frame an older protocol's build wrote still reads, as a frame
    /// this build knows: what an app or a server not yet updated sends.
    #[test]
    fn every_protocols_frames_still_read() {
        let all = fixtures();
        assert!(all.iter().any(|(n, _)| *n == crate::compat::PROTOCOL), "no protocol/{}.txt: run the tests with CARNET_BLESS=1", crate::compat::PROTOCOL);
        for (n, frames) in &all {
            for (name, bytes) in frames {
                match decode(bytes) {
                    Ok(Frame::Carnet(Carnet::Unknown { tag })) => panic!("protocol {n}'s {name} reads as unknown tag {tag}: a tag reused or dropped"),
                    Ok(_) => {}
                    Err(e) => panic!("protocol {n}'s {name} no longer reads: {e}"),
                }
            }
        }
    }

    /// This protocol's frames are written as its fixture says, byte for
    /// byte. A frame changed is a new protocol (raise `compat::PROTOCOL`,
    /// then bless); a frame added is blessed into this one.
    #[test]
    fn this_protocols_frames_are_written_as_its_fixture_says() {
        let now: Vec<Named> = samples().iter().map(|f| (name(f), encode(f))).collect();
        let file = fixtures_dir().join(format!("{}.txt", crate::compat::PROTOCOL));
        let kept = fixtures().into_iter().find(|(n, _)| *n == crate::compat::PROTOCOL).map(|(_, l)| l);
        let changed: Vec<&String> = kept.iter().flatten().filter(|k| !now.contains(k)).map(|(name, _)| name).collect();
        let added = kept.as_ref().is_none_or(|k| now.iter().any(|n| !k.contains(n)));
        if std::env::var_os("CARNET_BLESS").is_some() && changed.is_empty() && added {
            let mut text = format!(
                "# Protocol {}: one of every frame, as its builds write it.\n# Never edit a line: a frame written differently is the next protocol.\n",
                crate::compat::PROTOCOL
            );
            for (name, bytes) in &now {
                text.push_str(&format!("{name} {}\n", hex(bytes)));
            }
            std::fs::write(&file, text).unwrap();
            return;
        }
        assert!(
            changed.is_empty(),
            "frames written differently from protocol {}'s fixture: {changed:?}. Older peers read them as they were: raise compat::PROTOCOL, keep the old fixture, and bless the new one (CARNET_BLESS=1)",
            crate::compat::PROTOCOL
        );
        assert!(!added, "frames not in {}: bless them in (CARNET_BLESS=1)", file.display());
    }

    /// Every tag this build reads has a sample (so a fixture holds it),
    /// and no retired tag is read again.
    #[test]
    fn every_tag_has_a_sample_and_none_comes_back() {
        let sampled: Vec<u8> = samples().iter().map(encode).filter(|b| b.starts_with(CARNET)).map(|b| b[4]).collect();
        for t in 0..=255u8 {
            let known = !matches!(decode(&[b'%', b'C', b'R', b'N', t]), Ok(Frame::Carnet(Carnet::Unknown { .. })));
            if known {
                assert!(sampled.contains(&t), "tag {t} has no sample");
                assert!(!RETIRED.contains(&t), "tag {t} is retired");
            }
        }
    }

    /// A later protocol's frame is skipped, and its fields added at the
    /// end of a known frame are left unread; an earlier one's missing
    /// fields read as their defaults.
    #[test]
    fn later_frames_and_fields_are_skipped_and_earlier_ones_filled_in() {
        assert_eq!(decode(b"%CRN\xc8\x01\x02").unwrap(), Frame::Carnet(Carnet::Unknown { tag: 200 }));
        let mut later = encode(&Frame::Carnet(Carnet::Welcome { seq: 9 }));
        later.extend([1, 2, 3]);
        assert_eq!(decode(&later).unwrap(), Frame::Carnet(Carnet::Welcome { seq: 9 }));
        let mut nonce_only = b"%CRN\x01".to_vec();
        nonce_only.extend([5; 32]);
        assert_eq!(decode(&nonce_only).unwrap(), Frame::Carnet(Carnet::Challenge { nonce: [5; 32], server: Greeting::default() }));
        let mut words_only = b"%CRN\x04".to_vec();
        words_only.extend([2, b'n', b'o']);
        assert_eq!(decode(&words_only).unwrap(), Frame::Carnet(Carnet::Denied { reason: "no".into(), why: Deny::Other }));
        // A reason of a later protocol: Other.
        words_only.push(77);
        assert!(matches!(decode(&words_only), Ok(Frame::Carnet(Carnet::Denied { why: Deny::Other, .. }))));
    }
}
