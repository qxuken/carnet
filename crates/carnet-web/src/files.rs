//! What the page binds for files put in a note and opened from one, for
//! Markdown files dropped in as notes, and for the welcome note: the
//! vault's side of each ([`Notes`](crate::notes::Notes)), and the rules
//! the desktop goes by, from carnet-core.

use carnet_core::{doc, welcome};
use wasm_bindgen::prelude::*;

use crate::{Vault, WebRandom};

#[wasm_bindgen]
impl Vault {
    /// Where a file put in here, named `name` where it came from, goes:
    /// `attachments/` and its own name made plain, or the first one free.
    pub fn file_free(&self, name: &str) -> String {
        self.notes.file_free(name)
    }

    /// The file a link in a note in `folder` leads to (`href`, as the page
    /// shows it), as a path from the notes folder; none for a web address,
    /// a place in the note, or a file not in the vault.
    pub fn linked_file(&self, folder: &str, href: &str) -> Option<String> {
        self.notes.linked_file(folder, href)
    }

    /// A Markdown file dropped on the page brought in as a note in
    /// `folder`: its name and words, and the files dropped with it that it
    /// links to (their names, and the paths they were put at, in the same
    /// order). Its key; refused, with why.
    pub fn import_note(&mut self, folder: &str, file: &str, text: &str, names: Vec<String>, paths: Vec<String>) -> Result<String, JsError> {
        let brought: Vec<(String, String)> = names.into_iter().zip(paths).collect();
        self.notes.import_note(folder, file, text, &brought, &mut WebRandom).map_err(|e| JsError::new(&e))
    }

    /// The welcome note, its shortcuts as this browser's system writes
    /// them (`mac`): the one nobody has written in, or a new one. Its key.
    pub fn add_welcome(&mut self, mac: bool) -> String {
        let md = welcome_markdown(mac);
        self.notes.add_welcome(&md, &mut WebRandom)
    }
}

/// The welcome note's Markdown as a browser on a Mac (`mac`) or elsewhere
/// makes it: the desktop's words and keys, written as that system writes
/// them.
#[wasm_bindgen]
pub fn welcome_markdown(mac: bool) -> String {
    welcome::markdown(|s| welcome::label(s.spec(), mac))
}

/// Whether a file of this name could run a program, install something
/// or lead elsewhere as it opens (`doc::could_run`): asked about first.
#[wasm_bindgen]
pub fn could_run(name: &str) -> bool {
    doc::could_run(name)
}

/// The tiles a line of a note's words is drawn as, the desktop's rule
/// (`doc::file_line`): `[[name, href, kind]]` as JSON, one for each link
/// to a file the note keeps, when the line is those links and nothing
/// else; `null` for any other line, the words it is.
#[wasm_bindgen]
pub fn file_line(line: &str) -> String {
    match doc::file_line(line) {
        Some(files) => {
            let tiles: Vec<String> = files.iter().map(|f| format!("[{},{},{}]", crate::json(&f.name), crate::json(&f.target), crate::json(&doc::file_kind(&f.path)))).collect();
            format!("[{}]", tiles.join(","))
        }
        None => "null".into(),
    }
}

/// Whether a file of this name goes in as a picture (`doc::is_picture_name`),
/// where any other is linked on a line of its own, as a tile.
#[wasm_bindgen]
pub fn is_picture_name(name: &str) -> bool {
    doc::is_picture_name(name)
}

/// The names of the files a Markdown file's links name by a path,
/// lowercase: of those dropped with it, the ones it brings.
#[wasm_bindgen]
pub fn import_names(text: &str) -> Vec<String> {
    crate::import::linked_names(text)
}

/// The most a Markdown file may be to come in as a note, in bytes.
#[wasm_bindgen]
pub fn import_most() -> f64 {
    crate::notes::IMPORT_MOST as f64
}
