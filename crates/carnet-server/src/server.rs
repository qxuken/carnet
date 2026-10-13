//! The server as a program (docs/SYNC.md §6): HTTP and WebSockets around
//! the hub. The hub runs on a thread of its own, with the SQLite file;
//! every socket's frames go to it in one queue, so it sees one thing at a
//! time, exactly as in the simulation, and a slow disk never stalls the
//! sockets.
//!
//! ```text
//! GET  /              what this is
//! GET  /v1/info       {"server", "version", "origin", "features", …}
//! GET  /v1/health     200 {"ok": true} while the hub answers; 503 if not
//! GET  /v1/sync       the WebSocket: Carnet frames and Loro Protocol rooms
//!                     (an invite is used there too: `Enroll`)
//! GET  /v1/blobs/T    a sealed picture, by a link the socket gave (`links.rs`)
//! PUT  /v1/blobs/T    one put, the same way
//! ```
//!
//! With a metrics address, a listener of its own (kept off the public one:
//! the numbers say how busy the server is, if nothing of what it holds):
//!
//! ```text
//! GET  /metrics       Prometheus' text format (`metrics_text`)
//! ```
//!
//! TLS is the proxy's in front (nginx, Cloudflare, Caddy): this speaks
//! plain HTTP.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{ConnectInfo, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{any, get};
use axum::{Json, Router};
use carnet_sync::compat;
use carnet_sync::crypto::{OsRandom, Random};
use futures_util::{SinkExt, StreamExt};
use carnet_sync::invite::InviteCode;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::blobs::{self as pictures, BlobFiles, Blobs, Counts, FileBlobs, Job, Worker};
use crate::links::Signer;
use crate::hub::{BACKFILL_PART, ConnId, Hub, Open, Out, Stats};
use crate::limits::{self, ClientHeader, Limiter, Network};
use crate::quota::{Admission, ByInvite, PasswordEmails, Quota, Unlimited};
use crate::sqlite::Sharded;
use crate::storage::{Failed, Storage};

/// How the server is run.
#[derive(Clone, Debug)]
pub struct Config {
    /// Where the databases are: `carnet.db`, and `accounts/` beside it.
    pub data: PathBuf,
    /// The URL devices reach this server at, as they sign it into their
    /// logins (§7): behind a proxy, the proxy's public `https://…`.
    pub origin: String,
    /// A room is compacted past this many records (§6): [`COMPACT_AFTER`]
    /// unless a test wants it sooner.
    pub compact_after: usize,
    /// The PWA's build (web/dist), served at `/` (docs/SYNC.md §8); none,
    /// and `/` says what this server is.
    pub web: Option<std::path::PathBuf>,
    /// The proxies in front (nginx, Cloudflare's tunnel), whose word on
    /// who a request is from is taken: what the limits count by. None,
    /// and each request is from the address it came from.
    pub trusted: Vec<Network>,
    /// Which header the trusted proxies say it in: `X-Forwarded-For`
    /// unless they're Cloudflare's.
    pub client_header: ClientHeader,
    /// TLS on the listener itself, and the CA a client's certificate must
    /// be signed by, if any (tls.rs). None: plain HTTP, a proxy in front
    /// ending TLS.
    pub tls: Option<crate::tls::Tls>,
}

/// Records a room keeps one by one before it's asked for as a snapshot.
pub const COMPACT_AFTER: usize = 200;

pub use crate::hub::INVITE_DAYS;

/// The largest frame a socket takes: past it, the socket closes.
pub const MAX_FRAME: usize = 1 << 20;

/// How long a socket may take, and how much may wait for it
/// (docs/SYNC.md §6). [`SocketLimits::default`] unless a test wants
/// them sooner.
#[derive(Clone, Copy, Debug)]
pub struct SocketLimits {
    /// To log in, from the socket opening: no challenge waits longer for
    /// its answer. Past it, the socket closes.
    pub login: Duration,
    /// With nothing heard from it: then it's gone, and its socket closes.
    pub idle: Duration,
    /// Quiet this long, it's sent a WebSocket ping, which a browser
    /// answers by itself: an app that's there is never idle.
    pub ping: Duration,
    /// Bytes waiting to be written to it: past this, it isn't reading,
    /// and its socket closes, what waited dropped.
    pub queue: usize,
}

/// To log in: 30 seconds, the challenge's life (§7).
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);
/// Nothing heard for 5 minutes: ten of the apps' pings (every 30 s), or
/// five of the server's own, missed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Quiet for a minute: pinged.
pub const PING_AFTER: Duration = Duration::from_secs(60);
/// 16 frames' worth may wait for a socket: a room's backfill and the
/// others' writes meanwhile, many times over. A socket reads no more of
/// its device's frames while a frame's worth waits (or the hub hasn't
/// done what it gave it), so asking again and again without reading
/// gets nowhere; past this, something sent unasked piled up.
pub const MAX_QUEUED: usize = 16 * MAX_FRAME;

/// How long a request's head may take to come, and a connection may sit
/// idle between requests: past it, the connection closes, so one sent
/// slowly holds nothing for long ([`serve_app`]).
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a request may take, its body read and its answer begun: the
/// API's, the web app's files, a host's routes. Past it, 408.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A picture's put or fetch by its link: its largest, 32 MiB, on a line
/// as slow as 56 kB/s. (A socket, once upgraded, has its own limits.) A
/// put's room is held as long past its link's time (`links::PUT_TAKES`).
pub const BLOB_TIMEOUT: Duration = Duration::from_secs(crate::links::PUT_TAKES);

impl Default for SocketLimits {
    fn default() -> Self {
        SocketLimits { login: LOGIN_TIMEOUT, idle: IDLE_TIMEOUT, ping: PING_AFTER, queue: MAX_QUEUED }
    }
}

