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
//! each device forgets it. A file no note shows any more is dropped:
//! its path kept, holding `-<id>` where its blob's id was. The id stays so
//! that a note brought back showing it (an old version restored) can fetch
//! it again while the server still keeps it.
//!
//! The invites this account made (docs/SYNC.md §6) are kept here too,
//! code and all, by the code's hash: the server keeps only the hash, so
//! a code read off the server would let no one in, but each of the
//! account's devices can show the code again, after a restart too. One
//! taken back, used or run out is forgotten: the server's list of those
//! waiting says which.
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

use crate::invite::InviteCode;
use crate::note::{Key, NoteDoc, Version};

/// The vault doc's room.
pub const ROOM: &str = "vault";

const NOTES: &str = "notes";
const FOLDERS: &str = "folders";
/// The pictures' paths. Named so in every vault there is: what a file is
/// called changed, this key can't (a device would read no file at all).
const PICTURES: &str = "pictures";
/// The documents a note links to (a PDF, a recording), kept as pictures
/// are but in a map of their own: a Carnet from before them reads only
/// [`PICTURES`], and lets go there every path it doesn't take for a
/// picture's — which, there, would drop them on every device.
const FILES: &str = "files";
const INVITES: &str = "invites";

/// How long an invite made is kept here though the server doesn't list
/// it: its list may have been on its way as the invite was made, or the
/// clocks of the devices disagree a little.
pub const INVITE_GRACE: u64 = 3600;

/// How long an invite lasts (the server's `INVITE_DAYS`): past it, its
/// code is no use, and forgotten here whatever the server says.
pub const INVITE_LIFE: u64 = 7 * 86_400;

