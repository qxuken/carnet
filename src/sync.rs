//! Sync on the desktop (docs/SYNC.md §5, §7): off until it is set up in
//! Settings → Sync, and never a word about it otherwise.
//!
//! Three ways in: an invite (a new account, this its first device), a
//! pairing code from a device already in, or the recovery words. Then the
//! session runs here, on the app's own thread, beside the notes; a
//! [`Link`] keeps the socket on a thread of its own and wakes the app
//! when bytes come. Each tick, what came in goes to the session, notes
//! changed on another device go to the store, and notes changed here go
//! out.
//!
//! What is kept, and where:
//! - `sync.txt` beside the settings: the server, the account, this
//!   device's id, the feed's cursor. Nothing secret.
//! - The OS's keychain (Keychain, Credential Manager, Secret Service):
//!   this device's signing key and the vault key. On Linux, where a
//!   desktop may run no Secret Service, a file only the user can read,
//!   beside `sync.txt`, and Settings → Sync says so. On macOS and Windows
//!   a keychain that won't keep them is an error, never a file.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use carnet_sync::Key;
use carnet_sync::compat::Fit;
pub use carnet_sync::compat::UPDATE_SOON;
use carnet_sync::crypto::{BlobId, DeviceKey, OsRandom, Random, RecoveryCode, Ring};
use carnet_sync::invite::InviteCode;
use carnet_sync::http::Http;
use carnet_sync::net::{Link, NetEvent, sync_url};
use carnet_sync::pair::{Admitted, Enroller, Joiner, Joining, PairCode, PasswordJoiner, Recoverer};
use carnet_sync::session::{self, DeviceInfo, Event, Identity, PasskeyInfo, Session, Usage};
use carnet_sync::vault;
use carnet_sync::wire::{AccountRow, InviteRow, LogRow, RoomRow};

use crate::pictures::Pictures;
use crate::store::{Forgotten, Remote, Store};

/// Where a device's keys are kept.
pub trait Secrets: Send + std::marker::Sync {
    fn get(&self, name: &str) -> Option<Vec<u8>>;
    fn set(&self, name: &str, bytes: &[u8]) -> io::Result<()>;
    fn delete(&self, name: &str);
    /// Whether the keys were last read from, or written to, a file rather
    /// than a keychain.
    fn in_file(&self) -> bool {
        false
    }
}

/// Whether a key may be kept in a file when the keychain won't take it:
/// only on Linux, where a desktop may run no Secret Service. macOS and
/// Windows always have one.
const FILE_FALLBACK: bool = cfg!(target_os = "linux");

/// The OS's keychain, or, on Linux where there is none to use, a file in
/// `dir` only the user can read.
pub struct Keychain {
    dir: PathBuf,
    in_file: std::sync::atomic::AtomicBool,
}

impl Keychain {
    pub fn new(dir: PathBuf) -> Self {
        Keychain { dir, in_file: Default::default() }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.key"))
    }

    /// The keychain item, named for the settings folder as well as the
    /// secret: a second Carnet with settings of its own (`CARNET_CONFIG`)
    /// is a second device, and must neither read nor delete the first's
    /// keys.
    fn entry(&self, name: &str) -> Option<keyring::Entry> {
        keyring::Entry::new("Carnet", &format!("{name} {}", self.dir.display())).ok()
    }
}

impl Secrets for Keychain {
    /// The file first: a write to the keychain removes it, so a file
    /// there is the newest copy, and an older one left in the keychain
    /// can't hide it. On macOS and Windows a file is only ever an earlier
    /// Carnet's, and goes into the keychain as it's read.
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        let file = self.file(name);
        if let Ok(b) = std::fs::read(&file) {
            let moved = !FILE_FALLBACK && self.entry(name).is_some_and(|e| e.set_secret(&b).is_ok()) && std::fs::remove_file(&file).is_ok();
            self.in_file.store(!moved, std::sync::atomic::Ordering::Relaxed);
            return Some(b);
        }
        self.in_file.store(false, std::sync::atomic::Ordering::Relaxed);
        self.entry(name).and_then(|e| e.get_secret().ok())
    }

    fn set(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let kept = match self.entry(name) {
            Some(e) => e.set_secret(bytes).map_err(|e| e.to_string()),
            None => Err("there is no keychain to use".into()),
        };
        let file = self.file(name);
        match kept {
            Ok(()) => {
                // An older copy left in a file would be read first.
                match std::fs::remove_file(&file) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
                self.in_file.store(false, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            }
            Err(_) if FILE_FALLBACK => {
                std::fs::create_dir_all(&self.dir)?;
                write_private(&file, bytes)?;
                self.in_file.store(true, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            }
            Err(e) => Err(io::Error::other(format!("the keychain wouldn't keep them ({e})"))),
        }
    }

    fn delete(&self, name: &str) {
        if let Some(e) = self.entry(name) {
            let _ = e.delete_credential();
        }
        let _ = std::fs::remove_file(self.file(name));
        self.in_file.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    fn in_file(&self) -> bool {
        self.in_file.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// A file only its owner can read (on Unix: 0600 from the first byte).
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(bytes)
}

/// Secrets in memory: the tests', and the app's before main says where.
#[derive(Default)]
pub struct InMemory(std::sync::Mutex<HashMap<String, Vec<u8>>>);

impl Secrets for InMemory {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        self.0.lock().ok()?.get(name).cloned()
    }
    fn set(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.0.lock().map_err(|_| io::Error::other("poisoned"))?.insert(name.into(), bytes.to_vec());
        Ok(())
    }
    fn delete(&self, name: &str) {
        if let Ok(mut m) = self.0.lock() {
            m.remove(name);
        }
    }
}

/// What `sync.txt` says: nothing secret.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub origin: String,
    pub account: Vec<u8>,
    pub device: Vec<u8>,
    pub cursor: u64,
    /// What this device is called in the others' lists, if chosen; the
    /// system's name for it if not.
    pub name: Option<String>,
}

/// A name as one line of `sync.txt`, and not too long for a card.
fn one_line(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_NAME).collect()
}

/// The longest name a device may be given.
pub const MAX_NAME: usize = 64;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

impl Config {
    pub fn text(&self) -> String {
        format!(
            "# Carnet sync: which server, which account, this device. The keys are in the system's keychain.\n\
             origin = {}\naccount = {}\ndevice = {}\ncursor = {}\n",
            self.origin,
            hex(&self.account),
            hex(&self.device),
            self.cursor
        ) + &self.name.as_deref().map(|n| format!("name = {}\n", one_line(n))).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut v: HashMap<&str, &str> = HashMap::new();
        for line in text.lines() {
            if let Some((k, x)) = line.split_once('=') {
                v.insert(k.trim(), x.trim());
            }
        }
        Some(Config {
            origin: v.get("origin")?.to_string(),
            account: unhex(v.get("account")?)?,
            device: unhex(v.get("device")?)?,
            cursor: v.get("cursor").and_then(|c| c.parse().ok()).unwrap_or(0),
            name: v.get("name").map(|n| n.to_string()).filter(|n| !n.is_empty()),
        })
    }
}

/// This device's keys, as the keychain keeps them: signing key, vault
/// key, epoch.
struct Keys {
    key: DeviceKey,
    ring: Ring,
}

impl Keys {
    /// The signing key's secret, then the ring: each epoch, each key.
    fn bytes(&self) -> Vec<u8> {
        [&self.key.secret()[..], &self.ring.to_bytes()[..]].concat()
    }

