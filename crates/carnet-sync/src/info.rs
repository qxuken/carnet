//! What a server says of itself at `GET /v1/info`, read before the app
//! connects (docs/SYNC.md §14): whether it's a Carnet server at all, how
//! it and this build get on, and what it can do. A connect screen offers
//! only the ways in its server lists: a code always, an email and
//! password with `passwords`, a passkey with `passkeys`, a new account
//! with `signup`.

use crate::compat::{Fit, Greeting, feature};
use crate::http::Answer;

/// Where a server says what it is.
pub fn url(origin: &str) -> String {
    format!("{origin}/v1/info")
}

/// A Carnet server, as it says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerInfo {
    /// Its protocol, the oldest app it takes, where deprecation starts,
    /// and what it can do: what its socket's greeting says too.
    pub greeting: Greeting,
    /// Its build's version, as it says it: `0.1.1`.
    pub version: String,
    /// Its own signup page, if it has one (a hosted service's).
    pub signup_page: Option<String>,
    /// Which email a password signs in with, if it takes passwords:
    /// `any`, or `account` (the one the account signed up with).
    pub password_email: Option<String>,
}

impl ServerInfo {
    pub fn has(&self, feature: &str) -> bool {
        self.greeting.has(feature)
    }

    /// How this build and the server get on.
    pub fn fit(&self) -> Fit {
        self.greeting.fit()
    }

    /// Whether anyone may make an account here, by email. A hosted
    /// service from before signups were listed said only its page.
    pub fn takes_signups(&self) -> bool {
        self.has(feature::SIGNUP) || self.signup_page.is_some()
    }

    /// Whether a device may come in by an email and password here.
    pub fn takes_passwords(&self) -> bool {
        self.has(feature::PASSWORDS)
    }

    /// Whether a password's email is the one the account signed up with.
    pub fn password_is_signup_email(&self) -> bool {
        self.password_email.as_deref() == Some("account")
    }
}

/// Why the app can't go on with a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Nothing answered: a wrong address, or no connection.
    Unreachable,
    /// The DNS has no address for the server's name (its host): mistyped,
    /// or new and not seen everywhere yet.
    DomainNotFound(String),
    /// Something answered, but not a Carnet server.
    NotCarnet,
    /// A Carnet server this build and it can't talk.
    Refused(Fit),
}

impl Problem {
    /// What's said of it, to the person holding the app.
    pub fn words(&self) -> String {
        match self {
            Problem::Unreachable => "Couldn't reach that server. Check the address and your connection.".into(),
            Problem::DomainNotFound(host) => format!("Domain not found: {host}. Check the address; a new name can take a few minutes to appear."),
            Problem::NotCarnet => "Something answers at that address, but it isn't a Carnet server.".into(),
            Problem::Refused(fit) => fit.refusal().unwrap_or("Carnet can't sync with this server.").into(),
        }
    }
}

