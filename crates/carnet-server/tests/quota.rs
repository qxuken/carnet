//! What an account may keep (docs/SYNC.md §11): a server of one's own
//! keeps everything; a hosted plan's limit stops new pictures once it's
//! reached, never notes' text; a lapsed plan reads, and writes nothing,
//! and the device keeps what it wrote until it can. On memory and SQLite.

mod common;

use std::sync::{Arc, Mutex};

use carnet_server::quota::{FULL, Limit, Quota, READ_ONLY};
use carnet_sync::Origin;
use carnet_sync::crypto::VaultKey;
use carnet_sync::session::{Docs, Event};
use common::*;

/// A plan the test changes as it goes.
struct Plan(Mutex<Limit>);

impl Quota for Plan {
    fn limit(&self, _: &[u8]) -> Limit {
        self.0.lock().unwrap().clone()
    }
}

fn events(net: &mut Net, conn: carnet_server::ConnId) -> Vec<Event> {
    std::mem::take(&mut net.device(conn).events)
}

fn usage(net: &mut Net, conn: carnet_server::ConnId) -> carnet_sync::session::Usage {
    net.device(conn).session.usage();
    net.pump();
    events(net, conn).into_iter().rev().find_map(|e| if let Event::Usage(u) = e { Some(u) } else { None }).expect("usage came")
}

fn offer(net: &mut Net, conn: carnet_server::ConnId, pic: &[u8]) -> Vec<Event> {
    let id = net.device(conn).session.blob_id(pic);
    net.device(conn).session.offer_blobs(vec![id]);
    net.pump();
    events(net, conn)
}

#[test]
fn a_plan_limits_pictures_and_a_lapsed_one_reads_only() {
    for store in stores() {
        let mut net = Net::new(80, store);
        let plan = Arc::new(Plan(Mutex::new(Limit { bytes: Some(10_000), writable: true, plan: "Personal".into(), until: 1_900_000_000 })));
        net.hub.set_quota(plan.clone());
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        net.write(a, "some words");
        let _ = events(&mut net, a);

        // What it keeps, and may.
        let u = usage(&mut net, a);
        assert!(u.notes > 0 && u.pictures == 0);
        assert_eq!((u.limit, u.writable, u.plan.as_str(), u.until), (Some(10_000), true, "Personal", Some(1_900_000_000)));

        // Under the limit: a picture goes in, and may take it over.
        let big = vec![7u8; 9_990];
        assert!(offer(&mut net, a, &big).iter().any(|e| matches!(e, Event::BlobWanted(_))));
        {
            let Net { peers, rng, .. } = &mut net;
            let Some(Peer::Device(d)) = peers.get_mut(&a) else { panic!() };
            assert!(d.session.put_blob(&big, rng));
        }
        net.pump();
        assert!(usage(&mut net, a).pictures > 9_990, "over the limit by one picture");
        // Over it: the next waits, said once; one kept is still said kept.
        let said = offer(&mut net, a, &[8u8; 100]);
        assert!(said.contains(&Event::Refused(FULL.into())), "{said:?}");
        assert!(!said.iter().any(|e| matches!(e, Event::BlobWanted(_))));
        assert!(offer(&mut net, a, &big).iter().any(|e| matches!(e, Event::BlobStored(_))));
        // Notes' text is never refused for size.
        net.write(a, "more words, past the limit");
        let account = net.hub.store().account_list()[0].0.clone();
        let before = net.hub.store().usage(&account);

        // Lapsed: read only. A device logging in is told once; what it
        // writes stays with it, and nothing is sent; pictures wait.
        plan.0.lock().unwrap().writable = false;
        let away = net.take(a);
        let a = net.back(away);
        assert_eq!(events(&mut net, a).iter().filter(|e| matches!(e, Event::ReadOnly)).count(), 1);
        net.write(a, "written while read only");
        assert_eq!(net.hub.store().usage(&account), before, "nothing taken");
        assert!(offer(&mut net, a, &[9u8; 10]).contains(&Event::Refused(READ_ONLY.into())));
        // Renewed: on the next login, what waited goes.
        *plan.0.lock().unwrap() = Limit { bytes: Some(1 << 30), ..Limit::default() };
        let away = net.take(a);
        let a = net.back(away);
        assert!(net.hub.store().usage(&account) > before, "what waited went");
        let b = net.pair(a, b"laptop", 2);
        assert!(net.device(b).notes.doc(ROOM).text().contains("written while read only"));
        let _ = Origin::Save;
    }
}

