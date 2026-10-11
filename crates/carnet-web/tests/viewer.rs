//! The viewer's core against the hub, natively: a desktop-like device
//! writes notes, a folder, a pin and a checklist; the browser's side lists
//! them, draws them, ticks a box, and the desktop has the tick.

use std::collections::BTreeMap;

use carnet_server::{Hub, Memory, Out, Storage};
use carnet_core::compare;
use carnet_sync::crypto::{DeviceKey, Random, Ring, VaultKey};
use carnet_sync::pair::{Joiner, Joining, PairCode};
use carnet_sync::session::{Docs, Event, Identity, Session};
use carnet_sync::vault::{self, Place};
use carnet_sync::{Key, NoteDoc, Origin};
use carnet_web::notes::{Me, Newer, Notes, STATE, Said};

const ORIGIN: &str = "https://web.example";
const ACCOUNT: &[u8] = b"acct-web";

struct Seeded(u64);

impl Random for Seeded {
    fn fill(&mut self, out: &mut [u8]) {
        for b in out {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            *b = (z ^ (z >> 31)) as u8;
        }
    }
}

struct Desk {
    session: Session,
    docs: BTreeMap<String, NoteDoc>,
}

struct DeskDocs<'a>(&'a mut BTreeMap<String, NoteDoc>);

impl Docs for DeskDocs<'_> {
    fn rooms(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
    fn doc(&mut self, room: &str) -> &NoteDoc {
        self.0.entry(room.to_string()).or_insert_with(|| NoteDoc::new(7))
    }
}

thread_local! {
    /// The picture worker, as the server runs one beside the hub: one for
    /// each test (each runs on a thread of its own).
    static WORKER: std::cell::RefCell<carnet_server::blobs::Worker<carnet_server::blobs::MemoryBlobs>> =
        std::cell::RefCell::new(carnet_server::blobs::Worker::memory());
}

/// Everything delivered, both ways, until nothing moves.
fn pump(hub: &mut Hub<Memory>, desk: &mut Desk, web: &mut Notes, rng: &mut Seeded) {
    pump_with(hub, desk, web, &mut None, rng);
}

/// The same, with a new device joining on connection 3.
fn pump_with(hub: &mut Hub<Memory>, desk: &mut Desk, web: &mut Notes, joiner: &mut Option<Joiner>, rng: &mut Seeded) {
    for _ in 0..1000 {
        let mut moved = false;
        // Pictures' HTTP, done as the server's links would have it.
        let serve = |t: &carnet_sync::session::Transfer| WORKER.with(|w| w.borrow_mut().blobs.serve(t));
        for t in desk.session.take_transfers() {
            moved = true;
            let put = matches!(t, carnet_sync::session::Transfer::Put { .. });
            desk.session.transferred(t.id(), put, serve(&t));
        }
        for t in web.take_transfers() {
            moved = true;
            let put = matches!(t, carnet_sync::session::Transfer::Put { .. });
            web.transferred(t.id(), put, serve(&t), rng);
        }
        let mut outs: Vec<Out> = Vec::new();
        for b in desk.session.take_out() {
            moved = true;
            outs.extend(hub.receive(1, &b, rng));
        }
        for b in web.session.take_out() {
            moved = true;
            outs.extend(hub.receive(2, &b, rng));
        }
        for b in joiner.as_mut().map(Joiner::take_out).unwrap_or_default() {
            moved = true;
            outs.extend(hub.receive(3, &b, rng));
        }
        let outs = WORKER.with(|w| w.borrow_mut().done(outs));
        for o in outs {
            if let Out::Send(conn, bytes) = o {
                match conn {
                    1 => desk.session.receive(&bytes, &mut DeskDocs(&mut desk.docs), rng),
                    2 => web.receive(&bytes, rng),
                    _ => {
                        if let Some(j) = joiner.as_mut() {
                            j.receive(&bytes, rng);
                        }
                    }
                }
            }
        }
        if !moved {
            return;
        }
    }
    panic!("did not settle");
}

/// A desktop-like device and the browser, both in and connected.
fn two(rng: &mut Seeded) -> (Hub<Memory>, Desk, Notes) {
    two_with(rng, |_| {})
}

