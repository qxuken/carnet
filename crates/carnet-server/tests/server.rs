//! The server binary's own path, end to end: a real listener, HTTP for
//! enrolling, WebSockets for syncing, SQLite on disk, a restart in the
//! middle. The devices are `Session`s, driven over real sockets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use carnet_server::server::{self, Config};
use carnet_server::Sharded;
use carnet_sync::crypto::{DeviceKey, OsRandom, VaultKey, Ring};
use carnet_sync::pair::{Joiner, Joining, PairCode};
use carnet_sync::session::{Docs, Event, Identity, Session};
use carnet_sync::{NoteDoc, Origin};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

const ROOM: &str = "00000000000000000000000000000abc";

struct Notes {
    peer: u64,
    docs: BTreeMap<String, NoteDoc>,
}

impl Docs for Notes {
    fn rooms(&self) -> Vec<String> {
        self.docs.keys().cloned().collect()
    }
    fn doc(&mut self, room: &str) -> &NoteDoc {
        self.docs.entry(room.to_string()).or_insert_with(|| NoteDoc::new(self.peer))
    }
}

struct Running {
    addr: std::net::SocketAddr,
    stop: oneshot::Sender<()>,
    done: tokio::task::JoinHandle<std::io::Result<()>>,
}

async fn start(data: &Path, origin: Option<&str>) -> Running {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, rx) = oneshot::channel::<()>();
    let config = Config { data: data.to_path_buf(), origin: origin.map_or_else(|| format!("http://{addr}"), str::to_string), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new() };
    let done = tokio::spawn(server::serve(config, listener, async {
        let _ = rx.await;
    }));
    Running { addr, stop, done }
}

impl Running {
    async fn stop(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(10), self.done).await.expect("stops").unwrap().unwrap();
    }
}

/// An invite used over the socket, as the apps use one: the new account,
/// or why the server refused.
async fn enroll(addr: std::net::SocketAddr, invite: &str, device: &[u8], public: [u8; 32]) -> Result<Vec<u8>, String> {
    use carnet_sync::wire::{self, Carnet, Frame};
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/sync?protocol={}", carnet_sync::compat::PROTOCOL)).await.unwrap();
    let enroll = Frame::Carnet(Carnet::Enroll { invite: invite.to_string(), device: device.to_vec(), public });
    ws.send(Message::Binary(wire::encode(&enroll).into())).await.unwrap();
    while let Some(Ok(m)) = ws.next().await {
        let Message::Binary(b) = m else { continue };
        match wire::decode(&b) {
            Ok(Frame::Carnet(Carnet::Enrolled { account })) => return Ok(account),
            Ok(Frame::Carnet(Carnet::Denied { reason, .. })) => return Err(reason),
            _ => {}
        }
    }
    Err("the socket closed".into())
}

struct Device {
    session: Session,
    notes: Notes,
    events: Vec<Event>,
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

impl Device {
    fn new(identity: Identity, peer: u64) -> Self {
        Device { session: Session::new(identity, 0), notes: Notes { peer, docs: BTreeMap::new() }, events: Vec::new() }
    }

    async fn connect(&mut self, addr: std::net::SocketAddr) -> Socket {
        let (ws, _) = tokio_tungstenite::connect_async(carnet_sync::compat::with_app(&format!("ws://{addr}/v1/sync"), "test-app", "1.2.3")).await.unwrap();
        self.session.disconnected();
        self.session.connected();
        ws
    }

    /// Frames both ways until the session is idle and the socket quiet
    /// (or the socket closes).
    async fn settle(&mut self, ws: &mut Socket) {
        loop {
            for out in self.session.take_out() {
                ws.send(Message::Binary(out.into())).await.unwrap();
            }
            match tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
                Ok(Some(Ok(Message::Binary(b)))) => {
                    self.session.receive(&b, &mut self.notes, &mut OsRandom);
                    self.events.extend(self.session.take_events());
                }
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return,
                Ok(Some(Ok(_))) => {}
                Err(_) if self.session.is_idle() || !self.session.is_ready() => return,
                Err(_) => {}
            }
        }
    }

    fn write(&mut self, text: &str) {
        let doc = self.notes.doc(ROOM);
        doc.record(text, Origin::Save);
        self.session.saved(ROOM, &mut self.notes, &mut OsRandom);
    }

