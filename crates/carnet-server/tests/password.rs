//! Email and password (docs/SYNC.md §7): a device in sets one; a new
//! device signs in with it and has the notes, also after a device was
//! removed (the new key sealed to the password's slot too). A wrong
//! password, an unknown email, an email another account has, a server
//! that takes none: nothing let in. The password never reaches the server.
//! On memory and SQLite.

mod common;

use carnet_sync::crypto::{DeviceKey, VaultKey};
use carnet_sync::pair::PasswordJoiner;
use carnet_sync::password;
use carnet_sync::session::Event;
use carnet_sync::NoteDoc;
use common::*;

fn set(net: &mut Net, conn: carnet_server::ConnId, email: &str, pw: &str) -> Vec<Event> {
    {
        let Net { peers, rng, .. } = &mut *net;
        let Some(Peer::Device(d)) = peers.get_mut(&conn) else { panic!() };
        d.session.set_password(email, pw, rng).unwrap();
    }
    net.pump();
    std::mem::take(&mut net.device(conn).events)
}

fn sign_in(net: &mut Net, email: &str, pw: &str, device: &[u8]) -> (carnet_server::ConnId, DeviceKey) {
    let key = DeviceKey::generate(&mut net.rng);
    let c = net.connect(Peer::Password(PasswordJoiner::new(ORIGIN, email, pw, device.to_vec(), key.clone())));
    net.pump();
    (c, key)
}

#[test]
fn a_password_lets_a_new_device_in_with_the_notes() {
    for store in stores() {
        let mut net = Net::new(90, store);
        net.hub.set_passwords(password::Server::generate(&mut net.rng));
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        net.write(a, "# Diary\n\nall of it\n");
        let b = net.pair(a, b"laptop", 2);
        let _ = std::mem::take(&mut net.device(a).events);

        // Set, and said: the list knows the email.
        let said = set(&mut net, a, " Ann@Example.com ", "correct horse battery");
        assert!(said.contains(&Event::PasswordSet), "{said:?}");
        assert!(said.contains(&Event::Password { allowed: true, id: Some("ann@example.com".into()) }), "{said:?}");
        assert!(net.hub.store().password_of("ann@example.com").is_some());
        // Never the password itself, anywhere the server keeps.
        let (_, record) = net.hub.store().password_of("ann@example.com").unwrap();
        assert!(!record.windows(8).any(|w| w == b"correct "));

        // A device removed: a new key, sealed to the password's slot too.
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            d.session.list_devices();
            let _ = rng;
        }
        net.pump();
        net.revoke(a, b"laptop").unwrap();
        let _ = b;
        net.write(a, "# Diary\n\nall of it\nand after\n");

        // Signed in with it: in, with every epoch's key, and the notes.
        let (n, key) = sign_in(&mut net, "ANN@example.com", "correct horse battery", b"phone");
        net.admit(n, b"phone", key, 9);
        assert!(net.device(n).session.is_ready());
        assert_eq!(net.device(n).notes.docs.get(ROOM).map(NoteDoc::text).as_deref(), Some("# Diary\n\nall of it\nand after\n"));

        // Wrong password, unknown email: turned away, nothing let in.
        for (email, pw, name) in [("ann@example.com", "wrong horse battery", "wrong"), ("bob@example.com", "correct horse battery", "nobody")] {
            let (c, _) = sign_in(&mut net, email, pw, name.as_bytes());
            let Some(Peer::Password(p)) = net.peers.get(&c) else { panic!() };
            assert!(matches!(p.state(), carnet_sync::pair::Joining::Failed(why) if why == password::WRONG), "{name}");
            assert!(net.hub.store().device(ACCOUNT, name.as_bytes()).is_none(), "{name}: nothing let in");
        }

        // Changed: the old one lets nothing in, the new one does.
        assert!(set(&mut net, a, "ann@example.com", "a new long password").contains(&Event::PasswordSet));
        let (c, _) = sign_in(&mut net, "ann@example.com", "correct horse battery", b"old");
        let Some(Peer::Password(p)) = net.peers.get(&c) else { panic!() };
        assert!(matches!(p.state(), carnet_sync::pair::Joining::Failed(_)));
        let (n2, key) = sign_in(&mut net, "ann@example.com", "a new long password", b"tablet");
        net.admit(n2, b"tablet", key, 10);
        assert!(net.device(n2).session.is_ready());

        // Taken away: no sign-in at all.
        {
            let Net { peers, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            d.session.drop_password();
        }
        net.pump();
        assert!(net.hub.store().password_of("ann@example.com").is_none());
        let (c, _) = sign_in(&mut net, "ann@example.com", "a new long password", b"late");
        let Some(Peer::Password(p)) = net.peers.get(&c) else { panic!() };
        assert!(matches!(p.state(), carnet_sync::pair::Joining::Failed(_)));
    }
}

/// A server that takes no passwords says so, to a device setting one and
/// to one signing in.
#[test]
fn a_server_that_takes_no_passwords_says_so() {
    for store in stores() {
        let mut net = Net::new(91, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let said = set(&mut net, a, "ann@example.com", "correct horse battery");
        assert!(said.iter().any(|e| matches!(e, Event::Refused(why) if why == carnet_server::hub::NO_PASSWORDS)), "{said:?}");
        let (c, _) = sign_in(&mut net, "ann@example.com", "correct horse battery", b"x");
        assert!(net.denied.iter().any(|(d, why)| *d == c && why == carnet_server::hub::NO_PASSWORDS));
        let _ = vault;
    }
}
