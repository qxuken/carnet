//! Notes on disk: one Markdown file per note in a plain folder, named after
//! its title. Writes are atomic and synced (a temp file flushed to disk,
//! then renamed over the old one), and deleting moves the file to
//! `.trash/` — nothing is ever destroyed.
//!
//! The folder is shared with whatever else the user opens it in. A file
//! loaded here is written back only after an edit here, and only if it has
//! not changed on disk since it was read: a note edited elsewhere is
//! reloaded when it has no edits of its own, and when it has, the two are
//! merged.
//!
//! Every text a file is written with or read back at is also kept in the
//! [`Replica`]: each note's CRDT, so an edit here and one made elsewhere
//! merge instead of forking, and every version is remembered. The files
//! stay the truth; without a replica (it could not be opened, or a newer
//! Carnet wrote it), a note changed in both places is kept twice, as
//! before.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use carnet_core::doc::{self, Block, Doc, Kind, ListStyle, Table};
use carnet_sync::invite::InviteCode;
use carnet_sync::vault::{self, FolderRefusal, Layout, Place};
use carnet_sync::{Key, Newer, Origin, Replica, Revision, Version};

#[path = "store_owned.rs"]
mod owned;
pub use owned::{Moved, is_owned, same_file, set_cell_srcs, set_srcs};
#[cfg(test)]
pub use owned::SWEEP_GRACE;

pub struct Note {
    /// Process-unique; keys the note's editors and scroll position.
    pub id: u64,
    /// `None` until the first save.
    pub path: Option<PathBuf>,
    pub title: String,
    pub doc: Doc,
    pub modified: SystemTime,
    /// When the file was made (its modified time where the system keeps
    /// no birth time); now, for a note made here.
    pub created: SystemTime,
    pub pinned: bool,
    /// Edited since the last save.
    pub dirty: bool,
    /// Bumped on every change; what caches key on.
    pub rev: u64,
    /// The title is the file's first `# ` heading. A file that never had
    /// one is titled by its name, and keeps not having one.
    pub h1: bool,
    /// The title was edited here, so the file may follow it to a new name.
    /// A file whose name and heading disagree is left as named until then.
    pub renamable: bool,
    /// The file used CRLF line endings; it keeps them.
    pub crlf: bool,
    /// The file as last read or written (modified time, length): what tells
    /// an edit made elsewhere from our own.
    pub disk: Option<(SystemTime, u64)>,
    /// The folder it lives in, relative to the notes folder with `/`
    /// between the parts; empty for the top level.
    pub folder: String,
    /// Which note this is in the replica, across renames and moves; `None`
    /// until its file is first written or read with a replica open.
    pub key: Option<Key>,
}

impl Note {
    pub fn new(title: &str, doc: Doc) -> Self {
        Self {
            id: doc::new_id(),
            path: None,
            title: title.to_string(),
            doc,
            modified: SystemTime::now(),
            created: SystemTime::now(),
            pinned: false,
            dirty: true,
            rev: 0,
            h1: true,
            renamable: true,
            crlf: false,
            disk: None,
            folder: String::new(),
            key: None,
        }
    }

    /// The note in the file at `path`.
    fn load(path: &Path) -> io::Result<Self> {
        Self::load_text(path).map(|(note, _)| note)
    }

    /// The note in a file not in the list (in the trash), to read.
    pub fn load_file(path: &Path) -> io::Result<Self> {
        Self::load(path)
    }

    /// The note in the file at `path`, and the file's text as it is: what
    /// the replica keeps, which a note written back need not be byte for
    /// byte (a BOM, for one, is dropped).
    fn load_text(path: &Path) -> io::Result<(Self, String)> {
        let bytes = fs::read(path)?;
        let src = String::from_utf8(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let (title, doc) = doc::from_markdown(&src);
        let stem = stem_of(path);
        let note = Note {
            id: doc::new_id(),
            h1: title.is_some(),
            title: title.unwrap_or(stem),
            doc,
            modified: stamp(path).map_or(SystemTime::UNIX_EPOCH, |s| s.0),
            created: fs::metadata(path)
                .and_then(|m| m.created().or_else(|_| m.modified()))
                .unwrap_or(SystemTime::UNIX_EPOCH),
            pinned: false,
            dirty: false,
            rev: 0,
            renamable: false,
            crlf: src.contains("\r\n"),
            disk: stamp(path),
            folder: String::new(),
            path: Some(path.to_path_buf()),
            key: None,
        };
        Ok((note, src))
    }

    pub fn display_title(&self) -> &str {
        let t = self.title.trim();
        if t.is_empty() { "Untitled" } else { t }
    }

    pub fn touch(&mut self) {
        self.dirty = true;
        self.rev += 1;
        self.modified = SystemTime::now();
    }

    /// Nothing written in it: no title, and only empty text.
    pub fn is_blank(&self) -> bool {
        self.title.trim().is_empty()
            && self
                .doc
                .blocks()
                .iter()
                .all(|b| matches!(&b.kind, Kind::Text(t) if t.trim().is_empty()))
    }

    /// This note as the file at `path` has it now: the same note — its id,
    /// key, pin and folder kept — at a new revision, so what was counted
    /// from the old text is counted again. With the file's text.
    fn reread(&self, path: &Path) -> io::Result<(Note, String)> {
        let (mut fresh, text) = Note::load_text(path)?;
        fresh.id = self.id;
        fresh.key = self.key;
        fresh.pinned = self.pinned;
        fresh.folder = self.folder.clone();
        fresh.rev = self.rev.wrapping_add(1);
        Ok((fresh, text))
    }

    /// Its file moved to `to` by a rename here: one with no heading is
    /// called by its file's name, so by the new one now, as it would be
    /// read from there.
    fn renamed_to(&mut self, to: PathBuf) {
        self.disk = stamp(&to);
        if !self.h1 {
            self.title = stem_of(&to);
        }
        self.path = Some(to);
    }

    /// Where its file is, relative to the notes folder: how the replica
    /// names it.
    fn rel_path(&self) -> Option<String> {
        let name = file_name(self.path.as_deref()?);
        Some(if self.folder.is_empty() { name } else { format!("{}/{name}", self.folder) })
    }

    pub fn markdown(&self) -> String {
        let title = self.h1.then_some(self.title.as_str());
        let text = doc::to_markdown(title, &self.doc);
        if self.crlf {
            text.replace('\n', "\r\n")
        } else {
            text
        }
    }
}

/// What a save did.
#[derive(Debug, PartialEq)]
pub enum Saved {
    /// Nothing to write.
    Clean,
    Written,
    /// The file had changed on disk: the edits here and the ones there
    /// were merged, written, and the note reloaded with both.
    Merged,
    /// The file had changed on disk and there was nothing to merge from:
    /// the edits here went to a new file — the note at this index — and
    /// the note was reloaded from disk.
    Conflict(usize),
}

pub struct Store {
    pub dir: PathBuf,
    pub notes: Vec<Note>,
    /// `.md` files that could not be read (not UTF-8), by name: left alone,
    /// and said so.
    pub skipped: Vec<String>,
    /// Every folder under the notes folder, as relative paths, sorted:
    /// subfolders are folders too, and an empty one is still one.
    pub folders: Vec<String>,
    /// Folders of pictures other than the attachments folder, as relative
    /// paths: an export's `Attachments`, at any depth. What
    /// [`gather_pictures`](Self::gather_pictures) takes pictures from.
    pub picture_folders: Vec<String>,
    /// Every file in the notes folder that is not a note, by its path from
    /// it, as last looked: what an Obsidian embed's picture
    /// (`![[photo.png]]`) is looked for among ([`doc::find_picture`]).
    pub files: doc::Files,
    /// Files in those folders already looked at and left where they are
    /// (no note links to them, or one does in a way that can't be
    /// rewritten): looked at once, not at every rescan.
    gather_seen: HashSet<PathBuf>,
    /// Every note's CRDT, on this device; `None` when it could not be
    /// opened, and the notes work as plain files.
    pub replica: Option<Replica>,
    /// Why there is no replica, in words for the user, when they can do
    /// something about it: it was written by a newer Carnet, and left
    /// alone until this one is updated.
    pub replica_refused: Option<String>,
    /// Notes whose files took a new text here (a save, another app's edit,
    /// a restore) since sync last asked: what it has to send.
    changed: Vec<Key>,
    /// The vault doc took changes made here, not yet sent.
    vault_changed: bool,
    /// Notes made here from another device's text before the vault said
    /// where they go: not this device's to place.
    from_remote: HashSet<Key>,
    /// Deleted forever, here or elsewhere, and pictures put out of use:
    /// what sync has to tell the server.
    forgotten: Forgotten,
    /// Notes reloaded with another device's change, by note id: their
    /// docs as they were just before, for the app to carry the caret of
    /// the open one through the change ([`carnet_core::compare::carry`]).
    /// The first since last taken is kept.
    pub before_remote: HashMap<u64, Doc>,
    /// The vault's layout as last read, and the vault's version then.
    vault_read: Option<(Version, Arc<Layout>)>,
}

/// What sync has to tell the server of things gone (docs/SYNC.md §4): the
/// notes deleted forever, whose rooms go, and the pictures no note shows
/// any more (their blob ids, hex), which it may drop.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Forgotten {
    pub keys: Vec<Key>,
    pub pictures: Vec<String>,
}

impl Forgotten {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.pictures.is_empty()
    }
}

/// What a note changed on another device did here.
#[derive(Debug, PartialEq)]
pub enum Remote {
    /// Nothing to do: the file has it already.
    Same,
    /// Note `i`, with no edits here, reloaded with it.
    Updated(usize),
    /// Note `i` had edits here: the two merged, written, and reloaded.
    Merged(usize),
    /// A note new to this device: its file made, note `i`.
    Created(usize),
    /// Notes moved, renamed, pinned, trashed or put back, or folders made
    /// or gone, as another device did: the list is to be shown again.
    Layout,
    /// Pictures came from another device: ones shown as missing are to be
    /// tried again.
    Pictures,
}

/// `CARNET_DIR`, else `~/Documents/Carnet`.
/// Where the notes live: `CARNET_DIR` when set, else the folder the
/// settings chose, else `~/Documents/Carnet`.
pub fn default_dir(chosen: Option<&Path>) -> PathBuf {
    if let Some(d) = std::env::var_os("CARNET_DIR") {
        return PathBuf::from(d);
    }
    chosen.map_or_else(documents_dir, Path::to_path_buf)
}

/// `~/Documents/Carnet`.
pub fn documents_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Documents").join("Carnet")
}

/// The note gone whose text `text` is most like, when at least half the
/// lines of the shorter one are in the other, and no other comes as
/// close: it is that note, renamed and changed (a word or two added to a
/// short note changes most of it, but keeps all of its lines). Taken from
/// `vanished`, so it is matched once.
fn take_similar(vanished: &mut Vec<(Key, String)>, text: &str) -> Option<Key> {
    let lines = |s: &str| -> HashSet<String> {
        s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
    };
    let now = lines(text);
    let score = |old: &str| {
        let was = lines(old);
        let shorter = was.len().min(now.len());
        if shorter == 0 { 0.0 } else { was.intersection(&now).count() as f64 / shorter as f64 }
    };
    let mut scored: Vec<(f64, usize)> = vanished.iter().enumerate().map(|(i, (_, t))| (score(t), i)).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    match scored.as_slice() {
        [(best, i), rest @ ..] if *best >= 0.5 && rest.first().is_none_or(|r| r.0 < *best) => {
            Some(vanished.remove(*i).0)
        }
        _ => None,
    }
}

/// Where the replica of the notes folder `notes` lives: the app's data
/// folder, under a name made from the notes folder's path, so another
/// folder chosen in the settings has its own (docs/SYNC.md §4). Tests keep
/// theirs beside their notes folder.
fn replica_dir(notes: &Path) -> Option<PathBuf> {
    if cfg!(test) {
        return Some(notes.with_extension("replica"));
    }
    let data = if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("Carnet")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/Carnet")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?
            .join("carnet")
    };
    let notes = fs::canonicalize(notes).unwrap_or_else(|_| notes.to_path_buf());
    let name = format!("{:016x}", carnet_sync::text_hash(&notes.to_string_lossy()));
    Some(data.join("replica").join(name))
}

const PINS: &str = ".pinned";
const TRASH: &str = ".trash";
/// Inside `.trash/`: where each thing in it came from, and when it went —
/// a line each, `name in the trash`, tab, `folder`, tab, `its name there`,
/// tab, `milliseconds since 1970`. What lets the Deleted folder put anything
/// back where it was, long after the notice that offered Undo has gone.
const ORIGINS: &str = ".origins";

/// A note or a folder in the trash, as the Deleted folder lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Trashed {
    /// Where it is now, in `.trash/`.
    pub path: PathBuf,
    /// The folder it was in ("" for the top level) and its name there:
    /// where it goes back to. Unknown (trashed before this was kept), the
    /// top level under its name in the trash.
    pub folder: String,
    pub name: String,
    /// A note's title, a folder's name.
    pub title: String,
    /// A note's first words; a folder's notes, counted.
    pub snippet: String,
    pub is_folder: bool,
    pub when: SystemTime,
}

impl Store {
    pub fn open(dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let (replica, replica_refused) = match replica_dir(&dir).map(|d| Replica::open(&d)) {
            Some(Ok(r)) => (Some(r), None),
            Some(Err(e)) => (None, Newer::of(&e).map(Newer::to_string)),
            None => (None, None),
        };
        let mut store = Self {
            dir,
            notes: Vec::new(),
            skipped: Vec::new(),
            folders: Vec::new(),
            picture_folders: Vec::new(),
            files: doc::Files::default(),
            gather_seen: HashSet::new(),
            replica,
            replica_refused,
            changed: Vec::new(),
            vault_changed: false,
            from_remote: HashSet::new(),
            forgotten: Forgotten::default(),
            before_remote: HashMap::new(),
            vault_read: None,
        };
        let pins = store.read_pins();
        let mut taken = HashSet::new();
        for (path, folder) in store.note_files()? {
            match Note::load_text(&path) {
                Ok((mut note, text)) => {
                    note.pinned = pins.contains(&pin_key(&folder, &path));
                    note.folder = folder;
                    store.notes.push(note);
                    // Changed while the app was closed: another app's edit.
                    store.track_in(store.notes.len() - 1, &text, Origin::OtherApp, &mut taken);
                }
                Err(_) => store.skipped.push(file_name(&path)),
            }
        }
        if store.notes.is_empty() && store.skipped.is_empty() {
            store.notes.push(welcome());
            store.save(0, true)?;
        }
        Ok(store)
    }