    fn text(&self) -> Option<String> {
        self.notes.docs.get(ROOM).map(NoteDoc::text)
    }
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("carnet-server-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_devices_sync_through_the_binary_and_a_restart() {
    let data = temp("e2e");
    std::fs::create_dir_all(&data).unwrap();
    let code = {
        let mut db = Sharded::open(&data).unwrap();
        server::new_invite(&mut db).unwrap()
    };
    let srv = start(&data, None).await;
    let origin = format!("http://{}", srv.addr);

    // Enrol device A with the invite: a new account.
    let a_key = DeviceKey::generate(&mut OsRandom);
    let account = enroll(srv.addr, &code.text().to_lowercase(), b"\x0a", a_key.public()).await.expect("enrolled");
    // The invite is spent.
    assert!(enroll(srv.addr, &code.text(), b"\x0a", a_key.public()).await.is_err_and(|e| e.contains("not good")));

    let vault = VaultKey::generate(&mut OsRandom);
    let id = |device: &[u8], key: DeviceKey| Identity {
        origin: origin.clone(),
        account: account.clone(),
        device: device.to_vec(),
        key,
        ring: Ring::new(vault.clone()),
        name: String::new(),
    };
    let mut a = Device::new(id(b"\x0a", a_key), 1);
    let mut ws_a = a.connect(srv.addr).await;
    a.settle(&mut ws_a).await;
    assert!(a.events.contains(&Event::LoggedIn), "{:?}", a.events);
    a.write("# Shop\n\n- [ ] milk\n");
    a.settle(&mut ws_a).await;

    // Device B, paired from A over the sockets: A shows a code, B types it.
    a.session.pair_open(&mut OsRandom);
    a.settle(&mut ws_a).await;
    let code = a.events.iter().find_map(|e| match e {
        Event::PairCode(c) => PairCode::parse(c),
        _ => None,
    });
    let b_key = DeviceKey::generate(&mut OsRandom);
    let mut joiner = Joiner::new(code.expect("A shows a code"), b"\x0b".to_vec(), b_key.clone());
    let (mut ws_b, _) = tokio_tungstenite::connect_async(format!("ws://{}/v1/sync?protocol={}", srv.addr, carnet_sync::compat::PROTOCOL)).await.unwrap();
    // Both sockets at once, until B is let in.
    for _ in 0..200 {
        if !matches!(joiner.state, Joining::Going) {
            break;
        }
        for out in a.session.take_out() {
            ws_a.send(Message::Binary(out.into())).await.unwrap();
        }
        for out in joiner.take_out() {
            ws_b.send(Message::Binary(out.into())).await.unwrap();
        }
        tokio::select! {
            Some(Ok(Message::Binary(m))) = ws_a.next() => {
                a.session.receive(&m, &mut a.notes, &mut OsRandom);
                a.events.extend(a.session.take_events());
            }
            Some(Ok(Message::Binary(m))) = ws_b.next() => joiner.receive(&m, &mut OsRandom),
            _ = tokio::time::sleep(Duration::from_millis(300)) => {}
        }
    }
    let Joining::In(admitted) = joiner.state else { panic!("B was not let in: {:?}", joiner.state) };
    assert_eq!(admitted.account, account);
    assert!(a.events.contains(&Event::Paired(b"\x0b".to_vec())));
    // B is a device now: same socket, the challenge it was handed answered.
    let mut b = Device::new(
        Identity { origin: origin.clone(), account: admitted.account, device: b"\x0b".to_vec(), key: b_key.clone(), ring: admitted.ring, name: String::new() },
        2,
    );
    b.session.connected();
    b.session.receive(&admitted.challenge, &mut b.notes, &mut OsRandom);
    b.settle(&mut ws_b).await;
    assert_eq!(b.text().as_deref(), Some("# Shop\n\n- [ ] milk\n"), "B, paired, found the note by the feed, and got it");

    // The server stops and starts again on the same disk.
    drop(ws_a);
    drop(ws_b);
    srv.stop().await;
    let srv = start(&data, Some(&origin)).await;
    // (Same origin as before: the devices sign for the URL they know.)
    let srv_addr = srv.addr;
    let mut ws_a = a.connect(srv_addr).await;
    a.settle(&mut ws_a).await;
    let mut ws_b = b.connect(srv_addr).await;
    b.settle(&mut ws_b).await;

    // Live: A writes, B has it.
    a.write("# Shop\n\n- [ ] milk\n- [ ] eggs\n");
    a.settle(&mut ws_a).await;
    b.settle(&mut ws_b).await;
    assert_eq!(b.text(), a.text());
    assert!(b.text().unwrap().contains("eggs"));

    // A device that signs for another origin is turned away.
    let mut wrong = Device::new(Identity { origin: "https://elsewhere.example".into(), ..id(b"\x0b", b_key) }, 3);
    let mut ws_w = wrong.connect(srv_addr).await;
    wrong.settle(&mut ws_w).await;
    assert!(wrong.events.iter().any(|e| matches!(e, Event::Denied(_))), "{:?}", wrong.events);

    // What is on disk is sealed: every file of the data folder, the
    // account's database and its WAL among them.
    let account_db = Sharded::account_path(&data, &account).unwrap();
    assert!(account_db.exists(), "the account has a database of its own");
    let mut files = vec![data.clone()];
    let mut read = 0;
    while let Some(p) = files.pop() {
        if p.is_dir() {
            files.extend(std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()));
        } else {
            let bytes = std::fs::read(&p).unwrap_or_default();
            read += 1;
            assert!(!bytes.windows(4).any(|w| w == b"milk" || w == b"eggs"), "a note in the clear in {}", p.display());
        }
    }
    assert!(read >= 3);
    drop(ws_a);
    drop(ws_b);
    drop(ws_w);
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&data);
}

