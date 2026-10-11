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
    pictures: crate::pictures::Pictures,
}

struct Two {
    hub: Hub<Memory>,
    /// The picture worker, as the server runs one beside the hub.
    worker: carnet_server::blobs::Worker<carnet_server::blobs::MemoryBlobs>,
    sides: Vec<Side>,
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
                }
            })
            .collect();
        let mut two = Two { hub, worker: carnet_server::blobs::Worker::memory(), sides };
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
                s.store.reconcile(s.session.has_joined(vault::ROOM)).unwrap();
                if s.session.has_joined(vault::ROOM) {
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
                    let result = self.worker.blobs.serve(&t);
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

    two.sides[0].store.trash_folder("Job").unwrap();
    two.pump();
    assert!(!two.sides[1].dir.join("Job/Plan.md").exists());
    let mut trash = two.shape(1).2;
    trash.sort();
    assert_eq!(trash, ["Notes", "Plan"], "its notes in the trash there");
    assert!(!two.sides[1].store.folders.contains(&"Job".to_string()));
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

/// A picture beside its note, not in the attachments folder, goes too: to
/// the same path on the other device, and in the vault there, which is
/// where the browser looks for it. One no note shows stays here.
#[test]
fn a_picture_beside_its_note_follows_it() {
    let mut two = Two::new("beside");
    let hill: Vec<u8> = (0..40_000u32).map(|i| (i % 233) as u8).collect();
    two.make(0, "Home", "Weekend", "the view");
    let a = &mut two.sides[0];
    std::fs::write(a.dir.join("Home/hill.jpg"), &hill).unwrap();
    std::fs::write(a.dir.join("Home/unshown.jpg"), b"not shown").unwrap();
    let i = a.store.notes.iter().position(|n| n.title == "Weekend").unwrap();
    let n = &mut a.store.notes[i];
    n.doc.push(Block::new(carnet_core::doc::Kind::Image { src: "hill.jpg".into(), alt: "The hill at seven".into(), float: Default::default(), width: None, wiki: false }));
    n.touch();
    a.store.save(i, true).unwrap();
    a.store.rescan();
    for _ in 0..4 {
        two.pump();
    }
    let id = crate::pictures::hex(&two.sides[0].session.blob_id(&hill));
    for side in 0..2 {
        let vault = two.sides[side].store.replica.as_ref().unwrap().vault().pictures();
        assert_eq!(vault.get("Home/hill.jpg"), Some(&id), "in {side}'s vault");
        assert!(!vault.contains_key("Home/unshown.jpg"), "no note shows it");
    }
    assert_eq!(std::fs::read(two.sides[1].dir.join("Home/hill.jpg")).ok(), Some(hill));
    assert!(!two.sides[1].dir.join("Home/unshown.jpg").exists());
    assert!(two.file(1, "Weekend").contains("![The hill at seven](hill.jpg)"));
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
    n.touch();
    a.store.save(i, true).unwrap();
    a.store.rescan();
    for _ in 0..4 {
        first.pump();
    }
    let old = first.sides[0].session.blob_id(&hill);
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
    for (bytes, path) in [(&hill, "Home/hill.jpg"), (&photo, attached.as_str())] {
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
