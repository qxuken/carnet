//! Whose a picture is (docs/SYNC.md §4).
//!
//! Carnet's are the ones in the notes folder's attachments folder: where a
//! paste, a drop, an import or an export's gathering puts them. They sync,
//! and Carnet may tidy them away. Any other picture a note shows (beside
//! its note, elsewhere in the folder, or outside it) is the user's: shown,
//! never synced, never moved or removed but when the user asks for it
//! (Move into Carnet, Copy into Carnet).
//!
//! The sweep: a picture of Carnet's nothing shows any more (no note, a
//! note in Deleted included; no version of one; no text in the trash) for
//! [`SWEEP_GRACE`] goes to `.trash/attachments/`, where it can be put back
//! by hand, and is dropped in the vault, so other devices drop their
//! copies and the server, after its own 30 days, the blob.

use super::*;

/// How long a picture of Carnet's goes unshown before the sweep takes it:
/// a week, so one taken out by mistake and put back within days, or one a
/// note on another device shows and whose text hasn't come yet, stays.
pub const SWEEP_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Beside `pictures.txt` in the replica's folder: each picture of Carnet's
/// seen unshown, and since when (`path`, tab, milliseconds since 1970).
const UNSHOWN: &str = "unshown.txt";

/// Whether a path from the notes folder (`/` between parts) is in the
/// attachments folder, case aside, and goes nowhere else: each of its
/// parts a plain name ([`is_plain_path`](carnet_core::name::is_plain_path):
/// no `..`, no `\`, no drive).
pub fn is_owned(rel: &str) -> bool {
    carnet_core::name::is_plain_path(rel)
        && rel.split_once('/').is_some_and(|(top, rest)| top.eq_ignore_ascii_case(ATTACHMENTS) && !rest.is_empty())
}

/// Whether two paths name one file, `..` worked out and case aside, as
/// the file systems notes mostly live on see names.
pub fn same_file(a: &Path, b: &Path) -> bool {
    let (x, y) = (path_parts(a), path_parts(b));
    x.len() == y.len() && x.iter().zip(&y).all(|(p, q)| p.eq_ignore_ascii_case(q))
}

