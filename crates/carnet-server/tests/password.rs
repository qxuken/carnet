//! Email and password (docs/SYNC.md §7): a device in sets one; a new
//! device signs in with it and has the notes, also after a device was
//! removed (the new key sealed to the password's slot too). A wrong
//! password, an unknown email, an email another account has, a server
//! that takes none: nothing let in. The password never reaches the server.
//! A server holding passwords to the account's email (a hosted service)
//! takes that one alone, and tells the account's devices which.
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
        assert!(said.contains(&Event::Password { allowed: true, id: Some("ann@example.com".into()), only: None }), "{said:?}");
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

/// A host that knows each account's email: the one its signup confirmed.
struct Emails(Option<&'static str>);

impl carnet_server::quota::Admission for Emails {
    fn admitted(&self, _: [u8; 32], _: &[u8]) {}
    fn email_of(&self, account: &[u8]) -> Option<String> {
        self.0.filter(|_| account == ACCOUNT).map(str::to_string)
    }
}

fn listed(net: &mut Net, conn: carnet_server::ConnId) -> Vec<Event> {
    {
        let Some(Peer::Device(d)) = net.peers.get_mut(&conn) else { panic!() };
        d.session.list_devices();
    }
    net.pump();
    std::mem::take(&mut net.device(conn).events)
}

/// Held to the account's email: the devices are told it with their list,
/// another email is refused with the one it must be, that one is taken
/// (as typed, in any case), and a sign-in with it lets a device in. An
/// account the host knows no email of can't set one.
#[test]
fn a_hosted_service_holds_the_password_to_the_email_signed_up_with() {
    use carnet_server::quota::{self, PasswordEmails};
    for store in stores() {
        let mut net = Net::new(92, store);
        net.hub.set_passwords(password::Server::generate(&mut net.rng));
        net.hub.set_password_emails(PasswordEmails::Account);
        net.hub.set_admission(std::sync::Arc::new(Emails(Some(" Ann@Example.com"))));
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        net.write(a, "# Diary\n\nall of it\n");
        let _ = std::mem::take(&mut net.device(a).events);

        let said = listed(&mut net, a);
        assert!(said.contains(&Event::Password { allowed: true, id: None, only: Some("ann@example.com".into()) }), "{said:?}");

        // Another's email: refused, saying which; nothing kept.
        let said = set(&mut net, a, "bob@example.com", "correct horse battery");
        let why = quota::password_email_is("ann@example.com");
        assert!(said.iter().any(|e| matches!(e, Event::Refused(r) if *r == why)), "{said:?}");
        assert_eq!(why, "On this service, the password signs in with the email you signed up with: ann@example.com");
        assert!(net.hub.store().password_of("bob@example.com").is_none());
        assert!(net.hub.store().password_card(ACCOUNT).is_empty());

        // Its own: set, and a new device signs in with it.
        let said = set(&mut net, a, "ANN@example.com ", "correct horse battery");
        assert!(said.contains(&Event::PasswordSet), "{said:?}");
        assert!(said.contains(&Event::Password { allowed: true, id: Some("ann@example.com".into()), only: Some("ann@example.com".into()) }), "{said:?}");
        let (n, key) = sign_in(&mut net, "ann@example.com", "correct horse battery", b"phone");
        net.admit(n, b"phone", key, 9);
        assert!(net.device(n).session.is_ready());

        // The host knows no email for it: none may be set.
        net.hub.set_admission(std::sync::Arc::new(Emails(None)));
        let said = set(&mut net, a, "ann@example.com", "another long password");
        assert!(said.iter().any(|e| matches!(e, Event::Refused(r) if r == quota::NO_ACCOUNT_EMAIL)), "{said:?}");
    }
}

/// A server of one's own holds a password to no email in particular, and
/// says none.
#[test]
fn a_server_of_ones_own_takes_any_email() {
    for store in stores() {
        let mut net = Net::new(93, store);
        net.hub.set_passwords(password::Server::generate(&mut net.rng));
        // Even with a host that knows an email: the policy is Any.
        net.hub.set_admission(std::sync::Arc::new(Emails(Some("ann@example.com"))));
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let _ = std::mem::take(&mut net.device(a).events);
        let said = listed(&mut net, a);
        assert!(said.contains(&Event::Password { allowed: true, id: None, only: None }), "{said:?}");
        let said = set(&mut net, a, "bob@example.com", "correct horse battery");
        assert!(said.contains(&Event::PasswordSet), "{said:?}");
    }
}