    fn read_pins(&self) -> HashSet<String> {
        fs::read_to_string(self.dir.join(PINS))
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().to_lowercase())
            .filter(|l| !l.is_empty())
            .collect()
    }

    /// Every note file under the notes folder with the folder it is in,
    /// and the folders as found. Hidden ones (`.trash`, `.git`, an
    /// editor's own) are not notes.
    fn note_files(&mut self) -> io::Result<Vec<(PathBuf, String)>> {
        let mut found = Found::default();
        walk(&self.dir, "", 0, &mut found)?;
        found.folders.sort_by_key(|f: &String| f.to_lowercase());
        self.folders = found.folders;
        self.picture_folders = found.pictures;
        self.files = doc::Files::new(found.others);
        Ok(found.files)
    }

    /// Where a folder is on disk.
    pub fn folder_dir(&self, folder: &str) -> PathBuf {
        folder
            .split('/')
            .filter(|p| !p.is_empty())
            .fold(self.dir.clone(), |d, p| d.join(p))
    }

    /// `p`, if it is in the notes folder (or is it), `..` worked out as
    /// written. What a write, a rename, a removal or a folder made for
    /// sync, or for a path a note's text names, is checked by first: the
    /// vault reads a name that could reach out as not there, and this
    /// makes sure, whatever slipped past.
    pub(crate) fn inside(&self, p: PathBuf) -> io::Result<PathBuf> {
        if within(&self.dir, &p) {
            Ok(p)
        } else {
            Err(io::Error::new(io::ErrorKind::InvalidInput, "that place is outside the notes folder"))
        }
    }

    /// A new folder named `name` inside `parent` ("" for the top level).
    /// Returns its relative path.
    pub fn create_folder(&mut self, parent: &str, name: &str) -> io::Result<String> {
        // A folder named as the pictures' holds pictures, at any depth:
        // made here, it would not be listed. (The web's rule too.)
        let name = vault::folder_name(name).map_err(io::Error::other)?;
        let rel = if parent.is_empty() {
            name
        } else {
            format!("{parent}/{name}")
        };
        fs::create_dir_all(self.folder_dir(&rel))?;
        if !self.folders.iter().any(|f| same_stem(f, &rel)) {
            self.folders.push(rel.clone());
            self.folders.sort_by_key(|f| f.to_lowercase());
        }
        Ok(rel)
    }

    /// Whether a folder holds nothing: no note in it or under it (a new
    /// note nobody wrote in is not one).
    pub fn folder_is_empty(&self, folder: &str) -> bool {
        let under = format!("{folder}/");
        !self.notes.iter().any(|n| {
            (n.folder == folder || n.folder.starts_with(&under))
                && !(n.path.is_none() && n.is_blank())
        })
    }

    /// Renames `folder` to `name`, in the same parent: the folder on disk,
    /// then its subfolders, its notes' folders and files, and the pins
    /// that name them. Links between files inside it, and from it to the
    /// pictures' folder, keep working: it stays as deep as it was. A name
    /// another folder there has is refused; one that differs only in case
    /// is the same folder, renamed. Returns the new relative path. The
    /// notes' edits must be saved first.
    pub fn rename_folder(&mut self, folder: &str, name: &str) -> io::Result<String> {
        let name = vault::folder_name(name).map_err(io::Error::other)?;
        let parent = vault::parent_of(folder);
        let to = if parent.is_empty() { name } else { format!("{parent}/{name}") };
        if to == folder {
            return Ok(to);
        }
        if !same_stem(&to, folder) && self.folder_dir(&to).exists() {
            return Err(io::Error::other("there is a folder of that name already"));
        }
        self.relocate_folder(folder, &to)?;
        Ok(to)
    }

    /// Moves `folder` into `parent` ("" for the top level), under its own
    /// name: the folder on disk with everything in it, then as
    /// [`rename_folder`](Self::rename_folder) has it — its subfolders, its
    /// notes' folders and files, the pins. Not into itself or a folder
    /// inside it, nor where a folder of its name is. Links between files
    /// inside it keep working as they are; when it goes deeper or less
    /// deep, its notes' links to pictures outside it (the pictures'
    /// folder) are rewritten from where they now are, and those notes
    /// saved. Returns the new path and the notes rewritten. The notes'
    /// edits must be saved first.
    pub fn move_folder(&mut self, folder: &str, parent: &str) -> io::Result<(String, Vec<u64>)> {
        if vault::is_inside(parent, folder) {
            return Err(io::Error::other(FolderRefusal::IntoItself));
        }
        if !parent.is_empty() && !self.folders.iter().any(|f| f == parent) {
            return Err(io::Error::other(FolderRefusal::Gone));
        }
        let leaf = vault::leaf_of(folder);
        let to = if parent.is_empty() { leaf.to_string() } else { format!("{parent}/{leaf}") };
        if to == folder {
            return Ok((to, Vec::new()));
        }
        if self.folder_dir(&to).exists() || self.folders.iter().any(|f| same_stem(f, &to)) {
            return Err(io::Error::other(FolderRefusal::Taken(leaf.to_string())));
        }
        let depth = |f: &str| f.split('/').count();
        let deeper = depth(&to) != depth(folder);
        let from_dir = self.folder_dir(folder);
        let under = format!("{folder}/");
        // Where each embed's picture was found from, before: looked for
        // again from the note's new folder after.
        let was_files = self.files.clone();
        self.relocate_folder(folder, &to)?;
        if !deeper {
            return Ok((to, Vec::new()));
        }
        self.index_files();
        let to_under = format!("{to}/");
        // A path from the notes folder (or the folder's own) as it is now.
        let now = |f: &str| match f.strip_prefix(&under) {
            Some(rest) => format!("{to}/{rest}"),
            None if f == folder => to.clone(),
            None => f.to_string(),
        };
        let mut relinked = Vec::new();
        for i in 0..self.notes.len() {
            let new_folder = self.notes[i].folder.clone();
            if new_folder != to && !new_folder.starts_with(&to_under) {
                continue;
            }
            let old_folder = format!("{folder}{}", &new_folder[to.len()..]);
            let (old_dir, new_dir) = (self.folder_dir(&old_folder), self.folder_dir(&new_folder));
            let files = &self.files;
            let moved_src = |src: &str, wiki: bool| match wiki {
                // An embed is looked for, not followed: one found from the
                // note's old folder and not from its new one is named by
                // its path from the notes folder, as it now is.
                true => {
                    let was = doc::find_picture(&old_folder, src, true, &was_files).map(|w| now(&w));
                    was.filter(|w| doc::find_picture(&new_folder, src, true, files).as_ref() != Some(w))
                }
                false if is_relative(src) => {
                    let at = old_dir.join(decode_src(src));
                    // Moved with it: the link as it was still leads there.
                    if within(&from_dir, &at) {
                        return None;
                    }
                    Some(relative(&new_dir, &at))
                }
                false => None,
            };
            let note = &mut self.notes[i];
            let mut changed = false;
            for b in 0..note.doc.len() {
                let new = match &note.doc.blocks()[b].kind {
                    Kind::Image { src, wiki, .. } => moved_src(src, *wiki),
                    Kind::Table(t) => {
                        let mut cells = t.clone();
                        let mut any = false;
                        for cell in cells.rows.iter_mut().flat_map(|r| r.cells.iter_mut()) {
                            if let Some(new) = doc::relink_cell(cell, |p| moved_src(&p.src, p.wiki)) {
                                *cell = new;
                                any = true;
                            }
                        }
                        if any {
                            note.doc.block_mut(b).kind = Kind::Table(cells);
                            changed = true;
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(new) = new
                    && let Kind::Image { src, .. } = &mut note.doc.block_mut(b).kind
                    && *src != new
                {
                    *src = new;
                    changed = true;
                }
            }
            if changed {
                self.notes[i].touch();
                self.save(i, false)?;
                relinked.push(self.notes[i].id);
            }
        }
        Ok((to, relinked))
    }

    /// `folder` to `to` on disk, and everything that names it here: its
    /// subfolders, its notes' folders and files, the pins.
    fn relocate_folder(&mut self, folder: &str, to: &str) -> io::Result<()> {
        let (from_dir, to_dir) = (self.inside(self.folder_dir(folder))?, self.inside(self.folder_dir(to))?);
        rename_retrying(&from_dir, &to_dir)?;
        let moved = |f: &str| vault::relocated(f, folder, to);
        for f in &mut self.folders {
            if let Some(new) = moved(f) {
                *f = new;
            }
        }
        self.folders.sort_by_key(|f| f.to_lowercase());
        for n in &mut self.notes {
            let Some(new) = moved(&n.folder) else { continue };
            n.folder = new;
            if let Some(rest) = n.path.as_deref().and_then(|p| p.strip_prefix(&from_dir).ok()) {
                n.path = Some(to_dir.join(rest));
            }
        }
        self.save_pins()
    }

    /// Whether a folder has a file in it or under it, a note or not: a
    /// picture, a PDF, the Finder's `.DS_Store`. One that has can't just be
    /// removed; it goes to the trash, whole.
    pub fn folder_has_files(&self, folder: &str) -> bool {
        fn any_file(dir: &Path, depth: usize) -> bool {
            let Ok(entries) = fs::read_dir(dir) else { return false };
            entries.flatten().any(|e| match e.file_type() {
                Ok(t) if t.is_dir() => depth >= 16 || any_file(&e.path(), depth + 1),
                _ => true,
            })
        }
        any_file(&self.folder_dir(folder), 0)
    }

    /// Removes an empty folder (and its empty subfolders). A folder with
    /// anything in it — a note, or a file Carnet does not show — stays:
    /// that one goes to the trash instead.
    pub fn delete_folder(&mut self, folder: &str) -> io::Result<()> {
        if !self.folder_is_empty(folder) {
            return Err(io::Error::other("the folder still has notes in it"));
        }
        if self.folder_has_files(folder) {
            return Err(io::Error::other("the folder has files in it that aren't notes"));
        }
        // Deepest first, so each is empty when its turn comes: the
        // subfolders on disk, listed or not.
        fn remove_empty(dir: &Path) -> io::Result<()> {
            for e in fs::read_dir(dir)?.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    remove_empty(&e.path())?;
                }
            }
            fs::remove_dir(dir)
        }
        remove_empty(&self.folder_dir(folder))?;
        let under = format!("{folder}/");
        self.folders.retain(|f| f != folder && !f.starts_with(&under));
        for n in &mut self.notes {
            if n.folder == folder || n.folder.starts_with(&under) {
                n.folder.clear();
            }
        }
        Ok(())
    }

    /// Moves a folder — its notes, pictures and whatever else is in it —
    /// to `.trash/`, whole, and forgets its notes. Their edits must be
    /// saved first. Returns where it went.
    pub fn trash_folder(&mut self, folder: &str) -> io::Result<PathBuf> {
        let bin = self.dir.join(TRASH);
        fs::create_dir_all(&bin)?;
        let leaf = folder.rsplit('/').next().unwrap_or(folder);
        let mut to = bin.join(leaf);
        let mut n = 2;
        while to.exists() {
            to = bin.join(format!("{leaf} {n}"));
            n += 1;
        }
        rename_retrying(&self.folder_dir(folder), &to)?;
        let (parent, _) = folder.rsplit_once('/').unwrap_or(("", folder));
        self.note_origin(&to, Some((parent, leaf, false)));
        let under = format!("{folder}/");
        let inside = |f: &str| f == folder || f.starts_with(&under);
        // Its notes kept in the replica at their places in the trash: the
        // vault says they're trashed, not gone.
        let in_trash = file_name(&to);
        if let Some(r) = self.replica.as_mut() {
            for n in self.notes.iter().filter(|n| inside(&n.folder)) {
                if let (Some(key), Some(rel)) = (n.key, n.rel_path()) {
                    let inner = if n.folder == folder { rel.rsplit('/').next().unwrap_or(&rel).to_string() } else { rel[under.len()..].to_string() };
                    let _ = r.moved(key, &format!("{TRASH}/{in_trash}/{inner}"));
                }
            }
        }
        self.notes.retain(|n| !inside(&n.folder));
        self.folders.retain(|f| !inside(f));
        self.save_pins()?;
        Ok(to)
    }

    /// Puts a trashed folder back where it was, its notes with it.
    pub fn restore_folder(&mut self, from: &Path, folder: &str) -> io::Result<()> {
        let to = self.inside(self.folder_dir(folder))?;
        if to.exists() {
            return Err(io::Error::other("a folder of that name is there again"));
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        rename_retrying(from, &to)?;
        self.note_origin(from, None);
        self.rescan();
        Ok(())
    }

    /// Moves note `i` into `folder`, file and all, under its own name or
    /// the first free one there. Its pictures stay where they are; the
    /// note's links to them are rewritten from its new place: an edit,
    /// saved, which syncs like any other. The user's move, here; a note
    /// put where another device moved it follows with
    /// [`place_note`](Self::place_note) instead.
    pub fn move_note(&mut self, i: usize, folder: &str) -> io::Result<()> {
        if self.notes[i].folder == folder {
            return Ok(());
        }
        let old = self.place_note(i, folder)?;
        let (from_dir, to_dir) = (self.folder_dir(&old), self.folder_dir(folder));
        let mut relinked = false;
        let files = &self.files;
        // Where a picture is from the new folder, when it changes.
        let moved_src = |src: &str, wiki: bool| match wiki {
            // An embed is looked for, not followed: one found from the
            // note's old folder and not from its new one is named by its
            // path from the notes folder.
            true => {
                let was = doc::find_picture(&old, src, true, files);
                was.filter(|w| doc::find_picture(folder, src, true, files).as_ref() != Some(w))
            }
            false if is_relative(src) => Some(relative(&to_dir, &from_dir.join(decode_src(src)))),
            false => None,
        };
        let note = &mut self.notes[i];
        for b in 0..note.doc.len() {
            let new = match &note.doc.blocks()[b].kind {
                Kind::Image { src, wiki, .. } => moved_src(src, *wiki),
                // A table's pictures among its cells' words, the same way.
                Kind::Table(t) => {
                    let mut cells = t.clone();
                    let mut changed = false;
                    for cell in cells.rows.iter_mut().flat_map(|r| r.cells.iter_mut()) {
                        if let Some(new) = doc::relink_cell(cell, |p| moved_src(&p.src, p.wiki)) {
                            *cell = new;
                            changed = true;
                        }
                    }
                    if changed {
                        note.doc.block_mut(b).kind = Kind::Table(cells);
                        relinked = true;
                    }
                    None
                }
                _ => None,
            };
            if let Some(new) = new
                && let Kind::Image { src, .. } = &mut note.doc.block_mut(b).kind
                && *src != new
            {
                *src = new;
                relinked = true;
            }
        }
        if relinked {
            self.notes[i].touch();
            self.save(i, false)?;
        }
        self.save_pins()
    }

    /// Note `i`'s file moved into `folder`, under its own name or the
    /// first free one there, its text as it is. Where the vault says it
    /// lives, it goes so (docs/SYNC.md §4): its text was written there (a
    /// note come before its place, put at the top meanwhile), or rewritten
    /// by the device that moved it, and comes as that device's edit.
    /// Rewritten here as well, its links would be moved twice, or moved
    /// for a move nobody made. Returns the folder it was in.
    fn place_note(&mut self, i: usize, folder: &str) -> io::Result<String> {
        if self.notes[i].dirty {
            return Err(io::Error::other("it has changes that are not saved yet"));
        }
        // Changed in another app since it was read (a sync, a pull): that
        // is the version that moves, not ours from before.
        self.refresh(i);
        fs::create_dir_all(self.inside(self.folder_dir(folder))?)?;
        if let Some(from) = self.notes[i].path.clone() {
            let to = self.inside(self.free_path(&stem_of(&from), None, folder))?;
            rename_retrying(&from, &to)?;
            self.notes[i].renamed_to(to);
        }
        let old = std::mem::replace(&mut self.notes[i].folder, folder.to_string());
        if let (Some(key), Some(rel), Some(replica)) =
            (self.notes[i].key, self.notes[i].rel_path(), self.replica.as_mut())
        {
            let _ = replica.moved(key, &rel);
        }
        Ok(old)
    }

    /// Where an image's `src` points, for a note in `folder`: a file on
    /// disk, or `None` for a web address, or a file on another computer
    /// ([`on_network`]), which is never looked at.
    pub fn resolve(&self, folder: &str, src: &str) -> Option<PathBuf> {
        if is_web(src) {
            return None;
        }
        // As written (a share on Windows is a name with `\` in it here;
        // `file:////host/…` is one too), and as joined.
        let (named, p) = if let Some(rest) = src.strip_prefix("file://") {
            let path = decode_src(rest.strip_prefix('/').unwrap_or(rest));
            (PathBuf::from(decode_src(rest)), PathBuf::from(path))
        } else if Path::new(src).is_absolute() {
            (PathBuf::from(src), PathBuf::from(src))
        } else {
            let path = decode_src(src);
            (PathBuf::from(&path), self.folder_dir(folder).join(path))
        };
        (!on_network(&named) && !on_network(&p)).then_some(p)
    }

    /// Where a picture a note in `folder` shows is on disk: its `src`, and
    /// whether an Obsidian embed (`![[…]]`) names it, looked for as
    /// Obsidian looks ([`doc::find_picture`]). `None` for a web address.
    pub fn picture_file(&self, folder: &str, src: &str, wiki: bool) -> Option<PathBuf> {
        match wiki {
            true => doc::find_picture(folder, src, true, &self.files).map(|p| self.folder_dir(&p)),
            false => self.resolve(folder, src),
        }
    }

    /// The file a link in a note in `folder` points at, when it is one in
    /// the notes folder: an export's PDF, a recording. `None` for a web
    /// address, or a file elsewhere or not there.
    pub fn linked_file(&self, folder: &str, src: &str) -> Option<PathBuf> {
        let p = self.resolve(folder, src.trim())?;
        (p.is_file() && key_under(&self.dir, &p).is_some()).then_some(p)
    }

    /// Keeps a picture with the notes: `bytes` written to the attachments
    /// folder under a new name ending in `ext`. Returns the `src` a note in
    /// `folder` links to it by.
    pub fn save_attachment(&self, folder: &str, ext: &str, bytes: &[u8]) -> io::Result<String> {
        self.save_attachment_as(folder, &format!("pasted-{}", timestamp()), ext, bytes)
    }

    /// Keeps a picture with the notes, as [`save_attachment`](
    /// Self::save_attachment) does, named for `stem` made plain (or the
    /// first such name free).
    pub fn save_attachment_as(&self, folder: &str, stem: &str, ext: &str, bytes: &[u8]) -> io::Result<String> {
        let dir = self.dir.join(ATTACHMENTS);
        fs::create_dir_all(&dir)?;
        let path = free_file(&dir, &slug(stem).replace(' ', "-"), ext);
        write_bytes_atomic(&path, bytes)?;
        Ok(relative(&self.folder_dir(folder), &path))
    }

    /// Copies a picture from elsewhere into the attachments folder, under
    /// its own name (or the first free one). Returns its `src`, as
    /// `save_attachment` does.
    pub fn import_attachment(&self, folder: &str, from: &Path) -> io::Result<String> {
        let to = self.copy_in(from, false)?;
        Ok(relative(&self.folder_dir(folder), &to))
    }

    /// The files in an export's `Attachments` folders ([`picture_folders`](
    /// Self::picture_folders)) that notes link to move into the attachments
    /// folder, where pictures are kept and synced from, and the links follow
    /// them. Each goes under its own name made plain, or the first one free
    /// there: none is written over.
    ///
    /// A file moves only when every note that names it does so by a link
    /// that can be rewritten (`](…)`, `src="…"`, an Obsidian `[[…]]` or
    /// `![[…]]`, which stays one, named by its new path) and has no edits
    /// not yet saved. One no note links to stays, and so does one whose new
    /// name an Obsidian link elsewhere names, found by name: that link
    /// would find it there in place of its own. Each is
    /// copied first, then its notes' links are rewritten, and only then is
    /// the old file removed, so no link ever points at nothing; a note that
    /// can't be written keeps the old file where it was. An emptied folder
    /// goes. A file left where it is is not looked at again.
    pub fn gather_pictures(&mut self) -> Gathered {
        let mut out = Gathered::default();
        // The files, by their paths from the notes folder.
        let mut files: HashMap<String, PathBuf> = HashMap::new();
        for f in &self.picture_folders {
            for p in files_under(&self.folder_dir(f), 0) {
                if !self.gather_seen.contains(&p)
                    && let Some(k) = key_under(&self.dir, &p)
                {
                    files.insert(k, p);
                }
            }
        }
        if files.is_empty() {
            return out;
        }
        // Who links to which; and notes that name an attachments folder
        // other than by a link, whose files can't follow.
        let mut links: HashMap<String, Vec<usize>> = HashMap::new();
        let mut odd: Vec<String> = Vec::new();
        // The names Obsidian links find other files by: a file that took
        // one in the attachments folder would be found in their place.
        let mut named: HashSet<String> = HashSet::new();
        for (i, n) in self.notes.iter().enumerate() {
            let text = n.markdown();
            let from = self.folder_dir(&n.folder);
            let targets = link_targets(&text);
            for r in &targets {
                let t = &text[r.clone()];
                if is_relative(t)
                    && let Some(k) = key_under(&self.dir, &from.join(decode_src(t)))
                    && files.contains_key(&k)
                {
                    links.entry(k).or_default().push(i);
                }
            }
            let wikis = wiki_targets(&text);
            for r in &wikis {
                let t = &text[r.clone()];
                let Some(p) = doc::find_picture(&n.folder, t, true, &self.files) else { continue };
                let k = p.to_lowercase();
                if files.contains_key(&k) {
                    links.entry(k).or_default().push(i);
                } else if self.files.get(&p).is_some() && [format!("{}/{t}", n.folder), t.to_string()].iter().all(|q| self.files.get(q).is_none()) {
                    named.insert(t.rsplit('/').next().unwrap_or(t).to_lowercase());
                }
            }
            let low = text.to_lowercase();
            let linked: usize = targets.iter().chain(&wikis).map(|r| text[r.clone()].to_lowercase().matches("attachments/").count()).sum();
            if low.matches("attachments/").count() > linked {
                odd.push(decode_src(&low));
            }
        }
        let mut moved: HashMap<String, (PathBuf, PathBuf)> = HashMap::new();
        let dir = self.dir.join(ATTACHMENTS);
        for (k, from) in &files {
            let name = file_name(from).to_lowercase();
            let Some(notes) = links.get(k).filter(|_| !odd.iter().any(|t| t.contains(&name))) else {
                // Not linked, or linked in a way that can't follow.
                self.gather_seen.insert(from.clone());
                continue;
            };
            // Edits not saved yet: next time.
            if notes.iter().any(|&i| self.notes[i].dirty) || fs::create_dir_all(&dir).is_err() {
                continue;
            }
            let stem = slug(&stem_of(from)).replace(' ', "-");
            let ext = from.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            let to = free_file(&dir, &stem, &ext);
            if named.contains(&file_name(&to).to_lowercase()) {
                self.gather_seen.insert(from.clone());
                continue;
            }
            if fs::hard_link(from, &to).is_err() && fs::copy(from, &to).is_err() {
                let _ = fs::remove_file(&to);
                continue;
            }
            moved.insert(k.clone(), (from.clone(), to));
        }
        // The links, note by note; a file stays put where one can't follow.
        let mut held: HashSet<String> = HashSet::new();
        let mut todo: Vec<usize> = moved.keys().flat_map(|k| links[k].iter().copied()).collect();
        todo.sort_unstable();
        todo.dedup();
        for i in todo {
            match self.relink(i, &moved) {
                Ok(true) => out.notes.push(self.notes[i].id),
                Ok(false) => {}
                Err(_) => {
                    for (k, notes) in &links {
                        if notes.contains(&i) {
                            held.insert(k.clone());
                        }
                    }
                }
            }
        }
        for (k, (from, to)) in &moved {
            if held.contains(k) || fs::remove_file(from).is_err() {
                continue;
            }
            out.files += 1;
            out.pictures += usize::from(crate::images::is_image(to));
        }
        // Emptied: gone.
        for f in &self.picture_folders {
            remove_empty_dirs(&self.folder_dir(f));
        }
        let dir = self.dir.clone();
        self.picture_folders.retain(|f| f.split('/').fold(dir.clone(), |d, p| d.join(p)).exists());
        if out.files > 0 {
            self.index_files();
        }
        out
    }

    /// Looks again at the files in the notes folder that are not notes
    /// ([`files`](Self::files)): pictures came, or moved.
    pub fn index_files(&mut self) {
        let mut found = Found::default();
        if walk(&self.dir, "", 0, &mut found).is_ok() {
            self.files = doc::Files::new(found.others);
        }
    }

    /// Note `i`'s links to the files in `moved` (by their keys, from where
    /// to where) rewritten to where they are now, in its file as it is on
    /// disk; the note reread. True when anything changed.
    fn relink(&mut self, i: usize, moved: &HashMap<String, (PathBuf, PathBuf)>) -> io::Result<bool> {
        // Changed in another app since it was read: that text is the one
        // rewritten.
        self.refresh(i);
        let Some(path) = self.notes[i].path.clone() else { return Ok(false) };
        let text = fs::read_to_string(&path)?;
        let folder = &self.notes[i].folder;
        let from = self.folder_dir(folder);
        // Each target that names a moved file, and what it says now: a
        // link its path from the note, an Obsidian one its path from the
        // notes folder.
        let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        for r in link_targets(&text) {
            let t = &text[r.clone()];
            if let Some((_, to)) = is_relative(t)
                .then(|| key_under(&self.dir, &from.join(decode_src(t))))
                .flatten()
                .and_then(|k| moved.get(&k))
            {
                edits.push((r, encode_src(&relative(&from, to))));
            }
        }
        for r in wiki_targets(&text) {
            let t = &text[r.clone()];
            if let Some((_, to)) = doc::find_picture(folder, t, true, &self.files).and_then(|p| moved.get(&p.to_lowercase()))
                && let Some(k) = to.strip_prefix(&self.dir).ok().map(|p| p.to_string_lossy().replace('\\', "/"))
                && !k.contains(['[', ']', '|', '#'])
            {
                edits.push((r, k));
            }
        }
        if edits.is_empty() {
            return Ok(false);
        }
        edits.sort_by_key(|(r, _)| r.start);
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for (r, new) in edits {
            if r.start < last {
                continue;
            }
            out.push_str(&text[last..r.start]);
            out.push_str(&new);
            last = r.end;
        }
        out.push_str(&text[last..]);
        write_atomic(&path, &out)?;
        let (fresh, written) = self.notes[i].reread(&path)?;
        self.notes[i] = fresh;
        self.track(i, &written, Origin::Save);
        Ok(true)
    }

    /// Catches up with the folder: a note changed on disk and not here is
    /// reloaded, one deleted there and not edited here goes, and a new
    /// file comes in. Returns whether anything changed. What the app runs
    /// when its window comes back to the front.
    pub fn rescan(&mut self) -> bool {
        // Notes whose files went: one may be back under another name,
        // changed on the way (a rename and an edit from another machine).
        let mut vanished: Vec<(Key, String)> = Vec::new();
        let mut stale: HashSet<u64> = HashSet::new();
        let before = self.notes.len();
        self.notes.retain(|note| {
            let Some(path) = note.path.as_deref().filter(|_| !note.dirty) else { return true };
            match stamp(path) {
                None => {
                    if let Some(key) = note.key {
                        vanished.push((key, note.markdown()));
                    }
                    false
                }
                Some(s) => {
                    if Some(s) != note.disk {
                        stale.insert(note.id);
                    }
                    true
                }
            }
        });
        let mut changed = self.notes.len() != before;
        // The keys notes have, collected once: a folder of 10,000 notes
        // looked at again is not 10,000 times 10,000.
        let mut taken: HashSet<Key> = self.notes.iter().filter_map(|n| n.key).collect();
        for i in 0..self.notes.len() {
            let (note, Some(path)) = (&self.notes[i], self.notes[i].path.as_deref()) else { continue };
            if !stale.contains(&note.id) {
                continue;
            }
            if let Ok((fresh, text)) = note.reread(path) {
                self.notes[i] = fresh;
                self.bring_back(&self.notes[i].folder, &text);
                self.track_in(i, &text, Origin::OtherApp, &mut taken);
                changed = true;
            }
        }
        let pins = self.read_pins();
        let folders_before = self.folders.clone();
        if let Ok(files) = self.note_files() {
            // Each note's file looked up by its path, as a file system that
            // ignores case sees it, not compared with every other one's.
            let mut known: HashSet<String> = self.notes.iter().filter_map(|n| n.path.as_deref()).map(path_key).collect();
            for (path, folder) in files {
                if !known.insert(path_key(&path)) || self.skipped.contains(&file_name(&path)) {
                    continue;
                }
                if let Ok((mut note, text)) = Note::load_text(&path) {
                    note.pinned = pins.contains(&pin_key(&folder, &path));
                    note.folder = folder;
                    self.notes.push(note);
                    let i = self.notes.len() - 1;
                    if self.known_in(i, &text, &taken).is_none() {
                        self.notes[i].key = take_similar(&mut vanished, &text);
                    }
                    self.bring_back(&self.notes[i].folder, &text);
                    self.track_in(i, &text, Origin::OtherApp, &mut taken);
                    changed = true;
                }
            }
        }
        changed || self.folders != folders_before
    }

    /// Reloads note `i` from disk when it changed there and has no edits
    /// here. True when it did.
    pub fn refresh(&mut self, i: usize) -> bool {
        let note = &self.notes[i];
        let Some(path) = note.path.clone() else { return false };
        if note.dirty || stamp(&path) == note.disk || stamp(&path).is_none() {
            return false;
        }
        match note.reread(&path) {
            Ok((fresh, text)) => {
                self.notes[i] = fresh;
                self.track(i, &text, Origin::OtherApp);
                true
            }
            Err(_) => false,
        }
    }

    /// Writes note `i` if it has edits, to the file it already has. With
    /// `rename`, a note whose title was edited here also moves to the file
    /// its title names — the app passes it once the title is no longer
    /// being typed, so a file is not renamed letter by letter.
    pub fn save(&mut self, i: usize, rename: bool) -> io::Result<Saved> {
        let note = &self.notes[i];
        let wanted = slug(note.display_title());
        let current = note.path.clone();
        let named_right = current
            .as_ref()
            .is_some_and(|p| same_stem(&stem_of(p), &wanted) || numbered_as(p, &wanted));
        let move_it = current.is_some() && rename && note.renamable && !named_right;
        if !note.dirty && !move_it {
            return Ok(Saved::Clean);
        }
        // Changed on disk since we read it: merge, or keep both.
        if let Some(p) = &current
            && let Some(now) = stamp(p)
            && Some(now) != note.disk
        {
            return self.merge_disk(i, p);
        }
        let target = match &current {
            Some(p) if !move_it => p.clone(),
            _ => {
                let folder = note.folder.clone();
                fs::create_dir_all(self.folder_dir(&folder))?;
                self.free_path(&wanted, current.as_deref(), &folder)
            }
        };
        let text = note.markdown();
        write_made(&target, &text, Some(note.created))?;
        if let Some(old) = &current
            && *old != target
        {
            let _ = fs::remove_file(old);
            mark_own(old);
        }
        let disk = stamp(&target);
        let note = &mut self.notes[i];
        note.path = Some(target);
        note.dirty = false;
        note.disk = disk;
        let pins = note.pinned || move_it;
        self.track(i, &text, Origin::Save);
        if pins {
            self.save_pins()?;
        }
        Ok(Saved::Written)
    }

    /// Note `i` has edits, and its file at `path` changed on disk since it
    /// was read: the two become changes from the version the file last
    /// held, merged, and written; the note reloads with both. Without a
    /// version to merge from (no replica, or a file it never saw), both
    /// are kept as before.
    fn merge_disk(&mut self, i: usize, path: &Path) -> io::Result<Saved> {
        let ours = self.notes[i].markdown();
        let merged = match (self.notes[i].key, self.replica.as_mut(), fs::read_to_string(path)) {
            (Some(key), Some(replica), Ok(theirs)) => replica.merge(key, &theirs, &ours).ok().flatten(),
            _ => None,
        };
        let Some(merged) = merged else { return self.keep_both(i) };
        write_atomic(path, &merged)?;
        let (fresh, text) = self.notes[i].reread(path)?;
        self.notes[i] = fresh;
        self.track(i, &text, Origin::Save);
        Ok(Saved::Merged)
    }

    /// Every version of note `i`, newest first. Empty without a replica,
    /// or for a note not yet written or read with one.
    pub fn history(&mut self, i: usize) -> Vec<Revision> {
        match (self.notes[i].key, self.replica.as_mut()) {
            (Some(key), Some(r)) => r.history(key),
            _ => Vec::new(),
        }
    }

    /// When note `i`'s history starts: its first version, `None` without
    /// one.
    pub fn first_version(&mut self, i: usize) -> Option<SystemTime> {
        let first = self.history(i).last()?.time;
        Some(epoch_time(first.max(0) as u64).unwrap_or(UNIX_EPOCH))
    }

    /// Note `i`'s text as it was at `version`.
    pub fn text_at(&mut self, i: usize, version: &Version) -> Option<String> {
        let (key, r) = (self.notes[i].key?, self.replica.as_mut()?);
        r.text_at(key, version).ok()
    }

    /// Note `i` becomes what it was at `version`: written to its file
    /// (which keeps its name), reread, and a version of its own, so the
    /// restore can be restored away. Its edits must be saved first; one
    /// made in another app since it was read is taken in first, and stays
    /// in the history.
    pub fn restore_version(&mut self, i: usize, version: &Version) -> io::Result<()> {
        if self.notes[i].dirty {
            return Err(io::Error::other("it has changes that are not saved yet"));
        }
        self.refresh(i);
        let (Some(key), Some(path)) = (self.notes[i].key, self.notes[i].path.clone()) else {
            return Err(io::Error::other("it has no history yet"));
        };
        let Some(r) = self.replica.as_mut() else {
            return Err(io::Error::other("its history could not be opened"));
        };
        let text = r.restore(key, version)?;
        write_atomic(&path, &text)?;
        let (fresh, text) = self.notes[i].reread(&path)?;
        self.notes[i] = fresh;
        self.track(i, &text, Origin::Restore);
        Ok(())
    }

    /// The key the replica has for note `i`'s file holding `text`: by its
    /// path, or by its text at a path gone, and not one of `taken`.
    fn known_in(&self, i: usize, text: &str, taken: &HashSet<Key>) -> Option<Key> {
        let rel = self.notes[i].rel_path()?;
        self.replica
            .as_ref()?
            .key_for(&rel, text, |k| taken.contains(&k), |p| self.dir.join(p).exists())
    }

    /// The file of note `i` holds `text` now, written here or read from
    /// disk: the replica learns it, and the note learns its key.
    fn track(&mut self, i: usize, text: &str, origin: Origin) {
        // A picture of Carnet's it shows that the sweep took: back.
        self.bring_back(&self.notes[i].folder, text);
        let mut taken = self.notes.iter().filter_map(|n| n.key).collect();
        self.track_in(i, text, origin, &mut taken);
    }

    /// [`track`](Self::track), with the keys notes already have kept by the
    /// caller: opening 10,000 notes collects them once, not each time.
    fn track_in(&mut self, i: usize, text: &str, origin: Origin, taken: &mut HashSet<Key>) {
        let Some(rel) = self.notes[i].rel_path() else { return };
        let dir = &self.dir;
        let Some(replica) = self.replica.as_mut() else { return };
        let key = match self.notes[i].key {
            Some(k) => k,
            None => replica
                .key_for(&rel, text, |k| taken.contains(&k), |p| dir.join(p).exists())
                .unwrap_or_else(Key::random),
        };
        taken.insert(key);
        self.notes[i].key = Some(key);
        // A replica that cannot write is behind, not wrong: the files are
        // the truth, and it catches up from them when it next opens.
        let _ = replica.record(key, &rel, text, origin);
        if !self.changed.contains(&key) {
            self.changed.push(key);
        }
    }

    /// Where each note lives here, and the folders: the notes in the list
    /// (their folder, their file's name, pinned), and the ones in the
    /// trash, by where they came from. What the vault doc is kept in step
    /// with (docs/SYNC.md §4).
    ///
    /// A name is the one a note wants, as `vault` has it: a file numbered
    /// here to make room for another of that name ("Plan 2" for "Plan") has
    /// the name it wants, not a new one, or two devices would number each
    /// other's notes without end.
    pub fn layout(&self, vault: &Layout) -> Layout {
        let mut out = Layout::default();
        for n in &self.notes {
            if let (Some(key), Some(path)) = (n.key, n.path.as_deref()) {
                let stem = stem_of(path);
                let titled = slug(n.display_title());
                let name = match vault.notes.get(&key) {
                    Some(v) if numbered_as(path, &v.name) => v.name.clone(),
                    // Saved here under a number because another file had the
                    // name its title asks for: it wants that name.
                    _ if numbered_as(path, &titled) => titled,
                    _ => stem,
                };
                out.notes.insert(key, Place { folder: n.folder.clone(), name, pinned: n.pinned, trashed: false });
            }
        }
        if let Some(r) = &self.replica {
            let origins = self.origins();
            for key in r.keys() {
                if out.notes.contains_key(&key) {
                    continue;
                }
                let Some(rest) = r.entry(key).and_then(|e| e.path.strip_prefix(&format!("{TRASH}/")).map(str::to_string)) else {
                    continue;
                };
                let (top, inner) = match rest.split_once('/') {
                    Some((t, i)) => (t.to_string(), Some(i.to_string())),
                    None => (rest.clone(), None),
                };
                let (from_folder, from_name, was_pinned) = origins.get(&top).map(|(f, n, _, p)| (f.clone(), n.clone(), *p)).unwrap_or_default();
                let place = match inner {
                    // A note trashed alone: back where it was, as it was named.
                    None => {
                        let name = if from_name.is_empty() { top.clone() } else { from_name };
                        Place { folder: from_folder, name: name.strip_suffix(".md").unwrap_or(&name).to_string(), pinned: was_pinned, trashed: true }
                    }
                    // In a folder trashed whole: back inside that folder.
                    Some(inner) => {
                        let leaf = if from_name.is_empty() { top.clone() } else { from_name };
                        let base = if from_folder.is_empty() { leaf } else { format!("{from_folder}/{leaf}") };
                        let (sub, file) = inner.rsplit_once('/').map_or(("", inner.as_str()), |(s, f)| (s, f));
                        let folder = if sub.is_empty() { base } else { format!("{base}/{sub}") };
                        Place { folder, name: file.strip_suffix(".md").unwrap_or(file).to_string(), pinned: false, trashed: true }
                    }
                };
                out.notes.insert(key, place);
            }
        }
        // Each name as the vault may hold it: one another system would
        // read as more than a name goes as one it reads as one.
        for p in out.notes.values_mut() {
            p.folder = plain_folder(&p.folder);
            p.name = plain(&p.name);
        }
        out.folders = self.folders.iter().map(|f| plain_folder(f)).collect();
        out
    }

    /// The vault doc changed (another device's changes came in): kept, and
    /// applied to the files at the next [`reconcile`](Self::reconcile).
    pub fn vault_changed(&mut self) -> io::Result<()> {
        if let Some(r) = &self.replica {
            r.persist_vault()?;
        }
        Ok(())
    }

    /// The vault says `path` holds the picture `id` (pictures.rs). True if
    /// that changed; then it is kept, and sent. The picture it held before,
    /// if nothing else holds it now, the server may drop.
    pub fn set_picture(&mut self, path: &str, id: &str) -> bool {
        let Some(r) = self.replica.as_ref() else { return false };
        let (changed, unused) = r.vault().put_picture(path, id);
        if !changed {
            return false;
        }
        let _ = r.persist_vault();
        self.vault_changed = true;
        self.forgotten.pictures.extend(unused);
        true
    }

    /// An invite this account made, kept in the vault (docs/SYNC.md §6):
    /// its code, for every device of the account to show again. Kept on
    /// disk at once, and sent.
    pub fn keep_invite(&mut self, code: &InviteCode, made: u64) -> bool {
        self.vault_write(|v| v.keep_invite(code, made))
    }

    /// The invite whose hash is `hash` taken back: its code forgotten.
    pub fn forget_invite(&mut self, hash: &[u8; 32]) -> bool {
        self.vault_write(|v| v.forget_invite(hash))
    }

    /// Every invite's code forgotten (a device removed had them all):
    /// their hashes, to take back on the server.
    pub fn forget_all_invites(&mut self) -> Vec<[u8; 32]> {
        let mut taken = Vec::new();
        self.vault_write(|v| {
            taken = v.forget_all_invites();
            !taken.is_empty()
        });
        taken
    }

    /// The server listed `waiting` as the invites still waiting: the codes
    /// of the others forgotten, when the list says which (used, taken
    /// back), and every one run out, whatever it says.
    pub fn forget_invites_but(&mut self, waiting: &[[u8; 32]], now: u64) -> bool {
        self.vault_write(|v| v.forget_invites_but(waiting, now))
    }

    /// The code of an invite this account made, by its hash, as shown.
    pub fn invite_code(&self, hash: &[u8; 32]) -> Option<String> {
        self.replica.as_ref()?.vault().invite_code(hash).map(|c| c.text())
    }

    fn vault_write(&mut self, write: impl FnOnce(&carnet_sync::NoteDoc) -> bool) -> bool {
        let Some(r) = self.replica.as_ref() else { return false };
        if !write(r.vault()) {
            return false;
        }
        let _ = r.persist_vault();
        self.vault_changed = true;
        true
    }

    /// What sync has to tell the server of things gone, since last asked.
    pub fn take_forgotten(&mut self) -> Forgotten {
        std::mem::take(&mut self.forgotten)
    }

    /// The pictures the notes show, as paths from the notes folder: every
    /// note's, those in the trash too (put back, they show them again).
    pub fn pictures_in_use(&self) -> BTreeSet<String> {
        let mut used = BTreeSet::new();
        for n in &self.notes {
            used.extend(doc::picture_paths(&n.folder, &n.markdown(), &self.files));
        }
        for (_, rel, folder) in self.in_trash() {
            let Ok(text) = fs::read_to_string(self.dir.join(&rel)) else { continue };
            used.extend(doc::picture_paths(&folder, &text, &self.files));
        }
        used
    }

    /// The titles of the notes in a folder in the trash.
    pub fn trashed_titles(&self, t: &Trashed) -> Vec<String> {
        let mut found = Found::default();
        let _ = walk(&t.path, "", 1, &mut found);
        let mut titles: Vec<String> = found.files.iter().filter_map(|(p, _)| Note::load(p).ok()).map(|n| n.display_title().to_string()).collect();
        titles.sort_by_key(|t| t.to_lowercase());
        titles
    }

    /// The pictures the notes with these keys show now.
    pub fn shown_by(&self, keys: &[Key]) -> BTreeSet<String> {
        self.notes
            .iter()
            .filter(|n| n.key.is_some_and(|k| keys.contains(&k)))
            .flat_map(|n| doc::picture_paths(&n.folder, &n.markdown(), &self.files))
            .collect()
    }

    /// The key the replica has for the file at `rel`.
    fn key_at(&self, rel: &str) -> Option<Key> {
        let r = self.replica.as_ref()?;
        r.keys().into_iter().find(|k| r.entry(*k).is_some_and(|e| e.path == rel))
    }

    /// Deletes `t` (a note, or a folder of them) from the trash forever,
    /// on every device: its file, its history, its place in the vault
    /// (marked gone), and its room on the server, through sync. The
    /// pictures it showed, in any version, that no other note shows now
    /// (one in the trash counts) go too: from the attachments folder, and
    /// dropped in the vault for the server to drop.
    pub fn delete_forever(&mut self, t: &Trashed) -> io::Result<()> {
        let files: Vec<PathBuf> = if t.is_folder {
            let mut found = Found::default();
            walk(&t.path, "", 1, &mut found)?;
            found.files.into_iter().map(|(p, _)| p).collect()
        } else {
            vec![t.path.clone()]
        };
        let mut keys = Vec::new();
        let mut shown = BTreeSet::new();
        for path in &files {
            let rel = path.strip_prefix(&self.dir).ok().map(|r| r.to_string_lossy().replace('\\', "/"));
            let key = rel.and_then(|rel| self.key_at(&rel));
            // Where it came from: what its pictures' paths are from.
            let folder = key.and_then(|k| self.replica.as_ref()?.vault().place(k)).map(|p| p.folder).unwrap_or_else(|| {
                let inner = path.parent().and_then(|p| p.strip_prefix(&t.path).ok()).map(|p| p.to_string_lossy().replace('\\', "/"));
                match (t.is_folder, inner) {
                    (true, Some(inner)) => [t.folder.as_str(), t.name.as_str(), inner.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("/"),
                    _ => t.folder.clone(),
                }
            });
            shown.extend(doc::picture_paths(&folder, &fs::read_to_string(path).unwrap_or_default(), &self.files));
            if let (Some(key), Some(r)) = (key, self.replica.as_mut()) {
                for rev in r.history(key) {
                    if let Ok(text) = r.text_at(key, &rev.version) {
                        shown.extend(doc::picture_paths(&folder, &text, &self.files));
                    }
                }
                keys.push(key);
            }
        }
        if t.is_folder {
            fs::remove_dir_all(&t.path)?;
        } else {
            fs::remove_file(&t.path)?;
        }
        self.note_origin(&t.path, None);
        for key in &keys {
            self.changed.retain(|k| k != key);
            if let Some(r) = self.replica.as_mut() {
                r.forget(*key)?;
                r.vault().set_gone(*key);
            }
        }
        self.forgotten.keys.extend(keys);
        self.drop_pictures(&shown);
        if let Some(r) = &self.replica {
            r.persist_vault()?;
            self.vault_changed = true;
        }
        Ok(())
    }

    /// Of `paths`, the pictures some version of a note still here shows:
    /// one restored would show them again. Only notes whose history may
    /// hold the picture's name are read version by version.
    fn shown_in_history(&mut self, paths: &BTreeSet<String>) -> BTreeSet<String> {
        let mut shown = BTreeSet::new();
        let files = self.files.clone();
        let Some(r) = self.replica.as_mut() else { return shown };
        for key in r.keys() {
            let names: Vec<&str> = paths.iter().filter(|p| !shown.contains(*p)).map(|p| p.rsplit('/').next().unwrap_or(p)).collect();
            if names.is_empty() {
                break;
            }
            if !names.iter().any(|n| r.may_have_held(key, n)) {
                continue;
            }
            let folder = r.vault().place(key).map(|p| p.folder).unwrap_or_default();
            for rev in r.history(key) {
                if let Ok(text) = r.text_at(key, &rev.version) {
                    for p in doc::picture_paths(&folder, &text, &files) {
                        if let Some(q) = paths.iter().find(|q| q.eq_ignore_ascii_case(&p)) {
                            shown.insert(q.clone());
                        }
                    }
                }
            }
        }
        shown
    }

    /// Of the pictures at `paths`, Carnet's (in the attachments folder)
    /// that no note shows now, nor any version of one: their files gone,
    /// and dropped in the vault. The user's own are never touched.
    fn drop_pictures(&mut self, paths: &BTreeSet<String>) {
        let paths: BTreeSet<String> = paths.iter().filter(|p| is_owned(p)).cloned().collect();
        let paths = &paths;
        let mut used = self.pictures_in_use();
        let rest: BTreeSet<String> = paths.difference(&used).cloned().collect();
        used.extend(self.shown_in_history(&rest));
        for p in paths.difference(&used) {
            if let Some(r) = &self.replica
                && r.vault().pictures().contains_key(p)
            {
                let (_, unused) = r.vault().drop_picture(p);
                self.forgotten.pictures.extend(unused);
            }
            if let Ok(file) = self.inside(self.dir.join(p)) {
                let _ = fs::remove_file(&file);
                mark_own(&file);
            }
        }
    }

    /// Notes another device deleted forever, gone here too: from the
    /// trash, or from the list if they were there unchanged since. One put
    /// back or moved here meanwhile is kept, as a new note of its own; one
    /// with edits not yet saved waits. `(changed, waiting)`.
    fn forget_gone(&mut self, known: &Layout) -> io::Result<(bool, bool)> {
        let Some(r) = self.replica.as_ref() else { return Ok((false, false)) };
        // Only those still here are looked for in the list: the vault
        // keeps every note ever deleted forever.
        let here: HashSet<Key> = self.notes.iter().filter_map(|n| n.key).collect();
        let gone: Vec<Key> = r.vault().gone().into_iter().filter(|k| here.contains(k) || r.entry(*k).is_some()).collect();
        let (mut changed, mut waiting) = (false, false);
        for key in gone {
            if let Some(i) = self.notes.iter().position(|n| n.key == Some(key)) {
                let n = &self.notes[i];
                if n.dirty {
                    waiting = true;
                    continue;
                }
                let untouched = known.notes.get(&key).is_some_and(|b| !b.trashed && b.folder == n.folder && b.pinned == n.pinned && n.path.as_deref().is_some_and(|p| stem_of(p) == b.name || numbered_as(p, &b.name)));
                if let Some(r) = self.replica.as_mut() {
                    r.forget(key)?;
                }
                if untouched {
                    if let Some(path) = self.notes[i].path.clone().and_then(|p| self.inside(p).ok()) {
                        let _ = fs::remove_file(&path);
                        mark_own(&path);
                    }
                    self.notes.remove(i);
                    self.save_pins()?;
                } else {
                    // Its own note from now on.
                    self.notes[i].key = None;
                    let text = self.notes[i].markdown();
                    self.track(i, &text, Origin::Save);
                }
                changed = true;
            } else if let Some(rel) = self.replica.as_ref().and_then(|r| r.entry(key)).map(|e| e.path.clone()) {
                let path = self.dir.join(&rel);
                if within(&self.dir, &path) {
                    let _ = fs::remove_file(&path);
                    mark_own(&path);
                }
                if rel.matches('/').count() == 1 {
                    self.note_origin(&path, None);
                }
                if let Some(r) = self.replica.as_mut() {
                    r.forget(key)?;
                }
                changed = true;
            } else {
                continue;
            }
            self.changed.retain(|k| *k != key);
            self.forgotten.keys.push(key);
        }
        Ok((changed, waiting))
    }

    /// The replica's folder, where sync keeps what it knows beside it.
    pub fn replica_dir(&self) -> Option<PathBuf> {
        self.replica.as_ref().map(|r| r.dir().to_path_buf())
    }

    /// Whether the vault doc took changes made here since last asked: what
    /// sync sends.
    pub fn take_vault_changed(&mut self) -> bool {
        std::mem::take(&mut self.vault_changed)
    }

    /// The files and the vault doc brought in step (docs/SYNC.md §4), by
    /// what each changed since the vault was last applied here:
    /// - a note moved, renamed, pinned, trashed or put back here, or a
    ///   folder made or gone, is written to the vault (with `send`: not
    ///   before this device has the account's vault, or a note's place
    ///   here would overwrite its place there);
    /// - one changed on another device is done to the files here;
    /// - both: this device's wins here, and the vault's merge settles it
    ///   between devices.
    ///
    /// A note with edits not yet saved waits, and with it the vault's
    /// version as applied. Returns whether anything changed here.
    pub fn reconcile(&mut self, send: bool) -> io::Result<bool> {
        let Some(vault) = self.vault_layout() else { return Ok(false) };
        let r = self.replica.as_ref().expect("checked");
        // As last applied: most often the vault as it is, not read twice.
        let known = if self.vault_read.as_ref().is_some_and(|(v, _)| v == r.applied()) { vault.clone() } else { Arc::new(r.vault().layout_at(r.applied())) };
        // Deleted forever elsewhere: first, so it isn't put back.
        let (mut changed, mut waiting) = self.forget_gone(&known)?;
        let local = self.layout(&vault);
        let mut wrote = false;

        // Folders made elsewhere, before the notes that go in them.
        for f in vault.folders.difference(&known.folders) {
            if !local.folders.contains(f) && !self.folders.iter().any(|g| same_stem(g, f)) {
                fs::create_dir_all(self.inside(self.folder_dir(f))?)?;
                self.folders.push(f.clone());
                changed = true;
            }
        }
        self.folders.sort_by_key(|f| f.to_lowercase());

        let keys: BTreeSet<Key> = vault.notes.keys().chain(local.notes.keys()).copied().collect();
        for key in keys {
            let (here, there, before) = (local.notes.get(&key), vault.notes.get(&key), known.notes.get(&key));
            if there.is_some() {
                self.from_remote.remove(&key);
            }
            if here == there {
                continue;
            }
            let merged = match (here, there) {
                // New here (or the vault lost it): all of it is this
                // device's to say.
                (Some(h), None) => h.clone(),
                // Not here: as the vault has it.
                (None, Some(t)) => t.clone(),
                // Both: field by field, the ones changed here since the
                // vault was last applied from here, the rest from the
                // vault. Never applied here (new to this device): the
                // vault's, all of it.
                (Some(h), Some(t)) => {
                    let base = before.unwrap_or(h);
                    Place {
                        folder: if h.folder != base.folder { h.folder.clone() } else { t.folder.clone() },
                        name: if h.name != base.name { h.name.clone() } else { t.name.clone() },
                        pinned: if h.pinned != base.pinned { h.pinned } else { t.pinned },
                        trashed: if h.trashed != base.trashed { h.trashed } else { t.trashed },
                    }
                }
                (None, None) => continue,
            };
            // What changed here goes to the vault (only the fields that
            // differ from it are written).
            if here.is_some() && Some(&merged) != there && send && !self.from_remote.contains(&key) {
                wrote |= self.replica.as_ref().is_some_and(|r| r.vault().set_place(key, &merged));
            }
            // What changed elsewhere is done here.
            if Some(&merged) != here {
                match self.apply_place(key, &merged)? {
                    Some(c) => changed |= c,
                    None => waiting = true,
                }
            }
        }

        // Folders gone elsewhere: removed here if nothing is in them.
        for f in known.folders.difference(&vault.folders) {
            if self.folders.iter().any(|g| g == f) && self.folder_is_empty(f) && self.delete_folder(f).is_ok() {
                changed = true;
            }
        }
        // Folders made or gone here, as the vault may hold them.
        if send {
            let here: BTreeSet<String> = self.folders.iter().map(|f| plain_folder(f)).collect();
            let r = self.replica.as_ref().expect("checked");
            for f in here.difference(&known.folders) {
                wrote |= r.vault().set_folder(f, true);
            }
            for f in known.folders.difference(&here) {
                if vault.folders.contains(f) {
                    wrote |= r.vault().set_folder(f, false);
                }
            }
        }

        changed |= self.settle_names()?;
        let r = self.replica.as_mut().expect("checked");
        if wrote {
            r.persist_vault()?;
            self.vault_changed = true;
        }
        if !waiting && r.applied() != &r.vault().version() {
            r.set_applied()?;
        }
        Ok(changed)
    }

    /// The vault's layout, read again only when the vault has changed
    /// since: what sync compares the files with every second is not
    /// 10,000 notes' places read anew each time.
    fn vault_layout(&mut self) -> Option<Arc<Layout>> {
        let vault = self.replica.as_ref()?.vault();
        let now = vault.version();
        match &self.vault_read {
            Some((v, layout)) if *v == now => Some(layout.clone()),
            _ => {
                let layout = Arc::new(vault.layout());
                self.vault_read = Some((now, layout.clone()));
                Some(layout)
            }
        }
    }

    /// Notes that want one name in one folder get it the same way on every
    /// device (docs/SYNC.md §4): by their keys, the first `name.md`, the
    /// next `name 2.md`, and so on. A file is renamed only into a free
    /// place. Two notes in each other's places (a swap) can't both wait:
    /// one steps aside to a spare number, and the other moves in.
    fn settle_names(&mut self) -> io::Result<bool> {
        let mut changed = false;
        for _ in 0..4 {
            if !self.settle_names_once()? {
                break;
            }
            changed = true;
        }
        Ok(changed)
    }

    fn settle_names_once(&mut self) -> io::Result<bool> {
        let Some(vault) = self.vault_layout() else { return Ok(false) };
        let mut groups: HashMap<(String, String), Vec<Key>> = HashMap::new();
        for (key, p) in vault.notes.iter().filter(|(_, p)| !p.trashed) {
            groups.entry((p.folder.to_lowercase(), p.name.to_lowercase())).or_default().push(*key);
        }
        // A number never lands on a name another note wants outright.
        let plain: HashSet<(String, String)> = groups.keys().cloned().collect();
        let mut changed = false;
        // Where each note is in the list, looked up once: renames here
        // leave every note where it is in it.
        let at: HashMap<Key, usize> = self.notes.iter().enumerate().filter_map(|(i, n)| Some((n.key?, i))).collect();
        for ((folder, name), keys) in groups.iter_mut() {
            keys.sort();
            let mut numbers = (2..).filter(|m| !plain.contains(&(folder.clone(), format!("{name} {m}"))));
            let wants: Vec<Option<usize>> = (0..keys.len()).map(|k| if k == 0 { None } else { numbers.next() }).collect();
            for (k, key) in keys.iter().enumerate() {
                let Some(&i) = at.get(key) else { continue };
                let (Some(path), place) = (self.notes[i].path.clone(), &vault.notes[key]) else { continue };
                if self.notes[i].dirty || self.notes[i].folder != place.folder {
                    continue;
                }
                let want = match wants[k] {
                    None => place.name.clone(),
                    Some(m) => format!("{} {m}", place.name),
                };
                if stem_of(&path) == want {
                    continue;
                }
                let dir = self.folder_dir(&place.folder);
                let held = |me: &Self, to: &Path| {
                    (to.exists() && !same_path(to, &path))
                        || me.notes.iter().any(|n| n.path.as_deref().is_some_and(|q| same_path(q, to) && !same_path(q, &path)))
                };
                let mut to = dir.join(format!("{want}.md"));
                if held(self, &to) {
                    // Held by another of this name, itself out of place: step
                    // aside, past every number this name needs.
                    let other = self.notes.iter().find(|n| n.path.as_deref().is_some_and(|q| same_path(q, &to))).and_then(|n| n.key);
                    if !other.is_some_and(|o| keys.contains(&o)) {
                        continue;
                    }
                    let mut n = wants.iter().flatten().max().copied().unwrap_or(1) + 1;
                    loop {
                        to = dir.join(format!("{} {n}.md", place.name));
                        if !held(self, &to) {
                            break;
                        }
                        n += 1;
                    }
                    let past = wants.iter().flatten().max().copied().unwrap_or(1);
                    if stem_of(&path).rsplit_once(' ').and_then(|(_, n)| n.parse::<usize>().ok()).is_some_and(|m| m > past) {
                        continue;
                    }
                }
                let to = self.inside(to)?;
                rename_retrying(&path, &to)?;
                self.notes[i].renamed_to(to);
                if let (Some(rel), Some(r)) = (self.notes[i].rel_path(), self.replica.as_mut()) {
                    let _ = r.moved(*key, &rel);
                }
                changed = true;
            }
        }
        if changed {
            self.save_pins()?;
        }
        Ok(changed)
    }

    /// A note new here that another device has in the trash: its file goes
    /// straight to the trash, saying where it came from.
    fn arrive_trashed(&mut self, key: Key, place: &Place, text: &str) -> io::Result<()> {
        let bin = self.dir.join(TRASH);
        fs::create_dir_all(&bin)?;
        let mut to = bin.join(format!("{}.md", place.name));
        let mut n = 2;
        while to.exists() {
            to = bin.join(format!("{} {n}.md", place.name));
            n += 1;
        }
        let to = self.inside(to)?;
        write_atomic(&to, text)?;
        self.note_origin(&to, Some((&place.folder, &format!("{}.md", place.name), place.pinned)));
        if let Some(r) = self.replica.as_mut() {
            let _ = r.record(key, &format!("{TRASH}/{}", file_name(&to)), text, Origin::OtherApp);
        }
        Ok(())
    }

    /// Note `key` put where the vault says, as another device left it.
    /// `Some(changed)`, or `None` to wait (it has edits not yet saved).
    fn apply_place(&mut self, key: Key, place: &Place) -> io::Result<Option<bool>> {
        if let Some(i) = self.notes.iter().position(|n| n.key == Some(key)) {
            if self.notes[i].dirty {
                return Ok(None);
            }
            if place.trashed {
                self.trash(i)?;
                return Ok(Some(true));
            }
            let mut changed = false;
            // Its text as it is: written for where it lives (a note that
            // came before its place), or rewritten by the device that
            // moved it, its edit on its way.
            if self.notes[i].folder != place.folder {
                self.place_note(i, &place.folder)?;
                changed = true;
            }
            // Renamed elsewhere (a file numbered here to make room is
            // already as it wants: the numbers are settled after).
            if let Some(path) = self.notes[i].path.clone()
                && stem_of(&path) != place.name
                && !numbered_as(&path, &place.name)
            {
                let to = self.inside(self.free_path(&place.name, Some(&path), &place.folder))?;
                if !same_path(&to, &path) {
                    rename_retrying(&path, &to)?;
                    self.notes[i].renamed_to(to);
                    if let (Some(rel), Some(r)) = (self.notes[i].rel_path(), self.replica.as_mut()) {
                        let _ = r.moved(key, &rel);
                    }
                    changed = true;
                }
            }
            if self.notes[i].pinned != place.pinned {
                self.notes[i].pinned = place.pinned;
                changed = true;
            }
            if changed {
                self.save_pins()?;
            }
            return Ok(Some(changed));
        }
        // In the trash here.
        let trashed_at = self.replica.as_ref().and_then(|r| r.entry(key)).map(|e| e.path.clone()).filter(|p| p.starts_with(&format!("{TRASH}/")));
        if let Some(rel) = trashed_at {
            let from = self.inside(self.dir.join(&rel))?;
            // Put back elsewhere: put back here.
            if !place.trashed && from.exists() {
                let i = self.restore(&from, &format!("{}.md", place.name), &place.folder)?;
                if self.notes[i].pinned != place.pinned {
                    self.notes[i].pinned = place.pinned;
                    self.save_pins()?;
                }
                return Ok(Some(true));
            }
            // Still in the trash, but come from somewhere else as the vault
            // has it (moved or renamed elsewhere before it was trashed):
            // where it goes back to, here too. One trashed alone is
            // re-recorded; one inside a folder trashed whole steps out of
            // it, to be a note in the trash of its own.
            if place.trashed && from.exists() {
                let from = if rel[TRASH.len() + 1..].contains('/') {
                    let bin = self.dir.join(TRASH);
                    let mut to = bin.join(format!("{}.md", place.name));
                    let mut n = 2;
                    while to.exists() {
                        to = bin.join(format!("{} {n}.md", place.name));
                        n += 1;
                    }
                    let to = self.inside(to)?;
                    rename_retrying(&from, &to)?;
                    if let Some(r) = self.replica.as_mut() {
                        let _ = r.moved(key, &format!("{TRASH}/{}", file_name(&to)));
                    }
                    to
                } else {
                    from
                };
                self.note_origin(&from, Some((&place.folder, &format!("{}.md", place.name), place.pinned)));
                return Ok(Some(true));
            }
        }
        // Not here yet: its text, when it comes, is put by its place.
        Ok(Some(false))
    }

    /// The notes whose files took a new text here since last asked: what
    /// sync sends.
    pub fn take_changed(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.changed)
    }

    /// Another device's changes to the note `key` came in (sync took them
    /// into its doc): its file follows. A note with no edits here is
    /// rewritten and reloaded; one with edits has them written beside
    /// the other device's, both kept; a note new here gets a file where
    /// the vault says it lives, or, before the vault has said, one named
    /// for its title at the top of the notes folder, moved there as it is
    /// once it does.
    pub fn remote_changed(&mut self, key: Key) -> io::Result<Remote> {
        let Some(replica) = self.replica.as_mut() else { return Ok(Remote::Same) };
        replica.persist(key)?;
        let text = replica.text(key);
        let Some(i) = self.notes.iter().position(|n| n.key == Some(key)) else {
            if text.is_empty() {
                return Ok(Remote::Same);
            }
            // In the trash here: its file there takes the text.
            let in_trash = replica.entry(key).map(|e| e.path.clone()).filter(|p| p.starts_with(&format!("{TRASH}/")));
            if let Some(rel) = in_trash {
                let path = self.dir.join(&rel);
                if within(&self.dir, &path) && path.exists() {
                    write_atomic(&path, &text)?;
                    if let Some(r) = self.replica.as_mut() {
                        let _ = r.record(key, &rel, &text, Origin::OtherApp);
                    }
                    return Ok(Remote::Same);
                }
            }
            // Where the vault says it goes; without a word from it yet, at
            // the top, named for its title, until it does.
            let place = replica.vault().place(key);
            if let Some(p) = place.as_ref().filter(|p| p.trashed) {
                return self.arrive_trashed(key, p, &text).map(|_| Remote::Same);
            }
            let (folder, stem) = match &place {
                Some(p) => (p.folder.clone(), p.name.clone()),
                None => {
                    let (title, _) = doc::from_markdown(&text);
                    (String::new(), slug(title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or("Untitled")))
                }
            };
            if place.is_none() {
                self.from_remote.insert(key);
            }
            fs::create_dir_all(self.inside(self.folder_dir(&folder))?)?;
            if !folder.is_empty() && !self.folders.contains(&folder) {
                self.folders.push(folder.clone());
                self.folders.sort_by_key(|f| f.to_lowercase());
            }
            let path = self.inside(self.free_path(&stem, None, &folder))?;
            write_atomic(&path, &text)?;
            self.edited_when(key, &path);
            let (mut note, text) = Note::load_text(&path)?;
            note.key = Some(key);
            note.folder = folder;
            note.pinned = place.as_ref().is_some_and(|p| p.pinned);
            self.notes.push(note);
            let i = self.notes.len() - 1;
            self.track(i, &text, Origin::OtherApp);
            if self.notes[i].pinned {
                self.save_pins()?;
            }
            return Ok(Remote::Created(i));
        };
        let Some(path) = self.notes[i].path.clone() else { return Ok(Remote::Same) };
        let before = fs::read_to_string(&path).unwrap_or_default();
        if self.notes[i].dirty {
            let editor = self.notes[i].markdown();
            let merged = self.replica.as_mut().map(|r| r.absorb(key, &editor)).transpose()?.flatten().unwrap_or(text);
            self.keep_before_sync(i, &path, &editor, &merged)?;
            write_atomic(&path, &merged)?;
            let (fresh, written) = self.notes[i].reread(&path)?;
            self.reloaded(i, fresh);
            self.track(i, &written, Origin::Save);
            return Ok(Remote::Merged(i));
        }
        if before == text {
            return Ok(Remote::Same);
        }
        // The file changed outside since it was last read or written here
        // (another app, a moment before this came, not yet looked at):
        // that change and this one merged, not the file written over.
        let outside = self.replica.as_ref().and_then(|r| r.entry(key)).is_some_and(|e| e.hash != carnet_sync::text_hash(&before));
        if outside && let Some(merged) = self.replica.as_mut().map(|r| r.absorb_as(key, &before, Origin::OtherApp)).transpose()?.flatten() {
            write_atomic(&path, &merged)?;
            let (fresh, written) = self.notes[i].reread(&path)?;
            self.reloaded(i, fresh);
            self.track(i, &written, Origin::OtherApp);
            return Ok(Remote::Updated(i));
        }
        self.keep_before_sync(i, &path, &before, &text)?;
        write_atomic(&path, &text)?;
        self.edited_when(key, &path);
        let (fresh, written) = self.notes[i].reread(&path)?;
        self.reloaded(i, fresh);
        self.track(i, &written, Origin::OtherApp);
        Ok(Remote::Updated(i))
    }

    /// A note written as another device left it: its file's modified time
    /// is when it was last edited there, as its history says, not when it
    /// came — what the list and the page say of it, and sort it by. (Never
    /// later than now: another device's clock may be ahead.)
    fn edited_when(&mut self, key: Key, path: &Path) {
        let Some(secs) = self.replica.as_mut().map(|r| r.modified(key)).filter(|s| *s > 0) else { return };
        let Some(t) = epoch_time(secs as u64) else { return };
        let t = t.min(SystemTime::now());
        if let Ok(f) = fs::File::options().write(true).open(path)
            && f.set_modified(t).is_ok()
        {
            // As the app left it: no other app's change to the watch.
            drop(f);
            mark_own(path);
        }
    }

    /// Note `i` reloaded as `fresh` with another device's change: what it
    /// was kept in [`before_remote`](Self::before_remote).
    fn reloaded(&mut self, i: usize, fresh: Note) {
        let was = std::mem::replace(&mut self.notes[i], fresh);
        self.before_remote.entry(was.id).or_insert(was.doc);
    }

    /// A change from another device that empties note `i`, or takes more
    /// than half of it, is applied: it's a real edit, and every copy must
    /// end the same. But the text from just before goes to the trash as
    /// `… (before sync).md`, and shows in Deleted (docs/SYNC.md §9).
    fn keep_before_sync(&self, i: usize, path: &Path, before: &str, after: &str) -> io::Result<()> {
        let (was, now) = (before.trim().chars().count(), after.trim().chars().count());
        if was == 0 || now * 2 >= was {
            return Ok(());
        }
        let bin = self.dir.join(TRASH);
        fs::create_dir_all(&bin)?;
        let stem = format!("{} (before sync)", stem_of(path));
        let mut to = bin.join(format!("{stem}.md"));
        let mut n = 2;
        while to.exists() {
            to = bin.join(format!("{stem} {n}.md"));
            n += 1;
        }
        write_atomic(&to, before)?;
        self.note_origin(&to, Some((&self.notes[i].folder, &file_name(path), false)));
        Ok(())
    }

    fn keep_both(&mut self, i: usize) -> io::Result<Saved> {
        let note = &self.notes[i];
        let (id, pinned) = (note.id, note.pinned);
        let path = note.path.clone().unwrap_or_default();
        let folder = note.folder.clone();
        let copy = self.free_path(&format!("{} (conflict)", stem_of(&path)), None, &folder);
        write_atomic(&copy, &note.markdown())?;
        let mut mine = Note::load(&copy)?;
        mine.pinned = false;
        mine.folder = folder.clone();
        match Note::load(&path) {
            Ok(mut fresh) => {
                fresh.pinned = pinned;
                // The same note, reread: it keeps its place.
                fresh.id = id;
                fresh.folder = folder;
                self.notes[i] = fresh;
                self.notes.push(mine);
                Ok(Saved::Conflict(self.notes.len() - 1))
            }
            // Gone from disk: ours is the only copy left, in the new file,
            // and it takes the note's place — no other note's index moves.
            Err(_) => {
                mine.id = id;
                self.notes[i] = mine;
                Ok(Saved::Conflict(i))
            }
        }
    }

    /// Writes every note with edits, except new ones nobody wrote in.
    pub fn save_all(&mut self) -> io::Result<()> {
        let mut first_err = None;
        for i in 0..self.notes.len() {
            if self.notes[i].path.is_none() && self.notes[i].is_blank() {
                continue;
            }
            if let Err(e) = self.save(i, true) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    /// The last resort when a save failed as the app closes: the note goes
    /// to a new file of its own beside the others, so nothing typed is lost.
    pub fn rescue(&mut self, i: usize) -> io::Result<PathBuf> {
        let note = &self.notes[i];
        let stem = format!("{} (unsaved)", slug(note.display_title()));
        let folder = note.folder.clone();
        let path = self.free_path(&stem, None, &folder);
        fs::write(&path, note.markdown())?;
        Ok(path)
    }

    /// Moves note `i`'s file to `.trash/`, then forgets the note. A failed
    /// move forgets nothing. Returns where the file went.
    pub fn trash(&mut self, i: usize) -> io::Result<Option<PathBuf>> {
        let mut gone = None;
        if let Some(path) = self.notes[i].path.clone()
            && path.exists()
        {
            let bin = self.dir.join(TRASH);
            fs::create_dir_all(&bin)?;
            let mut to = bin.join(file_name(&path));
            let mut n = 2;
            while to.exists() {
                to = bin.join(format!("{} {n}.md", stem_of(&path)));
                n += 1;
            }
            rename_retrying(&path, &to)?;
            // Pinned, it comes back pinned.
            self.note_origin(&to, Some((&self.notes[i].folder, &file_name(&path), self.notes[i].pinned)));
            // Kept in the replica at its place in the trash: not a note
            // gone that a new file might be.
            if let (Some(key), Some(r)) = (self.notes[i].key, self.replica.as_mut()) {
                let _ = r.moved(key, &format!("{TRASH}/{}", file_name(&to)));
            }
            gone = Some(to);
        }
        self.notes.remove(i);
        self.save_pins()?;
        Ok(gone)
    }

    /// Joining an account that has notes already: this device's welcome
    /// note, if nobody has touched it and no other note holds anything
    /// (a new note not yet written in doesn't count), goes, since
    /// the account's own comes with the rest and a second device would
    /// otherwise have two. It simply goes: its file, not to the trash
    /// (nobody wrote in it, so Deleted has nothing to give back), and out
    /// of the replica, so it is never sent. True if it went.
    pub fn drop_welcome(&mut self) -> io::Result<bool> {
        let fresh = welcome().markdown();
        let welcome = |n: &Note| !n.dirty && n.folder.is_empty() && n.path.is_some() && n.markdown() == fresh;
        let Some(i) = self.notes.iter().position(welcome) else { return Ok(false) };
        if self.notes.iter().enumerate().any(|(j, n)| j != i && !(n.is_blank() && n.path.is_none())) {
            return Ok(false);
        }
        let key = self.notes[i].key;
        if let Some(path) = self.notes[i].path.clone() {
            match fs::remove_file(self.inside(path)?) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        let pinned = self.notes.remove(i).pinned;
        if pinned {
            self.save_pins()?;
        }
        if let Some(key) = key {
            self.changed.retain(|k| *k != key);
            if let Some(r) = self.replica.as_mut() {
                r.forget(key)?;
            }
        }
        Ok(true)
    }

    /// Joined an account and caught up with it, this device's welcome
    /// note dropped on the way in ([`drop_welcome`](Store::drop_welcome)):
    /// put back if the account has none of its own (nor one in Deleted,
    /// which was let go on purpose), to sync as any note does, so the
    /// account has one. True if it was put back.
    pub fn welcome_unless_the_account_has_one(&mut self) -> io::Result<bool> {
        let title = welcome().title;
        let has = self.notes.iter().any(|n| n.title == title) || self.trashed().iter().any(|t| !t.is_folder && t.title == title);
        if has {
            return Ok(false);
        }
        self.notes.push(welcome());
        self.save(self.notes.len() - 1, true)?;
        Ok(true)
    }

    /// Brings a trashed file back as a note under its old name (or the
    /// first free one), in the folder it was in. Returns its index.
    pub fn restore(&mut self, from: &Path, name: &str, folder: &str) -> io::Result<usize> {
        let stem = name.strip_suffix(".md").unwrap_or(name);
        fs::create_dir_all(self.inside(self.folder_dir(folder))?)?;
        let to = self.inside(self.free_path(stem, None, folder))?;
        rename_retrying(from, &to)?;
        let pinned = self.origins().get(&file_name(from)).is_some_and(|o| o.3);
        self.note_origin(from, None);
        if !folder.is_empty() && !self.folders.iter().any(|f| f == folder) {
            // Its folder went too and is back now, empty but for it.
            self.folders.push(folder.to_string());
            self.folders.sort_by_key(|f| f.to_lowercase());
        }
        let (mut note, text) = Note::load_text(&to)?;
        note.folder = folder.to_string();
        note.pinned = pinned;
        self.notes.push(note);
        // Back where it was, or found by its text: the same note, history
        // and all.
        self.track(self.notes.len() - 1, &text, Origin::OtherApp);
        if pinned {
            self.save_pins()?;
        }
        Ok(self.notes.len() - 1)
    }

    /// Where the things in the trash came from, by their names there: the
    /// folder, the name, when it went, and whether it was pinned (a fifth
    /// column an older trash lacks: not).
    fn origins(&self) -> HashMap<String, (String, String, u64, bool)> {
        fs::read_to_string(self.dir.join(TRASH).join(ORIGINS))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut f = l.split('\t');
                let (gone, folder, name, when) = (f.next()?, f.next()?, f.next()?, f.next()?);
                let pinned = f.next() == Some("pinned");
                Some((gone.to_string(), (folder.to_string(), name.to_string(), when.parse().ok()?, pinned)))
            })
            .collect()
    }

    /// Says where `gone` (a path in the trash) came from, or forgets it
    /// once it is back. Kept as well as it can be: a trash that cannot
    /// say where a thing came from puts it back at the top level.
    fn note_origin(&self, gone: &Path, from: Option<(&str, &str, bool)>) {
        let key = file_name(gone);
        let mut all = self.origins();
        match from {
            Some((folder, name, pinned)) => {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                // After whatever went in this same millisecond, so the
                // trash still lists the later first: a note deleted and
                // its folder straight after came out either way round.
                // Not past a clock set back, which would date it ahead.
                let latest = all.values().map(|o| o.2).max().unwrap_or(0);
                let now = if latest >= now && latest - now < 1000 { latest + 1 } else { now };
                all.insert(key, (folder.to_string(), name.to_string(), now, pinned));
            }
            None => {
                all.remove(&key);
            }
        }
        let mut lines: Vec<String> = all
            .into_iter()
            .filter(|(k, (f, n, _, _))| ![k, f, n].iter().any(|s| s.contains(['\t', '\n'])))
            .map(|(k, (f, n, w, pinned))| format!("{k}\t{f}\t{n}\t{w}{}", if pinned { "\tpinned" } else { "" }))
            .collect();
        lines.sort();
        let _ = write_atomic(&self.dir.join(TRASH).join(ORIGINS), &lines.join("\n"));
    }

    /// Everything in the trash, the latest gone first.
    pub fn trashed(&self) -> Vec<Trashed> {
        let bin = self.dir.join(TRASH);
        let origins = self.origins();
        let Ok(entries) = fs::read_dir(&bin) else { return Vec::new() };
        let mut out = Vec::new();
        for e in entries.flatten() {
            let path = e.path();
            let key = file_name(&path);
            if key.starts_with('.') {
                continue;
            }
            let is_folder = path.is_dir();
            // The pictures the sweep took (store_owned.rs): not a folder
            // of notes, and put back by hand if ever.
            if is_folder && key.eq_ignore_ascii_case(ATTACHMENTS) {
                continue;
            }
            if !is_folder && path.extension().is_none_or(|x| !x.eq_ignore_ascii_case("md")) {
                continue;
            }
            let (folder, name, when) = match origins.get(&key) {
                Some((f, n, w, _)) => (f.clone(), n.clone(), UNIX_EPOCH.checked_add(Duration::from_millis(*w)).unwrap_or(UNIX_EPOCH)),
                None => (String::new(), key.clone(), e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH)),
            };
            let (title, snippet) = if is_folder {
                let mut found = Found::default();
                let _ = walk(&path, "", 0, &mut found);
                let n = found.files.len();
                (name.clone(), format!("Folder · {n} {}", if n == 1 { "note" } else { "notes" }))
            } else {
                match Note::load(&path) {
                    Ok(note) => (note.display_title().to_string(), note.doc.snippet()),
                    Err(_) => continue,
                }
            };
            out.push(Trashed { path, folder, name, title, snippet, is_folder, when });
        }
        out.sort_by_key(|t| std::cmp::Reverse(t.when));
        out
    }

    /// Puts one thing from the trash back where it came from — a folder
    /// under the first name free there. A note comes back as a note;
    /// returns its index.
    pub fn restore_trashed(&mut self, t: &Trashed) -> io::Result<Option<usize>> {
        if !t.is_folder {
            return self.restore(&t.path, &t.name, &t.folder).map(Some);
        }
        let join = |name: &str| if t.folder.is_empty() { name.to_string() } else { format!("{}/{name}", t.folder) };
        let mut to = join(&t.name);
        let mut n = 2;
        while self.folder_dir(&to).exists() {
            to = join(&format!("{} {n}", t.name));
            n += 1;
        }
        self.restore_folder(&t.path, &to)?;
        Ok(None)
    }

    pub fn save_pins(&self) -> io::Result<()> {
        let names: Vec<String> = self
            .notes
            .iter()
            .filter(|n| n.pinned)
            .filter_map(|n| n.path.as_deref().map(|p| pin_key(&n.folder, p)))
            .collect();
        write_atomic(&self.dir.join(PINS), &names.join("\n"))
    }

    /// `name.md`, or `name 2.md`, ... in `folder` — the first not taken by
    /// another file.
    fn free_path(&self, stem: &str, mine: Option<&Path>, folder: &str) -> PathBuf {
        let dir = self.folder_dir(folder);
        let mut n = 1;
        loop {
            let name = if n == 1 {
                format!("{stem}.md")
            } else {
                format!("{stem} {n}.md")
            };
            let p = dir.join(&name);
            let taken_by_other = |q: &Path| mine.is_none_or(|m| !same_path(m, q));
            let used_in_memory = self.notes.iter().any(|note| {
                note.path
                    .as_deref()
                    .is_some_and(|q| same_path(q, &p) && taken_by_other(q))
            });
            if !used_in_memory && (!p.exists() || !taken_by_other(&p)) {
                return p;
            }
            n += 1;
        }
    }
}

/// A file's modified time and length, `None` when it is not there.
fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

pub fn stem_of(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// File names compare the way the file systems people use do: without
/// case, in every script (`Äpfel` and `äpfel` are one file on NTFS/APFS).
fn same_stem(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// The folder pictures pasted or dropped into notes are kept in, at the
/// top of the notes folder.
pub const ATTACHMENTS: &str = "attachments";

/// A `src` that is a path relative to the note.
fn is_relative(src: &str) -> bool {
    !(src.contains("://") || src.starts_with("data:") || Path::new(src).is_absolute())
}

/// `%20` and friends back to their characters, as a link written by
/// another editor may have them.
pub fn decode_src(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // Two hex digits after the `%`, checked as bytes first: slicing
        // the string at a byte inside a character would panic.
        let hex = |x: u8| (x as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| src.to_string())
}

#[cfg(test)]
#[test]
fn percent_escapes_decode_and_odd_ones_stay() {
    assert_eq!(decode_src("My%20Pictures/a.png"), "My Pictures/a.png");
    assert_eq!(decode_src("a%😀.png"), "a%😀.png", "no panic inside a character");
    assert_eq!(decode_src("100%"), "100%");
    assert_eq!(decode_src("%e2%82%ac"), "€");
}

/// What [`Store::gather_pictures`] did.
#[derive(Debug, Default, PartialEq)]
pub struct Gathered {
    /// Files moved into the attachments folder, and how many of them are
    /// pictures.
    pub files: usize,
    pub pictures: usize,
    /// The notes whose links follow them there, by id.
    pub notes: Vec<u64>,
}

/// A path's parts, `..` and `.` worked out as written: never past its
/// root (`/..` is `/`), as the system works them out.
fn path_parts(p: &Path) -> Vec<String> {
    use std::path::Component;
    let mut v: Vec<String> = Vec::new();
    let mut root = 0;
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if v.len() > root {
                    v.pop();
                }
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir => {
                v.push(c.as_os_str().to_string_lossy().into_owned());
                root = v.len();
            }
            c => v.push(c.as_os_str().to_string_lossy().into_owned()),
        }
    }
    v
}

/// Whether `p` is the folder `dir` or in it, `..` worked out as written:
/// [`key_under`]'s question, `dir` itself counted.
fn within(dir: &Path, p: &Path) -> bool {
    let (d, q) = (path_parts(dir), path_parts(p));
    q.len() >= d.len() && d.iter().zip(&q).all(|(a, b)| a.eq_ignore_ascii_case(b))
}

/// A name as the vault may hold it (docs/SYNC.md §4): as it is, if it's
/// one name on every system ([`is_plain_name`](carnet_core::name::is_plain_name));
/// else made so as a title is ([`slug`]). A file a Mac lets be named
/// `a:b` is `a-b` to the other devices, which would read `a:b` as a
/// drive, and then here too, renamed so as a note renamed elsewhere is.
fn plain(name: &str) -> String {
    if carnet_core::name::is_plain_name(name) { name.to_string() } else { slug(name) }
}

/// A folder as the vault may hold it: each of its parts [`plain`].
fn plain_folder(folder: &str) -> String {
    if folder.is_empty() {
        return String::new();
    }
    folder.split('/').map(plain).collect::<Vec<_>>().join("/")
}

/// Whether a picture's `src` is a web address (or the picture itself, as
/// `data:`): not a file.
pub fn is_web(src: &str) -> bool {
    src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:")
}

/// Whether a path a note's text names is on another computer: a share
/// (`\\host\share\…`, `//host/…`, and `\\?\…`, `\\.\…` with them), or a
/// folder the system mounts from the network once looked in (`/net/…`,
/// `/Network/…`). Never looked at: on Windows, looking alone hands that
/// computer the user's login, and the window waits on it.
pub fn on_network(p: &Path) -> bool {
    let s = p.to_string_lossy();
    let b = s.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'/' | b'\\') && matches!(b[1], b'/' | b'\\') {
        return true;
    }
    let parts = path_parts(p);
    parts.len() >= 2 && matches!(parts[0].as_str(), "/" | "\\") && ["net", "Network"].iter().any(|n| parts[1].eq_ignore_ascii_case(n))
}