/// "1 account", "3 accounts".
fn accounts(n: usize) -> String {
    format!("{n} account{}", if n == 1 { "" } else { "s" })
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A new invite, kept by its hash, good for a week. With no account yet,
/// it makes the owner's.
pub fn new_invite(store: &mut impl Storage) -> Result<InviteCode, Failed> {
    let owner = store.accounts() == 0;
    invite(store, owner)
}

/// An invite for an account of its own, good for [`INVITE_DAYS`]; the
/// owner's, or not (a hosted service's signups never are).
pub fn invite(store: &mut (impl Storage + ?Sized), owner: bool) -> Result<InviteCode, Failed> {
    let code = InviteCode::generate(&mut OsRandom);
    store.add_invite(code.hash(), owner, now() + INVITE_DAYS * 24 * 3600)?;
    Ok(code)
}

/// What a socket or a request asks of the hub's thread.
enum ToHub {
    Open { conn: ConnId, to: Line, client: String, app: compat::Client },
    Frame { conn: ConnId, bytes: Vec<u8> },
    Closed { conn: ConnId },
    /// Whether the hub answers: `/v1/health`.
    Ping { reply: oneshot::Sender<()> },
    /// A frame the picture worker made, for a socket: passed on as it is.
    Deliver { conn: ConnId, bytes: Vec<u8> },
    /// A socket with more of a room's records to come has written most of
    /// what it was sent: the next part ([`Hub::drained`]).
    Drained { conn: ConnId },
    /// The numbers for `/metrics`, asked at `asked`: the time the ask
    /// waited in the queue is the hub's lag.
    Metrics { asked: Instant, reply: oneshot::Sender<Snapshot> },
    /// Something a host asked done on the hub's thread ([`HubHandle`]).
    Run(HubJob),
    /// The server is stopping: sockets still open are let go.
    Shutdown,
}

/// What a host gives the hub: its limits, who's let in, and its OPAQUE
/// key if it takes passwords.
type Seams = (Arc<dyn Quota>, Arc<dyn Admission>, Option<carnet_sync::password::Server>, PasswordEmails, bool);

/// The server's OPAQUE key, made the first time and kept in `data`
/// (`opaque.key`, readable by the server alone): a new one would make
/// every password set with the old one fail.
fn opaque_key(data: &std::path::Path) -> std::io::Result<carnet_sync::password::Server> {
    use carnet_sync::password::Server;
    let path = data.join("opaque.key");
    match std::fs::read(&path) {
        Ok(b) => Server::from_bytes(&b).ok_or_else(|| std::io::Error::other(format!("{}: not an OPAQUE key", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let server = Server::generate(&mut OsRandom);
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, server.to_bytes())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
            }
            std::fs::rename(&tmp, &path)?;
            Ok(server)
        }
        Err(e) => Err(e),
    }
}

/// Work a host has done on the hub: what it says to send comes back.
type HubJob = Box<dyn FnOnce(&mut Hub<Store>) -> Vec<Out> + Send>;

/// The store the server runs on: SQLite files ([`Sharded`]) unless a host
/// brings its own ([`Host::store`]).
pub type Store = Box<dyn Storage + Send>;

/// What a host (a hosted service built on this server, docs/SYNC.md §11)
/// adds: its own routes beside the API, limits on what each account
/// keeps, its own store. The default is a server of one's own.
pub struct Host {
    /// Merged beside the API; `/` among them takes the place of the
    /// viewer's page there (a landing page), the viewer still everywhere
    /// else.
    pub routes: Router,
    pub quota: Arc<dyn Quota>,
    /// Told of each account an invite makes.
    pub admission: Arc<dyn Admission>,
    /// Where someone new makes an account (a hosted service's signup
    /// page): `/v1/info` says it, and the viewer's first screen links it.
    pub signup: Option<String>,
    /// Take passwords (§7): devices may set one, and sign in by email
    /// and password. Its OPAQUE key is made on the first start
    /// (`data/opaque.key`) and never leaves the server.
    pub passwords: bool,
    /// Which email a password may sign in as: any (a server of one's
    /// own), or only the one the account came in by, as `admission` says
    /// (a hosted service, whose signup confirmed it).
    pub password_emails: PasswordEmails,
    /// Print an invite for the first account when there's none (a server
    /// of one's own: its owner's way in).
    pub first_invite: bool,
    /// Instead of the SQLite files in `data`: a database server
    /// (`postgres::Postgres`, with the `postgres` feature).
    pub store: Option<Store>,
    /// Instead of the files in `data/blobs`: object storage
    /// (`s3::S3Blobs`, with the `s3` feature), its links its own. The
    /// server's `/v1/blobs/…` then has nothing to serve.
    pub blobs: Option<Box<dyn Blobs>>,
    /// Given the hub's handle once it runs.
    pub ready: Option<Box<dyn FnOnce(HubHandle) + Send>>,
}

impl Default for Host {
    fn default() -> Self {
        Host {
            routes: Router::new(),
            quota: Arc::new(Unlimited),
            admission: Arc::new(ByInvite),
            signup: None,
            passwords: false,
            password_emails: PasswordEmails::Any,
            first_invite: true,
            store: None,
            blobs: None,
            ready: None,
        }
    }
}

/// A way to the hub from a host's own code: its work done on the hub's
/// thread, one thing at a time like everything else there.
#[derive(Clone)]
pub struct HubHandle {
    tx: mpsc::UnboundedSender<ToHub>,
}

impl HubHandle {
    /// `f` run on the hub, with what it says to send sent; its answer back.
    /// `None` if the hub has stopped.
    pub async fn run<T: Send + 'static>(&self, f: impl FnOnce(&mut Hub<Store>, &mut Vec<Out>) -> T + Send + 'static) -> Option<T> {
        let (reply, rx) = oneshot::channel();
        let job = Box::new(move |hub: &mut Hub<Store>| {
            let mut outs = Vec::new();
            let _ = reply.send(f(hub, &mut outs));
            outs
        });
        self.tx.send(ToHub::Run(job)).ok()?;
        rx.await.ok()
    }

    /// `account`'s limit changed (`Hub::limit_changed`): its devices online
    /// are told now. For a host, after it changed what its `Quota` says
    /// (a plan set, a suspension lifted), and forgot what it kept of it.
    /// `None` if the hub has stopped.
    pub async fn limit_changed(&self, account: Vec<u8>) -> Option<()> {
        self.run(move |hub, outs| outs.extend(hub.limit_changed(&account))).await
    }
}

/// The server's numbers at one moment: the hub's, and the traffic and
/// timings its thread saw.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub stats: Stats,
    pub open: Open,
    pub accounts: usize,
    /// Sockets open (as the driver holds them), and opened since start.
    pub sockets: u64,
    pub sockets_opened: u64,
    pub frames_in: u64,
    pub bytes_in: u64,
    pub frames_out: u64,
    pub bytes_out: u64,
    /// How long each thing the hub was given took it.
    pub handled: Timings,
    /// How long this snapshot's ask waited in the queue.
    pub waited: Duration,
    /// Sockets refused before they opened: too many from one address.
    pub sockets_refused: u64,
    /// Pictures: kept (put new), their bytes, links given; and over HTTP,
    /// fetched, put, and puts that failed.
    pub blobs_stored: u64,
    pub blob_bytes: u64,
    pub blob_links: u64,
    /// Pictures a device said no note shows (marked), and those gone 30
    /// days on.
    pub blobs_dropped: u64,
    pub blobs_swept: u64,
    pub blob_gets: u64,
    pub blob_puts: u64,
    pub blob_failures: u64,
    /// Sockets closed for their limits ([`SocketLimits`]): too much
    /// waiting for them, too long to log in, nothing heard.
    pub closed_budget: u64,
    pub closed_login: u64,
    pub closed_idle: u64,
}

/// A histogram of durations, by Prometheus' rules: each bucket counts
/// those at or under its bound.
#[derive(Clone, Debug, Default)]
pub struct Timings {
    pub buckets: [u64; BOUNDS.len()],
    pub count: u64,
    pub sum: f64,
}

/// The buckets' bounds, in seconds.
pub const BOUNDS: [f64; 9] = [0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.1, 0.5, 2.5];

impl Timings {
    fn add(&mut self, d: Duration) {
        let secs = d.as_secs_f64();
        for (b, &bound) in self.buckets.iter_mut().zip(BOUNDS.iter()) {
            if secs <= bound {
                *b += 1;
            }
        }
        self.count += 1;
        self.sum += secs;
    }
}

#[derive(Clone)]
struct App {
    hub: mpsc::UnboundedSender<ToHub>,
    origin: Arc<str>,
    signup: Option<Arc<str>>,
    /// It takes passwords (§7).
    passwords: bool,
    /// And which email they sign in as.
    password_emails: PasswordEmails,
    next: Arc<AtomicU64>,
    trusted: Arc<[Network]>,
    client_header: ClientHeader,
    gate: Arc<std::sync::Mutex<Gate>>,
    refused: Arc<AtomicU64>,
    sockets: SocketLimits,
    closes: Arc<Closes>,
}

/// Sockets the server closed for its limits, by why, for `/metrics`.
#[derive(Default)]
struct Closes {
    /// More waited to be sent than [`SocketLimits::queue`].
    budget: AtomicU64,
    /// Not logged in within [`SocketLimits::login`].
    login: AtomicU64,
    /// Nothing heard for [`SocketLimits::idle`].
    idle: AtomicU64,
}

/// Sockets by client: how many each has open, and how fast it opens them.
struct Gate {
    open: std::collections::HashMap<String, usize>,
    opens: Limiter,
    started: Instant,
}

impl Gate {
    /// Whether `client` may open one more: then it's counted as open.
    fn admit(&mut self, client: &str) -> bool {
        let now = self.started.elapsed().as_secs_f64();
        if self.open.get(client).copied().unwrap_or(0) >= limits::MAX_SOCKETS || !self.opens.allow(client, now) {
            return false;
        }
        *self.open.entry(client.to_string()).or_default() += 1;
        if self.opens.len() > 10_000 {
            self.opens.prune(now);
        }
        true
    }

    fn closed(&mut self, client: &str) {
        if let Some(n) = self.open.get_mut(client) {
            *n -= 1;
            if *n == 0 {
                self.open.remove(client);
            }
        }
    }
}

