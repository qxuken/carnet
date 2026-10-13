//! Compaction and repair (docs/SYNC.md §6, §9), through the hub: a room
//! past its limits becomes one snapshot, asked of one device under a lease
//! when nobody else is writing; what it stands for is set aside, not gone,
//! for 30 days; the next device to open it checks it, and one that fails
//! is rolled back. On memory and on SQLite alike.

mod common;

use carnet_server::hub::{KEEP, QUIET};
use carnet_server::storage::covers;
use carnet_server::{ConnId, Out, Storage};
use carnet_sync::crypto::{Random, VaultKey};
use carnet_sync::elo::{self, Header};
use carnet_sync::session::{Docs, Event};
use carnet_sync::wire::{self, CrdtType, Frame};
use carnet_sync::NoteDoc;
use common::*;
use loro_protocol::ProtocolMessage;

const NOW: u64 = 1_760_000_000;

fn net(seed: u64, store: Box<dyn Storage>) -> (Net, VaultKey) {
    let mut net = Net::new(seed, store);
    net.hub.tick(NOW);
    net.hub.compact_when(5, 1 << 30);
    let vault = VaultKey::generate(&mut net.rng);
    (net, vault)
}

/// `n` saves of the room, each a line more.
fn write_lines(net: &mut Net, conn: ConnId, from: usize, n: usize) {
    for i in from..from + n {
        let text: String = (0..=i).map(|k| format!("line {k}\n")).collect();
        net.write(conn, &text);
    }
}

fn text(net: &mut Net, conn: ConnId) -> String {
    net.device(conn).notes.docs.get(ROOM).map(NoteDoc::text).unwrap_or_default()
}

#[test]
fn a_room_past_its_limits_becomes_a_base_a_new_device_starts_from() {
    for store in stores() {
        let (mut net, vault) = net(30, store);
        let a = net.first_device(&vault);
        write_lines(&mut net, a, 0, 4);
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_none(), "under its limits");
        write_lines(&mut net, a, 4, 3);
        // Alone online, past five records: asked, and its snapshot taken.
        let base = net.hub.store().base(ACCOUNT, ROOM).expect("a base");
        assert_eq!(base.by, b"first");
        assert!(!base.verified, "nobody has opened it yet");
        // Asked at the fifth save: it stands for those five; the two saves
        // after are live records past it.
        let shelf = net.hub.store().shelf(ACCOUNT, ROOM);
        assert_eq!(shelf.count, 5, "what it stands for set aside: {shelf:?}");
        assert_eq!(net.hub.store().spans(ACCOUNT, ROOM).len(), 2);
        let all = net.device(a).notes.docs[ROOM].version_vector();
        assert!(covers(&all, &base.vv) && base.vv != all);

        // A new device: the base and what's past it, and it checks the
        // base, which is then verified.
        write_lines(&mut net, a, 7, 1);
        let b = net.pair(a, b"laptop", 2);
        assert_eq!(text(&mut net, b), text(&mut net, a));
        assert!(net.hub.store().base(ACCOUNT, ROOM).unwrap().verified);
        // And nothing in the clear.
        let base = net.hub.store().base(ACCOUNT, ROOM).unwrap();
        assert!(!base.record.windows(6).any(|w| w == b"line 3"));
    }
}

/// A snapshot that doesn't hold what its header says (a buggy client):
/// the next device to open it finds out before taking it in, says so, and
/// the server puts back what it stood for; that device then gets it all.
#[test]
fn a_snapshot_that_lies_is_caught_and_rolled_back() {
    for store in stores() {
        let (mut net, vault) = net(31, store);
        let a = net.first_device(&vault);
        // A's snapshot, swapped on its way for an empty doc under the same
        // header.
        let v = vault.clone();
        let mut rng = Seeded(99);
        net.meddle = Some((a, Box::new(move |b: &mut Vec<u8>| {
            let Ok(Frame::Room(ProtocolMessage::DocUpdate { room_id, updates, batch_id, .. })) = wire::decode(b) else { return };
            let Some(r) = updates.first().and_then(|c| elo::records(c).ok()).and_then(|rs| rs.first().map(|r| r.to_vec())) else { return };
            let Ok(Header::Snapshot { vv, epoch }) = elo::header(&r) else { return };
            let lie = elo::seal(&v, &room_id, &Header::Snapshot { vv, epoch }, &NoteDoc::new(5).to_bytes(), &mut rng);
            *b = wire::encode(&Frame::Room(ProtocolMessage::DocUpdate {
                crdt: CrdtType::Elo,
                room_id,
                updates: vec![elo::container(&[lie])],
                batch_id,
            }));
        })));
        write_lines(&mut net, a, 0, 7);
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_some(), "the server can't tell: it reads no snapshot");
        net.meddle = None;

        let b = net.pair(a, b"laptop", 2);
        assert!(net.device(b).events.contains(&Event::BadSnapshot(ROOM.into())));
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_none(), "rolled back");
        assert_eq!(net.hub.store().shelf(ACCOUNT, ROOM).count, 0);
        assert_eq!(text(&mut net, b), text(&mut net, a), "and the laptop has it all, from the records put back");
        let log = net.hub.store().logged(ACCOUNT);
        assert!(log[0].what.contains("did not hold what it said: rolled back"), "{log:?}");
        assert_eq!(log[0].device, b"laptop");
    }
}

