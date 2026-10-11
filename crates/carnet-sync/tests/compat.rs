//! An app and a server of different builds (docs/SYNC.md §14): the app
//! reads the server's greeting before it says anything, goes on with one
//! deprecating it, stops for one too old either way, and skips frames of
//! a later protocol. A joining device does the same.

use std::collections::BTreeMap;

use carnet_sync::compat::{self, Fit, Greeting};
use carnet_sync::crypto::{DeviceKey, OsRandom, Ring, VaultKey};
use carnet_sync::pair::{Enroller, Joining};
use carnet_sync::session::{Docs, Event, Identity, Session};
use carnet_sync::wire::{self, Carnet, Deny, Frame};
use carnet_sync::NoteDoc;

struct Notes(BTreeMap<String, NoteDoc>);

impl Docs for Notes {
    fn rooms(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
    fn doc(&mut self, room: &str) -> &NoteDoc {
        self.0.entry(room.to_string()).or_insert_with(|| NoteDoc::new(1))
    }
}

fn session() -> Session {
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
    s
}

fn challenge(server: Greeting) -> Vec<u8> {
    wire::encode(&Frame::Carnet(Carnet::Challenge { nonce: [3; 32], server }))
}

fn said_hello(out: &[Vec<u8>]) -> bool {
    out.iter().any(|b| matches!(wire::decode(b), Ok(Frame::Carnet(Carnet::Hello { .. }))))
}

fn greeting(protocol: u32, oldest: u32, deprecated: u32) -> Greeting {
    Greeting { protocol, oldest, deprecated, features: vec!["passkeys".into()] }
}

#[test]
fn the_app_reads_the_servers_greeting_before_it_says_anything() {
    let mut notes = Notes(BTreeMap::new());
    let p = compat::PROTOCOL;

    // This build's own: hello, and what it can do is known.
    let mut s = session();
    s.receive(&challenge(greeting(p, compat::OLDEST, compat::DEPRECATED)), &mut notes, &mut OsRandom);
    assert!(said_hello(&s.take_out()));
    assert!(s.take_events().is_empty());
    assert!(s.server_has("passkeys") && !s.server_has("passwords"));

    // A later server that deprecates this build: hello still, and told.
    let mut s = session();
    s.receive(&challenge(greeting(p + 2, p, p + 1)), &mut notes, &mut OsRandom);
    assert!(said_hello(&s.take_out()));
    assert_eq!(s.take_events(), [Event::Outdated(Fit::UpdateSoon)]);

    // A later server that no longer takes it: nothing said.
    let mut s = session();
    s.receive(&challenge(greeting(p + 2, p + 1, p + 1)), &mut notes, &mut OsRandom);
    assert!(!said_hello(&s.take_out()));
    assert_eq!(s.take_events(), [Event::Outdated(Fit::AppTooOld)]);
    // …and its goodbye says the same.
    let bye = wire::encode(&Frame::Carnet(Carnet::Denied { reason: compat::APP_TOO_OLD.into(), why: Deny::Outdated }));
    s.receive(&bye, &mut notes, &mut OsRandom);
    assert_eq!(s.take_events(), [Event::Outdated(Fit::AppTooOld)]);

    // A server from before greetings: its challenge the nonce alone.
    let mut s = session();
    let mut bare = b"%CRN\x01".to_vec();
    bare.extend([3; 32]);
    s.receive(&bare, &mut notes, &mut OsRandom);
    assert!(!said_hello(&s.take_out()));
    assert_eq!(s.take_events(), [Event::Outdated(Fit::ServerTooOld)]);
    assert!(!s.server_has("passkeys"));
}

#[test]
fn a_later_protocols_frames_are_skipped_and_removal_is_read_by_its_reason() {
    let mut notes = Notes(BTreeMap::new());
    let mut s = session();
    s.receive(b"%CRN\xc8\x01\x02\x03", &mut notes, &mut OsRandom);
    assert!(s.take_events().is_empty() && s.take_out().is_empty());
    s.receive(&challenge(Greeting::ours(Vec::new())), &mut notes, &mut OsRandom);
    assert!(said_hello(&s.take_out()), "still talking");

    // Removed, whatever the words: read by the reason.
    for (why, account) in [(Deny::DeviceRemoved, false), (Deny::AccountRemoved, true)] {
        let mut s = session();
        s.receive(&wire::encode(&Frame::Carnet(Carnet::Denied { reason: "any words".into(), why })), &mut notes, &mut OsRandom);
        assert_eq!(s.take_events(), [Event::Removed { account }]);
    }
}

#[test]
fn a_joining_device_stops_at_a_server_too_old_for_it() {
    let mut e = Enroller::new("K7QF-2M9D-XW4H-P3TA", b"new".to_vec(), DeviceKey::generate(&mut OsRandom), &mut OsRandom);
    e.receive(&challenge(Greeting::default()));
    assert!(e.take_out().is_empty());
    assert!(matches!(&e.state, Joining::Failed(why) if why == compat::SERVER_TOO_OLD));

    let mut e = Enroller::new("K7QF-2M9D-XW4H-P3TA", b"new".to_vec(), DeviceKey::generate(&mut OsRandom), &mut OsRandom);
    e.receive(&challenge(Greeting::ours(Vec::new())));
    assert_eq!(e.take_out().len(), 1, "the invite, sent");
}
