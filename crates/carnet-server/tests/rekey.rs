//! New epochs' keys, through the hub (docs/SYNC.md §7, "Devices, removal
//! and key epochs"): each goes signed by the device that made it, and a
//! device takes one only from a device in the account at its newest
//! epoch, for the epoch right after, never in place of a key it has. A
//! device removed, with the server's help, hands nobody a key of its own,
//! nor is sealed the next by a card it sealed before it went.
//! On memory and on SQLite alike.

mod common;

use carnet_server::{ConnId, Storage};
use carnet_sync::crypto::{Card, DeviceKey, RecoveryCode, VaultKey};
use carnet_sync::elo;
use carnet_sync::pair::{Joiner, Joining, Recoverer};
use carnet_sync::rekey::{self, Signer};
use carnet_sync::session::{DeviceInfo, Docs, Event};
use carnet_sync::{passkey, password};
use carnet_sync::wire::{self, Carnet, Frame};
use carnet_sync::Origin;
use common::*;

/// A device paired in, its signing key kept: what it takes with it when
/// it's removed.
fn pair_keeping(net: &mut Net, from: ConnId, device: &[u8], peer: u64) -> (ConnId, DeviceKey) {
    let code = net.code(from);
    let key = DeviceKey::generate(&mut net.rng);
    let n = net.connect(Peer::Joiner(Joiner::new(code, device.to_vec(), key.clone())));
    net.pump();
    net.admit(n, device, key.clone(), peer);
    assert!(net.device(n).session.is_ready());
    (n, key)
}

fn rekeyed(net: &mut Net, conn: ConnId) -> Vec<u32> {
    std::mem::take(&mut net.device(conn).events).into_iter().filter_map(|e| if let Event::Rekeyed(n) = e { Some(n) } else { None }).collect()
}

/// `conn` writes `text` into `room`.
fn write_in(net: &mut Net, conn: ConnId, room: &str, text: &str) {
    let Net { peers, rng, .. } = net;
    let Some(Peer::Device(d)) = peers.get_mut(&conn) else { panic!() };
    d.notes.doc(room).record(text, Origin::Save);
    d.session.saved(room, &mut d.notes, rng);
    net.pump();
}

