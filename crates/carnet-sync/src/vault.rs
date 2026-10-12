//! The vault doc (docs/SYNC.md §4): where each note lives, and the
//! folders, for every device of an account. One more synced doc, in the
//! room [`ROOM`], beside the notes' own.
//!
//! Each note has four fields, each a value of its own in one map, keyed
//! `<note key>/<field>`: its folder, its file's name (without `.md`),
//! whether it is pinned, and whether it is in the trash (its folder and
//! name then say where it came from, and goes back to). A field written
//! on two devices at once keeps one of the two (Loro's map: the last
//! writer wins), field by field, so a note pinned on one device and moved
//! on another is both. Folders are a set: a path, there or not.
//!
//! A note deleted forever is marked gone (`<key>/gone`), never unmarked:
//! whatever else another device wrote of it meanwhile, it is gone, and
//! each device forgets it. A picture no note shows any more is dropped:
//! its path kept, holding `-<id>` where its blob's id was. The id stays so
//! that a note brought back showing it (an old version restored) can fetch
//! it again while the server still keeps it.
//!
//! What another device wrote is read as a name, a folder or a path from
//! the notes folder only when it is one on every system
//! ([`is_plain_path`]): no `..`, no full path, drive or share, no `\` or
//! `:`, no hidden name or device's. Anything else is read as not there,
//! so nothing past the vault ever joins it onto the notes folder; nor is
//! it written here.

use std::collections::{BTreeMap, BTreeSet};

use carnet_core::name::{is_plain_name, is_plain_path};
use loro::{LoroMap, LoroValue};

use crate::note::{Key, NoteDoc, Version};

/// The vault doc's room.
pub const ROOM: &str = "vault";

const NOTES: &str = "notes";
const FOLDERS: &str = "folders";
const PICTURES: &str = "pictures";

/// Where a note lives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Place {
    /// `/` between its parts; "" for the top of the notes folder.
    pub folder: String,
    /// The file's name, without `.md`.
    pub name: String,
    pub pinned: bool,
    pub trashed: bool,
}

impl Place {
    /// Whether its folder and name can be joined onto the notes folder:
    /// a plain path (or "" for the top), and a plain name.
    pub fn is_plain(&self) -> bool {
        (self.folder.is_empty() || is_plain_path(&self.folder)) && is_plain_name(&self.name)
    }
}

/// Every note's place and every folder, as the vault holds them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub notes: BTreeMap<Key, Place>,
    pub folders: BTreeSet<String>,
}

impl NoteDoc {
    fn notes_map(&self) -> LoroMap {
        self.loro().get_map(NOTES)
    }

    fn folders_map(&self) -> LoroMap {
        self.loro().get_map(FOLDERS)
    }

    /// One note's place, read directly: what a note arriving from another
    /// device is put by, without reading every note's.
    pub fn place(&self, key: Key) -> Option<Place> {
        let map = self.notes_map();
        let get = |field: &str| match map.get(&format!("{key}/{field}")) {
            Some(loro::ValueOrContainer::Value(v)) => Some(v),
            _ => None,
        };
        let text = |v: Option<LoroValue>| match v {
            Some(LoroValue::String(s)) => Some(s.to_string()),
            _ => None,
        };
        let flag = |v: Option<LoroValue>| matches!(v, Some(LoroValue::Bool(true)));
        if flag(get("gone")) {
            return None;
        }
        let name = text(get("name")).filter(|n| !n.is_empty())?;
        Some(Place { folder: text(get("folder")).unwrap_or_default(), name, pinned: flag(get("pinned")), trashed: flag(get("trashed")) })
            .filter(Place::is_plain)
    }

    /// The layout the vault holds now.
    pub fn layout(&self) -> Layout {
        read(&self.notes_map().get_deep_value(), &self.folders_map().get_deep_value())
    }

    /// Just the folders, without reading every note's place.
    pub fn folders(&self) -> BTreeSet<String> {
        read(&LoroValue::Null, &self.folders_map().get_deep_value()).folders
    }