/// The hub's thread: one thing at a time, from every socket.
#[allow(clippy::too_many_arguments)]
fn hub_thread(
    origin: String,
    compact_after: usize,
    store: Store,
    seams: Seams,
    limits: SocketLimits,
    mut rx: mpsc::UnboundedReceiver<ToHub>,
    worker: std::sync::mpsc::Sender<Job>,
) {
    let (quota, admission, passwords, password_emails, signup) = seams;
    let mut hub = Hub::new(origin, store).with_quota(quota).with_admission(admission).with_password_emails(password_emails).with_signup(signup);
    if let Some(p) = passwords {
        hub = hub.with_passwords(p);
    }
    hub.compact_when(compact_after, 256 * 1024);
    // A room's records go a part at a time, an eighth of what may wait
    // for a socket at most: the next once it has written all but its pace
    // (`Meter::more`). So what waits stays under its pace, a part, and a
    // message more (a big one, a fragment) whatever the room holds.
    hub.backfill_part(BACKFILL_PART.min(limits.queue / 8));
    let mut sockets: std::collections::HashMap<ConnId, Line> = Default::default();
    let mut rng = OsRandom;
    let mut seen = Snapshot::default();
    while let Some(msg) = rx.blocking_recv() {
        let began = Instant::now();
        hub.tick(now());
        // A frame done: its socket may give the next, once what it
        // brought is queued.
        let mut done = None;
        let outs = match msg {
            ToHub::Open { conn, to, client, app } => {
                sockets.insert(conn, to);
                seen.sockets_opened += 1;
                hub.connect_from(conn, &client, app, &mut rng)
            }
            ToHub::Frame { conn, bytes } => {
                seen.frames_in += 1;
                seen.bytes_in += bytes.len() as u64;
                done = Some((conn, bytes.len() + OVERHEAD));
                hub.receive(conn, &bytes, &mut rng)
            }
            ToHub::Closed { conn } => {
                sockets.remove(&conn);
                hub.disconnect(conn)
            }
            ToHub::Ping { reply } => {
                let _ = reply.send(());
                Vec::new()
            }
            ToHub::Deliver { conn, bytes } => vec![Out::Send(conn, bytes)],
            ToHub::Drained { conn } => hub.drained(conn),
            ToHub::Run(job) => job(&mut hub),
            ToHub::Metrics { asked, reply } => {
                let _ = reply.send(Snapshot {
                    stats: hub.stats().clone(),
                    open: hub.open(),
                    accounts: hub.store().accounts(),
                    sockets: sockets.len() as u64,
                    waited: asked.elapsed(),
                    ..seen.clone()
                });
                // A scrape isn't the hub's work: not timed.
                continue;
            }
            ToHub::Shutdown => break,
        };
        let mut outs = outs;
        loop {
            route(&mut hub, &mut sockets, outs, &mut seen, &worker);
            // Sockets with more of a room to come: sent it now if they've
            // written what they had, else once they have (their writer
            // says, `ToHub::Drained`).
            outs = Vec::new();
            for conn in hub.take_more() {
                if let Some(s) = sockets.get(&conn)
                    && s.wants_more()
                {
                    outs.extend(hub.drained(conn));
                }
            }
            if outs.is_empty() {
                break;
            }
        }
        if let Some((conn, cost)) = done
            && let Some(s) = sockets.get(&conn)
        {
            if hub.logged_in(conn) {
                s.meter.ready.store(true, Ordering::Release);
            }
            s.meter.handled(cost);
        }
        seen.handled.add(began.elapsed());
    }
}

/// What the hub said, done: frames queued for their sockets, sockets
/// closed, pictures handed to the worker.
fn route(hub: &mut Hub<Store>, sockets: &mut std::collections::HashMap<ConnId, Line>, outs: Vec<Out>, seen: &mut Snapshot, worker: &std::sync::mpsc::Sender<Job>) {
    for o in outs {
        match o {
            Out::Send(conn, bytes) => {
                if let Some(s) = sockets.get(&conn) {
                    let n = bytes.len() as u64;
                    if s.send(Message::Binary(bytes.into())) {
                        seen.frames_out += 1;
                        seen.bytes_out += n;
                    }
                }
            }
            Out::Close(conn) => {
                if let Some(s) = sockets.remove(&conn) {
                    let _ = s.send(Message::Close(None));
                }
                // What it was in has been told already, by the hub;
                // what it had coming goes.
                for o in hub.disconnect(conn) {
                    if let Out::Blob(job) = o {
                        let _ = worker.send(job);
                    }
                }
            }
            // Pictures: allowed, and handed on; the worker does them.
            Out::Blob(job) => {
                let _ = worker.send(job);
            }
        }
    }
}

/// Serves on `listener` until `stop` resolves. Prints an invite for the
/// first account when there is none.
pub async fn serve(config: Config, listener: TcpListener, stop: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    serve_with(config, listener, None, stop).await
}

/// [`serve`], and `/metrics` on `metrics` if given.
pub async fn serve_with(config: Config, listener: TcpListener, metrics: Option<TcpListener>, stop: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    serve_hosted(config, listener, metrics, Host::default(), stop).await
}

/// [`serve_with`], with what a host adds.
pub async fn serve_hosted(config: Config, listener: TcpListener, metrics: Option<TcpListener>, host: Host, stop: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    serve_limited(config, listener, metrics, host, SocketLimits::default(), stop).await
}

/// [`serve_hosted`], its sockets' timeouts and queues as said.
pub async fn serve_limited(
    config: Config,
    listener: TcpListener,
    metrics: Option<TcpListener>,
    host: Host,
    sockets: SocketLimits,
    stop: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    serve_started(config, listener, metrics, host, sockets, Start::default(), stop).await
}

/// How a server starts on what it finds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Start {
    /// Start with no account beside pictures of accounts the store
    /// doesn't have (and whose removal isn't recorded): refused unless
    /// said, as the records were likely not put back (a new data folder,
    /// an empty database) on pictures a server kept before. They're kept
    /// either way: the sweep takes only the pictures of accounts removed
    /// here (`blobs::Worker::sweep_unowned`).
    pub beside_old_pictures: bool,
}

