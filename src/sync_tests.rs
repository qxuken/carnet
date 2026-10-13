//! Two notes folders, two devices, syncing through the hub (in memory, in
//! order): what the store does with notes changed on another device.

use std::path::PathBuf;

use carnet_core::doc::{Block, Doc};
use carnet_server::{ConnId, Hub, Memory, Out, Storage};
use carnet_sync::Key;
use carnet_sync::crypto::{DeviceKey, OsRandom, VaultKey, Ring};
use carnet_sync::session::{Event, Identity, Session};
use carnet_sync::vault;

use crate::store::{Remote, Store};

const ORIGIN: &str = "https://sync.test";
const ACCOUNT: &[u8] = b"acct";

struct Side {
    store: Store,
    session: Session,
    conn: ConnId,
    dir: PathBuf,
    remote: Vec<Remote>,
    vault_in: bool,
    /// Where the notes go not yet looked at here: what changed in them is
    /// sent, their places wait.
    layout_held: bool,
    pictures: crate::pictures::Pictures,
    /// The invites the server last listed as waiting, by hash.
    invites: Option<Vec<[u8; 32]>>,
}

struct Two {
    hub: Hub<Memory>,
    /// The picture worker, as the server runs one beside the hub.
    worker: carnet_server::blobs::Worker<carnet_server::blobs::MemoryBlobs>,
    sides: Vec<Side>,
    /// The devices' clock, in seconds since 1970.
    now: u64,
    /// What the picture storage answers every send with, when it refuses
    /// them (`HTTP 403`); and how many sends were tried.
    refuse: Option<String>,
    puts: usize,
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("carnet-synced-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(d.with_extension("replica"));
    d
}

impl Two {
    fn new(name: &str) -> Self {
        Self::with_folders(name, [None, None])
    }

    /// Two devices of a new account (a vault key of its own), each with
    /// the notes folder given, as earlier runs left it (its replica and
    /// pictures index too), or a new one.
    fn with_folders(name: &str, dirs: [Option<PathBuf>; 2]) -> Self {
        let mut hub = Hub::new(ORIGIN, Memory::default());
        hub.store_mut().add_account(ACCOUNT, true).unwrap();
        let vault = VaultKey::generate(&mut OsRandom);
        let sides = dirs
            .into_iter()
            .enumerate()
            .map(|(i, dir)| {
                let key = DeviceKey::generate(&mut OsRandom);
                let device = format!("d{i}").into_bytes();
                hub.store_mut().add_device(ACCOUNT, &device, key.public()).unwrap();
                let id = Identity { origin: ORIGIN.into(), account: ACCOUNT.to_vec(), device, key, ring: Ring::new(vault.clone()), name: String::new() };
                let dir = dir.unwrap_or_else(|| temp(&format!("{name}-{i}")));
                let store = Store::open(dir.clone()).unwrap();
                Side {
                    pictures: crate::pictures::Pictures::open(store.replica_dir().as_deref()),
                    store,
                    session: Session::new(id, 0),
                    conn: i as ConnId + 1,
                    dir,
                    remote: Vec::new(),
                    vault_in: false,
                    layout_held: false,
                    invites: None,
                }
            })
            .collect();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let mut two = Two { hub, worker: carnet_server::blobs::Worker::memory(), sides, now, refuse: None, puts: 0 };
        for i in 0..2 {
            two.sides[i].session.connected();
            let outs = two.hub.connect(two.sides[i].conn, &mut OsRandom);
            two.deliver(outs);
        }
        two.pump();
        two
    }

    fn deliver(&mut self, outs: Vec<Out>) {
        let outs = self.worker.done(outs);
        let now = self.now;
        for o in outs {
            let Out::Send(conn, bytes) = o else { continue };
            let side = self.sides.iter_mut().find(|s| s.conn == conn).expect("a side");
            let replica = side.store.replica.as_mut().expect("a replica");
            side.session.receive(&bytes, replica, &mut OsRandom);
            for e in side.session.take_events() {
                if matches!(e, Event::BlobWanted(_) | Event::BlobStored(_) | Event::BlobGot(..) | Event::BlobFailed(_)) {
                    side.pictures.event(&e, &mut side.store, &mut side.session, &mut OsRandom).unwrap();
                    continue;
                }
                // As the app does: pictures held back, offered again once
                // there's room.
                if let Event::Refused(why) = &e
                    && (why == carnet_sync::session::FULL || why == carnet_sync::session::READ_ONLY)
                {
                    side.pictures.held_back();
                }
                if let Event::Usage(u) = &e
                    && u.has_room()
                {
                    side.pictures.retry();
                }
                // As the app does: the codes of invites no longer waiting
                // forgotten in the vault.
                if let Event::Invites { invites, .. } = &e {
                    let waiting: Vec<[u8; 32]> = invites.iter().map(|i| i.hash).collect();
                    side.store.forget_invites_but(&waiting, now);
                    side.invites = Some(waiting);
                }
                if let Event::Changed(room) = e {
                    if room == vault::ROOM {
                        side.store.vault_changed().unwrap();
                        side.vault_in = true;
                        continue;
                    }
                    let key = Key::parse(&room).expect("a key");
                    side.remote.push(side.store.remote_changed(key).unwrap());
                }
            }
        }
    }

    /// What the app does each tick: saves made sent, frames moved, until
    /// nothing moves.
    fn pump(&mut self) {
        for _ in 0..1000 {
            let mut moved = false;
            for i in 0..self.sides.len() {
                let s = &mut self.sides[i];
                // What the app does each tick: where the notes go, in step.
                // (Held: as the app does for up to a second, the notes'
                // texts go before their places.)
                if !s.layout_held {
                    s.store.reconcile(s.session.has_joined(vault::ROOM)).unwrap();
                }
                if s.session.has_joined(vault::ROOM) && !s.layout_held {
                    s.pictures.tick(&mut s.store, &mut s.session);
                }
                s.vault_in = false;
                let keys = s.store.take_changed();
                let shown = s.store.shown_by(&keys);
                s.pictures.want_back(&s.store, &mut s.session, &shown);
                for key in keys {
                    let replica = s.store.replica.as_mut().unwrap();
                    s.session.saved(&key.to_string(), replica, &mut OsRandom);
                }
                if s.store.take_vault_changed() {
                    let replica = s.store.replica.as_mut().unwrap();
                    s.session.saved(vault::ROOM, replica, &mut OsRandom);
                }
                // Deleted forever, and pictures out of use: told.
                let gone = s.store.take_forgotten();
                for key in &gone.keys {
                    s.session.remove_room(&key.to_string());
                }
                let ids: Vec<_> = gone.pictures.iter().map(|h| unhex(h)).collect();
                s.session.drop_blobs(&ids);
                // Pictures' HTTP, done as the server's links would have it.
                for t in s.session.take_transfers() {
                    moved = true;
                    let put = matches!(t, carnet_sync::session::Transfer::Put { .. });
                    self.puts += usize::from(put);
                    let result = match &self.refuse {
                        Some(answer) if put => Err(answer.clone()),
                        _ => self.worker.blobs.serve(&t),
                    };
                    // As the app does: the pictures hear first.
                    s.pictures.transferred(t.id(), put, &result);
                    s.session.transferred(t.id(), put, result);
                    for e in s.session.take_events() {
                        s.pictures.event(&e, &mut s.store, &mut s.session, &mut OsRandom).unwrap();
                    }
                }
                let (conn, out) = (s.conn, s.session.take_out());
                for b in out {
                    moved = true;
                    let outs = self.hub.receive(conn, &b, &mut OsRandom);
                    self.deliver(outs);
                }
            }
            if !moved {
                return;
            }
        }
        panic!("did not settle");
    }

    fn note(&self, side: usize, title: &str) -> Option<usize> {
        self.sides[side].store.notes.iter().position(|n| n.title == title)
    }

    fn file(&self, side: usize, title: &str) -> String {
        let i = self.note(side, title).unwrap_or_else(|| panic!("no {title} on {side}"));
        std::fs::read_to_string(self.sides[side].store.notes[i].path.as_ref().unwrap()).unwrap()
    }

