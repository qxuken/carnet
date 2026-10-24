//! Notes deleted forever and files dropped (docs/SYNC.md §4, §6): a
//! room removed is gone from the server at once and never taken again,
//! whoever had it; a file dropped is kept 30 days more, for a device
//! that still wants it. On memory and on SQLite alike.

mod common;

use carnet_server::Storage;
use carnet_server::blobs::Blobs;
use carnet_sync::Origin;
use carnet_sync::crypto::VaultKey;
use carnet_sync::session::{Docs, Event};
use common::*;

fn events(net: &mut Net, conn: carnet_server::ConnId) -> Vec<Event> {
    std::mem::take(&mut net.device(conn).events)
}

#[test]
fn a_room_removed_is_gone_for_good() {
    for store in stores() {
        let mut net = Net::new(70, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let b = net.pair(a, b"laptop", 2);
        net.write(a, "a note to delete");
        assert!(net.device(b).notes.docs.contains_key(ROOM), "b has it");
        let account = net.hub.store().account_list().first().map(|(id, ..)| id.clone()).unwrap();
        assert!(!net.hub.store().spans(&account, ROOM).is_empty());
        let _ = events(&mut net, a);
        let _ = events(&mut net, b);

        // Removed: gone at once; the asker told, and b, which had it.
        net.device(a).session.remove_room(ROOM);
        net.pump();
        assert!(events(&mut net, a).contains(&Event::RoomRemoved(ROOM.into())));
        assert!(events(&mut net, b).contains(&Event::RoomRemoved(ROOM.into())));
        assert!(net.hub.store().spans(&account, ROOM).is_empty() && net.hub.store().base(&account, ROOM).is_none());
        assert!(!net.hub.store().rooms(&account).contains(&ROOM.to_string()));
        assert!(net.hub.store().is_removed(&account, ROOM));

        // b writes on to it, unknowing: not taken, and told again.
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&b) else { panic!() };
            d.notes.doc(ROOM).record("b's last words", Origin::Save);
            d.session.saved(ROOM, &mut d.notes, rng);
        }
        net.pump();
        assert!(events(&mut net, b).contains(&Event::RoomRemoved(ROOM.into())));
        assert!(net.hub.store().spans(&account, ROOM).is_empty(), "nothing kept");

        // A device joining later is never offered it.
        let c = net.pair(a, b"phone", 3);
        assert!(!net.device(c).notes.docs.contains_key(ROOM));

        // The vault is every note's: never removed.
        let _ = events(&mut net, a);
        net.device(a).session.remove_room(carnet_sync::vault::ROOM);
        net.pump();
        assert!(events(&mut net, a).iter().any(|e| matches!(e, Event::Refused(_))));
    }
}

