//! New epochs' keys, through the hub (docs/SYNC.md §7, "Devices, removal
//! and key epochs"): each goes signed by the device that made it, and a
//! device takes one only from a device in the account at its newest
//! epoch, for the epoch right after, never in place of a key it has. A
//! device removed, with the server's help, hands nobody a key of its own.
//! On memory and on SQLite alike.

mod common;

use carnet_server::{ConnId, Storage};
use carnet_sync::crypto::{DeviceKey, RecoveryCode, VaultKey};
use carnet_sync::elo;
use carnet_sync::pair::{Joiner, Joining, Recoverer};
use carnet_sync::rekey::Signer;
use carnet_sync::session::{Docs, Event};
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
