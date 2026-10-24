//! Who may say what on a socket (docs/SYNC.md §7), frame by frame, sent
//! as an attacker would send them: raw, out of turn, signed with the wrong
//! key, for the wrong server, for another account. The devices' sessions
//! never send such frames, so their tests can't show the hub refuses
//! them; these do. On memory and on SQLite alike.

mod common;

use carnet_server::blobs::{MemoryBlobs, Worker};
use carnet_server::limits::TOO_MANY;
use carnet_server::storage::{PasskeySlot, PasswordSlot};
use carnet_server::{ConnId, Hub, Out, Storage};
use carnet_sync::crypto::{DeviceKey, VaultKey, login_message, recovery_message};
use carnet_sync::elo::{self, Header};
use carnet_sync::invite::InviteCode;
use carnet_sync::wire::{self, BatchId, Carnet, CrdtType, Frame, UpdateStatusCode};
use common::{ORIGIN, Seeded, stores};
use loro_protocol::ProtocolMessage;

const A: &[u8] = b"account-a";
const B: &[u8] = b"account-b";
const ROOM: &str = "0000000000000000000000000000a11c";

/// A hub with two accounts, A (the owner's: devices a1, a2) and B (b1),
/// and sockets driven by hand.
struct Raw {
    hub: Hub<Box<dyn Storage>>,
    rng: Seeded,
    next: ConnId,
    a1: DeviceKey,
    a2: DeviceKey,
    b1: DeviceKey,
    vault: VaultKey,
    /// The file worker, as the server runs one beside the hub.
    worker: Worker<MemoryBlobs>,
}

impl Raw {
    fn new(store: Box<dyn Storage>) -> Self {
        let mut rng = Seeded(7);
        let (a1, a2, b1) = (DeviceKey::generate(&mut rng), DeviceKey::generate(&mut rng), DeviceKey::generate(&mut rng));
        let mut hub = Hub::new(ORIGIN, store);
        let s = hub.store_mut();
        s.add_account(A, true).unwrap();
        s.add_account(B, false).unwrap();
        s.add_device(A, b"a1", a1.public()).unwrap();
        s.add_device(A, b"a2", a2.public()).unwrap();
        s.add_device(B, b"b1", b1.public()).unwrap();
        let vault = VaultKey::generate(&mut rng);
        Raw { hub, rng, next: 1, a1, a2, b1, vault, worker: common::worker() }
    }

    /// A socket from `client`: its id and the challenge's nonce.
    fn open(&mut self, client: &str) -> (ConnId, [u8; 32]) {
        let conn = self.next;
        self.next += 1;
        let outs = self.hub.connect_from(conn, client, carnet_sync::compat::Client { protocol: carnet_sync::compat::PROTOCOL, ..Default::default() }, &mut self.rng);
        let nonce = frames(&outs, conn).into_iter().find_map(|f| match f {
            Frame::Carnet(Carnet::Challenge { nonce, .. }) => Some(nonce),
            _ => None,
        });
        (conn, nonce.expect("a challenge first"))
    }

    /// What the hub sends for `frame`, file work done as the server does
    /// it (by the worker, its frames sent on).
    fn send(&mut self, conn: ConnId, frame: &Frame) -> Vec<Out> {
        let outs = self.hub.receive(conn, &wire::encode(frame), &mut self.rng);
        self.worker.done(outs)
    }

    fn hello(account: &[u8], device: &[u8], key: &DeviceKey, origin: &str, nonce: &[u8; 32]) -> Frame {
        let signature = key.sign(&login_message(origin, account, device, nonce));
        Frame::Carnet(Carnet::Hello { account: account.to_vec(), device: device.to_vec(), signature, since: 0 })
    }

    /// A socket logged in as `device` of `account`.
    fn login(&mut self, account: &[u8], device: &[u8], key: &DeviceKey) -> ConnId {
        let (conn, nonce) = self.open("");
        let outs = self.send(conn, &Self::hello(account, device, key, ORIGIN, &nonce));
        assert!(frames(&outs, conn).iter().any(|f| matches!(f, Frame::Carnet(Carnet::Welcome { .. }))), "logged in");
        conn
    }

    /// A delta record for `room`, sealed under the vault key.
    fn record(&mut self, room: &str, start: u64) -> Vec<u8> {
        elo::seal(&self.vault, room, &Header::Delta { peer: 9, start, end: start + 1, epoch: 0 }, b"sealed words", &mut self.rng)
    }
}

/// What was sent to `conn`, decoded.
fn frames(outs: &[Out], conn: ConnId) -> Vec<Frame> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Send(c, b) if *c == conn => wire::decode(b).ok(),
            _ => None,
        })
        .collect()
}