/// A path under the notes folder `dir` as a key: its parts from there,
/// joined with `/`, lowercased (one file, as a file system that ignores
/// case sees it). `None` for one outside it.
fn key_under(dir: &Path, p: &Path) -> Option<String> {
    let (d, q) = (path_parts(dir), path_parts(p));
    if q.len() <= d.len() || !d.iter().zip(&q).all(|(a, b)| a.eq_ignore_ascii_case(b)) {
        return None;
    }
    Some(q[d.len()..].join("/").to_lowercase())
}

/// The files in `dir` and under it, at most a few folders deep; hidden
/// ones (`.DS_Store`) and the ones being written left out.
fn files_under(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.ends_with(".tmp-carnet") {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                if depth < 4 {
                    out.extend(files_under(&e.path(), depth + 1));
                }
            }
            Ok(t) if t.is_file() => out.push(e.path()),
            _ => {}
        }
    }
    out
}

/// Removes `dir` and the folders in it, as far as they are empty; one with
/// anything in it stays, and so does `dir` then. The Finder's `.DS_Store`
/// (how it showed the folder) is not anything: it goes with its folder.
fn remove_empty_dirs(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_dirs(&e.path());
            }
        }
    }
    let only_finder = fs::read_dir(dir).is_ok_and(|es| es.flatten().all(|e| e.file_name() == ".DS_Store"));
    if only_finder {
        let _ = fs::remove_file(dir.join(".DS_Store"));
    }
    let _ = fs::remove_dir(dir);
}