/// The same, `before` done on the browser before it connects.
fn two_with(rng: &mut Seeded, before: impl FnOnce(&mut Notes)) -> (Hub<Memory>, Desk, Notes) {
    let mut hub = Hub::new(ORIGIN, Memory::default());
    hub.store_mut().add_account(ACCOUNT, true).unwrap();
    let ring = Ring::new(VaultKey::generate(rng));
    let (dk, wk) = (DeviceKey::generate(rng), DeviceKey::generate(rng));
    hub.store_mut().add_device(ACCOUNT, b"desk", dk.public()).unwrap();
    hub.store_mut().add_device(ACCOUNT, b"web", wk.public()).unwrap();
    let id = Identity { origin: ORIGIN.into(), account: ACCOUNT.to_vec(), device: b"desk".to_vec(), key: dk, ring: ring.clone(), name: "Desk".into() };
    let mut desk = Desk { session: Session::new(id, 0), docs: BTreeMap::new() };
    let me = Me { account: ACCOUNT.to_vec(), device: b"web".to_vec(), key: wk, ring };
    let mut web = Notes::open(ORIGIN, me, "Phone", &[], 99).unwrap();
    before(&mut web);
    desk.session.connected();
    web.session.connected();
    for c in [1, 2] {
        for o in hub.connect(c, rng) {
            if let Out::Send(_, b) = o {
                if c == 1 {
                    desk.session.receive(&b, &mut DeskDocs(&mut desk.docs), rng);
                } else {
                    web.receive(&b, rng);
                }
            }
        }
    }
    pump(&mut hub, &mut desk, &mut web, rng);
    (hub, desk, web)
}

#[test]
fn the_browser_lists_devices_lets_one_in_makes_words_and_removes_one() {
    let mut rng = Seeded(2);
    let (mut hub, mut desk, mut web) = two(&mut rng);

    // Listed: this browser first, each by the name its card gives.
    web.list_devices();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    // (The server also sends a list when another device's card changes:
    // the last is the one that counts.)
    let Some(Said::Devices(list)) = web.said.drain(..).rfind(|s| matches!(s, Said::Devices(_))) else { panic!("no list") };
    let names: Vec<(Option<&str>, bool)> = list.iter().map(|d| (d.name.as_deref(), d.you)).collect();
    assert_eq!(names, [(Some("Phone"), true), (Some("Desk"), false)]);

    // A new device, let in by a code this browser shows.
    web.pair_open(&mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let code = web.said.drain(..).find_map(|s| if let Said::PairCode(c) = s { Some(c) } else { None }).expect("a code");
    let link = PairCode::parse(&code).unwrap().link(ORIGIN);
    let mut joiner = Some(Joiner::new(PairCode::from_link(&link).unwrap(), b"tab".to_vec(), DeviceKey::generate(&mut rng)));
    for o in hub.connect(3, &mut rng) {
        if let Out::Send(_, b) = o {
            joiner.as_mut().unwrap().receive(&b, &mut rng);
        }
    }
    pump_with(&mut hub, &mut desk, &mut web, &mut joiner, &mut rng);
    assert!(matches!(joiner.unwrap().state, Joining::In(_)));
    assert!(web.said.contains(&Said::Paired(b"tab".to_vec())), "{:?}", web.said);

    // Recovery words, set on the server.
    let words = web.make_recovery(&mut rng).expect("connected");
    assert_eq!(words.split_whitespace().count(), 12);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.said.contains(&Said::RecoverySet));

    // The desktop taken out from here: the new device ("tab") hasn't
    // said who it is, so first it must be removed too. The browser gets
    // the new key and keeps it; the desktop is turned away. No list asked
    // for since the words were made: the session read it again itself,
    // or the server would refuse (the code wasn't in the last one).
    let before = web.me.ring.current().0;
    assert!(web.remove_device(b"tab", &mut rng).is_ok());
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.errors.is_empty(), "{:?}", web.errors);
    web.list_devices();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.remove_device(b"desk", &mut rng).is_ok());
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.me.ring.current().0 > before, "a new key, kept in its identity");
    assert!(web.said.contains(&Said::Rekeyed));
    assert!(desk.session.take_events().iter().any(|e| matches!(e, Event::Removed { account: false })), "the desktop is out");
}

/// The browser adds a passkey: listed here by name, and on the desktop,
/// whose removal of a device then seals the new key to it too; taken
/// away, it's gone from both lists.
#[test]
fn the_browser_adds_a_passkey_and_takes_it_away() {
    let mut rng = Seeded(5);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    web.list_devices();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let prf = [42; 32];
    web.add_passkey("Passkey from Chrome on Mac", b"credential", &prf, &mut rng).unwrap();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.said.contains(&Said::PasskeySet), "{:?}", web.said);
    let Some(Said::Passkeys(list)) = web.said.drain(..).rfind(|s| matches!(s, Said::Passkeys(_))) else { panic!("no list") };
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].name.as_deref(), list[0].credential.as_slice()), (Some("Passkey from Chrome on Mac"), &b"credential"[..]));
    let seen = desk.session.take_events().into_iter().rev().find_map(|e| if let Event::Passkeys(p) = e { Some(p) } else { None });
    assert_eq!(seen.as_deref(), Some(&list[..]), "the desktop's list has it");

    // The desktop removes the browser: the new key goes to the passkey's
    // slot as well, or the server would refuse it.
    desk.session.list_devices();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    desk.session.revoke(b"web", &mut rng).unwrap();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let slot = carnet_sync::passkey::slot(&list[0].id);
    assert_eq!(hub.store().rekeys(ACCOUNT, &slot).len(), 1);
    assert!(desk.session.take_events().iter().all(|e| !matches!(e, Event::Refused(_))));

    desk.session.remove_passkey(&list[0].id);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(hub.store().passkeys(ACCOUNT).is_empty() && hub.store().rekeys(ACCOUNT, &slot).is_empty());
}