/// [`serve_limited`], started as `start` says.
pub async fn serve_started(
    config: Config,
    listener: TcpListener,
    metrics: Option<TcpListener>,
    host: Host,
    sockets: SocketLimits,
    start: Start,
    stop: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let started = now();
    // Its own user's alone, before anything is made in it.
    crate::private::data_dir(&config.data)?;
    let Host { routes, quota, admission, signup, passwords, password_emails, first_invite, store: hosted_store, blobs: hosted_blobs, ready } = host;
    let passwords = if passwords { Some(opaque_key(&config.data)?) } else { None };
    let takes_passwords = passwords.is_some();
    let takes_signups = signup.is_some();
    // Pictures: files beside the databases, reached by signed links,
    // unless the host keeps them elsewhere.
    let files = BlobFiles::open(&config.data)?;
    let signer = Arc::new(Signer::open(&config.data, || OsRandom.bytes())?);
    let in_bucket = hosted_blobs.is_some();
    let blobs: Box<dyn Blobs> = hosted_blobs.unwrap_or_else(|| Box::new(FileBlobs::new(files.clone(), &config.origin, signer.clone())));
    // The store made ready off the async threads: a database's client
    // blocks (as the hub does, on its own thread, from then on).
    let (data, kept) = (config.data.clone(), files.clone());
    let (store, blobs) = tokio::task::spawn_blocking(move || -> std::io::Result<(Store, Box<dyn Blobs>)> {
        let mut blobs = blobs;
        let mut store: Store = match hosted_store {
            Some(store) => store,
            None => {
                // Those a server before kept in its databases are moved out
                // into files, once.
                let store = Sharded::open(&data).map_err(std::io::Error::other)?;
                let mut moved = 0;
                for (account, _, _) in store.account_list() {
                    let blobs = store.take_blobs(&account).map_err(std::io::Error::other)?;
                    for (id, sealed) in &blobs {
                        kept.keep(&account, id, sealed)?;
                    }
                    if !blobs.is_empty() {
                        store.drop_blobs(&account).map_err(std::io::Error::other)?;
                        moved += blobs.len();
                    }
                }
                if moved > 0 {
                    println!("carnet-server: moved {moved} pictures out of the databases, into {}", kept.root.display());
                }
                Box::new(store)
            }
        };
        if store.accounts() == 0 && !start.beside_old_pictures {
            let strangers = crate::blobs::strangers(blobs.as_mut(), |a| store.has_account(a));
            if !strangers.is_empty() {
                let (place, other) = if in_bucket { ("The bucket", "another bucket (or another folder in it)") } else { ("The data folder", "another data folder") };
                return Err(std::io::Error::other(format!(
                    "{place} holds pictures of {}, and there are no accounts here: their records are missing. Put the records back first (the data folder, or the Postgres database, from a backup), or start on {other}. To start anyway, with those pictures kept and unused, give --beside-old-pictures (CARNET_BESIDE_OLD_PICTURES=1)",
                    accounts(strangers.len())
                )));
            }
        }
        if first_invite && store.accounts() == 0 {
            let code = new_invite(&mut store).map_err(std::io::Error::other)?;
            println!("carnet-server: no accounts yet. Invite code for the first account (owner):");
            println!("    {}   (single use, good for {INVITE_DAYS} days)", code.text());
        }
        Ok((store, blobs))
    })
    .await
    .map_err(std::io::Error::other)??;
    let (tx, rx) = mpsc::unbounded_channel();
    let shutdown = tx.clone();
    let origin = config.origin.clone();
    let compact_after = config.compact_after;
    let http = Arc::new(BlobHttp::default());
    // The picture worker: its own thread; what it sends goes to the
    // sockets through the hub's queue.
    let csp = web_csp(&config.origin, blobs.origin().as_deref());
    let mut worker = Worker::new(blobs);
    let counts = worker.counts.clone();
    let (jobs, queued) = std::sync::mpsc::channel::<Job>();
    let deliver = tx.clone();
    // Once an hour, pictures dropped more than 30 days ago go, and those
    // of accounts removed (a put by a link given before its account was
    // deleted): the hub says which accounts the store doesn't have, and
    // of those, the ones whose removal is recorded go.
    let pictures = std::thread::Builder::new().name("carnet-pictures".into()).spawn(move || {
        let mut swept = Instant::now() - Duration::from_secs(3600);
        let mut kept_before = 0;
        loop {
            if swept.elapsed() >= Duration::from_secs(3600) {
                swept = Instant::now();
                worker.sweep(now());
                let unowned = worker.sweep_unowned(|owners| {
                    let (reply, answer) = oneshot::channel();
                    let ask: HubJob = Box::new(move |hub: &mut Hub<Store>| {
                        let _ = reply.send(owners.into_iter().filter(|a| !hub.store().has_account(a)).collect::<Vec<_>>());
                        Vec::new()
                    });
                    if deliver.send(ToHub::Run(ask)).is_err() {
                        return Vec::new();
                    }
                    answer.blocking_recv().unwrap_or_default()
                });
                if unowned.gone > 0 {
                    println!("carnet-server: pictures of {} removed, gone", accounts(unowned.gone));
                }
                // Said when it changes: not every hour.
                if unowned.kept != kept_before {
                    kept_before = unowned.kept;
                    if unowned.kept > 0 {
                        println!(
                            "carnet-server: pictures of {} this server has no record of are kept, not deleted (records restored from before they were made?)",
                            accounts(unowned.kept)
                        );
                    }
                }
            }
            match queued.recv_timeout(Duration::from_secs(3600).saturating_sub(swept.elapsed())) {
                Ok(job) => worker.run(job, &mut |o| {
                    if let Out::Send(conn, bytes) = o {
                        let _ = deliver.send(ToHub::Deliver { conn, bytes });
                    }
                }),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    })?;
    let hub =std::thread::Builder::new().name("carnet-hub".into()).spawn(move || hub_thread(origin, compact_after, store, (quota, admission, passwords, password_emails, takes_signups), sockets, rx, jobs))?;
    if let Some(ready) = ready {
        ready(HubHandle { tx: tx.clone() });
    }
    let refused = Arc::new(AtomicU64::new(0));
    let closes = Arc::new(Closes::default());
    let gate = Gate { open: Default::default(), opens: Limiter::new(limits::SOCKETS), started: Instant::now() };
    let app = App {
        hub: tx.clone(),
        origin: config.origin.as_str().into(),
        signup: signup.map(|s| s.as_str().into()),
        passwords: takes_passwords,
        password_emails,
        next: Arc::new(AtomicU64::new(1)),
        trusted: config.trusted.clone().into(),
        client_header: config.client_header,
        gate: Arc::new(std::sync::Mutex::new(gate)),
        refused: refused.clone(),
        sockets,
        closes: closes.clone(),
    };
    // One stop, for both listeners.
    let (stopping, stopped) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        stop.await;
        let _ = stopping.send(true);
    });
    let until_stopped = |mut rx: tokio::sync::watch::Receiver<bool>| async move {
        let _ = rx.wait_for(|&s| s).await;
    };
    let metrics_task = match metrics {
        Some(listener) => {
            let watch = Watch { hub: tx.clone(), data: config.data.clone(), started, refused: refused.clone(), counts: counts.clone(), http: http.clone(), closes };
            // Health too, in plain HTTP on this private address: what the
            // machine's own check asks when the public one wants TLS (and
            // a client certificate it hasn't).
            let router = Router::new().route("/metrics", get(metrics_route)).route("/v1/health", get(metrics_health)).with_state(watch).layer(timeout(REQUEST_TIMEOUT));
            println!("carnet-server: metrics on http://{}/metrics", listener.local_addr()?);
            Some(tokio::spawn(serve_app(listener, router, until_stopped(stopped.clone()))))
        }
        None => None,
    };
    // Each request in its time: a picture's in minutes, any other in
    // seconds (a socket's only until it's upgraded).
    let blobs: Router = Router::new()
        .route("/v1/blobs/{token}", get(blob_get).put(blob_put))
        .layer(axum::extract::DefaultBodyLimit::max(pictures::MAX as usize))
        .layer(timeout(BLOB_TIMEOUT))
        .with_state(Links { signer, files, http, hub: tx.clone() });
    let api = Router::new()
        .route("/v1/info", get(info))
        .route("/v1/health", get(health))
        .route("/v1/sync", get(sync_route))
        // Not the viewer's page: an API address that isn't one is a 404.
        .route("/v1/{*rest}", any(|| async { StatusCode::NOT_FOUND }))
        .with_state(app)
        .merge(routes)
        .layer(timeout(REQUEST_TIMEOUT));
    let router = blobs.merge(api);
    // The viewer at `/`, its files as built; an address it doesn't have is
    // its page (the app finds its way from there). Its files, and only
    // they, with its CSP (a host's own pages say their own).
    let router = match &config.web {
        Some(dir) => {
            let index = tower_http::services::ServeFile::new(dir.join("index.html"));
            let files = tower_http::services::ServeDir::new(dir).precompressed_br().precompressed_gzip().fallback(index);
            let csp = HeaderValue::from_str(&csp).map_err(std::io::Error::other)?;
            router.fallback_service(Router::new().fallback_service(files).layer(axum::middleware::from_fn_with_state(csp, web_headers)).layer(timeout(REQUEST_TIMEOUT)))
        }
        None => router.fallback(|uri: axum::http::Uri| async move {
            if uri.path() == "/" {
                (StatusCode::OK, "carnet-server: the sync server for Carnet notes. Devices connect at /v1/sync.\n")
            } else {
                (StatusCode::NOT_FOUND, "")
            }
        }),
    };
    // Nothing taken for another type than it's said to be, anywhere.
    let router = router.layer(axum::middleware::map_response(|mut res: axum::response::Response| async move {
        res.headers_mut().entry(header::X_CONTENT_TYPE_OPTIONS).or_insert(HeaderValue::from_static("nosniff"));
        res
    }));
    let addr: SocketAddr = listener.local_addr()?;
    let tls = match &config.tls {
        Some(t) if t.asks_clients() => " (TLS, a client certificate asked of each)",
        Some(_) => " (TLS)",
        None => "",
    };
    println!("carnet-server: listening on {addr}{tls}, origin {}", config.origin);
    let served = serve_app_on(listener, router, config.tls.clone(), until_stopped(stopped)).await;
    if let Some(t) = metrics_task {
        let _ = t.await;
    }
    // The hub's thread ends, the database closed with it; sockets still
    // open find the queue gone and close.
    let _ = shutdown.send(ToHub::Shutdown);
    let _ = tokio::task::spawn_blocking(move || {
        let _ = hub.join();
        // Its queue closed with the hub: it finishes what it has, and ends.
        let _ = pictures.join();
    })
    .await;
    served
}