/// Where each link's and picture's target is in Markdown text: `](…)`,
/// `](<…>)`, and an HTML `src="…"`. The byte ranges of the targets alone,
/// in order.
fn link_targets(md: &str) -> Vec<std::ops::Range<usize>> {
    let b = md.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = md[i..].find("](") {
        let start = i + at + 2;
        if b.get(start) == Some(&b'<') {
            let Some(len) = md[start + 1..].find(['>', '\n']) else { break };
            let end = start + 1 + len;
            if b[end] == b'>' && len > 0 {
                out.push(start + 1..end);
            }
            i = end;
            continue;
        }
        // Up to the `)` that closes it, parentheses inside kept in pairs.
        let mut depth = 0;
        let mut end = start;
        while end < b.len() {
            match b[end] {
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                b'\n' => break,
                _ => {}
            }
            end += 1;
        }
        if b.get(end) == Some(&b')') {
            // A title after it: `(src "title")`.
            let t = &md[start..end];
            let t = t.find(" \"").or_else(|| t.find(" '")).map_or(t, |s| &t[..s]).trim_end();
            if !t.is_empty() {
                out.push(start..start + t.len());
            }
        }
        i = end.max(start);
    }
    for q in ["src=\"", "src='"] {
        let close = q.as_bytes()[4] as char;
        let mut i = 0;
        while let Some(at) = md[i..].find(q) {
            let start = i + at + q.len();
            let Some(len) = md[start..].find([close, '\n', '>']) else { break };
            if md[start + len..].starts_with(close) && len > 0 {
                out.push(start..start + len);
            }
            i = start + len;
        }
    }
    out.sort_by_key(|r| r.start);
    out
}

