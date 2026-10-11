//! Letting devices in and taking them out (docs/SYNC.md §7), through the
//! hub: pairing from a device already in, the recovery code when none is
//! left, the list of devices, and removing one. On memory and on SQLite
//! alike.

mod common;

use carnet_server::{Memory, Storage};
use carnet_sync::crypto::{DeviceKey, RecoveryCode, VaultKey};
use carnet_sync::elo;
use carnet_sync::invite::InviteCode;
use carnet_sync::pair::{Enroller, Joiner, Joining, PairCode, Recoverer};
use carnet_sync::session::Event;
use carnet_sync::wire::{self, Carnet, Frame};
use carnet_sync::NoteDoc;
use common::*;

#[test]
fn a_paired_device_is_let_in_and_has_the_notes() {
    for store in stores() {
        let mut net = Net::new(1, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        net.write(a, "# Shop\n\n- [ ] milk\n");
        let code = net.code(a);

        let key = DeviceKey::generate(&mut net.rng);
        let n = net.connect(Peer::Joiner(Joiner::new(code, b"new".to_vec(), key.clone())));
        net.pump();
        assert!(net.device(a).events.contains(&Event::Paired(b"new".to_vec())));
        assert!(net.hub.store().device(ACCOUNT, b"new").is_some(), "the device in let it in");
        net.admit(n, b"new", key, 2);
        assert!(net.device(n).session.is_ready(), "it logs in as any device");
        // The device in hears of it by name, without asking: its list
        // comes again once the new device has said who it is.
        let named = net.device(a).events.iter().any(|e| matches!(e, Event::Devices(l) if l.iter().any(|d| d.name.as_deref() == Some("new"))));
        assert!(named, "{:?}", net.device(a).events);
        assert_eq!(net.device(n).notes.docs.get(ROOM).map(NoteDoc::text).as_deref(), Some("# Shop\n\n- [ ] milk\n"));
        // And from now on they sync.
        net.write(n, "# Shop\n\n- [ ] milk\n- [ ] eggs\n");
        assert!(net.device(a).notes.docs[ROOM].text().contains("eggs"));
        assert!(net.denied.is_empty());
    }
}

#[test]
fn a_wrong_code_spends_the_pairing() {
    for store in stores() {
        let mut net = Net::new(2, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let code = net.code(a);
        let wrong = PairCode { secret: (code.secret + 1) % 1_000_000, ..code };
        let key = DeviceKey::generate(&mut net.rng);
        let n = net.connect(Peer::Joiner(Joiner::new(wrong, b"guess".to_vec(), key.clone())));
        net.pump();
        assert!(net.device(a).events.iter().any(|e| matches!(e, Event::PairFailed(_))), "{:?}", net.device(a).events);
        assert!(matches!(&net.peers[&n], Peer::Joiner(j) if matches!(j.state, Joining::Failed(_))));
        assert!(net.hub.store().device(ACCOUNT, b"guess").is_none(), "nothing let in");
        // The one try is spent: the right code finds nothing now.
        let m = net.connect(Peer::Joiner(Joiner::new(code, b"late".to_vec(), key)));
        net.pump();
        assert!(matches!(&net.peers[&m], Peer::Joiner(j) if matches!(&j.state, Joining::Failed(r) if r.contains("no pairing"))));
    }
}

/// Two devices join the same pairing at once (one the owner's, one a
/// guesser racing it): the first is the one, the second finds nothing.
#[test]
fn a_second_joiner_is_turned_away_while_one_is_in() {
    for store in stores() {
        let mut net = Net::new(7, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let code = net.code(a);
        let (k1, k2) = (DeviceKey::generate(&mut net.rng), DeviceKey::generate(&mut net.rng));
        let first = net.connect(Peer::Joiner(Joiner::new(code, b"first-in".to_vec(), k1.clone())));
        let second = net.connect(Peer::Joiner(Joiner::new(code, b"racer".to_vec(), k2)));
        net.pump();
        assert!(matches!(&net.peers[&second], Peer::Joiner(j) if matches!(&j.state, Joining::Failed(r) if r.contains("no pairing"))));
        assert!(net.hub.store().device(ACCOUNT, b"racer").is_none());
        net.admit(first, b"first-in", k1, 3);
        assert!(net.device(first).session.is_ready());
    }
}

#[test]
fn a_server_that_meddles_lets_nothing_in() {
    for store in stores() {
        let mut net = Net::new(3, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let code = net.code(a);
        let key = DeviceKey::generate(&mut net.rng);
        let n = net.connect(Peer::Joiner(Joiner::new(code, b"new".to_vec(), key)));
        // The server alters the new device's sealed key on its way: the
        // last byte of every step it sends up.
        net.meddle = Some((n, Box::new(|b: &mut Vec<u8>| {
            if matches!(wire::decode(b), Ok(Frame::Carnet(Carnet::PairStep { .. }))) {
                *b.last_mut().unwrap() ^= 1;
            }
        })));
        net.pump();
        assert!(net.hub.store().device(ACCOUNT, b"new").is_none(), "a key the server touched is not let in");
        assert!(net.device(a).events.iter().any(|e| matches!(e, Event::PairFailed(_))));
        assert!(matches!(&net.peers[&n], Peer::Joiner(j) if !matches!(j.state, Joining::In(_))));
    }
}

#[test]
fn the_device_in_going_away_ends_the_pairing() {
    let mut net = Net::new(4, Box::new(Memory::default()));
    let vault = VaultKey::generate(&mut net.rng);
    let a = net.first_device(&vault);
    let code = net.code(a);
    net.disconnect(a);
    let key = DeviceKey::generate(&mut net.rng);
    let n = net.connect(Peer::Joiner(Joiner::new(code, b"new".to_vec(), key)));
    net.pump();
    assert!(matches!(&net.peers[&n], Peer::Joiner(j) if matches!(j.state, Joining::Failed(_))));
}

#[test]
fn the_recovery_words_let_a_device_in_when_none_is_left() {
    for store in stores() {
        let mut net = Net::new(5, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        net.write(a, "# Diary\n\nall of it\n");
        let code = RecoveryCode::generate(&mut net.rng);
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            d.session.set_recovery(&code, rng);
        }
        net.pump();
        assert!(net.device(a).events.contains(&Event::RecoverySet));
        let words = code.words();
        // Every device gone.
        net.disconnect(a);

        let key = DeviceKey::generate(&mut net.rng);
        let r = net.connect(Peer::Recoverer(Recoverer::new(ORIGIN, RecoveryCode::parse(&words).unwrap(), b"phoenix".to_vec(), key.clone())));
        net.pump();
        net.admit(r, b"phoenix", key, 9);
        assert!(net.device(r).session.is_ready());
        assert_eq!(net.device(r).notes.docs.get(ROOM).map(NoteDoc::text).as_deref(), Some("# Diary\n\nall of it\n"));
    }
}

#[test]
fn wrong_or_replaced_words_let_nothing_in() {
    for store in stores() {
        let mut net = Net::new(6, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let old = RecoveryCode::generate(&mut net.rng);
        let new = RecoveryCode::generate(&mut net.rng);
        for code in [&old, &new] {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            d.session.set_recovery(code, rng);
            net.pump();
        }
        for (name, code) in [("old", old), ("never set", RecoveryCode::generate(&mut net.rng))] {
            let key = DeviceKey::generate(&mut net.rng);
            let r = net.connect(Peer::Recoverer(Recoverer::new(ORIGIN, code, name.as_bytes().to_vec(), key)));
            net.pump();
            assert!(net.denied.iter().any(|(c, _)| *c == r), "{name}: turned away");
            assert!(net.hub.store().device(ACCOUNT, name.as_bytes()).is_none(), "{name}: nothing let in");
        }
        // The new one works.
        let key = DeviceKey::generate(&mut net.rng);
        let r = net.connect(Peer::Recoverer(Recoverer::new(ORIGIN, new, b"ok".to_vec(), key.clone())));
        net.pump();
        net.admit(r, b"ok", key, 7);
        assert!(net.device(r).session.is_ready());
    }
}

/// A first device makes its account over the socket, by an invite, and
/// logs in; the invite is spent.
#[test]
fn an_invite_makes_an_account_over_the_socket() {
    for store in stores() {
        let mut net = Net::new(8, store);
        let code = InviteCode::generate(&mut net.rng);
        net.hub.store_mut().add_invite(code.hash(), true, u64::MAX).unwrap();
        let key = DeviceKey::generate(&mut net.rng);
        let e = Enroller::new(code.text().to_lowercase(), b"laptop".to_vec(), key.clone(), &mut net.rng);
        let c = net.connect(Peer::Enroller(e));
        net.pump();
        net.admit(c, b"laptop", key, 11);
        assert!(net.device(c).session.is_ready());
        net.write(c, "# First\n");
        // Spent: a second device with it is turned away.
        let k2 = DeviceKey::generate(&mut net.rng);
        let e2 = Enroller::new(code.text(), b"other".to_vec(), k2, &mut net.rng);
        let again = net.connect(Peer::Enroller(e2));
        net.pump();
        assert!(net.denied.iter().any(|(d, r)| *d == again && r.contains("not good")), "{:?}", net.denied);
        // Past its time, too.
        let late = InviteCode::generate(&mut net.rng);
        net.hub.store_mut().add_invite(late.hash(), false, 10).unwrap();
        net.hub.tick(11);
        let k3 = DeviceKey::generate(&mut net.rng);
        let e3 = Enroller::new(late.text(), b"late".to_vec(), k3, &mut net.rng);
        let l = net.connect(Peer::Enroller(e3));
        net.pump();
        assert!(net.denied.iter().any(|(d, _)| *d == l));
    }
}

/// Removing a device: from then on it reads nothing new. Every device
/// kept gets the next epoch's key, sealed to it alone, at once if online
/// and on its next login if not; so do the recovery words. The one taken
/// out is shut out at once, and stays out.
#[test]
fn a_removed_device_reads_nothing_written_after() {
    for store in stores() {
        let mut net = Net::new(20, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let b = net.pair(a, b"laptop", 2);
        let c = net.pair(a, b"tablet", 3);
        let words = RecoveryCode::generate(&mut net.rng);
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            d.session.set_recovery(&words, rng);
        }
        net.pump();
        net.write(a, "# Plans\n\nbefore\n");

        // The list: every device by the name it gave, this one marked.
        let list = net.list(a);
        let mut names: Vec<(Option<String>, bool)> = list.iter().map(|d| (d.name.clone(), d.you)).collect();
        names.sort();
        assert_eq!(names, [(Some("first".into()), true), (Some("laptop".into()), false), (Some("tablet".into()), false)]);

        // The tablet is away while the laptop goes.
        let tablet = net.take(c);
        net.revoke(a, b"laptop").unwrap();
        assert!(net.denied.iter().any(|(conn, why)| *conn == b && why.contains("removed")), "{:?}", net.denied);
        assert!(net.hub.store().device(ACCOUNT, b"laptop").unwrap().revoked);
        assert_eq!(net.hub.store().epoch(ACCOUNT), 1);
        assert!(net.device(a).events.contains(&Event::Rekeyed(1)), "the remover has the new key once the server has it");
        assert_eq!(net.device(a).session.ring().epochs(), [0, 1]);
        let names: Vec<_> = net.list(a).iter().filter_map(|d| d.name.clone()).collect();
        assert_eq!(names.len(), 2, "the laptop gone from the list: {names:?}");

        // Written after: sealed under epoch 1, which the laptop never had.
        net.write(a, "# Plans\n\nbefore\nafter\n");
        let laptop_ring = net.device(b).session.ring().clone();
        assert_eq!(laptop_ring.epochs(), [0]);
        let spans = net.hub.store().spans(ACCOUNT, ROOM);
        let new: Vec<_> = spans.iter().filter(|s| matches!(elo::header(&s.record), Ok(elo::Header::Delta { epoch: 1, .. }))).collect();
        assert!(!new.is_empty(), "something was sealed under the new epoch");
        for s in &new {
            assert!(elo::open(&laptop_ring, ROOM, &s.record).is_err(), "the laptop can't open what came after");
            assert!(elo::open(net.device(a).session.ring(), ROOM, &s.record).is_ok());
        }

        // The laptop can't log in again.
        let laptop = net.take(b);
        let b2 = net.back(laptop);
        assert!(net.denied.iter().any(|(conn, why)| *conn == b2 && why == "this device was removed from the account"));

        // The tablet, back: its key comes on login, and it reads it all.
        let c2 = net.back(tablet);
        assert!(net.device(c2).session.is_ready());
        assert_eq!(net.device(c2).session.ring().epochs(), [0, 1]);
        assert!(net.device(c2).events.contains(&Event::Rekeyed(1)), "the app is told, to keep the ring");
        assert_eq!(net.device(c2).notes.docs[ROOM].text(), "# Plans\n\nbefore\nafter\n");

        // And the words, made before, still let a device in that reads it.
        let key = DeviceKey::generate(&mut net.rng);
        let r = net.connect(Peer::Recoverer(Recoverer::new(ORIGIN, RecoveryCode::parse(&words.words()).unwrap(), b"phoenix".to_vec(), key.clone())));
        net.pump();
        net.admit(r, b"phoenix", key, 9);
        assert_eq!(net.device(r).session.ring().epochs(), [0, 1]);
        assert_eq!(net.device(r).notes.docs[ROOM].text(), "# Plans\n\nbefore\nafter\n");
    }
}

/// The server keeps a new epoch only with its key for every device kept
/// and the recovery code, no more, no fewer: a device paired in meanwhile,
/// or a stale epoch, and nothing changes.
#[test]
fn a_removal_that_would_leave_a_device_without_the_key_is_refused() {
    for store in stores() {
        let mut net = Net::new(21, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        net.pair(a, b"laptop", 2);
        net.pair(a, b"tablet", 3);
        let refused = |net: &mut Net| {
            net.device(a).events.drain(..).filter_map(|e| match e {
                Event::Refused(why) => Some(why),
                _ => None,
            }).next_back()
        };
        let send = |net: &mut Net, keys: Vec<(Vec<u8>, Vec<u8>)>, epoch: u32| {
            let f = wire::encode(&Frame::Carnet(Carnet::Revoke { device: b"laptop".to_vec(), epoch, keys, cards: Vec::new() }));
            let outs = net.hub.receive(a, &f, &mut net.rng);
            net.route(outs);
            net.pump();
        };
        // As if from a list read before the tablet came in: no key for it.
        send(&mut net, vec![(b"first".to_vec(), vec![1; 80])], 1);
        assert!(refused(&mut net).is_some_and(|w| w.contains("meanwhile")));
        // A key for a device not in the account, too many.
        send(&mut net, vec![(b"first".to_vec(), vec![1; 80]), (b"tablet".to_vec(), vec![1; 80]), (b"ghost".to_vec(), vec![1; 80])], 1);
        assert!(refused(&mut net).is_some_and(|w| w.contains("meanwhile")));
        // An epoch not the next.
        send(&mut net, vec![(b"first".to_vec(), vec![1; 80]), (b"tablet".to_vec(), vec![1; 80])], 5);
        assert!(refused(&mut net).is_some_and(|w| w.contains("changed meanwhile")));
        assert!(!net.hub.store().device(ACCOUNT, b"laptop").unwrap().revoked, "nothing changed");
        assert_eq!(net.hub.store().epoch(ACCOUNT), 0);
        assert_eq!(net.device(a).session.ring().epochs(), [0]);

        // From the list the device has, it goes.
        let _ = net.list(a);
        net.revoke(a, b"laptop").unwrap();
        assert!(net.hub.store().device(ACCOUNT, b"laptop").unwrap().revoked);
        assert_eq!(net.hub.store().epoch(ACCOUNT), 1);
    }
}

/// A device that leaves by itself is not taken to be lost: no new key.
#[test]
fn a_device_leaving_changes_no_keys() {
    for store in stores() {
        let mut net = Net::new(22, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let b = net.pair(a, b"laptop", 2);
        net.revoke(b, b"laptop").unwrap();
        assert!(net.denied.iter().any(|(conn, why)| *conn == b && why.contains("left")));
        assert!(net.hub.store().device(ACCOUNT, b"laptop").unwrap().revoked);
        assert_eq!(net.hub.store().epoch(ACCOUNT), 0);
        assert_eq!(net.list(a).len(), 1);
    }
}

/// A card is the device it names, or it is not read: a server that puts
/// one device's card on another's row gets no name shown for it.
#[test]
fn a_card_on_another_devices_row_is_not_believed() {
    let mut net = Net::new(23, Box::new(Memory::default()));
    let vault = VaultKey::generate(&mut net.rng);
    let a = net.first_device(&vault);
    net.pair(a, b"laptop", 2);
    net.pair(a, b"tablet", 3);
    let devices = net.hub.store().devices(ACCOUNT);
    let card_of = |id: &[u8]| devices.iter().find(|(d, _)| d == id).unwrap().1.card.clone();
    let rows = devices
        .iter()
        .map(|(id, d)| wire::DeviceRow {
            id: id.clone(),
            public: d.public,
            // The laptop's card on the tablet's row.
            card: if id == b"tablet" { card_of(b"laptop") } else { d.card.clone() },
            seen: d.seen,
            you: id == b"first",
        })
        .collect();
    let forged = wire::encode(&Frame::Carnet(Carnet::Devices { epoch: 0, devices: rows, recovery: Vec::new(), password: Vec::new(), passwords: false, passkeys: Vec::new(), password_email: String::new() }));
    let Net { peers, rng, .. } = &mut net;
    let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
    d.session.receive(&forged, &mut d.notes, rng);
    let list = d.session.take_events().into_iter().find_map(|e| match e {
        Event::Devices(l) => Some(l),
        _ => None,
    });
    let tablet = list.unwrap().into_iter().find(|d| d.id == b"tablet").unwrap();
    assert_eq!(tablet.name, None);
}

/// A device renamed: its new name in its own list at once, and in the
/// others' (the server sends them the list again), and still after it
/// comes back.
#[test]
fn a_device_renamed_is_listed_by_its_new_name() {
    for store in stores() {
        let mut net = Net::new(1, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let b = net.pair(a, b"laptop", 2);
        let name_of = |list: &[carnet_sync::session::DeviceInfo], id: &[u8]| list.iter().find(|d| d.id == id).and_then(|d| d.name.clone());
        assert_eq!(name_of(&net.list(a), b"laptop").as_deref(), Some("laptop"));
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&b) else { panic!() };
            d.session.rename("Work laptop", rng);
        }
        net.pump();
        let mine = net.device(b).events.iter().rev().find_map(|e| match e {
            Event::Devices(l) => Some(l.clone()),
            _ => None,
        });
        assert_eq!(name_of(&mine.expect("listed again"), b"laptop").as_deref(), Some("Work laptop"));
        assert_eq!(name_of(&net.list(a), b"laptop").as_deref(), Some("Work laptop"));
        let d = net.take(b);
        let _b = net.back(d);
        assert_eq!(name_of(&net.list(a), b"laptop").as_deref(), Some("Work laptop"), "kept through a new login");
    }
}
