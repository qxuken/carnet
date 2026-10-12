//! A socket's own limits (docs/SYNC.md §6), on a real server: one that
//! pings without reading is closed once too much waits for it; one that
//! asks without reading is held to its own pace, and not closed; one that
//! never logs in, or goes quiet, is closed in time; one logged in and
//! answering stays. The limits are made small, so it's all quick.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use carnet_server::server::{self, Config, SocketLimits};
use carnet_server::{Sharded, Storage};
use carnet_sync::crypto::{DeviceKey, OsRandom, login_message};
use carnet_sync::wire::{self, Carnet, CrdtType, Frame};
use futures_util::{SinkExt, StreamExt};
use loro_protocol::ProtocolMessage;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

const ACCOUNT: &[u8] = b"account";
const ROOM: &str = "00000000000000000000000000000abc";

struct Running {
    addr: std::net::SocketAddr,
    key: DeviceKey,
    stop: oneshot::Sender<()>,
    done: tokio::task::JoinHandle<std::io::Result<()>>,
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("carnet-sockets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A server with one account and its device `d`, and a room of `room`
/// records of 2 KiB each, its sockets held to `limits`.
async fn start(data: &Path, limits: SocketLimits, room: u64) -> Running {
    let key = DeviceKey::generate(&mut OsRandom);
    {
        let mut db = Sharded::open(data).unwrap();
        db.add_account(ACCOUNT, true).unwrap();
        db.add_device(ACCOUNT, b"d", key.public()).unwrap();
        for i in 0..room {
            db.put(ACCOUNT, ROOM, 9, i, i + 1, vec![i as u8; 2048]).unwrap();
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, rx) = oneshot::channel::<()>();
    let config = Config { data: data.to_path_buf(), origin: format!("http://{addr}"), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new() };
    let host = server::Host { first_invite: false, ..server::Host::default() };
    let done = tokio::spawn(server::serve_limited(config, listener, None, host, limits, async {
        let _ = rx.await;
    }));
    Running { addr, key, stop, done }
}

impl Running {
    async fn open(&self) -> Socket {
        let url = format!("ws://{}/v1/sync?protocol={}", self.addr, carnet_sync::compat::PROTOCOL);
        tokio_tungstenite::connect_async(url).await.unwrap().0
    }

    /// A socket logged in as the device.
    async fn login(&self) -> Socket {
        let mut ws = self.open().await;
        let Some(Frame::Carnet(Carnet::Challenge { nonce, .. })) = next(&mut ws).await else { panic!("not a challenge") };
        let signature = self.key.sign(&login_message(&format!("http://{}", self.addr), ACCOUNT, b"d", &nonce));
        send(&mut ws, &Frame::Carnet(Carnet::Hello { account: ACCOUNT.to_vec(), device: b"d".to_vec(), signature, since: 0 })).await;
        until(&mut ws, |f| matches!(f, Frame::Carnet(Carnet::Welcome { .. }))).await.expect("logged in");
        ws
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(10), self.done).await.expect("stops").unwrap().unwrap();
    }
}

async fn send(ws: &mut Socket, f: &Frame) {
    ws.send(Message::Binary(wire::encode(f).into())).await.unwrap();
}

/// The next frame, or `None` once the socket ends (closed, or broken).
async fn next(ws: &mut Socket) -> Option<Frame> {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("something within 10 s") {
            Some(Ok(Message::Binary(b))) => return Some(wire::decode(&b).unwrap()),
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return None,
            Some(Ok(_)) => {}
        }
    }
}

/// Frames read until one is `want`: whether it came before the end.
async fn until(ws: &mut Socket, mut want: impl FnMut(&Frame) -> bool) -> Option<Frame> {
    while let Some(f) = next(ws).await {
        if want(&f) {
            return Some(f);
        }
    }
    None
}

/// The socket closed by the server, and why it said: read to its end.
async fn closed(ws: &mut Socket) -> Option<String> {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("closed within 10 s") {
            Some(Ok(Message::Close(c))) => return c.map(|c| c.reason.to_string()),
            Some(Err(_)) | None => return None,
            Some(Ok(_)) => {}
        }
    }
}

fn list() -> Frame {
    Frame::Carnet(Carnet::ListDevices)
}