/// A suspension put on and lifted while the account's devices are online
/// (the host says so: `Hub::limit_changed`): each is told at once, with
/// no login between. Suspended, it sends nothing; lifted, what it wrote
/// meanwhile goes, and it writes on as ever; its plan's line is the new
/// one without asking.
#[test]
fn a_limit_changed_is_told_to_the_devices_online() {
    for store in stores() {
        let mut net = Net::new(81, store);
        let plan = Arc::new(Plan(Mutex::new(Limit { bytes: Some(10_000), writable: true, plan: "Personal".into(), until: 0 })));
        net.hub.set_quota(plan.clone());
        let vault = VaultKey::generate(&mut net.rng);
        let a = net.first_device(&vault);
        let b = net.pair(a, b"laptop", 2);
        net.write(a, "before. ");
        let _ = (events(&mut net, a), events(&mut net, b));
        let account = net.hub.store().account_list()[0].0.clone();

        // Suspended: told now, unasked, both devices; nothing more taken.
        plan.0.lock().unwrap().writable = false;
        let outs = net.hub.limit_changed(&account);
        assert_eq!(outs.len(), 2, "one for each socket of the account");
        net.route(outs);
        net.pump();
        for d in [a, b] {
            let said = events(&mut net, d);
            assert_eq!(said.iter().filter(|e| matches!(e, Event::ReadOnly)).count(), 1, "{said:?}");
            assert!(said.iter().any(|e| matches!(e, Event::Usage(u) if !u.writable)));
        }
        let before = net.hub.store().usage(&account);
        let refused = net.hub.stats().batches_refused;
        net.write(a, "while suspended. ");
        assert_eq!(net.hub.store().usage(&account), before, "nothing taken");
        assert_eq!(net.device(a).session.waiting(), 1, "a change waits to be sent");
        assert_eq!(net.hub.stats().batches_refused, refused, "nor even sent to be refused");
        assert!(!net.device(b).notes.doc(ROOM).text().contains("while suspended"));

        // Lifted, on a bigger plan: told now; what waited goes, and the
        // other device has it, with no login between.
        *plan.0.lock().unwrap() = Limit { bytes: Some(1 << 30), writable: true, plan: "Family".into(), until: 0 };
        let outs = net.hub.limit_changed(&account);
        net.route(outs);
        net.pump();
        let said = events(&mut net, a);
        assert!(said.contains(&Event::Writable), "{said:?}");
        assert!(said.iter().any(|e| matches!(e, Event::Usage(u) if u.writable && u.plan == "Family" && u.limit == Some(1 << 30))));
        assert!(net.hub.store().usage(&account) > before, "what waited went");
        assert!(net.device(b).notes.doc(ROOM).text().contains("while suspended"));
        assert!(events(&mut net, b).contains(&Event::Writable));
        assert_eq!(net.device(a).session.waiting(), 0, "sent, and kept");
        net.write(a, "after.");
        assert!(net.device(b).notes.doc(ROOM).text().contains("after."));
        // Another account's sockets aren't told anything.
        assert!(net.hub.limit_changed(b"someone else").is_empty());
    }
}

/// What a device says waits to be sent: a change made offline, until the
/// server keeps it; a save that changes nothing waits for nothing.
#[test]
fn a_change_made_offline_waits_until_the_server_keeps_it() {
    let mut net = Net::new(82, Box::new(carnet_server::Memory::default()));
    let vault = VaultKey::generate(&mut net.rng);
    let a = net.first_device(&vault);
    net.write(a, "online. ");
    assert_eq!(net.device(a).session.waiting(), 0, "sent and kept at once");
    let mut d = net.take(a);
    // Saved again with nothing new: nothing waits.
    d.session.saved(ROOM, &mut d.notes, &mut net.rng);
    assert_eq!(d.session.waiting(), 0);
    d.notes.doc(ROOM).record("online. offline.", Origin::Save);
    d.session.saved(ROOM, &mut d.notes, &mut net.rng);
    assert_eq!(d.session.waiting(), 1, "written offline: waits");
    let a = net.back(d);
    assert_eq!(net.device(a).session.waiting(), 0, "back online: sent, and kept");
}
