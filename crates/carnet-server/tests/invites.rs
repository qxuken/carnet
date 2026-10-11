//! Invites from the app (docs/SYNC.md §6): the owner's device makes them,
//! sees them waiting, takes one back, and removes an account; nobody
//! else's device may.

mod common;

use carnet_sync::crypto::{DeviceKey, VaultKey};
use carnet_sync::pair::Enroller;
use carnet_sync::session::Event;
use carnet_sync::wire::{AccountRow, InviteRow};
use common::{ACCOUNT, Net, Peer, stores};

/// The last invite list `conn` was sent.
fn invites(net: &mut Net, conn: u64) -> (bool, Vec<InviteRow>, Vec<AccountRow>) {
    net.device(conn).session.list_invites();
    net.pump();
    last(net, conn)
}

fn last(net: &mut Net, conn: u64) -> (bool, Vec<InviteRow>, Vec<AccountRow>) {
    net.device(conn)
        .events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Invites { owner, invites, accounts } => Some((*owner, invites.clone(), accounts.clone())),
            _ => None,
        })
        .expect("a list")
}

fn refused(net: &mut Net, conn: u64) -> Option<String> {
    net.device(conn).events.iter().rev().find_map(|e| match e {
        Event::Refused(r) => Some(r.clone()),
        _ => None,
    })
}

#[test]
fn the_owner_invites_takes_back_and_removes() {
    for store in stores() {
        let mut net = Net::new(1, store);
        let vault = VaultKey::generate(&mut net.rng);
        let owner = net.first_device(&vault);

        let (is_owner, waiting, accounts) = invites(&mut net, owner);
        assert!(is_owner);
        assert!(waiting.is_empty());
        assert_eq!(accounts.len(), 1);
        assert!(accounts[0].you && accounts[0].owner && accounts[0].devices == 1);

        // An invite, made on the owner's device; the server has its hash.
        let code = {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&owner) else { panic!() };
            d.session.invite(rng).expect("logged in")
        };
        net.pump();
        let (_, waiting, _) = last(&mut net, owner);
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].hash, code.hash());
        assert_eq!(waiting[0].until, 7 * 86_400, "a week from the hub's now (0 here)");

        // Someone makes an account with it.
        let key = DeviceKey::generate(&mut net.rng);
        let e = Enroller::new(code.text(), b"guest".to_vec(), key.clone(), &mut net.rng);
        let guest = net.connect(Peer::Enroller(e));
        net.pump();
        net.admit(guest, b"guest", key, 9);
        assert!(net.device(guest).session.is_ready());

        // Theirs isn't the owner's: no invites, no accounts, and refused.
        let (is_owner, waiting, accounts) = invites(&mut net, guest);
        assert!(!is_owner && waiting.is_empty() && accounts.is_empty());
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&guest) else { panic!() };
            d.session.invite(rng);
        }
        net.pump();
        assert_eq!(refused(&mut net, guest).as_deref(), Some("only the server's owner can do that"));

        // The owner sees the invite used, and the new account.
        let (_, waiting, accounts) = invites(&mut net, owner);
        assert!(waiting.is_empty());
        assert_eq!(accounts.len(), 2);
        let theirs = accounts.iter().find(|a| !a.you).expect("the guest's").clone();
        assert!(!theirs.owner && theirs.devices == 1);
        assert_eq!(invites(&mut net, owner).1.len(), 0, "and none made by the guest's try");

        // An invite taken back is good no more.
        let code = {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&owner) else { panic!() };
            d.session.invite(rng).unwrap()
        };
        net.pump();
        net.device(owner).session.drop_invite(code.hash());
        net.pump();
        assert!(last(&mut net, owner).1.is_empty());
        let key = DeviceKey::generate(&mut net.rng);
        let e = Enroller::new(code.text(), b"late".to_vec(), key, &mut net.rng);
        let late = net.connect(Peer::Enroller(e));
        net.pump();
        assert!(net.denied.iter().any(|(c, r)| *c == late && r.contains("not good")), "{:?}", net.denied);

        // The owner's own account can't go.
        net.device(owner).session.remove_account(ACCOUNT);
        net.pump();
        assert_eq!(refused(&mut net, owner).as_deref(), Some("the owner's own account can't be removed"));

        // The guest's account removed: their device online is told, and
        // told again when it comes back.
        net.device(owner).session.remove_account(&theirs.id);
        net.pump();
        assert!(net.denied.iter().any(|(c, r)| *c == guest && r.contains("removed")), "{:?}", net.denied);
        assert_eq!(last(&mut net, owner).2.len(), 1);
        let d = net.take(guest);
        let again = net.back(d);
        assert!(net.denied.iter().any(|(c, r)| *c == again && r.contains("removed")), "{:?}", net.denied);
        assert!(net.hub.store().devices(&theirs.id).is_empty());
    }
}

/// The server prints an invite for the first account on each start until
/// there is one: once the owner is in, the others are good no more, or
/// any could make a second owner.
#[test]
fn the_owners_other_invites_go_when_the_owner_comes_in() {
    use carnet_server::Hub;
    use carnet_sync::crypto::Random;
    use carnet_sync::invite::InviteCode;
    for store in common::stores() {
        let mut hub = Hub::new(common::ORIGIN, store);
        let mut rng = common::Seeded(3);
        let first = InviteCode::generate(&mut rng);
        let second = InviteCode::generate(&mut rng);
        let later = InviteCode::generate(&mut rng);
        hub.store_mut().add_invite(first.hash(), true, 1000).unwrap();
        hub.store_mut().add_invite(second.hash(), true, 1000).unwrap();
        hub.store_mut().add_invite(later.hash(), false, 1000).unwrap();
        let key: [u8; 32] = rng.bytes();
        hub.enroll(&first, b"desk", key, &mut rng).unwrap();
        let left: Vec<[u8; 32]> = hub.store().invites().into_iter().map(|(h, _, _)| h).collect();
        assert_eq!(left, [later.hash()], "the other owner's invite gone, an ordinary one kept");
        assert!(hub.enroll(&second, b"other", key, &mut rng).is_err());
    }
}