    fn type_into(&mut self, side: usize, title: &str, text: &str) {
        let i = self.note(side, title).unwrap();
        let note = &mut self.sides[side].store.notes[i];
        note.doc.push(Block::text(text));
        note.touch();
    }
}

impl Drop for Two {
    fn drop(&mut self) {
        for s in &self.sides {
            let _ = std::fs::remove_dir_all(&s.dir);
            let _ = std::fs::remove_dir_all(s.dir.with_extension("replica"));
        }
    }
}

#[test]
fn a_note_made_on_one_device_is_a_file_on_the_other() {
    let mut two = Two::new("made");
    // Each had its welcome note: each now has the other's too.
    assert_eq!(two.sides[1].store.notes.len(), 2);
    let a = &mut two.sides[0].store;
    a.notes.push(crate::store::Note::new("Groceries", Doc::new(vec![Block::text("milk")])));
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    two.pump();
    assert_eq!(two.file(1, "Groceries"), two.file(0, "Groceries"));
    assert!(two.sides[1].remote.iter().any(|r| matches!(r, Remote::Created(_))));
    assert!(two.sides[1].dir.join("Groceries.md").exists(), "named for its title");
}

#[test]
fn an_edit_there_reloads_a_note_with_none_here() {
    let mut two = Two::new("edit");
    let a = &mut two.sides[0].store;
    a.notes.push(crate::store::Note::new("Plan", Doc::new(vec![Block::text("one")])));
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    two.pump();
    two.type_into(1, "Plan", "two");
    let i = two.note(1, "Plan").unwrap();
    two.sides[1].store.save(i, true).unwrap();
    two.pump();
    assert!(two.file(0, "Plan").contains("two"));
    let i = two.note(0, "Plan").unwrap();
    assert!(!two.sides[0].store.notes[i].dirty);
    assert_eq!(two.sides[0].store.notes[i].doc.blocks().iter().filter_map(|b| b.text_ref()).collect::<Vec<_>>(), ["one", "two"]);
}

#[test]
fn an_edit_there_under_edits_here_keeps_both() {
    let mut two = Two::new("both");
    let a = &mut two.sides[0].store;
    a.notes.push(crate::store::Note::new("Shared", Doc::new(vec![Block::text("start")])));
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    two.pump();
    // A types and does not save; B types, saves, and it arrives at A.
    two.type_into(0, "Shared", "from here");
    two.type_into(1, "Shared", "from there");
    let i = two.note(1, "Shared").unwrap();
    two.sides[1].store.save(i, true).unwrap();
    two.pump();
    assert!(two.sides[0].remote.iter().any(|r| matches!(r, Remote::Merged(_))), "{:?}", two.sides[0].remote);
    let here = two.file(0, "Shared");
    assert!(here.contains("from here") && here.contains("from there"), "{here}");
    // And B gets A's half once A's merge goes out.
    two.pump();
    assert_eq!(two.file(1, "Shared"), here);
}

/// Another app changed the file here a moment before another device's
/// change came, and this one hadn't looked yet: both stay, rather than the
/// file being written over.
#[test]
fn an_edit_in_another_app_here_and_one_there_both_stay() {
    let mut two = Two::new("outside");
    let a = &mut two.sides[0].store;
    a.notes.push(crate::store::Note::new("List", Doc::new(vec![Block::text("one")])));
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    two.pump();
    let i = two.note(1, "List").unwrap();
    let path = two.sides[1].store.notes[i].path.clone().unwrap();
    let outside = format!("{}\nfrom another app\n", std::fs::read_to_string(&path).unwrap());
    std::fs::write(&path, &outside).unwrap();
    two.type_into(0, "List", "from there");
    let i = two.note(0, "List").unwrap();
    two.sides[0].store.save(i, true).unwrap();
    two.pump();
    let here = two.file(1, "List");
    assert!(here.contains("from another app") && here.contains("from there"), "{here}");
    // And A gets the other app's line once B's merge goes out.
    two.pump();
    assert_eq!(two.file(0, "List"), here);
}

/// A change from another device that takes most of a note is applied, and
/// the text from before it kept in the trash, where Deleted shows it; a
/// small change keeps nothing.
#[test]
fn a_large_removal_there_keeps_the_text_from_before_here() {
    let mut two = Two::new("removal");
    let a = &mut two.sides[0].store;
    let lines: Vec<Block> = (0..10).map(|k| Block::text(format!("item {k} of the long list"))).collect();
    a.notes.push(crate::store::Note::new("Packing", Doc::new(lines)));
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    two.pump();
    let before = two.file(1, "Packing");

    // A small change there: nothing kept.
    two.type_into(0, "Packing", "and a hat");
    let i = two.note(0, "Packing").unwrap();
    two.sides[0].store.save(i, true).unwrap();
    two.pump();
    assert!(two.sides[1].store.trashed().is_empty());

    // All but the title gone there: applied here, the text before kept.
    let before = { let _ = before; two.file(1, "Packing") };
    let i = two.note(0, "Packing").unwrap();
    let note = &mut two.sides[0].store.notes[i];
    note.doc = Doc::new(vec![Block::text("")]);
    note.touch();
    two.sides[0].store.save(i, true).unwrap();
    two.pump();
    assert!(!two.file(1, "Packing").contains("item 3"), "the removal is applied");
    let kept = two.sides[1].store.trashed();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].path.file_name().unwrap().to_string_lossy().contains("(before sync)"));
    assert_eq!(std::fs::read_to_string(&kept[0].path).unwrap(), before);
    assert_eq!(kept[0].name, "Packing.md", "Deleted knows where it came from");
}

impl Two {
    /// A note on `side` made and saved, in `folder`.
    fn make(&mut self, side: usize, folder: &str, title: &str, body: &str) -> usize {
        let s = &mut self.sides[side].store;
        if !folder.is_empty() {
            s.create_folder("", folder).unwrap();
        }
        let mut n = crate::store::Note::new(title, Doc::new(vec![Block::text(body)]));
        n.folder = folder.to_string();
        s.notes.push(n);
        let i = s.notes.len() - 1;
        s.save(i, true).unwrap();
        i
    }

    /// `side`'s notes as (folder, file name, pinned), sorted; and its
    /// trash's names.
    #[allow(clippy::type_complexity)]
    fn shape(&self, side: usize) -> (Vec<(String, String, bool)>, Vec<String>, Vec<String>) {
        let s = &self.sides[side].store;
        let mut notes: Vec<_> = s
            .notes
            .iter()
            .filter_map(|n| Some((n.folder.clone(), n.path.as_ref()?.file_name()?.to_string_lossy().into_owned(), n.pinned)))
            .collect();
        notes.sort();
        let mut trash: Vec<String> = s.trashed().into_iter().map(|t| t.title).collect();
        trash.sort();
        (notes, s.folders.clone(), trash)
    }
}

/// A note moved into a folder, pinned, renamed (by its title), trashed and
/// put back on one device: the other does each, its files following.
#[test]
fn where_a_note_lives_follows_it_to_the_other_device() {
    let mut two = Two::new("vault-follow");
    let i = two.make(0, "", "Groceries", "milk");
    two.pump();
    assert_eq!(two.shape(0), two.shape(1));

    // Moved into a folder made for it.
    let a = &mut two.sides[0].store;
    a.create_folder("", "Home").unwrap();
    a.move_note(i, "Home").unwrap();
    two.pump();
    assert!(two.sides[1].dir.join("Home/Groceries.md").exists(), "{:?}", two.shape(1));
    assert_eq!(two.shape(0), two.shape(1));

    // Pinned.
    let a = &mut two.sides[0].store;
    let i = a.notes.iter().position(|n| n.title == "Groceries").unwrap();
    a.notes[i].pinned = true;
    a.save_pins().unwrap();
    two.pump();
    let b = &two.sides[1].store;
    assert!(b.notes.iter().find(|n| n.title == "Groceries").unwrap().pinned);

    // Renamed by its title.
    let a = &mut two.sides[0].store;
    let i = a.notes.iter().position(|n| n.title == "Groceries").unwrap();
    a.notes[i].title = "Shopping".into();
    a.notes[i].renamable = true;
    a.notes[i].touch();
    a.save(i, true).unwrap();
    two.pump();
    assert!(two.sides[1].dir.join("Home/Shopping.md").exists(), "{:?}", two.shape(1));
    assert!(!two.sides[1].dir.join("Home/Groceries.md").exists());
    assert_eq!(two.shape(0), two.shape(1));

    // Trashed, and put back.
    let a = &mut two.sides[0].store;
    let i = a.notes.iter().position(|n| n.title == "Shopping").unwrap();
    a.trash(i).unwrap();
    two.pump();
    assert!(two.sides[1].store.notes.iter().all(|n| n.title != "Shopping"), "trashed there too");
    assert_eq!(two.shape(1).2, ["Shopping"]);
    let t = two.sides[0].store.trashed().into_iter().find(|t| t.title == "Shopping").unwrap();
    two.sides[0].store.restore_trashed(&t).unwrap();
    two.pump();
    assert!(two.sides[1].dir.join("Home/Shopping.md").exists(), "back where it was: {:?}", two.shape(1));
    assert_eq!(two.shape(0), two.shape(1));
}

/// A pinned note trashed comes back pinned: put back here, or on the
/// other device.
#[test]
fn a_pin_is_kept_through_the_trash() {
    let mut two = Two::new("vault-pin-trash");
    two.make(0, "", "Plans", "soon");
    let a = &mut two.sides[0].store;
    let i = a.notes.iter().position(|n| n.title == "Plans").unwrap();
    a.notes[i].pinned = true;
    a.save_pins().unwrap();
    two.pump();
    let pinned = |two: &Two, side: usize| two.sides[side].store.notes.iter().find(|n| n.title == "Plans").map(|n| n.pinned);

    // Trashed and put back here.
    let a = &mut two.sides[0].store;
    let i = a.notes.iter().position(|n| n.title == "Plans").unwrap();
    a.trash(i).unwrap();
    two.pump();
    let t = two.sides[0].store.trashed().into_iter().find(|t| t.title == "Plans").unwrap();
    two.sides[0].store.restore_trashed(&t).unwrap();
    two.pump();
    assert_eq!((pinned(&two, 0), pinned(&two, 1)), (Some(true), Some(true)));
    assert!(std::fs::read_to_string(two.sides[0].dir.join(".pinned")).unwrap().contains("plans.md"), "kept for the next start");

    // Trashed here, put back there.
    let a = &mut two.sides[0].store;
    let i = a.notes.iter().position(|n| n.title == "Plans").unwrap();
    a.trash(i).unwrap();
    two.pump();
    let t = two.sides[1].store.trashed().into_iter().find(|t| t.title == "Plans").expect("in B's Deleted");
    two.sides[1].store.restore_trashed(&t).unwrap();
    two.pump();
    assert_eq!((pinned(&two, 0), pinned(&two, 1)), (Some(true), Some(true)));
    assert_eq!(two.shape(0), two.shape(1));
}