/// An invite this account made, as the vault keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invite {
    pub code: InviteCode,
    /// When, in seconds since 1970, by the clock of the device that made it.
    pub made: u64,
}

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

    /// The layout, and the notes deleted forever ([`NoteDoc::gone`]), read
    /// in one go.
    pub fn layout_and_gone(&self) -> (Layout, BTreeSet<Key>) {
        read_gone(&self.notes_map().get_deep_value(), &self.folders_map().get_deep_value())
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
        let wrote = self.put_place(key, place);
        if wrote {
            self.loro().commit();
        }
        wrote
    }

    /// [`set_place`](NoteDoc::set_place) without the commit: one of
    /// several changes made as one.
    fn put_place(&self, key: Key, place: &Place) -> bool {
        if !place.is_plain() {
            return false;
        }
        let map = self.notes_map();
        // This note's fields alone: reading every note's for each one set
        // made 10,000 notes take minutes.
        let now = self.place(key);
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

    /// The pictures' map and the documents' as one: each path in the map
    /// its name puts it in ([`map_of`]), so a path is never in both.
    fn file_map(&self) -> BTreeMap<String, String> {
        let read = |name: &str| match self.loro().get_map(name).get_deep_value() {
            LoroValue::Map(m) => m
                .iter()
                .filter_map(|(k, v)| match v {
                    LoroValue::String(s) if is_plain_path(k) && map_of(k) == name => Some((k.clone(), s.to_string())),
                    _ => None,
                })
                .collect(),
            _ => BTreeMap::new(),
        };
        let mut all = read(PICTURES);
        all.extend(read(FILES));
        all
    }

    /// The files: each path in the notes folder (`attachments/…`) and
    /// the id of its blob, in hex.
    pub fn files(&self) -> BTreeMap<String, String> {
        let mut all = self.file_map();
        all.retain(|_, id| live(id));
        all
    }

    /// The paths of files dropped: no note showed them any more.
    pub fn dropped_files(&self) -> BTreeSet<String> {
        self.file_map().into_iter().filter(|(_, id)| !live(id)).map(|(p, _)| p).collect()
    }

    /// The id a dropped file had, if it's known: what fetches it back.
    pub fn dropped_id(&self, path: &str) -> Option<String> {
        self.file_map().get(path).and_then(|id| id.strip_prefix('-')).filter(|id| !id.is_empty()).map(str::to_string)
    }

    /// A file at `path`, its blob `id` (hex). True if that changed.
    pub fn set_file(&self, path: &str, id: &str) -> bool {
        self.put_file(path, id).0
    }

    /// [`set_file`](NoteDoc::set_file), and the blob it put out of
    /// use: the one `path` held before, if no other path holds it now. That
    /// one the server may drop (docs/SYNC.md §6).
    pub fn put_file(&self, path: &str, id: &str) -> (bool, Option<String>) {
        if !is_plain_path(path) {
            return (false, None);
        }
        let all = self.file_map();
        let old = all.get(path).filter(|o| live(o) && o.as_str() != id).cloned();
        if all.get(path).map(String::as_str) == Some(id) {
            return (false, None);
        }
        let _ = self.loro().get_map(map_of(path)).insert(path, id);
        self.loro().commit();
        let unused = old.filter(|o| !all.iter().any(|(p, v)| p != path && v == o));
        (true, unused)
    }

    /// The file at `path` dropped, its id kept beside (see
    /// [`put_file`](NoteDoc::put_file) for what comes back).
    pub fn drop_file(&self, path: &str) -> (bool, Option<String>) {
        match self.file_map().get(path).filter(|id| live(id)).cloned() {
            Some(id) => self.put_file(path, &format!("-{id}")),
            None => (false, None),
        }
    }

    /// The invites this account made and hasn't forgotten, by their
    /// codes' hashes. An entry whose code isn't one, or doesn't hash to
    /// its key, is read as not there.
    pub fn invites(&self) -> BTreeMap<[u8; 32], Invite> {
        let LoroValue::Map(m) = self.loro().get_map(INVITES).get_deep_value() else { return BTreeMap::new() };
        m.iter()
            .filter_map(|(k, v)| {
                let LoroValue::Map(v) = v else { return None };
                let code = match v.get("code") {
                    Some(LoroValue::String(s)) => InviteCode::parse(s)?,
                    _ => return None,
                };
                let made = match v.get("made") {
                    Some(LoroValue::I64(n)) => u64::try_from(*n).unwrap_or(0),
                    _ => 0,
                };
                (*k == hex(&code.hash())).then(|| (code.hash(), Invite { code, made }))
            })
            .collect()
    }

    /// The code of the invite whose hash is `hash`, if this account made it.
    pub fn invite_code(&self, hash: &[u8; 32]) -> Option<InviteCode> {
        self.invites().get(hash).map(|i| i.code)
    }

    /// An invite made, kept for every device of the account. True if it
    /// wasn't.
    pub fn keep_invite(&self, code: &InviteCode, made: u64) -> bool {
        let key = hex(&code.hash());
        if self.invites().contains_key(&code.hash()) {
            return false;
        }
        let value: loro::LoroMapValue = vec![("code".to_string(), LoroValue::from(code.text())), ("made".to_string(), LoroValue::I64(i64::try_from(made).unwrap_or(i64::MAX)))].into();
        let _ = self.loro().get_map(INVITES).insert(&key, LoroValue::Map(value));
        self.loro().commit();
        true
    }

    /// The invite whose hash is `hash` forgotten: taken back. True if it
    /// was kept.
    pub fn forget_invite(&self, hash: &[u8; 32]) -> bool {
        let key = hex(hash);
        let map = self.loro().get_map(INVITES);
        if map.get(&key).is_none() {
            return false;
        }
        let _ = map.delete(&key);
        self.loro().commit();
        true
    }

    /// The server listed `waiting` as the invites still waiting. One kept
    /// here past its life ([`INVITE_LIFE`]) is forgotten, listed or not.
    /// One the list leaves out (used, taken back elsewhere) is forgotten
    /// only if the list names some, and it was made more than
    /// [`INVITE_GRACE`] before `now`. An empty list is no news: every
    /// device of the account holds these codes, and a server that wanted
    /// them gone could otherwise wipe them all by saying nothing. A used
    /// one then stays until it runs out, though never shown, as only the
    /// listed are. True if any was forgotten.
    pub fn forget_invites_but(&self, waiting: &[[u8; 32]], now: u64) -> bool {
        let map = self.loro().get_map(INVITES);
        let LoroValue::Map(m) = map.get_deep_value() else { return false };
        let kept: BTreeMap<String, ([u8; 32], Invite)> = self.invites().into_iter().map(|(h, i)| (hex(&h), (h, i))).collect();
        let mut gone = false;
        for k in m.keys() {
            let keep = kept.get(k).is_some_and(|(h, i)| {
                let alive = i.made.saturating_add(INVITE_LIFE) > now;
                let new = i.made.saturating_add(INVITE_GRACE) > now;
                alive && (waiting.is_empty() || waiting.contains(h) || new)
            });
            if !keep {
                let _ = map.delete(k);
                gone = true;
            }
        }
        if gone {
            self.loro().commit();
        }
        gone
    }

    /// Every invite kept here forgotten (taken back on the server too, by
    /// the caller): a device removed from the account had their codes.
    /// Their hashes, to take back.
    pub fn forget_all_invites(&self) -> Vec<[u8; 32]> {
        let map = self.loro().get_map(INVITES);
        let hashes: Vec<[u8; 32]> = self.invites().into_keys().collect();
        if map.is_empty() {
            return hashes;
        }
        let _ = map.clear();
        self.loro().commit();
        hashes
    }

    /// A folder there, or not. True if that changed.
    pub fn set_folder(&self, folder: &str, there: bool) -> bool {
        let wrote = self.put_folder(folder, there);
        if wrote {
            self.loro().commit();
        }
        wrote
    }

    /// [`set_folder`](NoteDoc::set_folder) without the commit.
    fn put_folder(&self, folder: &str, there: bool) -> bool {
        if !is_plain_path(folder) || self.folders().contains(folder) == there {
            return false;
        }
        let _ = self.folders_map().insert(folder, there);
        true
    }

    /// A new folder named `name` ([`folder_name`]) inside `parent` (""
    /// for the top), and the folders above it, should one have gone
    /// meanwhile. One of that name there already, case aside, is that
    /// one. Its path.
    pub fn make_folder(&self, parent: &str, name: &str) -> Result<String, FolderRefusal> {
        let name = folder_name(name)?;
        if !parent.is_empty() && !is_plain_path(parent) {
            return Err(FolderRefusal::Gone);
        }
        let path = join(parent, &name);
        let low = path.to_lowercase();
        if let Some(f) = self.folders().into_iter().find(|f| f.to_lowercase() == low) {
            return Ok(f);
        }
        let mut wrote = false;
        for f in with_parents(&path) {
            wrote |= self.put_folder(&f, true);
        }
        if wrote {
            self.loro().commit();
        }
        Ok(path)
    }

    /// `folder` renamed `name` ([`folder_name`]) where it is, the folders
    /// and notes inside it going with it ([`relocate`](Self::relocate)).
    /// The same name but for case is the same folder, renamed; a name
    /// another folder there has is refused.
    pub fn rename_folder(&self, folder: &str, name: &str) -> Result<Relocated, FolderRefusal> {
        let name = folder_name(name)?;
        self.relocate(folder, &join(parent_of(folder), &name))
    }

    /// `folder` into `parent` ("" for the top) under its own name, the
    /// folders and notes inside it going with it. Never into itself or a
    /// folder inside it ([`may_go`]), nor into a folder gone, nor where a
    /// folder of its name is.
    pub fn move_folder(&self, folder: &str, parent: &str) -> Result<Relocated, FolderRefusal> {
        if is_inside(parent, folder) {
            return Err(FolderRefusal::IntoItself);
        }
        if !parent.is_empty() && !self.folders().contains(parent) {
            return Err(FolderRefusal::Gone);
        }
        self.relocate(folder, &join(parent, leaf_of(folder)))
    }

    /// `folder` to `to`, as one change: it and each folder inside it, and
    /// every note in them. A note in Deleted keeps the place it goes back
    /// to, as the desktop's trash keeps where a note came from.
    fn relocate(&self, folder: &str, to: &str) -> Result<Relocated, FolderRefusal> {
        let folders = self.folders();
        if !folders.contains(folder) {
            return Err(FolderRefusal::Gone);
        }
        if to == folder {
            return Ok(Relocated { to: to.to_string(), notes: Vec::new() });
        }
        if !is_plain_path(to) {
            return Err(FolderRefusal::Gone);
        }
        let low = to.to_lowercase();
        if folders.iter().any(|f| f.to_lowercase() == low && f != folder) {
            return Err(FolderRefusal::Taken(leaf_of(to).to_string()));
        }
        for f in &folders {
            if let Some(new) = relocated(f, folder, to) {
                self.put_folder(f, false);
                self.put_folder(&new, true);
            }
        }
        let mut notes = Vec::new();
        for (key, p) in self.layout().notes {
            if p.trashed {
                continue;
            }
            if let Some(new) = relocated(&p.folder, folder, to) {
                self.put_place(key, &Place { folder: new, ..p.clone() });
                notes.push((key, p.folder));
            }
        }
        self.loro().commit();
        Ok(Relocated { to: to.to_string(), notes })
    }

    /// Whether `folder` has nothing in it or under it: no note (one in
    /// Deleted is the trash's), no file the vault names. Only such a
    /// one is just deleted; any other goes to Deleted, whole.
    pub fn folder_is_bare(&self, folder: &str) -> bool {
        let layout = self.layout();
        !layout.notes.values().any(|p| !p.trashed && is_inside(&p.folder, folder)) && !self.files().keys().any(|p| is_inside(parent_of(p), folder))
    }

    /// `folder` and each folder inside it gone, and the notes in them to
    /// Deleted, unpinned, as the desktop's trash takes a folder whole: each
    /// goes back to the folder it was in. What went, for
    /// [`put_back_folder`](Self::put_back_folder).
    pub fn trash_folder(&self, folder: &str) -> TrashedFolder {
        let mut gone = TrashedFolder::default();
        for f in self.folders() {
            if is_inside(&f, folder) && self.put_folder(&f, false) {
                gone.folders.push(f);
            }
        }
        for (key, p) in self.layout().notes {
            if !p.trashed && is_inside(&p.folder, folder) && self.put_place(key, &Place { trashed: true, pinned: false, ..p }) {
                gone.notes.push(key);
            }
        }
        if !gone.folders.is_empty() || !gone.notes.is_empty() {
            self.loro().commit();
        }
        gone
    }

    /// A folder [trashed](Self::trash_folder) put back: its folders, and
    /// its notes out of Deleted. True if anything was.
    pub fn put_back_folder(&self, gone: &TrashedFolder) -> bool {
        let mut wrote = false;
        for f in &gone.folders {
            wrote |= self.put_folder(f, true);
        }
        for key in &gone.notes {
            if let Some(p) = self.place(*key).filter(|p| p.trashed) {
                wrote |= self.put_place(*key, &Place { trashed: false, ..p });
            }
        }
        if wrote {
            self.loro().commit();
        }
        wrote
    }

    /// Note `key` into `folder` ("" for the top): its folder before, or
    /// `None` if it's in Deleted, not here, there already, or the folder
    /// is gone.
    pub fn move_note(&self, key: Key, folder: &str) -> Option<String> {
        let p = self.place(key).filter(|p| !p.trashed && p.folder != folder)?;
        if !folder.is_empty() && !self.folders().contains(folder) {
            return None;
        }
        let from = p.folder.clone();
        self.set_place(key, &Place { folder: folder.to_string(), ..p }).then_some(from)
    }

    /// Note `key` pinned, or not. True if that changed.
    pub fn set_pinned(&self, key: Key, pinned: bool) -> bool {
        match self.place(key) {
            Some(p) if p.pinned != pinned => self.set_place(key, &Place { pinned, ..p }),
            _ => false,
        }
    }
}