/// Where each Obsidian link's and embed's target is: `[[target]]`,
/// `![[target|300]]`, on one line. The byte ranges of the targets alone,
/// spaces around them left out, in order.
fn wiki_targets(md: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = md[i..].find("[[") {
        let start = i + at + 2;
        let Some(len) = md[start..].find(['|', ']', '[', '\n']) else { break };
        let end = start + len;
        let line = &md[end..md[end..].find('\n').map_or(md.len(), |n| end + n)];
        if (line.starts_with('|') && line.contains("]]")) || line.starts_with("]]") {
            let t = &md[start..end];
            let lead = t.len() - t.trim_start().len();
            if !t.trim().is_empty() {
                out.push(start + lead..start + lead + t.trim().len());
            }
        }
        i = end;
    }
    out
}

/// A path as a link's target: a space, a parenthesis or an angle bracket
/// written as its `%` escape, so the link reads the same with or without
/// `<…>` around it.
fn encode_src(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '"' => out.push_str("%22"),
            '\'' => out.push_str("%27"),
            c => out.push(c),
        }
    }
    out
}

/// The path from directory `from` to `to`, `/`-separated, worked out from
/// the paths as written (both under the notes folder).
fn relative(from: &Path, to: &Path) -> String {
    let (a, b) = (path_parts(from), path_parts(to));
    let common = a
        .iter()
        .zip(&b)
        .take_while(|(x, y)| x.eq_ignore_ascii_case(y))
        .count();
    let mut out: Vec<String> = vec!["..".to_string(); a.len() - common];
    out.extend(b[common..].iter().cloned());
    out.join("/")
}

