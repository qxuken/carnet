//! A note as a CRDT, and this device's copy of them all.
//!
//! Every note's file is a Loro text, byte for byte ([`NoteDoc`]): a save
//! here, an edit made in another app and, later, an edit from another
//! device all become changes to it, so two of them made apart merge
//! instead of forking, and every version stays in its history. The files
//! are still the truth; this is what they remember (docs/SYNC.md §3, §5).
//!
//! [`Replica`] keeps those docs on this device, beside the settings and
//! never in the notes folder. It is native only: the PWA keeps its own in
//! IndexedDB.

pub mod compat;
pub mod crypto;
pub mod elo;
pub mod invite;
#[cfg(all(feature = "net", not(target_arch = "wasm32")))]
pub mod http;
#[cfg(all(feature = "net", not(target_arch = "wasm32")))]
pub mod net;
pub mod pair;
pub mod passkey;
pub mod password;
mod note;
#[cfg(not(target_arch = "wasm32"))]
mod replica;
pub mod session;
pub mod vault;
pub mod wire;

pub use note::{Error, Key, NoteDoc, Origin, Revision, Version};
#[cfg(not(target_arch = "wasm32"))]
pub use replica::{Entry, LAYOUT, Newer, Replica, text_hash};
