//! Which apps and servers can talk (docs/SYNC.md §14).
//!
//! Each build speaks one protocol, a number raised whenever what goes
//! over the socket changes in a way an older peer would read wrongly or
//! not at all. It talks to peers as old as [`OLDEST`]; one older than
//! [`DEPRECATED`] is still served, and told to update soon.
//!
//! The app says its protocol in the socket's address (`?protocol=N`; none
//! is 0, the builds before there was one). The server says its own, the
//! oldest it takes, where its deprecation starts and what it can do in
//! every [`Greeting`], which rides on its challenge: an app too old is
//! turned away with [`crate::wire::Deny::Outdated`], and an app finds a
//! server too old for it by the greeting alone, before it says anything.

/// The protocol this build speaks.
///
/// - 1: the first.
/// - 2: a new epoch's key goes signed by the device that made it
///   (`rekey.rs`), and a device takes no other. `Rekey`, `Revoke`'s keys
///   and `Recovered`'s are laid out as before, and hold that.
pub const PROTOCOL: u32 = 2;
/// The oldest protocol this build talks to. Protocol 1's keys aren't
/// signed, and this build refuses them; and no release with sync spoke it.
pub const OLDEST: u32 = 2;
/// Peers older than this are still served, and told to update.
pub const DEPRECATED: u32 = 2;

/// What a server can do, by name, as its [`Greeting`] lists them. An app
/// leaves out what its server doesn't list, rather than asking and being
/// refused.
pub mod feature {
    /// Signing in with an email and password (`--passwords`).
    pub const PASSWORDS: &str = "passwords";
    /// Signing in with a passkey's PRF.
    pub const PASSKEYS: &str = "passkeys";
    /// An account deleted from one of its devices.
    pub const ACCOUNT_DELETE: &str = "account-delete";
    /// Notes deleted forever and pictures dropped.
    pub const REMOVE: &str = "remove";
    /// What an account keeps, and may.
    pub const USAGE: &str = "usage";
    /// Accounts made by anyone, by email: `POST /v1/signup` mails an
    /// invite (a hosted service, docs/SYNC.md §11). Without it, accounts
    /// come only from invites the server's owner makes.
    pub const SIGNUP: &str = "signup";
}

/// The server's side, on each challenge: its protocol, the oldest app it
/// takes, where deprecation starts, and its features. A challenge without
/// one is from a server older than greetings: protocol 0, nothing listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Greeting {
    pub protocol: u32,
    pub oldest: u32,
    pub deprecated: u32,
    pub features: Vec<String>,
}

impl Greeting {
    /// This build's greeting, as a server sends it.
    pub fn ours(features: Vec<String>) -> Self {
        Greeting { protocol: PROTOCOL, oldest: OLDEST, deprecated: DEPRECATED, features }
    }

    pub fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }

    /// How this build and the server it came from get on.
    pub fn fit(&self) -> Fit {
        fit(PROTOCOL, OLDEST, self)
    }
}

/// How an app and a server get on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Fine,
    /// The server still takes this app, not for much longer.
    UpdateSoon,
    /// The server no longer takes this app.
    AppTooOld,
    /// The app no longer talks to this server.
    ServerTooOld,
}

impl Fit {
    /// Why the two can't talk, said to the person holding the app.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            Fit::AppTooOld => Some(APP_TOO_OLD),
            Fit::ServerTooOld => Some(SERVER_TOO_OLD),
            Fit::Fine | Fit::UpdateSoon => None,
        }
    }
}

pub const APP_TOO_OLD: &str = "This version of Carnet is too old for the server. Update Carnet to keep syncing.";
pub const SERVER_TOO_OLD: &str = "The server is too old for this version of Carnet. Whoever runs it needs to update it.";
pub const UPDATE_SOON: &str = "The server will stop syncing with this version of Carnet soon. Update Carnet to keep syncing.";

/// An app speaking `protocol`, which talks to servers as old as `oldest`,
/// and the server that greeted it.
pub fn fit(protocol: u32, oldest: u32, server: &Greeting) -> Fit {
    if server.protocol < oldest {
        Fit::ServerTooOld
    } else if protocol < server.oldest {
        Fit::AppTooOld
    } else if protocol < server.deprecated {
        Fit::UpdateSoon
    } else {
        Fit::Fine
    }
}