/// Why `conn` was turned away, if it was: told, and its socket closed.
fn denied(outs: &[Out], conn: ConnId) -> Option<String> {
    let closed = outs.contains(&Out::Close(conn));
    let why = frames(outs, conn).into_iter().find_map(|f| match f {
        Frame::Carnet(Carnet::Denied { reason, .. }) => Some(reason),
        _ => None,
    });
    why.filter(|_| closed)
}

fn refused(outs: &[Out], conn: ConnId) -> Option<String> {
    frames(outs, conn).into_iter().find_map(|f| match f {
        Frame::Carnet(Carnet::Refused { reason }) => Some(reason),
        _ => None,
    })
}

fn join(room: &str) -> Frame {
    Frame::Room(ProtocolMessage::JoinRequest { crdt: CrdtType::Elo, room_id: room.into(), auth: Vec::new(), version: wire::encode_vv(&[]) })
}

fn put(room: &str, records: &[Vec<u8>]) -> Frame {
    Frame::Room(ProtocolMessage::DocUpdate { crdt: CrdtType::Elo, room_id: room.into(), updates: vec![elo::container(records)], batch_id: BatchId([1; 8]) })
}

fn acked(outs: &[Out], conn: ConnId) -> Option<UpdateStatusCode> {
    frames(outs, conn).into_iter().find_map(|f| match f {
        Frame::Room(ProtocolMessage::Ack { status, .. }) => Some(status),
        _ => None,
    })
}

/// Before a login, a socket may log in, use an invite, join a pairing or
/// recover, and nothing else: every other frame turns it away, and none
/// of them changes anything.
#[test]
fn nothing_but_a_login_before_one() {
    for store in stores() {
        let mut raw = Raw::new(store);
        let record = raw.record(ROOM, 0);
        let before = (raw.hub.store().devices(A).len(), raw.hub.store().invites().len(), raw.hub.store().accounts());
        let attempts: Vec<Frame> = vec![
            join(ROOM),
            put(ROOM, std::slice::from_ref(&record)),
            Frame::Carnet(Carnet::ListDevices),
            Frame::Carnet(Carnet::SetCard { card: vec![1; 40] }),
            Frame::Carnet(Carnet::Inspect),
            Frame::Carnet(Carnet::PairOpen),
            Frame::Carnet(Carnet::PairApprove { device: b"evil".to_vec(), public: [6; 32] }),
            Frame::Carnet(Carnet::Revoke { device: b"a2".to_vec(), epoch: 1, keys: Vec::new(), cards: Vec::new() }),
            Frame::Carnet(Carnet::RollBack { room: ROOM.into() }),
            Frame::Carnet(Carnet::Repair { room: ROOM.into() }),
            Frame::Carnet(Carnet::SetRecovery { public: [3; 32], wrapped: vec![1], card: vec![2] }),
            Frame::Carnet(Carnet::Checked { room: ROOM.into(), vv: Vec::new(), ok: false }),
            Frame::Carnet(Carnet::BlobOffer { ids: vec![[1; 32]], sizes: vec![100] }),
            Frame::Carnet(Carnet::BlobFetch { ids: vec![[1; 32]] }),
            Frame::Carnet(Carnet::BlobDrop { ids: vec![[1; 32]] }),
            Frame::Carnet(Carnet::RoomRemove { room: ROOM.into() }),
            Frame::Carnet(Carnet::AccountDelete),
            Frame::Carnet(Carnet::Invites),
            Frame::Carnet(Carnet::InviteAdd { hash: [5; 32] }),
            Frame::Carnet(Carnet::InviteDrop { hash: [5; 32] }),
            Frame::Carnet(Carnet::AccountRemove { account: B.to_vec() }),
            Frame::Carnet(Carnet::Welcome { seq: 1 }),
        ];
        for frame in attempts {
            let (conn, _) = raw.open("");
            let outs = raw.send(conn, &frame);
            assert!(denied(&outs, conn).is_some(), "{frame:?} before a login: {outs:?}");
            // Turned away is gone: what it sends next is nobody's.
            assert!(raw.send(conn, &join(ROOM)).is_empty(), "{frame:?}: the socket is forgotten");
        }
        let (conn, _) = raw.open("");
        let outs = raw.hub.receive(conn, b"not a frame at all", &mut raw.rng);
        assert_eq!(denied(&outs, conn).as_deref(), Some("not a frame"));
        let after = (raw.hub.store().devices(A).len(), raw.hub.store().invites().len(), raw.hub.store().accounts());
        assert_eq!(before, after, "nothing changed");
        assert!(raw.hub.store().spans(A, ROOM).is_empty() && !raw.hub.store().is_removed(A, ROOM));
    }
}

