//! Whether a newer Carnet is out: GitHub's latest release of the
//! repository, asked when the app starts and then once a day while it
//! runs, never more often (a question that got no answer at all, offline,
//! is asked again an hour on). Nothing is sent but the request itself: no
//! token, no id, a `User-Agent` GitHub asks for, and the last answer's
//! ETag, so an unchanged answer is a short `304`. Nothing is installed:
//! Settings says the version is out and opens its page. Off in Settings
//! ("Check for updates"), nothing is asked. A failure is said nowhere.

use std::cmp::Ordering;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use carnet_sync::http::{Answer, Ask};

/// Where GitHub says the repository's latest release: the newest that's
/// neither a draft nor a prerelease.
pub const LATEST: &str = "https://api.github.com/repos/qxuken/carnet/releases/latest";

/// Where a release's page is, by its tag.
const RELEASES: &str = "https://github.com/qxuken/carnet/releases/tag/";

/// How long after an answer the next question is asked.
pub const EVERY: Duration = Duration::from_secs(24 * 3600);

/// How long after a question that got no answer (offline, the name not
/// found, timed out) it's asked again.
pub const RETRY: Duration = Duration::from_secs(3600);

/// A version as Cargo and the release's tag write it: `0.1.2`, `v0.1.2`,
/// `0.2.0-beta.1`; what's after a `+` is passed over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// After the `-`: a prerelease, older than the version without it.
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let s = s.split_once('+').map_or(s, |(v, _)| v);
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
            Some(_) => return None,
            None => (s, None),
        };
        let mut n = core.split('.').map(|p| if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) { p.parse::<u64>().ok() } else { None });
        let (major, minor, patch) = (n.next()??, n.next()??, n.next()??);
        if n.next().is_some() {
            return None;
        }
        Some(Version { major, minor, patch, pre })
    }

    /// This build's.
    pub fn this() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo's version")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => pre_cmp(a, b),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Two prereleases, as semver orders them: part by part, numbers by
/// value and below words, and fewer parts first.
fn pre_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.split('.'), b.split('.'));
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

/// A release, as GitHub lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Its tag, as written: `v0.1.2`.
    pub tag: String,
}

impl Release {
    /// Its page on GitHub, where its downloads are: made here, from the
    /// tag, never taken from the answer.
    pub fn page(&self) -> String {
        format!("{RELEASES}{}", self.tag)
    }
}

/// What an answer from GitHub says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// The latest release, and the answer's ETag to ask with next.
    Latest(Release, Option<String>),
    /// As it was last time (`304`, asked with its ETag).
    Same,
    /// Nothing to go by: no release yet, too many questions, not what
    /// GitHub says.
    Nothing,
}

/// An answer from [`LATEST`], read: a release that is neither a draft nor
/// a prerelease, its tag a version.
pub fn read(answer: &Answer) -> Said {
    if answer.status == 304 {
        return Said::Same;
    }
    if answer.status != 200 {
        return Said::Nothing;
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&answer.body) else { return Said::Nothing };
    let yes = |k: &str| v.get(k).and_then(serde_json::Value::as_bool).unwrap_or(false);
    if yes("draft") || yes("prerelease") {
        return Said::Nothing;
    }
    let Some(tag) = v.get("tag_name").and_then(serde_json::Value::as_str) else { return Said::Nothing };
    // A tag that goes into a page's address as it is: nothing in it but
    // what a version is written with.
    let plain = !tag.is_empty() && tag.len() <= 64 && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b".-+_".contains(&b));
    match Version::parse(tag) {
        Some(version) if plain && version.pre.is_none() => {
            let etag = answer.header("etag").filter(|e| e.len() <= 200).map(str::to_string);
            Said::Latest(Release { version, tag: tag.to_string() }, etag)
        }
        _ => Said::Nothing,
    }
}

/// An answer to come.
type Asked = Receiver<Result<Answer, String>>;

/// How a question is put: [`LATEST`], over the app's HTTP, in the app;
/// in a test, whatever the test says. Given what to send and how to wake
/// the app.
pub type Asker = Box<dyn FnMut(Ask, Box<dyn Fn() + Send>) -> Option<Asked>>;