/// A GET by hand: the status, the headers (lower-cased) and the body.
async fn get(addr: std::net::SocketAddr, path: &str, extra: &str) -> (u16, String, Vec<u8>) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n{extra}Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&out[..split]).to_lowercase();
    (head[9..12].parse().unwrap(), head, out[split + 4..].to_vec())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_web_viewer_is_served_beside_the_socket() {
    let dir = temp("web");
    let web = dir.join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>viewer").unwrap();
    std::fs::write(web.join("assets/app.js"), "plain").unwrap();
    std::fs::write(web.join("assets/app.js.br"), "squeezed").unwrap();
    std::fs::write(web.join("sw.js"), "worker").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, rx) = oneshot::channel::<()>();
    let config = Config { data: dir.join("data"), origin: format!("http://{addr}"), compact_after: server::COMPACT_AFTER, web: Some(web), trusted: Vec::new() };
    let done = tokio::spawn(server::serve(config, listener, async {
        let _ = rx.await;
    }));
    let running = Running { addr, stop, done };

    let (status, head, body) = get(addr, "/", "").await;
    assert_eq!((status, body.as_slice()), (200, b"<!doctype html>viewer".as_slice()));
    // The app's headers: its CSP (its socket named), no referrer, nothing
    // sniffed, and its page asked for afresh each time.
    let csp = format!(
        "content-security-policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; connect-src 'self' ws://{addr}; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; worker-src 'self'; manifest-src 'self'\r\n"
    );
    for want in [csp.as_str(), "referrer-policy: no-referrer\r\n", "x-content-type-options: nosniff\r\n", "cache-control: no-cache\r\n"] {
        assert!(head.contains(want), "{want}: {head}");
    }
    // A note's address is the app's to read: the page, not a 404.
    let (status, head, body) = get(addr, "/note/abc", "").await;
    assert_eq!((status, body.as_slice()), (200, b"<!doctype html>viewer".as_slice()));
    assert!(head.contains(&csp) && head.contains("cache-control: no-cache"), "{head}");
    // The service worker afresh too; the build's files as they are.
    let (_, head, _) = get(addr, "/sw.js", "").await;
    assert!(head.contains(&csp) && head.contains("cache-control: no-cache"), "{head}");
    let (_, head, _) = get(addr, "/assets/app.js", "").await;
    assert!(head.contains(&csp) && head.contains("x-content-type-options: nosniff") && !head.contains("cache-control"), "{head}");
    // The Brotli copy goes to a browser that takes it, the file to one that doesn't.
    let (_, head, body) = get(addr, "/assets/app.js", "Accept-Encoding: br, gzip\r\n").await;
    assert!(head.contains("content-encoding: br"), "{head}");
    assert_eq!(body, b"squeezed");
    let (_, head, body) = get(addr, "/assets/app.js", "").await;
    assert!(!head.contains("content-encoding"), "{head}");
    assert_eq!(body, b"plain");
    // The API is still the API, and an address it doesn't have isn't the page.
    let (status, head, body) = get(addr, "/v1/info", "").await;
    assert_eq!(status, 200);
    assert!(head.contains("x-content-type-options: nosniff") && !head.contains("content-security-policy"), "{head}");
    let info = String::from_utf8_lossy(&body);
    assert!(info.contains("carnet-server") && !info.contains("accounts"), "how many use it isn't public: {info}");
    // An invite is used over the socket only.
    assert_eq!(get(addr, "/v1/enroll", "").await.0, 404);
    assert_eq!(get(addr, "/v1/nothing", "").await.0, 404);

    running.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// The value of `name` (labels and all) in a `/metrics` page.