/// A login is the device's signature over this socket's nonce, for this
/// server: another key's, another nonce's, another server's, another
/// account's device, a device taken out, and it's refused. Logged in, a
/// second login is out of turn.
#[test]
fn a_login_must_be_signed_by_the_device_for_this_nonce_and_server() {
    for store in stores() {
        let mut raw = Raw::new(store);
        let (a1, b1) = (raw.a1.clone(), raw.b1.clone());
        let try_login = |raw: &mut Raw, hello: &dyn Fn(&[u8; 32]) -> Frame| {
            let (conn, nonce) = raw.open("");
            let outs = raw.send(conn, &hello(&nonce));
            (denied(&outs, conn), frames(&outs, conn))
        };

        // Signed with another device's key.
        let (why, _) = try_login(&mut raw, &|n| Raw::hello(A, b"a1", &b1, ORIGIN, n));
        assert_eq!(why.as_deref(), Some("not a device of this account"));
        // Signed over another socket's nonce: a login seen once, replayed.
        let (_, other) = raw.open("");
        let (why, _) = try_login(&mut raw, &|_| Raw::hello(A, b"a1", &a1, ORIGIN, &other));
        assert!(why.is_some(), "a replayed login");
        // Signed for another server: one that passes logins on gets nowhere.
        let (why, _) = try_login(&mut raw, &|n| Raw::hello(A, b"a1", &a1, "https://elsewhere.example", n));
        assert!(why.is_some(), "signed for another origin");
        // A device not in the account; another account's device, claimed
        // under this one.
        let (why, _) = try_login(&mut raw, &|n| Raw::hello(A, b"nobody", &a1, ORIGIN, n));
        assert!(why.is_some());
        let (why, _) = try_login(&mut raw, &|n| Raw::hello(A, b"b1", &b1, ORIGIN, n));
        assert!(why.is_some(), "B's device as A's");
        // An account that isn't there.
        let (why, _) = try_login(&mut raw, &|n| Raw::hello(b"no-such", b"a1", &a1, ORIGIN, n));
        assert!(why.is_some());

        // The real one.
        let (why, said) = try_login(&mut raw, &|n| Raw::hello(A, b"a1", &a1, ORIGIN, n));
        assert!(why.is_none() && said.iter().any(|f| matches!(f, Frame::Carnet(Carnet::Welcome { .. }))));

        // Logged in, a second Hello is out of turn.
        let conn = raw.login(A, b"a1", &a1);
        let outs = raw.send(conn, &Raw::hello(A, b"a1", &a1, ORIGIN, &[0; 32]));
        assert_eq!(denied(&outs, conn).as_deref(), Some("not now"));

        // A device taken out can't log in.
        raw.hub.store_mut().revoke_device(A, b"a2").unwrap();
        let a2 = raw.a2.clone();
        let (why, _) = try_login(&mut raw, &|n| Raw::hello(A, b"a2", &a2, ORIGIN, n));
        assert_eq!(why.as_deref(), Some("this device was removed from the account"));
    }
}

/// The other ways in before a login need what they say: an invite the
/// server made and nobody used; a recovery signature by the account's
/// recovery key over this nonce and the new device.
#[test]
fn an_invite_or_a_recovery_must_be_real() {
    for store in stores() {
        let mut raw = Raw::new(store);
        let enroll = |invite: &str, device: &[u8], public: [u8; 32]| Frame::Carnet(Carnet::Enroll { invite: invite.into(), device: device.to_vec(), public });
        let pk = DeviceKey::generate(&mut raw.rng).public();

        let (conn, _) = raw.open("");
        let outs = raw.send(conn, &enroll("not a code", b"new", pk));
        assert!(denied(&outs, conn).is_some());
        let made = InviteCode::generate(&mut raw.rng);
        let (conn, _) = raw.open("");
        let outs = raw.send(conn, &enroll(&made.text(), b"new", pk));
        assert!(denied(&outs, conn).is_some_and(|w| w.contains("not good")), "an invite the server never made");
        assert_eq!(raw.hub.store().accounts(), 2);

        // Recovery: the account's recovery key, set; a signature by another.
        let recovery = DeviceKey::generate(&mut raw.rng);
        raw.hub.store_mut().set_recovery(A, recovery.public(), vec![1; 40], vec![2; 40]).unwrap();
        let newcomer = DeviceKey::generate(&mut raw.rng);
        let recover = |key: &DeviceKey, public: [u8; 32], nonce: &[u8; 32], device_public: [u8; 32]| {
            let signature = key.sign(&recovery_message(ORIGIN, &public, b"phone", &device_public, nonce));
            Frame::Carnet(Carnet::Recover { public, signature, device: b"phone".to_vec(), device_public })
        };
        let (conn, nonce) = raw.open("");
        let outs = raw.send(conn, &recover(&newcomer, recovery.public(), &nonce, newcomer.public()));
        assert_eq!(denied(&outs, conn).as_deref(), Some("not that recovery code's signature"));
        // Signed for one new device, claimed for another key.
        let (conn, nonce) = raw.open("");
        let mut f = recover(&recovery, recovery.public(), &nonce, newcomer.public());
        if let Frame::Carnet(Carnet::Recover { device_public, .. }) = &mut f {
            *device_public = DeviceKey::generate(&mut raw.rng).public();
        }
        let outs = raw.send(conn, &f);
        assert!(denied(&outs, conn).is_some(), "the device key swapped after signing");
        // A key no account has.
        let stranger = DeviceKey::generate(&mut raw.rng);
        let (conn, nonce) = raw.open("");
        let outs = raw.send(conn, &recover(&stranger, stranger.public(), &nonce, newcomer.public()));
        assert_eq!(denied(&outs, conn).as_deref(), Some("no account has that recovery code"));
        assert!(raw.hub.store().device(A, b"phone").is_none(), "nobody let in");
        // The real one lets the device in.
        let (conn, nonce) = raw.open("");
        let outs = raw.send(conn, &recover(&recovery, recovery.public(), &nonce, newcomer.public()));
        assert!(frames(&outs, conn).iter().any(|f| matches!(f, Frame::Carnet(Carnet::Recovered { .. }))));
        assert!(raw.hub.store().device(A, b"phone").is_some());
    }
}

