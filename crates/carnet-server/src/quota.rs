//! How much an account may keep, and whether it may write (docs/SYNC.md
//! §11): the seam a hosted service's plans plug into. A server of one's
//! own keeps everything, always ([`Unlimited`]).
//!
//! The hub asks before it takes a write, and the picture worker before it
//! gives a link to put a new picture. Notes' text is small: it's counted,
//! never refused for size, only when the account can't write at all. A
//! picture is let in while the account is under its limit, so one picture
//! may take it over, never two.

/// What an account may keep now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limit {
    /// Bytes, notes and pictures together (as sealed); `None`: no limit.
    pub bytes: Option<u64>,
    /// False: read only (a plan lapsed). What's there stays, and syncs to
    /// every device; nothing new is taken until it can write again.
    pub writable: bool,
    /// The plan's name, as the devices show it; "" on a server of one's
    /// own.
    pub plan: String,
    /// When the plan runs out, in seconds since 1970; 0 for never.
    pub until: u64,
}

impl Default for Limit {
    fn default() -> Self {
        Limit { bytes: None, writable: true, plan: String::new(), until: 0 }
    }
}

pub trait Quota: Send + Sync {
    fn limit(&self, account: &[u8]) -> Limit;
}

/// A server of one's own: no limits.
pub struct Unlimited;

impl Quota for Unlimited {
    fn limit(&self, _: &[u8]) -> Limit {
        Limit::default()
    }
}

/// Who is let in, as a host sees it: each invite used, and the account it
/// made (a hosted service: which signup became which account). A server
/// of one's own needs nothing told.
pub trait Admission: Send + Sync {
    fn admitted(&self, invite: [u8; 32], account: &[u8]);
    /// The account is gone: deleted from one of its devices, or removed.
    fn removed(&self, _account: &[u8]) {}
}

/// A server of one's own: invites are the whole story.
pub struct ByInvite;

impl Admission for ByInvite {
    fn admitted(&self, _: [u8; 32], _: &[u8]) {}
}

/// What a device is told when the account is full.
pub const FULL: &str = "This account is full, so new pictures stay on this device for now. Delete some, or move to a bigger plan.";

/// What a device is told when the account can't write.
pub const READ_ONLY: &str = "This account is read only for now: changes stay on this device, and sync once it can write again.";
