//! Passkeys (docs/SYNC.md §7): a device in adds one, by what its PRF
//! output comes to; a new device signs in with the same output and has the
//! notes, also after a device was removed (the new key sealed to the
//! passkey's slot too). Another output, a passkey taken away: nothing let
//! in. At most twenty to an account, and no key two slots'. On memory and
//! SQLite.

mod common;

use carnet_server::storage::PasskeySlot;
use carnet_sync::crypto::{DeviceKey, VaultKey};
use carnet_sync::pair::{Joining, NO_SLOT, Recoverer};
use carnet_sync::passkey;
use carnet_sync::session::{Event, PasskeyInfo};
use carnet_sync::wire::{self, Carnet, Frame};
use carnet_sync::NoteDoc;
use common::*;

const PRF: [u8; 32] = [7; 32];

fn add(net: &mut Net, conn: carnet_server::ConnId, name: &str, credential: &[u8], prf: &[u8]) -> Vec<Event> {
    {
        let Net { peers, rng, .. } = &mut *net;
        let Some(Peer::Device(d)) = peers.get_mut(&conn) else { panic!() };
        d.session.add_passkey(name, credential, prf, rng).unwrap();
    }
    net.pump();
    std::mem::take(&mut net.device(conn).events)
}

fn remove(net: &mut Net, conn: carnet_server::ConnId, id: [u8; 32]) -> Vec<Event> {
    net.device(conn).session.remove_passkey(&id);
    net.pump();
    std::mem::take(&mut net.device(conn).events)
}

/// The last list of passkeys among `events`.
fn passkeys(events: &[Event]) -> Vec<PasskeyInfo> {
    events.iter().rev().find_map(|e| if let Event::Passkeys(p) = e { Some(p.clone()) } else { None }).expect("a list of passkeys")
}

fn sign_in(net: &mut Net, prf: &[u8], device: &[u8]) -> (carnet_server::ConnId, DeviceKey) {
    let key = DeviceKey::generate(&mut net.rng);
    let c = net.connect(Peer::Recoverer(Recoverer::passkey(ORIGIN, prf, device.to_vec(), key.clone())));
    net.pump();
    (c, key)
}