/// Logged in, a device works in its own account only: the account is the
/// one it logged in to, whatever a frame names. Rooms of the same name in
/// two accounts never meet; lists, files and the inspector are its own.
#[test]
fn an_account_never_sees_another() {
    for store in stores() {
        let mut raw = Raw::new(store);
        let (a1, b1) = (raw.a1.clone(), raw.b1.clone());
        let a = raw.login(A, b"a1", &a1);
        let b = raw.login(B, b"b1", &b1);
        raw.send(a, &join(ROOM));
        let rec = raw.record(ROOM, 0);
        let outs = raw.send(a, &put(ROOM, std::slice::from_ref(&rec)));
        assert_eq!(acked(&outs, a), Some(UpdateStatusCode::Ok));

        // B joins a room of the same name: it has nothing of A's.
        let outs = raw.send(b, &join(ROOM));
        let got: Vec<_> = frames(&outs, b)
            .into_iter()
            .filter(|f| matches!(f, Frame::Room(ProtocolMessage::DocUpdate { .. })))
            .collect();
        assert!(got.is_empty(), "A's record went to B: {got:?}");
        // What B writes there is B's, and never reaches A.
        let theirs = raw.record(ROOM, 5);
        let outs = raw.send(b, &put(ROOM, &[theirs]));
        assert!(frames(&outs, a).is_empty(), "B's write reached A");
        assert_eq!(raw.hub.store().spans(A, ROOM).len(), 1);
        assert_eq!(raw.hub.store().spans(B, ROOM).len(), 1);

        // Its own devices only.
        let outs = raw.send(b, &Frame::Carnet(Carnet::ListDevices));
        let list = frames(&outs, b).into_iter().find_map(|f| match f {
            Frame::Carnet(Carnet::Devices { devices, .. }) => Some(devices),
            _ => None,
        });
        assert_eq!(list.unwrap().iter().map(|d| d.id.clone()).collect::<Vec<_>>(), [b"b1".to_vec()]);

        // A's file, asked for by B: B has none, and gets no link to it.
        raw.worker.blobs.put(A, &[4; 32], &[9; 100]);
        let outs = raw.send(b, &Frame::Carnet(Carnet::BlobFetch { ids: vec![[4; 32]] }));
        assert_eq!(frames(&outs, b), [Frame::Carnet(Carnet::BlobNone { id: [4; 32] })]);
        // Offered by B: a link to put it in B's own account, not A's (nor
        // a word that A has it).
        let outs = raw.send(b, &Frame::Carnet(Carnet::BlobOffer { ids: vec![[4; 32]], sizes: vec![100] }));
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        match &frames(&outs, b)[..] {
            [Frame::Carnet(Carnet::BlobLink { put: true, url, .. })] => assert!(url.contains(&hex(B)) && !url.contains(&hex(A)), "{url}"),
            other => panic!("{other:?}"),
        }

        // Dropped by B: B's own account only; A's file stays unmarked.
        raw.send(b, &Frame::Carnet(Carnet::BlobDrop { ids: vec![[4; 32]] }));
        assert!(!raw.worker.blobs.is_marked(A, &[4; 32]));

        // Removing a room of the same name: B's own; A's stays, and A's
        // device in it isn't told.
        let outs = raw.send(b, &Frame::Carnet(Carnet::RoomRemove { room: ROOM.into() }));
        assert!(frames(&outs, a).is_empty(), "A was told of B's removal");
        assert!(raw.hub.store().is_removed(B, ROOM) && !raw.hub.store().is_removed(A, ROOM));
        assert_eq!(raw.hub.store().spans(A, ROOM).len(), 1);

        // The inspector: B's rooms, not A's.
        let outs = raw.send(b, &Frame::Carnet(Carnet::Inspect));
        let rooms = frames(&outs, b).into_iter().find_map(|f| match f {
            Frame::Carnet(Carnet::Inspected { rooms, .. }) => Some(rooms),
            _ => None,
        });
        assert!(rooms.unwrap().iter().all(|r| r.live <= 1), "B's own room, with B's one record");

        // Taking out another account's device: not in this account.
        let outs = raw.send(b, &Frame::Carnet(Carnet::Revoke { device: b"a2".to_vec(), epoch: 1, keys: Vec::new(), cards: Vec::new() }));
        assert_eq!(refused(&outs, b).as_deref(), Some("that device is not in the account"));
        assert!(raw.hub.store().device(A, b"a2").is_some_and(|d| !d.revoked));

        // Rolling back another account's room touches B's own only.
        raw.send(b, &Frame::Carnet(Carnet::RollBack { room: ROOM.into() }));
        assert_eq!(raw.hub.store().spans(A, ROOM).len(), 1);
    }
}