/// Removed while away: told on the next login, and once the server says
/// so, not again.
#[test]
fn a_room_removed_offline_is_removed_on_the_next_login() {
    for store in stores() {
        let mut net = Net::new(71, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        net.write(a, "a note");
        let account = net.hub.store().account_list().first().map(|(id, ..)| id.clone()).unwrap();
        let mut away = net.take(a);
        away.session.remove_room(ROOM);
        away.notes.docs.remove(ROOM);
        assert!(!net.hub.store().is_removed(&account, ROOM));
        let a = net.back(away);
        assert!(events(&mut net, a).contains(&Event::RoomRemoved(ROOM.into())));
        assert!(net.hub.store().is_removed(&account, ROOM));
        // Done: the next login doesn't say it again.
        let away = net.take(a);
        let a = net.back(away);
        assert!(!events(&mut net, a).iter().any(|e| matches!(e, Event::RoomRemoved(_))));
    }
}

/// A file dropped stays 30 days, then goes; offered, asked for or put
/// meanwhile, it's wanted, and stays.
#[test]
fn a_file_dropped_goes_after_a_while_unless_wanted() {
    let mut net = Net::new(72, stores().remove(0));
    let vault = VaultKey::generate(&mut net.rng);
    let a = net.first_device(&vault);
    let account = net.hub.store().account_list().first().map(|(id, ..)| id.clone()).unwrap();
    let pics: Vec<Vec<u8>> = (0..3u8).map(|n| vec![n; 1000]).collect();
    let ids: Vec<[u8; 32]> = pics.iter().map(|p| net.device(a).session.blob_id(p)).collect();
    for p in &pics {
        let id = net.device(a).session.blob_id(p);
        net.device(a).session.offer_blobs(vec![(id, p.len())]);
        net.pump();
        let Net { peers, rng, .. } = &mut net;
        let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
        assert!(d.session.put_blob(p, rng));
        net.pump();
    }
    let _ = events(&mut net, a);

    net.device(a).session.drop_blobs(&ids);
    net.pump();
    assert!(events(&mut net, a).contains(&Event::BlobsDropped(ids.clone())));
    assert!(ids.iter().all(|id| net.worker.blobs.is_marked(&account, id)));
    // Asked for, and offered: wanted after all.
    net.device(a).session.get_blob(ids[0]);
    net.device(a).session.offer_blobs(vec![(ids[1], pics[1].len())]);
    net.pump();
    assert!(!net.worker.blobs.is_marked(&account, &ids[0]) && !net.worker.blobs.is_marked(&account, &ids[1]));

    // A day on, nothing goes; 30 days on, the one still marked does.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    assert_eq!(net.worker.sweep(now + 86_400), 0);
    assert_eq!(net.worker.sweep(now + 31 * 86_400), 1);
    assert!(net.worker.blobs.size(&account, &ids[2]).is_none());
    assert!(net.worker.blobs.size(&account, &ids[0]).is_some() && net.worker.blobs.size(&account, &ids[1]).is_some());
}

/// An account deleted from one of its own devices: everything the server
/// kept of it gone, every device let go (this one too), the host told.
/// The server's owner's account can't be.
#[test]
fn an_account_deleted_from_its_device_is_gone() {
    struct Told(std::sync::Mutex<Vec<Vec<u8>>>);
    impl carnet_server::quota::Admission for Told {
        fn admitted(&self, _: [u8; 32], _: &[u8]) {}
        fn removed(&self, account: &[u8]) {
            self.0.lock().unwrap().push(account.to_vec());
        }
    }
    for store in stores() {
        let mut net = Net::new(73, store);
        let told = std::sync::Arc::new(Told(std::sync::Mutex::new(Vec::new())));
        net.hub.set_admission(told.clone());
        // An account of someone's own, by an invite (not the owner's).
        let code = carnet_sync::invite::InviteCode::generate(&mut net.rng);
        net.hub.store_mut().add_invite(code.hash(), false, u64::MAX).unwrap();
        let key = carnet_sync::crypto::DeviceKey::generate(&mut net.rng);
        let e = carnet_sync::pair::Enroller::new(code.text(), b"phone".to_vec(), key.clone(), &mut net.rng);
        let a = net.connect(Peer::Enroller(e));
        net.pump();
        net.admit(a, b"phone", key, 11);
        let b = net.pair(a, b"laptop", 2);
        net.write(a, "words");
        let account = net.device(a).session.account().to_vec();
        assert!(!net.hub.store().is_owner(&account));
        net.device(a).session.delete_account();
        net.pump();
        assert!(net.denied.iter().any(|(c, _)| *c == a) && net.denied.iter().any(|(c, _)| *c == b), "both let go");
        assert!(net.hub.store().devices(&account).is_empty());
        assert!(net.hub.store().spans(&account, ROOM).is_empty());
        assert_eq!(told.0.lock().unwrap().as_slice(), std::slice::from_ref(&account));
    }
    // The owner's: refused, nothing gone.
    for store in stores() {
        let mut net = Net::new(74, store);
        net.hub.store_mut().add_account(ACCOUNT, true).unwrap();
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        net.device(a).session.delete_account();
        net.pump();
        assert!(net.device(a).events.iter().any(|e| matches!(e, Event::Refused(_))));
        assert!(!net.hub.store().devices(ACCOUNT).is_empty());
    }
}