/// The laptop is removed, and keeps what it had: its signing key, epoch
/// 0's key, the tablet's box key (on the tablet's card it read) and the
/// words'. It seals a key of its own to the tablet, signed, and the server
/// hands it over; then to the words, for the next device they let in.
/// Neither takes it, and both stay on the account's epoch 1.
#[test]
fn a_removed_device_cannot_hand_out_a_key_of_its_own() {
    for store in stores() {
        let mut net = Net::new(80, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let (b, laptop_key) = pair_keeping(&mut net, a, b"laptop", 2);
        let (c, tablet_key) = pair_keeping(&mut net, a, b"tablet", 3);
        let words = RecoveryCode::generate(&mut net.rng);
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            d.session.set_recovery(&words, rng);
        }
        net.pump();
        net.write(a, "before\n");
        let laptop_ring = net.device(b).session.ring().clone();

        net.revoke(a, b"laptop").unwrap();
        let k1 = net.device(a).session.ring().get(1).cloned().unwrap();
        assert_eq!(net.device(c).session.ring().current(), (1, &k1));
        assert_eq!(rekeyed(&mut net, c), [1]);

        // The laptop's key, for the epoch after the account's, and for the
        // account's own again: handed to the tablet, signed, by the server.
        let evil = VaultKey::generate(&mut net.rng);
        let signer = Signer::new(ACCOUNT, b"laptop", &laptop_key, &laptop_ring, &mut net.rng);
        for epoch in [2, 1, 0] {
            let sealed = signer.seal(&tablet_key.box_secret().public(), epoch, &evil, &mut net.rng);
            net.down.push_back((c, wire::encode(&Frame::Carnet(Carnet::Rekey { sealed }))));
            net.pump();
        }
        // Or as if it were in the account at epoch 1: its card sealed under
        // a key of its own for 1, which the tablet's doesn't open.
        let mut posing = laptop_ring.clone();
        posing.insert(1, VaultKey::generate(&mut net.rng));
        let sealed = Signer::new(ACCOUNT, b"laptop", &laptop_key, &posing, &mut net.rng).seal(&tablet_key.box_secret().public(), 2, &evil, &mut net.rng);
        net.down.push_back((c, wire::encode(&Frame::Carnet(Carnet::Rekey { sealed }))));
        net.pump();
        assert_eq!(net.device(c).session.ring().epochs(), [0, 1]);
        assert_eq!(net.device(c).session.ring().current(), (1, &k1), "the tablet's newest key is the account's still");
        assert!(rekeyed(&mut net, c).is_empty());

        // What the tablet writes after: under epoch 1, which a reads and
        // the laptop doesn't.
        net.write(c, "before\nafter the laptop\n");
        assert_eq!(net.device(a).notes.docs[ROOM].text(), "before\nafter the laptop\n");
        for s in net.hub.store().spans(ACCOUNT, ROOM) {
            let epoch = match elo::header(&s.record) {
                Ok(elo::Header::Delta { epoch, .. } | elo::Header::Snapshot { epoch, .. }) => epoch,
                other => panic!("{other:?}"),
            };
            assert!(epoch <= 1, "nothing sealed under the laptop's key");
            if epoch == 1 {
                assert!(elo::open(&laptop_ring, ROOM, &s.record).is_err());
            }
        }

        // The words: the server hands a recovery the laptop's key with the
        // real ones, ahead of them. The device let in is on epoch 1.
        let mut rekeys = vec![signer.seal(&words.box_secret().public(), 2, &evil, &mut net.rng)];
        rekeys.extend(net.hub.store().rekeys(ACCOUNT, &[]));
        rekeys.push(signer.seal(&words.box_secret().public(), 1, &evil, &mut net.rng));
        let mut r = Recoverer::new(ORIGIN, words.clone(), b"phoenix".to_vec(), DeviceKey::generate(&mut net.rng));
        let wrapped = carnet_sync::crypto::Ring::new(vault.clone()).wrap(&words.wrapping_key(), &mut net.rng);
        let challenge = |n| wire::encode(&Frame::Carnet(Carnet::Challenge { nonce: [n; 32], server: carnet_sync::compat::Greeting::ours(Vec::new()) }));
        r.receive(&challenge(1));
        r.receive(&wire::encode(&Frame::Carnet(Carnet::Recovered { account: ACCOUNT.to_vec(), wrapped, rekeys })));
        r.receive(&challenge(2));
        let Joining::In(admitted) = r.state else { panic!("{:?}", r.state) };
        assert_eq!(admitted.ring.epochs(), [0, 1]);
        assert_eq!(admitted.ring.current(), (1, &k1));
    }
}

