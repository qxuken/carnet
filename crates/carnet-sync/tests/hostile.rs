//! A server that isn't what it seems, as a device sees it: what it names
//! makes no note that isn't one, nor more than so many at once, and its
//! words aren't shown as Carnet's.

use std::collections::BTreeMap;

use carnet_sync::compat::{self, Greeting};
use carnet_sync::crypto::{DeviceKey, OsRandom, Ring, VaultKey};
use carnet_sync::elo::{self, Header};
use carnet_sync::session::{Docs, Event, Identity, MAX_NEW_ROOMS, FILES_UNREACHABLE, Session, TOO_MANY_NEW};
use carnet_sync::vault;
use carnet_sync::wire::{self, Carnet, CrdtType, Frame, Permission};
use carnet_sync::{Key, NoteDoc, Origin};
use loro_protocol::{BatchId, ProtocolMessage};

struct Notes(BTreeMap<String, NoteDoc>);

impl Docs for Notes {
    fn rooms(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
    fn doc(&mut self, room: &str) -> &NoteDoc {
        self.0.entry(room.to_string()).or_insert_with(|| NoteDoc::new(1))
    }
}

/// A device logged in to the server, its docs `notes`.
fn logged_in(notes: &mut Notes) -> Session {
    logged_in_as(notes, Ring::new(VaultKey::generate(&mut OsRandom)))
}

/// The same, its vault's keys `ring`.
fn logged_in_as(notes: &mut Notes, ring: Ring) -> Session {
    let id = Identity {
        origin: "https://notes.example".into(),
        account: b"acct".to_vec(),
        device: b"desk".to_vec(),
        key: DeviceKey::generate(&mut OsRandom),
        ring,
        name: "Desk".into(),
    };
    let mut s = Session::new(id, 0);
    s.connected();
    let server = Greeting { protocol: compat::PROTOCOL, oldest: compat::OLDEST, deprecated: compat::DEPRECATED, features: Vec::new() };
    s.receive(&wire::encode(&Frame::Carnet(Carnet::Challenge { nonce: [3; 32], server })), notes, &mut OsRandom);
    s.receive(&wire::encode(&Frame::Carnet(Carnet::Welcome { seq: 0 })), notes, &mut OsRandom);
    assert!(s.is_ready());
    s.take_out();
    s.take_events();
    s
}

/// A link to put a file at that isn't followed (on a host of this
/// network other than the server's, or plain http from an https server):
/// nothing is put there, and why the file waits is said, once a login.
#[test]
fn a_picture_store_out_of_reach_is_said_once_a_login() {
    let mut notes = Notes(BTreeMap::new());
    let mut s = logged_in(&mut notes);
    let link = |id: u8, url: &str| wire::encode(&Frame::Carnet(Carnet::BlobLink { id: [id; 32], put: true, url: url.into(), expires: 0 }));
    s.receive(&link(1, "https://10.0.0.5:9000/bucket/x"), &mut notes, &mut OsRandom);
    s.receive(&link(2, "http://pictures.example/bucket/y"), &mut notes, &mut OsRandom);
    assert_eq!(s.take_events(), [Event::Failed(FILES_UNREACHABLE.into())]);
    assert!(s.take_transfers().is_empty());
    // A link that is followed says nothing of it.
    s.receive(&link(3, "https://pictures.example/bucket/z"), &mut notes, &mut OsRandom);
    assert_eq!(s.take_events(), [Event::BlobWanted([3; 32])]);
    // Logged in again: said again, if it's so still.
    s.disconnected();
    s.connected();
    let server = Greeting { protocol: compat::PROTOCOL, oldest: compat::OLDEST, deprecated: compat::DEPRECATED, features: Vec::new() };
    s.receive(&wire::encode(&Frame::Carnet(Carnet::Challenge { nonce: [3; 32], server })), &mut notes, &mut OsRandom);
    s.receive(&wire::encode(&Frame::Carnet(Carnet::Welcome { seq: 0 })), &mut notes, &mut OsRandom);
    s.take_events();
    s.receive(&link(1, "https://10.0.0.5:9000/bucket/x"), &mut notes, &mut OsRandom);
    assert_eq!(s.take_events(), [Event::Failed(FILES_UNREACHABLE.into())]);
}

fn changed(rooms: Vec<(String, u64)>) -> Vec<u8> {
    wire::encode(&Frame::Carnet(Carnet::Changed { rooms }))
}

fn key(n: u128) -> String {
    Key::parse(&format!("{n:032x}")).unwrap().to_string()
}

/// Rooms named that no note has (not a key's hex, or not as a key is
/// written) make no doc and aren't joined; a note's and the vault's are.
#[test]
fn a_name_no_note_has_makes_nothing() {
    let mut notes = Notes(BTreeMap::new());
    let mut s = logged_in(&mut notes);
    let odd = ["../../etc", "x", "", "0000000000000000000000000000ABCD", "000000000000000000000000000000abc", "0000000000000000000000000000000g", "vault2"];
    let mut rooms: Vec<(String, u64)> = odd.iter().map(|r| (r.to_string(), 1)).collect();
    rooms.push((key(0xabcd), 2));
    rooms.push((vault::ROOM.to_string(), 3));
    s.receive(&changed(rooms), &mut notes, &mut OsRandom);
    let mut made = notes.rooms();
    made.sort();
    assert_eq!(made, [key(0xabcd), vault::ROOM.to_string()]);
    assert_eq!(s.cursor(), 3, "named, if not taken: not named again");
    let joined: Vec<String> = s
        .take_out()
        .iter()
        .filter_map(|b| match wire::decode(b) {
            Ok(Frame::Room(ProtocolMessage::JoinRequest { room_id, .. })) => Some(room_id),
            _ => None,
        })
        .collect();
    assert_eq!(joined, [key(0xabcd), vault::ROOM.to_string()]);
}

/// A server that answers a join never asked for makes no doc by it.
#[test]
fn an_answer_to_no_question_makes_nothing() {
    let mut notes = Notes(BTreeMap::new());
    let mut s = logged_in(&mut notes);
    for permission in [Permission::Read, Permission::Write] {
        let answer = ProtocolMessage::JoinResponseOk { crdt: CrdtType::Elo, room_id: key(7), permission, version: Vec::new(), extra: None };
        s.receive(&wire::encode(&Frame::Room(answer)), &mut notes, &mut OsRandom);
    }
    assert!(notes.rooms().is_empty());
    assert!(s.take_events().is_empty() && s.take_out().is_empty());
}

/// The rooms a device asked to join.
fn joins(s: &mut Session) -> Vec<String> {
    s.take_out()
        .iter()
        .filter_map(|b| match wire::decode(b) {
            Ok(Frame::Room(ProtocolMessage::JoinRequest { room_id, .. })) => Some(room_id),
            _ => None,
        })
        .collect()
}

fn joined(room: &str, held: &[(u64, u64)]) -> Vec<u8> {
    let ok = ProtocolMessage::JoinResponseOk { crdt: CrdtType::Elo, room_id: room.into(), permission: Permission::Write, version: wire::encode_vv(held), extra: None };
    wire::encode(&Frame::Room(ok))
}

fn update(room: &str, records: &[Vec<u8>]) -> Vec<u8> {
    let m = ProtocolMessage::DocUpdate { crdt: CrdtType::Elo, room_id: room.into(), updates: vec![elo::container(records)], batch_id: BatchId([0; 8]) };
    wire::encode(&Frame::Room(m))
}

/// More new rooms named than a login takes: [`MAX_NEW_ROOMS`] of them
/// made and joined, the cursor where it was, nothing said yet.
fn too_many(s: &mut Session, notes: &mut Notes) -> (u128, Vec<String>) {
    let many = MAX_NEW_ROOMS as u128 + 5;
    let rooms: Vec<(String, u64)> = (1..=many).map(|n| (key(n), n as u64)).collect();
    s.receive(&changed(rooms), notes, &mut OsRandom);
    assert_eq!(notes.0.len(), MAX_NEW_ROOMS);
    assert_eq!(s.cursor(), 0, "the rest named again next login");
    let asked = joins(s);
    assert_eq!(asked.len(), MAX_NEW_ROOMS);
    assert!(s.take_events().is_empty(), "nothing said before they answer");
    (many, asked)
}

/// The vault named with as many notes as a login takes: all of them
/// taken, the vault not counted among them.
#[test]
fn the_vault_is_never_one_of_the_new_notes_a_login_takes() {
    let mut notes = Notes(BTreeMap::new());
    let mut s = logged_in(&mut notes);
    let mut rooms: Vec<(String, u64)> = vec![(vault::ROOM.to_string(), 1)];
    rooms.extend((1..=MAX_NEW_ROOMS as u128).map(|n| (key(n), n as u64 + 1)));
    s.receive(&changed(rooms), &mut notes, &mut OsRandom);
    assert_eq!(notes.0.len(), MAX_NEW_ROOMS + 1);
    assert_eq!(s.cursor(), MAX_NEW_ROOMS as u64 + 1, "nothing left for the next login");
    assert_eq!(joins(&mut s).len(), MAX_NEW_ROOMS + 1);
}

/// So many new rooms in a login and no more: the rest wait for the next,
/// and if those taken came empty, that's said once.
#[test]
fn so_many_new_rooms_a_login_and_no_more() {
    let mut notes = Notes(BTreeMap::new());
    let mut s = logged_in(&mut notes);
    let (many, asked) = too_many(&mut s, &mut notes);
    for room in &asked {
        s.receive(&joined(room, &[]), &mut notes, &mut OsRandom);
    }
    assert_eq!(s.take_events(), [Event::Failed(TOO_MANY_NEW.into())]);
    // Named again, live: still no more, and not said again.
    s.receive(&changed(vec![(key(many + 1), many as u64 + 1)]), &mut notes, &mut OsRandom);
    assert_eq!(notes.0.len(), MAX_NEW_ROOMS);
    assert!(s.take_events().is_empty());
    // One already here, named again: taken, as nothing new is made.
    s.receive(&changed(vec![(key(1), many as u64 + 2)]), &mut notes, &mut OsRandom);
    assert_eq!(s.cursor(), many as u64 + 2);
}

/// A server saying it holds something of each, and sending what no key
/// here opens, has named nothing real: said, not connected again for.
#[test]
fn rooms_said_to_hold_words_that_dont_open_are_empty() {
    let mut notes = Notes(BTreeMap::new());
    let mut s = logged_in(&mut notes);
    let (_, asked) = too_many(&mut s, &mut notes);
    let other = VaultKey::generate(&mut OsRandom);
    let doc = NoteDoc::new(9);
    doc.record("made up", Origin::Save);
    for room in &asked {
        s.receive(&joined(room, &[(9, 1)]), &mut notes, &mut OsRandom);
        let record = elo::seal(&other, room, &Header::Delta { peer: 9, start: 0, end: doc.counter(9), epoch: 0 }, &doc.updates(), &mut OsRandom);
        s.receive(&update(room, &[record]), &mut notes, &mut OsRandom);
    }
    let events = s.take_events();
    assert!(!events.contains(&Event::MoreToCome));
    assert_eq!(events.iter().filter(|e| **e == Event::Failed(TOO_MANY_NEW.into())).count(), 1);
}

/// An account's own notes, more than a login takes: once each taken has
/// brought words the vault's key opens, the next login is asked for at
/// once, to bring the rest, and nothing is said.
#[test]
fn an_accounts_many_notes_come_in_logins_one_after_another() {
    let mut notes = Notes(BTreeMap::new());
    let ring = Ring::new(VaultKey::generate(&mut OsRandom));
    let mut s = logged_in_as(&mut notes, ring.clone());
    let (_, asked) = too_many(&mut s, &mut notes);
    let (epoch, vault) = ring.current();
    let doc = NoteDoc::new(9);
    doc.record("a note", Origin::Save);
    let end = doc.counter(9);
    for (i, room) in asked.iter().enumerate() {
        s.receive(&joined(room, &[(9, end)]), &mut notes, &mut OsRandom);
        let record = elo::seal(vault, room, &Header::Delta { peer: 9, start: 0, end, epoch }, &doc.updates(), &mut OsRandom);
        s.receive(&update(room, &[record]), &mut notes, &mut OsRandom);
        let more = s.take_events().contains(&Event::MoreToCome);
        assert_eq!(more, i + 1 == asked.len(), "asked for once, when the last has come");
    }
    assert_eq!(notes.doc(&asked[0]).text(), "a note");
}

/// Caught up with the account once what the server holds of every room
/// named has come, not when each join is answered: the words come after,
/// one room after another. A room it holds nothing of has come at once.
#[test]
fn caught_up_once_every_rooms_words_have_come() {
    let mut notes = Notes(BTreeMap::new());
    let ring = Ring::new(VaultKey::generate(&mut OsRandom));
    let mut s = logged_in_as(&mut notes, ring.clone());
    let rooms = [vault::ROOM.to_string(), key(1), key(2), key(3)];
    s.receive(&changed(rooms.iter().enumerate().map(|(i, r)| (r.clone(), i as u64 + 1)).collect()), &mut notes, &mut OsRandom);
    assert_eq!(joins(&mut s).len(), 4);
    assert!(!s.has_caught_up(), "asked, not answered");
    let (epoch, vault) = ring.current();
    let doc = NoteDoc::new(9);
    doc.record("a note", Origin::Save);
    let end = doc.counter(9);
    for room in &rooms[..3] {
        s.receive(&joined(room, &[(9, end)]), &mut notes, &mut OsRandom);
    }
    s.receive(&joined(&rooms[3], &[]), &mut notes, &mut OsRandom);
    assert!(s.has_joined(vault::ROOM));
    assert!(!s.has_caught_up(), "every join answered, the words not come");
    for (i, room) in rooms[..3].iter().enumerate() {
        let record = elo::seal(vault, room, &Header::Delta { peer: 9, start: 0, end, epoch }, &doc.updates(), &mut OsRandom);
        s.receive(&update(room, &[record]), &mut notes, &mut OsRandom);
        assert_eq!(s.has_caught_up(), i == 2, "caught up once the last has come");
    }
    // Gone again: not caught up until the next login has.
    s.disconnected();
    assert!(!s.has_caught_up());
}

/// What the server calls the account's plan is its own words: shown on
/// one line, nothing in it that hides letters or turns them around.
#[test]
fn a_plan_is_the_servers_words_made_fit_to_show() {
    let mut notes = Notes(BTreeMap::new());
    let mut s = logged_in(&mut notes);
    let plan = "Free\u{202e}\u{2066}\u{200b}\nplan\u{e0041}".to_string();
    let usage = Carnet::UsageIs { notes: 1, files: 2, limit: 0, writable: true, plan, until: 0 };
    s.receive(&wire::encode(&Frame::Carnet(usage)), &mut notes, &mut OsRandom);
    let plan = s.take_events().into_iter().find_map(|e| match e {
        Event::Usage(u) => Some(u.plan),
        _ => None,
    });
    assert_eq!(plan.as_deref(), Some("Free plan"));
}