/// Folders: an empty one made, one renamed with notes in it, one
/// trashed whole.
#[test]
fn folders_follow_too() {
    let mut two = Two::new("vault-folders");
    two.sides[0].store.create_folder("", "Ideas").unwrap();
    two.make(0, "Work", "Plan", "q3");
    two.make(0, "Work", "Notes", "misc");
    two.pump();
    assert!(two.sides[1].store.folders.contains(&"Ideas".to_string()), "an empty folder too");
    assert!(two.sides[1].dir.join("Work/Plan.md").exists());

    two.sides[0].store.rename_folder("Work", "Job").unwrap();
    two.pump();
    assert!(two.sides[1].dir.join("Job/Plan.md").exists() && two.sides[1].dir.join("Job/Notes.md").exists());
    assert!(!two.sides[1].dir.join("Work").exists(), "the old one gone, empty");
    assert_eq!(two.shape(0), two.shape(1));

    // Moved into another (dragged there): the same there.
    two.sides[0].store.move_folder("Job", "Ideas").unwrap();
    for _ in 0..4 {
        two.pump();
    }
    assert!(two.sides[1].dir.join("Ideas/Job/Plan.md").exists() && two.sides[1].dir.join("Ideas/Job/Notes.md").exists(), "{:?}", two.shape(1));
    assert!(!two.sides[1].dir.join("Job").exists(), "the old one gone, empty");
    assert_eq!(two.shape(0), two.shape(1));

    two.sides[0].store.trash_folder("Ideas/Job").unwrap();
    two.pump();
    assert!(!two.sides[1].dir.join("Ideas/Job/Plan.md").exists());
    let mut trash = two.shape(1).2;
    trash.sort();
    assert_eq!(trash, ["Notes", "Plan"], "its notes in the trash there");
    assert!(!two.sides[1].store.folders.contains(&"Ideas/Job".to_string()));
}

/// A note made in a folder on one device arrives in that folder, not at
/// the top; and one trashed there before this device ever had it
/// arrives in the trash.
#[test]
fn a_new_note_arrives_where_it_lives() {
    let mut two = Two::new("vault-new");
    // B offline for this: A's changes wait on the server.
    two.make(0, "Travel", "Lisbon", "trams");
    let gone = two.make(0, "", "Old list", "socks");
    two.sides[0].store.trash(gone).unwrap();
    two.pump();
    assert!(two.sides[1].dir.join("Travel/Lisbon.md").exists(), "{:?}", two.shape(1));
    assert!(two.sides[1].store.notes.iter().all(|n| n.title != "Old list"));
    assert_eq!(two.shape(1).2, ["Old list"], "in the trash there, as here");
    assert_eq!(two.shape(0), two.shape(1));
}

/// Both at once: pinned on one, moved on the other. Both happen, on both.
#[test]
fn a_pin_here_and_a_move_there_both_hold() {
    let mut two = Two::new("vault-both");
    two.make(0, "", "Recipes", "soup");
    two.sides[0].store.create_folder("", "Kitchen").unwrap();
    two.pump();
    // Apart: each changes a different field, then they meet.
    let a = &mut two.sides[0].store;
    let i = a.notes.iter().position(|n| n.title == "Recipes").unwrap();
    a.notes[i].pinned = true;
    a.save_pins().unwrap();
    let b = &mut two.sides[1].store;
    let i = b.notes.iter().position(|n| n.title == "Recipes").unwrap();
    b.move_note(i, "Kitchen").unwrap();
    two.pump();
    two.pump();
    assert_eq!(two.shape(0), two.shape(1));
    let a = &two.sides[0].store;
    let n = a.notes.iter().find(|n| n.title == "Recipes").unwrap();
    assert_eq!((n.folder.as_str(), n.pinned), ("Kitchen", true));
}

/// splitmix64, for the random runs: every choice from one seed.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Two devices move, pin, rename, trash and put back notes, and make,
/// rename and trash folders, at random, sometimes apart and sometimes in
/// step. When they meet: the same notes, in the same folders, under the
/// same names, pinned and trashed alike, on both; and no note lost.
#[test]
fn random_moves_on_two_devices_end_the_same() {
    let seeds: u64 = std::env::var("VAULT_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    for seed in 0..seeds {
        let mut two = Two::new(&format!("vault-rand-{seed}"));
        let mut d = Dice(seed);
        let folders = ["Home", "Work", "Ideas", "Trips"];
        let mut made = 0;
        for _ in 0..60 {
            let side = d.below(2);
            let s = &mut two.sides[side].store;
            match d.below(10) {
                0 | 1 => {
                    made += 1;
                    let f = if d.below(2) == 0 { "" } else { folders[d.below(folders.len())] };
                    if !f.is_empty() {
                        let _ = s.create_folder("", f);
                    }
                    let mut n = crate::store::Note::new(&format!("Note {seed} {made}"), Doc::new(vec![Block::text("words")]));
                    n.folder = f.to_string();
                    s.notes.push(n);
                    let i = s.notes.len() - 1;
                    s.save(i, true).unwrap();
                }
                2 if !s.notes.is_empty() => {
                    let i = d.below(s.notes.len());
                    let f = folders[d.below(folders.len())];
                    let _ = s.create_folder("", f);
                    let _ = s.move_note(i, f);
                }
                3 if !s.notes.is_empty() => {
                    let i = d.below(s.notes.len());
                    s.notes[i].pinned = !s.notes[i].pinned;
                    s.save_pins().unwrap();
                }
                4 if !s.notes.is_empty() => {
                    let i = d.below(s.notes.len());
                    s.notes[i].title = format!("Renamed {}", d.below(5));
                    s.notes[i].renamable = true;
                    s.notes[i].touch();
                    let _ = s.save(i, true);
                }
                5 if !s.notes.is_empty() => {
                    let i = d.below(s.notes.len());
                    let _ = s.trash(i);
                }
                6 => {
                    let t = s.trashed().into_iter().filter(|t| !t.is_folder).collect::<Vec<_>>();
                    if !t.is_empty() {
                        let _ = s.restore_trashed(&t[d.below(t.len())]);
                    }
                }
                7 => {
                    let f = folders[d.below(folders.len())];
                    if s.folders.iter().any(|g| g == f) {
                        let _ = s.rename_folder(f, folders[d.below(folders.len())]);
                    }
                }
                8 => {
                    let f = folders[d.below(folders.len())];
                    if s.folders.iter().any(|g| g == f) && d.below(3) == 0 {
                        let _ = s.trash_folder(f);
                    }
                }
                _ => two.pump(),
            }
        }
        two.pump();
        two.pump();
        let (a, b) = (two.shape(0), two.shape(1));
        assert_eq!(a.0, b.0, "seed {seed}: the notes differ");
        // Every note in the same place on both: in the list, or in the
        // trash coming from the same place (a folder trashed whole on one
        // device is its notes one by one on the other: the same places).
        let place = |side: usize| {
            let s = &two.sides[side].store;
            let vault = s.replica.as_ref().unwrap().vault().layout();
            (s.layout(&vault).notes, vault)
        };
        let ((la, va), (lb, vb)) = (place(0), place(1));
        assert_eq!(va, vb, "seed {seed}: the vaults differ");
        assert_eq!(la, lb, "seed {seed}: the places differ");
        assert_eq!(la, va.notes, "seed {seed}: the files aren't as the vault says");
        assert_eq!(la.len(), made + 2, "seed {seed}: a note lost (each made, and the two welcome notes)");
    }
}

/// A picture pasted into a note on one device is a file on the other, at
/// the same path, byte for byte; the same picture pasted twice is one blob
/// on the server, and two files there as here.
#[test]
fn pictures_follow_their_notes() {
    let mut two = Two::new("pictures");
    let photo: Vec<u8> = (0..900_000u32).map(|i| (i % 251) as u8).collect();
    let a = &mut two.sides[0].store;
    let src = a.save_attachment("", "png", &photo).unwrap();
    let again = a.save_attachment("", "png", &photo).unwrap();
    assert_ne!(src, again, "two names, one picture");
    let mut n = crate::store::Note::new("Holiday", Doc::new(vec![Block::text("the view")]));
    n.doc.push(Block::new(carnet_core::doc::Kind::Image { src: src.clone(), alt: String::new(), float: Default::default(), width: None, wiki: false }));
    a.notes.push(n);
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    // Each round of the app's tick: a pump is one.
    for _ in 0..4 {
        two.pump();
    }
    for p in [&src, &again] {
        let there = two.sides[1].dir.join(p);
        assert_eq!(std::fs::read(&there).ok().as_deref(), Some(photo.as_slice()), "{p} on the other device");
    }
    assert!(two.file(1, "Holiday").contains(&src), "and the note shows it");
    // Kept once on the server, sealed.
    let id = two.sides[0].session.blob_id(&photo);
    let kept = two.worker.blobs.get(ACCOUNT, &id).expect("kept");
    assert!(!kept.windows(64).any(|w| w == &photo[..64]));
    assert!(two.sides[1].pictures_arrived());
}