/// Past `limit`, a request is answered 408: [`REQUEST_TIMEOUT`] for the
/// API, a host's routes, and a host's other listeners'.
pub fn timeout(limit: Duration) -> tower_http::timeout::TimeoutLayer {
    tower_http::timeout::TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, limit)
}

/// `router` served on `listener` until `stop`, as `axum::serve` serves
/// (HTTP/1.1, each request's address its `ConnectInfo`, a socket
/// upgraded), and each request's head read within [`HEAD_TIMEOUT`]: a
/// connection that sends one slowly, or sits idle, is let go. Once
/// stopped, no more is taken, and what's under way finishes (a socket,
/// upgraded, is the hub's to let go). A host's other listeners use it too.
pub async fn serve_app(listener: TcpListener, router: Router, stop: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    serve_app_on(listener, router, None, stop).await
}

/// [`serve_app`], over TLS if given: each connection's handshake within
/// [`HEAD_TIMEOUT`], and one that fails it (no client certificate when
/// one is asked, one no CA given signed, not TLS at all) let go before
/// anything of it is read.
pub async fn serve_app_on(listener: TcpListener, router: Router, tls: Option<crate::tls::Tls>, stop: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    let (closing, closed) = tokio::sync::watch::channel(false);
    // Each connection holds one: all dropped, all done.
    let (open, mut done) = mpsc::channel::<()>(1);
    let mut stop = std::pin::pin!(stop);
    loop {
        let (io, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(a) => a,
                // Out of files, or one reset before it was taken: a
                // moment, and on.
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
            _ = &mut stop => break,
        };
        let (router, tls) = (router.clone(), tls.clone());
        let (open, closed) = (open.clone(), closed.clone());
        tokio::spawn(async move {
            let _open = open;
            match tls {
                None => connection(io, peer, router, closed).await,
                Some(tls) => {
                    if let Ok(Ok(io)) = tokio::time::timeout(HEAD_TIMEOUT, tls.acceptor.accept(io)).await {
                        connection(io, peer, router, closed).await;
                    }
                }
            }
        });
    }
    drop(listener);
    let _ = closing.send(true);
    drop(open);
    let _ = done.recv().await;
    drop(closed);
    Ok(())
}

/// One connection served, until it ends or the server stops.
async fn connection<I>(io: I, peer: SocketAddr, router: Router, mut closed: tokio::sync::watch::Receiver<bool>)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let mut req = req.map(axum::body::Body::new);
        req.extensions_mut().insert(ConnectInfo(peer));
        tower::ServiceExt::oneshot(router.clone(), req)
    });
    let mut http = hyper::server::conn::http1::Builder::new();
    http.timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(HEAD_TIMEOUT);
    let mut conn = std::pin::pin!(http.serve_connection(hyper_util::rt::TokioIo::new(io), service).with_upgrades());
    tokio::select! {
        _ = conn.as_mut() => return,
        _ = closed.wait_for(|&c| c) => {}
    }
    conn.as_mut().graceful_shutdown();
    let _ = conn.await;
}

/// The web app's Content-Security-Policy (docs/SYNC.md §8): its own
/// scripts and the wasm, inline styles (Svelte's and the notes' colours),
/// pictures as the page draws them (`blob:`, `data:`; none from the web),
/// and connections to this server (its socket named too, for browsers
/// whose `'self'` leaves out `ws:`) and where pictures are, if elsewhere.
pub fn web_csp(origin: &str, pictures: Option<&str>) -> String {
    let mut connect = vec!["'self'".to_string()];
    let host = |rest: &str| rest.split(['/', '?', '#']).next().unwrap_or_default().to_string();
    if let Some(rest) = origin.strip_prefix("https://") {
        connect.push(format!("wss://{}", host(rest)));
    } else if let Some(rest) = origin.strip_prefix("http://") {
        connect.push(format!("ws://{}", host(rest)));
    }
    connect.extend(pictures.map(str::to_string));
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; connect-src {}; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; worker-src 'self'; manifest-src 'self'",
        connect.join(" ")
    )
}

/// The web app's files with its headers: the CSP, no referrer sent from
/// it, and its page and service worker asked for afresh each time, so a
/// new build is taken at once.
async fn web_headers(State(csp): State<HeaderValue>, req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let worker = req.uri().path() == "/sw.js";
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, csp);
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    let page = h.get(header::CONTENT_TYPE).is_some_and(|t| t.as_bytes().starts_with(b"text/html"));
    if page || worker {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    res
}

/// What this server is. Not how many use it: that's for `/metrics`, kept
/// off the public address.
#[derive(Serialize)]
struct Info {
    server: &'static str,
    version: &'static str,
    origin: String,
    /// Where someone new makes an account, if anywhere but an invite.
    #[serde(skip_serializing_if = "Option::is_none")]
    signup: Option<String>,
    /// Whether a device may come in by an email and password here.
    passwords: bool,
    /// Which email a password signs in as, if it takes them: `any`, or
    /// `account` (the one the account signed up with). Never an email
    /// itself: an account's own devices are told theirs.
    #[serde(skip_serializing_if = "Option::is_none")]
    password_email: Option<&'static str>,
    /// The protocol it speaks, the oldest app's it takes, where its
    /// deprecation starts, and what it can do (`compat`, docs/SYNC.md
    /// §14): what an app reads before it connects.
    protocol: u32,
    oldest: u32,
    deprecated: u32,
    features: Vec<String>,
}

async fn info(State(app): State<App>) -> impl IntoResponse {
    Json(Info {
        server: "carnet-server",
        version: env!("CARGO_PKG_VERSION"),
        origin: app.origin.to_string(),
        signup: app.signup.as_deref().map(str::to_string),
        passwords: app.passwords,
        password_email: app.passwords.then(|| app.password_emails.name()),
        protocol: compat::PROTOCOL,
        oldest: compat::OLDEST,
        deprecated: compat::DEPRECATED,
        features: crate::hub::features(app.passwords, app.signup.is_some()),
    })
}

/// Whether the hub answers, soon: what a container's health check or an
/// uptime monitor asks.
async fn health(State(app): State<App>) -> impl IntoResponse {
    let (reply, rx) = oneshot::channel();
    let _ = app.hub.send(ToHub::Ping { reply });
    match tokio::time::timeout(Duration::from_secs(5), rx).await {
        Ok(Ok(_)) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))),
        _ => (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "ok": false, "error": "the hub is not answering" }))),
    }
}

/// What `/metrics` reads from.
#[derive(Clone)]
struct Watch {
    hub: mpsc::UnboundedSender<ToHub>,
    data: PathBuf,
    started: u64,
    refused: Arc<AtomicU64>,
    counts: Arc<Counts>,
    http: Arc<BlobHttp>,
    closes: Arc<Closes>,
}

/// `/v1/health` on the metrics address: as the public one answers it.
async fn metrics_health(State(w): State<Watch>) -> impl IntoResponse {
    let (reply, rx) = oneshot::channel();
    let _ = w.hub.send(ToHub::Ping { reply });
    match tokio::time::timeout(Duration::from_secs(5), rx).await {
        Ok(Ok(_)) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))),
        _ => (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "ok": false, "error": "the hub is not answering" }))),
    }
}