/// Any device can roll a base back while what it stood for is kept: 30
/// days. After that it's gone, and the base stays.
#[test]
fn a_base_rolls_back_for_thirty_days() {
    for store in stores() {
        let (mut net, vault) = net(32, store);
        let a = net.first_device(&vault);
        write_lines(&mut net, a, 0, 7);
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_some());
        net.device(a).session.roll_back(ROOM);
        net.pump();
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_none());
        assert!(net.hub.store().spans(ACCOUNT, ROOM).len() >= 7, "every record live again");
        assert_eq!(net.hub.store().logged(ACCOUNT)[0].what, "rolled back");
        assert!(net.device(a).events.iter().any(|e| matches!(e, Event::Inspected(..))), "the inspector told");

        // Compacted again, then 30 days on.
        write_lines(&mut net, a, 7, 1);
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_some());
        net.hub.tick(NOW + KEEP + 3600);
        assert!(!net.hub.store().base(ACCOUNT, ROOM).unwrap().undoable);
        assert_eq!(net.hub.store().shelf(ACCOUNT, ROOM).count, 0, "what it set aside, gone");
        net.device(a).session.roll_back(ROOM);
        net.pump();
        assert!(net.device(a).events.iter().any(|e| matches!(e, Event::Refused(w) if w.contains("can't be rolled back"))));
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_some());
    }
}

/// While another device is online and the room took a record lately,
/// nobody is asked; and a snapshot from a device without the lease is
/// refused.
#[test]
fn nobody_compacts_while_others_write_nor_without_the_lease() {
    for store in stores() {
        let (mut net, vault) = net(33, store);
        let a = net.first_device(&vault);
        let b = net.pair(a, b"laptop", 2);
        write_lines(&mut net, a, 0, 4);
        write_lines(&mut net, b, 4, 4);
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_none(), "two writing: not now");

        // B sends a snapshot unasked: refused.
        let doc = &net.device(b).notes.docs[ROOM];
        let rec = elo::seal(&vault, ROOM, &Header::Snapshot { vv: doc.version_vector(), epoch: 0 }, &doc.to_bytes(), &mut Seeded(5));
        let f = wire::encode(&Frame::Room(ProtocolMessage::DocUpdate {
            crdt: CrdtType::Elo,
            room_id: ROOM.into(),
            updates: vec![elo::container(&[rec])],
            batch_id: wire::BatchId([7; 8]),
        }));
        let outs = net.hub.receive(b, &f, &mut net.rng);
        assert!(outs.iter().any(|o| matches!(o, Out::Send(_, bytes) if matches!(wire::decode(bytes),
            Ok(Frame::Room(ProtocolMessage::Ack { status: wire::UpdateStatusCode::PermissionDenied, .. }))))));
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_none());
        // Nor in fragments: the header is refused, and nothing after it
        // is kept to be put together.
        let header = wire::encode(&Frame::Room(ProtocolMessage::DocUpdateFragmentHeader {
            crdt: CrdtType::Elo,
            room_id: ROOM.into(),
            batch_id: wire::BatchId([8; 8]),
            fragment_count: 2,
            total_size_bytes: 2 * elo::FRAGMENT as u64,
        }));
        let outs = net.hub.receive(b, &header, &mut net.rng);
        assert!(outs.iter().any(|o| matches!(o, Out::Send(_, bytes) if matches!(wire::decode(bytes),
            Ok(Frame::Room(ProtocolMessage::Ack { status: wire::UpdateStatusCode::PermissionDenied, .. }))))));
        for index in 0..2 {
            let f = wire::encode(&Frame::Room(ProtocolMessage::DocUpdateFragment {
                crdt: CrdtType::Elo,
                room_id: ROOM.into(),
                batch_id: wire::BatchId([8; 8]),
                index,
                fragment: vec![1; elo::FRAGMENT],
            }));
            assert!(net.hub.receive(b, &f, &mut net.rng).is_empty());
        }
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_none());

        // Quiet a while: the next device to join is asked.
        net.hub.tick(NOW + QUIET + 1);
        let laptop = net.take(b);
        let b2 = net.back(laptop);
        let _ = b2;
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_some(), "asked on joining, the room quiet");
        assert_eq!(net.hub.store().base(ACCOUNT, ROOM).unwrap().by, b"laptop");
    }
}