/// The owner's browser makes an invite, sees it waiting with its code,
/// and takes it back.
#[test]
fn the_owners_browser_invites_and_takes_back() {
    let mut rng = Seeded(4);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let invites = |web: &mut Notes| {
        web.said.drain(..).rev().find_map(|s| match s {
            Said::Invites { owner, waiting, accounts } => Some((owner, waiting, accounts)),
            _ => None,
        })
    };
    web.list_invites();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let (owner, waiting, accounts) = invites(&mut web).expect("a list");
    assert!(owner && waiting.is_empty());
    assert_eq!(accounts.iter().map(|a| (a.you, a.devices)).collect::<Vec<_>>(), [(true, 2)]);

    let code = web.make_invite(&mut rng).expect("connected");
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let (_, waiting, _) = invites(&mut web).expect("the list again");
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].1.as_deref(), Some(code.as_str()), "its code, made here");

    web.drop_invite(waiting[0].0.hash);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(invites(&mut web).expect("and again").1.is_empty());
}

#[test]
fn the_browser_shows_a_notes_history_marked_and_restores_one() {
    let mut rng = Seeded(3);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let k = Key::random().to_string();
    desk.docs.insert(k.clone(), NoteDoc::new(7));
    desk.docs[&k].record("# Note

first words
", Origin::Import);
    desk.session.saved(&k, &mut DeskDocs(&mut desk.docs), &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    // The desktop's sync names it in what it writes.
    desk.docs[&k].set_author(Some(b"desk"));
    desk.docs[&k].record("# Note

first words, then more
", Origin::Save);
    desk.session.saved(&k, &mut DeskDocs(&mut desk.docs), &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);

    let h = web.history(&k);
    assert_eq!(h.len(), 2, "{h:?}");
    assert_eq!(h.iter().map(|p| (p.origin, p.added, p.removed)).collect::<Vec<_>>(), [(Some(Origin::Save), 3, 1), (Some(Origin::Import), 3, 0)]);
    // The newest: what it added is washed green, as the desktop marks it.
    let newest = web.past_html(&k, &h[0].version).unwrap();
    assert!(newest.contains(&format!("--mark:#{:08x}", compare::ADDED)), "{newest}");
    // The first, as it was.
    assert!(web.past_html(&k, &h[1].version).unwrap().contains("first words</p>"));
    assert_eq!(web.past_html(&k, "00"), None);

    // Restored here: the desktop has the first version again.
    assert!(web.restore(&k, &h[1].version, &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(desk.docs[&k].text(), "# Note

first words
");
    let h3 = web.history(&k);
    assert_eq!(h3.len(), 3, "a restore is itself a version");
    // Who made each: here, the desktop (by its id), and the first, which
    // said nothing.
    let by: Vec<(Option<&[u8]>, bool)> = h3.iter().map(|p| (p.by.as_deref(), p.mine)).collect();
    assert_eq!(by, [(Some(b"web".as_slice()), true), (Some(b"desk".as_slice()), false), (None, false)]);
    assert!(!web.restore(&k, &h[1].version, &mut rng), "already so");
}

#[test]
fn the_browser_lists_draws_and_ticks() {
    let mut rng = Seeded(1);
    let mut hub = Hub::new(ORIGIN, Memory::default());
    hub.store_mut().add_account(ACCOUNT, true).unwrap();
    let ring = Ring::new(VaultKey::generate(&mut rng));
    let (dk, wk) = (DeviceKey::generate(&mut rng), DeviceKey::generate(&mut rng));
    hub.store_mut().add_device(ACCOUNT, b"desk", dk.public()).unwrap();
    hub.store_mut().add_device(ACCOUNT, b"web", wk.public()).unwrap();
    let id = Identity { origin: ORIGIN.into(), account: ACCOUNT.to_vec(), device: b"desk".to_vec(), key: dk, ring: ring.clone(), name: "Desk".into() };
    let mut desk = Desk { session: Session::new(id, 0), docs: BTreeMap::new() };

    // The desktop's notes: a checklist pinned in a folder, and another.
    let shop = Key::random();
    let plan = Key::random();
    desk.docs.insert(shop.to_string(), NoteDoc::new(7));
    desk.docs[&shop.to_string()].record("# Shop\n\n- [ ] milk\n- [x] bread\n\n![](../attachments/p.png)\n\n![The hill at seven](hill.jpg)\n", Origin::Save);
    desk.docs.insert(plan.to_string(), NoteDoc::new(7));
    desk.docs[&plan.to_string()].record("# Plan\n\nSome ==words== here.\n\n![[photo.png|200]]\n\n![](/Users/me/Pictures/sea.png)\n\n![](https://example.com/a.png)\n", Origin::Save);
    let v = NoteDoc::new(7);
    // A picture the plan names the Obsidian way, by its name alone.
    v.put_picture("attachments/2026/photo.png", &"ab".repeat(32));
    // One beside its note, not in the attachments folder, as an earlier
    // desktop named it: shown while the vault names it.
    v.put_picture("Home/hill.jpg", &"cd".repeat(32));
    v.set_place(shop, &Place { folder: "Home".into(), name: "Shop".into(), pinned: true, trashed: false });
    v.set_place(plan, &Place { folder: String::new(), name: "Plan".into(), pinned: false, trashed: false });
    v.set_folder("Home", true);
    desk.docs.insert(vault::ROOM.to_string(), v);

    // The browser, from nothing.
    let me = Me { account: ACCOUNT.to_vec(), device: b"web".to_vec(), key: wk, ring };
    assert_eq!(Me::from_bytes(&me.to_bytes()).map(|m| m.to_bytes()), Some(me.to_bytes()), "its keys read back");
    let mut web = Notes::open(ORIGIN, me, "Phone", &[], 99).unwrap();

    desk.session.connected();
    web.session.connected();
    for c in [1, 2] {
        for o in hub.connect(c, &mut rng) {
            if let Out::Send(_, b) = o {
                if c == 1 {
                    desk.session.receive(&b, &mut DeskDocs(&mut desk.docs), &mut rng);
                } else {
                    web.receive(&b, &mut rng);
                }
            }
        }
    }
    pump(&mut hub, &mut desk, &mut web, &mut rng);

    // Listed: titles, the folder, the pin, the checklist's count.
    let mut list = web.notes();
    list.sort_by(|a, b| a.title.cmp(&b.title));
    assert_eq!(list.len(), 2, "{list:?}");
    let s = list.iter().find(|n| n.title == "Shop").unwrap();
    assert_eq!((s.folder.as_str(), s.pinned, s.done, s.todos), ("Home", true, 1, 2));
    assert_eq!(web.folders(), ["Home"]);
    assert_eq!(web.search("WORDS"), [plan.to_string()]);

    // Drawn: real checkboxes, the highlight, the picture to fetch.
    let h = web.html(&shop.to_string());
    assert!(h.contains("data-task=\"0\"><span>milk") && h.contains("checked><span>bread") && !h.contains("<p></p>"), "{h}");
    assert!(h.contains("data-picture=\"attachments/p.png\""), "resolved from its folder: {h}");
    assert!(h.contains("data-picture=\"Home/hill.jpg\""), "beside its note: {h}");
    let h = web.html(&plan.to_string());
    assert!(h.contains("<mark>words</mark>"), "{h}");
    assert!(h.contains("<img data-picture=\"attachments/2026/photo.png\" alt=\"\" width=\"200\">"), "found by its name: {h}");
    // A file outside the notes folder: the page says it's on the device
    // that has it; one on the web, as it is.
    assert!(h.contains("data-picture=\"/Users/me/Pictures/sea.png\""), "{h}");
    assert!(h.contains("src=\"https://example.com/a.png\""), "{h}");
    // Only that one has a `src`: the browser asks for no other before the
    // page has it.
    assert_eq!(h.matches("src=").count(), 1, "{h}");
    assert_eq!(web.picture_path("", "photo.png", true).as_deref(), Some("attachments/2026/photo.png"));
    assert_eq!(web.picture_path("Home", "../attachments/p.png", false).as_deref(), Some("attachments/p.png"));
    assert_eq!(web.picture_path("Home", "Hill.JPG", false).as_deref(), Some("Home/hill.jpg"), "as the vault spells it");

    // Ticked here: the desktop has it.
    assert!(web.toggle_task(&shop.to_string(), 0, &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(desk.docs[&shop.to_string()].text(), "# Shop\n\n- [x] milk\n- [x] bread\n\n![](../attachments/p.png)\n\n![The hill at seven](hill.jpg)\n");

    // Kept and opened again: the same notes, the same cursor.
    let saved = web.save();
    let again = Notes::open(ORIGIN, Me::from_bytes(&web.me.to_bytes()).unwrap(), "Phone", &saved, 100).unwrap();
    assert_eq!(again.notes().len(), 2);
    assert_eq!(again.session.cursor(), web.session.cursor());
}

/// Search as the desktop's: the title the library shows — for a note from
/// the desktop, its file's name, which isn't in its words — and the words,
/// case aside; not the folder, nor "Untitled".
#[test]
fn search_finds_a_desktop_notes_title_its_file_name() {
    let mut rng = Seeded(11);
    let mut hub = Hub::new(ORIGIN, Memory::default());
    hub.store_mut().add_account(ACCOUNT, true).unwrap();
    let ring = Ring::new(VaultKey::generate(&mut rng));
    let (dk, wk) = (DeviceKey::generate(&mut rng), DeviceKey::generate(&mut rng));
    hub.store_mut().add_device(ACCOUNT, b"desk", dk.public()).unwrap();
    hub.store_mut().add_device(ACCOUNT, b"web", wk.public()).unwrap();
    let id = Identity { origin: ORIGIN.into(), account: ACCOUNT.to_vec(), device: b"desk".to_vec(), key: dk, ring: ring.clone(), name: "Desk".into() };
    let mut desk = Desk { session: Session::new(id, 0), docs: BTreeMap::new() };

    // As the desktop syncs them: the title is the file's name, kept in the
    // vault; the words have no heading.
    let weekend = Key::random();
    let item = Key::random();
    let heading = Key::random();
    let untitled = Key::random();
    let v = NoteDoc::new(7);
    for (k, text, folder, name) in [
        (weekend, "The light on the hill at seven.\n", "Home", "Weekend"),
        (item, "Note number 22 with a few words in it.\n", "Many", "Item 22"),
        (heading, "# Acme plan\n\nQuarterly words.\n", "Work", "Acme plan"),
        (untitled, "Nothing to title it by.\n", "", ""),
    ] {
        desk.docs.insert(k.to_string(), NoteDoc::new(7));
        desk.docs[&k.to_string()].record(text, Origin::Save);
        v.set_place(k, &Place { folder: folder.into(), name: name.into(), pinned: false, trashed: false });
    }
    desk.docs.insert(vault::ROOM.to_string(), v);

    let me = Me { account: ACCOUNT.to_vec(), device: b"web".to_vec(), key: wk, ring };
    let mut web = Notes::open(ORIGIN, me, "Phone", &[], 99).unwrap();
    desk.session.connected();
    web.session.connected();
    for c in [1, 2] {
        for o in hub.connect(c, &mut rng) {
            if let Out::Send(_, b) = o {
                if c == 1 {
                    desk.session.receive(&b, &mut DeskDocs(&mut desk.docs), &mut rng);
                } else {
                    web.receive(&b, &mut rng);
                }
            }
        }
    }
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(web.notes().len(), 4);

    let found = |q: &str| {
        let mut hits = web.search(q);
        hits.sort();
        hits
    };
    let one = |k: Key| vec![k.to_string()];
    // The title, any case, whole or in part.
    assert_eq!(found("Weekend"), one(weekend));
    assert_eq!(found("weekEND"), one(weekend));
    assert_eq!(found("Item 22"), one(item));
    assert_eq!(found("acme"), one(heading));
    // The words still.
    assert_eq!(found("HILL AT"), one(weekend));
    assert_eq!(found("quarterly"), one(heading));
    // Not by the word the library shows for an untitled note, nor by a
    // folder's name: the desktop finds neither.
    assert!(found("untitled").is_empty());
    assert!(found("Many").is_empty());
}

/// What a browser kept opens in the builds after it: the bytes every
/// build wrote before state had layouts (`CRS1`, then the cursor and each
/// room and doc, each after its length) still read. A newer build's are
/// refused, not started again over; bytes that don't read start again.
#[test]
fn kept_state_opens_in_later_builds_and_not_in_earlier_ones() {
    let mut rng = Seeded(6);
    let ring = Ring::new(VaultKey::generate(&mut rng));
    let me = Me { account: ACCOUNT.to_vec(), device: b"web".to_vec(), key: DeviceKey::generate(&mut rng), ring };
    let doc = NoteDoc::new(1);
    doc.record("# Kept\n", Origin::Save);
    let k = Key::random().to_string();
    let field = |b: &[u8]| [&(b.len() as u32).to_be_bytes()[..], b].concat();
    let old = [b"CRS1".as_slice(), &field(&42u64.to_be_bytes()), &field(k.as_bytes()), &field(&doc.to_bytes())].concat();
    let web = Notes::open(ORIGIN, me.clone(), "Phone", &old, 99).unwrap();
    assert_eq!(web.session.cursor(), 42);
    assert_eq!(web.rooms.docs[&k].text(), "# Kept\n");
    let saved = web.save();
    assert_eq!(&saved[..4], b"CRS1", "layout 1 is written as it always was");
    assert_eq!(carnet_web::state_refusal(&saved), "");

    let mut newer = saved.clone();
    newer[3] += 1;
    assert_eq!(Notes::open(ORIGIN, me.clone(), "Phone", &newer, 99).err(), Some(Newer(STATE + 1)));
    assert!(carnet_web::state_refusal(&newer).contains("newer Carnet"));

    for junk in [&b""[..], b"not state", b"CRS"] {
        let web = Notes::open(ORIGIN, me.clone(), "Phone", junk, 99).unwrap();
        assert_eq!((web.session.cursor(), web.rooms.docs.len()), (0, 0), "started again");
        assert_eq!(carnet_web::state_refusal(junk), "");
    }
}

#[test]
fn the_browser_edits_merges_renames_makes_and_deletes_notes() {
    let mut rng = Seeded(5);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let k = Key::random();
    let ks = k.to_string();
    desk.docs.insert(ks.clone(), NoteDoc::new(7));
    desk.docs[&ks].record("# Plan\n\nA walk.\n", Origin::Save);
    let v = desk.docs.entry(vault::ROOM.to_string()).or_insert_with(|| NoteDoc::new(7));
    v.set_place(k, &Place { folder: String::new(), name: "Plan".into(), pinned: false, trashed: false });
    for room in [vault::ROOM.to_string(), ks.clone()] {
        desk.session.saved(&room, &mut DeskDocs(&mut desk.docs), &mut rng);
    }
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let place = |desk: &Desk, k: Key| desk.docs[vault::ROOM].place(k);

    // Typed here: the desktop has it.
    let (text, base) = web.source(&ks).unwrap();
    assert_eq!(text, "# Plan\n\nA walk.\n");
    let (merged, base) = web.edit(&ks, &base, "# Plan\n\nA walk. Water.\n", &mut rng).unwrap();
    assert_eq!(merged, "# Plan\n\nA walk. Water.\n");
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(desk.docs[&ks].text(), merged);

    // Typed here while the desktop's change was arriving: both stay, and
    // the editor gets them merged.
    desk.docs[&ks].record("# Plan\n\nSunday: a walk. Water.\n", Origin::Save);
    desk.session.saved(&ks, &mut DeskDocs(&mut desk.docs), &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let (merged, _) = web.edit(&ks, &base, "# Plan\n\nA walk. Water. Bread.\n", &mut rng).unwrap();
    assert_eq!(merged, "# Plan\n\nSunday: a walk. Water. Bread.\n");
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(desk.docs[&ks].text(), merged);

    // A new title renames it everywhere; other words don't.
    let (_, base) = web.source(&ks).unwrap();
    web.edit(&ks, &base, "# Plan: Sunday\n\nSunday: a walk. Water. Bread.\n", &mut rng).unwrap();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(place(&desk, k).unwrap().name, "Plan- Sunday");

    // A note made here: nothing goes until it has words, then it's in its
    // folder with its title's name.
    let n = web.create("Home");
    assert!(web.notes().iter().all(|l| l.key != n), "blank: not listed");
    let (_, base) = web.source(&n).unwrap();
    let (_, base) = web.edit(&n, &base, "# ", &mut rng).unwrap();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(place(&desk, Key::parse(&n).unwrap()).is_none(), "still nothing sent");
    web.edit(&n, &base, "# Bread\n\nrye\n", &mut rng).unwrap();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let p = place(&desk, Key::parse(&n).unwrap()).expect("placed");
    assert_eq!((p.folder.as_str(), p.name.as_str()), ("Home", "Bread"));
    assert_eq!(desk.docs[&n].text(), "# Bread\n\nrye\n");
    assert!(!web.discard(&n), "written: not discarded");

    // One left blank is dropped, never sent.
    let blank = web.create("");
    assert!(web.discard(&blank));
    assert!(web.source(&blank).is_none());

    // Deleted here, put back: the desktop's vault follows.
    assert!(web.set_trashed(&ks, true, &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(place(&desk, k).unwrap().trashed);
    assert!(web.set_trashed(&ks, false, &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(!place(&desk, k).unwrap().trashed);
}

#[test]
fn a_picture_put_in_here_goes_to_the_server_then_the_vault() {
    let mut rng = Seeded(6);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let bytes: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let path = web.picture_free("pasted-2026-10-10-120000", "jpg");
    assert_eq!(path, "attachments/pasted-2026-10-10-120000.jpg");
    let id = web.upload(&path, bytes.clone()).expect("not too big");
    assert_eq!(web.picture_free("pasted-2026-10-10-120000", "jpg"), "attachments/pasted-2026-10-10-120000-2.jpg", "taken while it goes");
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.said.contains(&Said::Uploaded(id.clone())), "{:?}", web.said);
    // In the vault, for every device; the desktop fetches it whole.
    assert_eq!(desk.docs[vault::ROOM].pictures().get(&path), Some(&id));
    let blob: [u8; 32] = (0..32).map(|i| u8::from_str_radix(&id[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap();
    desk.session.get_blob(blob);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let got = desk.session.take_events().into_iter().find_map(|e| if let Event::BlobGot(_, b) = e { Some(b) } else { None });
    assert_eq!(got.as_deref(), Some(bytes.as_slice()));
    // Put in while not connected: offered once logged in.
    let mut path = String::new();
    let (mut hub, mut desk, mut offline) = two_with(&mut rng, |web| {
        path = web.picture_free("p", "png");
        web.upload(&path, vec![1, 2, 3]).unwrap();
    });
    pump(&mut hub, &mut desk, &mut offline, &mut rng);
    assert!(desk.docs[vault::ROOM].pictures().contains_key(&path));
    assert!(offline.said.iter().any(|s| matches!(s, Said::Uploaded(_))), "{:?}", offline.said);
}

/// A note in Deleted, deleted forever here: gone from the desktop and the
/// server, and the picture only it showed dropped (one another note shows
/// stays). Deleted forever on the desktop: gone here. Done offline, it's
/// kept across a reload, to say once in.
#[test]
fn a_note_deleted_forever_goes_both_ways() {
    let mut rng = Seeded(8);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let only = web.picture_free("only", "png");
    let shared = web.picture_free("shared", "png");
    let only_id = web.upload(&only, vec![1; 2000]).unwrap();
    let shared_id = web.upload(&shared, vec![2; 2000]).unwrap();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let write = |web: &mut Notes, rng: &mut Seeded, text: &str| {
        let n = web.create("");
        let (_, base) = web.source(&n).unwrap();
        web.edit(&n, &base, text, rng).unwrap();
        n
    };
    let gone = write(&mut web, &mut rng, &format!("# Trip\n\n![]({only})\n\n![]({shared})\n"));
    let album = write(&mut web, &mut rng, &format!("# Album\n\n![]({shared})\n"));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(!web.delete_forever(&gone, &mut rng), "not in Deleted: not deleted forever");
    assert!(web.set_trashed(&gone, true, &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);

    assert!(web.delete_forever(&gone, &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.notes().iter().all(|l| l.key != gone) && web.source(&gone).is_none());
    let k = Key::parse(&gone).unwrap();
    assert!(desk.docs[vault::ROOM].gone().contains(&k));
    assert!(hub.store().is_removed(ACCOUNT, &gone) && hub.store().spans(ACCOUNT, &gone).is_empty());
    assert!(desk.docs[vault::ROOM].dropped_pictures().contains(&only));
    assert!(desk.docs[vault::ROOM].pictures().contains_key(&shared), "Album still shows it");
    let unhex = |h: &str| -> [u8; 32] { (0..32).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap() };
    WORKER.with(|w| {
        let w = w.borrow();
        assert!(w.blobs.is_marked(ACCOUNT, &unhex(&only_id)) && !w.blobs.is_marked(ACCOUNT, &unhex(&shared_id)));
    });
    assert!(web.notes().iter().any(|l| l.key == album));

    // Deleted forever on the desktop: gone here too.
    let ka = Key::parse(&album).unwrap();
    desk.docs[vault::ROOM].set_gone(ka);
    desk.session.saved(vault::ROOM, &mut DeskDocs(&mut desk.docs), &mut rng);
    desk.session.remove_room(&album);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.notes().iter().all(|l| l.key != album) && web.source(&album).is_none());

    // Offline: kept in what's saved, and said again after a reload.
    let last = write(&mut web, &mut rng, "# Last\n\nwords\n");
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    web.set_trashed(&last, true, &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    web.session.disconnected();
    assert!(web.delete_forever(&last, &mut rng));
    let me = Me { account: web.me.account.clone(), device: web.me.device.clone(), key: web.me.key.clone(), ring: web.me.ring.clone() };
    let again = Notes::open(ORIGIN, me, "Phone", &web.save(), 99).unwrap();
    assert!(again.session.pending_removals().0.contains(&last));
}

/// The browser too: a picture an old version of another note shows stays
/// when the note showing it now is deleted forever; one dropped comes back
/// by the id it had when a note shows it again.
#[test]
fn a_picture_an_old_version_shows_stays_in_the_browser_too() {
    let mut rng = Seeded(9);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let pic = web.picture_free("p", "png");
    let id = web.upload(&pic, vec![3; 3000]).unwrap();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let write = |web: &mut Notes, rng: &mut Seeded, text: &str| {
        let n = web.create("");
        let (_, base) = web.source(&n).unwrap();
        web.edit(&n, &base, text, rng).unwrap();
        n
    };
    let gone = write(&mut web, &mut rng, &format!("# Trip\n\n![]({pic})\n"));
    let diary = write(&mut web, &mut rng, &format!("# Diary\n\n![]({pic})\n"));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    // Taken out on the desktop later: Diary's first version still shows it.
    desk.docs.get(&diary).unwrap().record("# Diary\n\nno picture now\n", Origin::Save);
    desk.session.saved(&diary, &mut DeskDocs(&mut desk.docs), &mut rng);
    web.set_trashed(&gone, true, &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(web.source(&diary).unwrap().0, "# Diary\n\nno picture now\n");
    assert!(web.delete_forever(&gone, &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(desk.docs[vault::ROOM].pictures().get(&pic), Some(&id), "Diary's old version shows it: kept");

    // Dropped anyway: asked for by its old id, it's named again.
    web.rooms.doc(vault::ROOM).drop_picture(&pic);
    assert!(web.picture_id(&pic).is_none());
    assert_eq!(web.get_picture(&pic).as_deref(), Some(id.as_str()));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(web.picture_id(&pic).as_deref(), Some(id.as_str()), "named again");
    assert!(web.pictures.iter().any(|(_, b)| b == &vec![3; 3000]));
}

/// A picture the vault names that the server hasn't got (a device named
/// it by an id from another account, say): asked for, it's said not to
/// have come, so the page can say so rather than wait for ever; once a
/// device has put it, asked again, it comes.
#[test]
fn a_picture_the_server_lacks_is_said_missing_then_comes() {
    let mut rng = Seeded(12);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let bytes = vec![4u8; 2000];
    let id = desk.session.blob_id(&bytes);
    let hexed: String = id.iter().map(|b| format!("{b:02x}")).collect();
    desk.docs.entry(vault::ROOM.into()).or_insert_with(|| NoteDoc::new(7)).set_picture("Home/hill.jpg", &hexed);
    desk.session.saved(vault::ROOM, &mut DeskDocs(&mut desk.docs), &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(web.picture_id("Home/hill.jpg").as_deref(), Some(hexed.as_str()));
    assert_eq!(web.get_picture("Home/hill.jpg").as_deref(), Some(hexed.as_str()));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.pictures.is_empty());
    assert_eq!(std::mem::take(&mut web.missing), vec![id], "said not to have come");

    // The desktop puts it; asked again, it comes.
    desk.session.offer_blobs(vec![id]);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    for e in desk.session.take_events() {
        if e == Event::BlobWanted(id) {
            desk.session.put_blob(&bytes, &mut rng);
        }
    }
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    web.get_picture("Home/hill.jpg");
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.missing.is_empty());
    assert!(web.pictures.iter().any(|(i, b)| *i == id && *b == bytes));
}

/// Not a test: a browser's state with 10,000 notes, written where
/// `CARNET_BIG` says, to load into the viewer and time it (docs/SYNC.md,
/// phase 3). `CARNET_BIG=/tmp/big cargo test -p carnet-web --test viewer
/// -- --ignored ten_thousand`.
#[test]
#[ignore]
fn ten_thousand_notes_state() {
    let Ok(out) = std::env::var("CARNET_BIG") else { return };
    let mut rng = Seeded(4);
    let ring = Ring::new(VaultKey::generate(&mut rng));
    let me = Me { account: b"big".to_vec(), device: b"web".to_vec(), key: DeviceKey::generate(&mut rng), ring };
    let web = Notes::open(ORIGIN, me, "Phone", &[], 99).unwrap();
    let mut web = web;
    let v = NoteDoc::new(99);
    let words = ["ferry", "pier", "lisbon", "milk", "bread", "plan", "garden", "letter", "window", "morning", "train", "budget"];
    for i in 0..10_000usize {
        let k = Key::random();
        let d = NoteDoc::new(99);
        let mut text = format!("# Note {i} about {}\n\n", words[i % words.len()]);
        for p in 0..(1 + i % 7) {
            for w in 0..(20 + (i * 7 + p) % 60) {
                text.push_str(words[(i + p + w) % words.len()]);
                text.push(' ');
            }
            text.push_str("\n\n");
        }
        if i % 5 == 0 {
            for (t, w) in words.iter().take(6).enumerate() {
                text.push_str(&format!("- [{}] {w} {t}\n", if t % 2 == 0 { "x" } else { " " }));
            }
        }
        d.record(&text, Origin::Save);
        // A few edits each, as a note that lived a while.
        d.record(&text.replace("ferry", "boat"), Origin::Save);
        let folder = if i % 3 == 0 { String::new() } else { format!("Folder {}", i % 40) };
        if !folder.is_empty() {
            v.set_folder(&folder, true);
        }
        v.set_place(k, &Place { folder, name: format!("Note {i}"), pinned: i % 500 == 0, trashed: i % 997 == 0 });
        web.rooms.docs.insert(k.to_string(), d);
    }
    web.rooms.docs.insert(vault::ROOM.to_string(), v);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(format!("{out}/identity"), web.me.to_bytes()).unwrap();
    let saved = web.save();
    std::fs::write(format!("{out}/state"), &saved).unwrap();
    let t = std::time::Instant::now();
    let again = Notes::open(ORIGIN, Me::from_bytes(&web.me.to_bytes()).unwrap(), "Phone", &saved, 100).unwrap();
    let opened = t.elapsed();
    let t = std::time::Instant::now();
    let n = again.notes().len();
    let listed = t.elapsed();
    let t = std::time::Instant::now();
    let hits = again.search("lisbon").len();
    let searched = t.elapsed();
    let mut again = again;
    let k = again.notes().iter().find(|n| n.todos > 0).map(|n| n.key.clone()).unwrap();
    again.toggle_task(&k, 1, &mut rng);
    let t = std::time::Instant::now();
    let _ = again.notes();
    let relisted = t.elapsed();
    let t = std::time::Instant::now();
    let _ = again.save();
    eprintln!(
        "state {} MB; natively: open {opened:?}, list {n} in {listed:?} (after a tick: {relisted:?}), search ({hits}) {searched:?}, save after a tick {:?}",
        saved.len() / 1_000_000,
        t.elapsed()
    );
}