/// The folder files are kept in, at the top of the notes folder: no
/// folder of notes may be called so, at any depth (made, it would be read
/// as one of files, its notes not listed).
pub const ATTACHMENTS: &str = "attachments";

/// Why a folder wasn't made, renamed or moved, in the desktop's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderRefusal {
    /// Into itself, or a folder inside it.
    IntoItself,
    /// It, or the folder it was to go into, isn't there any more.
    Gone,
    /// A folder of that name (this one) is there already.
    Taken(String),
    /// The name files' folder has ([`ATTACHMENTS`]).
    ForAttachments,
}

impl std::fmt::Display for FolderRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FolderRefusal::IntoItself => f.write_str("a folder can't go inside itself"),
            FolderRefusal::Gone => f.write_str("that folder isn't there any more"),
            FolderRefusal::Taken(name) => write!(f, "there is a folder called \u{201c}{name}\u{201d} there already"),
            FolderRefusal::ForAttachments => f.write_str("that name is kept for the notes' attachments; choose another"),
        }
    }
}

impl std::error::Error for FolderRefusal {}

/// A folder renamed or moved: its new path, and the notes that went with
/// it, each with the folder it was in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Relocated {
    pub to: String,
    pub notes: Vec<(Key, String)>,
}

/// A folder moved to Deleted whole: the folders gone, and the notes put
/// in Deleted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrashedFolder {
    pub folders: Vec<String>,
    pub notes: Vec<Key>,
}