/// A picture offered while the account is full is held back by the
/// server, with no word for it alone: it isn't waited on (the sweep isn't
/// held up for it), the sweep leaves it, and once the server says there's
/// room (a plan raised, told unasked) it goes, and reaches the other
/// device, with no new login.
#[test]
fn a_picture_held_back_while_full_goes_once_there_is_room() {
    use carnet_server::quota::{Limit, Quota};
    use std::sync::{Arc, Mutex};
    struct Plan(Mutex<Limit>);
    impl Quota for Plan {
        fn limit(&self, _: &[u8]) -> Limit {
            self.0.lock().unwrap().clone()
        }
    }
    let mut two = Two::new("held");
    let plan = Arc::new(Plan(Mutex::new(Limit { bytes: Some(1), writable: true, plan: "Tiny".into(), until: 0 })));
    two.hub.set_quota(plan.clone());
    let photo: Vec<u8> = (0..20_000u32).map(|i| (i % 241) as u8).collect();
    let a = &mut two.sides[0].store;
    let src = a.save_attachment("", "png", &photo).unwrap();
    let mut n = crate::store::Note::new("Over", Doc::new(vec![Block::text("full")]));
    n.doc.push(Block::new(carnet_core::doc::Kind::Image { src: src.clone(), alt: String::new(), float: Default::default(), width: None, wiki: false }));
    a.notes.push(n);
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    let p = &two.sides[0].pictures;
    assert!(p.is_holding(), "held back");
    assert!(!p.busy(), "not waited on");
    assert!(p.busy_paths().contains(&src), "but kept from the sweep");
    assert!(!two.sides[1].dir.join(&src).exists());
    // More pumps (ticks) send nothing more while full.
    two.pump();
    assert!(two.sides[0].pictures.is_holding());

    *plan.0.lock().unwrap() = Limit { bytes: Some(1 << 30), writable: true, plan: "Family".into(), until: 0 };
    let outs = two.hub.limit_changed(ACCOUNT);
    two.deliver(outs);
    for _ in 0..4 {
        two.pump();
    }
    assert!(!two.sides[0].pictures.is_holding());
    assert_eq!(std::fs::read(two.sides[1].dir.join(&src)).ok().as_deref(), Some(photo.as_slice()), "on the other device");
}

/// The server's picture storage turns a picture away (an HTTP 4xx): it
/// waits, Settings says why in the web's words, and it's tried again
/// after 15 s, then 30, and so on, not at every tick. A failure that
/// passes (503) waits as long, and says nothing. Once the storage takes
/// it, it goes, and nothing is said any more; a new login forgets why too.
#[test]
fn a_picture_the_storage_turns_away_waits_says_why_and_is_tried_less_often() {
    use std::time::Duration;
    let mut two = Two::new("turned-away");
    two.refuse = Some("HTTP 503".into());
    let photo: Vec<u8> = (0..20_000u32).map(|i| (i % 239) as u8).collect();
    let a = &mut two.sides[0].store;
    let src = a.save_attachment("", "png", &photo).unwrap();
    let mut n = crate::store::Note::new("Refused", Doc::new(vec![Block::text("a view")]));
    n.doc.push(Block::new(carnet_core::doc::Kind::Image { src: src.clone(), alt: String::new(), float: Default::default(), width: None, wiki: false }));
    a.notes.push(n);
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    let p = &two.sides[0].pictures;
    assert_eq!(two.puts, 1, "sent once");
    assert_eq!((p.waiting(), p.held_why(), p.put_wait()), (1, None, Duration::from_secs(15)), "busy for a while: waits, quietly");

    // Turned away: said, and the wait doubles.
    two.refuse = Some("HTTP 403".into());
    two.sides[0].pictures.wait_over();
    for _ in 0..4 {
        two.pump();
    }
    let p = &two.sides[0].pictures;
    assert_eq!(two.puts, 2);
    assert_eq!(p.held_why(), Some(crate::pictures::TURNED_AWAY));
    assert_eq!(p.held_why(), Some("The server's picture storage turned new pictures away, so they stay on this device for now."));
    assert_eq!((p.waiting(), p.put_wait()), (1, Duration::from_secs(30)));
    assert!(p.busy_paths().contains(&src), "kept from the sweep");
    // Ticks meanwhile send nothing.
    for _ in 0..4 {
        two.pump();
    }
    assert_eq!(two.puts, 2, "not tried at every tick");
    // Up to five minutes, no more.
    for _ in 0..6 {
        two.sides[0].pictures.wait_over();
        two.pump();
    }
    assert_eq!(two.sides[0].pictures.put_wait(), Duration::from_secs(5 * 60));
    assert!(!two.sides[1].dir.join(&src).exists());

    // Taken: it goes, and nothing's said; the next failure waits 15 s.
    two.refuse = None;
    two.sides[0].pictures.wait_over();
    for _ in 0..4 {
        two.pump();
    }
    let p = &two.sides[0].pictures;
    assert_eq!((p.waiting(), p.held_why(), p.put_wait()), (0, None, Duration::ZERO));
    assert_eq!(std::fs::read(two.sides[1].dir.join(&src)).ok().as_deref(), Some(photo.as_slice()), "on the other device");

    // Said again, then a new login: forgotten, and offered at once.
    two.refuse = Some("HTTP 401".into());
    let other: Vec<u8> = (0..20_000u32).map(|i| (i % 227) as u8).collect();
    let a = &mut two.sides[0].store;
    let src2 = a.save_attachment("", "png", &other).unwrap();
    let mut n = crate::store::Note::new("Again", Doc::new(vec![Block::text("another")]));
    n.doc.push(Block::new(carnet_core::doc::Kind::Image { src: src2, alt: String::new(), float: Default::default(), width: None, wiki: false }));
    a.notes.push(n);
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    assert_eq!(two.sides[0].pictures.held_why(), Some(crate::pictures::TURNED_AWAY));
    two.sides[0].pictures.retry();
    assert_eq!(two.sides[0].pictures.held_why(), None, "a new login");
}

/// A picture beside its note is the user's: it stays on its device, not
/// in the vault, and the other device never has it. Moved into Carnet,
/// it's in the attachments folder, and goes, the note showing it there on
/// both. One an earlier Carnet named in the vault (it synced those too)
/// is let go there at the next run, and no file goes anywhere.
#[test]
fn a_picture_beside_its_note_stays_until_moved_into_carnet() {
    let mut two = Two::new("beside");
    let hill: Vec<u8> = (0..40_000u32).map(|i| (i % 233) as u8).collect();
    two.make(0, "Home", "Weekend", "the view");
    let a = &mut two.sides[0];
    std::fs::write(a.dir.join("Home/hill.jpg"), &hill).unwrap();
    let i = a.store.notes.iter().position(|n| n.title == "Weekend").unwrap();
    let n = &mut a.store.notes[i];
    n.doc.push(Block::new(carnet_core::doc::Kind::Image { src: "hill.jpg".into(), alt: "The hill at seven".into(), float: Default::default(), width: None, wiki: false }));
    n.touch();
    a.store.save(i, true).unwrap();
    a.store.rescan();
    for _ in 0..4 {
        two.pump();
    }
    for side in 0..2 {
        let vault = two.sides[side].store.replica.as_ref().unwrap().vault().pictures();
        assert!(vault.is_empty(), "not in {side}'s vault: {vault:?}");
    }
    assert!(!two.sides[1].dir.join("Home/hill.jpg").exists());
    assert!(two.file(1, "Weekend").contains("![The hill at seven](hill.jpg)"));

    // Moved into Carnet: it goes.
    let a = &mut two.sides[0];
    let from = a.dir.join("Home/hill.jpg");
    let moved = a.store.move_picture(&from).unwrap();
    assert!(!from.exists() && moved.to == a.dir.join("attachments/hill.jpg"));
    for _ in 0..4 {
        two.pump();
    }
    let id = crate::pictures::hex(&two.sides[0].session.blob_id(&hill));
    for side in 0..2 {
        let vault = two.sides[side].store.replica.as_ref().unwrap().vault().pictures();
        assert_eq!(vault.get("attachments/hill.jpg"), Some(&id), "in {side}'s vault");
    }
    assert_eq!(std::fs::read(two.sides[1].dir.join("attachments/hill.jpg")).ok(), Some(hill.clone()));
    assert!(two.file(1, "Weekend").contains("![The hill at seven](../attachments/hill.jpg)"));

    // As an earlier Carnet left it: a picture beside a note, named.
    let b = &mut two.sides[1];
    std::fs::write(b.dir.join("Home/old.jpg"), b"old").unwrap();
    assert!(b.store.set_picture("Home/old.jpg", &id));
    for _ in 0..4 {
        two.pump();
    }
    assert!(!two.sides[0].dir.join("Home/old.jpg").exists(), "never fetched: it isn't Carnet's");
    // The next run lets it go.
    let a = &mut two.sides[0];
    a.pictures = crate::pictures::Pictures::open(a.store.replica_dir().as_deref());
    for _ in 0..4 {
        two.pump();
    }
    for side in 0..2 {
        let r = two.sides[side].store.replica.as_ref().unwrap();
        assert!(!r.vault().pictures().contains_key("Home/old.jpg"), "let go in {side}'s vault");
        assert!(r.vault().pictures().contains_key("attachments/hill.jpg"));
    }
    assert!(two.sides[1].dir.join("Home/old.jpg").exists(), "the file stays");
}