async fn metrics_route(State(w): State<Watch>) -> impl IntoResponse {
    let (reply, rx) = oneshot::channel();
    let _ = w.hub.send(ToHub::Metrics { asked: Instant::now(), reply });
    let Ok(Ok(mut snapshot)) = tokio::time::timeout(Duration::from_secs(10), rx).await else {
        return (StatusCode::SERVICE_UNAVAILABLE, "the hub is not answering\n".to_string()).into_response();
    };
    snapshot.sockets_refused = w.refused.load(Ordering::Relaxed);
    snapshot.blobs_stored = w.http.stored.load(Ordering::Relaxed);
    snapshot.blob_bytes = w.http.stored_bytes.load(Ordering::Relaxed);
    snapshot.blob_links = w.counts.links.load(Ordering::Relaxed);
    snapshot.blobs_dropped = w.counts.dropped.load(Ordering::Relaxed);
    snapshot.blobs_swept = w.counts.swept.load(Ordering::Relaxed);
    snapshot.blob_gets = w.http.gets.load(Ordering::Relaxed);
    snapshot.blob_puts = w.http.puts.load(Ordering::Relaxed);
    snapshot.blob_failures = w.http.failures.load(Ordering::Relaxed);
    snapshot.closed_budget = w.closes.budget.load(Ordering::Relaxed);
    snapshot.closed_login = w.closes.login.load(Ordering::Relaxed);
    snapshot.closed_idle = w.closes.idle.load(Ordering::Relaxed);
    let data = w.data.clone();
    let bytes = tokio::task::spawn_blocking(move || dir_bytes(&data)).await.unwrap_or(0);
    ([(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], metrics_text(&snapshot, bytes, w.started)).into_response()
}

/// The bytes of every file under `dir`: the databases and their WALs.
fn dir_bytes(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(_) => e.metadata().map_or(0, |m| m.len()),
            Err(_) => 0,
        })
        .sum()
}

/// `s` in Prometheus' text format, with what the data takes on disk and
/// when the server started.
pub fn metrics_text(s: &Snapshot, data_bytes: u64, started: u64) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let mut one = |name: &str, kind: &str, help: &str, rows: &[(&str, f64)]| {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        for (labels, v) in rows {
            let _ = writeln!(out, "{name}{labels} {v}");
        }
    };
    let n = |v: u64| v as f64;
    let version = format!("{{version=\"{}\"}}", env!("CARGO_PKG_VERSION"));
    one("carnet_build_info", "gauge", "The server's version, as a label.", &[(&version, 1.0)]);
    one("process_start_time_seconds", "gauge", "When the server started, in seconds since 1970.", &[("", n(started))]);
    one("carnet_accounts", "gauge", "Accounts on this server.", &[("", s.accounts as f64)]);
    one("carnet_data_bytes", "gauge", "What the databases take on disk.", &[("", n(data_bytes))]);
    one("carnet_sockets", "gauge", "Sockets open.", &[("", n(s.sockets))]);
    one("carnet_sockets_logged_in", "gauge", "Sockets of devices logged in.", &[("", n(s.open.logged_in))]);
    let by: Vec<(String, f64)> = s.open.protocols.iter().map(|(p, c)| (format!("{{protocol=\"{p}\"}}"), n(*c))).collect();
    let by: Vec<(&str, f64)> = by.iter().map(|(l, v)| (l.as_str(), *v)).collect();
    one("carnet_sockets_by_protocol", "gauge", "Sockets of devices logged in, by the protocol their app speaks.", &by);
    let by: Vec<(String, f64)> = s.open.apps.iter().map(|((a, v), c)| (format!("{{app=\"{a}\",version=\"{v}\"}}"), n(*c))).collect();
    let by: Vec<(&str, f64)> = by.iter().map(|(l, v)| (l.as_str(), *v)).collect();
    one("carnet_sockets_by_app", "gauge", "Sockets of devices logged in, by the app and its version.", &by);
    let by: Vec<(String, f64)> = s.stats.logins_by_app.iter().map(|((a, v), c)| (format!("{{app=\"{a}\",version=\"{v}\"}}"), n(*c))).collect();
    let by: Vec<(&str, f64)> = by.iter().map(|(l, v)| (l.as_str(), *v)).collect();
    one("carnet_logins_by_app_total", "counter", "Logins let in, by the app and its version.", &by);
    one("carnet_pairings_open", "gauge", "Pairings open, waiting for a new device.", &[("", n(s.open.pairings))]);
    one("carnet_sockets_opened_total", "counter", "Sockets opened.", &[("", n(s.sockets_opened))]);
    one(
        "carnet_sockets_closed_total",
        "counter",
        "Sockets the server closed for its limits: too much waiting to be sent, too long to sign in, nothing heard for too long.",
        &[("{reason=\"budget\"}", n(s.closed_budget)), ("{reason=\"login_timeout\"}", n(s.closed_login)), ("{reason=\"idle\"}", n(s.closed_idle))],
    );
    one("carnet_frames_received_total", "counter", "Frames from devices.", &[("", n(s.frames_in))]);
    one("carnet_received_bytes_total", "counter", "Bytes of frames from devices.", &[("", n(s.bytes_in))]);
    one("carnet_frames_sent_total", "counter", "Frames to devices.", &[("", n(s.frames_out))]);
    one("carnet_sent_bytes_total", "counter", "Bytes of frames to devices.", &[("", n(s.bytes_out))]);
    let st = &s.stats;
    one("carnet_logins_total", "counter", "Logins, let in or refused.", &[("{result=\"ok\"}", n(st.logins)), ("{result=\"refused\"}", n(st.logins_refused))]);
    one("carnet_denied_total", "counter", "Sockets turned away, for any reason.", &[("", n(st.denied))]);
    one("carnet_enrollments_total", "counter", "Invites redeemed for an account, or refused.", &[("{result=\"ok\"}", n(st.enrolled)), ("{result=\"refused\"}", n(st.enroll_refused))]);
    one("carnet_pairings_total", "counter", "Pairings opened.", &[("", n(st.pairings))]);
    one("carnet_recoveries_total", "counter", "Devices let in by recovery words.", &[("", n(st.recoveries))]);
    one("carnet_records_total", "counter", "Sealed records kept.", &[("", n(st.records))]);
    one("carnet_record_bytes_total", "counter", "Bytes of sealed records kept.", &[("", n(st.record_bytes))]);
    one("carnet_batches_refused_total", "counter", "Batches of records refused.", &[("", n(st.batches_refused))]);
    one("carnet_snapshots_total", "counter", "Rooms compacted.", &[("", n(st.snapshots))]);
    one("carnet_blobs_total", "counter", "Sealed pictures kept.", &[("", n(s.blobs_stored))]);
    one("carnet_blob_bytes_total", "counter", "Bytes of sealed pictures kept.", &[("", n(s.blob_bytes))]);
    one("carnet_blob_links_total", "counter", "Links given to put or fetch a picture.", &[("", n(s.blob_links))]);
    one(
        "carnet_blobs_dropped_total",
        "counter",
        "Pictures no note shows any more: marked by a device's word, and gone 30 days on.",
        &[("{step=\"marked\"}", n(s.blobs_dropped)), ("{step=\"gone\"}", n(s.blobs_swept))],
    );
    one(
        "carnet_blob_requests_total",
        "counter",
        "Pictures fetched and put over HTTP, by their links.",
        &[("{way=\"get\"}", n(s.blob_gets)), ("{way=\"put\"}", n(s.blob_puts))],
    );
    one("carnet_storage_failures_total", "counter", "Writes to the databases that failed, pictures' too.", &[("", n(st.storage_failures + s.blob_failures))]);
    one(
        "carnet_limited_total",
        "counter",
        "Turned away for asking too much from one address: sockets refused before opening, and sockets denied for too many tries.",
        &[("{what=\"sockets\"}", n(s.sockets_refused)), ("{what=\"tries\"}", n(st.limited))],
    );
    one("carnet_hub_queue_seconds", "gauge", "How long this scrape waited for the hub: its lag.", &[("", s.waited.as_secs_f64())]);
    let h = &s.handled;
    let _ = writeln!(out, "# HELP carnet_hub_handle_seconds How long the hub took over each thing it was given.\n# TYPE carnet_hub_handle_seconds histogram");
    for (bound, count) in BOUNDS.iter().zip(h.buckets.iter()) {
        let _ = writeln!(out, "carnet_hub_handle_seconds_bucket{{le=\"{bound}\"}} {count}");
    }
    let _ = writeln!(out, "carnet_hub_handle_seconds_bucket{{le=\"+Inf\"}} {}", h.count);
    let _ = writeln!(out, "carnet_hub_handle_seconds_sum {}", h.sum);
    let _ = writeln!(out, "carnet_hub_handle_seconds_count {}", h.count);
    out
}

/// What a picture's link is checked by, and where its file is.
#[derive(Clone)]
struct Links {
    signer: Arc<Signer>,
    files: BlobFiles,
    http: Arc<BlobHttp>,
    /// Whether a put's account is still one: the hub's store says.
    hub: mpsc::UnboundedSender<ToHub>,
}