/// `stem.ext`, or `stem-2.ext`, ... — the first name not taken in `dir`.
/// With no `ext`, `stem`, `stem-2`, ...
fn free_file(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let dot = if ext.is_empty() { "" } else { "." };
    let mut n = 1;
    loop {
        let name = if n == 1 {
            format!("{stem}{dot}{ext}")
        } else {
            format!("{stem}-{n}{dot}{ext}")
        };
        let p = dir.join(name);
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

/// Now, as `2026-10-07-140322` (UTC): a name that sorts by time.
fn timestamp() -> String {
    wall_clock(jiff::Timestamp::now(), &jiff::tz::TimeZone::system())
}

/// `at` as the clock in `tz` says it, "2026-10-13-143626": a pasted
/// picture's name says when it was pasted as the clock on the wall did,
/// not as Greenwich's.
fn wall_clock(at: jiff::Timestamp, tz: &jiff::tz::TimeZone) -> String {
    at.to_zoned(tz.clone()).strftime("%Y-%m-%d-%H%M%S").to_string()
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp-carnet");
    {
        let mut f = fs::File::create(&tmp)?;
        io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
    }
    rename_retrying(&tmp, path)
}

/// The files the app wrote, renamed or removed itself, each as it left
/// it (its modified time and length, or gone): what the folder's watch
/// (watch.rs) leaves out, so a save, or a note come from another device,
/// is not taken for another app's change. Keyed as [`same_path`] keys.
static OWN: std::sync::Mutex<Option<Stamps>> = std::sync::Mutex::new(None);

/// Files by their [`path_key`], each as it was left: [`stamp`]ed.
type Stamps = HashMap<String, Option<(SystemTime, u64)>>;

/// The file at `path` is as the app left it: written, renamed, removed.
pub(crate) fn mark_own(path: &Path) {
    let mut own = OWN.lock().unwrap_or_else(|e| e.into_inner());
    let own = own.get_or_insert_with(HashMap::new);
    // Never more than a big folder's worth: the oldest are no use.
    if own.len() > 100_000 {
        own.clear();
    }
    own.insert(path_key(path), stamp(path));
}

/// Whether the file at `path` is still as the app left it: a change the
/// system says it had is the app's own.
pub fn is_own(path: &Path) -> bool {
    let own = OWN.lock().unwrap_or_else(|e| e.into_inner());
    own.as_ref().and_then(|o| o.get(&path_key(path))).is_some_and(|s| *s == stamp(path))
}

/// One file, as a file system that ignores case sees it.
fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// A path as [`same_path`] compares it: what a set of paths is keyed by.
fn path_key(p: &Path) -> String {
    let key = p.to_string_lossy().to_lowercase();
    // Windows takes `/` as `\`, and says `\`: a picture sync wrote at
    // `attachments/x.png` is the one the system names `attachments\x.png`.
    if cfg!(windows) { key.replace('/', "\\") } else { key }
}

/// How `.pinned` names a note: its path under the notes folder.
fn pin_key(folder: &str, path: &Path) -> String {
    let name = file_name(path);
    if folder.is_empty() {
        name.to_lowercase()
    } else {
        format!("{folder}/{name}").to_lowercase()
    }
}

/// What a walk of the notes folder found.
#[derive(Default)]
struct Found {
    /// Every note file, with the folder it is in.
    files: Vec<(PathBuf, String)>,
    /// The folders that are folders of notes.
    folders: Vec<String>,
    /// Folders of pictures other than the attachments folder: an export's
    /// `Attachments`, at any depth.
    pictures: Vec<String>,
    /// Every other file, by its path from where the walk began.
    others: Vec<String>,
}

/// Collects the notes and the folders under `dir`, at most eight deep.
/// A folder with no note in or under it, but other files (pictures, a
/// PDF), is not a folder of notes: an export's `Attachments`, an editor's
/// `images`. One named for the attachments folder is never one, at any
/// depth, unless notes are in it. Returns whether `dir` has notes in or
/// under it, and whether it has other files.
fn walk(dir: &Path, rel: &str, depth: usize, found: &mut Found) -> io::Result<(bool, bool)> {
    // The notes folder itself must be readable; a subfolder that is not
    // (permissions, a sync placeholder) is left out, not fatal.
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if depth == 0 => return Err(e),
        Err(_) => return Ok((false, false)),
    };
    let (mut notes, mut other) = (false, false);
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let name = file_name(&path);
        if name.starts_with('.') {
            continue;
        }
        // The type the folder's listing gives, with no look at the file
        // itself (10,000 of them add up): a link's, where it leads.
        let is_dir = match entry.file_type() {
            Ok(t) if !t.is_symlink() => t.is_dir(),
            _ => path.is_dir(),
        };
        if is_dir {
            let pictures = name.eq_ignore_ascii_case(ATTACHMENTS);
            // Pictures, not notes: the attachments folder; and one that
            // differs from it in case only, where case counts, is an
            // export's, its pictures to be gathered.
            if depth == 0 && pictures {
                for f in files_under(&path, 0) {
                    if let Ok(r) = f.strip_prefix(dir) {
                        found.others.push(r.to_string_lossy().replace('\\', "/"));
                    }
                }
                if name != ATTACHMENTS && !same_dir(&path, &dir.join(ATTACHMENTS)) {
                    found.pictures.push(name);
                }
                other = true;
                continue;
            }
            if depth >= 8 {
                continue;
            }
            let sub = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let at = found.folders.len();
            found.folders.push(sub.clone());
            let (has_notes, has_other) = walk(&path, &sub, depth + 1, found)?;
            if !has_notes && (has_other || pictures) {
                // Not a folder of notes, and nor is anything in it.
                found.folders.truncate(at);
                if pictures {
                    found.pictures.push(sub);
                }
            }
            notes |= has_notes;
            other |= has_other;
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            found.files.push((path, rel.to_string()));
            notes = true;
        } else {
            found.others.push(if rel.is_empty() { name } else { format!("{rel}/{name}") });
            other = true;
        }
    }
    Ok((notes, other))
}

/// Whether two paths are one folder: where the file system ignores case,
/// `Attachments` is `attachments`.
fn same_dir(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        same_path(a, b)
    }
}

/// `Title 3.md` is still the file for "Title": a collision suffix is not a
/// rename.
fn numbered_as(p: &Path, wanted: &str) -> bool {
    let stem = stem_of(p);
    stem.rsplit_once(' ').is_some_and(|(base, n)| {
        same_stem(base, wanted) && n.parse::<u32>().is_ok_and(|n| n >= 2)
    })
}

/// A file name for a title (in carnet-core, so the web viewer names a
/// note as the desktop does).
pub use carnet_core::name::slug;

/// A rename that rides out the moment another program (a sync client, an
/// indexer, a virus scanner) holds the file open, as Windows makes it fail.
fn rename_retrying(from: &Path, to: &Path) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => {
                mark_own(from);
                mark_own(to);
                return Ok(());
            }
            Err(e) if tries < 6 && e.kind() != io::ErrorKind::NotFound => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(40 * tries));
            }
            Err(e) => return Err(e),
        }
    }
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    write_made(path, text, None)
}

/// [`write_atomic`], the file born at `made` (by default when the one it
/// replaces was) where the system lets that be set: a save is a new file
/// taking the old one's place, which would be born anew at every save.
fn write_made(path: &Path, text: &str, made: Option<SystemTime>) -> io::Result<()> {
    let made = made.or_else(|| fs::metadata(path).and_then(|m| m.created()).ok());
    let tmp = path.with_extension("md.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        if let Some(t) = made.filter(|t| *t > UNIX_EPOCH) {
            keep_created(&f, t);
        }
        // On disk before it takes the old file's place: a power cut leaves
        // the old version or the new one, never an empty file.
        f.sync_all()?;
    }
    rename_retrying(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Sets a file's birth time, where the system has one that can be set.
fn keep_created(f: &fs::File, t: SystemTime) {
    #[cfg(target_os = "macos")]
    use std::os::macos::fs::FileTimesExt;
    #[cfg(windows)]
    use std::os::windows::fs::FileTimesExt;
    #[cfg(any(target_os = "macos", windows))]
    let _ = f.set_times(fs::FileTimes::new().set_created(t));
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (f, t);
}

/// `secs` since 1970 as a time, when it is one a day can be said of: past
/// the year 9999 (a server's mistake, or a server's harm) it is none,
/// where adding it to 1970 would overflow and stop the app.
pub fn epoch_time(secs: u64) -> Option<SystemTime> {
    const LAST: u64 = 253_402_300_799;
    (secs <= LAST).then(|| UNIX_EPOCH + Duration::from_secs(secs))
}

/// Whether a file a note links to is one to open in its own app: a
/// document, a picture, a recording, a film. Anything else (a program, a
/// script) is shown in its folder, never run from a link.
pub fn opens_safely(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "pdf" | "txt" | "md" | "rtf" | "csv" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx"
                | "pages" | "numbers" | "key" | "odt" | "ods" | "odp" | "ics" | "vcf"
                | "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "heic" | "heif" | "tif" | "tiff"
                | "m4a" | "mp3" | "wav" | "aac" | "aiff" | "caf" | "flac" | "ogg"
                | "mov" | "mp4" | "m4v" | "webm"
        )
    })
}

