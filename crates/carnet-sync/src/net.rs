//! The desktop's socket (docs/SYNC.md §5): a thread of its own that keeps
//! a WebSocket to the server open, and moves bytes. Nothing else.
//!
//! What the bytes say is the session's business, on the app's own thread:
//! the session and the notes' CRDTs stay where the notes are, and only
//! the waiting on a network leaves. The thread wakes the app when bytes
//! come, as `images.rs`'s decoder does when a picture is ready.
//!
//! It connects, and when the socket drops, connects again, waiting longer
//! each time (1 s, 2 s, 4 s … a minute at most), the wait cut short when
//! asked. It sends a text `ping` every 30 s, as the Loro Protocol does,
//! so a proxy does not close a quiet socket. TLS is rustls with ring and
//! the web's roots built in, and, for a server with a certificate of its
//! own (a home server, a private CA), the one certificate given to trust
//! besides.

use std::net::IpAddr;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc as tmpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream, connect_async_tls_with_config};

/// What the socket did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetEvent {
    /// Open: the server speaks first (its challenge).
    Connected,
    Frame(Vec<u8>),
    /// Closed, and why; it tries again by itself.
    Disconnected(String),
}

enum Command {
    Send(Vec<u8>),
    /// Drop the socket and connect again now (the user asked, or the
    /// session needs a fresh start).
    Reconnect,
}

/// The longest wait between tries.
const MAX_WAIT: Duration = Duration::from_secs(60);
const KEEPALIVE: Duration = Duration::from_secs(30);

/// A socket to one server, kept open by its thread.
pub struct Link {
    commands: tmpsc::UnboundedSender<Command>,
    events: mpsc::Receiver<NetEvent>,
    peer: Arc<Mutex<Option<IpAddr>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The socket's URL for a server's origin, with this build's protocol
/// and which app it is: `https://x` → `wss://x/v1/sync?protocol=2&app=…`.
pub fn sync_url(origin: &str, app: &str, version: &str) -> Option<String> {
    let origin = origin.trim_end_matches('/');
    let rest = origin.strip_prefix("https://").map(|r| ("wss", r)).or_else(|| origin.strip_prefix("http://").map(|r| ("ws", r)))?;
    Some(crate::compat::with_app(&format!("{}://{}/v1/sync", rest.0, rest.1), app, version))
}

/// The address a socket is connected to.
fn peer_of(s: &MaybeTlsStream<tokio::net::TcpStream>) -> Option<IpAddr> {
    match s {
        MaybeTlsStream::Plain(t) => t.peer_addr().ok(),
        MaybeTlsStream::Rustls(t) => t.get_ref().0.peer_addr().ok(),
        _ => None,
    }
    .map(|a| a.ip())
}

fn tls(trusted: &[Vec<u8>]) -> Connector {
    Connector::Rustls(client_config(trusted))
}

/// TLS as the desktop speaks it, to its server and the server's picture
/// links alike: rustls with ring, the web's roots, and these besides.
pub(crate) fn client_config(trusted: &[Vec<u8>]) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    for der in trusted {
        // One that is not a certificate is left out, and the server's own
        // then fails to verify, which says so.
        let _ = roots.add(rustls::pki_types::CertificateDer::from(der.clone()));
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring offers TLS 1.2 and 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Arc::new(config)
}

impl Link {
    /// Starts the thread: it connects to `url` (`wss://…/v1/sync`) at
    /// once, and calls `wake` whenever there is something to [`poll`].
    ///
    /// [`poll`]: Link::poll
    pub fn start(url: String, wake: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        Self::start_trusting(url, Vec::new(), wake)
    }