    fn from(b: &[u8]) -> Option<Self> {
        let (key, ring) = b.split_at_checked(32)?;
        Some(Keys { key: DeviceKey::from_secret(key.try_into().ok()?), ring: Ring::from_bytes(ring)? })
    }
}

/// What this device is called in the others' lists: the computer's own
/// name, as the system settings show it.
pub fn device_name() -> String {
    let asked = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let name = if cfg!(target_os = "macos") {
        asked("scutil", &["--get", "ComputerName"])
    } else if cfg!(windows) {
        std::env::var("COMPUTERNAME").ok()
    } else {
        std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).or_else(|| asked("hostname", &[]))
    };
    name.unwrap_or_else(|| "This computer".into())
}

/// The keychain entry's name.
const SECRET: &str = "sync";

/// Said in Settings → Sync while the keys are in a file (Linux with no
/// Secret Service).
pub const KEYS_IN_FILE: &str = "This computer has no keychain running, so sync's keys are kept in a file only you can read.";

/// How a device comes in.
#[derive(Clone, Debug, PartialEq)]
pub enum How {
    /// A code, told apart by its shape: an invite (`K7QF-2M9D-XW4H-P3TA`)
    /// makes a new account; a pairing code (`47-381095`), from a device
    /// already in, joins its account.
    Code(String),
    /// The recovery words.
    Recover(String),
    /// An email and password, on a server that takes them.
    Password { email: String, password: String },
}

enum Starter {
    Enroll(Enroller),
    Join(Joiner),
    Recover(Recoverer),
    // Its OPAQUE state is large; the others are not.
    Password(Box<PasswordJoiner>),
}

impl Starter {
    fn state(&self) -> &Joining {
        match self {
            Starter::Enroll(e) => &e.state,
            Starter::Join(j) => &j.state,
            Starter::Recover(r) => &r.state,
            Starter::Password(p) => p.state(),
        }
    }

    fn take_out(&mut self) -> Vec<Vec<u8>> {
        match self {
            Starter::Enroll(e) => e.take_out(),
            Starter::Join(j) => j.take_out(),
            Starter::Recover(r) => r.take_out(),
            Starter::Password(p) => p.take_out(),
        }
    }

    fn receive(&mut self, b: &[u8]) {
        match self {
            Starter::Enroll(e) => e.receive(b),
            Starter::Join(j) => j.receive(b, &mut OsRandom),
            Starter::Recover(r) => r.receive(b),
            Starter::Password(p) => p.receive(b, &mut OsRandom),
        }
    }
}

// One of it, ever: boxing a variant would save nothing.
#[allow(clippy::large_enum_variant)]
enum Phase {
    Off,
    Starting { origin: String, device: Vec<u8>, key: DeviceKey, link: Link, starter: Starter },
    /// Set up, the keys being read from the keychain on a thread of their
    /// own: the OS may ask the user first (a new build, an update), and
    /// the window must not wait for that.
    Unlocking { config: Config, keys: std::sync::mpsc::Receiver<Option<Keys>> },
    On { config: Config, session: Session, link: Option<Link> },
}

/// What sync is doing, as Settings → Sync says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Off,
    /// Setting up: an invite, a code, the words, being tried.
    Starting,
    /// Set up; this device's keys awaited from the keychain.
    Unlocking,
    Connecting,
    Online,
    Offline(String),
}

type Waker = Arc<dyn Fn() + Send + std::marker::Sync>;

/// What the link says when asked to connect again (net.rs).
const RECONNECTING: &str = "reconnecting";

/// A dropped connection in plain words: what the network said (a refused
/// connection, a name not found, a timeout) is no business of the line
/// that says sync is offline. A certificate that isn't trusted is said,
/// since trying again won't mend it.
pub fn offline_words(why: &str) -> &'static str {
    if why.to_lowercase().contains("certificate") {
        "The server's certificate isn't trusted, so Carnet won't connect."
    } else {
        "Can't reach the server. Trying again\u{2026}"
    }
}

/// The socket's URL for `origin`, saying which app this is: the desktop,
/// on which system, and this build's version, for the server's counts.
fn socket_url(origin: &str) -> Option<String> {
    sync_url(origin, &format!("desktop-{}", std::env::consts::OS), env!("CARGO_PKG_VERSION"))
}

/// How often moves made here are looked for, to go to the vault.
const LAYOUT_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The notes on the server are asked for again this long after the
/// account's notes last changed, while the inspector shows them: once a
/// burst of saves is over, not at each.
const INSPECT_QUIET: std::time::Duration = std::time::Duration::from_secs(2);

pub struct Sync {
    phase: Phase,
    /// Where `sync.txt` is; `None`: kept nowhere (the tests').
    config_path: Option<PathBuf>,
    secrets: Arc<dyn Secrets>,
    wake: Option<Waker>,
    /// The same, for a thread started before it was known (the keychain's,
    /// at load): it looks here when it's done.
    wake_later: Arc<std::sync::Mutex<Option<Waker>>>,
    pub status: Status,
    /// Offline because the connection dropped: what the network said, in
    /// its own terms — for a tooltip, never the line itself.
    pub offline_detail: Option<String>,
    /// The last thing that went wrong, for the settings to say.
    pub error: Option<String>,
    /// A pairing code being shown.
    pub code: Option<String>,
    /// Recovery words just made, shown once, when the server has said it
    /// keeps them.
    pub words: Option<String>,
    /// Recovery words made, the server not yet saying it keeps them: shown
    /// only then, so words it never took are never written down.
    words_waiting: Option<String>,
    /// A device just let in by pairing, for a notice.
    pub paired: Option<String>,
    /// The last device let in by a code shown here, for Settings to say
    /// so where the code was, until another is shown.
    pub joined: Option<String>,
    /// The account's devices, as last listed; `None` until asked and
    /// told, and again when it may have changed.
    pub devices: Option<Vec<DeviceInfo>>,
    /// Settings shows the devices: list them when they aren't.
    pub want_devices: bool,
    asked_devices: bool,
    /// The devices (and what the account keeps) to be asked for again,
    /// though listed: Settings opened, so what it shows is fresh.
    devices_due: bool,
    /// The account can't write for now ([`Event::ReadOnly`], until
    /// [`Event::Writable`]): Settings says so, once, in the account's card.
    pub read_only: bool,
    /// What the account keeps on the server, and may (a hosted plan's
    /// limit), as last asked: with the devices.
    pub usage: Option<Usage>,
    /// With the devices: whether the server takes passwords, and the email
    /// the account's password signs in as, if it has one.
    pub password: Option<(bool, Option<String>)>,
    /// With the devices: the only email a password may sign in as on this
    /// server, if it holds them to one (a hosted service: the email the
    /// account signed up with). Settings shows it instead of a field.
    pub password_email: Option<String>,
    /// A password was set (or taken away) since Settings last looked.
    pub password_set: bool,
    /// The account's deletion asked from here: the server's goodbye says
    /// so.
    deleting: bool,
    /// With the devices: the account's passkeys, made in a browser (this
    /// app can't make one, but lists and removes them).
    pub passkeys: Vec<PasskeyInfo>,
    /// The server will stop taking this app soon (`compat`): Settings
    /// asks for an update.
    pub update_soon: bool,
    asked_usage: bool,
    /// Whether this account is the server's owner's, and if so its
    /// invites and accounts (§6), as last listed; `None` until asked and
    /// told, and again when they may have changed.
    pub invites: Option<Invites>,
    /// Settings shows them: list them when they aren't.
    pub want_invites: bool,
    asked_invites: bool,
    /// The invite just made, shown until put away.
    pub invite: Option<String>,
    /// The server's rooms and log, as last inspected (§9).
    pub inspection: Option<(Vec<RoomRow>, Vec<LogRow>)>,
    /// When the server said them.
    pub inspected_at: Option<std::time::SystemTime>,
    /// The inspector is open: inspect when there's nothing to show, and
    /// again once the account's notes have changed since.
    pub want_inspection: bool,
    asked_inspection: bool,
    /// The session's [`Session::moves`] when the inspection shown was
    /// asked for, and when the one on its way was.
    inspected_moves: u64,
    asked_moves: u64,
    /// The same for what the account keeps ([`Sync::usage`]).
    usage_moves: u64,
    /// The session's moves as last seen, and when they last grew.
    last_moves: (u64, std::time::Instant),
    /// Something happened since the notes' places and the pictures were
    /// last compared with the vault: they are to be again, soon, though
    /// nothing else wakes the app ([`Sync::due`]).
    layout_due: bool,
    /// The server this device was set up with, when its keys couldn't be
    /// read: Settings offers it again.
    pub last_origin: Option<String>,
    /// When the notes' places were last compared with the vault's.
    last_layout: std::time::Instant,
    /// Pictures between devices: made when sync first runs here.
    pictures: Option<Pictures>,
    /// Things gone the server is still to be told of: made when sync
    /// first runs here.
    pending: Option<Pending>,
    /// What this device is called, in the others' lists: asked of the
    /// system when first needed, or set.
    name: Option<String>,
    /// Pictures' HTTP, to and from the links the server gives: started
    /// with the socket.
    http: Option<Http>,
    saved_cursor: u64,
}

