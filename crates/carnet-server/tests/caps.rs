//! What one account may ask of the server at once, and hold of notes
//! (docs/SYNC.md §6, §11): an offer of pictures is answered for so many
//! and gets no more links than there's room for; a room id is short; a
//! room past what it keeps uncompacted takes no more until it is, an
//! account no more rooms or notes past its own caps; and what compaction
//! needs to know is counted, not read. On memory and SQLite.

mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use carnet_server::hub::{MAX_ROOM_ID, MAX_ROOMS, QUIET};
use carnet_server::quota::{FULL, Limit, NOTES_FULL, Quota, ROOM_FULL};
use carnet_server::{ConnId, Memory, Storage};
use carnet_sync::crypto::{VaultKey, sealed_len};
use carnet_sync::session::Event;
use carnet_sync::wire::{self, Carnet, CrdtType, Frame, MAX_IDS};
use carnet_sync::{NoteDoc, Origin};
use common::*;
use loro_protocol::ProtocolMessage;

const NOW: u64 = 1_760_000_000;

/// A plan: a limit, and a ceiling for notes.
struct Plan {
    limit: Mutex<Limit>,
    notes: u64,
}

impl Quota for Plan {
    fn limit(&self, _: &[u8]) -> Limit {
        self.limit.lock().unwrap().clone()
    }
    fn notes_ceiling(&self, _: &[u8]) -> u64 {
        self.notes
    }
}

fn events(net: &mut Net, conn: ConnId) -> Vec<Event> {
    std::mem::take(&mut net.device(conn).events)
}

/// `frame` from `conn` as it is, and what comes back to it, the picture
/// worker's answers among them, read here rather than by the device.
fn raw(net: &mut Net, conn: ConnId, frame: &Frame) -> Vec<Frame> {
    let outs = net.hub.receive(conn, &wire::encode(frame), &mut net.rng);
    net.route(outs);
    let mut back = Vec::new();
    while let Some((c, b)) = net.down.pop_front() {
        if c == conn {
            back.push(wire::decode(&b).unwrap());
        }
    }
    back
}

fn id(n: u32) -> [u8; 32] {
    let mut id = [0xee; 32];
    id[..4].copy_from_slice(&n.to_be_bytes());
    id
}

/// A save of `room` on `conn`, a line more.
fn write_to(net: &mut Net, conn: ConnId, room: &str, text: &str) {
    let Net { peers, rng, .. } = net;
    let Some(Peer::Device(d)) = peers.get_mut(&conn) else { panic!() };
    let doc = d.notes.docs.entry(room.to_string()).or_insert_with(|| NoteDoc::new(d.notes.peer));
    let mut all = doc.text();
    all.push_str(text);
    doc.record(&all, Origin::Save);
    d.session.saved(room, &mut d.notes, rng);
    net.pump();
}

/// An offer of thousands of pictures is answered for [`MAX_IDS`] of
/// them, and of those, no more get a link than there's room for, the
/// links out counted in; another offer before any is put gets none. A
/// device sends its offers in frames of that many, each picture with its
/// length sealed.
#[test]
fn an_offer_is_answered_for_so_many_and_no_more_than_there_is_room_for() {
    for store in stores() {
        let mut net = Net::new(90, store);
        let plan = Arc::new(Plan { limit: Mutex::new(Limit { bytes: Some(20_000), ..Limit::default() }), notes: u64::MAX });
        net.hub.set_quota(plan);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);

        let offer = |from: u32, n: u32| Frame::Carnet(Carnet::BlobOffer { ids: (from..from + n).map(id).collect(), sizes: vec![1000; n as usize] });
        let said = raw(&mut net, a, &offer(0, 5000));
        let links = said.iter().filter(|f| matches!(f, Frame::Carnet(Carnet::BlobLink { put: true, .. }))).count();
        assert_eq!(links, 20, "20,000 bytes of room, 1,000 a picture");
        assert!(said.len() <= MAX_IDS, "{} answers", said.len());
        assert_eq!(said.iter().filter(|f| **f == Frame::Carnet(Carnet::Refused { reason: FULL.into() })).count(), 1);
        assert_eq!(net.worker.pending(ACCOUNT), (20, 20_000));
        // Again, before any is put: nothing more.
        let said = raw(&mut net, a, &offer(10_000, 64));
        assert!(!said.iter().any(|f| matches!(f, Frame::Carnet(Carnet::BlobLink { .. }))), "{said:?}");

        // A device's offer of 150: three frames, each picture's length as
        // sealed said with it.
        let pics: Vec<([u8; 32], usize)> = (0..150).map(|n| (id(20_000 + n), 100 + n as usize)).collect();
        net.device(a).session.offer_blobs(pics.clone());
        let sent: Vec<Frame> = net.device(a).session.take_out().iter().map(|b| wire::decode(b).unwrap()).collect();
        let offers: Vec<_> = sent
            .iter()
            .filter_map(|f| if let Frame::Carnet(Carnet::BlobOffer { ids, sizes }) = f { Some((ids.clone(), sizes.clone())) } else { None })
            .collect();
        assert_eq!(offers.iter().map(|(ids, _)| ids.len()).collect::<Vec<_>>(), [64, 64, 22]);
        let (ids, sizes): (Vec<_>, Vec<_>) = offers.into_iter().unzip();
        assert_eq!(ids.concat(), pics.iter().map(|(id, _)| *id).collect::<Vec<_>>());
        assert_eq!(sizes.concat(), pics.iter().map(|(_, n)| sealed_len(*n)).collect::<Vec<_>>());
    }
}