/// A folder's name as typed, as it is kept: one plain name, made so as a
/// title is ([`slug`](carnet_core::name::slug)). The files' folder's
/// name is refused.
pub fn folder_name(typed: &str) -> Result<String, FolderRefusal> {
    let name = carnet_core::name::slug(typed);
    if name.eq_ignore_ascii_case(ATTACHMENTS) {
        return Err(FolderRefusal::ForAttachments);
    }
    Ok(name)
}

/// The map a file's path is kept in: a picture's by a picture's name,
/// where a Carnet from before files reads it (and lets go what it can't
/// take); any other file's ([`FILES`]). What syncs there grew (any file,
/// where once only a PDF, a sheet, a recording): a Carnet that knows the
/// fewer kinds reads one of the others there as written in the wrong map
/// (its own reading puts it in the pictures'), so as not there, and
/// neither fetches nor lets it go.
fn map_of(path: &str) -> &'static str {
    if carnet_core::doc::is_picture_name(path) { PICTURES } else { FILES }
}

/// The folder `folder` is in ("" for the top).
pub fn parent_of(folder: &str) -> &str {
    folder.rsplit_once('/').map_or("", |(p, _)| p)
}

/// A folder's own name: the last part of its path.
pub fn leaf_of(folder: &str) -> &str {
    folder.rsplit('/').next().unwrap_or(folder)
}

