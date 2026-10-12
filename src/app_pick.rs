//! Several notes and folders chosen at once in the library, as Finder
//! chooses them: ⇧-click takes everything from the last one clicked to
//! the one clicked now, ⌘-click (Ctrl elsewhere) adds or takes one; ⇧
//! with ↑ or ↓ grows or shrinks the range from the keyboard. What is
//! chosen goes to the trash together, or to a folder: dragged there, or,
//! notes only, by the menu.

use kui_native::KeyMods;

use super::*;

/// A row of the library that can be chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// A note, by id.
    Note(u64),
    /// A folder, by its path.
    Folder(String),
}

/// Whether a click with these keys held adds to a choice: ⌘ on a Mac,
/// Ctrl elsewhere.
fn adds(m: KeyMods) -> bool {
    if cfg!(target_os = "macos") { m.super_key } else { m.ctrl }
}

impl Carnet {
    /// The library's rows as they read, top to bottom: the folders, then
    /// the notes shown. What a ⇧-click's range runs over.
    fn pick_rows(&self) -> Vec<Pick> {
        let folders = self.store.folders.iter().cloned().map(Pick::Folder);
        let notes = self.order.iter().map(|&i| Pick::Note(self.store.notes[i].id));
        folders.chain(notes).collect()
    }

    /// The row a range starts from: the last one clicked, else the folder
    /// shown, else the open note.
    fn pick_from(&self) -> Pick {
        self.pick_anchor.clone().unwrap_or_else(|| match &self.folder {
            Some(f) if !self.deleted => Pick::Folder(f.clone()),
            _ => Pick::Note(self.store.notes[self.current].id),
        })
    }

    /// A click on a library row with `m` held: true when it chose (⇧ or ⌘
    /// held), false for a plain click, which lets go of any choice and is
    /// the row's own.
    pub(super) fn pick_click(&mut self, row: Pick, m: KeyMods) -> bool {
        if m.shift {
            let rows = self.pick_rows();
            let from = self.pick_from();
            let (Some(a), Some(b)) = (rows.iter().position(|r| *r == from), rows.iter().position(|r| *r == row)) else {
                self.picked = vec![row];
                return true;
            };
            let (a, b) = (a.min(b), a.max(b));
            self.picked = rows[a..=b].to_vec();
            self.pick_anchor = Some(from);
            self.pick_head = Some(row);
            return true;
        }
        if adds(m) {
            // The open note (or the folder shown) is chosen already: the
            // first ⌘-click adds to it.
            if self.picked.is_empty() {
                let from = self.pick_from();
                if from != row {
                    self.picked.push(from);
                }
            }
            match self.picked.iter().position(|r| *r == row) {
                Some(k) => {
                    self.picked.remove(k);
                }
                None => self.picked.push(row.clone()),
            }
            self.pick_anchor = Some(row.clone());
            self.pick_head = Some(row);
            return true;
        }
        self.picked.clear();
        self.pick_anchor = Some(row);
        self.pick_head = None;
        false
    }

    /// ⇧↑ or ⇧↓ in one of the library's lists, `rows` (the folders, or
    /// the notes shown), the keyboard on `at`: the range from where it
    /// began to its moving end goes one row further, or back toward where
    /// it began — what a ⇧-click on that row would choose. With nothing
    /// chosen yet, it begins at `at`. The row come to, if any.
    pub(super) fn pick_step(&mut self, up: bool, at: Pick, rows: &[Pick]) -> Option<Pick> {
        let pos = |p: &Pick| rows.iter().position(|r| r == p);
        let ends = (!self.picked.is_empty())
            .then(|| self.pick_anchor.as_ref().zip(self.pick_head.as_ref()))
            .flatten()
            .and_then(|(a, h)| pos(a).zip(pos(h)));
        let (a, h) = match ends {
            Some(ends) => ends,
            None => {
                let i = pos(&at)?;
                (i, i)
            }
        };
        let to = if up { h.checked_sub(1) } else { Some(h + 1) }.filter(|&i| i < rows.len()).unwrap_or(h);
        self.picked = rows[a.min(to)..=a.max(to)].to_vec();
        self.pick_anchor = Some(rows[a].clone());
        self.pick_head = Some(rows[to].clone());
        Some(rows[to].clone())
    }

    /// Whether a row is among those chosen.
    pub(super) fn is_picked(&self, row: &Pick) -> bool {
        self.picked.len() > 1 && self.picked.contains(row)
    }