/// A room id longer than any device makes is never joined, written or
/// repaired; nor is a room the server doesn't keep repaired.
#[test]
fn a_long_room_id_is_refused() {
    for store in stores() {
        let mut net = Net::new(91, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let long = "r".repeat(MAX_ROOM_ID + 1);
        let join = |room: &str| Frame::Room(ProtocolMessage::JoinRequest { crdt: CrdtType::Elo, room_id: room.into(), auth: Vec::new(), version: Vec::new() });
        let said = raw(&mut net, a, &join(&long));
        assert!(matches!(&said[..], [Frame::Room(ProtocolMessage::JoinError { .. })]), "{said:?}");
        let said = raw(&mut net, a, &join(&"r".repeat(MAX_ROOM_ID)));
        assert!(matches!(&said[..], [Frame::Room(ProtocolMessage::JoinResponseOk { .. })]), "{said:?}");
        // Not joined: a write to it is refused.
        let update = Frame::Room(ProtocolMessage::DocUpdate { crdt: CrdtType::Elo, room_id: long.clone(), updates: Vec::new(), batch_id: wire::BatchId([1; 8]) });
        assert!(matches!(&raw(&mut net, a, &update)[..], [Frame::Room(ProtocolMessage::Ack { status: wire::UpdateStatusCode::PermissionDenied, .. })]));
        // Nor repaired, nor a room with nothing kept.
        for room in [long.clone(), "nothing-here".into()] {
            let said = raw(&mut net, a, &Frame::Carnet(Carnet::Repair { room }));
            assert!(matches!(&said[..], [Frame::Carnet(Carnet::Refused { .. })]), "{said:?}");
        }
        assert_eq!(net.hub.compaction_kept().1, 0, "no lease held");
        assert!(net.hub.store().rooms(ACCOUNT).is_empty());
    }
}

/// A room past what it keeps uncompacted takes no more: the device is
/// told once, and holds what it writes rather than sending it to be
/// refused again. Once it's compacted, what waited goes.
#[test]
fn a_room_past_its_cap_takes_no_more_until_compacted() {
    for store in stores() {
        let mut net = Net::new(92, store);
        net.hub.tick(NOW);
        // No compaction, for now; five records a room at most.
        net.hub.compact_when(1000, 1 << 30);
        net.hub.room_caps(5, u64::MAX, MAX_ROOMS);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        for i in 0..5 {
            write_to(&mut net, a, ROOM, &format!("line {i}\n"));
        }
        assert_eq!(net.hub.store().live(ACCOUNT, ROOM).records, 5);
        let _ = events(&mut net, a);

        // The sixth: refused, said once; the device holds on to it.
        let refused = net.hub.stats().batches_refused;
        write_to(&mut net, a, ROOM, "line 5\n");
        assert_eq!(net.hub.store().live(ACCOUNT, ROOM).records, 5);
        assert_eq!(net.hub.stats().batches_refused, refused + 1);
        assert_eq!(events(&mut net, a), [Event::Refused(ROOM_FULL.into())]);
        assert_eq!(net.device(a).session.waiting(), 1);
        // The next isn't sent to be refused again.
        write_to(&mut net, a, ROOM, "line 6\n");
        assert_eq!(net.hub.stats().batches_refused, refused + 1);
        assert!(events(&mut net, a).is_empty());
        // Another room still takes writes.
        write_to(&mut net, a, "0000000000000000000000000000cafe", "elsewhere\n");
        assert_eq!(net.hub.store().live(ACCOUNT, "0000000000000000000000000000cafe").records, 1);

        // Compaction on: at the next login the room is compacted, and
        // what waited goes.
        net.hub.compact_when(3, 1 << 30);
        net.hub.room_caps(5, u64::MAX, MAX_ROOMS);
        let away = net.take(a);
        let a = net.back(away);
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_some(), "compacted");
        assert_eq!(net.device(a).session.waiting(), 0, "what waited went");
        let b = net.pair(a, b"laptop", 2);
        assert!(net.device(b).notes.docs[ROOM].text().contains("line 6"));
    }
}

