//! How much an account may keep, and whether it may write (docs/SYNC.md
//! §11): the seam a hosted service's plans plug into. A server of one's
//! own keeps everything, always ([`Unlimited`]).
//!
//! The hub asks before it takes a write, and the file worker before it
//! gives a link to put a new file. Notes' text is small: it's counted,
//! and not refused at the account's limit, only when the account can't
//! write at all, or past [`Quota::notes_ceiling`], far above what notes
//! take (a ceiling against an account that never compacts, not a plan).
//! A file is let in while the account is under its limit, with the
//! files it has links to put and hasn't yet counted in, so one file
//! may take it over, never two.

/// What an account may keep now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limit {
    /// Bytes, notes and files together (as sealed); `None`: no limit.
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

/// The most an account's notes may take, sealed, history and all, unless
/// its [`Quota`] says otherwise: past it, the hub takes no more writes
/// to its notes (snapshots, which make them smaller, still).
pub const NOTES_CEILING: u64 = 4 << 30;

pub trait Quota: Send + Sync {
    fn limit(&self, account: &[u8]) -> Limit;

    /// The most `account`'s notes may take, sealed: a ceiling, not a
    /// plan's limit (notes still sync on an account at its limit). Asked
    /// on the hub's thread before each write: quick, or kept.
    fn notes_ceiling(&self, _account: &[u8]) -> u64 {
        NOTES_CEILING
    }
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
    /// Whether an invite (by its code's hash) may make an account, asked
    /// before it's taken: a hosted service lets in only the invites its
    /// signups sent. No, and the device is told as of an invite that
    /// isn't good, and the invite stays as it was. Asked on the hub's
    /// thread: quick. Yes unless the host says.
    fn admit(&self, _invite: &[u8; 32]) -> bool {
        true
    }
    fn admitted(&self, invite: [u8; 32], account: &[u8]);
    /// The account is gone: deleted from one of its devices, or removed.
    fn removed(&self, _account: &[u8]) {}
    /// The email the account came in by, if the host knows one (a hosted
    /// service: the one its invite was mailed to, and so confirmed). What
    /// [`PasswordEmails::Account`] holds passwords to. Asked on the hub's
    /// thread: quick, or kept.
    fn email_of(&self, _account: &[u8]) -> Option<String> {
        None
    }
}

/// Which email a password may sign in as (docs/SYNC.md §7). The email is
/// the password's name, nothing more: OPAQUE never shows the server the
/// password, and a sign-in finds the account by the email. Who may name
/// a password after which email is the server's to say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PasswordEmails {
    /// Any the account's device types, not taken by another account: a
    /// server of one's own, which mails nobody and knows no emails.
    #[default]
    Any,
    /// Only the email the account came in by, as its host says
    /// ([`Admission::email_of`]): a hosted service, whose signup mailed
    /// the invite to it, so it's known to be theirs. Another is refused,
    /// with the one it must be ([`password_email_is`]); an account the
    /// host knows no email of can't set one. The account's devices are
    /// told the email with their list, to show it rather than ask.
    ///
    /// (A third, `Confirm`, is for later: any email, once a code mailed
    /// to it comes back. It needs a seam to mail by, and a frame for the
    /// code between `PasswordSetStart` and `PasswordSetFinish`: the
    /// server mails the code on the start, keeps it with the account for
    /// ten minutes, and takes the finish only with it.)
    Account,
}

impl PasswordEmails {
    /// How `/v1/info` says it.
    pub fn name(self) -> &'static str {
        match self {
            PasswordEmails::Any => "any",
            PasswordEmails::Account => "account",
        }
    }
}

/// What a device is told when it sets a password with an email other than
/// the account's, under [`PasswordEmails::Account`].
pub fn password_email_is(email: &str) -> String {
    format!("On this service, the password signs in with the email you signed up with: {email}")
}

/// Under [`PasswordEmails::Account`], when the host knows no email for the
/// account.
pub const NO_ACCOUNT_EMAIL: &str = "On this service, the password signs in with the email you signed up with, and this account has none here: ask the service's help";

/// A server of one's own: invites are the whole story.
pub struct ByInvite;

impl Admission for ByInvite {
    fn admitted(&self, _: [u8; 32], _: &[u8]) {}
}

/// What a device is told when the account is full, and when it can't
/// write: the apps know these words (`carnet_sync::session`), and offer
/// what was held back once there's room.
pub use carnet_sync::session::{FULL, READ_ONLY};

/// What a device is told when a note's changes are past what the server
/// keeps of one room until it's compacted (docs/SYNC.md §6).
pub const ROOM_FULL: &str = "This note has more changes on the server than it keeps before tidying them up, so new ones wait on this device for now. Carnet tidies notes by itself: keep it open and connected for a few minutes.";

/// What a device is told when the account's notes are past
/// [`Quota::notes_ceiling`], or it has as many notes as a server keeps.
pub const NOTES_FULL: &str = "This account's notes take all the room the server gives notes, so new changes wait on this device for now. Delete notes forever to make room.";