/// What Settings says while the account can't write.
pub const READ_ONLY: &str = "Read only for now: what's written here stays here, and syncs once the account can write again.";

/// What the server says of invites (§6): whether this account is the
/// owner's; if so, the invites waiting and every account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invites {
    pub owner: bool,
    pub waiting: Vec<InviteRow>,
    pub accounts: Vec<AccountRow>,
}

impl Sync {
    /// Off, keeping nothing anywhere: until main says where.
    pub fn off() -> Self {
        Sync {
            phase: Phase::Off,
            config_path: None,
            secrets: Arc::new(InMemory::default()),
            wake: None,
            wake_later: Arc::default(),
            status: Status::Off,
            offline_detail: None,
            error: None,
            code: None,
            words: None,
            words_waiting: None,
            paired: None,
            joined: None,
            devices: None,
            want_devices: false,
            asked_devices: false,
            devices_due: false,
            read_only: false,
            usage: None,
            asked_usage: false,
            password: None,
            password_email: None,
            password_set: false,
            deleting: false,
            passkeys: Vec::new(),
            update_soon: false,
            invites: None,
            want_invites: false,
            asked_invites: false,
            invite: None,
            inspection: None,
            inspected_at: None,
            want_inspection: false,
            asked_inspection: false,
            inspected_moves: 0,
            asked_moves: 0,
            usage_moves: 0,
            last_moves: (0, std::time::Instant::now()),
            layout_due: false,
            name: None,
            last_origin: None,
            last_layout: std::time::Instant::now(),
            pictures: None,
            pending: None,
            http: None,
            saved_cursor: 0,
        }
    }

    /// What this device is called, in the others' lists (on its next
    /// login).
    #[cfg(test)]
    pub fn set_name(&mut self, name: String) {
        self.name = Some(name);
    }