/// What an answer to `GET /v1/info` says: a Carnet server's info, or
/// what's wrong. One without a protocol is from before there were any:
/// protocol 0, nothing listed.
pub fn read(answer: &Answer) -> Result<ServerInfo, Problem> {
    if !(200..300).contains(&answer.status) {
        return Err(Problem::NotCarnet);
    }
    let v: serde_json::Value = serde_json::from_slice(&answer.body).map_err(|_| Problem::NotCarnet)?;
    if v.get("server").and_then(serde_json::Value::as_str) != Some("carnet-server") {
        return Err(Problem::NotCarnet);
    }
    let n = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0) as u32;
    let s = |k: &str| v.get(k).and_then(serde_json::Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let features = v
        .get("features")
        .and_then(serde_json::Value::as_array)
        .map(|f| f.iter().filter_map(|f| f.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let info = ServerInfo {
        greeting: Greeting { protocol: n("protocol"), oldest: n("oldest"), deprecated: n("deprecated"), features },
        version: s("version").unwrap_or_default(),
        signup_page: s("signup"),
        password_email: s("password_email"),
    };
    match info.fit() {
        f @ (Fit::AppTooOld | Fit::ServerTooOld) => Err(Problem::Refused(f)),
        Fit::Fine | Fit::UpdateSoon => Ok(info),
    }
}

/// [`read`], for an answer that may not have come: a name with no
/// address is told apart from a server that didn't answer.
pub fn checked(answer: Result<Answer, String>) -> Result<ServerInfo, Problem> {
    answer
        .map_err(|why| match crate::http::domain_not_found(&why) {
            Some(host) => Problem::DomainNotFound(host.to_string()),
            None => Problem::Unreachable,
        })
        .and_then(|a| read(&a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::{OLDEST, PROTOCOL};

    fn answer(status: u16, body: &str) -> Answer {
        Answer { status, body: body.as_bytes().to_vec(), ..Answer::default() }
    }

    #[test]
    fn a_servers_info_says_what_it_can_do() {
        let hosted = format!(
            r#"{{"server":"carnet-server","version":"0.1.1","signup":"https://x.co/start","passwords":true,"password_email":"account","protocol":{PROTOCOL},"oldest":{OLDEST},"deprecated":{OLDEST},"features":["passkeys","passwords","signup"]}}"#
        );
        let i = read(&answer(200, &hosted)).unwrap();
        assert!(i.takes_signups() && i.takes_passwords() && i.has(feature::PASSKEYS) && i.password_is_signup_email());
        assert_eq!((i.signup_page.as_deref(), i.version.as_str(), i.fit()), (Some("https://x.co/start"), "0.1.1", Fit::Fine));
        let own = format!(r#"{{"server":"carnet-server","protocol":{PROTOCOL},"oldest":{OLDEST},"deprecated":{OLDEST},"features":["passkeys"]}}"#);
        let i = read(&answer(200, &own)).unwrap();
        assert!(!i.takes_signups() && !i.takes_passwords());
        // A hosted service from before signups were a feature.
        let older = format!(r#"{{"server":"carnet-server","signup":"https://x.co/start","protocol":{PROTOCOL},"oldest":{OLDEST},"deprecated":{OLDEST},"features":[]}}"#);
        assert!(read(&answer(200, &older)).unwrap().takes_signups());
    }

    #[test]
    fn what_isnt_a_server_to_go_on_with_is_said() {
        assert_eq!(read(&answer(404, "not found")), Err(Problem::NotCarnet));
        assert_eq!(read(&answer(200, "<html>")), Err(Problem::NotCarnet));
        assert_eq!(read(&answer(200, r#"{"server":"something-else"}"#)), Err(Problem::NotCarnet));
        // From before protocols: too old for this build.
        assert_eq!(read(&answer(200, r#"{"server":"carnet-server"}"#)), Err(Problem::Refused(Fit::ServerTooOld)));
        let later = format!(r#"{{"server":"carnet-server","protocol":{0},"oldest":{0},"deprecated":{0}}}"#, PROTOCOL + 1);
        assert_eq!(read(&answer(200, &later)), Err(Problem::Refused(Fit::AppTooOld)));
        assert_eq!(checked(Err("refused".into())), Err(Problem::Unreachable));
        assert_eq!(checked(Err("timed out".into())), Err(Problem::Unreachable));
        let unknown = crate::http::ask("http://carnet-test.invalid/v1/info".into(), None, || {}).unwrap().recv().unwrap();
        let p = checked(unknown).unwrap_err();
        assert_eq!(p, Problem::DomainNotFound("carnet-test.invalid".into()));
        assert_eq!(p.words(), "Domain not found: carnet-test.invalid. Check the address; a new name can take a few minutes to appear.");
        for p in [Problem::Unreachable, Problem::DomainNotFound("x.example".into()), Problem::NotCarnet, Problem::Refused(Fit::ServerTooOld)] {
            assert!(p.words().ends_with('.'), "{p:?}");
        }
    }
}