/// An account with as many rooms as it may have makes no more; one whose
/// notes are past its ceiling takes no more writes.
#[test]
fn an_account_past_its_rooms_or_its_ceiling_takes_no_more() {
    for store in stores() {
        let mut net = Net::new(93, store);
        net.hub.room_caps(10_000, u64::MAX, 2);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        write_to(&mut net, a, ROOM, "one\n");
        write_to(&mut net, a, "0000000000000000000000000000cafe", "two\n");
        let _ = events(&mut net, a);
        write_to(&mut net, a, "0000000000000000000000000000f00d", "three\n");
        assert_eq!(events(&mut net, a), [Event::Refused(NOTES_FULL.into())]);
        assert_eq!(net.hub.store().room_count(ACCOUNT), 2);
        // A room it has still takes writes.
        write_to(&mut net, a, ROOM, "more\n");
        assert_eq!(net.hub.store().live(ACCOUNT, ROOM).records, 2);
    }
    for store in stores() {
        // A ceiling for notes: reached, no more.
        let mut net = Net::new(94, store);
        net.hub.set_quota(Arc::new(Plan { limit: Mutex::new(Limit::default()), notes: 1 }));
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        write_to(&mut net, a, ROOM, "one\n");
        let _ = events(&mut net, a);
        let before = net.hub.store().usage(ACCOUNT);
        write_to(&mut net, a, ROOM, "two\n");
        assert_eq!(events(&mut net, a), [Event::Refused(NOTES_FULL.into())]);
        assert_eq!(net.hub.store().usage(ACCOUNT), before);
    }
}

/// Whether a room is due for compaction is counted where its records are,
/// none of them read: a write reads no room whole. Each store counts
/// alike.
#[test]
fn compaction_is_judged_without_reading_a_record() {
    let memory = Memory::default();
    let reads = memory.spans_read();
    let mut net = Net::new(95, Box::new(memory));
    net.hub.tick(NOW);
    net.hub.compact_when(10, 1 << 30);
    let vault = VaultKey::generate(&mut net.rng);
    let a = net.first_device(&vault);
    write_to(&mut net, a, ROOM, "joined\n");
    let before = reads.load(Ordering::Relaxed);
    for i in 0..12 {
        write_to(&mut net, a, ROOM, &format!("line {i}\n"));
    }
    assert!(net.hub.store().base(ACCOUNT, ROOM).is_some(), "compacted all the same");
    assert_eq!(reads.load(Ordering::Relaxed), before, "a write read a room whole");

    for store in stores() {
        let mut net = Net::new(96, store);
        let a = net.first_device(&vault);
        for i in 0..4 {
            write_to(&mut net, a, ROOM, &format!("line {i}\n"));
        }
        let spans = net.hub.store().spans(ACCOUNT, ROOM);
        let live = net.hub.store().live(ACCOUNT, ROOM);
        assert_eq!((live.records, live.bytes, live.based), (spans.len() as u64, spans.iter().map(|s| s.record.len() as u64).sum(), false));
        assert!(live.records > 0);
        assert!(net.hub.store().live(ACCOUNT, "nothing-here").is_empty());
        assert_eq!(net.hub.store().room_count(ACCOUNT), 1);
    }
}

/// What the hub keeps for compaction goes once it can't matter: a room's
/// last write past [`QUIET`], a lease past its time.
#[test]
fn what_compaction_kept_is_pruned() {
    for store in stores() {
        let mut net = Net::new(97, store);
        net.hub.tick(NOW);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        for room in [ROOM, "0000000000000000000000000000cafe"] {
            write_to(&mut net, a, room, "words\n");
        }
        raw(&mut net, a, &Frame::Carnet(Carnet::Repair { room: ROOM.into() }));
        assert_eq!(net.hub.compaction_kept(), (2, 1));
        net.hub.tick(NOW + 60);
        assert_eq!(net.hub.compaction_kept(), (2, 1), "within the hour: kept");
        net.hub.tick(NOW + 3600 + QUIET);
        assert_eq!(net.hub.compaction_kept(), (0, 0));
    }
}