/// What a server makes of an app speaking `protocol`.
pub fn takes(protocol: u32) -> Fit {
    if protocol < OLDEST {
        Fit::AppTooOld
    } else if protocol < DEPRECATED {
        Fit::UpdateSoon
    } else {
        Fit::Fine
    }
}

/// The socket's address with this build's protocol on it.
pub fn with_protocol(url: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}protocol={PROTOCOL}")
}

/// The same, with which app it is and its version, for the server's
/// counts of which apps are about: `desktop-macos`, `0.1.1`.
pub fn with_app(url: &str, app: &str, version: &str) -> String {
    format!("{}&app={}&version={}", with_protocol(url), label(app), label(version))
}

/// What an app said of itself in its socket's query.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Client {
    /// None is 0: a build from before protocols.
    pub protocol: u32,
    /// Which app, and its version; empty when it didn't say. Held to a
    /// label's letters and length, whatever was sent: they're metrics'
    /// labels and the helpdesk's words.
    pub app: String,
    pub version: String,
}

impl Client {
    pub fn from_query(query: Option<&str>) -> Self {
        let get = |k: &str| query.into_iter().flat_map(|q| q.split('&')).find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='));
        Client {
            protocol: get("protocol").and_then(|v| v.parse().ok()).unwrap_or(0),
            app: get("app").map(label).unwrap_or_default(),
            version: get("version").map(label).unwrap_or_default(),
        }
    }

    /// The app and its version, as people read them: `desktop-macos
    /// 0.1.1`; "an app from before" when it said nothing.
    pub fn words(&self) -> String {
        match (self.app.is_empty(), self.version.is_empty()) {
            (true, _) => "an app that didn't say".into(),
            (false, true) => self.app.clone(),
            (false, false) => format!("{} {}", self.app, self.version),
        }
    }
}

/// `s` as a label: letters, digits and `.-+_` only, 24 at most; anything
/// else dropped.
fn label(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_')).take(24).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(protocol: u32, oldest: u32, deprecated: u32) -> Greeting {
        Greeting { protocol, oldest, deprecated, features: Vec::new() }
    }

    #[test]
    fn an_app_and_a_server_get_on_within_each_others_window() {
        // An app on 5 that talks to servers from 3, against servers:
        assert_eq!(fit(5, 3, &server(5, 3, 4)), Fit::Fine);
        assert_eq!(fit(5, 3, &server(9, 4, 6)), Fit::UpdateSoon, "deprecated, still served");
        assert_eq!(fit(5, 3, &server(9, 6, 6)), Fit::AppTooOld);
        assert_eq!(fit(5, 3, &server(2, 1, 1)), Fit::ServerTooOld);
        assert_eq!(fit(5, 3, &Greeting::default()), Fit::ServerTooOld, "no greeting: a server before there were any");
        assert_eq!(fit(5, 0, &Greeting::default()), Fit::Fine);
        // This build, against itself.
        assert_eq!(Greeting::ours(Vec::new()).fit(), Fit::Fine);
        assert_eq!(takes(PROTOCOL), Fit::Fine);
        assert_eq!(takes(0), if OLDEST > 0 { Fit::AppTooOld } else { Fit::Fine });
    }

    #[test]
    fn the_protocol_rides_in_the_query() {
        assert_eq!(with_protocol("wss://x/v1/sync"), format!("wss://x/v1/sync?protocol={PROTOCOL}"));
        assert_eq!(with_protocol("wss://x/v1/sync?a=1"), format!("wss://x/v1/sync?a=1&protocol={PROTOCOL}"));
        assert_eq!(Client::from_query(Some("a=1&protocol=7")).protocol, 7);
        assert_eq!(Client::from_query(Some("protocol=x")).protocol, 0);
        assert_eq!(Client::from_query(None), Client::default());
        let url = with_app("wss://x/v1/sync", "desktop-macos", "0.1.1");
        let c = Client::from_query(url.split_once('?').map(|(_, q)| q));
        assert_eq!(c, Client { protocol: PROTOCOL, app: "desktop-macos".into(), version: "0.1.1".into() });
        assert_eq!(c.words(), "desktop-macos 0.1.1");
        // Whatever is sent, a label: no quotes, no braces, not long.
        let odd = Client::from_query(Some("app=\"}evil{&version=1234567890123456789012345678"));
        assert_eq!((odd.app.as_str(), odd.version.len()), ("evil", 24));
    }

    #[test]
    fn the_window_is_in_order() {
        const { assert!(OLDEST <= DEPRECATED && DEPRECATED <= PROTOCOL) };
    }
}
