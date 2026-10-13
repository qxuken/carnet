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

/// A host may refuse an invite before it's taken (`Admission::admit`: a
/// hosted service lets in only those its signups sent). Refused, the
/// device is told what it's told of an invite never made, and the invite
/// stays as it was; one the host lets in makes its account.
#[test]
fn a_host_refuses_an_invite_it_never_sent_before_it_makes_an_account() {
    use carnet_server::Hub;
    use carnet_server::quota::Admission;
    use carnet_sync::crypto::Random;
    use carnet_sync::invite::InviteCode;
    struct Only([u8; 32]);
    impl Admission for Only {
        fn admit(&self, invite: &[u8; 32]) -> bool {
            *invite == self.0
        }
        fn admitted(&self, _: [u8; 32], _: &[u8]) {}
    }
    for store in common::stores() {
        let mut rng = common::Seeded(5);
        let (sent, stray, never) = (InviteCode::generate(&mut rng), InviteCode::generate(&mut rng), InviteCode::generate(&mut rng));
        let mut hub = Hub::new(common::ORIGIN, store).with_admission(std::sync::Arc::new(Only(sent.hash())));
        hub.store_mut().add_invite(sent.hash(), false, 1000).unwrap();
        hub.store_mut().add_invite(stray.hash(), false, 1000).unwrap();
        let key: [u8; 32] = rng.bytes();
        let refused = hub.enroll(&stray, b"desk", key, &mut rng).unwrap_err();
        let unknown = hub.enroll(&never, b"desk", key, &mut rng).unwrap_err();
        assert_eq!(refused, unknown, "told as an invite never made");
        assert_eq!(hub.store().accounts(), 0);
        assert!(hub.store().invites().iter().any(|(h, _, _)| *h == stray.hash()), "not taken");
        assert_eq!(hub.stats().enroll_refused, 2);
        hub.enroll(&sent, b"desk", key, &mut rng).unwrap();
        assert_eq!(hub.store().accounts(), 1);
    }
}

/// The server's own invite for the first account is good for a day (it
/// prints a new one on each start), any other for a week; one run out is
/// told so, with what gets a new one, in words the apps show as their own.
#[test]
fn an_invite_run_out_says_so_and_the_first_lasts_a_day() {
    use carnet_server::Hub;
    use carnet_server::server::{self, INVITE_DAYS, OWNER_INVITE_DAYS};
    use carnet_sync::crypto::Random;
    use carnet_sync::invite::{InviteCode, RUN_OUT, RUN_OUT_FIRST};
    assert_eq!((OWNER_INVITE_DAYS, INVITE_DAYS), (1, 7));
    assert_eq!((server::invite_days(true), server::invite_days(false)), (1, 7));
    for store in common::stores() {
        let mut hub = Hub::new(common::ORIGIN, store);
        let made = server::new_invite(hub.store_mut()).unwrap();
        let (_, owner, expires) = hub.store().invites().into_iter().find(|(h, _, _)| *h == made.hash()).unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(owner, "no account yet: the owner's");
        assert!(expires.abs_diff(now + 86_400) < 60, "good for a day");

        let mut rng = common::Seeded(9);
        let (first, other, never) = (InviteCode::generate(&mut rng), InviteCode::generate(&mut rng), InviteCode::generate(&mut rng));
        hub.store_mut().add_invite(first.hash(), true, 1000).unwrap();
        hub.store_mut().add_invite(other.hash(), false, 1000).unwrap();
        hub.tick(1000);
        let key: [u8; 32] = rng.bytes();
        assert_eq!(hub.enroll(&first, b"desk", key, &mut rng).unwrap_err(), RUN_OUT_FIRST);
        assert_eq!(hub.enroll(&other, b"desk", key, &mut rng).unwrap_err(), RUN_OUT);
        // Taken, it's gone: tried again, it's one never made.
        assert!(hub.enroll(&first, b"desk", key, &mut rng).unwrap_err().contains("not good"));
        assert!(hub.enroll(&never, b"desk", key, &mut rng).unwrap_err().contains("not good"));
        assert_eq!(hub.store().accounts(), 0);
        // Still good, the printed one makes the owner's account.
        hub.enroll(&made, b"desk", key, &mut rng).unwrap();
        assert!(hub.store().is_owner(&hub.store().account_list()[0].0));
    }
}

/// An app enrolling by a run-out invite says the server's hint as its own,
/// not as "The server says: …".
#[test]
fn an_app_says_an_invite_run_out_as_its_own() {
    use carnet_sync::crypto::Random;
    use carnet_sync::invite::{InviteCode, RUN_OUT_FIRST};
    use carnet_sync::pair::Joining;
    for store in common::stores() {
        let mut net = Net::new(1, store);
        let code = InviteCode::generate(&mut net.rng);
        net.hub.store_mut().add_invite(code.hash(), true, 1000).unwrap();
        net.hub.tick(1000);
        let key = DeviceKey::generate(&mut net.rng);
        let device: [u8; 4] = net.rng.bytes();
        let e = Enroller::new(code.text(), device.to_vec(), key, &mut net.rng);
        let c = net.connect(Peer::Enroller(e));
        net.pump();
        let Some(Peer::Enroller(e)) = net.peers.get(&c) else { panic!() };
        assert!(matches!(&e.state, Joining::Failed(why) if why == RUN_OUT_FIRST), "{:?}", e.state);
    }
}