    /// What is chosen, as it still is: folders that are gone and notes
    /// that are gone left out, and what is inside a chosen folder left to
    /// go with it.
    pub(super) fn picked_now(&self) -> (Vec<String>, Vec<u64>) {
        let mut folders: Vec<String> = self
            .picked
            .iter()
            .filter_map(|p| match p {
                Pick::Folder(f) if self.store.folders.contains(f) => Some(f.clone()),
                _ => None,
            })
            .collect();
        let inside = |f: &str, of: &str| f == of || f.starts_with(&format!("{of}/"));
        let all = folders.clone();
        folders.retain(|f| !all.iter().any(|o| o != f && inside(f, o)));
        let notes = self
            .picked
            .iter()
            .filter_map(|p| match p {
                Pick::Note(id) => self.store.notes.iter().find(|n| n.id == *id),
                _ => None,
            })
            .filter(|n| !folders.iter().any(|f| inside(&n.folder, f)))
            .map(|n| n.id)
            .collect();
        (folders, notes)
    }

    /// "3 notes", "2 folders and a note": what is chosen, in words.
    pub(super) fn picked_words(folders: usize, notes: usize) -> String {
        let count = |n: usize, one: &str, many: &str| match n {
            1 => format!("a {one}"),
            n => format!("{n} {many}"),
        };
        match (folders, notes) {
            (0, n) => count(n, "note", "notes"),
            (f, 0) => count(f, "folder", "folders"),
            (f, n) => format!("{} and {}", count(f, "folder", "folders"), count(n, "note", "notes")),
        }
    }

    /// The menu for what is chosen: to the trash, or to a folder (never
    /// one of the chosen folders, or one inside them).
    pub(super) fn picked_menu(&self) -> Vec<MenuItem> {
        let (folders, notes) = self.picked_now();
        let what = Self::picked_words(folders.len(), notes.len());
        let item = |label: String, to: Option<&str>| {
            let mut m = vec![("picked", Value::str(if to.is_some() { "move" } else { "trash" }))];
            if let Some(f) = to {
                m.push(("folder", Value::str(f)));
            }
            MenuItem::new(label).id(Value::map(m))
        };
        let mut items = Vec::new();
        if !folders.is_empty() || !notes.is_empty() {
            let inside = |f: &str| folders.iter().any(|o| f == o || f.starts_with(&format!("{o}/")));
            let mut targets = vec![item("To the top".into(), Some(""))];
            targets.extend(self.store.folders.iter().filter(|f| !inside(f)).map(|f| item(f.clone(), Some(f))));
            items.push(MenuItem::submenu(format!("Move {what} to"), targets));
            items.push(MenuItem::separator());
        }
        items.push(item(format!("Move {what} to the trash"), None).accel(crate::keys::TRASH));
        items
    }

    /// The chosen notes and folders to the trash, together: one notice
    /// says what went, and its Undo brings all of it back.
    pub(super) fn trash_picked(&mut self) {
        let (folders, notes) = self.picked_now();
        self.picked.clear();
        self.pick_anchor = None;
        self.pick_head = None;
        let mut undo = Vec::new();
        let (mut gone_f, mut gone_n) = (0, 0);
        for f in &folders {
            let had = self.store.folders.contains(f);
            self.notice = None;
            self.folder_action(f, "delete");
            if had && !self.store.folders.contains(f) {
                gone_f += 1;
            }
            undo.extend(self.notice.take().and_then(|n| n.undo));
        }
        for id in notes {
            let Some(i) = self.store.notes.iter().position(|n| n.id == id) else { continue };
            self.notice = None;
            self.delete_note_at(i);
            if !self.store.notes.iter().any(|n| n.id == id) {
                gone_n += 1;
            }
            undo.extend(self.notice.take().and_then(|n| n.undo));
        }
        if gone_f + gone_n > 0 {
            let what = Self::picked_words(gone_f, gone_n);
            self.notify(format!("Moved {what} to the trash"), (!undo.is_empty()).then_some(Undo::Many(undo)));
        }
    }

    /// The chosen notes and folders into folder `to` ("" for the top
    /// level), folders with all in them; none into itself or a folder
    /// inside it.
    pub(super) fn move_picked(&mut self, to: &str) {
        let (folders, notes) = self.picked_now();
        self.picked.clear();
        self.pick_anchor = None;
        self.pick_head = None;
        let mut moved_f = 0;
        for f in folders {
            if parent_of(&f) == to || to == f || to.starts_with(&format!("{f}/")) {
                continue;
            }
            if self.move_folder(&f, to).is_some() {
                moved_f += 1;
            }
        }
        let mut moved = 0;
        for id in notes {
            let Some(i) = self.store.notes.iter().position(|n| n.id == id) else { continue };
            if self.store.notes[i].folder == to {
                continue;
            }
            self.move_note(i, to);
            if self.store.notes.iter().any(|n| n.id == id && n.folder == to) {
                moved += 1;
            }
        }
        if moved_f + moved > 0 {
            let whose = if moved_f + moved > 1 { "their" } else { "its" };
            self.notify(format!("Moved {} {}", Self::picked_words(moved_f, moved), Self::place_words(to, whose)), None);
        }
    }
}