/// Whether `account` is still an account, as the hub's store says; no if
/// the hub doesn't answer (the server is stopping).
async fn account_here(hub: &mpsc::UnboundedSender<ToHub>, account: Vec<u8>) -> bool {
    let (reply, answer) = oneshot::channel();
    let ask: HubJob = Box::new(move |hub: &mut Hub<Store>| {
        let _ = reply.send(hub.store().has_account(&account));
        Vec::new()
    });
    hub.send(ToHub::Run(ask)).is_ok() && answer.await.unwrap_or(false)
}

/// Pictures over HTTP, for `/metrics`.
#[derive(Default)]
struct BlobHttp {
    gets: AtomicU64,
    puts: AtomicU64,
    failures: AtomicU64,
    stored: AtomicU64,
    stored_bytes: AtomicU64,
    next: AtomicU64,
}

/// A link's grant, or why not: refused alike whatever is wrong with it.
fn granted(l: &Links, token: &str, put: bool) -> Result<crate::links::Grant, (StatusCode, &'static str)> {
    match l.signer.check(token, now()) {
        Ok(g) if g.put == put => Ok(g),
        Ok(_) => Err((StatusCode::FORBIDDEN, "this link is for the other way")),
        Err(why) => Err((StatusCode::FORBIDDEN, why)),
    }
}