    /// The layout as it was at `version` (an empty one for a version this
    /// doc doesn't have).
    pub fn layout_at(&self, version: &Version) -> Layout {
        match self.loro().fork_at(version.frontiers()) {
            Ok(old) => read(&old.get_map(NOTES).get_deep_value(), &old.get_map(FOLDERS).get_deep_value()),
            Err(_) => Layout::default(),
        }
    }

    /// `key`'s place becomes `place`: only the fields that differ are
    /// written, so a field this device didn't change doesn't overwrite
    /// another device's. True if anything was. One not
    /// [plain](Place::is_plain) isn't written.
    pub fn set_place(&self, key: Key, place: &Place) -> bool {
        if !place.is_plain() {
            return false;
        }
        let map = self.notes_map();
        let now = self.layout().notes.get(&key).cloned();
        let mut wrote = false;
        let mut put = |field: &str, v: LoroValue| {
            let _ = map.insert(&format!("{key}/{field}"), v);
            wrote = true;
        };
        if now.as_ref().is_none_or(|p| p.folder != place.folder) {
            put("folder", place.folder.clone().into());
        }
        if now.as_ref().is_none_or(|p| p.name != place.name) {
            put("name", place.name.clone().into());
        }
        if now.as_ref().is_none_or(|p| p.pinned != place.pinned) {
            put("pinned", place.pinned.into());
        }
        if now.as_ref().is_none_or(|p| p.trashed != place.trashed) {
            put("trashed", place.trashed.into());
        }
        if wrote {
            self.loro().commit();
        }
        wrote
    }

    /// The notes deleted forever.
    pub fn gone(&self) -> BTreeSet<Key> {
        match self.notes_map().get_deep_value() {
            LoroValue::Map(m) => m
                .iter()
                .filter(|(_, v)| matches!(v, LoroValue::Bool(true)))
                .filter_map(|(k, _)| k.strip_suffix("/gone").and_then(Key::parse))
                .collect(),
            _ => BTreeSet::new(),
        }
    }

    /// `key` deleted forever, on every device. True if it wasn't.
    pub fn set_gone(&self, key: Key) -> bool {
        let field = format!("{key}/gone");
        if matches!(self.notes_map().get(&field), Some(loro::ValueOrContainer::Value(LoroValue::Bool(true)))) {
            return false;
        }
        let _ = self.notes_map().insert(&field, true);
        self.loro().commit();
        true
    }

    fn picture_map(&self) -> BTreeMap<String, String> {
        match self.loro().get_map(PICTURES).get_deep_value() {
            LoroValue::Map(m) => m
                .iter()
                .filter_map(|(k, v)| match v {
                    LoroValue::String(s) if is_plain_path(k) => Some((k.clone(), s.to_string())),
                    _ => None,
                })
                .collect(),
            _ => BTreeMap::new(),
        }
    }

    /// The pictures: each path in the notes folder (`attachments/…`) and
    /// the id of its blob, in hex.
    pub fn pictures(&self) -> BTreeMap<String, String> {
        let mut all = self.picture_map();
        all.retain(|_, id| live(id));
        all
    }

    /// The paths of pictures dropped: no note showed them any more.
    pub fn dropped_pictures(&self) -> BTreeSet<String> {
        self.picture_map().into_iter().filter(|(_, id)| !live(id)).map(|(p, _)| p).collect()
    }

    /// The id a dropped picture had, if it's known: what fetches it back.
    pub fn dropped_id(&self, path: &str) -> Option<String> {
        self.picture_map().get(path).and_then(|id| id.strip_prefix('-')).filter(|id| !id.is_empty()).map(str::to_string)
    }

    /// A picture at `path`, its blob `id` (hex). True if that changed.
    pub fn set_picture(&self, path: &str, id: &str) -> bool {
        self.put_picture(path, id).0
    }

    /// [`set_picture`](NoteDoc::set_picture), and the blob it put out of
    /// use: the one `path` held before, if no other path holds it now. That
    /// one the server may drop (docs/SYNC.md §6).
    pub fn put_picture(&self, path: &str, id: &str) -> (bool, Option<String>) {
        if !is_plain_path(path) {
            return (false, None);
        }
        let all = self.picture_map();
        let old = all.get(path).filter(|o| live(o) && o.as_str() != id).cloned();
        if all.get(path).map(String::as_str) == Some(id) {
            return (false, None);
        }
        let _ = self.loro().get_map(PICTURES).insert(path, id);
        self.loro().commit();
        let unused = old.filter(|o| !all.iter().any(|(p, v)| p != path && v == o));
        (true, unused)
    }

