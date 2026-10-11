//! A note as Carnet writes and reads it: the block model and its GFM
//! Markdown form ([`doc`]), the inline styles inside a block's text
//! ([`inline`]), the note as HTML ([`export`]), two versions of it
//! compared ([`compare`]), its file name ([`name`]), and the page's editing
//! ([`page`]: the caret, the selection and every edit, as plain functions;
//! [`history`]: undo and redo), shared by the desktop and the web editor.
//!
//! Nothing here draws or touches a disk, so the same Markdown dialect runs
//! in the desktop app and, compiled to wasm, in the PWA (docs/SYNC.md §8).

pub mod compare;
pub mod doc;
pub mod export;
pub mod history;
pub mod inline;
pub mod name;
pub mod page;