/// A picture of Carnet's no note shows, nor any version of one, after the
/// grace period: to the trash on the device that swept it, dropped in the
/// vault, so the other device lets go of its copy too. A note saved
/// showing it again brings it back from the trash, and it goes again.
#[test]
fn a_picture_no_note_shows_is_swept_everywhere_and_comes_back_when_shown() {
    let mut two = Two::new("sweep");
    let photo: Vec<u8> = (0..30_000u32).map(|i| (i % 211) as u8).collect();
    // Pasted, and the paste undone before the note was saved.
    let src = two.sides[0].store.save_attachment("", "png", &photo).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    assert!(two.sides[1].dir.join(&src).exists(), "it went, as every picture of Carnet's does");
    let t0 = std::time::SystemTime::now();
    let none = std::collections::BTreeSet::new();
    let a = &mut two.sides[0];
    assert!(a.store.sweep_pictures(&none, t0).unwrap().is_empty());
    assert_eq!(a.store.sweep_pictures(&none, t0 + crate::store::SWEEP_GRACE + std::time::Duration::from_secs(60)).unwrap(), vec![src.clone()]);
    for _ in 0..4 {
        two.pump();
    }
    assert!(!two.sides[0].dir.join(&src).exists() && two.sides[0].dir.join(".trash").join(&src).exists());
    assert!(!two.sides[1].dir.join(&src).exists(), "the other device let go of it");
    for side in 0..2 {
        assert!(two.sides[side].store.replica.as_ref().unwrap().vault().dropped_pictures().contains(&src));
    }

    // Shown again: back, and named again.
    let i = two.make(0, "", "Album", "the view");
    let a = &mut two.sides[0].store;
    a.notes[i].doc.push(Block::new(carnet_core::doc::Kind::Image { src: src.clone(), alt: String::new(), float: Default::default(), width: None, wiki: false }));
    a.notes[i].touch();
    a.save(i, true).unwrap();
    assert!(two.sides[0].dir.join(&src).exists(), "back from the trash");
    for _ in 0..4 {
        two.pump();
    }
    assert_eq!(std::fs::read(two.sides[1].dir.join(&src)).ok(), Some(photo));
}

/// A notes folder that synced with one account, then set up with another
/// (a new server, or the same one afresh): its pictures go to the new
/// account too. Their ids there are the new vault key's, so the vault and
/// the pictures index carried over from the old account name ids the new
/// server never had.
#[test]
fn pictures_go_to_a_new_account_too() {
    let hill: Vec<u8> = (0..40_000u32).map(|i| (i % 233) as u8).collect();
    let photo: Vec<u8> = (0..30_000u32).map(|i| (i % 211) as u8).collect();
    let mut first = Two::new("new-account");
    first.make(0, "Home", "Weekend", "the view");
    let a = &mut first.sides[0];
    std::fs::write(a.dir.join("Home/hill.jpg"), &hill).unwrap();
    let src = a.store.save_attachment("Home", "jpg", &photo).unwrap();
    let i = a.store.notes.iter().position(|n| n.title == "Weekend").unwrap();
    let n = &mut a.store.notes[i];
    for src in ["hill.jpg", src.as_str()] {
        n.doc.push(Block::new(carnet_core::doc::Kind::Image { src: src.into(), alt: String::new(), float: Default::default(), width: None, wiki: false }));
    }
    // Home/hill.jpg, beside the note, is the user's: it stays here.
    n.touch();
    a.store.save(i, true).unwrap();
    a.store.rescan();
    for _ in 0..4 {
        first.pump();
    }
    let old = first.sides[0].session.blob_id(&photo);
    assert!(first.worker.blobs.get(ACCOUNT, &first.sides[0].session.blob_id(&hill)).is_none(), "the user's own stays on its device");
    assert!(first.worker.blobs.get(ACCOUNT, &old).is_some(), "on the first account's server");
    // The first account's other device goes with it; this folder stays,
    // its replica and pictures index as they were.
    let side = first.sides.remove(0);
    drop(first);
    let dir = side.dir.clone();
    drop(side);

    // The same folder in a new account, a new device beside it.
    let mut second = Two::with_folders("new-account-2", [Some(dir), None]);
    for _ in 0..4 {
        second.pump();
    }
    let attached = src.trim_start_matches("../").to_string();
    assert!(!second.sides[1].dir.join("Home/hill.jpg").exists());
    for (bytes, path) in [(&photo, attached.as_str())] {
        let id = second.sides[0].session.blob_id(bytes);
        assert_ne!(id, old, "ids are the account's");
        assert!(second.worker.blobs.get(ACCOUNT, &id).is_some(), "{path} on the new account's server");
        for side in 0..2 {
            let vault = second.sides[side].store.replica.as_ref().unwrap().vault().pictures();
            assert_eq!(vault.get(path), Some(&crate::pictures::hex(&id)), "{path} named by its new id in {side}'s vault");
        }
        assert_eq!(std::fs::read(second.sides[1].dir.join(path)).ok().as_ref(), Some(bytes), "{path} on the new device");
    }
}

impl Side {
    fn pictures_arrived(&mut self) -> bool {
        std::mem::take(&mut self.pictures.arrived)
    }
}