/// Whether `path` is `folder` or inside it.
pub fn is_inside(path: &str, folder: &str) -> bool {
    !folder.is_empty() && (path == folder || path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/')))
}

/// Whether `folder` may go into `to` ("" the top): never into itself or a
/// folder inside it, and not where it is already.
pub fn may_go(folder: &str, to: &str) -> bool {
    !is_inside(to, folder) && parent_of(folder) != to
}

/// `path` (a folder, or a note's) after `folder` went to `to`; `None` if
/// it isn't `folder` or inside it.
pub fn relocated(path: &str, folder: &str, to: &str) -> Option<String> {
    if !is_inside(path, folder) {
        return None;
    }
    Some(format!("{to}{}", &path[folder.len()..]))
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") }
}

/// `path` and each folder above it, the topmost first.
fn with_parents(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in path.split('/') {
        out.push(match out.last() {
            Some(above) => format!("{above}/{part}"),
            None => part.to_string(),
        });
    }
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A file's value in the map that names one shown: not dropped.
fn live(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('-')
}

fn read(notes: &LoroValue, folders: &LoroValue) -> Layout {
    read_gone(notes, folders).0
}

fn read_gone(notes: &LoroValue, folders: &LoroValue) -> (Layout, BTreeSet<Key>) {
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
    (out, gone)
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

    /// Folders made, renamed, moved and moved to Deleted, each as one
    /// change, as the desktop does them to its files: the folders and
    /// notes inside go with them, never into itself, never onto a name
    /// taken; a note in Deleted keeps where it goes back to.
    #[test]
    fn folders_take_what_is_in_them_along() {
        let v = NoteDoc::new(1);
        let folders = |v: &NoteDoc| v.folders().into_iter().collect::<Vec<_>>();
        assert_eq!(v.make_folder("", "Work"), Ok("Work".into()));
        assert_eq!(v.make_folder("Work", "Q4: plans"), Ok("Work/Q4- plans".into()), "a plain name, as a title's");
        assert_eq!(v.make_folder("", "work"), Ok("Work".into()), "case aside, the one there");
        assert_eq!(v.make_folder("Work", "Attachments"), Err(FolderRefusal::ForAttachments));
        assert_eq!(v.make_folder("Deep/Er", "X"), Ok("Deep/Er/X".into()), "the folders above it too");
        assert_eq!(folders(&v), ["Deep", "Deep/Er", "Deep/Er/X", "Work", "Work/Q4- plans"]);

        let (plan, q4, loose, old) = (Key::random(), Key::random(), Key::random(), Key::random());
        v.set_place(plan, &Place { pinned: true, ..place("Work", "Plan") });
        v.set_place(q4, &place("Work/Q4- plans", "Q4"));
        v.set_place(loose, &place("", "Loose"));
        v.set_place(old, &Place { trashed: true, ..place("Work", "Old") });

        // Renamed: what's inside with it, in one change.
        let before = v.version();
        let r = v.rename_folder("Work", "Job").unwrap();
        assert_eq!(r.to, "Job");
        let mut moved = r.notes.clone();
        moved.sort();
        let mut want = vec![(plan, "Work".to_string()), (q4, "Work/Q4- plans".to_string())];
        want.sort();
        assert_eq!(moved, want);
        assert_eq!(v.version().frontiers().len(), 1);
        assert_ne!(v.version(), before);
        assert_eq!(folders(&v), ["Deep", "Deep/Er", "Deep/Er/X", "Job", "Job/Q4- plans"]);
        assert_eq!(v.place(plan), Some(Place { pinned: true, ..place("Job", "Plan") }), "pinned still");
        assert_eq!(v.place(q4).unwrap().folder, "Job/Q4- plans");
        assert_eq!(v.place(old).unwrap().folder, "Work", "in Deleted: goes back where it came from");
        assert_eq!(v.rename_folder("Job", "job").unwrap().to, "job", "case alone: the same folder, renamed");
        v.rename_folder("job", "Job").unwrap();

        // Never into itself, or a folder inside it; never onto a name taken.
        assert_eq!(v.move_folder("Job", "Job"), Err(FolderRefusal::IntoItself));
        assert_eq!(v.move_folder("Job", "Job/Q4- plans"), Err(FolderRefusal::IntoItself));
        v.make_folder("Deep", "job").unwrap();
        assert_eq!(v.move_folder("Job", "Deep"), Err(FolderRefusal::Taken("Job".into())));
        assert_eq!(v.rename_folder("Job", "DEEP"), Err(FolderRefusal::Taken("DEEP".into())));
        assert_eq!(v.move_folder("Nowhere", ""), Err(FolderRefusal::Gone));
        assert_eq!(v.move_folder("Job", "Nowhere"), Err(FolderRefusal::Gone));
        assert_eq!(v.place(plan).unwrap().folder, "Job", "nothing moved");

        // Moved, in and out again.
        let r = v.move_folder("Job/Q4- plans", "Deep/Er").unwrap();
        assert_eq!((r.to.as_str(), r.notes.as_slice()), ("Deep/Er/Q4- plans", &[(q4, "Job/Q4- plans".to_string())][..]));
        assert_eq!(v.place(q4).unwrap().folder, "Deep/Er/Q4- plans");
        assert_eq!(v.move_folder("Deep/Er/Q4- plans", "").unwrap().to, "Q4- plans");
        assert_eq!(v.move_folder("Q4- plans", "").unwrap(), Relocated { to: "Q4- plans".into(), notes: vec![] }, "there already");
        assert!(may_go("A", "B") && may_go("A/B", "") && may_go("Ab", "A") && may_go("A", "Ab"));
        assert!(!may_go("A", "A") && !may_go("A", "A/B") && !may_go("A/B", "A") && !may_go("A", ""));

        // A note moved, pinned.
        assert_eq!(v.move_note(loose, "Job"), Some(String::new()));
        assert_eq!(v.move_note(loose, "Job"), None, "there already");
        assert_eq!(v.move_note(loose, "Nowhere"), None, "no such folder");
        assert_eq!(v.move_note(old, ""), None, "in Deleted");
        assert!(v.set_pinned(loose, true) && !v.set_pinned(loose, true));
        assert!(v.place(loose).unwrap().pinned);

        // To Deleted whole: its notes there, unpinned, each going back
        // where it was; then put back.
        assert!(!v.folder_is_bare("Job") && v.folder_is_bare("Deep/Er/X") && v.folder_is_bare("Deep/job"));
        v.make_folder("Job", "Sub").unwrap();
        let gone = v.trash_folder("Job");
        assert_eq!(gone.folders, ["Job", "Job/Sub"]);
        let mut keys = gone.notes.clone();
        keys.sort();
        let mut want = vec![plan, loose];
        want.sort();
        assert_eq!(keys, want);
        assert_eq!(v.place(plan), Some(Place { trashed: true, ..place("Job", "Plan") }));
        assert!(!v.folders().contains("Job") && !v.folders().contains("Job/Sub"));
        assert!(v.put_back_folder(&gone) && !v.put_back_folder(&gone));
        assert_eq!(v.place(plan), Some(place("Job", "Plan")));
        assert!(v.folders().contains("Job/Sub"));
        // A picture beside notes, as an older desktop kept them: not bare.
        v.put_file("Deep/Er/X/a.png", "aa");
        assert!(!v.folder_is_bare("Deep"));
    }

    /// A file put over another, or dropped: the one it held comes back
    /// if nothing else holds it, for the server to drop.
    #[test]
    fn a_picture_put_out_of_use_is_said() {
        let v = NoteDoc::new(1);
        assert_eq!(v.put_file("attachments/a.png", "aa"), (true, None));
        assert_eq!(v.put_file("attachments/b.png", "aa"), (true, None));
        assert_eq!(v.put_file("attachments/a.png", "aa"), (false, None), "the same: nothing");
        assert_eq!(v.put_file("attachments/a.png", "cc"), (true, None), "aa still at b");
        assert_eq!(v.put_file("attachments/b.png", "dd"), (true, Some("aa".into())));
        assert_eq!(v.drop_file("attachments/a.png"), (true, Some("cc".into())));
        assert_eq!(v.drop_file("attachments/a.png"), (false, None));
        assert_eq!(v.files().into_iter().collect::<Vec<_>>(), [("attachments/b.png".to_string(), "dd".to_string())]);
        assert_eq!(v.dropped_files().into_iter().collect::<Vec<_>>(), ["attachments/a.png"]);
        assert_eq!(v.dropped_id("attachments/a.png").as_deref(), Some("cc"), "its id kept, to fetch it back");
        assert_eq!(v.drop_file("attachments/nowhere.png"), (false, None), "nothing there: nothing to drop");
        // Put back: there again.
        v.set_file("attachments/a.png", "cc");
        assert!(v.dropped_files().is_empty());
    }

    /// An invite made on one device is known, code and all, on another;
    /// taken back on either, it is gone from both. One the server no
    /// longer lists (used, run out) is forgotten once past the grace; one
    /// just made isn't, though a list from before it says nothing of it.
    #[test]
    fn invites_are_kept_for_every_device_until_done_with() {
        use crate::crypto::tests::Seeded;
        let (a, b) = (NoteDoc::new(1), NoteDoc::new(2));
        let mut rng = Seeded(9);
        let (one, two, three) = (InviteCode::generate(&mut rng), InviteCode::generate(&mut rng), InviteCode::generate(&mut rng));
        let t = 1_800_000_000;
        assert!(a.keep_invite(&one, t) && a.keep_invite(&two, t) && !a.keep_invite(&one, t + 5));
        b.import(&a.updates()).unwrap();
        assert_eq!(b.invite_code(&one.hash()), Some(one));
        assert_eq!(b.invites()[&two.hash()], Invite { code: two, made: t });

        // Taken back on B: gone on A too.
        assert!(b.forget_invite(&one.hash()) && !b.forget_invite(&one.hash()));
        a.import(&b.updates()).unwrap();
        assert_eq!(a.invite_code(&one.hash()), None);

        // Two used (the server lists only three), three made a moment ago.
        a.keep_invite(&three, t + 100);
        assert!(!a.forget_invites_but(&[], t + 200), "within the grace: kept");
        assert!(!a.forget_invites_but(&[three.hash()], t + 200), "within the grace: kept");
        assert!(a.forget_invites_but(&[three.hash()], t + INVITE_GRACE + 50));
        assert_eq!(a.invites().into_keys().collect::<Vec<_>>(), [three.hash()]);
        assert!(!a.forget_invites_but(&[three.hash()], t + 10 * INVITE_GRACE), "still waiting: kept");
        // Run out: forgotten, listed or not.
        assert!(a.forget_invites_but(&[three.hash()], t + 100 + INVITE_LIFE));
        assert!(a.invites().is_empty());

        // An entry not a code, or not under its own hash: not there, and
        // cleared away with the rest.
        let map = a.loro().get_map(INVITES);
        let wrong: loro::LoroMapValue = vec![("code".to_string(), LoroValue::from(one.text())), ("made".to_string(), LoroValue::I64(0))].into();
        map.insert(&hex(&two.hash()), LoroValue::Map(wrong)).unwrap();
        map.insert("junk", "x").unwrap();
        a.loro().commit();
        a.keep_invite(&three, t + 100);
        assert_eq!(a.invites().len(), 1);
        assert!(a.forget_invites_but(&[three.hash()], t));
        assert_eq!(a.loro().get_map(INVITES).len(), 1);
    }

    /// A server whose list is empty (by mistake, or to wipe them) takes no
    /// code away: an empty list is no news. A code still goes once run
    /// out.
    #[test]
    fn an_empty_list_from_the_server_wipes_no_code() {
        use crate::crypto::tests::Seeded;
        let a = NoteDoc::new(1);
        let mut rng = Seeded(4);
        let (one, two) = (InviteCode::generate(&mut rng), InviteCode::generate(&mut rng));
        let t = 1_800_000_000;
        a.keep_invite(&one, t);
        a.keep_invite(&two, t + 3 * 86_400);
        for later in [t + INVITE_GRACE + 1, t + 86_400, t + INVITE_LIFE - 1] {
            assert!(!a.forget_invites_but(&[], later), "{later}");
            assert_eq!(a.invites().len(), 2);
        }
        assert!(a.forget_invites_but(&[], t + INVITE_LIFE));
        assert_eq!(a.invites().into_keys().collect::<Vec<_>>(), [two.hash()]);
    }

    /// A device removed had every code: all forgotten at once, their
    /// hashes given, to take back on the server.
    #[test]
    fn every_invite_is_forgotten_on_a_removal() {
        use crate::crypto::tests::Seeded;
        let (a, b) = (NoteDoc::new(1), NoteDoc::new(2));
        let mut rng = Seeded(5);
        let (one, two) = (InviteCode::generate(&mut rng), InviteCode::generate(&mut rng));
        a.keep_invite(&one, 1);
        a.keep_invite(&two, 2);
        b.import(&a.updates()).unwrap();
        let mut gone = b.forget_all_invites();
        gone.sort();
        let mut want = vec![one.hash(), two.hash()];
        want.sort();
        assert_eq!(gone, want);
        a.import(&b.updates()).unwrap();
        assert!(a.invites().is_empty() && b.invites().is_empty());
        assert!(b.forget_all_invites().is_empty());
    }

    /// A place, a folder or a file's path another device wrote that
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
        assert_eq!(v.files().into_keys().collect::<Vec<_>>(), ["attachments/a.png"]);
        assert!(v.dropped_files().is_empty() && v.dropped_id("attachments/../gone.png").is_none());
        assert!(!v.set_folder("../x", true) && !v.set_file("/x.png", "cc"));
        // A bad place is put right as any is written over: by a device
        // writing its own.
        assert!(v.set_place(bad_name, &place("", "Found")));
        assert_eq!(v.place(bad_name), Some(place("", "Found")));
    }

    /// A file's path but a picture's is kept in its own map, where a
    /// Carnet from before files doesn't look (it lets go of every path in
    /// the pictures' it doesn't take for a picture's), and read back with
    /// the pictures as one; dropped and brought back as a picture is. One
    /// written into the other map than its name's is read as not there.
    #[test]
    fn files_are_kept_apart_from_pictures() {
        let v = NoteDoc::new(1);
        assert!(v.set_file("attachments/a.png", "aa"));
        assert!(v.set_file("attachments/report.pdf", "bb"));
        // One with no kind, source code and a program, as a PDF.
        assert!(v.set_file("attachments/README", "ff"));
        assert!(v.set_file("attachments/main.rs", "gg"));
        assert!(v.set_file("attachments/setup.exe", "ii"));
        let raw = |m: &str| match v.loro().get_map(m).get_deep_value() {
            LoroValue::Map(m) => m.keys().cloned().collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        assert_eq!(
            (raw(PICTURES), raw(FILES)),
            (
                vec!["attachments/a.png".to_string()],
                vec![
                    "attachments/README".to_string(),
                    "attachments/main.rs".to_string(),
                    "attachments/report.pdf".to_string(),
                    "attachments/setup.exe".to_string()
                ]
            )
        );
        assert_eq!(
            v.files().into_keys().collect::<Vec<_>>(),
            ["attachments/README", "attachments/a.png", "attachments/main.rs", "attachments/report.pdf", "attachments/setup.exe"]
        );
        assert_eq!(v.drop_file("attachments/report.pdf"), (true, Some("bb".to_string())));
        assert_eq!(v.dropped_files().into_iter().collect::<Vec<_>>(), ["attachments/report.pdf"]);
        assert_eq!(v.dropped_id("attachments/report.pdf").as_deref(), Some("bb"));
        // Each in the other's map, as no Carnet writes them.
        v.loro().get_map(PICTURES).insert("attachments/notes.txt", "cc").unwrap();
        v.loro().get_map(PICTURES).insert("attachments/run.sh", "hh").unwrap();
        v.loro().get_map(FILES).insert("attachments/b.png", "dd").unwrap();
        v.loro().commit();
        assert_eq!(v.files().into_keys().collect::<Vec<_>>(), ["attachments/README", "attachments/a.png", "attachments/main.rs", "attachments/setup.exe"]);
    }

    /// What a Carnet that took only some kinds for files, and the rest for
    /// pictures' (4cbeaaa: a PDF, a sheet, a recording), reads of a vault
    /// this one wrote: the kinds it knows, and none of the others, so it
    /// neither fetches nor lets go of a program, a `README`, source code.
    #[test]
    fn a_carnet_that_knew_fewer_files_reads_the_others_as_not_there() {
        let v = NoteDoc::new(1);
        for (p, id) in [("attachments/a.png", "aa"), ("attachments/report.pdf", "bb"), ("attachments/README", "cc"), ("attachments/setup.exe", "dd"), ("attachments/main.rs", "ee")] {
            assert!(v.set_file(p, id));
        }
        // Its own map_of: a file's map for its few kinds, else the pictures'.
        let known = ["pdf", "txt", "csv", "rtf", "docx", "xlsx", "pptx", "odt", "ods", "odp", "m4a", "mp3", "wav", "mov", "mp4"];
        let old_map = |p: &str| {
            let doc = p.rsplit_once('.').is_some_and(|(_, e)| known.contains(&e.to_ascii_lowercase().as_str()));
            if doc && !carnet_core::doc::is_picture_name(p) { FILES } else { PICTURES }
        };
        let read = |name: &str| match v.loro().get_map(name).get_deep_value() {
            LoroValue::Map(m) => m.keys().filter(|k| old_map(k) == name).cloned().collect::<std::collections::BTreeSet<_>>(),
            _ => Default::default(),
        };
        let mut seen = read(PICTURES);
        seen.extend(read(FILES));
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), ["attachments/a.png", "attachments/report.pdf"]);
    }
}
