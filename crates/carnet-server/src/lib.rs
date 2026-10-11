//! Carnet's sync server (docs/SYNC.md §6): the hub, which logs devices in,
//! keeps and passes on the sealed records of their notes, and tells each
//! what changed while it was away; and the storage it keeps them in. It
//! never holds a key: what it stores, a stolen disk shows as noise.

pub mod blobs;
pub mod hub;
pub mod limits;
pub mod links;
#[cfg(feature = "postgres")]
pub mod migrate;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod quota;
pub mod rows;
#[cfg(feature = "s3")]
pub mod s3;
pub mod server;
pub mod sqlite;
pub mod storage;

pub use hub::{ConnId, Hub, Out};
pub use sqlite::Sharded;
pub use storage::{Memory, Storage};