    /// [`start`](Link::start), trusting these certificates (DER) besides
    /// the web's roots: a self-hosted server's own, or its private CA's.
    pub fn start_trusting(url: String, trusted: Vec<Vec<u8>>, wake: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (commands, mut rx) = tmpsc::unbounded_channel::<Command>();
        let (tx, events) = mpsc::channel::<NetEvent>();
        let peer = Arc::new(Mutex::new(None));
        let at = peer.clone();
        let thread = std::thread::Builder::new().name("carnet-sync".into()).spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            rt.block_on(async move {
                let emit = |e: NetEvent| {
                    let ok = tx.send(e).is_ok();
                    wake();
                    ok
                };
                let mut wait = Duration::from_secs(1);
                loop {
                    let connected = tokio::time::timeout(
                        Duration::from_secs(20),
                        connect_async_tls_with_config(url.as_str(), None, true, Some(tls(&trusted))),
                    )
                    .await;
                    let reason = match connected {
                        Ok(Ok((ws, _))) => {
                            wait = Duration::from_secs(1);
                            *at.lock().unwrap_or_else(|e| e.into_inner()) = peer_of(ws.get_ref());
                            if !emit(NetEvent::Connected) {
                                return;
                            }
                            let (mut sink, mut stream) = ws.split();
                            let mut ping = tokio::time::interval(KEEPALIVE);
                            ping.tick().await;
                            loop {
                                tokio::select! {
                                    c = rx.recv() => match c {
                                        Some(Command::Send(b)) => {
                                            if let Err(e) = sink.send(Message::Binary(b.into())).await {
                                                break e.to_string();
                                            }
                                        }
                                        Some(Command::Reconnect) => break "reconnecting".to_string(),
                                        // The Link is gone: so is the thread.
                                        None => return,
                                    },
                                    m = stream.next() => match m {
                                        Some(Ok(Message::Binary(b))) => {
                                            if !emit(NetEvent::Frame(b.to_vec())) {
                                                return;
                                            }
                                        }
                                        Some(Ok(Message::Close(f))) => {
                                            break f.map_or_else(|| "closed".to_string(), |f| f.reason.to_string());
                                        }
                                        Some(Ok(_)) => {}
                                        Some(Err(e)) => break e.to_string(),
                                        None => break "closed".to_string(),
                                    },
                                    _ = ping.tick() => {
                                        if let Err(e) = sink.send(Message::Text("ping".into())).await {
                                            break e.to_string();
                                        }
                                    }
                                }
                            }
                        }
                        Ok(Err(e)) => e.to_string(),
                        Err(_) => "timed out connecting".to_string(),
                    };
                    if !emit(NetEvent::Disconnected(reason)) {
                        return;
                    }
                    // Wait, unless asked to try now; sends meanwhile are
                    // dropped (the session works out again what to send).
                    let sleep = tokio::time::sleep(wait);
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => break,
                            c = rx.recv() => match c {
                                Some(Command::Reconnect) => break,
                                Some(Command::Send(_)) => {}
                                None => return,
                            },
                        }
                    }
                    wait = (wait * 2).min(MAX_WAIT);
                }
            });
        })?;
        Ok(Link { commands, events, peer, thread: Some(thread) })
    }

    /// The address the socket is (or was last) connected to: what a
    /// picture's link to the server's own host may lead to, and whether
    /// the server is on this computer or its network (`http`).
    pub fn peer(&self) -> Option<IpAddr> {
        *self.peer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sends a frame, if the socket is open (else it is dropped: the
    /// session sends again what is needed once it is back).
    pub fn send(&self, bytes: Vec<u8>) {
        let _ = self.commands.send(Command::Send(bytes));
    }

    /// Drops the socket and connects again now.
    pub fn reconnect(&self) {
        let _ = self.commands.send(Command::Reconnect);
    }

    /// What happened since last asked.
    pub fn poll(&self) -> Vec<NetEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for Link {
    /// The thread ends with its Link: the channel's end is its signal.
    fn drop(&mut self) {
        let (dead, _) = tmpsc::unbounded_channel();
        drop(std::mem::replace(&mut self.commands, dead));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_names_its_socket() {
        let p = crate::compat::PROTOCOL;
        assert_eq!(sync_url("https://notes.example.com", "desktop-macos", "1.2"), Some(format!("wss://notes.example.com/v1/sync?protocol={p}&app=desktop-macos&version=1.2")));
        assert_eq!(sync_url("http://127.0.0.1:8080/", "t", "0"), Some(format!("ws://127.0.0.1:8080/v1/sync?protocol={p}&app=t&version=0")));
        assert_eq!(sync_url("notes.example.com", "t", "0"), None);
    }

    /// A wss server with a certificate of its own making: trusted, the
    /// link connects and bytes go both ways; not, it is refused.
    #[test]
    fn tls_with_a_certificate_given_to_trust() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let der = cert.cert.der().to_vec();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![der.clone().into()], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (got, heard) = std::sync::mpsc::channel::<Vec<u8>>();
        rt.spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { return };
                let (acceptor, got) = (acceptor.clone(), got.clone());
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(tcp).await else { return };
                    let Ok(mut ws) = tokio_tungstenite::accept_async(tls).await else { return };
                    let _ = ws.send(Message::Binary(b"hello".to_vec().into())).await;
                    while let Some(Ok(m)) = ws.next().await {
                        if let Message::Binary(b) = m {
                            let _ = got.send(b.to_vec());
                        }
                    }
                });
            }
        });
        let url = format!("wss://localhost:{port}/v1/sync");
        let wait = |link: &Link| {
            let t0 = std::time::Instant::now();
            let mut seen = Vec::new();
            while t0.elapsed() < Duration::from_secs(10) && !seen.iter().any(|e| matches!(e, NetEvent::Frame(_) | NetEvent::Disconnected(_))) {
                seen.extend(link.poll());
                std::thread::sleep(Duration::from_millis(10));
            }
            seen
        };

        let trusting = Link::start_trusting(url.clone(), vec![der], || {}).unwrap();
        let seen = wait(&trusting);
        assert_eq!(seen, [NetEvent::Connected, NetEvent::Frame(b"hello".to_vec())]);
        // Where the socket is: what a picture's link may lead to.
        assert!(trusting.peer().is_some_and(|ip| ip.is_loopback()), "{:?}", trusting.peer());
        trusting.send(b"back".to_vec());
        assert_eq!(heard.recv_timeout(Duration::from_secs(5)).unwrap(), b"back");
        drop(trusting);

        let wary = Link::start(url, || {}).unwrap();
        let seen = wait(&wary);
        assert!(matches!(&seen[..], [NetEvent::Disconnected(why)] if why.to_lowercase().contains("certificate")), "{seen:?}");
    }

    #[test]
    fn a_server_that_is_not_there_is_tried_again_and_the_link_stops() {
        let link = Link::start("ws://127.0.0.1:9/v1/sync".into(), || {}).unwrap();
        let t0 = std::time::Instant::now();
        let mut seen = Vec::new();
        while t0.elapsed() < Duration::from_secs(5) && seen.is_empty() {
            seen.extend(link.poll());
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(matches!(seen.first(), Some(NetEvent::Disconnected(_))), "{seen:?}");
        drop(link);
    }
}