/// A file system's error in plain words: what went wrong, without the
/// system's number for it.
pub fn plain_error(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "it isn't there any more".to_string(),
        io::ErrorKind::PermissionDenied => "Carnet isn't allowed to change it".to_string(),
        io::ErrorKind::DirectoryNotEmpty => "there are still files in it".to_string(),
        io::ErrorKind::AlreadyExists => "something of that name is there already".to_string(),
        io::ErrorKind::StorageFull => "the disk is full".to_string(),
        io::ErrorKind::ReadOnlyFilesystem => "the disk can't be written to".to_string(),
        _ => {
            let s = e.to_string();
            match s.find(" (os error") {
                Some(at) => s[..at].to_string(),
                None => s,
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    /// What a test had the system open, in order: nothing is opened.
    pub static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Opens a web or mail address in the system's own app for it.
#[cfg(test)]
pub fn open_url(url: &str) -> io::Result<()> {
    OPENED.with_borrow_mut(|o| o.push(url.to_string()));
    Ok(())
}

/// Opens a web or mail address in the system's own app for it.
#[cfg(not(test))]
pub fn open_url(url: &str) -> io::Result<()> {
    use std::process::Command;
    #[cfg(windows)]
    Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn()?;
    #[cfg(target_os = "macos")]
    Command::new("open").arg(url).spawn()?;
    #[cfg(not(any(windows, target_os = "macos")))]
    Command::new("xdg-open").arg(url).spawn()?;
    Ok(())
}
/// Opens the OS file manager on `path`: a file selected in its folder, or a
/// folder itself. Linux file managers have no common "select this file",
/// so there it opens the folder.
pub fn reveal(path: &Path) -> io::Result<()> {
    use std::process::Command;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Explorer wants `/select,"C:\a b\c.md"` exactly; Rust's own
        // quoting would wrap the whole argument and Explorer ignores it.
        let arg = if path.is_dir() {
            format!("\"{}\"", path.display())
        } else {
            format!("/select,\"{}\"", path.display())
        };
        Command::new("explorer").raw_arg(arg).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        if !path.is_dir() {
            cmd.arg("-R");
        }
        cmd.arg(path).spawn()?;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        Command::new("xdg-open").arg(dir).spawn()?;
    }
    Ok(())
}

fn welcome() -> Note {
    use crate::keys;
    let k = keys::label;
    let item = |style, indent, done, text: &str| {
        Block::new(Kind::Item {
            style,
            indent,
            done,
            num: 0,
            text: text.into(),
        })
    };
    let cells = |rows: &[&[&str]]| {
        Table::from_cells(
            rows.iter()
                .map(|r| r.iter().map(|c| c.to_string()).collect())
                .collect(),
        )
    };
    let doc = Doc::new(vec![
            Block::text(
                "Carnet is a plain notes app: every note is a Markdown file in your Documents folder. \
                 It does two things properly: lists and tables.",
            ),
            Block::new(Kind::Heading {
                level: 2,
                text: "Type to make blocks".into(),
            }),
            item(ListStyle::Todo, 0, true, "Start a line with \"- \" for a bullet, \"1. \" for numbers, \"[] \" for a checklist"),
            item(ListStyle::Todo, 0, false, "\"# \" makes a heading, \"---\" a divider, \"/table\" and Enter a table"),
            item(ListStyle::Todo, 0, false, "Enter continues a list; Enter on an empty item steps out of it"),
            item(ListStyle::Todo, 1, false, "Tab indents an item, Shift+Tab brings it back"),
            Block::new(Kind::Heading {
                level: 2,
                text: "Tables that stay put".into(),
            }),
            Block::new(Kind::Table(cells(&[
                &["Key", "In a cell"],
                &["Tab / Shift+Tab", "next / previous cell"],
                &["Enter", "cell below; a new row at the end"],
                &["Enter in an empty last row", "leave the table"],
                &["Toolbar", "add or remove rows and columns, sort by the column you're in"],
            ]))),
            Block::text("Columns keep their width while you type, so the grid never jumps."),
            Block::new(Kind::Divider),
            // The shortcuts as this platform writes them (⌘B on a Mac).
            Block::text(format!(
                "The arrows walk from block to block and through tables; {} and {} go to the \
                 top and the bottom. Shift+arrows, or a drag, select across blocks; Esc selects \
                 the block you are in. {} copies Markdown, and {} undoes anything.",
                k(keys::TOP),
                k(keys::BOTTOM),
                k("mod+c"),
                k(keys::UNDO)
            )),
            Block::text(format!(
                "Type / on an empty line for every kind of block. Select some words and press {} \
                 for bold, {} for italic, {} to highlight, {} for a link; the toolbar colours \
                 them. The page shows them styled, and the file keeps them as Markdown, so it \
                 reads the same anywhere.",
                k(keys::BOLD),
                k(keys::ITALIC),
                k(keys::HIGHLIGHT),
                k(keys::LINK)
            )),
            Block::text(format!(
                "Paste a screenshot with {}, or drop a picture on the page: it is kept in the \
                 attachments folder beside your notes. {} opens the settings: colours, fonts, \
                 text size, density, where your notes live, and every shortcut.",
                k(keys::PASTE),
                k(keys::SETTINGS)
            )),
            Block::text(format!(
                "Drag a note onto a folder to move it there, or right-click it. {} makes a note, \
                 {} a folder, and {} searches.",
                k(keys::NEW_NOTE),
                k(keys::NEW_FOLDER),
                k(keys::SEARCH)
            )),
        ]);
    Note::new("Welcome to Carnet", doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pasted picture's name is the local time it was pasted at: 11:36
    /// UTC is 14:36 three hours east, and the day turns at local midnight.
    #[test]
    fn a_pasted_pictures_name_is_the_local_time() {
        let at: jiff::Timestamp = "2026-10-13T11:36:26Z".parse().unwrap();
        let east = jiff::tz::TimeZone::fixed(jiff::tz::offset(3));
        assert_eq!(wall_clock(at, &east), "2026-10-13-143626");
        assert_eq!(wall_clock(at, &jiff::tz::TimeZone::UTC), "2026-10-13-113626");
        let late: jiff::Timestamp = "2026-10-13T22:10:00Z".parse().unwrap();
        assert_eq!(wall_clock(late, &east), "2026-10-14-011000");
        // The name made now is this machine's clock's.
        let dir = temp_dir("pasted-local");
        let s = Store::open(dir.clone()).unwrap();
        let src = s.save_attachment("", "png", b"p").unwrap();
        let _ = fs::remove_dir_all(dir);
        let now = jiff::Zoned::now();
        let hour = now.strftime("%Y-%m-%d-%H").to_string();
        let hour_before = now.checked_sub(jiff::Span::new().minutes(1)).unwrap().strftime("%Y-%m-%d-%H").to_string();
        assert!(src.contains(&format!("pasted-{hour}")) || src.contains(&format!("pasted-{hour_before}")), "{src} at {now}");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("carnet-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let _ = fs::remove_dir_all(d.with_extension("replica"));
        d
    }

    /// A file changed by another program: a later time and a new length.
    fn edit_elsewhere(path: &Path, text: &str) {
        std::thread::sleep(Duration::from_millis(20));
        fs::write(path, text).unwrap();
    }

    /// A save is a new file taking the old one's place, and keeps its
    /// birth time: what a note's Created date is read from. A note that
    /// knows an older one (from its history) gives the file that.
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn a_save_keeps_the_files_birth_time() {
        let dir = temp_dir("birth");
        let mut s = Store::open(dir.clone()).unwrap();
        let path = s.notes[0].path.clone().unwrap();
        let secs = |t: SystemTime| t.duration_since(UNIX_EPOCH).unwrap().as_secs();
        let born = secs(fs::metadata(&path).unwrap().created().unwrap());
        std::thread::sleep(Duration::from_millis(1100));
        s.notes[0].touch();
        s.save(0, false).unwrap();
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(secs(meta.created().unwrap()), born);
        assert!(secs(meta.modified().unwrap()) > born, "saved");
        let older = UNIX_EPOCH + Duration::from_secs(born - 86_400);
        s.notes[0].created = older;
        s.notes[0].touch();
        s.save(0, false).unwrap();
        assert_eq!(secs(fs::metadata(&path).unwrap().created().unwrap()), born - 86_400);
        drop(s);
        assert_eq!(secs(Store::open(dir.clone()).unwrap().notes[0].created), born - 86_400, "and read back");
        let _ = fs::remove_dir_all(dir.with_extension("replica"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A replica a newer Carnet wrote is left alone, and says so; the
    /// notes open from their files as ever, without history.
    #[test]
    fn a_newer_replica_leaves_the_notes_open_without_history() {
        let dir = temp_dir("newer-replica");
        drop(Store::open(dir.clone()).unwrap());
        let replica = dir.with_extension("replica");
        fs::write(replica.join("version"), format!("{}\n", carnet_sync::LAYOUT + 1)).unwrap();
        let s = Store::open(dir.clone()).unwrap();
        assert_eq!(s.notes.len(), 1, "the welcome note, from its file");
        assert!(s.replica.is_none() && s.notes[0].key.is_none());
        assert_eq!(s.replica_refused.as_deref(), Some("These notes' history was written by a newer Carnet. Update Carnet to open it."));
        assert_eq!(s.replica_dir(), None, "nothing else written there either");
        assert_eq!(fs::read_to_string(replica.join("version")).unwrap(), format!("{}\n", carnet_sync::LAYOUT + 1));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&replica);
    }

    /// Its welcome note let go as it joined, a device caught up with the
    /// account puts it back only if the account has none of its own, nor
    /// one in Deleted: so the account has one, never two.
    #[test]
    fn a_welcome_note_comes_back_only_to_an_account_without_one() {
        let dir = temp_dir("welcome-back");
        let mut s = Store::open(dir.clone()).unwrap();
        assert!(s.drop_welcome().unwrap());
        // The account's notes came, none a welcome: it's back, to be sent.
        s.notes.push(Note::new("Plan", Doc::new(vec![Block::text("one")])));
        s.save(0, true).unwrap();
        let _ = s.take_changed();
        assert!(s.welcome_unless_the_account_has_one().unwrap());
        assert!(dir.join("Welcome to Carnet.md").exists());
        let key = s.notes.iter().find(|n| n.title == "Welcome to Carnet").and_then(|n| n.key).expect("kept to send");
        assert!(s.take_changed().contains(&key), "sent");
        // One there already (the account's): none more.
        assert!(!s.welcome_unless_the_account_has_one().unwrap());
        assert_eq!(s.notes.iter().filter(|n| n.title == "Welcome to Carnet").count(), 1);
        // The account's in Deleted: let go on purpose, not brought back.
        let i = s.notes.iter().position(|n| n.title == "Welcome to Carnet").unwrap();
        s.trash(i).unwrap();
        assert!(!s.welcome_unless_the_account_has_one().unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A device joining an account lets its own welcome note, untouched,
    /// go: not to Deleted, and out of what it syncs; touched, or beside
    /// other notes, it stays.
    #[test]
    fn a_joining_device_drops_its_untouched_welcome_only() {
        let dir = temp_dir("drop-welcome");
        let mut s = Store::open(dir.clone()).unwrap();
        let key = s.notes[0].key.expect("a key");
        s.changed.push(key);
        assert!(s.drop_welcome().unwrap());
        assert!(s.notes.is_empty());
        assert!(s.take_changed().is_empty(), "not sent");
        assert!(s.replica.as_ref().unwrap().keys().is_empty(), "nor kept to send later");
        assert!(!dir.join("Welcome to Carnet.md").exists(), "its file gone");
        assert!(!dir.join(".trash/Welcome to Carnet.md").exists(), "and not to the trash");
        assert!(s.trashed().is_empty(), "nothing in Deleted");
        let _ = fs::remove_dir_all(&dir);

        let dir = temp_dir("keep-welcome");
        let mut s = Store::open(dir.clone()).unwrap();
        s.notes[0].doc.push(Block::text("mine now"));
        s.notes[0].touch();
        s.save(0, true).unwrap();
        assert!(!s.drop_welcome().unwrap(), "touched: it stays");
        assert_eq!(s.notes.len(), 1);
        let _ = fs::remove_dir_all(&dir);

        // Beside a new note nobody wrote in, it goes; beside one with
        // anything in it, it stays.
        let dir = temp_dir("welcome-beside");
        let mut s = Store::open(dir.clone()).unwrap();
        s.notes.push(Note::new("Plans", Doc::default()));
        s.save(1, true).unwrap();
        assert!(!s.drop_welcome().unwrap());
        s.trash(1).unwrap();
        s.notes.push(Note::new("", Doc::new(vec![Block::text("")])));
        assert!(s.drop_welcome().unwrap());
        assert_eq!(s.notes.len(), 1, "the new note stays");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn welcome_on_first_open_then_rename_and_trash() {
        let dir = temp_dir("store");
        let mut s = Store::open(dir.clone()).unwrap();
        assert_eq!(s.notes.len(), 1);
        assert!(dir.join("Welcome to Carnet.md").exists());

        // Retitled here: the file follows once renaming is allowed.
        s.notes[0].title = "Hello: world?".into();
        s.notes[0].renamable = true;
        s.notes[0].touch();
        s.save(0, false).unwrap();
        assert!(dir.join("Welcome to Carnet.md").exists(), "not while typing");
        assert_eq!(s.save(0, true).unwrap(), Saved::Written);
        assert!(dir.join("Hello- world-.md").exists());
        assert!(!dir.join("Welcome to Carnet.md").exists());

        // A second note with the same title gets a suffix, and keeps it.
        s.notes.push(Note::new("Hello: world?", Doc::default()));
        s.save(1, true).unwrap();
        assert!(dir.join("Hello- world- 2.md").exists());
        s.notes[1].touch();
        s.save(1, true).unwrap();
        assert!(dir.join("Hello- world- 2.md").exists());

        // Pins survive a reopen; trash keeps the file.
        s.notes[1].pinned = true;
        s.save_pins().unwrap();
        let gone = s.trash(0).unwrap().unwrap();
        assert_eq!(gone, dir.join(".trash").join("Hello- world-.md"));
        let s = Store::open(dir.clone()).unwrap();
        assert_eq!(s.notes.len(), 1);
        assert!(s.notes[0].pinned);
        assert_eq!(s.notes[0].title, "Hello: world?");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_without_a_heading_stays_as_it_is() {
        let dir = temp_dir("noh1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("2024-05-01.md"), "Standup notes\r\n\r\n- one\r\n").unwrap();
        let mut s = Store::open(dir.clone()).unwrap();
        assert_eq!(s.notes[0].title, "2024-05-01");
        s.notes[0].doc.block_mut(1).text_mut().unwrap().push_str(" more");
        s.notes[0].touch();
        s.save(0, true).unwrap();
        // Same name, no heading added, CRLF kept.
        let back = fs::read_to_string(dir.join("2024-05-01.md")).unwrap();
        assert_eq!(back, "Standup notes\r\n\r\n- one more\r\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edit_made_elsewhere_is_never_overwritten() {
        let dir = temp_dir("conflict");
        let mut s = Store::open(dir.clone()).unwrap();
        let path = s.notes[0].path.clone().unwrap();

        // Not edited here: a refresh picks the other version up.
        edit_elsewhere(&path, "# Welcome to Carnet\n\nChanged in another editor.\n");
        assert!(s.refresh(0));
        assert_eq!(s.notes[0].doc.blocks()[0].text_ref(), Some("Changed in another editor."));

        // Edited in both places: the two are merged, into the file and
        // the note.
        s.notes[0].doc.block_mut(0).text_mut().unwrap().push_str(" And here.");
        s.notes[0].touch();
        edit_elsewhere(&path, "# Welcome to Carnet\n\nChanged in another editor, and again there.\n");
        assert_eq!(s.save(0, true).unwrap(), Saved::Merged);
        let merged = "# Welcome to Carnet\n\nChanged in another editor, and again there. And here.\n";
        assert_eq!(fs::read_to_string(&path).unwrap(), merged);
        assert_eq!(s.notes[0].markdown(), merged);
        assert!(!s.notes[0].dirty);
        assert_eq!(s.notes.len(), 1, "no copy");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Not a pass/fail bound: what the replica adds to opening a big
    /// folder, the first time (every note's doc made) and after.
    /// `cargo test --release opening_many -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn opening_many_notes_cost() {
        let dir = temp_dir("many");
        fs::create_dir_all(&dir).unwrap();
        let body: String = (0..30).map(|i| format!("- [ ] item {i}: something to remember\n")).collect();
        for n in 0..5000 {
            fs::write(dir.join(format!("Note {n}.md")), format!("# Note {n}\n\n{body}")).unwrap();
        }
        let time = |label: &str, replica: bool| {
            let t0 = std::time::Instant::now();
            let s = if replica {
                Store::open(dir.clone()).unwrap()
            } else {
                let mut s = Store { dir: dir.clone(), notes: Vec::new(), skipped: Vec::new(), folders: Vec::new(), picture_folders: Vec::new(), files: doc::Files::default(), gather_seen: HashSet::new(), replica: None, replica_refused: None, changed: Vec::new(), vault_changed: false, from_remote: HashSet::new(), forgotten: Forgotten::default(), before_remote: HashMap::new(), vault_read: None };
                for (path, _) in s.note_files().unwrap() {
                    s.notes.push(Note::load(&path).unwrap());
                }
                s
            };
            println!("{label}: {} notes in {:?}", s.notes.len(), t0.elapsed());
        };
        time("plain files", false);
        time("first open, every doc made", true);
        time("next open", true);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(dir.with_extension("replica"));
    }

    #[test]
    fn a_version_comes_back_and_the_restore_is_one_too() {
        let dir = temp_dir("restore");
        let mut s = Store::open(dir.clone()).unwrap();
        let path = s.notes[0].path.clone().unwrap();
        let welcome = fs::read_to_string(&path).unwrap();
        s.notes[0].doc = Doc::new(vec![Block::text("Short now.")]);
        s.notes[0].touch();
        s.save(0, false).unwrap();
        let h = s.history(0);
        assert_eq!(h.len(), 2, "the welcome note, then the save");
        assert_eq!(s.text_at(0, &h[1].version).as_deref(), Some(welcome.as_str()));

        s.restore_version(0, &h[1].version).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), welcome);
        assert_eq!(s.notes[0].markdown(), welcome);
        assert_eq!(s.notes[0].path.as_deref(), Some(path.as_path()), "the file keeps its name");
        let h = s.history(0);
        assert_eq!(h[0].origin, Some(Origin::Restore));
        // Edits not saved yet are not restored over.
        s.notes[0].touch();
        assert!(s.restore_version(0, &h[1].version).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    /// What happens to a notes folder, as the property test drives it.
    #[derive(Clone, Debug)]
    enum Op {
        /// A word typed into a note here, not saved yet.
        Type(proptest::sample::Index),
        Save(proptest::sample::Index),
        /// Another app adds a word to a note's file.
        Elsewhere(proptest::sample::Index),
        /// Another app renames a note's file; the window comes back.
        RenameElsewhere(proptest::sample::Index),
        /// The window comes back to the front.
        Rescan,
        /// Quit and start again.
        Restart,
        /// Quit, the replica gone (deleted, or a new machine), start again.
        LoseReplica,
        /// To the trash and straight back.
        TrashAndBack(proptest::sample::Index),
    }

    fn ops() -> impl proptest::strategy::Strategy<Value = Vec<Op>> {
        use proptest::prelude::*;
        let op = prop_oneof![
            4 => any::<proptest::sample::Index>().prop_map(Op::Type),
            3 => any::<proptest::sample::Index>().prop_map(Op::Save),
            3 => any::<proptest::sample::Index>().prop_map(Op::Elsewhere),
            1 => any::<proptest::sample::Index>().prop_map(Op::RenameElsewhere),
            1 => Just(Op::Rescan),
            1 => Just(Op::Restart),
            1 => Just(Op::LoseReplica),
            1 => any::<proptest::sample::Index>().prop_map(Op::TrashAndBack),
        ];
        proptest::collection::vec(op, 1..24)
    }

    /// Every `.md` file under `dir`, the trash's too, read.
    fn all_text(dir: &Path) -> String {
        let mut out = String::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "md") {
                    out.push_str(&fs::read_to_string(&p).unwrap_or_default());
                }
            }
        }
        out
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(
            std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(48)
        ))]

        /// Whatever happens to the folder — edits here and in another app
        /// at once, renames behind its back, restarts, a lost replica, the
        /// trash — no word anyone typed is ever lost: each is in a note's
        /// file, or in a copy kept beside it. A note saved has its file's
        /// text in the replica, and keeps its key across a rename made
        /// elsewhere and a restart.
        #[test]
        fn no_word_typed_anywhere_is_lost(ops in ops()) {
            use std::sync::atomic::{AtomicU64, Ordering};
            static RUN: AtomicU64 = AtomicU64::new(0);
            let dir = temp_dir(&format!("model-{}", RUN.fetch_add(1, Ordering::Relaxed)));
            let mut s = Store::open(dir.clone()).unwrap();
            for name in ["Shop", "Plan"] {
                s.notes.push(Note::new(name, Doc::new(vec![Block::text(format!("{name} starts here."))])));
                let last = s.notes.len() - 1;
                s.save(last, true).unwrap();
            }
            let mut typed: Vec<String> = Vec::new();
            let mut renames = 0;
            let word = |typed: &mut Vec<String>| {
                let w = format!("w{:04}x", typed.len());
                typed.push(w.clone());
                w
            };
            let pick = |s: &Store, i: &proptest::sample::Index| (!s.notes.is_empty()).then(|| i.index(s.notes.len()));
            for op in ops {
                match op {
                    Op::Type(i) => if let Some(i) = pick(&s, &i) {
                        let w = word(&mut typed);
                        s.notes[i].doc.push(Block::text(w));
                        s.notes[i].touch();
                    },
                    Op::Save(i) => if let Some(i) = pick(&s, &i) {
                        let saved = s.save(i, true).unwrap();
                        if matches!(saved, Saved::Written | Saved::Merged)
                            && let (Some(key), Some(path), Some(r)) = (s.notes[i].key, s.notes[i].path.clone(), s.replica.as_ref())
                        {
                            let file = fs::read_to_string(&path).unwrap();
                            proptest::prop_assert_eq!(r.entry(key).map(|e| e.hash), Some(carnet_sync::text_hash(&file)));
                        }
                    },
                    Op::Elsewhere(i) => if let Some(i) = pick(&s, &i)
                        && let Some(path) = s.notes[i].path.clone()
                        && path.exists()
                    {
                        let w = word(&mut typed);
                        let was = fs::read_to_string(&path).unwrap();
                        fs::write(&path, format!("{}\n\n{w}\n", was.trim_end())).unwrap();
                    },
                    Op::RenameElsewhere(i) => if let Some(i) = pick(&s, &i)
                        && !s.notes[i].dirty
                        && let Some(path) = s.notes[i].path.clone()
                        && path.exists()
                        && s.notes[i].key.is_some()
                    {
                        let key = s.notes[i].key;
                        renames += 1;
                        let to = path.with_file_name(format!("Renamed {renames}.md"));
                        fs::rename(&path, &to).unwrap();
                        s.rescan();
                        let moved = s.notes.iter().find(|n| n.path.as_deref() == Some(to.as_path()));
                        // Its text is its own (every note has its own words),
                        // so the replica knows it by them.
                        proptest::prop_assert_eq!(moved.and_then(|n| n.key), key);
                    },
                    Op::Rescan => { s.rescan(); }
                    Op::Restart | Op::LoseReplica => {
                        s.save_all().unwrap();
                        let keys: Vec<(PathBuf, Option<Key>)> = s.notes.iter().filter_map(|n| Some((n.path.clone()?, n.key))).collect();
                        drop(s);
                        let lose = matches!(op, Op::LoseReplica);
                        if lose {
                            let _ = fs::remove_dir_all(dir.with_extension("replica"));
                        }
                        s = Store::open(dir.clone()).unwrap();
                        if !lose {
                            for (path, key) in keys {
                                let now = s.notes.iter().find(|n| n.path.as_deref() == Some(path.as_path())).and_then(|n| n.key);
                                proptest::prop_assert_eq!(now, key, "{:?}", path);
                            }
                        }
                    }
                    Op::TrashAndBack(i) => if let Some(i) = pick(&s, &i)
                        && !s.notes[i].dirty
                        && let Some(path) = s.notes[i].path.clone()
                        && path.exists()
                    {
                        let (name, folder) = (file_name(&path), s.notes[i].folder.clone());
                        if let Some(gone) = s.trash(i).unwrap() {
                            s.restore(&gone, &name, &folder).unwrap();
                        }
                    },
                }
            }
            s.save_all().unwrap();
            let all = all_text(&dir);
            for w in &typed {
                proptest::prop_assert!(all.contains(w.as_str()), "{w} lost");
            }
            drop(s);
            let _ = fs::remove_dir_all(dir.with_extension("replica"));
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn with_nothing_to_merge_from_both_versions_are_kept() {
        let dir = temp_dir("conflict-plain");
        let mut s = Store::open(dir.clone()).unwrap();
        s.replica = None;
        let path = s.notes[0].path.clone().unwrap();
        s.notes[0].doc.block_mut(0).text_mut().unwrap().push_str(" And here.");
        s.notes[0].touch();
        edit_elsewhere(&path, "# Welcome to Carnet\n\nChanged there again, longer.\n");
        let Saved::Conflict(mine) = s.save(0, true).unwrap() else { panic!("no conflict") };
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# Welcome to Carnet\n\nChanged there again, longer.\n"
        );
        let copy = s.notes[mine].path.clone().unwrap();
        assert!(fs::read_to_string(copy).unwrap().contains("And here."));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_keeps_its_key_across_runs_renames_and_the_trash() {
        let dir = temp_dir("keys");
        let s = Store::open(dir.clone()).unwrap();
        let key = s.notes[0].key.expect("the welcome note has a key");
        let path = s.notes[0].path.clone().unwrap();
        // Renamed in another app, while closed.
        drop(s);
        fs::rename(&path, dir.join("Renamed elsewhere.md")).unwrap();
        let mut s = Store::open(dir.clone()).unwrap();
        assert_eq!(s.notes[0].key, Some(key));
        // Moved to a folder here, trashed, put back.
        s.create_folder("", "Archive").unwrap();
        s.move_note(0, "Archive").unwrap();
        let gone = s.trash(0).unwrap().unwrap();
        let back = s.restore(&gone, "Renamed elsewhere.md", "Archive").unwrap();
        assert_eq!(s.notes[back].key, Some(key));
        let replica = s.replica.as_ref().unwrap();
        assert_eq!(replica.entry(key).unwrap().path, "Archive/Renamed elsewhere.md");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rescan_follows_the_folder() {
        let dir = temp_dir("rescan");
        let mut s = Store::open(dir.clone()).unwrap();
        fs::write(dir.join("From elsewhere.md"), "# From elsewhere\n\nhi\n").unwrap();
        assert!(s.rescan());
        assert_eq!(s.notes.len(), 2);
        fs::remove_file(dir.join("From elsewhere.md")).unwrap();
        assert!(s.rescan());
        assert_eq!(s.notes.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 10,000 notes (`CARNET_SCALE` to try another number): put in the
    /// vault, compared with it again as sync does every second, and the
    /// folder looked at again as when the window comes to the front, then
    /// with files new to it. Each takes time as the notes grow, not as
    /// their square: the bounds are many times what it takes, in a debug
    /// build, and far below what the square of 10,000 took.
    #[test]
    fn ten_thousand_notes_stay_quick() {
        let n: usize = std::env::var("CARNET_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(10_000);
        let dir = temp_dir("scale");
        fs::create_dir_all(dir.join("Bulk")).unwrap();
        for i in 0..n {
            fs::write(dir.join("Bulk").join(format!("Note {i}.md")), format!("# Note {i}\n\nWords of note {i}.\n")).unwrap();
        }
        let timed = |label: &str, f: &mut dyn FnMut()| {
            let t0 = std::time::Instant::now();
            f();
            let took = t0.elapsed();
            println!("{n} notes, {label}: {took:?}");
            took
        };
        let mut s = None;
        timed("open", &mut || s = Some(Store::open(dir.clone()).unwrap()));
        let mut s = s.unwrap();
        assert_eq!(s.notes.len(), n);
        let first = timed("first reconcile, every place written", &mut || {
            s.reconcile(true).unwrap();
        });
        assert_eq!(s.replica.as_ref().unwrap().vault().layout().notes.len(), n);
        let again = timed("reconcile again, nothing new", &mut || {
            assert!(!s.reconcile(true).unwrap());
        });
        let rescan = timed("rescan, nothing new", &mut || assert!(!s.rescan()));
        for i in 0..40 {
            fs::write(dir.join("Bulk").join(format!("New {i}.md")), format!("# New {i}\n")).unwrap();
        }
        let new = timed("rescan, 40 new", &mut || assert!(s.rescan()));
        assert_eq!(s.notes.len(), n + 40);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(dir.with_extension("replica"));
        if n >= 10_000 {
            let secs = Duration::from_secs;
            assert!(first < secs(20), "first reconcile took {first:?}");
            assert!(again < secs(3), "reconcile took {again:?}");
            assert!(rescan < secs(2) && new < secs(2), "rescan took {rescan:?}, {new:?}");
        }
    }

    #[test]
    fn quitting_does_not_write_blank_notes() {
        let dir = temp_dir("blank");
        let mut s = Store::open(dir.clone()).unwrap();
        s.notes.push(Note::new("", Doc::new(vec![Block::text("")])));
        s.save_all().unwrap();
        let files = fs::read_dir(&dir)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "md"))
            .count();
        assert_eq!(files, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stems_match_without_case() {
        assert!(same_stem("Äpfel", "äpfel"));
    }

    #[test]
    fn a_folder_renamed_takes_its_subfolders_notes_and_pins() {
        let dir = temp_dir("rename-folder");
        let mut s = Store::open(dir.clone()).unwrap();
        let work = s.create_folder("", "Work").unwrap();
        let plans = s.create_folder(&work, "Plans").unwrap();
        s.create_folder("", "Home").unwrap();
        for (title, folder) in [("Trip", &work), ("Q4", &plans)] {
            s.notes.push(Note::new(title, Doc::new(vec![Block::text("x")])));
            let i = s.notes.len() - 1;
            s.notes[i].folder = folder.clone();
            s.notes[i].pinned = title == "Q4";
            s.notes[i].touch();
            s.save(i, true).unwrap();
        }
        s.save_pins().unwrap();
        assert_eq!(s.rename_folder("Work", "Job: old").unwrap(), "Job- old");
        assert_eq!(s.folders, ["Home", "Job- old", "Job- old/Plans"]);
        assert!(dir.join("Job- old").join("Plans").join("Q4.md").exists());
        assert!(!dir.join("Work").exists());
        let q4 = s.notes.iter().find(|n| n.title == "Q4").unwrap();
        assert_eq!(q4.folder, "Job- old/Plans");
        assert_eq!(q4.path.as_deref(), Some(dir.join("Job- old").join("Plans").join("Q4.md").as_path()));
        // Read again from disk: the same, the pin kept.
        let again = Store::open(dir.clone()).unwrap();
        assert_eq!(again.folders, s.folders);
        assert!(again.notes.iter().find(|n| n.title == "Q4").is_some_and(|n| n.pinned));
        // A subfolder renames in its parent; a taken name is refused; the
        // same name is nothing to do; a change of case is a rename.
        assert_eq!(s.rename_folder("Job- old/Plans", "Ideas").unwrap(), "Job- old/Ideas");
        assert!(s.rename_folder("Home", "Job- old").is_err());
        assert_eq!(s.rename_folder("Home", "Home").unwrap(), "Home");
        assert_eq!(s.rename_folder("Home", "home").unwrap(), "home");
        assert!(s.folders.contains(&"home".to_string()));
        assert!(s.rename_folder("home", "attachments").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_move_keeps_an_edit_made_elsewhere_and_attachments_is_not_a_folder() {
        let dir = temp_dir("move-ext");
        let mut s = Store::open(dir.clone()).unwrap();
        s.notes.push(Note::new("Trip", Doc::new(vec![Block::text("old")])));
        let i = s.notes.len() - 1;
        s.notes[i].touch();
        s.save(i, true).unwrap();
        // A sync writes a newer version while Carnet is in front.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(dir.join("Trip.md"), "# Trip\n\nNEW FROM SYNC\n").unwrap();
        let work = s.create_folder("", "Work").unwrap();
        s.move_note(i, &work).unwrap();
        let moved = fs::read_to_string(dir.join("Work").join("Trip.md")).unwrap();
        assert!(moved.contains("NEW FROM SYNC"), "{moved}");
        assert!(s.notes[i].doc.contains("new from sync"), "and in memory");
        assert_eq!(s.notes[i].folder, "Work");
        assert!(s.create_folder("", "Attachments").is_err());
        assert!(s.create_folder("Work", "Attachments").is_err(), "at any depth");
        let _ = fs::remove_dir_all(&dir);
    }

    /// An export from Apple Notes, as its Markdown export lays it out: a
    /// folder of notes with an `Attachments` folder beside them, linked
    /// by `Attachments/…` with spaces as `%20`.
    fn apple_export(dir: &Path) {
        let ex = dir.join("Archive");
        let at = ex.join("Attachments");
        fs::create_dir_all(&at).unwrap();
        for f in ["IMG_0412.jpeg", "Tram ride.jpeg", "Boarding pass.pdf", "New Recording.m4a", "Unlinked.png", "odd.png"] {
            fs::write(at.join(f), format!("bytes of {f}")).unwrap();
        }
        fs::write(at.join(".DS_Store"), "finder").unwrap();
        fs::write(
            ex.join("Trip to Lisbon.md"),
            "# Trip to Lisbon\n\nUp to the castle.\n\n![](Attachments/IMG_0412.jpeg)\n\n![The tram](Attachments/Tram%20ride.jpeg)\n\nTickets: [Boarding pass.pdf](Attachments/Boarding%20pass.pdf)\n",
        )
        .unwrap();
        fs::write(ex.join("Recipes.md"), "# Recipes\n\n- [ ] eggs\n\n<img src=\"Attachments/IMG_0412.jpeg\" width=\"300\">\n").unwrap();
        fs::write(ex.join("Voice memo.md"), "# Voice memo\n\n[New Recording.m4a](<Attachments/New Recording.m4a>)\n").unwrap();
        // Named twice: by a link, and in words, which can't follow.
        fs::write(ex.join("Odd.md"), "# Odd\n\n![](Attachments/odd.png)\n\nIt is Attachments/odd.png.\n").unwrap();
    }

    #[test]
    fn an_exports_attachments_are_pictures_not_a_folder_and_gather_once() {
        let dir = temp_dir("gather");
        let mut s = Store::open(dir.clone()).unwrap();
        // A picture of that name already there: not written over.
        fs::create_dir_all(dir.join(ATTACHMENTS)).unwrap();
        fs::write(dir.join(ATTACHMENTS).join("IMG_0412.jpeg"), "mine").unwrap();
        apple_export(&dir);
        s.rescan();
        assert_eq!(s.folders, ["Archive"], "Attachments is no folder of notes");
        assert_eq!(s.picture_folders, ["Archive/Attachments"]);
        let trip = |s: &Store| s.notes.iter().position(|n| n.title == "Trip to Lisbon").unwrap();
        // Shown from where they are, before anything moves.
        let src = |s: &Store, i: usize| match &s.notes[i].doc.blocks()[1].kind {
            Kind::Image { src, .. } => src.clone(),
            k => panic!("{k:?}"),
        };
        assert!(s.resolve("Archive", &src(&s, trip(&s))).unwrap().exists());

        let g = s.gather_pictures();
        assert_eq!((g.files, g.pictures), (4, 2), "{g:?}");
        assert_eq!(g.notes.len(), 3);
        let pics = dir.join(ATTACHMENTS);
        assert_eq!(fs::read_to_string(pics.join("IMG_0412.jpeg")).unwrap(), "mine", "never written over");
        assert_eq!(fs::read_to_string(pics.join("IMG_0412-2.jpeg")).unwrap(), "bytes of IMG_0412.jpeg");
        assert!(pics.join("Tram-ride.jpeg").exists() && pics.join("Boarding-pass.pdf").exists() && pics.join("New-Recording.m4a").exists());
        let read = |n: &str| fs::read_to_string(dir.join("Archive").join(n)).unwrap();
        assert_eq!(
            read("Trip to Lisbon.md"),
            "# Trip to Lisbon\n\nUp to the castle.\n\n![](../attachments/IMG_0412-2.jpeg)\n\n![The tram](../attachments/Tram-ride.jpeg)\n\nTickets: [Boarding pass.pdf](../attachments/Boarding-pass.pdf)\n"
        );
        assert!(read("Recipes.md").contains("<img src=\"../attachments/IMG_0412-2.jpeg\" width=\"300\">"));
        assert!(read("Voice memo.md").contains("](<../attachments/New-Recording.m4a>)"));
        assert!(s.resolve("Archive", &src(&s, trip(&s))).unwrap().exists(), "the note in memory follows");
        // What no note links to, and what one names in a way that can't
        // follow, stay; so does their folder.
        let ex = dir.join("Archive").join("Attachments");
        assert!(ex.join("Unlinked.png").exists() && ex.join("odd.png").exists());
        assert!(!ex.join("IMG_0412.jpeg").exists() && !ex.join("Tram ride.jpeg").exists());
        assert!(read("Odd.md").contains("![](Attachments/odd.png)"));
        // Once: nothing more to do.
        assert_eq!(s.gather_pictures(), Gathered::default());
        let _ = fs::remove_dir_all(&dir);
    }

    /// An Obsidian vault's embeds, `![[photo.png]]`: the pictures they find
    /// by name join the attachments folder, and the embeds stay embeds,
    /// named by the pictures' new paths. One whose new name another embed
    /// finds a picture by elsewhere stays where it is.
    #[test]
    fn obsidian_embeds_gather_and_stay_embeds() {
        let dir = temp_dir("gather-wiki");
        let mut s = Store::open(dir.clone()).unwrap();
        let (vault, at) = (dir.join("Vault"), dir.join("Vault").join("Attachments"));
        fs::create_dir_all(&at).unwrap();
        fs::create_dir_all(dir.join("Elsewhere")).unwrap();
        for f in ["photo.png", "beach.png", "sun.png"] {
            fs::write(at.join(f), format!("bytes of {f}")).unwrap();
        }
        fs::write(dir.join("Elsewhere").join("sun.png"), "another sun").unwrap();
        let trip = "# Trip\n\n![[photo.png]]\n\n![[Attachments/beach.png|300]]\n\nSee [[photo.png]] too.\n\n![[Attachments/sun.png]]\n";
        fs::write(vault.join("Trip.md"), trip).unwrap();
        // Finds the other sun by its name: the shortest path.
        fs::write(dir.join("Sky.md"), "# Sky\n\n![[sun.png|the sun]]\n").unwrap();
        s.rescan();
        assert_eq!(s.picture_folders, ["Vault/Attachments"]);
        let shown = s.pictures_in_use();
        assert!(["Vault/Attachments/photo.png", "Vault/Attachments/beach.png", "Elsewhere/sun.png"].iter().all(|p| shown.contains(*p)), "{shown:?}");
        assert_eq!(s.picture_file("Vault", "photo.png", true), Some(at.join("photo.png")));

        let g = s.gather_pictures();
        assert_eq!((g.files, g.pictures, g.notes.len()), (2, 2, 1), "{g:?}");
        let pics = dir.join(ATTACHMENTS);
        assert_eq!(fs::read_to_string(pics.join("photo.png")).unwrap(), "bytes of photo.png");
        assert_eq!(fs::read_to_string(pics.join("beach.png")).unwrap(), "bytes of beach.png");
        assert_eq!(
            fs::read_to_string(vault.join("Trip.md")).unwrap(),
            "# Trip\n\n![[attachments/photo.png]]\n\n![[attachments/beach.png|300]]\n\nSee [[attachments/photo.png]] too.\n\n![[Attachments/sun.png]]\n"
        );
        assert!(at.join("sun.png").exists() && !pics.join("sun.png").exists(), "the sky's sun would be found in its place");
        assert_eq!(fs::read_to_string(dir.join("Sky.md")).unwrap(), "# Sky\n\n![[sun.png|the sun]]\n");
        // Shown from where they are now, by the note in memory too.
        let i = s.notes.iter().position(|n| n.title == "Trip").unwrap();
        let Kind::Image { src, wiki: true, .. } = &s.notes[i].doc.blocks()[0].kind else { panic!() };
        assert_eq!(s.picture_file("Vault", src, true), Some(pics.join("photo.png")));
        assert_eq!(s.picture_file("", "sun.png", true), Some(dir.join("Elsewhere").join("sun.png")));
        assert_eq!(s.gather_pictures(), Gathered::default());

        // Moved to another folder, an embed found beside the note, that
        // would find another picture from there, is named by its path from
        // the notes folder; the others are found as before.
        fs::write(vault.join("here.png"), "here").unwrap();
        fs::create_dir_all(dir.join("Deep")).unwrap();
        fs::write(dir.join("Deep").join("here.png"), "another").unwrap();
        fs::write(vault.join("Beside.md"), "# Beside\n\n![[here.png]]\n\n![[photo.png]]\n").unwrap();
        s.rescan();
        let i = s.notes.iter().position(|n| n.title == "Beside").unwrap();
        s.move_note(i, "").unwrap();
        assert_eq!(fs::read_to_string(dir.join("Beside.md")).unwrap(), "# Beside\n\n![[Vault/here.png]]\n\n![[photo.png]]\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn gathering_waits_for_a_notes_edits_and_takes_an_emptied_folder_away() {
        let dir = temp_dir("gather-wait");
        let mut s = Store::open(dir.clone()).unwrap();
        let at = dir.join("Lisbon").join("attachments");
        fs::create_dir_all(&at).unwrap();
        fs::write(at.join("a b.png"), "pic").unwrap();
        fs::write(at.join(".DS_Store"), "finder").unwrap();
        fs::write(dir.join("Lisbon").join("Day.md"), "# Day\n\n![|left|300](attachments/a%20b.png)\n").unwrap();
        s.rescan();
        let i = s.notes.iter().position(|n| n.title == "Day").unwrap();
        s.notes[i].doc.push(Block::text("typing"));
        s.notes[i].touch();
        assert_eq!(s.gather_pictures().files, 0, "edits not saved: it waits");
        assert!(at.join("a b.png").exists());
        s.save(i, false).unwrap();
        assert_eq!(s.gather_pictures().files, 1);
        assert!(!at.exists(), "emptied, gone");
        assert!(dir.join(ATTACHMENTS).join("a-b.png").exists());
        let md = fs::read_to_string(dir.join("Lisbon").join("Day.md")).unwrap();
        assert!(md.contains("![|left|300](../attachments/a-b.png)") && md.contains("typing"), "{md}");
        assert_eq!(s.folders, ["Lisbon"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_of_files_that_arent_notes_is_not_listed_and_goes_to_the_trash_whole() {
        let dir = temp_dir("asset-folders");
        let mut s = Store::open(dir.clone()).unwrap();
        fs::create_dir_all(dir.join("Scans")).unwrap();
        fs::write(dir.join("Scans").join("receipt.pdf"), "pdf").unwrap();
        fs::create_dir_all(dir.join("Work").join("Attachments")).unwrap();
        fs::write(dir.join("Work").join("Plan.md"), "# Plan\n").unwrap();
        fs::write(dir.join("Work").join("budget.xlsx"), "x").unwrap();
        s.rescan();
        s.create_folder("", "Empty").unwrap();
        assert_eq!(s.folders, ["Empty", "Work"], "a folder of files only, and an empty Attachments, aren't folders of notes");
        // Made here, then a file put in it from the Finder: it can't just
        // be removed, and isn't half-removed trying.
        let f = s.create_folder("", "Trip").unwrap();
        fs::create_dir_all(dir.join("Trip").join("Attachments")).unwrap();
        fs::write(dir.join("Trip").join("Attachments").join("x.png"), "pic").unwrap();
        fs::write(dir.join("Trip").join(".DS_Store"), "finder").unwrap();
        assert!(s.folder_is_empty(&f) && s.folder_has_files(&f));
        let e = s.delete_folder(&f).unwrap_err();
        assert_eq!(plain_error(&e), "the folder has files in it that aren't notes");
        assert!(dir.join("Trip").join("Attachments").join("x.png").exists());
        let gone = s.trash_folder(&f).unwrap();
        assert!(gone.join("Attachments").join("x.png").exists(), "in the trash, whole");
        assert!(!s.folders.contains(&f));
        // Nothing in it but an empty folder of pictures: removed.
        let g = s.create_folder("", "Bare").unwrap();
        fs::create_dir_all(dir.join("Bare").join("Attachments")).unwrap();
        assert!(!s.folder_has_files(&g));
        s.delete_folder(&g).unwrap();
        assert!(!dir.join("Bare").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn link_targets_are_found_as_written() {
        let md = "a ![x](p q.png) [t](<a b.pdf>) [u](x.md \"title\") ![](f(1).png) <img src=\"s.png\"> [](<>)";
        let found: Vec<&str> = link_targets(md).into_iter().map(|r| &md[r]).collect();
        assert_eq!(found, ["p q.png", "a b.pdf", "x.md", "f(1).png", "s.png"]);
        assert_eq!(encode_src("../attachments/a b(1).png"), "../attachments/a%20b%281%29.png");
        assert_eq!(key_under(Path::new("/n"), Path::new("/n/A/../B/./C.png")).as_deref(), Some("b/c.png"));
        assert_eq!(key_under(Path::new("/n"), Path::new("/m/x.png")), None);
    }

    #[test]
    fn a_link_to_a_file_beside_the_notes_opens_it_and_a_program_is_only_shown() {
        let dir = temp_dir("file-links");
        let s = Store::open(dir.clone()).unwrap();
        fs::create_dir_all(dir.join("attachments")).unwrap();
        fs::create_dir_all(dir.join("Archive")).unwrap();
        fs::write(dir.join("attachments").join("Boarding pass.pdf"), "pdf").unwrap();
        fs::write(dir.join("attachments").join("run.command"), "#!/bin/sh").unwrap();
        let pdf = s.linked_file("Archive", "../attachments/Boarding%20pass.pdf").expect("found from a folder");
        assert!(opens_safely(&pdf));
        assert!(!opens_safely(&s.linked_file("", "attachments/run.command").unwrap()), "shown, not run");
        // Nothing that runs, or opens as a page that could: whatever its
        // case, and none without a kind.
        for name in ["x.COMMAND", "x.sh", "x.app", "x.terminal", "x.tool", "x.jar", "x.exe", "x.bat", "x.ps1", "x.html", "x.svg", "x.webloc", "x.fileloc", "x.scpt", "x.pkg", "x.dmg", "x.docm", "x"] {
            assert!(!opens_safely(Path::new(name)), "{name}");
        }
        for name in ["x.PDF", "x.png", "x.m4a", "x.mov", "x.md"] {
            assert!(opens_safely(Path::new(name)), "{name}");
        }
        assert_eq!(s.linked_file("", "example.com"), None, "a web address");
        assert_eq!(s.linked_file("", "attachments/gone.pdf"), None);
        let outside = format!("carnet-outside-{}.pdf", std::process::id());
        fs::write(dir.parent().unwrap().join(&outside), "pdf").unwrap();
        assert_eq!(s.linked_file("", &format!("../{outside}")), None, "not outside the notes folder");
        let _ = fs::remove_file(dir.parent().unwrap().join(&outside));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A picture on another computer, as a note's text may name it (a
    /// share, `file://` to one, a folder mounted from the network when
    /// looked in), resolves to nothing: never looked at, shown as not
    /// found. One on this computer, by its full path, still resolves.
    #[test]
    fn a_picture_on_another_computer_is_never_looked_at() {
        let dir = temp_dir("network");
        let s = Store::open(dir.clone()).unwrap();
        for src in [
            r"\\host\share\x.png",
            "//host/share/x.png",
            r"\\?\UNC\host\share\x.png",
            r"\\?\C:\x.png",
            r"\\.\pipe\x",
            r"file://\\host\share\x.png",
            "file:////host/share/x.png",
            "file://///host/share/x.png",
            "%5C%5Chost%5Cshare%5Cx.png",
            "/net/host/share/x.png",
            "/Network/Servers/host/x.png",
            "file:///net/host/x.png",
            // Climbed to from the notes folder: `/net` on a Mac or Linux,
            // and on Windows `C:\net`, a folder like any other.
            #[cfg(unix)]
            "../../../../../../../../../../net/host/x.png",
        ] {
            assert_eq!(s.resolve("", src), None, "{src}");
            assert_eq!(s.picture_file("", src, false), None, "{src}");
        }
        assert!(is_web("https://e.com/x.png") && !is_web(r"\\host\x.png"));
        #[cfg(unix)]
        assert_eq!(s.resolve("", "/Users/me/x.png"), Some(PathBuf::from("/Users/me/x.png")));
        assert_eq!(s.resolve("Home", "x.png"), Some(dir.join("Home").join("x.png")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn errors_in_plain_words() {
        let e = io::Error::from_raw_os_error(if cfg!(windows) { 145 } else if cfg!(target_os = "macos") { 66 } else { 39 });
        assert_eq!(plain_error(&e), "there are still files in it");
        assert_eq!(plain_error(&io::Error::other("the folder still has notes in it")), "the folder still has notes in it");
    }
}