/// Inside its account too, a device does what its turn allows: it writes
/// where it has joined, sends a snapshot only when asked, lets a device
/// in only from a pairing it opened, and records past their size are
/// refused, not kept.
#[test]
fn a_device_writes_where_it_joined_and_snapshots_when_asked() {
    for store in stores() {
        let mut raw = Raw::new(store);
        let a1 = raw.a1.clone();
        let a = raw.login(A, b"a1", &a1);

        // Not joined: refused, nothing kept.
        let rec = raw.record(ROOM, 0);
        let outs = raw.send(a, &put(ROOM, &[rec]));
        assert_eq!(acked(&outs, a), Some(UpdateStatusCode::PermissionDenied));
        assert!(raw.hub.store().spans(A, ROOM).is_empty());

        // A snapshot nobody asked for: refused, the base untouched.
        raw.send(a, &join(ROOM));
        let snap = elo::seal(&raw.vault, ROOM, &Header::Snapshot { vv: vec![(9, 1)], epoch: 0 }, b"all of it", &mut raw.rng);
        let outs = raw.send(a, &put(ROOM, &[snap]));
        assert_eq!(acked(&outs, a), Some(UpdateStatusCode::PermissionDenied));
        assert!(raw.hub.store().base(A, ROOM).is_none());

        // A record as big as a record may be, in a message past what the
        // Loro Protocol writes (its decoder takes it): kept, and passed on
        // to another device in the room in fragments, as a message that
        // big can't be written. One byte more: refused, not kept.
        let a2k = raw.a2.clone();
        let other = raw.login(A, b"a2", &a2k);
        const BIG: &str = "00000000000000000000000000000b16";
        raw.send(a, &join(BIG));
        raw.send(other, &join(BIG));
        let mut big = Vec::new();
        for n in [carnet_server::hub::MAX_RECORD, carnet_server::hub::MAX_RECORD + 1] {
            let mut plain = vec![0; n - 100];
            let record = loop {
                let r = elo::seal(&raw.vault, BIG, &Header::Delta { peer: 9, start: 5, end: 6, epoch: 0 }, &plain, &mut raw.rng);
                if r.len() == n {
                    break r;
                }
                plain.resize(plain.len() + n - r.len(), 0);
            };
            big.push(record);
        }
        let put_big = |record: &[u8]| {
            let mut w = loro_protocol::BytesWriter::new();
            w.push_bytes(b"%ELO");
            w.push_var_string(BIG);
            w.push_byte(3);
            w.push_uleb128(1);
            w.push_var_bytes(&elo::container(&[record]));
            w.push_bytes(&[2; 8]);
            w.finalize()
        };
        let bytes = put_big(&big[1]);
        let outs = raw.worker.done(raw.hub.receive(a, &bytes, &mut raw.rng));
        assert_eq!(acked(&outs, a), Some(UpdateStatusCode::PayloadTooLarge));
        assert!(raw.hub.store().spans(A, BIG).is_empty());
        let bytes = put_big(&big[0]);
        assert!(loro_protocol::encode(&wire::decode(&bytes).map(|f| match f { Frame::Room(m) => m, _ => panic!() }).unwrap()).is_err(), "past what a message holds");
        let outs = raw.worker.done(raw.hub.receive(a, &bytes, &mut raw.rng));
        assert_eq!(acked(&outs, a), Some(UpdateStatusCode::Ok));
        assert_eq!(raw.hub.store().spans(A, BIG).len(), 1);
        let theirs = frames(&outs, other);
        assert!(matches!(theirs.first(), Some(Frame::Room(ProtocolMessage::DocUpdateFragmentHeader { .. }))), "{}", theirs.len());
        let mut pieces = None;
        let mut whole = None;
        for f in theirs {
            match f {
                Frame::Room(ProtocolMessage::DocUpdateFragmentHeader { room_id, batch_id, fragment_count, total_size_bytes, .. }) => {
                    pieces = elo::Pieces::start(&room_id, batch_id, fragment_count, total_size_bytes);
                }
                Frame::Room(ProtocolMessage::DocUpdateFragment { room_id, batch_id, index, fragment, .. }) => {
                    whole = pieces.as_mut().unwrap().add(&room_id, batch_id, index, &fragment).unwrap().or(whole);
                }
                _ => {}
            }
        }
        assert_eq!(elo::records(&whole.unwrap()).unwrap(), [&big[0][..]]);
        // Not a record at all.
        let outs = raw.send(a, &put(ROOM, &[b"junk".to_vec()]));
        assert_eq!(acked(&outs, a), Some(UpdateStatusCode::InvalidUpdate));
        assert!(raw.hub.store().spans(A, ROOM).is_empty());

        // Letting a device in without a pairing of its own open.
        raw.send(a, &Frame::Carnet(Carnet::PairApprove { device: b"evil".to_vec(), public: [6; 32] }));
        assert!(raw.hub.store().device(A, b"evil").is_none());
        // Opened, but nobody has joined it yet.
        raw.send(a, &Frame::Carnet(Carnet::PairOpen));
        raw.send(a, &Frame::Carnet(Carnet::PairApprove { device: b"evil".to_vec(), public: [6; 32] }));
        assert!(raw.hub.store().device(A, b"evil").is_none());
        // Another device's pairing: a2 can't approve what a1 opened.
        let a2k = raw.a2.clone();
        let a2 = raw.login(A, b"a2", &a2k);
        let (j, n) = raw.open("");
        raw.send(j, &Frame::Carnet(Carnet::PairJoin { nameplate: 1, pake: vec![1] }));
        let _ = n;
        raw.send(a2, &Frame::Carnet(Carnet::PairApprove { device: b"evil".to_vec(), public: [6; 32] }));
        assert!(raw.hub.store().device(A, b"evil").is_none(), "approved by a device that didn't open it");
    }
}

