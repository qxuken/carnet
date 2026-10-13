//! A server that isn't what it seems, as a device sees it: what it names
//! makes no note that isn't one, nor more than so many at once, and its
//! words aren't shown as Carnet's.

use std::collections::BTreeMap;

use carnet_sync::compat::{self, Greeting};
use carnet_sync::crypto::{DeviceKey, OsRandom, Ring, VaultKey};
use carnet_sync::session::{Docs, Event, Identity, MAX_NEW_ROOMS, Session, TOO_MANY_NEW};
use carnet_sync::vault;
use carnet_sync::wire::{self, Carnet, CrdtType, Frame, Permission};
use carnet_sync::{Key, NoteDoc};
use loro_protocol::ProtocolMessage;

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
    let id = Identity {
        origin: "https://notes.example".into(),
        account: b"acct".to_vec(),
        device: b"desk".to_vec(),
        key: DeviceKey::generate(&mut OsRandom),
        ring: Ring::new(VaultKey::generate(&mut OsRandom)),
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

/// So many new rooms in a login and no more: the rest wait for the next,
/// the cursor staying where it was, and that's said once.
#[test]
fn so_many_new_rooms_a_login_and_no_more() {
    let mut notes = Notes(BTreeMap::new());
    let mut s = logged_in(&mut notes);
    let many = MAX_NEW_ROOMS as u128 + 5;
    let rooms: Vec<(String, u64)> = (1..=many).map(|n| (key(n), n as u64)).collect();
    s.receive(&changed(rooms), &mut notes, &mut OsRandom);
    assert_eq!(notes.0.len(), MAX_NEW_ROOMS);
    assert_eq!(s.cursor(), 0, "the rest named again next login");
    assert_eq!(s.take_events(), [Event::Failed(TOO_MANY_NEW.into())]);
    // Named again, live: still no more, and not said again.
    s.receive(&changed(vec![(key(many + 1), many as u64 + 1)]), &mut notes, &mut OsRandom);
    assert_eq!(notes.0.len(), MAX_NEW_ROOMS);
    assert!(s.take_events().is_empty());
    // One already here, named again: taken, as nothing new is made.
    s.receive(&changed(vec![(key(1), many as u64 + 2)]), &mut notes, &mut OsRandom);
    assert_eq!(s.cursor(), many as u64 + 2);
}

/// What the server calls the account's plan is its own words: shown on
/// one line, nothing in it that hides letters or turns them around.
#[test]
fn a_plan_is_the_servers_words_made_fit_to_show() {
    let mut notes = Notes(BTreeMap::new());
    let mut s = logged_in(&mut notes);
    let plan = "Free\u{202e}\u{2066}\u{200b}\nplan\u{e0041}".to_string();
    let usage = Carnet::UsageIs { notes: 1, pictures: 2, limit: 0, writable: true, plan, until: 0 };
    s.receive(&wire::encode(&Frame::Carnet(usage)), &mut notes, &mut OsRandom);
    let plan = s.take_events().into_iter().find_map(|e| match e {
        Event::Usage(u) => Some(u.plan),
        _ => None,
    });
    assert_eq!(plan.as_deref(), Some("Free plan"));
}