/// The asking, now and then, and what it found.
pub struct Updates {
    ask: Option<Asker>,
    asking: Option<Asked>,
    /// When it's next asked; `None` at once.
    next: Option<Instant>,
    etag: Option<String>,
    /// The latest release, as last said.
    latest: Option<Release>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Updates {
    /// Asking GitHub, over the app's HTTP.
    pub fn github() -> Self {
        let ask: Asker = Box::new(|with, wake| carnet_sync::http::ask_with(LATEST.to_string(), None, with, wake).ok());
        Self::with(Some(ask))
    }

    /// Asking as `ask` does; never, with none (the tests' apps).
    pub fn with(ask: Option<Asker>) -> Self {
        Updates { ask, asking: None, next: None, etag: None, latest: None, wake: None }
    }

    /// How the app is woken when an answer comes.
    pub fn set_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.wake = Some(Arc::new(wake));
    }

    /// Its turn: asked, if it's time and checking is `on`; an answer
    /// taken. A release newer than this build, the first time it's said.
    pub fn tick(&mut self, on: bool, now: Instant) -> Option<Release> {
        let mut found = None;
        if let Some(rx) = &self.asking
            && let Ok(answer) = rx.try_recv()
        {
            self.asking = None;
            let before = self.newer().cloned();
            self.next = Some(now + if answer.is_ok() { EVERY } else { RETRY });
            if let Ok(Said::Latest(release, etag)) = answer.as_ref().map(read) {
                self.latest = Some(release);
                // One without an ETag is asked for whole next time.
                self.etag = etag;
            }
            found = self.newer().filter(|n| before.as_ref() != Some(*n)).cloned();
        }
        if on && self.asking.is_none() && self.next.is_none_or(|t| now >= t) {
            self.asking = self.ask_now();
            if self.asking.is_none() {
                self.next = Some(now + RETRY);
            }
        }
        found
    }

    fn ask_now(&mut self) -> Option<Asked> {
        let ask = self.ask.as_mut()?;
        let mut send = vec![
            ("Accept".to_string(), "application/vnd.github+json".to_string()),
            ("X-GitHub-Api-Version".to_string(), "2022-11-28".to_string()),
        ];
        // The ETag only with the release it came with: a `304` is no use
        // without it.
        if let (Some(etag), Some(_)) = (&self.etag, &self.latest) {
            send.push(("If-None-Match".to_string(), etag.clone()));
        }
        let wake = self.wake.clone();
        ask(
            Ask { send, keep: vec!["etag".into()] },
            Box::new(move || {
                if let Some(w) = &wake {
                    w();
                }
            }),
        )
    }

    /// How long until it's next asked, if checking is `on` and nothing is
    /// being asked: for the frame clock.
    pub fn due(&self, on: bool, now: Instant) -> Option<Duration> {
        if !on || self.asking.is_some() || self.ask.is_none() {
            return None;
        }
        Some(self.next.map_or(Duration::ZERO, |t| t.saturating_duration_since(now)))
    }

    /// The latest release, if it's newer than this build.
    pub fn newer(&self) -> Option<&Release> {
        self.latest.as_ref().filter(|r| r.version > Version::this())
    }

    /// Whether GitHub has answered with a release.
    pub fn checked(&self) -> bool {
        self.latest.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn versions_compare_as_semver_says() {
        assert_eq!(v("v0.1.2"), Version { major: 0, minor: 1, patch: 2, pre: None });
        assert_eq!(v("V1.0.0+build.5").to_string(), "1.0.0");
        assert!(v("0.1.2") > v("0.1.1"));
        assert!(v("0.2.0") > v("0.1.9"));
        assert!(v("0.1.10") > v("0.1.9"), "numbers, not letters");
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("0.1.1").cmp(&v("v0.1.1")), Ordering::Equal);
        assert!(v("0.2.0") > v("0.2.0-rc.1"), "a release after its prerelease");
        assert!(v("0.2.0-rc.2") > v("0.2.0-rc.1"));
        assert!(v("0.2.0-rc.10") > v("0.2.0-rc.9"));
        assert!(v("0.2.0-beta") > v("0.2.0-alpha.1"));
        assert!(v("0.2.0-alpha.1") > v("0.2.0-alpha"));
        assert!(v("0.2.0-alpha.beta") > v("0.2.0-alpha.1"), "words after numbers");
        for bad in ["", "0.1", "0.1.2.3", "x.1.2", "0.1.-2", "0.1.2-", "latest", "0..1"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Version::this().to_string(), env!("CARGO_PKG_VERSION"));
    }