/// A big import with another device online: the vault takes a record
/// for every note, its writer never quiet and never alone. Before the
/// room reaches what it keeps uncompacted, the writer is asked for it
/// compacted all the same (another device writing meanwhile is harmless:
/// what the snapshot doesn't hold stays), so the vault never stops
/// taking writes.
#[test]
fn a_room_near_its_cap_is_compacted_whoever_else_is_online() {
    const VAULT: &str = "vault";
    for store in stores() {
        let (mut net, vault) = net(36, store);
        net.hub.room_caps(40, u64::MAX, carnet_server::hub::MAX_ROOMS);
        let a = net.first_device(&vault);
        let b = net.pair(a, b"browser", 2);
        let mut text = String::new();
        for i in 0..120 {
            text.push_str(&format!("note {i}\n"));
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            d.notes.doc(VAULT).record(&text, carnet_sync::Origin::Save);
            d.session.saved(VAULT, &mut d.notes, rng);
            net.pump();
            let live = net.hub.store().live(ACCOUNT, VAULT);
            assert!(live.records < 40, "at save {i}, the vault holds {live:?}");
        }
        assert_eq!(net.device(a).session.waiting(), 0, "nothing held back");
        assert!(!net.device(a).events.iter().any(|e| matches!(e, Event::Refused(_))), "nothing refused");
        assert_eq!(net.hub.store().base(ACCOUNT, VAULT).unwrap().by, b"first");
        assert_eq!(net.device(b).notes.docs[VAULT].text(), text, "the other device has it all");
        let c = net.pair(a, b"phone", 3);
        assert_eq!(net.device(c).notes.docs[VAULT].text(), text, "and a new one, from the base");
    }
}

/// Repair: a device whose own copy is good makes it the base, whatever
/// the base holds now; it says so in the log. The inspector shows a room
/// as the server keeps it.
#[test]
fn a_device_replaces_the_servers_copy_and_the_inspector_shows_it() {
    for store in stores() {
        let (mut net, vault) = net(34, store);
        let a = net.first_device(&vault);
        write_lines(&mut net, a, 0, 3);
        // A base the server holds that claims more than anyone has (as a
        // bad one might), and so can't be compacted past.
        net.hub.store_mut().set_base(ACCOUNT, ROOM, vec![(424242, 9)], b"junk".to_vec(), b"ghost", NOW).unwrap();
        // Compaction can't get past it: a snapshot must cover the base.
        write_lines(&mut net, a, 3, 5);
        assert_eq!(net.hub.store().base(ACCOUNT, ROOM).unwrap().record, b"junk", "not covered: refused");
        net.device(a).session.repair(ROOM);
        net.pump();
        let base = net.hub.store().base(ACCOUNT, ROOM).unwrap();
        assert_eq!(base.by, b"first");
        assert_eq!(base.vv, net.device(a).notes.docs[ROOM].version_vector());
        assert_eq!(net.hub.store().logged(ACCOUNT)[0].what, "replaced the server's copy with its own");
        assert!(net.device(a).events.iter().any(|e| matches!(e, Event::Inspected(..))), "the inspector told, unasked");

        net.device(a).session.inspect();
        net.pump();
        let (rooms, log) = net.device(a).events.iter().rev().find_map(|e| match e {
            Event::Inspected(r, l) => Some((r.clone(), l.clone())),
            _ => None,
        })
        .unwrap();
        let row = rooms.iter().find(|r| r.room == ROOM).unwrap();
        let b = row.base.as_ref().unwrap();
        assert_eq!((b.by.as_slice(), b.verified, b.undoable), (&b"first"[..], false, true));
        assert_eq!(row.held, net.device(a).notes.docs[ROOM].version_vector());
        assert_eq!(row.shelved, 1 + 8, "the junk base and A's eight records, set aside");
        assert_eq!(log.len(), 1);
    }
}