    /// The picture at `path` dropped, its id kept beside (see
    /// [`put_picture`](NoteDoc::put_picture) for what comes back).
    pub fn drop_picture(&self, path: &str) -> (bool, Option<String>) {
        match self.picture_map().get(path).filter(|id| live(id)).cloned() {
            Some(id) => self.put_picture(path, &format!("-{id}")),
            None => (false, None),
        }
    }

    /// A folder there, or not. True if that changed.
    pub fn set_folder(&self, folder: &str, there: bool) -> bool {
        if !is_plain_path(folder) || self.layout().folders.contains(folder) == there {
            return false;
        }
        let _ = self.folders_map().insert(folder, there);
        self.loro().commit();
        true
    }
}

/// A picture's value in the map that names one shown: not dropped.
fn live(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('-')
}

fn read(notes: &LoroValue, folders: &LoroValue) -> Layout {
    let mut out = Layout::default();
    let mut gone = BTreeSet::new();
    if let LoroValue::Map(m) = notes {
        for (k, v) in m.iter() {
            let Some((key, field)) = k.split_once('/') else { continue };
            let Some(key) = Key::parse(key) else { continue };
            let p = out.notes.entry(key).or_default();
            match (field, v) {
                ("folder", LoroValue::String(s)) => p.folder = s.to_string(),
                ("name", LoroValue::String(s)) => p.name = s.to_string(),
                ("pinned", LoroValue::Bool(b)) => p.pinned = *b,
                ("trashed", LoroValue::Bool(b)) => p.trashed = *b,
                ("gone", LoroValue::Bool(true)) => {
                    gone.insert(key);
                }
                _ => {}
            }
        }
    }
    out.notes.retain(|k, _| !gone.contains(k));
    // A note with no name yet (only some of its fields arrived) has no
    // place to put it; nor has one whose place isn't plain.
    out.notes.retain(|_, p| !p.name.is_empty() && p.is_plain());
    if let LoroValue::Map(m) = folders {
        out.folders = m.iter().filter(|(k, v)| matches!(v, LoroValue::Bool(true)) && is_plain_path(k)).map(|(k, _)| k.clone()).collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(folder: &str, name: &str) -> Place {
        Place { folder: folder.into(), name: name.into(), pinned: false, trashed: false }
    }

    /// Two devices change one note at once, different fields: both stay.
    /// The same field: one of the two, the same on both.
    #[test]
    fn fields_merge_one_by_one() {
        let (a, b) = (NoteDoc::new(1), NoteDoc::new(2));
        let k = Key::random();
        assert!(a.set_place(k, &place("Work", "Plan")));
        assert!(!a.set_place(k, &place("Work", "Plan")), "nothing new, nothing written");
        a.set_folder("Work", true);
        b.import(&a.updates()).unwrap();
        let before = a.version();

        a.set_place(k, &Place { pinned: true, ..place("Work", "Plan") });
        b.set_place(k, &place("Home", "Plan"));
        b.set_folder("Home", true);
        b.set_folder("Work", false);
        a.import(&b.updates()).unwrap();
        b.import(&a.updates()).unwrap();
        assert_eq!(a.layout(), b.layout());
        let p = &a.layout().notes[&k];
        assert_eq!((p.folder.as_str(), p.pinned), ("Home", true), "moved there, pinned here: both");
        assert_eq!(a.layout().folders.iter().collect::<Vec<_>>(), ["Home"]);

        assert_eq!(a.place(k), Some(a.layout().notes[&k].clone()));
        assert_eq!(a.place(Key::random()), None);

        // Deleted forever on one, moved on the other at once: gone on
        // both, for good.
        let k2 = Key::random();
        a.set_place(k2, &place("", "Old"));
        b.import(&a.updates()).unwrap();
        assert!(a.set_gone(k2) && !a.set_gone(k2));
        b.set_place(k2, &place("Work", "Old"));
        a.import(&b.updates()).unwrap();
        b.import(&a.updates()).unwrap();
        for d in [&a, &b] {
            assert!(!d.layout().notes.contains_key(&k2) && d.place(k2).is_none());
            assert_eq!(d.gone().into_iter().collect::<Vec<_>>(), [k2]);
        }

        // As it was, before either.
        assert_eq!(a.layout_at(&before).notes[&k], place("Work", "Plan"));
        assert!(a.layout_at(&before).folders.contains("Work"));
    }

    /// A picture put over another, or dropped: the one it held comes back
    /// if nothing else holds it, for the server to drop.
    #[test]
    fn a_picture_put_out_of_use_is_said() {
        let v = NoteDoc::new(1);
        assert_eq!(v.put_picture("attachments/a.png", "aa"), (true, None));
        assert_eq!(v.put_picture("attachments/b.png", "aa"), (true, None));
        assert_eq!(v.put_picture("attachments/a.png", "aa"), (false, None), "the same: nothing");
        assert_eq!(v.put_picture("attachments/a.png", "cc"), (true, None), "aa still at b");
        assert_eq!(v.put_picture("attachments/b.png", "dd"), (true, Some("aa".into())));
        assert_eq!(v.drop_picture("attachments/a.png"), (true, Some("cc".into())));
        assert_eq!(v.drop_picture("attachments/a.png"), (false, None));
        assert_eq!(v.pictures().into_iter().collect::<Vec<_>>(), [("attachments/b.png".to_string(), "dd".to_string())]);
        assert_eq!(v.dropped_pictures().into_iter().collect::<Vec<_>>(), ["attachments/a.png"]);
        assert_eq!(v.dropped_id("attachments/a.png").as_deref(), Some("cc"), "its id kept, to fetch it back");
        assert_eq!(v.drop_picture("attachments/nowhere.png"), (false, None), "nothing there: nothing to drop");
        // Put back: there again.
        v.set_picture("attachments/a.png", "cc");
        assert!(v.dropped_pictures().is_empty());
    }

    /// A place, a folder or a picture's path another device wrote that
    /// reaches out of the notes folder (or names a drive, a share, a
    /// device) is read as not there; nor is one written here.
    #[test]
    fn what_reaches_outside_is_not_there() {
        let v = NoteDoc::new(1);
        let (good, bad_folder, bad_name) = (Key::random(), Key::random(), Key::random());
        v.set_place(good, &place("Work", "Plan"));
        assert!(!v.set_place(bad_name, &place("", "../../x")), "not written here");
        // As a hostile or older device would write them: straight into
        // the maps.
        let notes = v.notes_map();
        for (key, folder, name) in [(bad_folder, "../../Library", "x"), (bad_name, "", r"..\..\x")] {
            notes.insert(&format!("{key}/folder"), folder).unwrap();
            notes.insert(&format!("{key}/name"), name).unwrap();
        }
        let folders = v.folders_map();
        for f in ["Work", "/etc", "..", r"a\b", "C:", "Work/../.."] {
            folders.insert(f, true).unwrap();
        }
        let pictures = v.loro().get_map(PICTURES);
        for p in ["attachments/a.png", "attachments/../../../Library/LaunchAgents/x.plist", "/abs.png", r"attachments\..\x.png"] {
            pictures.insert(p, "aa").unwrap();
        }
        pictures.insert("attachments/../gone.png", "-bb").unwrap();
        v.loro().commit();

        assert_eq!(v.layout().notes.keys().collect::<Vec<_>>(), [&good]);
        assert_eq!((v.place(bad_folder), v.place(bad_name)), (None, None));
        assert_eq!(v.folders().into_iter().collect::<Vec<_>>(), ["Work"]);
        assert_eq!(v.pictures().into_keys().collect::<Vec<_>>(), ["attachments/a.png"]);
        assert!(v.dropped_pictures().is_empty() && v.dropped_id("attachments/../gone.png").is_none());
        assert!(!v.set_folder("../x", true) && !v.set_picture("/x.png", "cc"));
        // A bad place is put right as any is written over: by a device
        // writing its own.
        assert!(v.set_place(bad_name, &place("", "Found")));
        assert_eq!(v.place(bad_name), Some(place("", "Found")));
    }
}