/// The owner's frames are the owner's: another account's device is
/// refused, whatever account it names; the owner can't remove its own.
#[test]
fn only_the_owner_invites_and_removes_and_not_itself() {
    for store in stores() {
        let mut raw = Raw::new(store);
        let (a1, b1) = (raw.a1.clone(), raw.b1.clone());
        let a = raw.login(A, b"a1", &a1);
        let b = raw.login(B, b"b1", &b1);
        for frame in [
            Frame::Carnet(Carnet::InviteAdd { hash: [5; 32] }),
            Frame::Carnet(Carnet::InviteDrop { hash: [5; 32] }),
            Frame::Carnet(Carnet::AccountRemove { account: A.to_vec() }),
            Frame::Carnet(Carnet::AccountRemove { account: B.to_vec() }),
        ] {
            let outs = raw.send(b, &frame);
            assert_eq!(refused(&outs, b).as_deref(), Some("only the server's owner can do that"), "{frame:?}");
        }
        assert_eq!(raw.hub.store().accounts(), 2);
        assert!(raw.hub.store().invites().is_empty());
        let outs = raw.send(b, &Frame::Carnet(Carnet::Invites));
        assert_eq!(frames(&outs, b), [Frame::Carnet(Carnet::InviteList { owner: false, invites: Vec::new(), accounts: Vec::new() })]);
        let outs = raw.send(a, &Frame::Carnet(Carnet::AccountRemove { account: A.to_vec() }));
        assert_eq!(refused(&outs, a).as_deref(), Some("the owner's own account can't be removed"));
        assert_eq!(raw.hub.store().accounts(), 2);
    }
}