/// A note whose snapshot, history and all, is past what a delta's record
/// may hold (and a message): compacted all the same, the snapshot going in
/// fragments both ways, and the note syncs on past the most a room keeps
/// uncompacted. A new device starts from that base, sent it in fragments.
#[test]
fn a_note_too_big_for_a_record_is_compacted_and_syncs_on() {
    for store in stores() {
        let (mut net, vault) = net(37, store);
        // Twenty records a room at most, uncompacted.
        net.hub.room_caps(20, u64::MAX, carnet_server::hub::MAX_ROOMS);
        let a = net.first_device(&vault);
        // Letters a compressor can't do much with: well past a record.
        let big: String = (0..3 * elo::MAX_PLAIN).map(|_| char::from(b'a' + net.rng.bytes::<1>()[0] % 26)).collect();
        let mut note = format!("# Big\n\n{big}\n");
        net.write(a, &note);
        for i in 0..60 {
            note.push_str(&format!("line {i}\n"));
            net.write(a, &note);
        }
        let base = net.hub.store().base(ACCOUNT, ROOM).expect("compacted");
        assert!(base.record.len() > elo::MAX_PLAIN && base.record.len() <= elo::MAX_SNAPSHOT, "{}", base.record.len());
        assert!(net.hub.store().live(ACCOUNT, ROOM).records < 20);
        let said = std::mem::take(&mut net.device(a).events);
        assert!(!said.iter().any(|e| matches!(e, Event::Refused(_))), "{said:?}");
        assert_eq!(net.device(a).session.waiting(), 0, "everything kept");

        // A new device: the base in fragments, checked, and the rest.
        let b = net.pair(a, b"laptop", 2);
        assert_eq!(text(&mut net, b), note);
        assert!(net.hub.store().base(ACCOUNT, ROOM).unwrap().verified, "checked by the new device");
        note.push_str("from the laptop\n");
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&b) else { panic!() };
            d.notes.docs.get_mut(ROOM).unwrap().record(&note, carnet_sync::Origin::Save);
            d.session.saved(ROOM, &mut d.notes, rng);
        }
        net.pump();
        assert_eq!(text(&mut net, a), note);
    }
}

/// A note whose snapshot is past what a snapshot may weigh (made small
/// here): asked for it, the device sends none and says why, once, in its
/// own words; the room fills, and what's written after waits on the
/// device, said too.
#[test]
fn a_note_too_big_to_compact_is_said() {
    use carnet_server::quota::ROOM_FULL;
    use carnet_sync::session::{TOO_BIG, refusal_words};
    for store in stores() {
        let (mut net, vault) = net(38, store);
        net.hub.room_caps(20, u64::MAX, carnet_server::hub::MAX_ROOMS);
        let a = net.first_device(&vault);
        net.device(a).session.set_max_snapshot(8 * 1024);
        let big: String = (0..16 * 1024).map(|_| char::from(b'a' + net.rng.bytes::<1>()[0] % 26)).collect();
        let mut note = format!("# Big\n\n{big}\n");
        for i in 0..30 {
            note.push_str(&format!("line {i}\n"));
            net.write(a, &note);
        }
        assert!(net.hub.store().base(ACCOUNT, ROOM).is_none(), "not compacted");
        let said: Vec<String> = std::mem::take(&mut net.device(a).events)
            .into_iter()
            .filter_map(|e| if let Event::Refused(why) = e { Some(why) } else { None })
            .collect();
        assert_eq!(said.iter().filter(|w| *w == TOO_BIG).count(), 1, "{said:?}");
        assert!(said.iter().any(|w| w == ROOM_FULL), "{said:?}");
        assert_eq!(refusal_words(TOO_BIG), TOO_BIG, "its own words, not the server's");
        assert_eq!(net.device(a).session.waiting(), 1, "what's written after waits here");
    }
}