fn metric(page: &str, name: &str) -> f64 {
    page.lines()
        .find_map(|l| l.strip_prefix(name).and_then(|rest| rest.strip_prefix(' ')))
        .unwrap_or_else(|| panic!("{name} not in:\n{page}"))
        .parse()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_and_metrics_say_how_the_server_is() {
    let data = temp("metrics");
    std::fs::create_dir_all(&data).unwrap();
    let code = {
        let mut db = Sharded::open(&data).unwrap();
        server::new_invite(&mut db).unwrap()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let metrics = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let maddr = metrics.local_addr().unwrap();
    let (stop, rx) = oneshot::channel::<()>();
    let config = Config { data: data.clone(), origin: format!("http://{addr}"), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new() };
    let done = tokio::spawn(server::serve_with(config, listener, Some(metrics), async {
        let _ = rx.await;
    }));

    let (status, _, body) = get(addr, "/v1/health", "").await;
    assert_eq!((status, String::from_utf8_lossy(&body).as_ref()), (200, r#"{"ok":true}"#));
    // The numbers are on their own listener, not the public one.
    assert_eq!(get(addr, "/metrics", "").await.0, 404);

    // An account by invite, and the spent invite refused.
    let key = DeviceKey::generate(&mut OsRandom);
    let account = enroll(addr, &code.text(), b"\x0a", key.public()).await.expect("enrolled");
    assert!(enroll(addr, &code.text(), b"\x0a", key.public()).await.is_err(), "spent");

    // A device logs in and writes a note.
    let identity = Identity {
        origin: format!("http://{addr}"),
        account: account.clone(),
        device: b"\x0a".to_vec(),
        key,
        ring: Ring::new(VaultKey::generate(&mut OsRandom)),
        name: String::new(),
    };
    let mut a = Device::new(identity, 1);
    let mut ws = a.connect(addr).await;
    a.settle(&mut ws).await;
    a.write("# Shop\n\n- [ ] milk\n");
    a.settle(&mut ws).await;
    // And a stranger is turned away.
    let stranger = Identity {
        origin: format!("http://{addr}"),
        account,
        device: b"\x0f".to_vec(),
        key: DeviceKey::generate(&mut OsRandom),
        ring: Ring::new(VaultKey::generate(&mut OsRandom)),
        name: String::new(),
    };
    let mut s = Device::new(stranger, 2);
    let mut ws_s = s.connect(addr).await;
    s.settle(&mut ws_s).await;

    let (status, head, page) = get(maddr, "/metrics", "").await;
    let page = String::from_utf8(page).unwrap();
    assert_eq!(status, 200);
    assert!(head.contains("content-type: text/plain; version=0.0.4"), "{head}");
    assert_eq!(metric(&page, "carnet_accounts"), 1.0);
    assert_eq!(metric(&page, r#"carnet_enrollments_total{result="ok"}"#), 1.0);
    assert_eq!(metric(&page, r#"carnet_enrollments_total{result="refused"}"#), 1.0);
    assert_eq!(metric(&page, r#"carnet_logins_total{result="ok"}"#), 1.0);
    assert_eq!(metric(&page, r#"carnet_logins_total{result="refused"}"#), 1.0);
    assert_eq!(metric(&page, "carnet_sockets_logged_in"), 1.0);
    assert_eq!(metric(&page, &format!("carnet_sockets_by_protocol{{protocol=\"{}\"}}", carnet_sync::compat::PROTOCOL)), 1.0);
    assert_eq!(metric(&page, r#"carnet_sockets_by_app{app="test-app",version="1.2.3"}"#), 1.0);
    assert!(metric(&page, r#"carnet_logins_by_app_total{app="test-app",version="1.2.3"}"#) >= 1.0);
    assert!(metric(&page, "carnet_records_total") >= 1.0, "{page}");
    assert!(metric(&page, "carnet_record_bytes_total") > 0.0);
    assert!(metric(&page, "carnet_frames_received_total") >= 3.0);
    assert!(metric(&page, "carnet_sent_bytes_total") > 0.0);
    assert!(metric(&page, "carnet_data_bytes") > 0.0);
    assert_eq!(metric(&page, "carnet_storage_failures_total"), 0.0);
    assert!(metric(&page, "carnet_hub_handle_seconds_count") >= 3.0);
    assert_eq!(metric(&page, r#"carnet_hub_handle_seconds_bucket{le="+Inf"}"#), metric(&page, "carnet_hub_handle_seconds_count"));
    assert!(metric(&page, "carnet_hub_queue_seconds") < 1.0);
    assert_eq!(metric(&page, &format!("carnet_build_info{{version=\"{}\"}}", env!("CARGO_PKG_VERSION"))), 1.0);
    // Nothing of what the note said.
    assert!(!page.contains("milk") && !page.contains("Shop"));

    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.expect("stops").unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&data);
}

/// Sockets from one address are limited before they open, by the address
/// a trusted proxy says each is for: thirty at once from one, then 429;
/// another, behind the same proxy, still opens; `/metrics` counts it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sockets_from_one_address_are_limited_behind_a_trusted_proxy() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let data = temp("limits");
    std::fs::create_dir_all(&data).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let metrics = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let maddr = metrics.local_addr().unwrap();
    let (stop, rx) = oneshot::channel::<()>();
    let trusted = carnet_server::limits::networks("127.0.0.1").unwrap();
    let config = Config { data: data.clone(), origin: format!("http://{addr}"), compact_after: server::COMPACT_AFTER, web: None, trusted };
    let done = tokio::spawn(server::serve_with(config, listener, Some(metrics), async {
        let _ = rx.await;
    }));
    let open = |from: &'static str| async move {
        let mut req = format!("ws://{addr}/v1/sync?protocol={}", carnet_sync::compat::PROTOCOL).into_client_request().unwrap();
        req.headers_mut().insert("x-forwarded-for", from.parse().unwrap());
        tokio_tungstenite::connect_async(req).await
    };
    let mut held = Vec::new();
    for _ in 0..30 {
        held.push(open("198.51.100.7").await.expect("within the limit").0);
    }
    let refused = open("198.51.100.7").await.expect_err("the thirty-first");
    assert!(refused.to_string().contains("429"), "{refused}");
    assert!(open("203.0.113.9").await.is_ok(), "another address behind the proxy");
    let (_, _, page) = get(maddr, "/metrics", "").await;
    let page = String::from_utf8(page).unwrap();
    assert_eq!(metric(&page, r#"carnet_limited_total{what="sockets"}"#), 1.0);
    drop(held);
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.expect("stops").unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&data);
}

/// An HTTP request by hand, with a body: the status, the headers (lower
/// cased) and the body.
async fn request(addr: std::net::SocketAddr, method: &str, path: &str, extra: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{extra}Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    // A request refused before its body is read: the server may close
    // with the body unread, and the socket be reset. What came before is
    // the answer.
    let _ = s.write_all(body).await;
    let mut out = Vec::new();
    let mut buf = [0u8; 65536];
    loop {
        match s.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
        }
    }
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&out[..split]).to_lowercase();
    (head[9..12].parse().unwrap(), head, out[split + 4..].to_vec())
}

/// A link's path on this server.
fn path_of(url: &str) -> String {
    format!("/{}", url.split_once("://").unwrap().1.split_once('/').unwrap().1)
}

/// A picture over HTTP, by the links the socket gives: offered, put at
/// its link, kept on disk by its 204 (the device's word); fetched by its link,
/// whole, ranges too. A link the other way, changed, or past its size
/// gets nowhere; the file on disk is sealed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_picture_goes_by_its_links_over_http() {
    use carnet_sync::session::Transfer;
    let data = temp("links");
    std::fs::create_dir_all(&data).unwrap();
    let code = server::new_invite(&mut Sharded::open(&data).unwrap()).unwrap();
    let srv = start(&data, None).await;
    let key = DeviceKey::generate(&mut OsRandom);
    let account = enroll(srv.addr, &code.text(), b"\x0a", key.public()).await.unwrap();
    let identity = Identity {
        origin: format!("http://{}", srv.addr),
        account: account.clone(),
        device: b"\x0a".to_vec(),
        key,
        ring: Ring::new(VaultKey::generate(&mut OsRandom)),
        name: String::new(),
    };
    let mut a = Device::new(identity, 1);
    let mut ws = a.connect(srv.addr).await;
    a.settle(&mut ws).await;

    let pic: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let id = a.session.blob_id(&pic);
    a.session.offer_blobs(vec![(id, pic.len())]);
    a.settle(&mut ws).await;
    assert!(a.events.contains(&Event::BlobWanted(id)), "{:?}", a.events);
    assert!(a.session.put_blob(&pic, &mut OsRandom));
    let [Transfer::Put { url, sealed, .. }] = &a.session.take_transfers()[..] else { panic!("a put") };
    let (put_path, sealed) = (path_of(url), sealed.clone());

    // The put: refused without its length, past its size, of another
    // length than the device said in its offer, or by a changed link;
    // then done.
    let len = |n: usize| format!("Content-Length: {n}\r\n");
    assert_eq!(request(srv.addr, "PUT", &put_path, "", b"").await.0, 411);
    assert_eq!(request(srv.addr, "PUT", &put_path, &len(64 << 20), b"").await.0, 413);
    let longer = [sealed.as_slice(), b"x"].concat();
    assert_eq!(request(srv.addr, "PUT", &put_path, &len(longer.len()), &longer).await.0, 413);
    let shorter = &sealed[..sealed.len() - 1];
    assert_eq!(request(srv.addr, "PUT", &put_path, &len(shorter.len()), shorter).await.0, 400);
    let mut changed = put_path.clone().into_bytes();
    let last = changed.len() - 1;
    changed[last] = if changed[last] == b'0' { b'1' } else { b'0' };
    assert_eq!(request(srv.addr, "PUT", std::str::from_utf8(&changed).unwrap(), &len(sealed.len()), &sealed).await.0, 403);
    assert_eq!(request(srv.addr, "GET", &put_path, "", b"").await.0, 403, "a put link doesn't fetch");
    assert_eq!(request(srv.addr, "PUT", &put_path, &len(sealed.len()), &sealed).await.0, 204);
    let files: Vec<_> = walk(&data.join("blobs"));
    assert_eq!(files.len(), 1);
    let on_disk = std::fs::read(&files[0]).unwrap();
    assert_eq!(on_disk, sealed);
    assert!(!on_disk.windows(64).any(|w| w == &pic[1000..1064]), "sealed on disk");
    assert!(std::fs::read_dir(data.join("spool")).unwrap().next().is_none(), "nothing left in the spool");
    // The 204 is the word: kept, with nothing more said on the socket.
    a.events.clear();
    a.session.transferred(id, true, Ok(Vec::new()));
    assert_eq!(a.session.take_events(), [Event::BlobStored(id)]);
    assert!(a.session.take_out().is_empty());
    // A put that didn't go: failed, to be offered again.
    a.session.transferred(id, true, Err("reset".into()));
    assert_eq!(a.session.take_events(), [Event::BlobFailed(id)]);

    // Offered again: there already.
    a.session.offer_blobs(vec![(id, pic.len())]);
    a.settle(&mut ws).await;
    assert_eq!(a.events.last(), Some(&Event::BlobStored(id)), "there already");

    // Fetched by its link: whole, and a range of it.
    a.events.clear();
    a.session.get_blob(id);
    a.settle(&mut ws).await;
    let [Transfer::Get { url, .. }] = &a.session.take_transfers()[..] else { panic!("a get") };
    let get_path = path_of(url);
    assert_eq!(request(srv.addr, "PUT", &get_path, &len(sealed.len()), &sealed).await.0, 403, "a get link doesn't put");
    let (status, head, body) = request(srv.addr, "GET", &get_path, "", b"").await;
    assert_eq!(status, 200);
    assert!(head.contains("cache-control: private, max-age="), "{head}");
    // Bytes from the app's own origin: never sniffed, shown or run.
    for want in ["content-type: application/octet-stream\r\n", "x-content-type-options: nosniff\r\n", "content-disposition: attachment\r\n", "content-security-policy: sandbox\r\n"] {
        assert!(head.contains(want), "{want}: {head}");
    }
    let (status, _, part) = request(srv.addr, "GET", &get_path, "Range: bytes=10-19\r\n", b"").await;
    assert_eq!((status, part.as_slice()), (206, &sealed[10..20]));
    a.session.transferred(id, false, Ok(body));
    assert_eq!(a.session.take_events(), [Event::BlobGot(id, pic)]);

    // The link key: made once, kept to the server.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(data.join("links.key")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&data);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.path().is_dir() {
            out.extend(walk(&e.path()));
        } else {
            out.push(e.path());
        }
    }
    out
}

/// Pictures a server kept in its databases before move out into files
/// on its next start, once, sealed as they were.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pictures_kept_in_a_database_move_to_files_on_start() {
    use carnet_server::Storage;
    let data = temp("move");
    {
        let mut db = Sharded::open(&data).unwrap();
        db.add_account(b"acct", true).unwrap();
        db.add_device(b"acct", b"d", [1; 32]).unwrap();
    }
    let file = Sharded::account_path(&data, b"acct").unwrap();
    let conn = rusqlite::Connection::open(&file).unwrap();
    conn.execute("INSERT INTO blobs (id, bytes) VALUES (?1, ?2)", rusqlite::params![[7u8; 32].as_slice(), b"SEALED-OLD".as_slice()]).unwrap();
    drop(conn);
    let srv = start(&data, None).await;
    // Up (it moves them before it serves).
    assert_eq!(get(srv.addr, "/v1/health", "").await.0, 200);
    let moved = data.join("blobs").join(b"acct".iter().map(|b| format!("{b:02x}")).collect::<String>()).join("07".repeat(32));
    assert_eq!(std::fs::read(&moved).unwrap(), b"SEALED-OLD");
    srv.stop().await;
    let left: i64 = rusqlite::Connection::open(&file).unwrap().query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 0, "out of the database");
    // A second start moves nothing, and keeps the file.
    let srv = start(&data, None).await;
    assert_eq!(get(srv.addr, "/v1/health", "").await.0, 200);
    assert!(moved.exists());
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&data);
}

/// A host's server (a hosted service, docs/SYNC.md §11): its routes beside
/// the API, `/` its own (a landing page) while unknown API paths are still
/// 404; no first invite printed; its work done on the hub by the handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_adds_routes_and_works_on_the_hub() {
    use axum::routing::get;
    let data = temp("hosted");
    std::fs::create_dir_all(&data).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (handle_tx, handle_rx) = oneshot::channel();
    type Admitted = Vec<([u8; 32], Vec<u8>)>;
    struct Seen(std::sync::Arc<std::sync::Mutex<Admitted>>);
    impl carnet_server::quota::Admission for Seen {
        fn admitted(&self, invite: [u8; 32], account: &[u8]) {
            self.0.lock().unwrap().push((invite, account.to_vec()));
        }
    }
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let host = server::Host {
        routes: axum::Router::new().route("/", get(|| async { "landing" })).route("/v1/signup", get(|| async { "signup" })),
        first_invite: false,
        signup: Some("https://notes.example/start".into()),
        admission: std::sync::Arc::new(Seen(seen.clone())),
        ready: Some(Box::new(move |h| {
            let _ = handle_tx.send(h);
        })),
        ..server::Host::default()
    };
    let (stop, rx) = oneshot::channel::<()>();
    let config = Config { data: data.clone(), origin: format!("http://{addr}"), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new() };
    let done = tokio::spawn(server::serve_hosted(config, listener, None, host, async {
        let _ = rx.await;
    }));
    let hub: server::HubHandle = handle_rx.await.unwrap();
    let (status, _, body) = request(addr, "GET", "/", "", b"").await;
    assert_eq!((status, body.as_slice()), (200, b"landing".as_slice()));
    assert_eq!(request(addr, "GET", "/v1/signup", "", b"").await.2, b"signup");
    assert_eq!(request(addr, "GET", "/v1/nothing", "", b"").await.0, 404);
    let info = String::from_utf8_lossy(&request(addr, "GET", "/v1/info", "", b"").await.2).into_owned();
    assert!(info.contains("\"passwords\":false"), "no passwords unless asked: {info}");
    assert!(!info.contains("password_email"), "nor which email they'd take: {info}");
    // Signups, a capability as the others are: listed, and the page said.
    let v: serde_json::Value = serde_json::from_str(&info).unwrap();
    assert!(v["features"].as_array().unwrap().iter().any(|f| f == "signup"), "{info}");
    assert_eq!(v["signup"], "https://notes.example/start");
    assert_eq!(request(addr, "GET", "/v1/health", "", b"").await.0, 200);
    // An invite made by the host, on the hub, used over the socket.
    let code = hub.run(|hub, _| server::invite(hub.store_mut(), false).unwrap()).await.unwrap();
    let key = DeviceKey::generate(&mut OsRandom);
    let account = enroll(addr, &code.text(), b"\x01", key.public()).await.unwrap();
    assert_eq!(hub.run(|hub, _| hub.store().accounts()).await, Some(1));
    assert_eq!(seen.lock().unwrap().as_slice(), [(code.hash(), account.clone())], "told which account the invite made");
    let a = account.clone();
    assert!(!hub.run(move |hub, _| hub.store().is_owner(&a)).await.unwrap(), "a signup is never the owner");
    assert!(!account.is_empty());
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.unwrap().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&data);
}

/// A host's pages beside the web app: its own headers, not the app's CSP;
/// the app's files and any other address, the app's; and the CSP lets
/// the page fetch pictures where the host keeps them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hosts_pages_keep_their_headers_beside_the_app() {
    use axum::routing::get;
    let dir = temp("hosted-web");
    let web = dir.join("web");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>viewer").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let host = server::Host {
        routes: axum::Router::new().route("/start", get(|| async { axum::response::Html("<form method=post>start</form>") })),
        first_invite: false,
        ..server::Host::default()
    };
    let (stop, rx) = oneshot::channel::<()>();
    let config = Config { data: dir.join("data"), origin: "https://notes.example".into(), compact_after: server::COMPACT_AFTER, web: Some(web), trusted: Vec::new() };
    let done = tokio::spawn(server::serve_hosted(config, listener, None, host, async {
        let _ = rx.await;
    }));
    let (status, head, body) = request(addr, "GET", "/start", "", b"").await;
    assert_eq!((status, body.as_slice()), (200, b"<form method=post>start</form>".as_slice()));
    assert!(!head.contains("content-security-policy") && head.contains("x-content-type-options: nosniff"), "{head}");
    let (status, head, _) = request(addr, "GET", "/", "", b"").await;
    assert_eq!(status, 200);
    assert!(head.contains("connect-src 'self' wss://notes.example;"), "{head}");
    // Pictures elsewhere (a bucket): its origin allowed too.
    assert!(server::web_csp("https://notes.example/", Some("https://pics.s3.example")).contains("connect-src 'self' wss://notes.example https://pics.s3.example;"));
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.unwrap().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())).permissions().mode() & 0o777
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What the server keeps is its own user's alone: a data folder left open
/// to others is closed on start, and the folders it makes, its databases
/// and its pictures are made so. A link to put a picture given before its
/// account was deleted puts nothing after: the account's folder isn't
/// made again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_data_is_private_and_a_deleted_accounts_link_puts_nothing() {
    use carnet_sync::session::Transfer;
    let data = temp("private");
    std::fs::create_dir_all(&data).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (handle_tx, handle_rx) = oneshot::channel();
    let host = server::Host {
        first_invite: false,
        ready: Some(Box::new(move |h| {
            let _ = handle_tx.send(h);
        })),
        ..server::Host::default()
    };
    let (stop, rx) = oneshot::channel::<()>();
    let config = Config { data: data.clone(), origin: format!("http://{addr}"), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new() };
    let done = tokio::spawn(server::serve_hosted(config, listener, None, host, async {
        let _ = rx.await;
    }));
    let hub: server::HubHandle = handle_rx.await.unwrap();
    #[cfg(unix)]
    {
        assert_eq!(mode(&data), 0o700, "closed on start");
        for dir in ["accounts", "blobs", "spool"] {
            assert_eq!(mode(&data.join(dir)), 0o700, "{dir}");
        }
        for file in ["carnet.db", "links.key"] {
            assert_eq!(mode(&data.join(file)), 0o600, "{file}");
        }
    }

    let code = hub.run(|hub, _| server::invite(hub.store_mut(), false).unwrap()).await.unwrap();
    let key = DeviceKey::generate(&mut OsRandom);
    let account = enroll(addr, &code.text(), b"\x0a", key.public()).await.unwrap();
    let identity = Identity { origin: format!("http://{addr}"), account: account.clone(), device: b"\x0a".to_vec(), key, ring: Ring::new(VaultKey::generate(&mut OsRandom)), name: String::new() };
    let mut a = Device::new(identity, 1);
    let mut ws = a.connect(addr).await;
    a.settle(&mut ws).await;
    let len = |n: usize| format!("Content-Length: {n}\r\n");
    let offer = |a: &mut Device, pic: &[u8]| {
        let id = a.session.blob_id(pic);
        a.session.offer_blobs(vec![(id, pic.len())]);
        id
    };
    // One put, kept: its folder and file the server's alone.
    let first: Vec<u8> = vec![1; 5000];
    let id = offer(&mut a, &first);
    a.settle(&mut ws).await;
    assert!(a.session.put_blob(&first, &mut OsRandom));
    let [Transfer::Put { url, sealed, .. }] = &a.session.take_transfers()[..] else { panic!("a put") };
    assert_eq!(request(addr, "PUT", &path_of(url), &len(sealed.len()), sealed).await.0, 204);
    a.session.transferred(id, true, Ok(Vec::new()));
    let folder = data.join("blobs").join(hex(&account));
    #[cfg(unix)]
    {
        assert_eq!(mode(&folder), 0o700);
        for f in walk(&folder) {
            assert_eq!(mode(&f), 0o600, "{}", f.display());
        }
        let db = Sharded::account_path(&data, &account).unwrap();
        assert_eq!(mode(&db), 0o600);
        let wal = PathBuf::from(format!("{}-wal", db.display()));
        if wal.exists() {
            assert_eq!(mode(&wal), 0o600, "the WAL as its database");
        }
    }

    // A second picture's link, then the account deleted.
    let second: Vec<u8> = vec![2; 7000];
    offer(&mut a, &second);
    a.settle(&mut ws).await;
    assert!(a.session.put_blob(&second, &mut OsRandom));
    let [Transfer::Put { url, sealed, .. }] = &a.session.take_transfers()[..] else { panic!("a put") };
    let (put_path, sealed) = (path_of(url), sealed.clone());
    let gone = account.clone();
    hub.run(move |hub, outs| outs.extend(hub.remove_account(&gone).unwrap())).await.unwrap();
    for _ in 0..50 {
        if !folder.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!folder.exists(), "its pictures gone with it");
    let (status, _, body) = request(addr, "PUT", &put_path, &len(sealed.len()), &sealed).await;
    assert_eq!(status, 403, "{}", String::from_utf8_lossy(&body));
    assert!(!folder.exists(), "not made again");
    assert!(std::fs::read_dir(data.join("spool")).unwrap().next().is_none());
    drop(ws);
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.unwrap().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&data);
}

/// Pictures whose account is no more (put by a link given before it was
/// deleted, or missed by its deletion) go on the hourly sweep, the first
/// at start; an account's own stay, and a folder that names no account
/// is left alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pictures_of_accounts_that_are_no_more_are_swept() {
    use carnet_server::Storage;
    let data = temp("unowned");
    {
        let mut db = Sharded::open(&data).unwrap();
        db.add_account(b"here", true).unwrap();
        db.add_device(b"here", b"d", [1; 32]).unwrap();
    }
    let blobs = data.join("blobs");
    for owner in [hex(b"here"), hex(b"gone"), "not-an-account".into()] {
        std::fs::create_dir_all(blobs.join(&owner)).unwrap();
        std::fs::write(blobs.join(&owner).join("07".repeat(32)), b"SEALED").unwrap();
    }
    let srv = start(&data, None).await;
    for _ in 0..50 {
        if !blobs.join(hex(b"gone")).exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!blobs.join(hex(b"gone")).exists(), "an account that's no more: its pictures gone");
    assert!(blobs.join(hex(b"here")).join("07".repeat(32)).exists(), "an account's own stay");
    assert!(blobs.join("not-an-account").exists(), "not a picture folder: left as it is");
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&data);
}