fn is_list(f: &Frame) -> bool {
    matches!(f, Frame::Carnet(Carnet::Devices { .. }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socket_that_pings_and_never_reads_is_closed_once_too_much_waits() {
    let data = temp("pings");
    // No timeout comes first: it's the queue that closes it.
    let long = Duration::from_secs(600);
    let srv = start(&data, SocketLimits { queue: 64 * 1024, login: long, idle: long, ping: long }, 0).await;
    let mut ws = srv.open().await;
    // Pings, never a read: the pongs fill the socket's buffers, then its
    // queue, and the server lets it go. No login needed for that.
    let began = Instant::now();
    let mut pings = 0u64;
    let mut failed = false;
    while pings < 5_000_000 && began.elapsed() < Duration::from_secs(60) {
        if ws.feed(Message::Text("ping".into())).await.is_err() {
            failed = true;
            break;
        }
        pings += 1;
        if pings.is_multiple_of(1000) && ws.flush().await.is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed, "{pings} pings unread, and the socket still open");
    // Others are served as before.
    let mut other = srv.login().await;
    send(&mut other, &list()).await;
    assert!(until(&mut other, is_list).await.is_some());
    drop((ws, other));
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socket_that_asks_without_reading_is_held_to_its_pace_not_closed() {
    let data = temp("pace");
    // A room of 20 KiB, asked for a thousand times, unread: 20 MiB, were
    // it all answered at once; the queue holds 64 KiB.
    let srv = start(&data, SocketLimits { queue: 64 * 1024, ..SocketLimits::default() }, 10).await;
    let mut ws = srv.login().await;
    let join = Frame::Room(ProtocolMessage::JoinRequest { crdt: CrdtType::Elo, room_id: ROOM.into(), auth: Vec::new(), version: wire::encode_vv(&[]) });
    for _ in 0..1000 {
        ws.feed(Message::Binary(wire::encode(&join).into())).await.unwrap();
    }
    send(&mut ws, &list()).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    // Read now: every answer comes, the list's last, on the same socket.
    let mut joined = 0;
    let listed = until(&mut ws, |f| {
        if matches!(f, Frame::Room(ProtocolMessage::JoinResponseOk { .. })) {
            joined += 1;
        }
        is_list(f)
    })
    .await;
    assert!(listed.is_some(), "the socket was closed after {joined} answers");
    assert_eq!(joined, 1000);
    drop(ws);
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socket_that_never_logs_in_is_closed_after_the_login_timeout() {
    let data = temp("login");
    let login = Duration::from_secs(1);
    let srv = start(&data, SocketLimits { login, ..SocketLimits::default() }, 0).await;
    let mut late = srv.open().await;
    let opened = Instant::now();
    assert!(matches!(next(&mut late).await, Some(Frame::Carnet(Carnet::Challenge { .. }))));
    let mut device = srv.login().await;
    // Both ping meanwhile: that keeps neither one in, nor the other out.
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = late.send(Message::Text("ping".into())).await;
        device.send(Message::Text("ping".into())).await.unwrap();
    }
    let why = closed(&mut late).await;
    assert!(opened.elapsed() >= login, "closed before its time");
    assert_eq!(why.as_deref(), Some("took too long to sign in"));
    // The device logged in stays.
    send(&mut device, &list()).await;
    assert!(until(&mut device, is_list).await.is_some(), "the device logged in was let go too");
    drop(device);
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quiet_socket_is_pinged_and_closed_if_it_never_answers() {
    let data = temp("idle");
    let srv = start(&data, SocketLimits { idle: Duration::from_secs(1), ping: Duration::from_millis(300), ..SocketLimits::default() }, 0).await;
    let mut gone = srv.login().await;
    let mut there = srv.login().await;
    // `there` reads (its WebSocket answers the server's pings by itself)
    // and says nothing; `gone` neither reads nor says anything.
    let quiet = Instant::now();
    let mut pinged = 0;
    while quiet.elapsed() < Duration::from_secs(3) {
        if let Ok(Some(Ok(Message::Ping(_)))) = tokio::time::timeout(Duration::from_millis(100), there.next()).await {
            pinged += 1;
        }
    }
    assert!(pinged >= 2, "pinged {pinged} times");
    send(&mut there, &list()).await;
    assert!(until(&mut there, is_list).await.is_some(), "the socket that answered was let go");
    assert_eq!(closed(&mut gone).await.as_deref(), Some("nothing heard for too long"));
    drop(there);
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&data);
}

/// The sockets the server closes for its limits are counted, by why:
/// `carnet_sockets_closed_total{reason}` on `/metrics`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sockets_closed_for_their_limits_are_counted() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let data = temp("closes");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let metrics = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let maddr = metrics.local_addr().unwrap();
    let (stop, rx) = oneshot::channel::<()>();
    let config = Config { data: data.clone(), origin: format!("http://{addr}"), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new() };
    let host = server::Host { first_invite: false, ..server::Host::default() };
    let limits = SocketLimits { login: Duration::from_millis(500), ..SocketLimits::default() };
    let done = tokio::spawn(server::serve_limited(config, listener, Some(metrics), host, limits, async {
        let _ = rx.await;
    }));
    let url = format!("ws://{addr}/v1/sync?protocol={}", carnet_sync::compat::PROTOCOL);
    let mut late = tokio_tungstenite::connect_async(url).await.unwrap().0;
    assert_eq!(closed(&mut late).await.as_deref(), Some("took too long to sign in"));
    let mut s = TcpStream::connect(maddr).await.unwrap();
    s.write_all(b"GET /metrics HTTP/1.1\r\nHost: m\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut page = String::new();
    s.read_to_string(&mut page).await.unwrap();
    for (reason, n) in [("login_timeout", 1), ("idle", 0), ("budget", 0)] {
        let line = format!("carnet_sockets_closed_total{{reason=\"{reason}\"}} {n}\n");
        assert!(page.contains(&line), "{line}: {page}");
    }
    assert!(page.contains("# TYPE carnet_sockets_closed_total counter"));
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.unwrap().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&data);
}