/// Another device than the first removes two, one after the other, while
/// the first is away. Back, the first takes both keys, in order; the two
/// left end on epoch 2 and read what each other writes after.
#[test]
fn an_honest_removal_by_any_device_is_taken_by_every_device_kept() {
    for store in stores() {
        let mut net = Net::new(81, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let b = net.pair(a, b"laptop", 2);
        let c = net.pair(a, b"tablet", 3);
        let d = net.pair(a, b"phone", 4);
        net.write(a, "# Plans\n");
        let away = net.take(a);

        let _ = net.list(b);
        net.revoke(b, b"tablet").unwrap();
        assert_eq!(net.hub.store().epoch(ACCOUNT), 1);
        assert_eq!(net.device(d).session.ring().epochs(), [0, 1]);
        net.revoke(b, b"phone").unwrap();
        assert_eq!(net.hub.store().epoch(ACCOUNT), 2);
        assert_eq!(net.device(b).session.ring().epochs(), [0, 1, 2]);
        let k2 = net.device(b).session.ring().get(2).cloned().unwrap();
        let _ = (c, d);

        // Away, it's sent both keys at its next login: the second first, as
        // a server might; it's held, and taken once the first has come.
        let mut away = away;
        away.events.clear();
        let sent = net.hub.store().rekeys(ACCOUNT, b"first");
        assert_eq!(sent.len(), 2);
        for sealed in sent.iter().rev() {
            let Net { rng, .. } = &mut net;
            away.session.receive(&wire::encode(&Frame::Carnet(Carnet::Rekey { sealed: sealed.clone() })), &mut away.notes, rng);
            away.events.extend(away.session.take_events());
            if away.session.ring().epochs() == [0] {
                assert!(away.events.is_empty(), "the second, held");
            }
        }
        assert_eq!(away.events, [Event::Rekeyed(1), Event::Rekeyed(2)]);
        away.events.clear();
        let a = net.back(away);
        assert_eq!(net.device(a).session.ring().current(), (2, &k2));
        assert!(rekeyed(&mut net, a).is_empty(), "sent again at login: had them");

        // The two left read what each other writes after, sealed under 2.
        net.write(a, "# Plans\n\nfrom the first\n");
        assert_eq!(net.device(b).notes.docs[ROOM].text(), "# Plans\n\nfrom the first\n");
        write_in(&mut net, b, ROOM, "# Plans\n\nfrom the first\nand the laptop\n");
        assert_eq!(net.device(a).notes.docs[ROOM].text(), "# Plans\n\nfrom the first\nand the laptop\n");
        let late: Vec<_> = net.hub.store().spans(ACCOUNT, ROOM).into_iter().filter(|s| matches!(elo::header(&s.record), Ok(elo::Header::Delta { epoch: 2, .. }))).collect();
        assert!(late.len() >= 2, "both wrote under epoch 2");
    }
}

/// A server saying the account is at the last epoch there is: the next
/// isn't worked out by adding to it, and nothing is sent.
#[test]
fn an_epoch_from_the_server_does_not_overflow() {
    for store in stores() {
        let mut net = Net::new(82, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let _b = net.pair(a, b"laptop", 2);
        let _ = net.list(a);
        let devices = net.hub.store().devices(ACCOUNT);
        let rows = devices
            .iter()
            .map(|(id, dev)| wire::DeviceRow { id: id.clone(), public: dev.public, card: dev.card.clone(), seen: 0, you: id == b"first", online: true })
            .collect();
        let list = Carnet::Devices { epoch: u32::MAX, devices: rows, recovery: Vec::new(), password: Vec::new(), passwords: false, passkeys: Vec::new(), password_email: String::new() };
        net.down.push_back((a, wire::encode(&Frame::Carnet(list))));
        net.pump();
        let _ = net.device(a).session.take_out();
        let Net { peers, rng, .. } = &mut net;
        let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
        assert!(d.session.revoke(b"laptop", rng).is_err());
        assert!(d.session.take_out().is_empty(), "nothing sent");
        assert!(!net.hub.store().device(ACCOUNT, b"laptop").unwrap().revoked);
        assert_eq!(net.device(a).session.ring().epochs(), [0]);
    }
}

/// The list as the hub would send it to `conn`, from what the store holds:
/// for a test's server to alter before handing it over.
fn honest_list(net: &Net) -> (Vec<wire::DeviceRow>, Vec<u8>, Vec<u8>, Vec<wire::PasskeyRow>) {
    let store = net.hub.store();
    let rows = store
        .devices(ACCOUNT)
        .into_iter()
        .filter(|(_, d)| !d.revoked)
        .map(|(id, d)| wire::DeviceRow { you: id == b"first", online: true, id, public: d.public, card: d.card, seen: 0 })
        .collect();
    let passkeys = store.passkeys(ACCOUNT).into_iter().map(|p| wire::PasskeyRow { public: p.public, card: p.card, made: p.made }).collect();
    (rows, store.recovery_card(ACCOUNT), store.password_card(ACCOUNT), passkeys)
}

/// `conn` handed a list of devices by the server, as it is.
fn hand_list(net: &mut Net, conn: ConnId, (devices, recovery, password, passkeys): (Vec<wire::DeviceRow>, Vec<u8>, Vec<u8>, Vec<wire::PasskeyRow>)) -> Vec<DeviceInfo> {
    let epoch = net.hub.store().epoch(ACCOUNT);
    let list = Carnet::Devices { epoch, devices, recovery, password, passwords: true, passkeys, password_email: String::new() };
    net.down.push_back((conn, wire::encode(&Frame::Carnet(list))));
    net.pump();
    std::mem::take(&mut net.device(conn).events)
        .into_iter()
        .rev()
        .find_map(|e| if let Event::Devices(l) = e { Some(l) } else { None })
        .expect("a list")
}

/// The laptop is removed, and keeps epoch 0's key. The server, with it,
/// hands the first device a list in which something new to seal to is a
/// card the laptop sealed under epoch 0, its own box key in it: the
/// recovery code's (the account has none), the password's, a passkey's, a
/// device that isn't there, or the tablet's. Each such card is one that
/// hasn't said who it is, and the next removal is refused, nothing sent:
/// the laptop is never sealed epoch 2's key. With the list as it is, the
/// removal goes ahead.
#[test]
fn a_card_under_an_older_epoch_is_never_sealed_to() {
    for store in stores() {
        let mut net = Net::new(83, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let (b, laptop_key) = pair_keeping(&mut net, a, b"laptop", 2);
        let _c = net.pair(a, b"tablet", 3);
        let _d = net.pair(a, b"phone", 4);
        let laptop_ring = net.device(b).session.ring().clone();
        let _ = net.list(a);
        net.revoke(a, b"laptop").unwrap();
        assert_eq!(net.hub.store().epoch(ACCOUNT), 1);
        assert_eq!(net.device(a).session.ring().current().0, 1);

        // The laptop's card for each, under epoch 0.
        let laptop_box = laptop_key.box_secret().public();
        let posing = |net: &mut Net, device: &[u8], public: [u8; 32], name: &str| {
            Card { device: device.to_vec(), public, boxed: laptop_box, name: name.into(), credential: Vec::new() }.seal(&laptop_ring, &mut net.rng)
        };
        let passkey_public = DeviceKey::generate(&mut net.rng).public();
        let tablet_public = net.hub.store().device(ACCOUNT, b"tablet").unwrap().public;
        let mut lists = Vec::new();
        let mut l = honest_list(&net);
        l.1 = posing(&mut net, &[], laptop_key.public(), "");
        lists.push(("the recovery code's", l));
        let mut l = honest_list(&net);
        l.2 = posing(&mut net, password::SLOT, laptop_key.public(), "ann@example.com");
        lists.push(("the password's", l));
        let mut l = honest_list(&net);
        let card = posing(&mut net, &passkey::slot(&passkey_public), passkey_public, "Passkey");
        l.3.push(wire::PasskeyRow { public: passkey_public, card, made: 1 });
        lists.push(("a passkey's", l));
        let mut l = honest_list(&net);
        let card = posing(&mut net, b"ghost", laptop_key.public(), "ghost");
        l.0.push(wire::DeviceRow { id: b"ghost".to_vec(), public: laptop_key.public(), card, seen: 0, you: false, online: false });
        lists.push(("a device not in the account", l));
        let mut l = honest_list(&net);
        let card = posing(&mut net, b"tablet", tablet_public, "tablet");
        l.0.iter_mut().find(|r| r.id == b"tablet").unwrap().card = card;
        lists.push(("the tablet's", l));

        for (what, list) in lists {
            let shown = hand_list(&mut net, a, list);
            for row in &shown {
                if row.id == b"ghost" || (what == "the tablet's" && row.id == b"tablet") {
                    assert_eq!(row.name, None, "{what}: shown as not having said who it is");
                }
            }
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            let _ = d.session.take_out();
            assert!(d.session.revoke(b"phone", rng).is_err(), "{what}: refused");
            assert!(d.session.take_out().is_empty(), "{what}: nothing sent");
            assert!(!net.hub.store().device(ACCOUNT, b"phone").unwrap().revoked);
            assert_eq!(net.hub.store().epoch(ACCOUNT), 1);
        }

        // The list as it is: removed, and epoch 2's key sealed to nothing
        // the laptop opens.
        let list = honest_list(&net);
        hand_list(&mut net, a, list);
        net.revoke(a, b"phone").unwrap();
        assert_eq!(net.hub.store().epoch(ACCOUNT), 2);
        assert_eq!(net.device(a).session.ring().current().0, 2);
        let mut ring = laptop_ring.clone();
        for to in [&b"first"[..], b"tablet", b"laptop", b"ghost", &[]] {
            let mut sent = net.hub.store().rekeys(ACCOUNT, to);
            rekey::take_all(&mut ring, ACCOUNT, &laptop_key.box_secret(), &mut sent);
        }
        assert_eq!(ring.epochs(), [0], "the laptop has no key after its own");
    }
}

/// A list read before this device took a new key: its cards were the old
/// epoch's. A removal by it is refused, and the list asked for again; then
/// it goes ahead.
#[test]
fn a_list_from_before_a_new_key_is_read_again() {
    for store in stores() {
        let mut net = Net::new(84, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let _b = net.pair(a, b"laptop", 2);
        let c = net.pair(a, b"tablet", 3);
        let _d = net.pair(a, b"phone", 4);
        // The tablet reads the list, then is away while the laptop is
        // removed; back, it takes epoch 1's key, its list still epoch 0's.
        let _ = net.list(c);
        let away = net.take(c);
        let _ = net.list(a);
        net.revoke(a, b"laptop").unwrap();
        let c = net.back(away);
        assert_eq!(net.device(c).session.ring().current().0, 1);
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&c) else { panic!() };
            let _ = d.session.take_out();
            assert!(d.session.revoke(b"phone", rng).is_err());
            let out = d.session.take_out();
            assert_eq!(out.len(), 1, "the list asked for, nothing else");
            assert!(matches!(wire::decode(&out[0]), Ok(Frame::Carnet(Carnet::ListDevices))));
            d.session.list_devices();
        }
        net.pump();
        net.revoke(c, b"phone").unwrap();
        assert_eq!(net.hub.store().epoch(ACCOUNT), 2);
        assert_eq!(net.device(a).session.ring().current().0, 2);
    }
}

/// Two removals, one after the other, with the tablet away across both and
/// a recovery code and a passkey set: the second is sealed to the tablet's
/// card and the slots' as the first sealed them anew. The tablet, back,
/// takes both keys (the card it sends as it logs in, under epoch 0, isn't
/// kept in place of the removal's); the words and the passkey let a device
/// in with every key; and a third removal goes ahead with the tablet in it.
#[test]
fn a_second_removal_includes_a_device_away_across_the_first() {
    for store in stores() {
        let mut net = Net::new(85, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let _b = net.pair(a, b"laptop", 2);
        let c = net.pair(a, b"tablet", 3);
        let _d = net.pair(a, b"phone", 4);
        let _e = net.pair(a, b"watch", 5);
        let words = RecoveryCode::generate(&mut net.rng);
        let prf = [7; 32];
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            d.session.set_recovery(&words, rng);
            d.session.add_passkey("Passkey", b"cred", &prf, rng).unwrap();
        }
        net.pump();
        net.write(a, "# Plans\n");
        let away = net.take(c);

        let _ = net.list(a);
        net.revoke(a, b"laptop").unwrap();
        assert_eq!(net.hub.store().epoch(ACCOUNT), 1);
        let card = net.hub.store().device(ACCOUNT, b"tablet").unwrap().card;
        assert_eq!(Card::epoch(&card), Some(1), "the tablet's card, sealed anew by the removal");
        let shown = net.list(a);
        assert!(shown.iter().all(|r| r.name.is_some()), "{shown:?}");
        net.revoke(a, b"phone").unwrap();
        assert_eq!(net.hub.store().epoch(ACCOUNT), 2);
        assert_eq!(net.hub.store().rekeys(ACCOUNT, b"tablet").len(), 2);

        let c = net.back(away);
        assert_eq!(net.device(c).session.ring().epochs(), [0, 1, 2]);
        assert_eq!(Card::epoch(&net.hub.store().device(ACCOUNT, b"tablet").unwrap().card), Some(2));
        net.write(a, "# Plans\n\nafter both\n");
        assert_eq!(net.device(c).notes.docs[ROOM].text(), "# Plans\n\nafter both\n");

        // The words, and the passkey: in, with every key.
        for n in [&b"phoenix"[..], b"griffin"] {
            let key = DeviceKey::generate(&mut net.rng);
            let r = if n == b"phoenix" { Recoverer::new(ORIGIN, words.clone(), n.to_vec(), key.clone()) } else { Recoverer::passkey(ORIGIN, &prf, n.to_vec(), key.clone()) };
            let conn = net.connect(Peer::Recoverer(r));
            net.pump();
            net.admit(conn, n, key, 9);
            assert_eq!(net.device(conn).session.ring().epochs(), [0, 1, 2]);
            assert_eq!(net.device(conn).notes.docs[ROOM].text(), "# Plans\n\nafter both\n");
        }

        let _ = net.list(c);
        net.revoke(c, b"watch").unwrap();
        assert_eq!(net.hub.store().epoch(ACCOUNT), 3);
        assert_eq!(net.device(a).session.ring().current().0, 3);
    }
}
