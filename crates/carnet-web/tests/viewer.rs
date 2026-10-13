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
    /// The pictures the browser has put on the server, counted.
    static WEB_PUTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How the browser's puts fail, while they do: as the page says it.
    static WEB_PUT_FAILS: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// Everything delivered, both ways, until nothing moves.
fn pump(hub: &mut Hub<Memory>, desk: &mut Desk, web: &mut Notes, rng: &mut Seeded) {
    pump_with(hub, desk, web, &mut None, rng);
}

/// The same, with a new device joining on connection 3.
fn pump_with(hub: &mut Hub<Memory>, desk: &mut Desk, web: &mut Notes, joiner: &mut Option<Joiner>, rng: &mut Seeded) {
    deliver(hub, desk, web, joiner, rng);
    // As the page does after frames: what changed taken, the notes
    // deleted forever elsewhere forgotten with it.
    web.take_changed();
}

/// Everything delivered, both ways, until nothing moves; the page not
/// yet told.
fn deliver(hub: &mut Hub<Memory>, desk: &mut Desk, web: &mut Notes, joiner: &mut Option<Joiner>, rng: &mut Seeded) {
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
            WEB_PUTS.with(|p| p.set(p.get() + usize::from(put)));
            let result = match WEB_PUT_FAILS.get() {
                Some(why) if put => Err(why.to_string()),
                _ => serve(&t),
            };
            web.transferred(t.id(), put, result, rng);
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
/// and takes it back. The code is kept in the vault: the desktop has it,
/// the browser shows it again after a reload, and one the desktop made
/// shows its code here too. Taken back, it's gone from the vault.
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
    let hash = waiting[0].0.hash;
    let desk_code = |desk: &Desk, hash: &[u8; 32]| desk.docs[vault::ROOM].invite_code(hash).map(|c| c.text());
    assert_eq!(desk_code(&desk, &hash), Some(code.clone()), "in the vault, for the desktop too");

    // Reloaded: listed again, with its code.
    let mut web = Notes::open(ORIGIN, web.me.clone(), "Phone", &web.save(), 99).unwrap();
    web.session.connected();
    for o in hub.connect(2, &mut rng) {
        if let Out::Send(_, b) = o {
            web.receive(&b, &mut rng);
        }
    }
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    web.list_invites();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let (_, waiting, _) = invites(&mut web).expect("listed after a reload");
    assert_eq!(waiting.iter().map(|(_, c)| c.clone()).collect::<Vec<_>>(), [Some(code.clone())]);

    // One the desktop made: its code shown here too.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let theirs = desk.session.invite(&mut rng).expect("connected");
    assert!(desk.docs[vault::ROOM].keep_invite(&theirs, now));
    desk.session.saved(vault::ROOM, &mut DeskDocs(&mut desk.docs), &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    web.list_invites();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let (_, waiting, _) = invites(&mut web).expect("both");
    let mut codes: Vec<_> = waiting.iter().map(|(_, c)| c.clone().expect("each with its code")).collect();
    codes.sort();
    let mut both = vec![code.clone(), theirs.text()];
    both.sort();
    assert_eq!(codes, both);

    // Taken back here: gone from the vault, the desktop's too.
    web.drop_invite(hash, &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let (_, waiting, _) = invites(&mut web).expect("and again");
    assert_eq!(waiting.iter().map(|(i, c)| (i.hash, c.clone())).collect::<Vec<_>>(), [(theirs.hash(), Some(theirs.text()))]);
    assert_eq!(desk_code(&desk, &hash), None);
}

/// A device removed had every waiting invite's code (they're in the
/// vault): the browser that removes it takes each back, on the server
/// and in the vault, and says so.
#[test]
fn removing_a_device_takes_back_the_invites_it_knew() {
    let mut rng = Seeded(6);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let code = web.make_invite(&mut rng).expect("connected");
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let hash = carnet_sync::invite::InviteCode::parse(&code).unwrap().hash();
    assert!(desk.docs[vault::ROOM].invite_code(&hash).is_some(), "the desktop has the code");
    assert_eq!(hub.store().invites().len(), 1);

    web.list_devices();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(web.remove_device(b"desk", &mut rng), Ok(true), "said: invites taken back");
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.errors.is_empty(), "{:?}", web.errors);
    assert!(desk.session.take_events().iter().any(|e| matches!(e, Event::Removed { account: false })), "the desktop is out");
    assert!(hub.store().invites().is_empty(), "taken back on the server");
    assert!(web.rooms.doc(vault::ROOM).invites().is_empty(), "and forgotten in the vault");
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
    // that has it; one on the web, that it isn't loaded.
    assert!(h.contains("data-picture=\"/Users/me/Pictures/sea.png\""), "{h}");
    assert!(h.contains("data-picture=\"https://example.com/a.png\""), "{h}");
    // None has a `src`: the browser asks for none before the page has it,
    // and never for one on the web.
    assert_eq!(h.matches("src=").count(), 0, "{h}");
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
    web.take_changed();
    deliver(&mut hub, &mut desk, &mut web, &mut None, &mut rng);
    assert!(web.said.contains(&Said::Uploaded(id.clone())), "{:?}", web.said);
    // The page told the vault changed: it names the picture now, so a
    // note showing "not on the server yet" in its place asks again.
    let changed = web.take_changed();
    assert!(changed.iter().any(|r| r == vault::ROOM), "{changed:?}");
    assert_eq!(web.picture_id(&path).as_deref(), Some(id.as_str()));
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

/// The same picture put in twice, under two names (pasted twice): sent
/// once, and both names in the vault, so the desktop has a file at each.
/// Put in again once it's on the server: that name too, nothing sent.
#[test]
fn the_same_picture_put_in_twice_is_named_at_both() {
    let mut rng = Seeded(16);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let bytes: Vec<u8> = (0..50_000u32).map(|i| (i * 13 % 241) as u8).collect();
    let first = web.picture_free("pasted-2026-10-12-015034", "png");
    let id = web.upload(&first, bytes.clone()).unwrap();
    let second = web.picture_free("pasted-2026-10-12-015102", "png");
    assert_eq!(web.upload(&second, bytes.clone()).as_deref(), Some(id.as_str()), "one picture, one id");
    assert_eq!(web.picture_free("pasted-2026-10-12-015034", "png"), "attachments/pasted-2026-10-12-015034-2.png", "both taken while they go");
    assert_eq!(web.picture_free("pasted-2026-10-12-015102", "png"), "attachments/pasted-2026-10-12-015102-2.png");
    let puts = WEB_PUTS.with(|p| p.get());
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(WEB_PUTS.with(|p| p.get()) - puts, 1, "sent once");
    assert_eq!(web.said.iter().filter(|s| **s == Said::Uploaded(id.clone())).count(), 1, "{:?}", web.said);
    let pictures = desk.docs[vault::ROOM].pictures();
    assert_eq!(pictures.get(&first), Some(&id), "the first name too");
    assert_eq!(pictures.get(&second), Some(&id));
    // What the desktop fetches for either name: the picture.
    let blob: [u8; 32] = (0..32).map(|i| u8::from_str_radix(&id[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap();
    desk.session.get_blob(blob);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let got = desk.session.take_events().into_iter().find_map(|e| if let Event::BlobGot(_, b) = e { Some(b) } else { None });
    assert_eq!(got.as_deref(), Some(bytes.as_slice()));

    // Once more, now the server has it: named, and nothing sent.
    let third = web.picture_free("pasted-2026-10-12-015200", "png");
    web.upload(&third, bytes.clone()).unwrap();
    let puts = WEB_PUTS.with(|p| p.get());
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(WEB_PUTS.with(|p| p.get()), puts, "the server has it");
    let pictures = desk.docs[vault::ROOM].pictures();
    assert!([&first, &second, &third].iter().all(|p| pictures.get(*p) == Some(&id)), "{pictures:?}");
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
    deliver(&mut hub, &mut desk, &mut web, &mut None, &mut rng);
    // Not listed, nor written to, even before the page lists the notes
    // again (which forgets it).
    assert!(web.notes().iter().all(|l| l.key != album));
    let (_, base) = web.source(&album).unwrap();
    assert!(web.edit(&album, &base, "# Album\n\nmore\n", &mut rng).is_none());
    assert!(web.source(&album).is_none());
    web.take_changed();

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
    desk.session.offer_blobs(vec![(id, bytes.len())]);
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
/// A plan the test changes as it goes.
struct Plan(std::sync::Mutex<carnet_server::quota::Limit>);

impl carnet_server::quota::Quota for Plan {
    fn limit(&self, _: &[u8]) -> carnet_server::quota::Limit {
        self.0.lock().unwrap().clone()
    }
}

/// The account suspended and the suspension lifted while the page is
/// open: it's told each at once (the host calls `limit_changed`), says
/// read only, then takes the words back; what was typed meanwhile goes
/// to the desktop without a reload; the plan's line is the new one.
#[test]
fn an_open_browser_learns_its_account_can_write_again() {
    use carnet_server::quota::Limit;
    let mut rng = Seeded(11);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let plan = std::sync::Arc::new(Plan(std::sync::Mutex::new(Limit::default())));
    hub.set_quota(plan.clone());
    let k = Key::random();
    let ks = k.to_string();
    desk.docs.insert(ks.clone(), NoteDoc::new(7));
    desk.docs[&ks].record("# Plan\n\nA walk.\n", Origin::Save);
    desk.session.saved(&ks, &mut DeskDocs(&mut desk.docs), &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    web.said.clear();

    // The page's socket is told as the host says: through the worker.
    let tell = |hub: &mut Hub<Memory>, desk: &mut Desk, web: &mut Notes, rng: &mut Seeded| {
        let outs = WORKER.with(|w| w.borrow_mut().done(hub.limit_changed(ACCOUNT)));
        for o in outs {
            if let Out::Send(conn, bytes) = o {
                match conn {
                    1 => desk.session.receive(&bytes, &mut DeskDocs(&mut desk.docs), rng),
                    _ => web.receive(&bytes, rng),
                }
            }
        }
        pump(hub, desk, web, rng);
    };
    plan.0.lock().unwrap().writable = false;
    tell(&mut hub, &mut desk, &mut web, &mut rng);
    // Said once, for the status line and the card: not as an error too.
    assert_eq!(web.said.iter().filter(|s| **s == Said::ReadOnly).count(), 1, "{:?}", web.said);
    assert!(web.errors.is_empty(), "{:?}", web.errors);
    let (_, base) = web.source(&ks).unwrap();
    web.edit(&ks, &base, "# Plan\n\nA walk. Typed while suspended.\n", &mut rng).unwrap();
    // A picture put in meanwhile: held back, and that's not said again.
    let pic = web.picture_free("suspended", "png");
    web.upload(&pic, vec![5; 1500]).unwrap();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(!desk.docs[&ks].text().contains("suspended"), "nothing sent");
    assert!(web.errors.is_empty(), "{:?}", web.errors);

    *plan.0.lock().unwrap() = Limit { bytes: Some(5_000_000), writable: true, plan: "Personal".into(), until: 0 };
    tell(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.said.contains(&Said::Writable), "{:?}", web.said);
    assert!(web.said.iter().any(|s| matches!(s, Said::Usage(u) if u.writable && u.plan == "Personal")));
    assert!(desk.docs[&ks].text().contains("Typed while suspended."), "what waited went");
    assert!(desk.docs[vault::ROOM].pictures().contains_key(&pic), "the picture too, without a reload");
}

/// A picture put in while the account is over its plan is held back (the
/// server says it's full); the plan raised while the page is open, it
/// goes at once, with no reload, and reaches the vault.
#[test]
fn a_picture_held_back_over_the_plan_goes_once_the_plan_is_raised() {
    use carnet_server::quota::Limit;
    let mut rng = Seeded(13);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    // A plan the notes alone fill.
    let plan = std::sync::Arc::new(Plan(std::sync::Mutex::new(Limit { bytes: Some(1), writable: true, plan: "Tiny".into(), until: 0 })));
    hub.set_quota(plan.clone());
    let k = Key::random();
    let ks = k.to_string();
    desk.docs.insert(ks.clone(), NoteDoc::new(7));
    desk.docs[&ks].record("# Plan\n\nA walk.\n", Origin::Save);
    desk.session.saved(&ks, &mut DeskDocs(&mut desk.docs), &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let pic = web.picture_free("over", "png");
    let id = web.upload(&pic, vec![6; 2500]).unwrap();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(web.errors, [carnet_sync::session::FULL.to_string()], "said full");
    assert!(desk.docs.get(vault::ROOM).is_none_or(|v| !v.pictures().contains_key(&pic)), "held back");
    assert_eq!(web.pictures_waiting(), 1, "waiting, not up to date");
    web.said.clear();

    *plan.0.lock().unwrap() = Limit { bytes: Some(5_000_000), writable: true, plan: "Personal".into(), until: 0 };
    let outs = WORKER.with(|w| w.borrow_mut().done(hub.limit_changed(ACCOUNT)));
    for o in outs {
        if let Out::Send(conn, bytes) = o {
            match conn {
                1 => desk.session.receive(&bytes, &mut DeskDocs(&mut desk.docs), &mut rng),
                _ => web.receive(&bytes, &mut rng),
            }
        }
    }
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.said.contains(&Said::Uploaded(id.clone())), "{:?}", web.said);
    assert_eq!(desk.docs[vault::ROOM].pictures().get(&pic), Some(&id));
    assert_eq!(web.pictures_waiting(), 0, "none waiting once it's there");
}

/// A picture the server's storage won't take from here: why is said (not
/// as an error), it waits, and it's tried again, going once it can. One
/// that fails a while (busy, offline) waits saying nothing.
#[test]
fn a_picture_the_storage_wont_take_says_why_and_waits() {
    use carnet_web::notes::{TURNED_AWAY, UNREACHABLE};
    let mut rng = Seeded(21);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let pic = web.picture_free("refused", "png");
    let id = web.upload(&pic, vec![8; 900]).unwrap();
    // Busy: nothing said.
    WEB_PUT_FAILS.set(Some("HTTP 503"));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!((web.pictures_waiting(), web.pictures_held()), (1, None));
    // A store without CORS (no answer while the server is up), then one
    // that refuses: said, each in its words, and never as an error.
    for (fails, said) in [(UNREACHABLE, carnet_sync::session::PICTURES_UNREACHABLE), ("HTTP 403", TURNED_AWAY)] {
        WEB_PUT_FAILS.set(Some(fails));
        web.retry_pictures();
        pump(&mut hub, &mut desk, &mut web, &mut rng);
        assert_eq!((web.pictures_waiting(), web.pictures_held()), (1, Some(said)));
    }
    // A passing failure after: what was said stays.
    WEB_PUT_FAILS.set(Some("timed out"));
    web.retry_pictures();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(web.pictures_held(), Some(TURNED_AWAY));
    // Mended: it goes, and nothing is said.
    WEB_PUT_FAILS.set(None);
    web.retry_pictures();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.said.contains(&Said::Uploaded(id.clone())), "{:?}", web.said);
    assert_eq!((web.pictures_waiting(), web.pictures_held()), (0, None));
    assert_eq!(desk.docs[vault::ROOM].pictures().get(&pic), Some(&id));

    // A link to put at that isn't followed (a host of this network not
    // the server's): said, and the picture offered again when retried.
    let pic = web.picture_free("inner", "png");
    let id = web.upload(&pic, vec![9; 900]).unwrap();
    let blob: [u8; 32] = (0..32).map(|i| u8::from_str_radix(&id[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap();
    let link = carnet_sync::wire::Frame::Carnet(carnet_sync::wire::Carnet::BlobLink { id: blob, put: true, url: "http://192.168.1.20:9000/b/x".into(), expires: 0 });
    web.receive(&carnet_sync::wire::encode(&link), &mut rng);
    assert_eq!(web.pictures_held(), Some(carnet_sync::session::PICTURES_UNREACHABLE));
    assert!(web.errors.is_empty(), "{:?}", web.errors);
    web.retry_pictures();
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.said.contains(&Said::Uploaded(id.clone())), "offered again, and it went");
    assert_eq!(web.pictures_held(), None);
    assert!(web.errors.is_empty(), "{:?}", web.errors);
}

/// A browser opened on an account that already has many notes (more
/// than the desktop's replica keeps in memory) lists every one.
#[test]
fn a_browser_new_to_an_account_gets_every_note() {
    let mut rng = Seeded(9);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let me = web.me.clone();
    web.session.disconnected();
    let _ = hub.disconnect(2);
    let mut rooms = vec![vault::ROOM.to_string()];
    desk.docs.entry(vault::ROOM.to_string()).or_insert_with(|| NoteDoc::new(7));
    for i in 0..70 {
        let k = Key::random();
        let d = NoteDoc::new(7);
        d.record(&format!("# Note {i}\n\nWords {i}.\n"), Origin::Save);
        desk.docs[vault::ROOM].set_place(k, &Place { folder: String::new(), name: format!("Note {i}"), pinned: false, trashed: false });
        desk.docs.insert(k.to_string(), d);
        rooms.push(k.to_string());
    }
    for room in &rooms {
        desk.session.saved(room, &mut DeskDocs(&mut desk.docs), &mut rng);
    }
    pump(&mut hub, &mut desk, &mut web, &mut rng);

    // A browser with nothing kept, this account's.
    let mut fresh = Notes::open(ORIGIN, me, "Phone", &[], 98).unwrap();
    fresh.session.connected();
    for o in hub.connect(2, &mut rng) {
        if let Out::Send(_, b) = o {
            fresh.receive(&b, &mut rng);
        }
    }
    pump(&mut hub, &mut desk, &mut fresh, &mut rng);
    let listed = fresh.notes();
    assert_eq!(listed.len(), 70, "every note");
    assert!(listed.iter().any(|n| n.title == "Note 69"));
}

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

/// What the browser hasn't sent is known, even after a reload, when this
/// session knows nothing yet: what the page kept says, until the server
/// says it keeps it. And it comes out as Markdown files, by folder and
/// title, for a browser removed before it could send them.
#[test]
fn what_is_not_sent_is_known_across_a_reload_and_handed_over() {
    let mut rng = Seeded(9);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let k = Key::random();
    let ks = k.to_string();
    desk.docs.insert(ks.clone(), NoteDoc::new(7));
    desk.docs[&ks].record("# Plan\n\nA walk.\n", Origin::Save);
    let v = desk.docs.entry(vault::ROOM.to_string()).or_insert_with(|| NoteDoc::new(7));
    v.set_place(k, &Place { folder: "Home".into(), name: "Plan".into(), pinned: false, trashed: false });
    for room in [vault::ROOM.to_string(), ks.clone()] {
        desk.session.saved(&room, &mut DeskDocs(&mut desk.docs), &mut rng);
    }
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.unsent(&[]).is_empty(), "all sent");

    // Typed offline: waiting, and its file.
    web.session.disconnected();
    let (_, base) = web.source(&ks).unwrap();
    web.edit(&ks, &base, "# Plan\n\nA walk. Water.\n", &mut rng).unwrap();
    let unsent = web.unsent(&[]);
    assert_eq!(unsent, std::slice::from_ref(&ks));
    assert_eq!(web.note_files(&unsent), [("Home/Plan.md".to_string(), "# Plan\n\nA walk. Water.\n".to_string())]);
    // The library waiting too: every note.
    assert_eq!(web.note_files(&[vault::ROOM.to_string()]).len(), 1);

    // Reloaded: this session knows nothing yet, but the page's list does.
    let again = Notes::open(ORIGIN, web.me.clone(), "Phone", &web.save(), 99).unwrap();
    assert!(again.unsent(&[]).is_empty(), "nothing known by the session alone");
    assert_eq!(again.unsent(&unsent), unsent);
    // Back online and sent: kept, so no longer unsent.
    let mut web = again;
    web.session.connected();
    for o in hub.connect(2, &mut rng) {
        if let Out::Send(_, b) = o {
            web.receive(&b, &mut rng);
        }
    }
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(desk.docs[&ks].text(), "# Plan\n\nA walk. Water.\n");
    assert!(web.unsent(&unsent).is_empty(), "the server keeps it now");
}

/// The desktop's notes, as its session has them: each made, placed, and
/// sent.
fn desk_notes(desk: &mut Desk, rng: &mut Seeded, folders: &[&str], notes: &[(&str, &str, &str)]) -> Vec<Key> {
    let mut keys = Vec::new();
    let v = desk.docs.entry(vault::ROOM.to_string()).or_insert_with(|| NoteDoc::new(7));
    for f in folders {
        v.set_folder(f, true);
    }
    for (folder, name, text) in notes {
        let k = Key::random();
        desk.docs[vault::ROOM].set_place(k, &Place { folder: folder.to_string(), name: name.to_string(), pinned: false, trashed: false });
        desk.docs.insert(k.to_string(), NoteDoc::new(7));
        desk.docs[&k.to_string()].record(text, Origin::Save);
        keys.push(k);
    }
    let rooms: Vec<String> = std::iter::once(vault::ROOM.to_string()).chain(keys.iter().map(Key::to_string)).collect();
    for room in rooms {
        desk.session.saved(&room, &mut DeskDocs(&mut desk.docs), rng);
    }
    keys
}

/// Folders made, renamed, nested and deleted in the browser, notes moved
/// into them and pinned: each reaches the desktop through the vault, as
/// the desktop itself would have written it. A folder never goes into
/// itself; one deleted with notes in it sends them to Deleted, as the
/// desktop's trash takes a folder whole, and comes back with them.
#[test]
fn the_browser_makes_renames_nests_and_deletes_folders() {
    let mut rng = Seeded(11);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let ks = desk_notes(
        &mut desk,
        &mut rng,
        &["Work", "Work/Q4"],
        &[("Work", "Plan", "# Plan\n\nsoon\n"), ("Work/Q4", "Q4", "# Q4\n\n![](../../attachments/p.png)\n"), ("", "Loose", "# Loose\n\nends\n")],
    );
    let (plan, q4, loose) = (ks[0], ks[1], ks[2]);
    desk.docs[vault::ROOM].set_place(plan, &Place { folder: "Work".into(), name: "Plan".into(), pinned: true, trashed: false });
    desk.session.saved(vault::ROOM, &mut DeskDocs(&mut desk.docs), &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(web.folders(), ["Work", "Work/Q4"]);
    let on_desk = |desk: &Desk| desk.docs[vault::ROOM].folders().into_iter().collect::<Vec<_>>();
    let place = |desk: &Desk, k: Key| desk.docs[vault::ROOM].place(k).unwrap();

    // Made, at the top and inside another; the pictures' name refused.
    assert_eq!(web.make_folder("", "Ideas", &mut rng).as_deref(), Ok("Ideas"));
    assert_eq!(web.make_folder("Ideas", "Later: maybe", &mut rng).as_deref(), Ok("Ideas/Later- maybe"));
    assert_eq!(web.make_folder("Ideas", "attachments", &mut rng), Err("that name is kept for pictures; choose another".to_string()));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(on_desk(&desk), ["Ideas", "Ideas/Later- maybe", "Work", "Work/Q4"]);

    // Renamed: its folder and notes with it, a pin kept.
    assert_eq!(web.rename_folder("Work", "Job", &mut rng).as_deref(), Ok("Job"));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(on_desk(&desk), ["Ideas", "Ideas/Later- maybe", "Job", "Job/Q4"]);
    assert_eq!((place(&desk, plan).folder.as_str(), place(&desk, plan).pinned), ("Job", true));
    assert_eq!(place(&desk, q4).folder, "Job/Q4");
    assert_eq!(desk.docs[&q4.to_string()].text(), "# Q4\n\n![](../../attachments/p.png)\n", "as deep as it was: its picture's path as it was");
    assert_eq!(web.rename_folder("Job", "ideas", &mut rng), Err("there is a folder called \u{201c}ideas\u{201d} there already".to_string()));

    // Never into itself, or a folder inside it: nothing sent.
    let before = desk.docs[vault::ROOM].version();
    assert_eq!(web.move_folder("Job", "Job/Q4", &mut rng), Err("a folder can't go inside itself".to_string()));
    assert_eq!(web.move_folder("Job", "Job", &mut rng), Err("a folder can't go inside itself".to_string()));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(desk.docs[vault::ROOM].version(), before);
    // What the "Move to" menu offers: neither, nor where it is.
    assert_eq!(web.folder_targets("Job"), ["Ideas", "Ideas/Later- maybe"]);
    assert_eq!(web.folder_targets("Job/Q4"), ["", "Ideas", "Ideas/Later- maybe"]);

    // Nested, then to the top: its note's picture still shown from there.
    assert_eq!(web.move_folder("Job/Q4", "Ideas", &mut rng).as_deref(), Ok("Ideas/Q4"));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(place(&desk, q4).folder, "Ideas/Q4");
    assert_eq!(web.move_folder("Ideas/Q4", "", &mut rng).as_deref(), Ok("Q4"));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(on_desk(&desk), ["Ideas", "Ideas/Later- maybe", "Job", "Q4"]);
    assert_eq!(place(&desk, q4).folder, "Q4");
    assert_eq!(desk.docs[&q4.to_string()].text(), "# Q4\n\n![](../attachments/p.png)\n");
    assert_eq!(web.picture_path("Q4", "../attachments/p.png", false).as_deref(), Some("attachments/p.png"));

    // A note moved in, and back out, its picture's path following; pinned.
    assert!(web.move_note(&q4.to_string(), "Ideas/Later- maybe", &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(place(&desk, q4).folder, "Ideas/Later- maybe");
    assert_eq!(desk.docs[&q4.to_string()].text(), "# Q4\n\n![](../../attachments/p.png)\n");
    assert!(web.move_note(&loose.to_string(), "Ideas", &mut rng));
    assert!(!web.move_note(&loose.to_string(), "Ideas", &mut rng), "there already");
    assert!(!web.move_note(&loose.to_string(), "Nowhere", &mut rng), "no such folder");
    assert!(web.set_pinned(&loose.to_string(), true, &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(place(&desk, loose), Place { folder: "Ideas".into(), name: "Loose".into(), pinned: true, trashed: false });
    assert_eq!(desk.docs[&loose.to_string()].text(), "# Loose\n\nends\n", "no picture: its words as they were");
    // Its folder's own notes only, as the desktop counts them.
    let in_ideas = web.notes().iter().filter(|n| !n.trashed && n.folder == "Ideas").count();
    assert_eq!(in_ideas, 1);

    // An empty folder is just deleted.
    assert!(web.folder_is_bare("Q4") && !web.folder_is_bare("Ideas") && !web.folder_is_bare("Job"));
    assert_eq!(web.delete_folder("Q4", &mut rng), (vec!["Q4".to_string()], vec![]));

    // One with notes: to Deleted whole, its notes unpinned, each going
    // back to the folder it was in; then put back.
    let (folders, notes) = web.delete_folder("Ideas", &mut rng);
    assert_eq!(folders, ["Ideas", "Ideas/Later- maybe"]);
    let mut notes: Vec<Key> = notes.iter().map(|k| Key::parse(k).unwrap()).collect();
    notes.sort();
    let mut want = vec![q4, loose];
    want.sort();
    assert_eq!(notes, want);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(on_desk(&desk), ["Job"]);
    assert_eq!(place(&desk, loose), Place { folder: "Ideas".into(), name: "Loose".into(), pinned: false, trashed: true });
    assert!(place(&desk, q4).trashed);
    let keys: Vec<String> = notes.iter().map(Key::to_string).collect();
    assert!(web.put_back_folder(&folders, &keys, &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(on_desk(&desk), ["Ideas", "Ideas/Later- maybe", "Job"]);
    assert!(!place(&desk, loose).trashed && !place(&desk, q4).trashed);

    // A note of a folder deleted, restored alone: its folder comes back.
    web.delete_folder("Job", &mut rng);
    assert!(web.set_trashed(&plan.to_string(), false, &mut rng));
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(on_desk(&desk).contains(&"Job".to_string()));
    assert_eq!(place(&desk, plan), Place { folder: "Job".into(), name: "Plan".into(), pinned: false, trashed: false });
}

/// The other way: what the desktop does to its folders, pins and notes'
/// places shows in the browser's library as it comes.
#[test]
fn the_desktops_folders_and_pins_show_in_the_browser() {
    let mut rng = Seeded(12);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    let ks = desk_notes(&mut desk, &mut rng, &["Home"], &[("Home", "Shop", "# Shop\n\nmilk\n"), ("", "Plan", "# Plan\n\nwalk\n")]);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    let listed = |web: &Notes, k: Key| web.notes().into_iter().find(|n| n.key == k.to_string()).unwrap();

    // Renamed and nested there, as its files were.
    let v = &desk.docs[vault::ROOM];
    v.make_folder("", "Life").unwrap();
    v.rename_folder("Home", "House").unwrap();
    v.move_folder("House", "Life").unwrap();
    v.move_note(ks[1], "Life").unwrap();
    v.set_pinned(ks[1], true);
    desk.session.saved(vault::ROOM, &mut DeskDocs(&mut desk.docs), &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert_eq!(web.folders(), ["Life", "Life/House"]);
    assert_eq!(listed(&web, ks[0]).folder, "Life/House");
    let plan = listed(&web, ks[1]);
    assert_eq!((plan.folder.as_str(), plan.pinned), ("Life", true));
    assert_eq!(web.folder_targets("Life/House"), [""]);

    // Moved to the trash whole there: in Deleted here, the folder gone.
    desk.docs[vault::ROOM].trash_folder("Life");
    desk.session.saved(vault::ROOM, &mut DeskDocs(&mut desk.docs), &mut rng);
    pump(&mut hub, &mut desk, &mut web, &mut rng);
    assert!(web.folders().is_empty());
    assert!(listed(&web, ks[0]).trashed && listed(&web, ks[1]).trashed);
}

/// What the browser did with what came: its time (and of that, taking
/// frames in), the frames, and how many times it listed every note.
#[derive(Default)]
struct PageWork {
    took: std::time::Duration,
    receiving: std::time::Duration,
    lists: usize,
    frames: usize,
    logins: usize,
}

/// When the page lists every note again, after notes came.
#[derive(Clone, Copy, Debug)]
enum Lists {
    /// After each frame, as sync.svelte.ts did.
    EachFrame,
    /// After the frames that came together.
    EachBatch,
    /// At most this often (and once at the end), as sync.svelte.ts does.
    AtMostEvery(std::time::Duration),
}

/// The desktop makes `n` notes, `tick` at a time (each tick's places in
/// the vault, sent after the notes, as its sync tick does), while the
/// browser is in: every frame for it handed over as the hub makes it,
/// the list made as `lists` says.
fn intake(n: usize, tick: usize, lists: Lists) -> (Notes, PageWork) {
    let mut rng = Seeded(21);
    let (mut hub, mut desk, mut web) = two(&mut rng);
    desk.docs.entry(vault::ROOM.to_string()).or_insert_with(|| NoteDoc::new(7));
    desk.docs[vault::ROOM].set_folder("Bulk", true);
    let mut page = PageWork::default();
    let mut listed = std::time::Instant::now();
    let mut made = 0;
    while made < n {
        let mut rooms = Vec::new();
        for _ in 0..tick.min(n - made) {
            let k = Key::random();
            let d = NoteDoc::new(7);
            d.record(&format!("# Bulk {made}\n\nA small note, number {made}.\n"), Origin::Save);
            desk.docs[vault::ROOM].set_place(k, &Place { folder: "Bulk".into(), name: format!("Bulk {made}"), pinned: false, trashed: false });
            desk.docs.insert(k.to_string(), d);
            rooms.push(k.to_string());
            made += 1;
        }
        rooms.push(vault::ROOM.to_string());
        for room in &rooms {
            desk.session.saved(room, &mut DeskDocs(&mut desk.docs), &mut rng);
        }
        // Until nothing moves.
        loop {
            let mut outs: Vec<Out> = Vec::new();
            for b in desk.session.take_out() {
                outs.extend(hub.receive(1, &b, &mut rng));
            }
            for b in web.session.take_out() {
                outs.extend(hub.receive(2, &b, &mut rng));
            }
            if outs.is_empty() {
                break;
            }
            let mut batch = 0;
            for o in outs {
                let Out::Send(conn, bytes) = o else { continue };
                if conn == 1 {
                    desk.session.receive(&bytes, &mut DeskDocs(&mut desk.docs), &mut rng);
                    continue;
                }
                let t = std::time::Instant::now();
                web.receive(&bytes, &mut rng);
                page.receiving += t.elapsed();
                page.frames += 1;
                batch += 1;
                if let Lists::EachFrame = lists {
                    page_after(&mut web, &mut page, true);
                }
                page.took += t.elapsed();
            }
            if batch > 0 {
                let t = std::time::Instant::now();
                match lists {
                    Lists::EachFrame => {}
                    Lists::EachBatch => page_after(&mut web, &mut page, true),
                    Lists::AtMostEvery(gap) => {
                        let due = listed.elapsed() >= gap;
                        page_after(&mut web, &mut page, due);
                        if due {
                            listed = std::time::Instant::now();
                        }
                    }
                }
                page.took += t.elapsed();
            }
            // More new notes than a login takes: in again, as the page
            // connects again, for the rest.
            if std::mem::take(&mut web.said).contains(&Said::MoreToCome) {
                page.logins += 1;
                web.session.disconnected();
                let _ = hub.disconnect(2);
                web.session.connected();
                for o in hub.connect(2, &mut rng) {
                    if let Out::Send(_, b) = o {
                        web.receive(&b, &mut rng);
                    }
                }
            }
        }
    }
    let t = std::time::Instant::now();
    page_after(&mut web, &mut page, true);
    page.took += t.elapsed();
    (web, page)
}

/// What the page does after frames: frames out, the counts; and the list
/// made again if a note changed and it's `due`.
fn page_after(web: &mut Notes, page: &mut PageWork, due: bool) {
    let _ = web.session.waiting();
    let _ = web.unsent(&[]);
    if web.has_changed() && due && !web.take_changed().is_empty() {
        let _ = web.notes();
        let _ = web.folders();
        page.lists += 1;
    }
}

/// Measured, not run by default: `CARNET_INTAKE=10000 cargo test --release
/// -p carnet-web --test viewer intake_measured -- --ignored --nocapture`
/// (`CARNET_TICK`: notes the desktop sends at a time; `CARNET_LISTS`:
/// `frame`, `batch`, or the least milliseconds between lists).
#[test]
#[ignore]
fn intake_measured() {
    let var = |k: &str| std::env::var(k).ok();
    let n: usize = var("CARNET_INTAKE").and_then(|s| s.parse().ok()).unwrap_or(2000);
    let tick: usize = var("CARNET_TICK").and_then(|s| s.parse().ok()).unwrap_or(1);
    let lists = match var("CARNET_LISTS").as_deref() {
        Some("frame") => Lists::EachFrame,
        Some("batch") => Lists::EachBatch,
        Some(ms) => Lists::AtMostEvery(std::time::Duration::from_millis(ms.parse().unwrap())),
        None => Lists::AtMostEvery(std::time::Duration::from_millis(250)),
    };
    let t = std::time::Instant::now();
    let (web, page) = intake(n, tick, lists);
    let all = t.elapsed();
    let listed = web.notes();
    assert_eq!(listed.iter().filter(|l| l.folder == "Bulk").count(), n);
    eprintln!(
        "{n} notes, {tick} a tick, lists {lists:?}: browser {:?} (receiving {:?}) over {} frames, {} lists, {} logins again; everything {all:?}",
        page.took, page.receiving, page.frames, page.lists, page.logins
    );
}

/// Two thousand notes from the desktop, one a tick, each tick's place
/// sent after it: the browser reads the vault's every note a few times in
/// all, as the page lists them, not once for each change of the vault (as
/// it did, which made taking ten thousand in take minutes). Every note is
/// listed in its folder.
#[test]
fn many_notes_come_in_with_the_vault_read_a_few_times() {
    let n = 2000;
    let (web, page) = intake(n, 1, Lists::AtMostEvery(std::time::Duration::from_secs(3600)));
    assert!(page.frames >= 3 * n, "each note came in frames of its own: {}", page.frames);
    assert!(web.vault_reads() <= 3, "the vault read {} times", web.vault_reads());
    let listed = web.notes();
    assert_eq!(listed.len(), n);
    assert!(listed.iter().all(|l| l.folder == "Bulk" && l.title.starts_with("Bulk ")));
    // And listed after each batch of frames, as often as the page might:
    // once for each list, never more.
    let (web, page) = intake(300, 1, Lists::EachBatch);
    assert!(web.vault_reads() <= page.lists + 1, "{} reads for {} lists", web.vault_reads(), page.lists);
}