fn failed(net: &Net, conn: carnet_server::ConnId) -> Option<String> {
    match net.peers.get(&conn) {
        Some(Peer::Recoverer(r)) => match &r.state {
            Joining::Failed(why) => Some(why.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// A frame sent as the device `conn` would, past its session's own
/// checks: what a device of another build, or a meddler, could send.
fn raw(net: &mut Net, conn: carnet_server::ConnId, c: Carnet) {
    let outs = net.hub.receive(conn, &wire::encode(&Frame::Carnet(c)), &mut net.rng);
    net.route(outs);
    net.pump();
}

#[test]
fn a_passkey_lets_a_new_device_in_with_the_notes() {
    for store in stores() {
        let mut net = Net::new(120, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        net.write(a, "# Diary\n\nall of it\n");
        let b = net.pair(a, b"laptop", 2);
        let _ = std::mem::take(&mut net.device(a).events);
        let _ = std::mem::take(&mut net.device(b).events);

        // Added, and said: listed by name and credential, here and on the
        // other device.
        let said = add(&mut net, a, "Passkey  from Chrome on Mac", b"cred-1", &PRF);
        assert!(said.contains(&Event::PasskeySet), "{said:?}");
        let listed = passkeys(&said);
        let id = passkey::secret(&PRF).signing_key().public();
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].id, listed[0].name.as_deref(), listed[0].credential.as_slice()), (id, Some("Passkey from Chrome on Mac"), &b"cred-1"[..]));
        let theirs = passkeys(&std::mem::take(&mut net.device(b).events));
        assert_eq!(theirs, listed, "the other device's list says it too");
        // Nothing the server keeps is the PRF output, nor the name.
        let slot = &net.hub.store().passkeys(ACCOUNT)[0];
        assert!(!slot.card.windows(7).any(|w| w == b"Passkey") && !slot.wrapped.windows(32).any(|w| w == PRF));

        // A device removed: a new key, sealed to the passkey's slot too.
        net.list(a);
        net.revoke(a, b"laptop").unwrap();
        assert_eq!(net.hub.store().rekeys(ACCOUNT, &passkey::slot(&id)).len(), 1);
        net.write(a, "# Diary\n\nall of it\nand after\n");

        // Signed in with it: in, with every epoch's key, and the notes.
        let (n, key) = sign_in(&mut net, &PRF, b"phone");
        net.admit(n, b"phone", key, 9);
        assert!(net.device(n).session.is_ready());
        assert_eq!(net.device(n).notes.docs.get(ROOM).map(NoteDoc::text).as_deref(), Some("# Diary\n\nall of it\nand after\n"));

        // Another passkey's output: turned away, nothing let in.
        let (c, _) = sign_in(&mut net, &[8; 32], b"stranger");
        assert_eq!(failed(&net, c).as_deref(), Some(NO_SLOT));
        assert!(net.hub.store().device(ACCOUNT, b"stranger").is_none());

        // Taken away: it lets nothing in, and what was sealed to it goes.
        let said = remove(&mut net, a, id);
        assert!(said.contains(&Event::PasskeySet), "{said:?}");
        assert!(passkeys(&said).is_empty());
        assert!(net.hub.store().passkeys(ACCOUNT).is_empty());
        assert!(net.hub.store().rekeys(ACCOUNT, &passkey::slot(&id)).is_empty());
        let (c, _) = sign_in(&mut net, &PRF, b"late");
        assert_eq!(failed(&net, c).as_deref(), Some(NO_SLOT));
        assert!(net.hub.store().device(ACCOUNT, b"late").is_none());
        // Taken away again: there's none.
        let said = remove(&mut net, a, id);
        assert!(said.iter().any(|e| matches!(e, Event::Refused(why) if why.contains("isn't in the account"))), "{said:?}");
    }
}

/// Several passkeys, each its own slot: a device removed seals the new key
/// to each, and each still lets a device in; added again, one is replaced,
/// not doubled.
#[test]
fn each_passkey_is_a_slot_of_its_own() {
    for store in stores() {
        let mut net = Net::new(121, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        net.write(a, "# One\n");
        let _b = net.pair(a, b"laptop", 2);
        for (i, name) in ["Passkey from Safari on iPhone", "Passkey from Chrome on Mac", "Passkey from Firefox on Linux"].iter().enumerate() {
            add(&mut net, a, name, &[i as u8 + 1; 16], &[i as u8 + 10; 32]);
        }
        let said = add(&mut net, a, "Passkey from Chrome on Mac", &[2; 16], &[11; 32]);
        assert_eq!(passkeys(&said).len(), 3, "the same one again replaces it");
        assert_eq!(passkeys(&said).iter().map(|p| p.name.clone().unwrap()).collect::<Vec<_>>()[0], "Passkey from Safari on iPhone", "the oldest first");

        net.list(a);
        net.revoke(a, b"laptop").unwrap();
        net.write(a, "# One\n\nafter\n");
        for (i, prf) in [[10; 32], [11; 32], [12; 32]].iter().enumerate() {
            let device = format!("new-{i}");
            let (n, key) = sign_in(&mut net, prf, device.as_bytes());
            net.admit(n, device.as_bytes(), key, 20 + i as u64);
            assert_eq!(net.device(n).notes.docs.get(ROOM).map(NoteDoc::text).as_deref(), Some("# One\n\nafter\n"), "{device}");
        }
    }
}

/// Twenty to an account: the device says so before asking, and the server
/// refuses one more from a device that asks anyway. A key another
/// account's passkey has is refused.
#[test]
fn passkeys_are_limited_and_never_share_a_key() {
    for store in stores() {
        let mut net = Net::new(122, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        for i in 0..passkey::MAX {
            add(&mut net, a, &format!("Passkey {i}"), &[i as u8 + 1; 16], &[i as u8 + 1; 32]);
        }
        assert_eq!(net.hub.store().passkeys(ACCOUNT).len(), passkey::MAX);
        let mut rng = Seeded(1);
        let past = net.device(a).session.add_passkey("One more", &[99; 16], &[99; 32], &mut rng);
        assert!(past.unwrap_err().contains("20 passkeys at most"));

        let public = passkey::secret(&[99; 32]).signing_key().public();
        raw(&mut net, a, Carnet::PasskeyAdd { public, wrapped: vec![1; 60], card: vec![2; 120] });
        let said = std::mem::take(&mut net.device(a).events);
        assert!(said.iter().any(|e| matches!(e, Event::Refused(why) if why.contains("20 passkeys at most"))), "{said:?}");
        assert_eq!(net.hub.store().passkeys(ACCOUNT).len(), passkey::MAX);

        // Another account's key: refused, and theirs stays theirs.
        net.hub.store_mut().add_account(b"other", false).unwrap();
        let theirs = PasskeySlot { public: [5; 32], wrapped: vec![3; 60], card: vec![4; 120], made: 1 };
        net.hub.store_mut().add_passkey(b"other", theirs.clone()).unwrap();
        let first = net.hub.store().passkeys(ACCOUNT)[0].public;
        remove(&mut net, a, first);
        raw(&mut net, a, Carnet::PasskeyAdd { public: [5; 32], wrapped: vec![1; 60], card: vec![2; 120] });
        let said = std::mem::take(&mut net.device(a).events);
        assert!(said.iter().any(|e| matches!(e, Event::Refused(why) if why.contains("couldn't be added"))), "{said:?}");
        assert_eq!(net.hub.store().passkey_slot(&[5; 32]).map(|(acct, _)| acct), Some(b"other".to_vec()));
        assert_eq!(net.hub.store().passkeys(b"other"), [theirs]);
        // An account removed: its passkeys with it.
        net.hub.store_mut().remove_account(b"other").unwrap();
        assert!(net.hub.store().passkey_slot(&[5; 32]).is_none());
    }
}