/// Tries from one address are limited: failed logins, invites, pairings
/// joined, recoveries. Past ten at once, the next is turned away until
/// time passes; another address isn't; a login that succeeds is never
/// counted, nor refused.
#[test]
fn too_many_tries_from_one_address_are_turned_away() {
    for store in stores() {
        let mut raw = Raw::new(store);
        let (a1, b1) = (raw.a1.clone(), raw.b1.clone());
        raw.hub.tick(1_000);
        let bad_login = |raw: &mut Raw, client: &str| {
            let (conn, nonce) = raw.open(client);
            let outs = raw.send(conn, &Raw::hello(A, b"a1", &b1, ORIGIN, &nonce));
            denied(&outs, conn).unwrap()
        };
        for _ in 0..10 {
            assert_eq!(bad_login(&mut raw, "198.51.100.7"), "not a device of this account");
        }
        assert_eq!(bad_login(&mut raw, "198.51.100.7"), TOO_MANY);
        // Joining pairings, using invites: counted the same.
        let (conn, _) = raw.open("198.51.100.7");
        let outs = raw.send(conn, &Frame::Carnet(Carnet::PairJoin { nameplate: 1, pake: vec![1] }));
        assert_eq!(denied(&outs, conn).as_deref(), Some(TOO_MANY));
        // Another address has its own.
        assert_eq!(bad_login(&mut raw, "203.0.113.9"), "not a device of this account");
        // The device itself, from the busy address, still logs in.
        let (conn, nonce) = raw.open("198.51.100.7");
        let outs = raw.send(conn, &Raw::hello(A, b"a1", &a1, ORIGIN, &nonce));
        assert!(frames(&outs, conn).iter().any(|f| matches!(f, Frame::Carnet(Carnet::Welcome { .. }))));
        // A minute on, tries again.
        raw.hub.tick(1_060);
        assert_eq!(bad_login(&mut raw, "198.51.100.7"), "not a device of this account");
        assert!(raw.hub.stats().limited >= 2);

        // Joining open pairings by guessing their numbers, from one address:
        // ten, then turned away, so a pairing can't be spoilt over and over.
        let a = raw.login(A, b"a1", &a1);
        raw.send(a, &Frame::Carnet(Carnet::PairOpen));
        let mut spoilt = 0;
        for _ in 0..20 {
            let (j, _) = raw.open("192.0.2.1");
            let outs = raw.send(j, &Frame::Carnet(Carnet::PairJoin { nameplate: 1, pake: vec![1] }));
            if denied(&outs, j).as_deref() == Some(TOO_MANY) {
                continue;
            }
            spoilt += 1;
        }
        assert_eq!(spoilt, 10);
    }
}

/// A recovery key is one account's. Another account setting it as its
/// own code is refused, and so is a key any account's password or passkey
/// signs with: the account it was still finds it, and nobody's recovery
/// goes elsewhere. An account setting its own code again, or a new one,
/// is let.
#[test]
fn a_recovery_key_is_never_taken_from_another_account() {
    for store in stores() {
        let mut raw = Raw::new(store);
        let (a1, b1) = (raw.a1.clone(), raw.b1.clone());
        let a = raw.login(A, b"a1", &a1);
        let b = raw.login(B, b"b1", &b1);
        let set = |public: [u8; 32]| Frame::Carnet(Carnet::SetRecovery { public, wrapped: vec![1; 40], card: vec![2; 40] });
        let is_set = |outs: &[Out], conn: ConnId| frames(outs, conn).iter().any(|f| matches!(f, Frame::Carnet(Carnet::RecoverySet)));

        let code = DeviceKey::generate(&mut raw.rng).public();
        let outs = raw.send(a, &set(code));
        assert!(is_set(&outs, a), "A's own code");
        let outs = raw.send(b, &set(code));
        assert!(refused(&outs, b).is_some() && !is_set(&outs, b), "B naming A's code as its own");
        assert_eq!(raw.hub.store().recovery(&code).map(|(acct, _)| acct).as_deref(), Some(A), "A's code is A's still");
        assert!(raw.hub.store().recovery_card(B).is_empty(), "B has none");

        // A passkey's key and a password's, of A: not B's code either.
        let passkey = DeviceKey::generate(&mut raw.rng).public();
        raw.hub.store_mut().add_passkey(A, PasskeySlot { public: passkey, wrapped: vec![3; 40], card: vec![4; 40], made: 0 }).unwrap();
        let password = DeviceKey::generate(&mut raw.rng).public();
        raw.hub
            .store_mut()
            .set_password(A, PasswordSlot { id: "a@example.com".into(), record: vec![5; 40], public: password, wrapped: vec![6; 40], card: vec![7; 40] })
            .unwrap();
        for key in [passkey, password] {
            let outs = raw.send(b, &set(key));
            assert!(refused(&outs, b).is_some() && !is_set(&outs, b), "another account's slot's key");
            assert!(raw.hub.store().recovery(&key).is_none(), "a recovery finds the slot it was, and nothing first");
        }
        assert_eq!(raw.hub.store().passkey_slot(&passkey).map(|(acct, _)| acct).as_deref(), Some(A));
        assert_eq!(raw.hub.store().password_slot(&password).map(|(acct, _)| acct).as_deref(), Some(A));

        // Its own code again, or a new one: let.
        let outs = raw.send(a, &set(code));
        assert!(is_set(&outs, a), "A setting its code again");
        let mine = DeviceKey::generate(&mut raw.rng).public();
        let outs = raw.send(b, &set(mine));
        assert!(is_set(&outs, b), "B's own new code");
        assert_eq!(raw.hub.store().recovery(&mine).map(|(acct, _)| acct).as_deref(), Some(B));

        // And the store itself refuses, whoever asks it.
        assert!(raw.hub.store_mut().set_recovery(B, code, vec![1], vec![2]).is_err(), "the store gives no key away");
        assert_eq!(raw.hub.store().recovery(&code).map(|(acct, _)| acct).as_deref(), Some(A));
        assert_eq!(raw.hub.store().recovery(&mine).map(|(acct, _)| acct).as_deref(), Some(B), "and B's own is as it was");
        let newer = DeviceKey::generate(&mut raw.rng).public();
        raw.hub.store_mut().set_recovery(A, newer, vec![1], vec![2]).unwrap();
        assert!(raw.hub.store().recovery(&code).is_none(), "a new code replaces the last");
        assert_eq!(raw.hub.store().recovery(&newer).map(|(acct, _)| acct).as_deref(), Some(A));
    }
}