/// A page of another site, in someone's browser, opens no socket here: a
/// socket whose `Origin` isn't this server's is refused before it
/// upgrades. The apps send none, and the web app is at the server's own
/// origin; a server on this machine (development) takes its pages too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socket_from_another_sites_page_is_refused() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    assert!(server::origin_allowed("https://notes.example", "https://notes.example"));
    assert!(server::origin_allowed("https://notes.example/", "https://NOTES.example:443"));
    assert!(!server::origin_allowed("https://notes.example", "http://notes.example"));
    assert!(!server::origin_allowed("https://notes.example", "https://notes.example.evil.test"));
    assert!(!server::origin_allowed("https://notes.example", "https://notes.example:8443"));
    assert!(!server::origin_allowed("https://notes.example", "null"));
    assert!(!server::origin_allowed("https://notes.example", "http://localhost:5173"), "not a server on this machine");
    assert!(server::origin_allowed("http://127.0.0.1:8787", "http://localhost:5173"));
    assert!(server::origin_allowed("http://[::1]:8787", "http://127.0.0.1:5173"));

    let data = temp("origin");
    std::fs::create_dir_all(&data).unwrap();
    let srv = start(&data, Some("https://notes.example")).await;
    let open = |origin: Option<&'static str>| async move {
        let mut req = format!("ws://{}/v1/sync?protocol={}", srv.addr, carnet_sync::compat::PROTOCOL).into_client_request().unwrap();
        if let Some(o) = origin {
            req.headers_mut().insert("origin", o.parse().unwrap());
        }
        tokio_tungstenite::connect_async(req).await
    };
    let refused = open(Some("https://evil.example")).await.expect_err("another site's page");
    assert!(refused.to_string().contains("403"), "{refused}");
    assert!(open(Some("null")).await.is_err(), "a page with no origin of its own");
    assert!(open(None).await.is_ok(), "an app: no Origin");
    assert!(open(Some("https://notes.example")).await.is_ok(), "the web app, at the server's own origin");
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&data);
}
