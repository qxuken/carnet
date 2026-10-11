//! Pictures (docs/SYNC.md §4, §7) through the hub: offered, a link given to
//! put each the server lacks, kept once by id and said so only once there;
//! fetched by a link, and opened only as themselves. On memory and on
//! SQLite alike (the pictures themselves on memory: the links' HTTP is
//! `server.rs`'s test).

mod common;

use carnet_server::blobs::Blobs;

use std::collections::BTreeMap;

use carnet_server::Storage;
use carnet_sync::crypto::{Ring, VaultKey};
use carnet_sync::session::{Event, Identity, Session};
use common::*;

fn events(net: &mut Net, conn: carnet_server::ConnId) -> Vec<Event> {
    std::mem::take(&mut net.device(conn).events)
}

/// `pic` offered by `conn`, put at the link the server gives, and kept.
fn send_picture(net: &mut Net, conn: carnet_server::ConnId, pic: &[u8]) {
    let id = net.device(conn).session.blob_id(pic);
    net.device(conn).session.offer_blobs(vec![id]);
    net.pump();
    {
        let Net { peers, rng, .. } = &mut *net;
        let Some(Peer::Device(d)) = peers.get_mut(&conn) else { panic!() };
        assert!(d.session.put_blob(pic, rng), "wanted, so put");
    }
    net.pump();
}

/// A picture of `n` bytes, not all alike.
fn picture(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

#[test]
fn a_picture_goes_up_by_its_link_and_comes_down_whole() {
    for store in stores() {
        let mut net = Net::new(40, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let b = net.pair(a, b"laptop", 2);
        let big = picture(700_000, 1);
        let id = net.device(a).session.blob_id(&big);
        let _ = events(&mut net, a);
        let _ = events(&mut net, b);

        // Offered: the server lacks it.
        net.device(a).session.offer_blobs(vec![id]);
        net.pump();
        assert_eq!(events(&mut net, a), [Event::BlobWanted(id)]);
        // Put at the link the server gave, then said: kept.
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            assert!(d.session.put_blob(&big, rng));
        }
        net.pump();
        assert_eq!(events(&mut net, a), [Event::BlobStored(id)]);
        let kept = net.worker.blobs.get(ACCOUNT, &id).expect("kept").clone();
        assert!(!kept.windows(64).any(|w| w == &big[1000..1064]), "sealed");
        // Offered again (or by another device): there already.
        net.device(b).session.offer_blobs(vec![id]);
        net.pump();
        assert_eq!(events(&mut net, b), [Event::BlobStored(id)]);

        // The laptop fetches it: the very picture.
        net.device(b).session.get_blob(id);
        net.pump();
        assert_eq!(events(&mut net, b), [Event::BlobGot(id, big.clone())]);

        // One the server doesn't have: failed, said so.
        let other = net.device(b).session.blob_id(b"never sent");
        net.device(b).session.get_blob(other);
        net.pump();
        assert_eq!(events(&mut net, b), [Event::BlobFailed(other)]);

        // The server swaps one blob for another under its id: it doesn't
        // open as that picture.
        let small = picture(3000, 2);
        let small_id = net.device(a).session.blob_id(&small);
        send_picture(&mut net, a, &small);
        let swapped = net.worker.blobs.get(ACCOUNT, &small_id).unwrap().clone();
        net.worker.blobs.put(ACCOUNT, &[9; 32], &swapped);
        let _ = events(&mut net, b);
        net.device(b).session.get_blob([9; 32]);
        net.pump();
        assert_eq!(events(&mut net, b), [Event::BlobFailed([9; 32])]);
    }
}

/// Another account's picture isn't there for this one, even by its id.
#[test]
fn a_picture_stays_in_its_account() {
    for store in stores() {
        let mut net = Net::new(41, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let pic = picture(5000, 3);
        let id = net.device(a).session.blob_id(&pic);
        send_picture(&mut net, a, &pic);
        net.hub.store_mut().add_account(b"other", false).unwrap();
        assert!(net.worker.blobs.size(ACCOUNT, &id).is_some());
        assert_eq!(net.worker.blobs.size(b"other", &id), None);
        // Asked for from the other account, by its id: not there for it.
        let key = carnet_sync::crypto::DeviceKey::generate(&mut net.rng);
        net.hub.store_mut().add_device(b"other", b"theirs", key.public()).unwrap();
        let id_other = Identity { origin: common::ORIGIN.into(), account: b"other".to_vec(), device: b"theirs".to_vec(), key, ring: Ring::new(VaultKey::generate(&mut net.rng)), name: String::new() };
        let mut session = Session::new(id_other, 0);
        session.connected();
        let o = net.connect(Peer::Device(Device { session, notes: Notes { peer: 7, docs: BTreeMap::new() }, events: Vec::new() }));
        net.pump();
        net.device(o).session.get_blob(id);
        net.pump();
        assert_eq!(events(&mut net, o).last(), Some(&Event::BlobFailed(id)));
    }
}

/// A picture asked for while the device is still logging in (a note
/// opened at once): asked once it's in, and got.
#[test]
fn a_picture_asked_for_while_logging_in_comes_once_in() {
    for store in stores() {
        let mut net = Net::new(42, store);
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let pic = picture(4000, 5);
        let id = net.device(a).session.blob_id(&pic);
        send_picture(&mut net, a, &pic);
        let mut d = net.take(a);
        d.session.connected();
        d.session.get_blob(id);
        let _ = std::mem::take(&mut d.events);
        // `back` connects again; the session was told already.
        let conn = net.connect(Peer::Device(d));
        net.pump();
        assert!(net.device(conn).events.contains(&Event::BlobGot(id, pic)), "{:?}", net.device(conn).events);
    }
}