    /// A passkey added as a browser adds one: the app itself can't make
    /// one.
    #[cfg(test)]
    pub fn add_passkey_for_test(&mut self, name: &str, credential: &[u8], prf: &[u8]) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            session.add_passkey(name, credential, prf, &mut OsRandom).unwrap();
            flush(session, link.as_ref());
        }
    }

    fn name(&mut self) -> String {
        self.name.get_or_insert_with(device_name).clone()
    }

    /// As it was left: on, if `config_path` and the secrets both say so.
    ///
    /// The keys are read on a thread: until they come, sync is
    /// [`Status::Unlocking`], and the app goes on without it.
    pub fn load(config_path: PathBuf, secrets: Arc<dyn Secrets>) -> Self {
        let mut s = Sync { config_path: Some(config_path.clone()), secrets, ..Sync::off() };
        let Some(config) = std::fs::read_to_string(&config_path).ok().and_then(|t| Config::parse(&t)) else { return s };
        let (tx, rx) = std::sync::mpsc::channel();
        let secrets = s.secrets.clone();
        let wake = s.wake_later.clone();
        let started = std::thread::Builder::new().name("carnet-keychain".into()).spawn(move || {
            let _ = tx.send(secrets.get(SECRET).and_then(|b| Keys::from(&b)));
            if let Some(w) = wake.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                w();
            }
        });
        if started.is_ok() {
            s.phase = Phase::Unlocking { config, keys: rx };
            s.status = Status::Unlocking;
        }
        s
    }

    /// The keys came from the keychain, or didn't.
    fn unlocked(&mut self, config: Config, keys: Option<Keys>) {
        if config.name.is_some() {
            self.name = config.name.clone();
        }
        match keys {
            Some(keys) => {
                self.saved_cursor = config.cursor;
                let name = self.name();
                let session = Session::new(identity(&config, keys, name), config.cursor);
                self.phase = Phase::On { config, session, link: None };
                self.status = Status::Connecting;
            }
            // `sync.txt` is kept: the next start asks the keychain again.
            None => {
                self.last_origin = Some(config.origin.clone());
                self.phase = Phase::Off;
                self.status = Status::Off;
                self.error = Some(
                    "This device's sync keys couldn't be read from the keychain (was access denied?). Sync is off until Carnet starts again and is allowed, or is set up anew."
                        .into(),
                );
            }
        }
    }

    /// Whether this device's keys are in a file, not a keychain: Linux
    /// with no Secret Service (Settings → Sync says so).
    pub fn keys_in_file(&self) -> bool {
        self.is_on() && self.secrets.in_file()
    }

    pub fn is_on(&self) -> bool {
        matches!(self.phase, Phase::On { .. })
    }

    /// The server's origin, once on.
    pub fn origin(&self) -> Option<&str> {
        match &self.phase {
            Phase::On { config, .. } => Some(&config.origin),
            Phase::Starting { origin, .. } => Some(origin),
            Phase::Unlocking { config, .. } => Some(&config.origin),
            Phase::Off => None,
        }
    }

    /// The account's id, short, once on.
    /// This device's id, while sync is on.
    pub fn device(&self) -> Option<&[u8]> {
        match &self.phase {
            Phase::On { config, .. } => Some(&config.device),
            _ => None,
        }
    }

    /// Whether the server said it can do `feature` (`compat::feature`),
    /// as it greeted this device last.
    pub fn server_has(&self, feature: &str) -> bool {
        match &self.phase {
            Phase::On { session, .. } => session.server_has(feature),
            _ => false,
        }
    }

    pub fn account(&self) -> Option<String> {
        match &self.phase {
            Phase::On { config, .. } => Some(hex(&config.account[..config.account.len().min(4)])),
            _ => None,
        }
    }

    /// How the app is woken when bytes come. The socket starts with it.
    pub fn set_wake(&mut self, wake: impl Fn() + Send + std::marker::Sync + 'static) {
        let wake: Waker = Arc::new(wake);
        *self.wake_later.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake.clone());
        self.wake = Some(wake);
    }

    fn link(&self, origin: &str) -> io::Result<Link> {
        let url = socket_url(origin).ok_or_else(|| io::Error::other("A server's address starts with https:// (or http://)"))?;
        let wake = self.wake.clone();
        Link::start(url, move || {
            if let Some(w) = &wake {
                w();
            }
        })
    }

    /// Sets up: `origin` is the server (`https://notes.example.com`), and
    /// `how` the way in.
    pub fn start(&mut self, origin: &str, how: How) {
        self.error = None;
        if origin.trim().is_empty() {
            return self.error = Some("Which server? Its address goes in Server, above".into());
        }
        let Some(origin) = carnet_sync::signup::origin(origin) else {
            return self.error = Some("That isn't a server's address".into());
        };
        let device = OsRandom.bytes::<8>().to_vec();
        let key = DeviceKey::generate(&mut OsRandom);
        let starter = match how {
            How::Code(code) => match (PairCode::parse(&code), InviteCode::parse(&code)) {
                (Some(c), _) => Starter::Join(Joiner::new(c, device.clone(), key.clone())),
                (None, Some(_)) => Starter::Enroll(Enroller::new(code.trim(), device.clone(), key.clone(), &mut OsRandom)),
                (None, None) => {
                    return self.error = Some(
                        "That isn't a code: an invite looks like K7QF-2M9D-XW4H-P3TA, and one from another device like 47-381095".into(),
                    );
                }
            },
            How::Recover(words) => match RecoveryCode::parse(&words) {
                Some(c) => Starter::Recover(Recoverer::new(origin.clone(), c, device.clone(), key.clone())),
                None => return self.error = Some(RecoveryCode::problem(&words)),
            },
            How::Password { email, password } => {
                if !email.contains('@') {
                    return self.error = Some("That doesn't look like an email address".into());
                }
                if password.is_empty() {
                    return self.error = Some("And the password, beside it".into());
                }
                Starter::Password(Box::new(PasswordJoiner::new(origin.clone(), &email, password, device.clone(), key.clone())))
            }
        };
        match self.link(&origin) {
            Ok(link) => {
                self.phase = Phase::Starting { origin, device, key, link, starter };
                self.status = Status::Starting;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Stops setting up.
    pub fn cancel(&mut self) {
        if matches!(self.phase, Phase::Starting { .. }) {
            self.phase = Phase::Off;
            self.status = Status::Off;
        }
    }

    /// Sync off on this device: its keys and `sync.txt` forgotten. The
    /// notes stay, and so does their history.
    /// Off, on this device: taken out of the account first, if the server
    /// can be told (else it stays listed, to be removed from another).
    pub fn turn_off(&mut self) {
        if let Phase::On { config, session, link } = &mut self.phase
            && session.is_ready()
        {
            let me = config.device.clone();
            let _ = session.revoke(&me, &mut OsRandom);
            flush(session, link.as_ref());
        }
        self.forget();
    }

    /// Takes `device` out of the account (§7); the list says when it's
    /// done, or `error` why not.
    pub fn revoke(&mut self, device: &[u8]) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            match session.revoke(device, &mut OsRandom) {
                Ok(()) => flush(session, link.as_ref()),
                Err(e) => self.error = Some(sentence(&e)),
            }
        }
    }

    /// Sets (or changes) the account's password, signing in as `email`;
    /// [`Sync::password_set`] when done, or `error` why not.
    pub fn set_password(&mut self, email: &str, password: &str) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            match session.set_password(email, password, &mut OsRandom) {
                Ok(()) => flush(session, link.as_ref()),
                Err(e) => self.error = Some(sentence(&e)),
            }
        }
    }

    /// The account deleted, everything the server keeps of it, and every
    /// device let go; the notes stay here, as files.
    pub fn delete_account(&mut self) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            self.deleting = true;
            session.delete_account();
            flush(session, link.as_ref());
        }
    }

    /// No password any more.
    pub fn drop_password(&mut self) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            session.drop_password();
            flush(session, link.as_ref());
        }
    }

    /// The passkey `id` names lets no device in any more; the list says
    /// when it's done.
    pub fn remove_passkey(&mut self, id: &[u8; 32]) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            session.remove_passkey(id);
            flush(session, link.as_ref());
        }
    }

    /// A new invite, for someone to make an account of their own on this
    /// server: its code in [`Sync::invite`], and the list again. The server
    /// keeps only its hash; the code is kept in the vault, for every device
    /// of the account to show again ([`Sync::invite_code`]).
    pub fn make_invite(&mut self, store: &mut Store) {
        if let Phase::On { session, link, .. } = &mut self.phase
            && let Some(code) = session.invite(&mut OsRandom)
        {
            // The invite to the server first, then its code to the vault:
            // a device that sees the code then sees the invite listed.
            store.keep_invite(&code, unix_now());
            send_vault(session, store);
            flush(session, link.as_ref());
            self.invite = Some(code.text());
        }
    }

    /// An invite not used yet, taken back: its code forgotten in the vault.
    pub fn drop_invite(&mut self, store: &mut Store, hash: [u8; 32]) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            session.drop_invite(hash);
            store.forget_invite(&hash);
            send_vault(session, store);
            flush(session, link.as_ref());
        }
        if self.invite.as_deref().and_then(InviteCode::parse).is_some_and(|c| c.hash() == hash) {
            self.invite = None;
        }
    }

    /// Another person's account taken off the server, with all it keeps
    /// there.
    pub fn remove_account(&mut self, account: &[u8]) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            session.remove_account(account);
            flush(session, link.as_ref());
        }
    }

    /// The code of an invite this account made, by its hash: kept in the
    /// vault, on whichever of its devices made it. `None` for one made
    /// before Carnet kept them there.
    pub fn invite_code(&self, store: &Store, hash: &[u8; 32]) -> Option<String> {
        store.invite_code(hash)
    }

    /// What this device is called in the others' lists, changed now; a
    /// blank name gives it back the system's. Kept in `sync.txt`.
    pub fn rename(&mut self, name: &str) {
        let chosen = Some(one_line(name)).filter(|n| !n.is_empty());
        self.name = Some(chosen.clone().unwrap_or_else(device_name));
        let name = self.name();
        if let Phase::On { config, session, link } = &mut self.phase {
            config.name = chosen;
            if let Some(p) = &self.config_path {
                let _ = std::fs::write(p, config.text());
            }
            session.rename(&name, &mut OsRandom);
            flush(session, link.as_ref());
        }
    }

    /// Asks the server again how it keeps the rooms: what's shown stays
    /// until the answer comes.
    pub fn refresh_inspection(&mut self) {
        self.asked_inspection = false;
        self.inspected_moves = u64::MAX;
        self.last_moves.1 = std::time::Instant::now() - INSPECT_QUIET;
    }

    /// Whether the inspection shown is out of date: the account's notes
    /// have changed since the server said it (it's asked again shortly).
    pub fn inspection_stale(&self) -> bool {
        match &self.phase {
            Phase::On { session, .. } => self.inspection.is_some() && session.moves() != self.inspected_moves,
            _ => false,
        }
    }

    /// The notes folder may have changed outside sync's sight (another
    /// app, looked at again): the places and pictures are compared with
    /// the vault soon.
    /// Settings opened: the devices it lists, who's online among them, and
    /// what the account keeps, asked for again, the list shown meanwhile.
    /// (A server that tells when a device comes or goes keeps the list
    /// fresh while it stays open, docs/SYNC.md §7.)
    pub fn devices_again(&mut self) {
        self.devices_due = true;
    }

    pub fn poke(&mut self) {
        self.layout_due = true;
    }

    /// How many changes (a note's, or the library's) the server isn't
    /// known to keep yet: offline, read only, or on their way.
    pub fn waiting(&self) -> usize {
        match &self.phase {
            Phase::On { session, .. } => session.waiting(),
            _ => 0,
        }
    }

    /// How many pictures put in here the server hasn't got yet.
    pub fn pictures_waiting(&self) -> usize {
        self.pictures.as_ref().map_or(0, Pictures::waiting)
    }

    /// Logged in, every room joined, everything sent acknowledged.
    #[cfg(test)]
    pub fn is_idle(&self) -> bool {
        matches!(&self.phase, Phase::On { session, .. } if session.is_idle())
    }

    /// Whether the sweep of pictures no note shows may run now, and the
    /// pictures it must leave (being sent, or wanted back): with sync off,
    /// at any time; with it on, once this device has caught up with the
    /// account and no picture is on its way, up or down. `None`: later.
    pub fn sweep_may_run(&self) -> Option<std::collections::BTreeSet<String>> {
        match &self.phase {
            Phase::Off => Some(Default::default()),
            Phase::On { session, .. } if session.is_idle() && session.has_joined(vault::ROOM) => match &self.pictures {
                Some(p) if p.busy() => None,
                Some(p) => Some(p.busy_paths()),
                None => Some(Default::default()),
            },
            _ => None,
        }
    }

    /// How long until sync wants a turn though nothing wakes the app: the
    /// notes' places and pictures compared with the vault after what
    /// happened (a first login, a save, something come), and the notes
    /// on the server asked again once the account is quiet. `None`: only
    /// when woken.
    pub fn due(&self) -> Option<std::time::Duration> {
        let Phase::On { session, .. } = &self.phase else { return None };
        if !session.has_joined(vault::ROOM) {
            return None;
        }
        let mut due = None::<std::time::Duration>;
        let mut at = |d: std::time::Duration| due = Some(due.map_or(d, |e| e.min(d)));
        if self.layout_due {
            at(LAYOUT_EVERY.saturating_sub(self.last_layout.elapsed()));
        }
        let m = session.moves();
        let inspect = self.want_inspection && !self.asked_inspection && self.inspection.is_some() && m != self.inspected_moves;
        let usage = self.want_devices && !self.asked_usage && self.usage.is_some() && m != self.usage_moves;
        if inspect || usage {
            // Quiet long enough, and still waiting for what's in flight:
            // looked at again in a while, not at every frame.
            let wait = INSPECT_QUIET.saturating_sub(self.last_moves.1.elapsed());
            at(if wait.is_zero() { LAYOUT_EVERY } else { wait });
        }
        due
    }

    /// `room`'s base on the server dropped, and what it stood for put back
    /// (§9); the inspector shows it when the server answers.
    pub fn roll_back(&mut self, room: &str) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            session.roll_back(room);
            flush(session, link.as_ref());
        }
    }

    /// This device's copy of `room` made the server's base (§9).
    pub fn repair(&mut self, room: &str) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            session.repair(room);
            flush(session, link.as_ref());
        }
    }

    /// This device's keys and `sync.txt` forgotten; the notes stay.
    fn forget(&mut self) {
        self.secrets.delete(SECRET);
        if let Some(p) = &self.config_path {
            let _ = std::fs::remove_file(p);
        }
        self.phase = Phase::Off;
        self.status = Status::Off;
        self.code = None;
        self.words = None;
        self.devices = None;
        self.read_only = false;
        self.joined = None;
        self.passkeys.clear();
        self.inspection = None;
        self.inspected_at = None;
        self.invites = None;
        self.invite = None;
    }

    /// Opens a pairing: its code comes in [`Sync::code`].
    pub fn pair_open(&mut self) {
        self.joined = None;
        if let Phase::On { session, link, .. } = &mut self.phase {
            session.pair_open(&mut OsRandom);
            flush(session, link.as_ref());
        }
    }

    pub fn pair_cancel(&mut self) {
        if let Phase::On { session, link, .. } = &mut self.phase {
            session.pair_cancel();
            flush(session, link.as_ref());
        }
        self.code = None;
    }

    /// Makes a recovery code: the server keeps the vault key under it, and
    /// once it says so its words are shown once, in [`Sync::words`], never
    /// kept here. It replaces any code before.
    pub fn make_recovery(&mut self) {
        if let Phase::On { session, link, .. } = &mut self.phase
            && session.is_ready()
        {
            let code = RecoveryCode::generate(&mut OsRandom);
            session.set_recovery(&code, &mut OsRandom);
            flush(session, link.as_ref());
            self.words = None;
            self.words_waiting = Some(code.words());
        }
    }

    /// A recovery code made, the server not yet saying it keeps it.
    pub fn making_recovery(&self) -> bool {
        self.words_waiting.is_some()
    }

    /// Whether the account has a recovery code, as far as is known: one
    /// made would replace it. Not known yet: taken to.
    pub fn has_recovery(&self) -> bool {
        match &self.phase {
            Phase::On { session, .. } => session.has_recovery() != Some(false),
            _ => false,
        }
    }

    /// What came, taken in; what is to go, sent. Returns what notes
    /// changed on another device did here.
    pub fn tick(&mut self, store: &mut Store) -> Vec<Remote> {
        let mut changed = Vec::new();
        if let Phase::Unlocking { keys, .. } = &self.phase {
            match keys.try_recv() {
                Ok(k) => {
                    let Phase::Unlocking { config, .. } = std::mem::replace(&mut self.phase, Phase::Off) else { unreachable!() };
                    self.unlocked(config, k);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let Phase::Unlocking { config, .. } = std::mem::replace(&mut self.phase, Phase::Off) else { unreachable!() };
                    self.unlocked(config, None);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return changed,
            }
        }
        match &mut self.phase {
            // Off: what's written here names no device, and what's deleted
            // forever was never anyone else's to tell.
            Phase::Off => {
                if let Some(replica) = store.replica.as_mut() {
                    replica.set_author(None);
                }
                store.take_forgotten();
            }
            Phase::Unlocking { .. } => {}
            Phase::Starting { link, starter, .. } => {
                let mut dropped = None;
                for e in link.poll() {
                    match e {
                        NetEvent::Frame(b) => starter.receive(&b),
                        NetEvent::Disconnected(why) => dropped = Some(why),
                        NetEvent::Connected => {}
                    }
                }
                for b in starter.take_out() {
                    link.send(b);
                }
                match starter.state() {
                    Joining::In(_) => self.admitted(store),
                    Joining::Failed(why) => {
                        self.error = Some(why.clone());
                        self.phase = Phase::Off;
                        self.status = Status::Off;
                    }
                    Joining::Going => {
                        if let Some(why) = dropped {
                            self.error = Some(format!("Couldn't reach the server: {why}"));
                            self.phase = Phase::Off;
                            self.status = Status::Off;
                        }
                    }
                }
            }
            Phase::On { config, session, link } => {
                if link.is_none() && self.wake.is_some() {
                    let url = socket_url(&config.origin).unwrap_or_default();
                    let wake = self.wake.clone();
                    *link = Link::start(url, move || {
                        if let Some(w) = &wake {
                            w();
                        }
                    })
                    .ok();
                }
                let Some(replica) = store.replica.as_mut() else {
                    self.error = Some(store.replica_refused.clone().unwrap_or_else(|| "The notes' history couldn't be opened, and sync needs it".into()));
                    return changed;
                };
                // What's written here names this device in the history.
                replica.set_author(Some(&config.device));
                // Anything at all happening: the places and pictures are
                // compared with the vault again soon ([`Sync::due`]).
                let mut busy = false;
                if let Some(l) = link.as_ref() {
                    for e in l.poll() {
                        busy = true;
                        match e {
                            NetEvent::Connected => {
                                session.connected();
                                self.update_soon = false;
                                self.status = Status::Connecting;
                            }
                            NetEvent::Frame(b) => session.receive(&b, replica, &mut OsRandom),
                            NetEvent::Disconnected(why) => {
                                session.disconnected();
                                // Asked to connect again (a new server
                                // address, a wake): connecting, not out.
                                self.status = if why == RECONNECTING { Status::Connecting } else { Status::Offline(offline_words(&why).into()) };
                                self.offline_detail = Some(why);
                                // Words the server may or may not have
                                // kept: never shown, and said so.
                                if self.words_waiting.take().is_some() {
                                    self.error = Some("The connection dropped before the server said it kept the new recovery code. Make one again once online.".into());
                                }
                            }
                        }
                    }
                }
                // Pictures put or fetched over HTTP: how each went.
                if self.http.is_none() {
                    // The app's wake, whenever it's known.
                    let wake = self.wake_later.clone();
                    self.http = Http::start(Vec::new(), move || {
                        if let Some(w) = wake.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                            w();
                        }
                    })
                    .ok();
                }
                if let Some(http) = &self.http {
                    for d in http.poll() {
                        busy = true;
                        session.transferred(d.id, d.put, d.result);
                    }
                }
                let mut rooms = Vec::new();
                let mut gone = None;
                let mut blobs = Vec::new();
                let mut listed: Option<Vec<[u8; 32]>> = None;
                let events = session.take_events();
                self.layout_due |= busy || !events.is_empty();
                for e in events {
                    match e {
                        Event::LoggedIn => {
                            // A new session: what was offered and not
                            // answered, or held back, is offered again.
                            if let Some(p) = self.pictures.as_mut() {
                                p.retry();
                            }
                            self.status = Status::Online;
                            self.offline_detail = None;
                            self.devices = None;
                            self.asked_devices = false;
                            self.usage = None;
                            self.asked_usage = false;
                            self.invites = None;
                            self.asked_invites = false;
                        }
                        // Taken out of the account, here or on another
                        // device: its keys are no use now.
                        Event::Removed { account } => gone = Some(account),
                        // Too old, the app or the server: nothing syncs
                        // until one is updated; tried again now and then.
                        Event::Outdated(fit) => match fit.refusal() {
                            Some(why) => {
                                self.error = Some(why.into());
                                self.status = Status::Offline(if fit == Fit::AppTooOld { "This version of Carnet is too old for the server.".into() } else { "The server is too old for this version of Carnet.".into() });
                                self.offline_detail = None;
                            }
                            None => self.update_soon = true,
                        },
                        Event::Denied(why) => {
                            self.error = Some(format!("The server turned this device away: {why}"));
                            self.status = Status::Offline("The server turned this device away.".into());
                            self.offline_detail = Some(why);
                        }
                        Event::Devices(mut list) => {
                            // This one first, then by name.
                            list.sort_by(|a, b| b.you.cmp(&a.you).then_with(|| a.name.cmp(&b.name)));
                            self.devices = Some(list);
                            self.asked_devices = false;
                        }
                        // A new epoch's key: kept at once, or a restart
                        // would lose what it opens.
                        Event::Rekeyed(_) => {
                            if let Some(mut keys) = self.secrets.get(SECRET).and_then(|b| Keys::from(&b)) {
                                keys.ring = session.ring().clone();
                                if let Err(e) = self.secrets.set(SECRET, &keys.bytes()) {
                                    self.error = Some(format!("Couldn't keep this device's new key: {e}"));
                                }
                            }
                            self.devices = None;
                        }
                        // Pictures held back: offered again once there's
                        // room. Read only, the card says so already.
                        Event::Refused(why) if why == session::READ_ONLY => {
                            if let Some(p) = self.pictures.as_mut() {
                                p.held_back();
                            }
                        }
                        Event::Refused(why) => {
                            if why == session::FULL
                                && let Some(p) = self.pictures.as_mut()
                            {
                                p.held_back();
                            }
                            self.error = Some(sentence(&session::refusal_words(&why)));
                        }
                        Event::Invites { owner, invites, accounts } => {
                            listed = Some(invites.iter().map(|i| i.hash).collect());
                            self.invites = Some(Invites { owner, waiting: invites, accounts });
                            self.asked_invites = false;
                        }
                        Event::Inspected(rooms, log) => {
                            self.inspection = Some((rooms, log));
                            self.inspected_at = Some(std::time::SystemTime::now());
                            // Unasked, after a roll back or a repair: as of now.
                            self.inspected_moves = if self.asked_inspection { self.asked_moves } else { session.moves() };
                            self.asked_inspection = false;
                        }
                        // Told to the server, which rolls it back; the room
                        // is asked for again. The inspector's log says so.
                        Event::BadSnapshot(_) => self.inspection = None,
                        Event::Changed(room) => rooms.push(room),
                        Event::PairCode(c) => self.code = Some(c),
                        Event::Paired(device) => {
                            self.code = None;
                            self.paired = Some(hex(&device));
                            self.joined = self.paired.clone();
                            self.devices = None;
                            self.asked_devices = false;
                        }
                        Event::PairFailed(why) => {
                            self.code = None;
                            self.error = Some(format!("Pairing ended: {why}"));
                        }
                        // The server keeps the code: its words shown now.
                        Event::RecoverySet => self.words = self.words_waiting.take().or(self.words.take()),
                        Event::Unreadable(_) => {}
                        Event::Password { allowed, id, only } => {
                            self.password = Some((allowed, id));
                            self.password_email = only;
                        }
                        Event::PasswordSet => {
                            self.password_set = true;
                            self.devices = None;
                        }
                        Event::Passkeys(list) => self.passkeys = list,
                        // The session asks for the list again itself.
                        Event::PasskeySet => {}
                        Event::Usage(u) => {
                            // Room again (a plan raised, pictures deleted):
                            // the pictures held back go now, and "full" goes.
                            if u.has_room() {
                                if let Some(p) = self.pictures.as_mut() {
                                    p.retry();
                                }
                                if self.error.as_deref() == Some(session::FULL) {
                                    self.error = None;
                                }
                            }
                            self.usage = Some(u);
                            self.asked_usage = false;
                        }
                        Event::ReadOnly => self.read_only = true,
                        // Writable again: what waited is on its way, and
                        // the plan's line is asked for again.
                        Event::Writable => {
                            self.read_only = false;
                            self.usage = None;
                            self.asked_usage = false;
                        }
                        Event::RoomRemoved(room) => {
                            if let Some(p) = self.pending.as_mut() {
                                p.done_room(&room);
                            }
                            // A room told of and gone before its note came
                            // here: no room of this device's any more.
                            if let (Some(key), Some(r)) = (Key::parse(&room), store.replica.as_mut())
                                && r.entry(key).is_none()
                            {
                                let _ = r.forget(key);
                            }
                        }
                        Event::BlobsDropped(ids) => {
                            if let Some(p) = self.pending.as_mut() {
                                p.done_pictures(&ids);
                            }
                        }
                        e @ (Event::BlobWanted(_) | Event::BlobStored(_) | Event::BlobGot(..) | Event::BlobFailed(_)) => blobs.push(e),
                    }
                }
                if self.want_devices && (self.devices.is_none() || self.devices_due) && !self.asked_devices && session.is_ready() {
                    session.list_devices();
                    self.asked_devices = true;
                    // What the account keeps, with them, as when first asked.
                    if self.devices_due && !self.asked_usage {
                        session.usage();
                        self.asked_usage = true;
                        self.usage_moves = session.moves();
                    }
                    self.devices_due = false;
                }
                if self.want_invites && self.invites.is_none() && !self.asked_invites && session.is_ready() {
                    session.list_invites();
                    self.asked_invites = true;
                }
                // The notes on the server: asked when the inspector opens,
                // and again once the account's notes have changed since and
                // gone quiet, so what it shows is never long out of date.
                let moves = session.moves();
                if moves != self.last_moves.0 {
                    self.last_moves = (moves, std::time::Instant::now());
                }
                let stale = self.inspection.is_some() && moves != self.inspected_moves;
                let quiet = session.is_idle() && self.last_moves.1.elapsed() >= INSPECT_QUIET;
                if self.want_inspection && !self.asked_inspection && session.is_ready() && (self.inspection.is_none() || (stale && quiet)) {
                    session.inspect();
                    self.asked_inspection = true;
                    self.asked_moves = moves;
                }
                // What the account keeps, likewise: "0 bytes kept" isn't
                // left standing once the notes have gone up.
                let usage_stale = self.usage.is_some() && moves != self.usage_moves && quiet;
                if self.want_devices && !self.asked_usage && session.is_ready() && (self.usage.is_none() || usage_stale) {
                    session.usage();
                    self.asked_usage = true;
                    self.usage_moves = moves;
                }
                if let Some(account) = gone {
                    self.forget();
                    let deleted_here = std::mem::take(&mut self.deleting);
                    self.error = Some(if deleted_here {
                        "The account is deleted, with everything the server kept of it. Your notes stay here, as files.".into()
                    } else if account {
                        "This account isn't on the server any more: it was deleted, or removed by the server's owner. Its notes stay here.".into()
                    } else {
                        "This device was removed from the account on another device. Its notes stay here.".into()
                    });
                    return changed;
                }
                // The invites the server lists as waiting: the codes of the
                // rest (used, taken back, run out) forgotten in the vault.
                if let Some(waiting) = listed {
                    store.forget_invites_but(&waiting, unix_now());
                }
                let mut vault_in = false;
                for room in rooms {
                    if room == vault::ROOM {
                        vault_in = true;
                        if let Err(e) = store.vault_changed() {
                            self.error = Some(format!("Couldn't keep where the notes go: {e}"));
                        }
                    } else if let Some(key) = Key::parse(&room) {
                        match store.remote_changed(key) {
                            Ok(r) => changed.push(r),
                            Err(e) => self.error = Some(format!("Couldn't write a note from another device: {e}")),
                        }
                    }
                }
                // Where the notes go (docs/SYNC.md §4): at once when another
                // device moved one, else every second or so for moves here.
                // Nothing of this device's is written before the account's
                // vault has come, or it would overwrite where they are there.
                // Pictures: what the session said of them, then (with the
                // layout) new ones offered and missing ones asked for.
                let pictures = self.pictures.get_or_insert_with(|| Pictures::open(store.replica_dir().as_deref()));
                for e in &blobs {
                    if let Err(err) = pictures.event(e, store, session, &mut OsRandom) {
                        self.error = Some(format!("Couldn't write a picture from another device: {err}"));
                    }
                }
                if vault_in || !changed.is_empty() || self.last_layout.elapsed() >= LAYOUT_EVERY {
                    self.last_layout = std::time::Instant::now();
                    self.layout_due = !session.has_joined(vault::ROOM);
                    match store.reconcile(session.has_joined(vault::ROOM)) {
                        Ok(true) => changed.push(Remote::Layout),
                        Ok(false) => {}
                        Err(e) => self.error = Some(format!("Couldn't move a note as another device did: {e}")),
                    }
                    if session.has_joined(vault::ROOM) {
                        pictures.tick(store, session);
                    }
                }
                if std::mem::take(&mut pictures.arrived) {
                    changed.push(Remote::Pictures);
                }
                // Deleted forever, here or by another device, and pictures
                // put out of use: kept until the server says done, and told.
                let pending = self.pending.get_or_insert_with(|| Pending::open(store.replica_dir().as_deref()));
                pending.add(store.take_forgotten());
                for room in &pending.rooms {
                    session.remove_room(room);
                }
                let ids: Vec<BlobId> = pending.pictures.iter().filter_map(|h| unhex32(h)).collect();
                session.drop_blobs(&ids);
                let keys = store.take_changed();
                self.layout_due |= !keys.is_empty();
                // A note saved (or a version restored) showing a picture
                // dropped since: fetched back while the server keeps it.
                if !keys.is_empty() && store.replica.as_ref().is_some_and(|r| !r.vault().dropped_pictures().is_empty()) {
                    let shown = store.shown_by(&keys);
                    pictures.want_back(store, session, &shown);
                }
                let replica = store.replica.as_mut().expect("checked");
                for key in keys {
                    session.saved(&key.to_string(), replica, &mut OsRandom);
                }
                if store.take_vault_changed() {
                    let replica = store.replica.as_mut().expect("checked");
                    session.saved(vault::ROOM, replica, &mut OsRandom);
                }
                flush(session, link.as_ref());
                // Pictures to put or fetch: over HTTP, by their links.
                if let Some(http) = &self.http {
                    for t in session.take_transfers() {
                        http.send_for(&config.origin, t);
                    }
                }
                if session.cursor() != self.saved_cursor {
                    config.cursor = session.cursor();
                    self.saved_cursor = config.cursor;
                    if let Some(p) = &self.config_path {
                        let _ = std::fs::write(p, config.text());
                    }
                }
            }
        }
        changed
    }

    /// A way in that worked: keys to the keychain, `sync.txt` written, the
    /// session started on the same socket.
    fn admitted(&mut self, store: &mut Store) {
        let Phase::Starting { origin, device, key, link, starter } = std::mem::replace(&mut self.phase, Phase::Off) else { return };
        let joining = !matches!(starter, Starter::Enroll(_));
        let state = match starter {
            Starter::Enroll(e) => e.state,
            Starter::Join(j) => j.state,
            Starter::Recover(r) => r.state,
            Starter::Password(p) => (*p).into_state(),
        };
        let Joining::In(Admitted { account, ring, challenge }) = state else { return };
        let keys = Keys { key, ring };
        if let Err(e) = self.secrets.set(SECRET, &keys.bytes()) {
            self.error = Some(format!("Couldn't keep this device's keys: {e}"));
            self.status = Status::Off;
            return;
        }
        let config = Config { origin, account, device, cursor: 0, name: None };
        if let Some(p) = &self.config_path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, config.text());
        }
        if joining && let Err(e) = store.drop_welcome() {
            self.error = Some(format!("Couldn't remove this device's welcome note: {e}"));
        }
        let name = self.name();
        let mut session = Session::new(identity(&config, keys, name), 0);
        session.connected();
        if let Some(replica) = store.replica.as_mut() {
            session.receive(&challenge, replica, &mut OsRandom);
        }
        flush(&mut session, Some(&link));
        self.saved_cursor = 0;
        self.phase = Phase::On { config, session, link: Some(link) };
        self.status = Status::Connecting;
    }
}