    fn ok(status: u16, body: &str, etag: Option<&str>) -> Answer {
        Answer { status, body: body.as_bytes().to_vec(), headers: etag.map(|e| ("etag".to_string(), e.to_string())).into_iter().collect() }
    }

    /// GitHub's answer for a release, trimmed.
    const FIXTURE: &str = include_str!("../assets/fixtures/github-release.json");

    #[test]
    fn githubs_answer_is_read_for_its_release() {
        let said = read(&ok(200, FIXTURE, Some("W/\"5f1\"")));
        let Said::Latest(r, etag) = said else { panic!("{said:?}") };
        assert_eq!((r.version.to_string(), r.tag.as_str(), etag.as_deref()), ("0.1.2".to_string(), "v0.1.2", Some("W/\"5f1\"")));
        assert_eq!(r.page(), "https://github.com/qxuken/carnet/releases/tag/v0.1.2");
        assert_eq!(read(&ok(304, "", None)), Said::Same);
        assert_eq!(read(&ok(404, r#"{"message":"Not Found"}"#, None)), Said::Nothing, "no release yet");
        assert_eq!(read(&ok(403, r#"{"message":"API rate limit exceeded"}"#, None)), Said::Nothing);
        assert_eq!(read(&ok(200, "<html>", None)), Said::Nothing);
        let with = |k: &str, to: &str| FIXTURE.replacen(&format!("\"{k}\": false"), &format!("\"{k}\": {to}"), 1);
        assert_eq!(read(&ok(200, &with("draft", "true"), None)), Said::Nothing);
        assert_eq!(read(&ok(200, &with("prerelease", "true"), None)), Said::Nothing);
        let tag = |t: &str| read(&ok(200, &release(t), None));
        assert!(matches!(tag("0.1.3"), Said::Latest(r, _) if r.page().ends_with("/tag/0.1.3")));
        assert_eq!(tag("v0.2.0-rc.1"), Said::Nothing, "a prerelease by its tag");
        assert_eq!(tag("nightly"), Said::Nothing);
        assert_eq!(tag("v1.0.0/../../evil"), Said::Nothing, "a tag that would lead elsewhere");
    }

    /// A fake GitHub: each question kept, with what it sent; answered when
    /// the test says.
    #[derive(Default)]
    struct Fake {
        asked: Vec<(Ask, mpsc::Sender<Result<Answer, String>>)>,
    }

    fn updates() -> (Updates, Rc<RefCell<Fake>>) {
        let fake = Rc::new(RefCell::new(Fake::default()));
        let f = fake.clone();
        let ask: Asker = Box::new(move |with, _wake| {
            let (tx, rx) = mpsc::channel();
            f.borrow_mut().asked.push((with, tx));
            Some(rx)
        });
        (Updates::with(Some(ask)), fake)
    }

    fn answer(fake: &Rc<RefCell<Fake>>, a: Result<Answer, String>) {
        fake.borrow().asked.last().unwrap().1.send(a).unwrap();
    }

    fn release(tag: &str) -> String {
        FIXTURE.replace("\"v0.1.2\"", &format!("\"{tag}\""))
    }

    fn sent(fake: &Rc<RefCell<Fake>>, n: usize, header: &str) -> Option<String> {
        fake.borrow().asked[n].0.send.iter().find(|(k, _)| k == header).map(|(_, v)| v.clone())
    }

    #[test]
    fn asked_at_start_then_once_a_day_and_an_hour_after_no_answer() {
        let (mut u, fake) = updates();
        let t0 = Instant::now();
        let asked = |f: &Rc<RefCell<Fake>>| f.borrow().asked.len();
        assert_eq!(u.due(true, t0), Some(Duration::ZERO), "at once");
        assert_eq!(u.tick(true, t0), None);
        assert_eq!(asked(&fake), 1, "at start");
        assert_eq!(sent(&fake, 0, "If-None-Match"), None, "no ETag yet");
        assert_eq!(sent(&fake, 0, "Accept").as_deref(), Some("application/vnd.github+json"));
        u.tick(true, t0 + Duration::from_secs(5));
        assert_eq!(asked(&fake), 1, "not again while asking");
        assert_eq!(u.due(true, t0), None);

        // A newer release than this build: found, once.
        answer(&fake, Ok(ok(200, &release("v999.0.0"), Some("\"e1\""))));
        let t1 = t0 + Duration::from_secs(10);
        let found = u.tick(true, t1).expect("found");
        assert_eq!(found.version.to_string(), "999.0.0");
        assert_eq!(u.newer().map(|r| r.tag.as_str()), Some("v999.0.0"));
        assert_eq!(u.due(true, t1), Some(EVERY));
        u.tick(true, t1 + EVERY - Duration::from_secs(1));
        assert_eq!(asked(&fake), 1, "not before a day");

        // A day on: asked with the ETag; the same, said shortly.
        u.tick(true, t1 + EVERY);
        assert_eq!(asked(&fake), 2);
        assert_eq!(sent(&fake, 1, "If-None-Match").as_deref(), Some("\"e1\""));
        answer(&fake, Ok(ok(304, "", None)));
        let t2 = t1 + EVERY + Duration::from_secs(1);
        assert_eq!(u.tick(true, t2), None, "said once only");
        assert!(u.newer().is_some(), "still out");

        // No answer at all (offline): asked again an hour on, not before.
        u.tick(true, t2 + EVERY);
        answer(&fake, Err("no such domain: api.github.com".into()));
        let t3 = t2 + EVERY + Duration::from_secs(1);
        assert_eq!(u.tick(true, t3), None, "said nowhere");
        assert_eq!(u.due(true, t3), Some(RETRY));
        u.tick(true, t3 + RETRY - Duration::from_secs(1));
        assert_eq!(asked(&fake), 3);
        u.tick(true, t3 + RETRY);
        assert_eq!(asked(&fake), 4);
        // An answer that says nothing (rate limited) waits a day.
        answer(&fake, Ok(ok(403, "{}", None)));
        let t4 = t3 + RETRY + Duration::from_secs(1);
        u.tick(true, t4);
        assert_eq!(u.due(true, t4), Some(EVERY));
        assert!(u.newer().is_some(), "what was found stays");
    }

    #[test]
    fn off_nothing_is_asked_and_on_again_it_asks_when_due() {
        let (mut u, fake) = updates();
        let t0 = Instant::now();
        assert_eq!(u.tick(false, t0), None);
        assert_eq!(u.due(false, t0), None);
        assert!(fake.borrow().asked.is_empty(), "off: never asked");
        u.tick(true, t0 + Duration::from_secs(60));
        assert_eq!(fake.borrow().asked.len(), 1, "on: asked at once");
        // This build's own version: nothing to say.
        answer(&fake, Ok(ok(200, &release(&format!("v{}", env!("CARGO_PKG_VERSION"))), None)));
        assert_eq!(u.tick(true, t0 + Duration::from_secs(61)), None);
        assert!(u.checked() && u.newer().is_none());
        // Off and on again within the day: not asked again.
        u.tick(false, t0 + Duration::from_secs(120));
        u.tick(true, t0 + Duration::from_secs(180));
        assert_eq!(fake.borrow().asked.len(), 1);
        // The tests' apps never ask.
        let mut none = Updates::with(None);
        assert_eq!((none.tick(true, t0), none.due(true, t0)), (None, None));
    }
}