/// A device id an account has had is never given again: not to another
/// key (which would be that device's name taken), and not to a removed
/// device (which would be let back in). The same device added again is
/// no change, and another account may have one of that id.
#[test]
fn a_device_id_the_account_has_had_is_never_given_again() {
    for store in stores() {
        let mut raw = Raw::new(store);
        let (a2, other) = (raw.a2.public(), DeviceKey::generate(&mut raw.rng).public());
        let s = raw.hub.store_mut();
        assert!(s.add_device(A, b"a2", other).is_err(), "another key under a2's id");
        s.add_device(A, b"a2", a2).unwrap();
        s.revoke_device(A, b"a2").unwrap();
        assert!(s.add_device(A, b"a2", a2).is_err(), "a removed device stays removed");
        let d = s.device(A, b"a2").unwrap();
        assert!(d.revoked && d.public == a2);
        s.add_device(B, b"a2", other).unwrap();

        // Over the socket: a recovery naming a device the account has.
        let recovery = DeviceKey::generate(&mut raw.rng);
        raw.hub.store_mut().set_recovery(A, recovery.public(), vec![1; 40], vec![2; 40]).unwrap();
        let newcomer = DeviceKey::generate(&mut raw.rng);
        for device in [&b"a1"[..], b"a2"] {
            let (conn, nonce) = raw.open("");
            let signature = recovery.sign(&recovery_message(ORIGIN, &recovery.public(), device, &newcomer.public(), &nonce));
            let outs = raw.send(conn, &Frame::Carnet(Carnet::Recover { public: recovery.public(), signature, device: device.to_vec(), device_public: newcomer.public() }));
            assert!(denied(&outs, conn).is_some(), "{device:?} taken over by a recovery");
        }
        assert_eq!(raw.hub.store().device(A, b"a1").unwrap().public, raw.a1.public());
        assert!(raw.hub.store().device(A, b"a2").unwrap().revoked);
    }
}

/// A login naming an account that isn't here, and one naming a device
/// that isn't in an account that is, are answered alike, frame for frame:
/// a login tells nobody which accounts there are. (Either way the app
/// forgets the account: an account deleted while its device was away is
/// the first.) A device removed is told so; a device of the account
/// signed with the wrong key is only turned away.
#[test]
fn a_login_tells_nobody_which_accounts_there_are() {
    use carnet_sync::wire::Deny;
    for store in stores() {
        let mut raw = Raw::new(store);
        let b1 = raw.b1.clone();
        let mut n = 0;
        let mut denial = |raw: &mut Raw, account: &[u8], device: &[u8]| {
            // Each from an address of its own: no limit on tries meets it.
            n += 1;
            let (conn, nonce) = raw.open(&format!("198.51.100.{n}"));
            let outs = raw.send(conn, &Raw::hello(account, device, &b1, ORIGIN, &nonce));
            assert!(denied(&outs, conn).is_some(), "turned away");
            frames(&outs, conn).into_iter().find(|f| matches!(f, Frame::Carnet(Carnet::Denied { .. }))).unwrap()
        };
        let unknown = denial(&mut raw, b"no-such-account", b"b1");
        assert_eq!(denial(&mut raw, A, b"b1"), unknown, "B's device, named as A's");
        assert_eq!(denial(&mut raw, A, b"a9"), unknown, "a device A never had");
        let Frame::Carnet(Carnet::Denied { why, .. }) = &unknown else { unreachable!() };
        assert_eq!(*why, Deny::AccountRemoved);
        // A device of A's, signed with another's key: only turned away.
        let wrong = denial(&mut raw, A, b"a1");
        assert!(matches!(wrong, Frame::Carnet(Carnet::Denied { why: Deny::Other, .. })), "{wrong:?}");
        // A device removed is told so, to forget the account.
        raw.hub.store_mut().revoke_device(A, b"a2").unwrap();
        let removed = denial(&mut raw, A, b"a2");
        assert!(matches!(removed, Frame::Carnet(Carnet::Denied { why: Deny::DeviceRemoved, .. })), "{removed:?}");
    }
}