/// A path under `dir` as its parts from there, joined with `/`, as they
/// are written. `None` for one outside it.
fn rel_under(dir: &Path, p: &Path) -> Option<String> {
    let (d, q) = (path_parts(dir), path_parts(p));
    if q.len() <= d.len() || !d.iter().zip(&q).all(|(a, b)| a.eq_ignore_ascii_case(b)) {
        return None;
    }
    Some(q[d.len()..].join("/"))
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The pictures in a doc that `edits` names (block id, src) given those
/// `src`s. True if any changed.
pub fn set_srcs(doc: &mut Doc, edits: &[(u64, String)]) -> bool {
    let mut changed = false;
    for (id, new) in edits {
        let Some(i) = doc.index_of(*id) else { continue };
        if matches!(&doc.blocks()[i].kind, Kind::Image { src, .. } if src != new)
            && let Kind::Image { src, .. } = &mut doc.block_mut(i).kind
        {
            *src = new.clone();
            changed = true;
        }
    }
    changed
}

/// The pictures among a doc's table cells' words that `edits` names (the
/// `src` it has, whether an Obsidian embed names it, the `src` it gets)
/// given those `src`s. True if any changed.
pub fn set_cell_srcs(doc: &mut Doc, edits: &[(String, bool, String)]) -> bool {
    if edits.is_empty() {
        return false;
    }
    let mut changed = false;
    for i in 0..doc.len() {
        let Kind::Table(t) = &doc.blocks()[i].kind else { continue };
        let mut cells = t.clone();
        let mut here = false;
        for cell in cells.rows.iter_mut().flat_map(|r| r.cells.iter_mut()) {
            let to = |p: &carnet_core::doc::CellPicture| edits.iter().find(|(src, wiki, _)| *src == p.src && *wiki == p.wiki).map(|(_, _, new)| new.clone());
            if let Some(new) = carnet_core::doc::relink_cell(cell, to) {
                *cell = new;
                here = true;
            }
        }
        if here {
            doc.block_mut(i).kind = Kind::Table(cells);
            changed = true;
        }
    }
    changed
}

/// What Move into Carnet did: enough to undo it.
#[derive(Clone, Debug, PartialEq)]
pub struct Moved {
    /// Where the picture was, and where it is now.
    pub from: PathBuf,
    pub to: PathBuf,
    /// The notes that show it there now, by id, each with the pictures'
    /// `src`s as they were (block id, src).
    pub notes: Vec<(u64, Vec<(u64, String)>)>,
    /// Notes in Deleted that show it there now: their file, and the text
    /// before and after.
    pub trashed: Vec<(PathBuf, String, String)>,
    /// The file couldn't be taken from where it was (a folder Carnet may
    /// not write to): it's there still, and a copy is Carnet's.
    pub left: bool,
}

impl Store {
    /// Whether the file at `path` is Carnet's: in the attachments folder.
    pub fn owns(&self, path: &Path) -> bool {
        key_under(&self.dir, path).is_some_and(|k| is_owned(&k))
    }

    /// The `src` a note in `folder` shows the file at `to` by: its path
    /// from the note, or an Obsidian embed's from the notes folder; one
    /// outside the notes folder by its full path.
    pub fn src_for(&self, folder: &str, to: &Path, wiki: bool) -> String {
        match rel_under(&self.dir, to) {
            Some(rel) if wiki => rel,
            Some(_) => encode_src(&relative(&self.folder_dir(folder), to)),
            None => format!("file://{}", encode_src(&to.to_string_lossy().replace('\\', "/"))),
        }
    }

    /// Note `i`'s pictures that show the file at `file`: each one's block
    /// and its `src`, and whether an Obsidian embed names it.
    fn showing(&self, i: usize, file: &Path) -> Vec<(u64, String, bool)> {
        let n = &self.notes[i];
        n.doc
            .blocks()
            .iter()
            .filter_map(|b| match &b.kind {
                Kind::Image { src, wiki, .. } if self.picture_file(&n.folder, src, *wiki).is_some_and(|p| same_file(&p, file)) => {
                    Some((b.id, src.clone(), *wiki))
                }
                _ => None,
            })
            .collect()
    }

    /// Note `i`'s pictures among its table cells' words that show the file
    /// at `file`: each one's `src`, whether an Obsidian embed names it,
    /// and the `src` that shows `to` instead ([`set_cell_srcs`]).
    pub fn cell_relinks(&self, i: usize, file: &Path, to: &Path) -> Vec<(String, bool, String)> {
        let n = &self.notes[i];
        let mut out: Vec<(String, bool, String)> = Vec::new();
        for b in n.doc.blocks() {
            let Kind::Table(t) = &b.kind else { continue };
            for cell in t.rows.iter().flat_map(|r| &r.cells) {
                for p in carnet_core::inline::cell_pictures(cell) {
                    if self.picture_file(&n.folder, &p.src, p.wiki).is_some_and(|f| same_file(&f, file)) && !out.iter().any(|(s, w, _)| *s == p.src && *w == p.wiki) {
                        let new = self.src_for(&n.folder, to, p.wiki);
                        out.push((p.src, p.wiki, new));
                    }
                }
            }
        }
        out
    }

    /// Note `i`'s pictures that show the file at `file`, each with the
    /// `src` that shows `to` instead.
    pub fn relinks(&self, i: usize, file: &Path, to: &Path) -> Vec<(u64, String)> {
        let folder = &self.notes[i].folder;
        self.showing(i, file).into_iter().map(|(id, _, wiki)| (id, self.src_for(folder, to, wiki))).collect()
    }

    /// A copy of the file at `from` in the attachments folder, under its
    /// own name made plain, or the first such name free there. With
    /// `link`, the same file under a second name where the system allows
    /// (what a move does before the old name goes).
    pub fn copy_in(&self, from: &Path, link: bool) -> io::Result<PathBuf> {
        let dir = self.dir.join(ATTACHMENTS);
        fs::create_dir_all(&dir)?;
        let stem = slug(&stem_of(from)).replace(' ', "-");
        // A picture's or a document's own kind, or none: never a name
        // that would open as something else, or run.
        let ext = from
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .filter(|e| doc::is_picture_name(&format!("x.{e}")) || doc::is_document_name(&format!("x.{e}")));
        let to = free_file(&dir, &stem, ext.as_deref().unwrap_or("png"));
        if !(link && fs::hard_link(from, &to).is_ok()) {
            fs::copy(from, &to)?;
        }
        Ok(to)
    }

    /// Move into Carnet: the user's picture at `from` into the attachments
    /// folder, under a free name, and every note that shows it (one in
    /// Deleted too) shows it there, saved as any edit is, so its history
    /// and sync see it. Then it's Carnet's, and syncs. The notes are
    /// written before the old file goes, so none ever shows nothing; if
    /// one can't be, the old file stays where it was.
    pub fn move_picture(&mut self, from: &Path) -> io::Result<Moved> {
        if !from.is_file() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "the picture isn't there any more"));
        }
        if self.owns(from) {
            return Err(io::Error::other("it's in Carnet already"));
        }
        let to = self.copy_in(from, true)?;
        let mut moved = Moved { from: from.to_path_buf(), to: to.clone(), notes: Vec::new(), trashed: Vec::new(), left: false };
        if let Err(e) = self.show_elsewhere(from, &to, &mut moved) {
            self.index_files();
            return Err(e);
        }
        moved.left = fs::remove_file(from).is_err();
        if !moved.left {
            self.unname_picture(from);
        }
        self.index_files();
        Ok(moved)
    }

    /// Move into Carnet taken back: the picture where it was, and the
    /// notes showing it as they did, where nothing changed them since.
    pub fn unmove_picture(&mut self, m: &Moved) -> io::Result<()> {
        if !m.to.is_file() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "it isn't in the attachments folder any more"));
        }
        if !m.left {
            if m.from.exists() {
                return Err(io::Error::other("another file is where it was"));
            }
            if let Some(dir) = m.from.parent() {
                fs::create_dir_all(dir)?;
            }
            if fs::hard_link(&m.to, &m.from).is_err() {
                fs::copy(&m.to, &m.from)?;
            }
        }
        for (id, srcs) in &m.notes {
            let Some(i) = self.notes.iter().position(|n| n.id == *id) else { continue };
            let still: Vec<u64> = self.showing(i, &m.to).into_iter().map(|(b, ..)| b).collect();
            let back: Vec<(u64, String)> = srcs.iter().filter(|(b, _)| still.contains(b)).cloned().collect();
            // A cell's: shown where it was again, by its path from there.
            let cells = self.cell_relinks(i, &m.to, &m.from);
            if set_srcs(&mut self.notes[i].doc, &back) | set_cell_srcs(&mut self.notes[i].doc, &cells) {
                self.notes[i].touch();
                self.save(i, false)?;
            }
        }
        for (path, before, after) in &m.trashed {
            if fs::read_to_string(path).is_ok_and(|t| t == *after) {
                write_atomic(path, before)?;
                self.record_trashed(path, before);
            }
        }
        fs::remove_file(&m.to)?;
        self.unname_picture(&m.to);
        self.index_files();
        Ok(())
    }

    /// Every note that shows the file at `from` (in Deleted too) shows `to`
    /// instead, saved; what was rewritten kept in `moved`.
    fn show_elsewhere(&mut self, from: &Path, to: &Path, moved: &mut Moved) -> io::Result<()> {
        for i in 0..self.notes.len() {
            let before = self.showing(i, from);
            let cells = self.cell_relinks(i, from, to);
            if before.is_empty() && cells.is_empty() {
                continue;
            }
            let edits = self.relinks(i, from, to);
            set_srcs(&mut self.notes[i].doc, &edits);
            set_cell_srcs(&mut self.notes[i].doc, &cells);
            self.notes[i].touch();
            self.save(i, false)?;
            moved.notes.push((self.notes[i].id, before.into_iter().map(|(b, src, _)| (b, src)).collect()));
        }
        // In Deleted: their files, as written, by where they came from.
        for (_, rel, folder) in self.in_trash() {
            let path = self.dir.join(&rel);
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Some(new) = self.rewrite_links(&folder, &text, from, to) else { continue };
            write_atomic(&path, &new)?;
            self.record_trashed(&path, &new);
            moved.trashed.push((path, text, new));
        }
        Ok(())
    }

    /// The notes in Deleted this device's history knows: each one's key,
    /// its path from the notes folder, and the folder it came from (what
    /// its pictures' paths are from): as the vault says, else as the
    /// trash does.
    pub(super) fn in_trash(&self) -> Vec<(Key, String, String)> {
        let Some(r) = &self.replica else { return Vec::new() };
        let origins = self.origins();
        let mut out = Vec::new();
        for key in r.keys() {
            let Some(rel) = r.entry(key).map(|e| e.path.clone()) else { continue };
            let Some(inner) = rel.strip_prefix(&format!("{TRASH}/")) else { continue };
            let folder = r.vault().place(key).map(|p| p.folder).unwrap_or_else(|| {
                let parts: Vec<&str> = inner.split('/').collect();
                match (origins.get(parts[0]), parts.len()) {
                    (Some((folder, ..)), 1) => folder.clone(),
                    (Some((folder, name, ..)), n) => {
                        let mut all: Vec<&str> = folder.split('/').filter(|p| !p.is_empty()).collect();
                        all.push(name);
                        all.extend(&parts[1..n - 1]);
                        all.join("/")
                    }
                    (None, _) => String::new(),
                }
            });
            out.push((key, rel, folder));
        }
        out
    }

    /// `text`, a note in `folder`, with every link and picture that names
    /// the file at `from` naming `to`; `None` if none does.
    fn rewrite_links(&self, folder: &str, text: &str, from: &Path, to: &Path) -> Option<String> {
        let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        for r in link_targets(text) {
            if self.resolve(folder, &text[r.clone()]).is_some_and(|p| same_file(&p, from)) {
                edits.push((r, self.src_for(folder, to, false)));
            }
        }
        for r in wiki_targets(text) {
            if self.picture_file(folder, &text[r.clone()], true).is_some_and(|p| same_file(&p, from)) {
                edits.push((r, self.src_for(folder, to, true)));
            }
        }
        if edits.is_empty() {
            return None;
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
        Some(out)
    }

    /// A note in Deleted written anew: its history learns it, and sync
    /// sends it.
    fn record_trashed(&mut self, path: &Path, text: &str) {
        let Some(rel) = rel_under(&self.dir, path) else { return };
        let Some(key) = self.key_at(&rel) else { return };
        if let Some(r) = self.replica.as_mut() {
            let _ = r.record(key, &rel, text, Origin::Save);
        }
        if !self.changed.contains(&key) {
            self.changed.push(key);
        }
    }

    /// The vault no longer names the picture at `path` (from the notes
    /// folder): dropped, so other devices let go of their copies and the
    /// server may drop it. True if the vault changed.
    pub fn unname_picture(&mut self, path: &Path) -> bool {
        let Some(rel) = rel_under(&self.dir, path) else { return false };
        self.unname(&rel)
    }

    /// [`unname_picture`](Self::unname_picture), by its path from the
    /// notes folder as the vault has it.
    pub fn unname(&mut self, rel: &str) -> bool {
        let Some(r) = &self.replica else { return false };
        let (changed, unused) = r.vault().drop_picture(rel);
        if !changed {
            return false;
        }
        let _ = r.persist_vault();
        self.vault_changed = true;
        self.forgotten.pictures.extend(unused);
        true
    }

    /// The pictures in the attachments folder, by their paths from the
    /// notes folder (as written on disk), and where they are. A few
    /// folders deep; hidden files and ones being written left out.
    pub fn attachments(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        let mut stack = vec![(self.dir.join(ATTACHMENTS), ATTACHMENTS.to_string(), 0)];
        while let Some((dir, rel, depth)) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                // Nor one another system would read as more than a name
                // (`a:b.png`, a Mac allows it): never synced, nor swept.
                if name.starts_with('.') || name.ends_with(".tmp-carnet") || !carnet_core::name::is_plain_name(&name) {
                    continue;
                }
                match e.file_type() {
                    Ok(t) if t.is_dir() => {
                        if depth < 4 {
                            stack.push((e.path(), format!("{rel}/{name}"), depth + 1));
                        }
                    }
                    Ok(t) if t.is_file() => out.push((format!("{rel}/{name}"), e.path())),
                    _ => {}
                }
            }
        }
        out
    }

    /// The pictures of Carnet's `text` (a note in `folder`) shows that the
    /// sweep took to the trash: put back where they were.
    pub(super) fn bring_back(&self, folder: &str, text: &str) {
        for rel in doc::picture_paths(folder, text, &self.files) {
            if !is_owned(&rel) {
                continue;
            }
            let (Ok(here), Ok(swept)) = (self.inside(self.dir.join(&rel)), self.inside(self.dir.join(TRASH).join(&rel))) else { continue };
            if here.exists() || !swept.is_file() {
                continue;
            }
            if let Some(dir) = here.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::rename(&swept, &here);
        }
    }

    /// The sweep: the pictures in the attachments folder that nothing
    /// shows (no note, its unsaved edits and the notes in Deleted
    /// included; no version of a note this device keeps; no text in the
    /// trash) and that aren't `busy` (being sent, or wanted back: paths
    /// from the notes folder), seen so for [`SWEEP_GRACE`] since first
    /// seen, go to `.trash/` under the same path, and are dropped in the
    /// vault. Returns their paths. Without a replica nothing goes: there
    /// is no history to ask.
    pub fn sweep_pictures(&mut self, busy: &BTreeSet<String>, now: SystemTime) -> io::Result<Vec<String>> {
        let Some(record) = self.replica_dir().map(|d| d.join(UNSHOWN)) else { return Ok(Vec::new()) };
        let seen: HashMap<String, u64> = fs::read_to_string(&record)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (p, t) = l.split_once('\t')?;
                Some((p.to_string(), t.parse().ok()?))
            })
            .collect();
        let low = |p: &str| p.to_lowercase();
        let mut used: HashSet<String> = self.pictures_in_use().iter().chain(busy).map(|p| low(p)).collect();
        // The swept ones keep their place in the trash, and aren't here.
        // Only what syncs (pictures, and documents, which `used` has when
        // a note links to them): a file of another kind is no one's to
        // judge, and stays.
        let mut unshown: Vec<(String, PathBuf)> = self
            .attachments()
            .into_iter()
            .filter(|(rel, _)| crate::pictures::syncs(rel) && !used.contains(&low(rel)))
            .collect();
        if !unshown.is_empty() {
            // A note in the trash this device's history doesn't know (put
            // there by hand, or before): any text there that names it.
            let mut texts = String::new();
            for f in files_under(&self.dir.join(TRASH), 0) {
                if f.extension().is_some_and(|x| x.eq_ignore_ascii_case("md"))
                    && let Ok(t) = fs::read_to_string(&f)
                {
                    texts.push_str(&decode_src(&t).to_lowercase());
                }
            }
            unshown.retain(|(rel, _)| !texts.contains(&low(rel.rsplit('/').next().unwrap_or(rel))));
        }
        if !unshown.is_empty() {
            let paths: BTreeSet<String> = unshown.iter().map(|(p, _)| p.clone()).collect();
            used.extend(self.shown_in_history(&paths).iter().map(|p| low(p)));
            unshown.retain(|(rel, _)| !used.contains(&low(rel)));
        }
        let now = millis(now);
        let grace = SWEEP_GRACE.as_millis() as u64;
        let mut next: Vec<(String, u64)> = Vec::new();
        let mut swept = Vec::new();
        for (rel, path) in unshown {
            let first = seen.get(&rel).copied().unwrap_or(now);
            if now.saturating_sub(first) >= grace && self.sweep_one(&rel, &path).is_ok() {
                swept.push(rel);
            } else {
                next.push((rel, first));
            }
        }
        if next.is_empty() {
            let _ = fs::remove_file(&record);
        } else {
            let text: String = next.iter().filter(|(p, _)| !p.contains(['\t', '\n'])).map(|(p, t)| format!("{p}\t{t}\n")).collect();
            let tmp = record.with_extension("tmp");
            fs::write(&tmp, text)?;
            fs::rename(&tmp, &record)?;
        }
        if !swept.is_empty() {
            self.index_files();
        }
        Ok(swept)
    }

    /// Copy into Carnet taken back: the copy it made (`copy`, in the
    /// attachments folder) goes where the sweep puts what nothing shows —
    /// `.trash/`, from where a version restored that shows it brings it
    /// back — and is dropped in the vault. Not while a note shows it (one
    /// in Deleted too): then it stays, for the sweep to judge in time. True
    /// if it went.
    pub fn take_back_copy(&mut self, copy: &Path) -> io::Result<bool> {
        let Some(rel) = rel_under(&self.dir, copy) else { return Ok(false) };
        if !is_owned(&rel) || !copy.is_file() {
            return Ok(false);
        }
        if self.pictures_in_use().iter().any(|p| p.eq_ignore_ascii_case(&rel)) {
            return Ok(false);
        }
        self.sweep_one(&rel, copy)?;
        self.index_files();
        Ok(true)
    }

    /// One picture swept: to `.trash/` under its path from the notes
    /// folder (or the first name free there), and dropped in the vault.
    fn sweep_one(&mut self, rel: &str, path: &Path) -> io::Result<()> {
        let mut to = self.dir.join(TRASH).join(rel);
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir)?;
            if to.exists() {
                let ext = to.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
                to = free_file(dir, &stem_of(&to), &ext);
            }
        }
        fs::rename(path, &to)?;
        self.unname(rel);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("carnet-owned-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(dir.with_extension("replica"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn note(s: &mut Store, folder: &str, title: &str, body: &str) -> usize {
        fs::create_dir_all(s.folder_dir(folder)).unwrap();
        let path = s.folder_dir(folder).join(format!("{title}.md"));
        fs::write(&path, format!("# {title}\n\n{body}\n")).unwrap();
        s.rescan();
        s.notes.iter().position(|n| n.title == title).unwrap()
    }

    fn edit(s: &mut Store, i: usize, body: &str) {
        let title = s.notes[i].title.clone();
        let (_, d) = doc::from_markdown(&format!("# {title}\n\n{body}\n"));
        s.notes[i].doc = d;
        s.notes[i].touch();
        s.save(i, false).unwrap();
    }

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    #[test]
    fn whose_a_picture_is() {
        assert!(is_owned("attachments/a.png") && is_owned("Attachments/2026/a.png"));
        assert!(!is_owned("attachments") && !is_owned("Home/a.png") && !is_owned("Home/attachments/a.png"));
        let dir = temp_dir("whose");
        let s = Store::open(dir.clone()).unwrap();
        assert!(s.owns(&dir.join("Home/../attachments/a.png")));
        assert!(!s.owns(&dir.join("Home/a.png")) && !s.owns(Path::new("/tmp/a.png")));
        assert_eq!(s.src_for("Home", &dir.join("attachments/a b.png"), false), "../attachments/a%20b.png");
        assert_eq!(s.src_for("Home", &dir.join("attachments/a b.png"), true), "attachments/a b.png");
    }

    /// Move into Carnet: the file into the attachments folder, every note
    /// showing it (one in Deleted too) saved showing it there; undone, all
    /// as it was, and the vault doesn't name it.
    #[test]
    fn a_picture_moved_into_carnet_takes_every_note_with_it_and_comes_back() {
        let dir = temp_dir("move");
        let mut s = Store::open(dir.clone()).unwrap();
        fs::create_dir_all(dir.join("Home")).unwrap();
        fs::write(dir.join("Home/hill view.jpg"), b"hill").unwrap();
        let a = note(&mut s, "Home", "Walk", "![The hill](hill%20view.jpg)");
        let b = note(&mut s, "", "Index", "Words.\n\n![](Home/hill%20view.jpg)");
        let c = note(&mut s, "Home", "Old", "![](hill%20view.jpg)");
        s.trash(c).unwrap();
        let (a_id, b_id) = (s.notes[a].id, s.notes[b].id);
        let from = dir.join("Home/hill view.jpg");
        let m = s.move_picture(&from).unwrap();
        assert_eq!(m.to, dir.join("attachments/hill-view.jpg"));
        assert!(!from.exists() && m.to.exists() && !m.left);
        let file = |s: &Store, id: u64| fs::read_to_string(s.notes.iter().find(|n| n.id == id).unwrap().path.as_ref().unwrap()).unwrap();
        assert!(file(&s, a_id).contains("![The hill](../attachments/hill-view.jpg)"), "{}", file(&s, a_id));
        assert!(file(&s, b_id).contains("![](attachments/hill-view.jpg)"));
        let trashed = fs::read_to_string(&m.trashed[0].0).unwrap();
        assert!(trashed.contains("](../attachments/hill-view.jpg)"), "{trashed}");
        assert!(s.pictures_in_use().contains("attachments/hill-view.jpg"));
        // Its history has it, as a save.
        let i = s.notes.iter().position(|n| n.id == a_id).unwrap();
        assert!(s.history(i).len() >= 2);

        s.unmove_picture(&m).unwrap();
        assert!(from.exists() && !m.to.exists());
        assert!(file(&s, a_id).contains("![The hill](hill%20view.jpg)"));
        assert!(file(&s, b_id).contains("![](Home/hill%20view.jpg)"));
        assert!(fs::read_to_string(&m.trashed[0].0).unwrap().contains("](hill%20view.jpg)"));
    }

    /// Delete forever takes Carnet's pictures the note showed, never the
    /// user's.
    #[test]
    fn deleting_forever_takes_only_carnets_pictures() {
        let dir = temp_dir("forever");
        let mut s = Store::open(dir.clone()).unwrap();
        fs::create_dir_all(dir.join(ATTACHMENTS)).unwrap();
        fs::create_dir_all(dir.join("Home")).unwrap();
        fs::write(dir.join("attachments/pasted.png"), b"p").unwrap();
        fs::write(dir.join("Home/hill.jpg"), b"h").unwrap();
        let i = note(&mut s, "Home", "Walk", "![](hill.jpg)\n\n![](../attachments/pasted.png)");
        s.trash(i).unwrap();
        let t = s.trashed().into_iter().next().unwrap();
        s.delete_forever(&t).unwrap();
        assert!(!dir.join("attachments/pasted.png").exists(), "Carnet's goes");
        assert!(dir.join("Home/hill.jpg").exists(), "the user's stays");
    }

    /// A note's picture whose path climbs out with Windows' `\` (Windows
    /// reads `attachments\..\..\x` as `x`, beside the notes folder), or
    /// names a drive or another folder outright, is never Carnet's: Delete
    /// forever leaves it, wherever it is. And no removal or rename asked
    /// for a path outside the notes folder is done.
    #[test]
    fn deleting_forever_never_reaches_outside_the_notes_folder() {
        let base = temp_dir("forever-outside");
        let dir = base.join("notes");
        let mut s = Store::open(dir.clone()).unwrap();
        fs::create_dir_all(dir.join(ATTACHMENTS)).unwrap();
        fs::write(base.join("secret.docx"), b"s").unwrap();
        fs::write(dir.join("attachments/pasted.png"), b"p").unwrap();
        let outside = base.join("secret.docx").to_string_lossy().replace('\\', "/");
        let body = format!(
            "![](attachments/..\\..\\secret.docx)\n\n![](attachments\\..\\..\\secret.docx)\n\n![](attachments/../../secret.docx)\n\n![]({outside})\n\n![](file://{outside})\n\n![](attachments/pasted.png)"
        );
        let i = note(&mut s, "", "Walk", &body);
        assert_eq!(doc::picture_paths("", &s.notes[i].markdown(), &s.files).into_iter().collect::<Vec<_>>(), ["attachments/pasted.png"]);
        s.trash(i).unwrap();
        let t = s.trashed().into_iter().next().unwrap();
        s.delete_forever(&t).unwrap();
        assert!(base.join("secret.docx").exists(), "outside: never touched");
        assert!(!dir.join("attachments/pasted.png").exists(), "Carnet's own goes");
        // Asked outright, past every check before it: refused.
        for p in ["attachments/../../secret.docx", "../secret.docx"] {
            assert!(s.inside(dir.join(p)).is_err(), "{p}");
        }
        assert!(s.inside(base.join("secret.docx")).is_err() && s.inside(dir.join("attachments/a.png")).is_ok() && s.inside(dir.clone()).is_ok());
        s.drop_pictures(&["attachments/../../secret.docx".to_string()].into());
        s.bring_back("", "![](attachments/../../secret.docx)");
        assert!(base.join("secret.docx").exists());
        let _ = fs::remove_dir_all(dir.with_extension("replica"));
        let _ = fs::remove_dir_all(&base);
    }

    /// Only a picture is taken in: whatever else a note names keeps its
    /// own kind out of the attachments folder.
    #[test]
    fn a_copy_in_is_named_as_a_picture() {
        let dir = temp_dir("copy-in-kind");
        let s = Store::open(dir.clone()).unwrap();
        // A document keeps its kind too; a page or a macro's sheet doesn't.
        for (name, kind) in [
            ("photo.JPG", "jpg"),
            ("id_rsa", "png"),
            ("run.command", "png"),
            ("x.webp", "webp"),
            ("Report.PDF", "pdf"),
            ("page.html", "png"),
            ("sums.xlsm", "png"),
        ] {
            fs::write(dir.join(name), b"x").unwrap();
            let to = s.copy_in(&dir.join(name), false).unwrap();
            assert_eq!(to.extension().and_then(|e| e.to_str()), Some(kind), "{name}");
        }
        let _ = fs::remove_dir_all(dir.with_extension("replica"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A picture pasted and taken out before the note was saved: no
    /// version shows it, so it's swept once it has gone unshown for the
    /// grace period, to the trash, and a save showing it again brings it
    /// back. The user's own pictures are never looked at.
    #[test]
    fn an_unshown_picture_is_swept_after_the_grace_period_and_comes_back_when_shown() {
        let dir = temp_dir("sweep");
        let mut s = Store::open(dir.clone()).unwrap();
        fs::create_dir_all(dir.join(ATTACHMENTS)).unwrap();
        fs::create_dir_all(dir.join("Home")).unwrap();
        fs::write(dir.join("attachments/pasted.png"), b"p").unwrap();
        fs::write(dir.join("Home/mine.jpg"), b"m").unwrap();
        fs::write(dir.join("loose.png"), b"l").unwrap();
        let i = note(&mut s, "Home", "Walk", "Words.");
        let t0 = SystemTime::now();
        let none = BTreeSet::new();
        assert!(s.sweep_pictures(&none, t0).unwrap().is_empty(), "seen unshown, kept for now");
        assert!(s.sweep_pictures(&none, t0 + 6 * DAY).unwrap().is_empty());
        // Busy (being sent): kept, and its week starts again.
        let busy: BTreeSet<String> = ["attachments/pasted.png".to_string()].into();
        assert!(s.sweep_pictures(&busy, t0 + 8 * DAY).unwrap().is_empty());
        assert!(s.sweep_pictures(&none, t0 + 8 * DAY).unwrap().is_empty());
        assert_eq!(s.sweep_pictures(&none, t0 + 15 * DAY).unwrap(), vec!["attachments/pasted.png".to_string()]);
        assert!(!dir.join("attachments/pasted.png").exists());
        assert!(dir.join(".trash/attachments/pasted.png").exists(), "in the trash, to be put back by hand");
        assert!(dir.join("Home/mine.jpg").exists() && dir.join("loose.png").exists(), "the user's are never swept");
        assert!(s.trashed().is_empty(), "not listed in Deleted");
        // Shown again (a paste's undo undone, a version restored): back.
        edit(&mut s, i, "![](../attachments/pasted.png)");
        assert!(dir.join("attachments/pasted.png").exists());
    }

    /// A document in the attachments folder (one the paperclip put there,
    /// or an export gathered) a note links to is in use, as a picture it
    /// shows is, and stays; one no note links to is swept as an unshown
    /// picture is. A file of another kind is never swept. (A linked one
    /// used to go to the trash a week after it came.)
    #[test]
    fn a_document_is_swept_only_when_no_note_links_to_it() {
        let dir = temp_dir("sweep-docs");
        let mut s = Store::open(dir.clone()).unwrap();
        fs::create_dir_all(dir.join(ATTACHMENTS)).unwrap();
        for f in ["report.pdf", "old.pdf", "pasted.png", "tool.sh"] {
            fs::write(dir.join(ATTACHMENTS).join(f), b"x").unwrap();
        }
        note(&mut s, "", "Trip", "See [report.pdf](attachments/report.pdf).");
        let t0 = SystemTime::now();
        let none = BTreeSet::new();
        assert!(s.sweep_pictures(&none, t0).unwrap().is_empty());
        let swept = s.sweep_pictures(&none, t0 + 15 * DAY).unwrap();
        assert_eq!(swept, ["attachments/old.pdf", "attachments/pasted.png"], "the unlinked document and the unshown picture");
        assert!(dir.join("attachments/report.pdf").is_file(), "the linked document stays");
        assert!(dir.join("attachments/tool.sh").is_file(), "no kind of Carnet's: never touched");
        let _ = fs::remove_dir_all(dir.with_extension("replica"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Pictures among a table's cells' words are pictures as any other:
    /// shown, so in use and never swept (nor dropped on another device's
    /// word); a note moved to another folder has their paths rewritten
    /// from there, and the user's own one moved into Carnet is shown from
    /// the attachments folder, and from where it was again when that is
    /// undone.
    #[test]
    fn pictures_in_a_tables_cells_are_pictures_too() {
        let dir = temp_dir("cells");
        let mut s = Store::open(dir.clone()).unwrap();
        fs::create_dir_all(dir.join(ATTACHMENTS)).unwrap();
        fs::create_dir_all(dir.join("Home")).unwrap();
        fs::write(dir.join("attachments/cell.png"), b"c").unwrap();
        fs::write(dir.join("Home/mine.png"), b"m").unwrap();
        let i = note(&mut s, "Home", "Walk", "| a | b |\n| --- | --- |\n| ![|left|260](../attachments/cell.png) and words | ![](mine.png) |");
        assert!(s.pictures_in_use().contains("attachments/cell.png"));
        let key = s.notes[i].key.unwrap();
        assert!(s.shown_by(&[key]).contains("attachments/cell.png"), "what sync asks back");
        let t0 = SystemTime::now();
        let none = BTreeSet::new();
        for d in [0, 8, 30] {
            assert!(s.sweep_pictures(&none, t0 + d * DAY).unwrap().is_empty(), "day {d}");
        }
        assert!(dir.join("attachments/cell.png").exists());
        // Moved to the top: each path from there.
        s.move_note(i, "").unwrap();
        let read = || fs::read_to_string(dir.join("Walk.md")).unwrap();
        assert!(read().contains("| ![|left|260](attachments/cell.png) and words | ![](Home/mine.png) |"), "{}", read());
        // The user's own, moved into Carnet: shown from there; back.
        let m = s.move_picture(&dir.join("Home/mine.png")).unwrap();
        assert!(read().contains("| ![](attachments/mine.png) |"), "{}", read());
        s.unmove_picture(&m).unwrap();
        assert!(read().contains("| ![](Home/mine.png) |"), "{}", read());
        let _ = fs::remove_dir_all(dir.with_extension("replica"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A picture taken out of its note by an edit stays while the note's
    /// history shows it, which is for as long as the note is kept; one only
    /// a note in Deleted shows stays too.
    #[test]
    fn a_picture_any_version_or_a_deleted_note_shows_is_kept() {
        let dir = temp_dir("kept");
        let mut s = Store::open(dir.clone()).unwrap();
        fs::create_dir_all(dir.join(ATTACHMENTS)).unwrap();
        fs::write(dir.join("attachments/old.png"), b"o").unwrap();
        fs::write(dir.join("attachments/binned.png"), b"b").unwrap();
        let i = note(&mut s, "", "Walk", "![](attachments/old.png)");
        edit(&mut s, i, "No picture now.");
        let j = note(&mut s, "", "Bin", "![](attachments/binned.png)");
        s.trash(j).unwrap();
        let t0 = SystemTime::now();
        let none = BTreeSet::new();
        for d in [0, 8, 30] {
            assert!(s.sweep_pictures(&none, t0 + d * DAY).unwrap().is_empty(), "day {d}");
        }
        assert!(dir.join("attachments/old.png").exists() && dir.join("attachments/binned.png").exists());
        // The note in Deleted, deleted forever: its picture goes with it
        // (no other note, nor version, shows it).
        let t = s.trashed().into_iter().next().unwrap();
        s.delete_forever(&t).unwrap();
        assert!(!dir.join("attachments/binned.png").exists());
        assert!(dir.join("attachments/old.png").exists(), "its note's history still shows it");
    }
}