/// A message from below, as a sentence: its first letter up.
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn identity(config: &Config, keys: Keys, name: String) -> Identity {
    Identity {
        origin: config.origin.clone(),
        account: config.account.clone(),
        device: config.device.clone(),
        key: keys.key,
        ring: keys.ring,
        name,
    }
}

/// The vault's changes made here, sent now.
fn send_vault(session: &mut Session, store: &mut Store) {
    if store.take_vault_changed()
        && let Some(replica) = store.replica.as_mut()
    {
        session.saved(vault::ROOM, replica, &mut OsRandom);
    }
}

/// Seconds since 1970, by this device's clock.
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn flush(session: &mut Session, link: Option<&Link>) {
    let out = session.take_out();
    if let Some(l) = link {
        for b in out {
            l.send(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carnet_sync::crypto::VaultKey;

    #[test]
    fn config_and_keys_read_back() {
        let c = Config { origin: "https://n.example".into(), account: vec![1, 2, 255], device: vec![9; 8], cursor: 41, name: None };
        assert_eq!(Config::parse(&c.text()), Some(c));
        let named = Config { name: Some("Work laptop".into()), ..Config { origin: "o".into(), account: vec![1], device: vec![2], cursor: 0, name: None } };
        assert_eq!(Config::parse(&named.text()), Some(named));
        let mut ring = Ring::new(VaultKey::generate(&mut OsRandom));
        ring.insert(3, VaultKey::generate(&mut OsRandom));
        let k = Keys { key: DeviceKey::generate(&mut OsRandom), ring };
        let back = Keys::from(&k.bytes()).unwrap();
        assert_eq!((back.key.public(), &back.ring), (k.key.public(), &k.ring));
        assert!(Keys::from(&[0; 10]).is_none());
        assert!(Keys::from(&[0; 33]).is_none(), "a key and no ring");
    }

    /// A keychain that answers when let: the OS asking the user first.
    struct Gated {
        keys: Option<Vec<u8>>,
        gate: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl Secrets for Gated {
        fn get(&self, _: &str) -> Option<Vec<u8>> {
            let _ = self.gate.lock().unwrap().recv();
            self.keys.clone()
        }
        fn set(&self, _: &str, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn delete(&self, _: &str) {}
    }

    /// Starting up never waits on the keychain: sync is "Unlocking" until
    /// it answers, then on; denied, off and saying so, `sync.txt` kept for
    /// the next start. The app is woken either way.
    #[test]
    fn the_keychain_is_waited_for_off_the_window() {
        let dir = std::env::temp_dir().join(format!("carnet-unlock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("sync.txt");
        let c = Config { origin: "https://n.example".into(), account: vec![1], device: vec![2; 8], cursor: 5, name: None };
        std::fs::write(&config, c.text()).unwrap();
        let keys = Keys { key: DeviceKey::generate(&mut OsRandom), ring: Ring::new(VaultKey::generate(&mut OsRandom)) }.bytes();
        let mut store = crate::store::Store::open(dir.join("notes")).unwrap();

        for allowed in [true, false] {
            let (open, gate) = std::sync::mpsc::channel();
            let secrets = Arc::new(Gated { keys: allowed.then(|| keys.clone()), gate: std::sync::Mutex::new(gate) });
            let t0 = std::time::Instant::now();
            let mut s = Sync::load(config.clone(), secrets);
            assert!(t0.elapsed() < std::time::Duration::from_secs(1), "load returned without the keychain");
            assert_eq!(s.status, Status::Unlocking);
            let (woke, waking) = std::sync::mpsc::channel();
            s.set_wake(move || {
                let _ = woke.send(());
            });
            s.tick(&mut store);
            assert_eq!(s.status, Status::Unlocking, "still waiting");
            open.send(()).unwrap();
            waking.recv_timeout(std::time::Duration::from_secs(5)).expect("woken when it answers");
            s.tick(&mut store);
            if allowed {
                assert!(s.is_on());
                assert_eq!(s.status, Status::Connecting);
            } else {
                assert!(!s.is_on());
                assert!(s.error.as_deref().is_some_and(|e| e.contains("keychain")));
                assert!(config.exists(), "kept, to try again next start");
                assert_eq!(s.last_origin.as_deref(), Some("https://n.example"), "offered again");
            }
        }
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(dir.join("notes").with_extension("replica"));
    }

    #[cfg(unix)]
    #[test]
    fn a_key_file_is_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("carnet-keyfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("sync.key");
        write_private(&p, b"secret").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_code_or_words_say_so_and_start_nothing() {
        let mut s = Sync::off();
        s.start("notes.example.com", How::Code("47-12".into()));
        assert!(s.error.as_deref().is_some_and(|e| e.starts_with("That isn't a code")));
        assert!(!matches!(s.phase, Phase::Starting { .. }));
        s.start("notes.example.com", How::Recover("apple banana".into()));
        assert!(s.error.as_deref().is_some_and(|e| e.contains("That's 2 words")));
        s.start("  ", How::Code("K7QF-2M9D-XW4H-P3TA".into()));
        assert!(s.error.as_deref().is_some_and(|e| e.starts_with("Which server?")));
    }
}

/// Things gone that the server is still to be told of (docs/SYNC.md §4):
/// the rooms of notes deleted forever, and pictures no note shows. Kept
/// beside the replica (`forgotten.txt`) until the server says done, so a
/// quit, or a deletion made offline, doesn't leave them there for good.
#[derive(Default)]
struct Pending {
    file: Option<PathBuf>,
    rooms: std::collections::BTreeSet<String>,
    /// Blob ids, hex.
    pictures: std::collections::BTreeSet<String>,
}

impl Pending {
    fn open(dir: Option<&Path>) -> Self {
        let file = dir.map(|d| d.join("forgotten.txt"));
        let mut p = Pending { file, ..Default::default() };
        for line in p.file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default().lines() {
            match line.split_once(' ') {
                Some(("room", r)) => {
                    p.rooms.insert(r.to_string());
                }
                Some(("picture", id)) => {
                    p.pictures.insert(id.to_string());
                }
                _ => {}
            }
        }
        p
    }

    fn keep(&self) {
        let Some(file) = &self.file else { return };
        let text: String = self.rooms.iter().map(|r| format!("room {r}\n")).chain(self.pictures.iter().map(|p| format!("picture {p}\n"))).collect();
        let tmp = file.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }

    fn add(&mut self, f: Forgotten) {
        if f.is_empty() {
            return;
        }
        self.rooms.extend(f.keys.iter().map(Key::to_string));
        self.pictures.extend(f.pictures);
        self.keep();
    }

    fn done_room(&mut self, room: &str) {
        if self.rooms.remove(room) {
            self.keep();
        }
    }

    fn done_pictures(&mut self, ids: &[BlobId]) {
        let before = self.pictures.len();
        for id in ids {
            self.pictures.remove(&hex(id));
        }
        if self.pictures.len() != before {
            self.keep();
        }
    }
}

fn unhex32(s: &str) -> Option<BlobId> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}