fn unhex(h: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

/// A note deleted forever on one device is gone from every device and the
/// server, history and all; the pictures only it showed go too (from
/// every attachments folder, and marked on the server to go), and one
/// another note shows stays.
#[test]
fn a_note_deleted_forever_goes_everywhere_and_its_pictures_with_it() {
    let mut two = Two::new("forever");
    let only: Vec<u8> = (0..50_000u32).map(|i| (i % 241) as u8).collect();
    let shared: Vec<u8> = (0..50_000u32).map(|i| (i % 239) as u8).collect();
    let a = &mut two.sides[0].store;
    let (only_src, shared_src) = (a.save_attachment("", "png", &only).unwrap(), a.save_attachment("", "png", &shared).unwrap());
    let image = |src: &str| Block::new(carnet_core::doc::Kind::Image { src: src.into(), alt: String::new(), float: Default::default(), width: None, wiki: false });
    let mut holiday = crate::store::Note::new("Holiday", Doc::new(vec![Block::text("the view"), image(&only_src), image(&shared_src)]));
    holiday.touch();
    let mut album = crate::store::Note::new("Album", Doc::new(vec![image(&shared_src)]));
    album.touch();
    a.notes.push(holiday);
    a.notes.push(album);
    let n = a.notes.len();
    a.save(n - 2, true).unwrap();
    a.save(n - 1, true).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    assert!(two.sides[1].dir.join(&only_src).exists() && two.note(1, "Holiday").is_some());

    // To the trash, everywhere; then deleted forever from it, on A.
    let i = two.note(0, "Holiday").unwrap();
    let key = two.sides[0].store.notes[i].key.expect("a key");
    two.sides[0].store.trash(i).unwrap();
    for _ in 0..3 {
        two.pump();
    }
    assert!(two.sides[1].store.trashed().iter().any(|t| t.title == "Holiday"), "in B's trash too");
    let t = two.sides[0].store.trashed().into_iter().find(|t| t.title == "Holiday").unwrap();
    two.sides[0].store.delete_forever(&t).unwrap();
    for _ in 0..4 {
        two.pump();
    }

    for side in 0..2 {
        let s = &two.sides[side];
        assert!(s.store.trashed().is_empty(), "nothing in {side}'s trash");
        assert!(s.store.replica.as_ref().unwrap().entry(key).is_none(), "{side} forgot it, history and all");
        assert!(!s.dir.join(&only_src).exists(), "its own picture gone from {side}");
        assert!(s.dir.join(&shared_src).exists(), "the shared one stays on {side}");
        assert!(s.store.replica.as_ref().unwrap().vault().dropped_pictures().contains(&only_src));
        assert!(two.note(side, "Album").is_some());
    }
    assert!(two.hub.store().is_removed(ACCOUNT, &key.to_string()));
    assert!(two.hub.store().spans(ACCOUNT, &key.to_string()).is_empty());
    let (only_id, shared_id) = (two.sides[0].session.blob_id(&only), two.sides[0].session.blob_id(&shared));
    assert!(two.worker.blobs.is_marked(ACCOUNT, &only_id), "the server may drop it");
    assert!(!two.worker.blobs.is_marked(ACCOUNT, &shared_id));
}

/// One picture under two names, each shown by a note of its own: the
/// note showing one deleted forever drops that name only. The other stays
/// in the vault and on both devices, and the server keeps the picture.
#[test]
fn a_picture_under_two_names_stays_while_one_is_shown() {
    let mut two = Two::new("twice");
    let photo: Vec<u8> = (0..30_000u32).map(|i| (i % 229) as u8).collect();
    let a = &mut two.sides[0].store;
    let (first, second) = (a.save_attachment("", "png", &photo).unwrap(), a.save_attachment("", "png", &photo).unwrap());
    let image = |src: &str| Block::new(carnet_core::doc::Kind::Image { src: src.into(), alt: String::new(), float: Default::default(), width: None, wiki: false });
    let mut one = crate::store::Note::new("One", Doc::new(vec![image(&first)]));
    one.touch();
    let mut other = crate::store::Note::new("Other", Doc::new(vec![image(&second)]));
    other.touch();
    a.notes.push(one);
    a.notes.push(other);
    let n = a.notes.len();
    a.save(n - 2, true).unwrap();
    a.save(n - 1, true).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    let id = crate::pictures::hex(&two.sides[0].session.blob_id(&photo));
    for p in [&first, &second] {
        assert_eq!(two.sides[1].store.replica.as_ref().unwrap().vault().pictures().get(p), Some(&id));
        assert!(two.sides[1].dir.join(p).exists(), "{p} on the other device");
    }

    let i = two.note(0, "One").unwrap();
    two.sides[0].store.trash(i).unwrap();
    two.pump();
    let t = two.sides[0].store.trashed().into_iter().find(|t| t.title == "One").unwrap();
    two.sides[0].store.delete_forever(&t).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    for side in 0..2 {
        let s = &two.sides[side];
        let vault = s.store.replica.as_ref().unwrap().vault();
        assert!(vault.dropped_pictures().contains(&first), "{side}");
        assert_eq!(vault.pictures().get(&second), Some(&id), "{side}");
        assert!(!s.dir.join(&first).exists() && s.dir.join(&second).exists(), "{side}");
    }
    assert!(!two.worker.blobs.is_marked(ACCOUNT, &unhex(&id)), "still shown: kept");
}

/// A picture an old version of another note shows stays when the note
/// that shows it now is deleted forever: that version, restored, shows it
/// again. And one dropped anyway comes back from the server when a note
/// shows it again (while the server keeps it).
#[test]
fn a_picture_an_old_version_shows_stays_and_a_dropped_one_comes_back() {
    let mut two = Two::new("versions");
    let photo: Vec<u8> = (0..40_000u32).map(|i| (i % 233) as u8).collect();
    let a = &mut two.sides[0].store;
    let src = a.save_attachment("", "png", &photo).unwrap();
    let image = |src: &str| Block::new(carnet_core::doc::Kind::Image { src: src.into(), alt: String::new(), float: Default::default(), width: None, wiki: false });
    // Two notes show it; one then stops (its history still does).
    for title in ["Holiday", "Diary"] {
        let mut n = crate::store::Note::new(title, Doc::new(vec![Block::text("words"), image(&src)]));
        n.touch();
        a.notes.push(n);
        let i = a.notes.len() - 1;
        a.save(i, true).unwrap();
    }
    for _ in 0..4 {
        two.pump();
    }
    let a = &mut two.sides[0].store;
    let d = a.notes.iter().position(|n| n.title == "Diary").unwrap();
    a.notes[d].doc = Doc::new(vec![Block::text("words, no picture now")]);
    a.notes[d].touch();
    a.save(d, false).unwrap();
    let h = a.notes.iter().position(|n| n.title == "Holiday").unwrap();
    a.trash(h).unwrap();
    for _ in 0..3 {
        two.pump();
    }
    let t = two.sides[0].store.trashed().into_iter().find(|t| t.title == "Holiday").unwrap();
    two.sides[0].store.delete_forever(&t).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    let id = two.sides[0].session.blob_id(&photo);
    assert!(two.sides[0].dir.join(&src).exists(), "Diary's old version shows it: kept");
    assert!(!two.worker.blobs.is_marked(ACCOUNT, &id));

    // Dropped anyway (as a picture typed into a note a letter at a time
    // could be): a note showing it again fetches it back.
    let a = &mut two.sides[0].store;
    let (_, unused) = a.replica.as_ref().unwrap().vault().drop_picture(&src);
    assert!(unused.is_some());
    std::fs::remove_file(two.sides[0].dir.join(&src)).unwrap();
    let a = &mut two.sides[0].store;
    let d = a.notes.iter().position(|n| n.title == "Diary").unwrap();
    a.notes[d].doc = Doc::new(vec![Block::text("words"), image(&src)]);
    a.notes[d].touch();
    a.save(d, false).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    let s = &two.sides[0];
    assert_eq!(std::fs::read(s.dir.join(&src)).ok().as_deref(), Some(photo.as_slice()), "back, byte for byte");
    assert_eq!(s.store.replica.as_ref().unwrap().vault().pictures().get(&src), Some(&crate::pictures::hex(&id)), "named again");
}

impl Two {
    /// Every change in note `title`'s history on `side`, by the run of the
    /// app that made it.
    fn peers(&mut self, side: usize, title: &str) -> Vec<u64> {
        let i = self.note(side, title).unwrap_or_else(|| panic!("no {title} on {side}"));
        let key = self.sides[side].store.notes[i].key.expect("a key");
        self.sides[side].store.replica.as_mut().unwrap().history(key).iter().map(|r| r.peer).collect()
    }
}

/// A note written in a folder by another app while both devices are
/// online: its pictures' links, written from that folder, stay as they
/// are on both. The other device puts it where it lives, perhaps at the
/// top first, until it knows where, and that is no edit of its own.
#[test]
fn a_note_written_in_a_folder_keeps_its_picture_links_on_both() {
    let mut two = Two::new("placed-links");
    let pond: Vec<u8> = (0..20_000u32).map(|i| (i % 241) as u8).collect();
    let a = &mut two.sides[0];
    std::fs::create_dir_all(a.dir.join("attachments")).unwrap();
    std::fs::write(a.dir.join("attachments/pond.jpg"), &pond).unwrap();
    std::fs::create_dir_all(a.dir.join("Projects/Garden")).unwrap();
    std::fs::create_dir_all(a.dir.join("Home")).unwrap();
    std::fs::write(a.dir.join("Home/own2.jpg"), b"mine").unwrap();
    let garden = "# Repro4\n\n![](../../attachments/pond.jpg)\n";
    let home = "# Repro3\n\n![](own2.jpg)\n";
    std::fs::write(a.dir.join("Projects/Garden/Repro4.md"), garden).unwrap();
    std::fs::write(a.dir.join("Home/Repro3.md"), home).unwrap();
    a.store.rescan();
    // Their texts there before their places, as the app sends them.
    a.layout_held = true;
    two.pump();
    assert!(two.sides[1].dir.join("Repro4.md").exists(), "at the top until its place comes: {:?}", two.shape(1));
    assert_eq!(std::fs::read_to_string(two.sides[1].dir.join("Repro4.md")).unwrap(), garden, "as written");
    two.sides[0].layout_held = false;
    for _ in 0..4 {
        two.pump();
    }
    assert!(!two.sides[1].dir.join("Repro4.md").exists(), "{:?}", two.shape(1));
    assert!(two.sides[1].dir.join("Projects/Garden/Repro4.md").exists(), "{:?}", two.shape(1));
    assert!(two.sides[1].dir.join("Home/Repro3.md").exists(), "{:?}", two.shape(1));
    for side in 0..2 {
        assert_eq!(two.file(side, "Repro4"), garden, "on {side}");
        assert_eq!(two.file(side, "Repro3"), home, "on {side}");
    }
    assert_eq!(std::fs::read(two.sides[1].dir.join("attachments/pond.jpg")).ok(), Some(pond));
    // A's doing alone, on both.
    for side in 0..2 {
        for title in ["Repro4", "Repro3"] {
            let peers = two.peers(side, title);
            assert!(peers.windows(2).all(|w| w[0] == w[1]), "{title} edited by another run on {side}: {peers:?}");
        }
    }

    // A folder renamed: as deep as it was, so no link changes, here or
    // there, though there each note follows it on its own.
    two.sides[0].store.rename_folder("Home", "House").unwrap();
    two.sides[0].store.rename_folder("Projects", "Work").unwrap();
    for _ in 0..4 {
        two.pump();
    }
    assert!(two.sides[1].dir.join("House/Repro3.md").exists(), "{:?}", two.shape(1));
    assert!(two.sides[1].dir.join("Work/Garden/Repro4.md").exists(), "{:?}", two.shape(1));
    for side in 0..2 {
        assert_eq!(two.file(side, "Repro4"), garden, "on {side}");
        assert_eq!(two.file(side, "Repro3"), home, "on {side}");
    }
}

/// A note with no heading is called by its file's name. Come to the
/// other device before its place, it is put at the top as "Untitled"
/// until the vault names it; then it is called by that name there too,
/// not "Untitled" still.
#[test]
fn a_note_without_a_heading_is_called_the_same_on_both() {
    let mut two = Two::new("no-heading");
    let a = &mut two.sides[0];
    std::fs::write(a.dir.join("Missing.md"), "Just words, no heading.\n").unwrap();
    a.store.rescan();
    a.layout_held = true;
    two.pump();
    assert!(two.sides[1].dir.join("Untitled.md").exists(), "at the top until its place comes: {:?}", two.shape(1));
    two.sides[0].layout_held = false;
    for _ in 0..4 {
        two.pump();
    }
    assert_eq!(two.shape(0), two.shape(1));
    for side in 0..2 {
        let titles: Vec<&str> = two.sides[side].store.notes.iter().map(|n| n.display_title()).collect();
        assert!(titles.contains(&"Missing"), "on {side}: {titles:?}");
        assert!(!titles.contains(&"Untitled"), "on {side}: {titles:?}");
    }
}

/// A note moved into another folder on one device: its links rewritten
/// there, once; the other device follows the move and takes that edit,
/// without rewriting them again.
#[test]
fn a_note_moved_there_has_its_links_rewritten_once() {
    let mut two = Two::new("moved-links");
    let pond: Vec<u8> = (0..20_000u32).map(|i| (i % 239) as u8).collect();
    let a = &mut two.sides[0].store;
    let src = a.save_attachment("Home", "jpg", &pond).unwrap();
    assert!(src.starts_with("../attachments/"), "{src}");
    a.create_folder("", "Home").unwrap();
    a.create_folder("", "Projects").unwrap();
    a.create_folder("Projects", "Garden").unwrap();
    let mut n = crate::store::Note::new("Pond", Doc::new(vec![Block::text("the pond")]));
    n.folder = "Home".into();
    n.doc.push(Block::new(carnet_core::doc::Kind::Image { src: src.clone(), alt: String::new(), float: Default::default(), width: None, wiki: false }));
    a.notes.push(n);
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    assert!(two.file(1, "Pond").contains(&format!("]({src})")));

    let a = &mut two.sides[0].store;
    let i = a.notes.iter().position(|n| n.title == "Pond").unwrap();
    a.move_note(i, "Projects/Garden").unwrap();
    for _ in 0..4 {
        two.pump();
    }
    let moved = format!("](../{src})");
    assert!(two.sides[1].dir.join("Projects/Garden/Pond.md").exists(), "{:?}", two.shape(1));
    for side in 0..2 {
        let text = two.file(side, "Pond");
        assert!(text.contains(&moved), "on {side}: {text}");
    }
    // A's two changes (made, moved), and none of B's.
    for side in 0..2 {
        let peers = two.peers(side, "Pond");
        assert!(peers.windows(2).all(|w| w[0] == w[1]), "edited by another run on {side}: {peers:?}");
    }
}

/// What a hostile device, or a buggy one, could write in the vault:
/// places, folders and pictures' paths that climb out of the notes
/// folder, full paths, Windows' separator. The other device reads none of
/// them: the notes come at the top, named for their titles, no such
/// folder is made, no such picture written, nothing at all outside its
/// notes folder, and nothing fails.
#[test]
fn a_vault_naming_places_outside_the_notes_folder_writes_nothing_there() {
    let base = temp("outside");
    std::fs::create_dir_all(base.join("in")).unwrap();
    let mut two = Two::with_folders("outside", [Some(base.join("in/notes0")), Some(base.join("in/notes1"))]);
    // A picture of Carnet's, on the server: what the bad paths name.
    let photo: Vec<u8> = (0..20_000u32).map(|i| (i % 241) as u8).collect();
    let a = &mut two.sides[0].store;
    let src = a.save_attachment("", "png", &photo).unwrap();
    let mut n = crate::store::Note::new("Shown", Doc::new(vec![Block::text("a picture")]));
    n.doc.push(Block::new(carnet_core::doc::Kind::Image { src: src.clone(), alt: String::new(), float: Default::default(), width: None, wiki: false }));
    a.notes.push(n);
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    let id = crate::pictures::hex(&two.sides[0].session.blob_id(&photo));

    // Written as no Carnet writes them: straight into the vault's maps.
    let abs = base.join("abs").to_string_lossy().into_owned();
    let raw = loro::LoroDoc::new();
    raw.set_peer_id(0xbad).unwrap();
    let notes = raw.get_map("notes");
    let mut evil = Vec::new();
    for (title, folder, name) in [("Climbing", "../../outside", "Climbing"), ("Absolute", abs.as_str(), "Absolute"), ("Sideways", "", r"..\..\sideways")] {
        let k = Key::random();
        notes.insert(&format!("{k}/folder"), folder).unwrap();
        notes.insert(&format!("{k}/name"), name).unwrap();
        evil.push((k, title));
    }
    let folders = raw.get_map("folders");
    for f in ["../../outside-folder", abs.as_str(), r"a\b", "Fine/../../.."] {
        folders.insert(f, true).unwrap();
    }
    let pictures = raw.get_map("pictures");
    let abs_png = format!("{abs}.png");
    for p in ["attachments/../../../outside.png", abs_png.as_str(), r"attachments\..\..\..\sideways.png", "attachments/../in.png"] {
        pictures.insert(p, id.as_str()).unwrap();
    }
    raw.commit();
    let bytes = raw.export(loro::ExportMode::all_updates()).unwrap();
    let a = &mut two.sides[0];
    let r = a.store.replica.as_mut().unwrap();
    r.vault().import(&bytes).unwrap();
    r.persist_vault().unwrap();
    for (k, title) in &evil {
        r.record(*k, &format!("elsewhere/{title}.md"), &format!("# {title}\n\nfrom elsewhere\n"), carnet_sync::Origin::Save).unwrap();
        a.session.saved(&k.to_string(), r, &mut OsRandom);
    }
    a.session.saved(vault::ROOM, r, &mut OsRandom);
    for _ in 0..4 {
        two.pump();
    }

    let b = &two.sides[1];
    for (_, title) in &evil {
        let i = two.note(1, title).unwrap_or_else(|| panic!("{title} came: {:?}", two.shape(1)));
        assert_eq!(b.store.notes[i].path.as_deref(), Some(b.dir.join(format!("{title}.md")).as_path()), "at the top, named for its title");
    }
    assert!(b.store.folders.is_empty(), "{:?}", b.store.folders);
    assert_eq!(b.store.replica.as_ref().unwrap().vault().pictures().into_keys().collect::<Vec<_>>(), std::slice::from_ref(&src));
    assert_eq!(std::fs::read(b.dir.join(&src)).ok(), Some(photo), "the picture's own path is fetched");
    let mut outside: Vec<String> = std::fs::read_dir(&base).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    outside.sort();
    assert_eq!(outside, ["in"]);
    let mut beside: Vec<String> = std::fs::read_dir(base.join("in")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    beside.sort();
    assert_eq!(beside, ["notes0", "notes0.replica", "notes1", "notes1.replica"]);
    drop(two);
    let _ = std::fs::remove_dir_all(&base);
}

/// A file another device names in `attachments/` that isn't a picture by
/// its name (a program, a script, an app) is never written here, whatever
/// its bytes, and isn't sent from here either. A picture that comes is
/// marked on macOS as from elsewhere, as a download is; one made here
/// isn't.
#[test]
fn only_pictures_come_by_sync() {
    let mut two = Two::new("only-pictures");
    let photo: Vec<u8> = (0..20_000u32).map(|i| (i % 239) as u8).collect();
    let a = &mut two.sides[0].store;
    let src = a.save_attachment("", "png", &photo).unwrap();
    let mut n = crate::store::Note::new("Shown", Doc::new(vec![Block::text("a picture")]));
    n.doc.push(Block::new(carnet_core::doc::Kind::Image { src: src.clone(), alt: String::new(), float: Default::default(), width: None, wiki: false }));
    a.notes.push(n);
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    // Not a picture by its name: not sent.
    std::fs::write(a.dir.join("attachments/notes.txt"), b"plain words").unwrap();
    for _ in 0..4 {
        two.pump();
    }
    assert_eq!(std::fs::read(two.sides[1].dir.join(&src)).ok(), Some(photo.clone()));
    assert!(!two.sides[1].dir.join("attachments/notes.txt").exists());
    let id = crate::pictures::hex(&two.sides[0].session.blob_id(&photo));

    // The picture's id under names that would run, written as no Carnet
    // writes them.
    let raw = loro::LoroDoc::new();
    raw.set_peer_id(0xbad).unwrap();
    let pictures = raw.get_map("pictures");
    let bad = ["attachments/run.command", "attachments/Tool.app", "attachments/fix.sh", "attachments/setup.exe", "attachments/readme", "attachments/page.html"];
    for p in bad {
        pictures.insert(p, id.as_str()).unwrap();
    }
    raw.commit();
    let bytes = raw.export(loro::ExportMode::all_updates()).unwrap();
    let a = &mut two.sides[0];
    let r = a.store.replica.as_mut().unwrap();
    r.vault().import(&bytes).unwrap();
    r.persist_vault().unwrap();
    a.session.saved(vault::ROOM, r, &mut OsRandom);
    // And the picture asked for again on B, as when it's gone there.
    std::fs::remove_file(two.sides[1].dir.join(&src)).unwrap();
    for _ in 0..4 {
        two.pump();
    }
    let b = &two.sides[1];
    for p in bad {
        assert!(!b.dir.join(p).exists(), "{p} written");
    }
    let got = b.dir.join(&src);
    assert_eq!(std::fs::read(&got).ok(), Some(photo), "the picture itself comes");
    #[cfg(target_os = "macos")]
    {
        let mark = |p: &std::path::Path| {
            let out = std::process::Command::new("xattr").args(["-p", "com.apple.quarantine"]).arg(p).output().unwrap();
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        let m = mark(&got).expect("marked as from elsewhere");
        assert!(m.starts_with("0081;") && m.ends_with(";Carnet;"), "{m}");
        assert_eq!(mark(&two.sides[0].dir.join(&src)), None, "made here: not marked");
    }
}

/// A name a Mac allows and another system reads as more than a name
/// (`a:b`, `x\y`): sent as one it reads as a name, and the file here
/// follows, as one renamed elsewhere would. (Windows names no file so.)
#[cfg(unix)]
#[test]
fn a_name_only_a_mac_allows_goes_as_a_plain_one() {
    let mut two = Two::new("mac-names");
    two.make(0, "", "Ratio", "two to one");
    let a = &mut two.sides[0];
    let i = a.store.notes.iter().position(|n| n.title == "Ratio").unwrap();
    let from = a.store.notes[i].path.clone().unwrap();
    std::fs::rename(&from, a.dir.join(r"2:1 \ ratio.md")).unwrap();
    a.store.rescan();
    for _ in 0..4 {
        two.pump();
    }
    for side in 0..2 {
        let (notes, ..) = two.shape(side);
        assert!(notes.iter().any(|(_, f, _)| f == "2-1 - ratio.md"), "on {side}: {notes:?}");
        assert!(notes.iter().all(|(_, f, _)| !f.contains([':', '\\'])), "on {side}: {notes:?}");
    }
    let r = two.sides[1].store.replica.as_ref().unwrap();
    assert!(r.vault().layout().notes.values().any(|p| p.name == "2-1 - ratio"));
}

/// An invite made on one device: its code is in the vault, so the other
/// shows it too, and this one after a restart. Taken back on the other,
/// it's gone from the vault on both; used, it's gone once the server no
/// longer lists it (and a list from before it was made can't have).
#[test]
fn an_invites_code_is_known_on_every_device_until_done_with() {
    let mut two = Two::new("invites");
    let make = |two: &mut Two| {
        let (s, now) = (&mut two.sides[0], two.now);
        let code = s.session.invite(&mut OsRandom).expect("logged in");
        assert!(s.store.keep_invite(&code, now));
        two.pump();
        code
    };
    let list = |two: &mut Two, side: usize| {
        two.sides[side].session.list_invites();
        two.pump();
        two.sides[side].invites.clone().expect("a list")
    };
    let one = make(&mut two);
    assert_eq!(list(&mut two, 1), [one.hash()]);
    assert_eq!(two.sides[1].store.invite_code(&one.hash()), Some(one.text()), "shown on the other device");
    let restarted = Store::open(two.sides[0].dir.clone()).unwrap();
    assert_eq!(restarted.invite_code(&one.hash()), Some(one.text()), "and here after a restart");
    drop(restarted);

    // Taken back on the other device: forgotten on both.
    let s = &mut two.sides[1];
    s.session.drop_invite(one.hash());
    assert!(s.store.forget_invite(&one.hash()));
    two.pump();
    assert!(list(&mut two, 0).is_empty());
    for side in 0..2 {
        assert_eq!(two.sides[side].store.invite_code(&one.hash()), None, "gone from {side}'s vault");
    }

    // Used: the server lists it no more. Kept a while, for a list from
    // before it was made would not name it either. A list naming none is
    // no news (a server could wipe every code by it): kept still.
    let two_ = make(&mut two);
    two.hub.store_mut().drop_invite(two_.hash()).unwrap();
    assert!(list(&mut two, 1).is_empty());
    assert_eq!(two.sides[0].store.invite_code(&two_.hash()), Some(two_.text()));
    two.now += carnet_sync::vault::INVITE_GRACE;
    assert!(list(&mut two, 1).is_empty());
    for side in 0..2 {
        assert_eq!(two.sides[side].store.invite_code(&two_.hash()), Some(two_.text()), "kept in {side}'s vault");
    }
    // A list naming another: the used one forgotten, on both.
    let three = make(&mut two);
    assert_eq!(list(&mut two, 1), [three.hash()]);
    for side in 0..2 {
        assert_eq!(two.sides[side].store.invite_code(&two_.hash()), None, "gone from {side}'s vault");
        assert_eq!(two.sides[side].store.invite_code(&three.hash()), Some(three.text()));
    }
    // Run out: forgotten, whatever the server says.
    two.now += carnet_sync::vault::INVITE_LIFE;
    list(&mut two, 0);
    for side in 0..2 {
        assert_eq!(two.sides[side].store.invite_code(&three.hash()), None, "gone from {side}'s vault");
    }
}

/// The browser's folder changes, written into the vault as it writes them
/// (carnet-sync's `make_folder`, `rename_folder`, `move_folder`,
/// `move_note`, `set_pinned`, `trash_folder`), with no file moved: each
/// desktop's files follow. Here B writes them so, as the browser would
/// beside it, and both desktops end alike.
#[test]
fn the_browsers_folder_changes_reach_the_files() {
    let mut two = Two::new("web-folders");
    two.make(0, "Work", "Plan", "q3");
    two.make(0, "", "Loose", "ends");
    for _ in 0..2 {
        two.pump();
    }
    let key = |two: &Two, title: &str| two.sides[0].store.notes.iter().find(|n| n.title == title).and_then(|n| n.key).unwrap();
    let loose = key(&two, "Loose");
    let browser = |two: &mut Two, write: &dyn Fn(&carnet_sync::NoteDoc)| {
        let s = &mut two.sides[1];
        let r = s.store.replica.as_mut().unwrap();
        write(r.vault());
        s.store.vault_changed().unwrap();
        let r = s.store.replica.as_mut().unwrap();
        s.session.saved(vault::ROOM, r, &mut OsRandom);
        for _ in 0..3 {
            two.pump();
        }
    };

    browser(&mut two, &|v| {
        v.make_folder("", "Ideas").unwrap();
        v.rename_folder("Work", "Job").unwrap();
    });
    for side in 0..2 {
        let dir = &two.sides[side].dir;
        assert!(dir.join("Job/Plan.md").exists() && dir.join("Ideas").is_dir(), "on {side}: {:?}", two.shape(side));
        assert!(!dir.join("Work").exists(), "on {side}: the old one gone, empty");
    }
    assert_eq!(two.shape(0), two.shape(1));

    browser(&mut two, &|v| {
        v.move_folder("Job", "Ideas").unwrap();
        v.move_note(loose, "Ideas").unwrap();
        v.set_pinned(loose, true);
        assert_eq!(v.move_folder("Ideas", "Ideas/Job"), Err(vault::FolderRefusal::IntoItself));
    });
    for side in 0..2 {
        let dir = &two.sides[side].dir;
        assert!(dir.join("Ideas/Job/Plan.md").exists() && dir.join("Ideas/Loose.md").exists(), "on {side}: {:?}", two.shape(side));
        assert!(two.sides[side].store.notes.iter().find(|n| n.title == "Loose").is_some_and(|n| n.pinned), "pinned on {side}");
    }
    assert_eq!(two.shape(0), two.shape(1));

    // Deleted with its notes: to the trash on each, the folder gone.
    browser(&mut two, &|v| {
        let gone = v.trash_folder("Ideas");
        assert_eq!(gone.notes.len(), 2);
    });
    for side in 0..2 {
        let (notes, folders, trash) = two.shape(side);
        assert!(notes.iter().all(|(_, f, _)| f != "Plan.md" && f != "Loose.md"), "on {side}: {notes:?}");
        assert_eq!(trash, ["Loose", "Plan"], "on {side}");
        assert!(!folders.iter().any(|f| f.starts_with("Ideas")), "on {side}: {folders:?}");
        assert!(!two.sides[side].dir.join("Ideas").exists(), "on {side}");
    }
}

/// A note that came from another device is dated when it was last edited
/// there, not when it came: its file's modified time, which the list and
/// the page say and sort by, is its history's.
#[test]
fn a_synced_note_is_dated_when_it_was_edited_not_when_it_came() {
    let secs = |t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let mut two = Two::new("dated");
    let a = &mut two.sides[0].store;
    a.notes.push(crate::store::Note::new("Older", Doc::new(vec![Block::text("first")])));
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    let older = secs(a.notes[i].modified);
    std::thread::sleep(std::time::Duration::from_millis(2100));
    let a = &mut two.sides[0].store;
    a.notes.push(crate::store::Note::new("Newer", Doc::new(vec![Block::text("second")])));
    let i = a.notes.len() - 1;
    a.save(i, true).unwrap();
    let newer = secs(a.notes[i].modified);
    // Both come now, together, a while after the first was written.
    std::thread::sleep(std::time::Duration::from_millis(2100));
    two.pump();
    let came = secs(std::time::SystemTime::now());
    let b = &two.sides[1].store;
    let when = |t: &str| secs(b.notes[two.note(1, t).unwrap()].modified);
    assert!(when("Older").abs_diff(older) <= 1, "Older: {} (edited {older}, came {came})", when("Older"));
    assert!(when("Newer").abs_diff(newer) <= 1, "Newer: {} (edited {newer}, came {came})", when("Newer"));
    assert!(when("Older") + 2 <= came && when("Older") < when("Newer"), "sorted as edited, not as they came");
    // The file says so too, read again from disk.
    let path = b.notes[two.note(1, "Older").unwrap()].path.clone().unwrap();
    assert_eq!(secs(std::fs::metadata(&path).unwrap().modified().unwrap()), when("Older"));
    // An edit there later: dated then, here too.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    two.type_into(0, "Older", "more");
    let i = two.note(0, "Older").unwrap();
    two.sides[0].store.save(i, true).unwrap();
    let edited = secs(two.sides[0].store.notes[i].modified);
    two.pump();
    let b = &two.sides[1].store;
    let now_when = secs(b.notes[two.note(1, "Older").unwrap()].modified);
    assert!(now_when.abs_diff(edited) <= 1 && now_when > older, "{now_when} vs {edited}");
}