/// A sealed picture, by its link: as a file is served, ranges and all,
/// kept by the device's browser no longer than the link is good.
async fn blob_get(State(l): State<Links>, axum::extract::Path(token): axum::extract::Path<String>, req: axum::extract::Request) -> axum::response::Response {
    use tower::ServiceExt;
    let g = match granted(&l, &token, false) {
        Ok(g) => g,
        Err(r) => return r.into_response(),
    };
    let path = l.files.path(&g.account, &g.id);
    if !path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tower_http::services::ServeFile::new(path).oneshot(req).await {
        Ok(res) => {
            l.http.gets.fetch_add(1, Ordering::Relaxed);
            let mut res = res.into_response();
            let age = g.expires.saturating_sub(now());
            if let Ok(v) = format!("private, max-age={age}").parse() {
                res.headers_mut().insert(axum::http::header::CACHE_CONTROL, v);
            }
            // Sealed bytes, from the app's own origin: never taken for a
            // page or a script, opened, or run.
            let h = res.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
            h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
            h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
            h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));
            res
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// A sealed picture put by its link: its length said up front, and the
/// length the link was given for (the device said it when it offered the
/// picture), written to the spool as it comes, then kept whole and on the
/// disk before the 204 that says so. Already kept, it stays as it was.
async fn blob_put(
    State(l): State<Links>,
    axum::extract::Path(token): axum::extract::Path<String>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> axum::response::Response {
    use tokio::io::AsyncWriteExt;
    let g = match granted(&l, &token, true) {
        Ok(g) => g,
        Err(r) => return r.into_response(),
    };
    let Some(len) = headers.get(axum::http::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse::<u64>().ok()) else {
        return StatusCode::LENGTH_REQUIRED.into_response();
    };
    if len > g.max {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    if len != g.max {
        return (StatusCode::BAD_REQUEST, "not the length this link is for").into_response();
    }
    // Its account deleted since the link was given: nothing of it is kept
    // again. (One deleted while the put comes is swept within the hour.)
    if !account_here(&l.hub, g.account.clone()).await {
        return (StatusCode::FORBIDDEN, "this link's account is no more").into_response();
    }
    let n = l.http.next.fetch_add(1, Ordering::Relaxed);
    let tmp = l.files.spool.join(format!("put-{n}"));
    let failed = |l: &Links, tmp: &std::path::Path, status: StatusCode| {
        l.http.failures.fetch_add(1, Ordering::Relaxed);
        let _ = std::fs::remove_file(tmp);
        status.into_response()
    };
    let Ok(mut file) = tokio::fs::OpenOptions::from(crate::private::create()).open(&tmp).await else {
        return failed(&l, &tmp, StatusCode::INTERNAL_SERVER_ERROR);
    };
    let mut got = 0u64;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { return failed(&l, &tmp, StatusCode::BAD_REQUEST) };
        got += chunk.len() as u64;
        if got > len {
            return failed(&l, &tmp, StatusCode::BAD_REQUEST);
        }
        if file.write_all(&chunk).await.is_err() {
            return failed(&l, &tmp, StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    if got != len {
        return failed(&l, &tmp, StatusCode::BAD_REQUEST);
    }
    // On the disk before it's said kept: the 204 is the device's word
    // that the picture is safe, and it names it in its notes on that.
    if file.flush().await.is_err() || file.sync_all().await.is_err() {
        return failed(&l, &tmp, StatusCode::INTERNAL_SERVER_ERROR);
    }
    drop(file);
    let files = l.files.clone();
    let kept = tokio::task::spawn_blocking({
        let (tmp, account) = (tmp.clone(), g.account.clone());
        move || files.keep_file(&account, &g.id, &tmp)
    })
    .await;
    match kept {
        Ok(Ok(new)) => {
            // Put again: wanted, if it was dropped.
            l.files.unmark(&g.account, &g.id);
            l.http.puts.fetch_add(1, Ordering::Relaxed);
            if new {
                l.http.stored.fetch_add(1, Ordering::Relaxed);
                l.http.stored_bytes.fetch_add(len, Ordering::Relaxed);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        _ => failed(&l, &tmp, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn sync_route(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    // A page of another site, in someone's browser, opens no socket here.
    if let Some(page) = headers.get(header::ORIGIN)
        && !origin_allowed(&app.origin, page.to_str().unwrap_or("null"))
    {
        return (StatusCode::FORBIDDEN, "a page of another site may not open this server's socket").into_response();
    }
    // The app's protocol, name and version, in its address; no
    // protocol, a build from before.
    let app_said = compat::Client::from_query(query.as_deref());
    let client = limits::client_of(peer.ip(), header("cf-connecting-ip"), header("x-forwarded-for"), &app.trusted, app.client_header).to_string();
    if !app.gate.lock().unwrap_or_else(|e| e.into_inner()).admit(&client) {
        app.refused.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::TOO_MANY_REQUESTS, limits::TOO_MANY).into_response();
    }
    // Counted open until the socket is done, or the upgrade never comes:
    // dropped either way, the count goes down.
    let held = Held { gate: app.gate.clone(), client: client.clone() };
    let upgraded = ws.max_message_size(MAX_FRAME).max_frame_size(MAX_FRAME).on_upgrade(move |socket| async move {
        let _held = held;
        socket_task(app, socket, client, app_said).await;
    });
    upgraded.into_response()
}

/// Whether a browser's page at `page` (its `Origin`) may open a socket on
/// the server at `own` (its `--origin`): the web app this server serves
/// is at its own origin. Behind a proxy that's the public one; a server
/// on this machine (development, Vite's proxy) takes a page on this
/// machine too, which another site can't be. The apps send no `Origin`,
/// and aren't asked this.
pub fn origin_allowed(own: &str, page: &str) -> bool {
    match (site(own), site(page)) {
        (Some(own), Some(page)) => own == page || (loopback(&own.1) && loopback(&page.1)),
        _ => false,
    }
}

/// An origin's scheme, host and port, as a browser says it: lowercase,
/// the default port said. `None` for `null` or anything not HTTP's.
fn site(origin: &str) -> Option<(String, String, u16)> {
    let origin = origin.trim().to_ascii_lowercase();
    let (scheme, rest) = origin.split_once("://")?;
    let default = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let rest = rest.split(['/', '?', '#']).next()?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (h, p.parse().ok()?),
        _ => (rest, default),
    };
    (!host.is_empty()).then(|| (scheme.to_string(), host.to_string(), port))
}

/// A name or address of this machine's own.
fn loopback(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost" || host.ends_with(".localhost") || bare.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// A socket a client holds open, for the [`Gate`].
struct Held {
    gate: Arc<std::sync::Mutex<Gate>>,
    client: String,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.gate.lock().unwrap_or_else(|e| e.into_inner()).closed(&self.client);
    }
}

/// How much may wait for a socket before its device's next frame waits
/// instead, and before the next part of a room it joined is sent: a
/// frame's worth, or half its budget if that's less.
fn pace(budget: usize) -> usize {
    MAX_FRAME.min(budget / 2)
}

/// What a message costs a socket's queue beside its bytes: the queue's
/// own keeping of it. A pong is four bytes; a million of them waiting
/// are a lot more than four megabytes.
const OVERHEAD: usize = 64;

fn cost(m: &Message) -> usize {
    OVERHEAD
        + match m {
            Message::Binary(b) | Message::Ping(b) | Message::Pong(b) => b.len(),
            Message::Text(t) => t.len(),
            Message::Close(c) => c.as_ref().map_or(0, |c| c.reason.len()),
        }
}

/// One socket's counts, shared by its task, its writer and the hub.
#[derive(Default)]
struct Meter {
    /// What waits to be written to it ([`cost`]).
    queued: AtomicUsize,
    /// What it gave the hub that the hub hasn't done yet.
    given: AtomicUsize,
    /// Past its budget: nothing more is queued, and it closes.
    over: AtomicBool,
    /// Logged in: no login timeout.
    ready: AtomicBool,
    /// The hub has more of a room for it, to be asked for once what waits
    /// for it is under its pace (`ToHub::Drained`): whichever sees both
    /// first, the hub's thread or its writer, takes it.
    more: AtomicBool,
    /// Its task woken: something above changed.
    changed: Notify,
}

impl Meter {
    /// Written.
    fn sent(&self, cost: usize) {
        self.queued.fetch_sub(cost, Ordering::AcqRel);
        self.changed.notify_one();
    }

    /// Done by the hub.
    fn handled(&self, cost: usize) {
        self.given.fetch_sub(cost, Ordering::AcqRel);
        self.changed.notify_one();
    }

    /// Whether its device's next frame is read: the hub has done all it
    /// was given, and (nearly) all it sent is written.
    fn caught_up(&self, pace: usize) -> bool {
        self.given.load(Ordering::Acquire) == 0 && self.queued.load(Ordering::Acquire) < pace
    }
}

/// A socket's way out, for the hub and its own task: messages queued, up
/// to its budget.
#[derive(Clone)]
struct Line {
    to: mpsc::UnboundedSender<Message>,
    meter: Arc<Meter>,
    budget: usize,
}

impl Line {
    /// The hub has more for it: whether to ask for it now (what waits is
    /// under its pace), or its writer is to once it is.
    fn wants_more(&self) -> bool {
        self.meter.more.store(true, Ordering::SeqCst);
        self.meter.queued.load(Ordering::SeqCst) < pace(self.budget) && self.meter.more.swap(false, Ordering::SeqCst)
    }

    /// `m` queued, whether it was. Past the budget it isn't, nor is
    /// anything after, and the socket's task closes the socket.
    fn send(&self, m: Message) -> bool {
        let meter = &self.meter;
        if meter.over.load(Ordering::Acquire) {
            return false;
        }
        let cost = cost(&m);
        if meter.queued.fetch_add(cost, Ordering::AcqRel) + cost > self.budget {
            meter.over.store(true, Ordering::Release);
            meter.changed.notify_one();
            return false;
        }
        if self.to.send(m).is_err() {
            meter.queued.fetch_sub(cost, Ordering::AcqRel);
            return false;
        }
        true
    }
}

/// Why a socket's task stops reading it.
enum End {
    /// The device closed it, or it broke.
    Gone,
    /// Past its budget: it isn't reading, so nothing more is sent.
    Over,
    /// Too long to log in, or nothing heard: it's told, briefly.
    Late(&'static str),
}

/// What a socket closed for taking too long is told.
const LOGIN_LATE: &str = "took too long to sign in";
const QUIET: &str = "nothing heard for too long";

/// One device's socket: frames in to the hub, the hub's frames out.
///
/// Its next frame goes to the hub only once the hub has done the last,
/// and what came of it is mostly written; one more is held meanwhile, and
/// nothing more read. So a device that asks without reading is held to
/// its own pace, and the hub's queue holds a frame per socket at most (it
/// needs no bound of its own, and the hub never waits on a socket). Past
/// [`SocketLimits::queue`] (pongs to pings never read, the others' writes
/// piling up unread, one answer too big), the socket closes.
async fn socket_task(app: App, socket: WebSocket, client: String, said: compat::Client) {
    let conn = app.next.fetch_add(1, Ordering::Relaxed);
    let limits = app.sockets;
    let (to, mut from_hub) = mpsc::unbounded_channel::<Message>();
    let meter = Arc::new(Meter::default());
    let line = Line { to, meter: meter.clone(), budget: limits.queue };
    let pace = pace(limits.queue);
    if app.hub.send(ToHub::Open { conn, to: line.clone(), client, app: said }).is_err() {
        return;
    }
    let (mut sink, mut stream) = socket.split();
    let mut writer = tokio::spawn({
        let meter = meter.clone();
        let hub = app.hub.clone();
        async move {
            while let Some(m) = from_hub.recv().await {
                let (cost, closing) = (cost(&m), matches!(m, Message::Close(_)));
                let sent = sink.send(m).await;
                meter.sent(cost);
                if sent.is_err() || closing {
                    break;
                }
                // Written down to its pace with more of a room to come: the
                // next part asked for.
                if meter.more.load(Ordering::SeqCst) && meter.queued.load(Ordering::SeqCst) < pace && meter.more.swap(false, Ordering::SeqCst) {
                    let _ = hub.send(ToHub::Drained { conn });
                }
            }
        }
    });
    let opened = tokio::time::Instant::now();
    let mut heard = opened;
    let mut pinged = false;
    // A frame read while the hub or the writer was behind.
    let mut held: Option<Vec<u8>> = None;
    let give = |bytes: Vec<u8>| {
        meter.given.fetch_add(bytes.len() + OVERHEAD, Ordering::AcqRel);
        app.hub.send(ToHub::Frame { conn, bytes }).is_ok()
    };
    let end = loop {
        if meter.over.load(Ordering::Acquire) {
            break End::Over;
        }
        if held.is_some() && meter.caught_up(pace) && !held.take().is_some_and(give) {
            break End::Gone;
        }
        let idle = (heard + limits.idle, QUIET);
        let (deadline, why) = match opened + limits.login {
            login if !meter.ready.load(Ordering::Acquire) && login < idle.0 => (login, LOGIN_LATE),
            _ => idle,
        };
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break End::Late(why),
            _ = tokio::time::sleep_until(heard + limits.ping), if !pinged => {
                pinged = true;
                line.send(Message::Ping(Default::default()));
            }
            // Written, done, over, or logged in: looked at again.
            _ = meter.changed.notified() => {}
            m = stream.next(), if held.is_none() => {
                let Some(Ok(m)) = m else { break End::Gone };
                heard = tokio::time::Instant::now();
                pinged = false;
                match m {
                    Message::Binary(b) if meter.caught_up(pace) => {
                        if !give(b.to_vec()) {
                            break End::Gone;
                        }
                    }
                    Message::Binary(b) => held = Some(b.to_vec()),
                    // The Loro Protocol's keepalive: a text "ping", a text
                    // "pong", queued like anything else.
                    Message::Text(t) if t.as_str() == "ping" => {
                        line.send(Message::Text("pong".into()));
                    }
                    Message::Close(_) => break End::Gone,
                    _ => {}
                }
            }
        }
    };
    let _ = app.hub.send(ToHub::Closed { conn });
    let counted = match end {
        End::Over => Some(&app.closes.budget),
        End::Late(LOGIN_LATE) => Some(&app.closes.login),
        End::Late(_) => Some(&app.closes.idle),
        End::Gone => None,
    };
    if let Some(c) = counted {
        c.fetch_add(1, Ordering::Relaxed);
    }
    match end {
        End::Over => writer.abort(),
        End::Late(why) => {
            line.send(Message::Close(Some(CloseFrame { code: close_code::POLICY, reason: why.into() })));
        }
        End::Gone => {}
    }
    drop(line);
    // What waits is written, a moment allowed; a device that doesn't
    // take it is let go.
    if tokio::time::timeout(Duration::from_secs(5), &mut writer).await.is_err() {
        writer.abort();
    }
}
