//! TLS on the server's own listener (tls.rs), as behind Cloudflare with
//! an Origin certificate and Authenticated Origin Pulls: CAs of the test's
//! own making stand in for Cloudflare's. A device with the client
//! certificate makes its account, logs in and puts and fetches a file,
//! all over TLS; a client without one, or with one another CA signed, or
//! speaking plain HTTP, gets nothing. Without a client CA, TLS alone.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use carnet_server::Sharded;
use carnet_server::server::{self, Config};
use carnet_server::tls::Tls;
use carnet_sync::crypto::{DeviceKey, OsRandom, Ring, VaultKey};
use carnet_sync::session::{Event, Identity, Session, Transfer};
use futures_util::{SinkExt, StreamExt};
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use tokio_rustls::rustls::{self, ClientConfig, RootCertStore};
use tokio_tungstenite::tungstenite::Message;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("carnet-server-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// DER as PEM: base64 in lines of 64.
fn pem(label: &str, der: &[u8]) -> String {
    const B: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut b64 = String::new();
    for c in der.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            b64.push(if i <= c.len() { B[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|l| std::str::from_utf8(l).unwrap()).collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

fn ca(name: &str) -> CertifiedIssuer<'static, KeyPair> {
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.distinguished_name.push(DnType::CommonName, name);
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    CertifiedIssuer::self_signed(p, KeyPair::generate().unwrap()).unwrap()
}

/// A certificate `issuer` signed, and its key.
fn leaf(issuer: &CertifiedIssuer<'static, KeyPair>, names: &[&str], purpose: ExtendedKeyUsagePurpose) -> (CertificateDer<'static>, KeyPair) {
    let mut p = CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
    p.distinguished_name.push(DnType::CommonName, names.first().copied().unwrap_or("client"));
    p.extended_key_usages = vec![purpose];
    let key = KeyPair::generate().unwrap();
    (p.signed_by(&key, issuer).unwrap().der().clone(), key)
}

struct Certs {
    dir: PathBuf,
    origin_ca: CertificateDer<'static>,
    /// A client certificate the pull CA signed, and one another CA did.
    client: (CertificateDer<'static>, KeyPair),
    rogue: (CertificateDer<'static>, KeyPair),
}

impl Certs {
    fn make(dir: &Path) -> Certs {
        let origin_ca = ca("test origin CA");
        let (server, server_key) = leaf(&origin_ca, &["localhost"], ExtendedKeyUsagePurpose::ServerAuth);
        std::fs::write(dir.join("origin.pem"), pem("CERTIFICATE", &server)).unwrap();
        std::fs::write(dir.join("origin.key"), pem("PRIVATE KEY", server_key.serialized_der())).unwrap();
        let pull_ca = ca("test origin-pull CA");
        std::fs::write(dir.join("pull-ca.pem"), pem("CERTIFICATE", pull_ca.der())).unwrap();
        let client = leaf(&pull_ca, &["origin-pull.test"], ExtendedKeyUsagePurpose::ClientAuth);
        let rogue = leaf(&ca("someone else's CA"), &["origin-pull.test"], ExtendedKeyUsagePurpose::ClientAuth);
        Certs { dir: dir.to_path_buf(), origin_ca: origin_ca.der().clone(), client, rogue }
    }

    fn tls(&self, clients: bool) -> Tls {
        let ca = self.dir.join("pull-ca.pem");
        Tls::load(&self.dir.join("origin.pem"), &self.dir.join("origin.key"), clients.then_some(ca.as_path())).unwrap()
    }

    /// A client that trusts the origin CA, showing `cert` if given.
    fn client(&self, cert: Option<&(CertificateDer<'static>, KeyPair)>) -> tokio_rustls::TlsConnector {
        let mut roots = RootCertStore::empty();
        roots.add(self.origin_ca.clone()).unwrap();
        let b = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().with_root_certificates(roots);
        let config = match cert {
            Some((c, k)) => b.with_client_auth_cert(vec![c.clone()], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(k.serialize_der()))).unwrap(),
            None => b.with_no_client_auth(),
        };
        tokio_rustls::TlsConnector::from(Arc::new(config))
    }
}

type Tlsed = tokio_rustls::client::TlsStream<TcpStream>;

async fn connect(port: u16, connector: &tokio_rustls::TlsConnector) -> std::io::Result<Tlsed> {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await?;
    connector.connect(ServerName::try_from("localhost").unwrap(), tcp).await
}

/// A request by hand over TLS: the status (0 if nothing came back), and
/// the body.
async fn request(port: u16, connector: &tokio_rustls::TlsConnector, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let Ok(mut s) = connect(port, connector).await else { return (0, Vec::new()) };
    let head = format!("{method} {path} HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    if s.write_all(head.as_bytes()).await.is_err() || s.write_all(body).await.is_err() {
        return (0, Vec::new());
    }
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out).await;
    let Some(split) = out.windows(4).position(|w| w == b"\r\n\r\n") else { return (0, Vec::new()) };
    let status = String::from_utf8_lossy(&out[9..12]).parse().unwrap_or(0);
    (status, out[split + 4..].to_vec())
}

async fn start(data: &Path, tls: Tls) -> (u16, tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<std::io::Result<()>>) {
    start_with(data, tls, None).await
}

async fn start_with(data: &Path, tls: Tls, metrics: Option<TcpListener>) -> (u16, tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<std::io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Config {
        data: data.to_path_buf(),
        origin: format!("https://localhost:{port}"),
        compact_after: server::COMPACT_AFTER,
        web: None,
        trusted: Vec::new(),
        client_header: Default::default(),
        tls: Some(tls),
    };
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let done = tokio::spawn(server::serve_with(config, listener, metrics, async {
        let _ = rx.await;
    }));
    tokio::time::sleep(Duration::from_millis(100)).await;
    (port, stop, done)
}

type Socket = tokio_tungstenite::WebSocketStream<Tlsed>;

async fn socket(port: u16, connector: &tokio_rustls::TlsConnector) -> Socket {
    let stream = connect(port, connector).await.unwrap();
    let url = carnet_sync::compat::with_protocol(&format!("wss://localhost:{port}/v1/sync"));
    tokio_tungstenite::client_async(url, stream).await.unwrap().0
}

struct NoNotes;

impl carnet_sync::session::Docs for NoNotes {
    fn rooms(&self) -> Vec<String> {
        Vec::new()
    }
    fn doc(&mut self, _: &str) -> &carnet_sync::NoteDoc {
        unreachable!("no notes here")
    }
}

async fn settle(session: &mut Session, ws: &mut Socket) -> Vec<Event> {
    let mut said = Vec::new();
    loop {
        for out in session.take_out() {
            ws.send(Message::Binary(out.into())).await.unwrap();
        }
        match tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => {
                session.receive(&b, &mut NoNotes, &mut OsRandom);
                said.extend(session.take_events());
            }
            Ok(Some(Ok(_))) => {}
            Ok(_) => return said,
            Err(_) if session.is_idle() || !session.is_ready() => return said,
            Err(_) => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_tls_with_a_client_certificate_a_device_logs_in_and_puts_a_file() {
    use carnet_sync::wire::{self, Carnet, Frame};
    let data = temp("tls");
    let certs = Certs::make(&data);
    let code = server::new_invite(&mut Sharded::open(&data).unwrap()).unwrap();
    let (port, stop, done) = start(&data, certs.tls(true)).await;
    let pulled = certs.client(Some(&certs.client));

    // Without a client certificate, with one another CA signed, or in
    // plain HTTP: nothing.
    assert_eq!(request(port, &certs.client(None), "GET", "/v1/health", b"").await.0, 0, "no client certificate");
    assert_eq!(request(port, &certs.client(Some(&certs.rogue)), "GET", "/v1/health", b"").await.0, 0, "another CA's");
    let mut plain = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    plain.write_all(format!("GET /v1/health HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), plain.read_to_end(&mut out)).await;
    assert!(!out.starts_with(b"HTTP/1.1 200"), "plain HTTP: {}", String::from_utf8_lossy(&out));
    // With it: well.
    let (status, body) = request(port, &pulled, "GET", "/v1/health", b"").await;
    assert_eq!((status, body.as_slice()), (200, br#"{"ok":true}"#.as_slice()));

    // The account made, a device logged in, over TLS (wss).
    let key = DeviceKey::generate(&mut OsRandom);
    let mut ws = socket(port, &pulled).await;
    ws.send(Message::Binary(wire::encode(&Frame::Carnet(Carnet::Enroll { invite: code.text(), device: b"\x0a".to_vec(), public: key.public() })).into())).await.unwrap();
    let account = loop {
        let Some(Ok(Message::Binary(b))) = ws.next().await else { panic!("the socket closed") };
        if let Ok(Frame::Carnet(Carnet::Enrolled { account })) = wire::decode(&b) {
            break account;
        }
    };
    drop(ws);
    let identity = Identity { origin: format!("https://localhost:{port}"), account, device: b"\x0a".to_vec(), key, ring: Ring::new(VaultKey::generate(&mut OsRandom)), name: String::new() };
    let mut session = Session::new(identity, 0);
    let mut ws = socket(port, &pulled).await;
    session.connected();
    let said = settle(&mut session, &mut ws).await;
    assert!(said.contains(&Event::LoggedIn), "{said:?}");

    // A file, put and fetched by its links, over TLS.
    let pic: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let id = session.blob_id(&pic);
    session.offer_blobs(vec![(id, pic.len())]);
    let said = settle(&mut session, &mut ws).await;
    assert!(said.contains(&Event::BlobWanted(id)), "{said:?}");
    assert!(session.put_blob(&pic, &mut OsRandom));
    let [Transfer::Put { url, sealed, .. }] = &session.take_transfers()[..] else { panic!("a put") };
    assert!(url.starts_with(&format!("https://localhost:{port}/v1/blobs/")), "{url}");
    let path = |url: &str| format!("/{}", url.split_once("://").unwrap().1.split_once('/').unwrap().1);
    assert_eq!(request(port, &certs.client(None), "PUT", &path(url), sealed).await.0, 0, "not without the certificate");
    assert_eq!(request(port, &pulled, "PUT", &path(url), sealed).await.0, 204);
    session.transferred(id, true, Ok(Vec::new()));
    session.get_blob(id);
    settle(&mut session, &mut ws).await;
    let [Transfer::Get { url, .. }] = &session.take_transfers()[..] else { panic!("a get") };
    let (status, body) = request(port, &pulled, "GET", &path(url), b"").await;
    assert_eq!(status, 200);
    session.transferred(id, false, Ok(body));
    assert!(session.take_events().contains(&Event::BlobGot(id, pic)));

    drop(ws);
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.unwrap().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&data);
}

/// Without a client CA: TLS, and any client. Health is asked on the
/// metrics address too, in plain HTTP (the machine's own check).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_client_ca_any_client_is_served_over_tls() {
    let data = temp("tls-open");
    let certs = Certs::make(&data);
    let metrics = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = metrics.local_addr().unwrap();
    let (port, stop, done) = start_with(&data, certs.tls(false), Some(metrics)).await;
    let (status, body) = request(port, &certs.client(None), "GET", "/v1/health", b"").await;
    assert_eq!((status, body.as_slice()), (200, br#"{"ok":true}"#.as_slice()));
    let mut s = TcpStream::connect(at).await.unwrap();
    s.write_all(format!("GET /v1/health HTTP/1.1\r\nHost: {at}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 200") && out.ends_with(r#"{"ok":true}"#), "{out}");
    // A key that isn't the certificate's is refused when loaded.
    std::fs::write(data.join("other.key"), pem("PRIVATE KEY", KeyPair::generate().unwrap().serialized_der())).unwrap();
    let refused = |cert: &str, key: &str, ca: Option<&str>| Tls::load(&data.join(cert), &data.join(key), ca.map(|c| data.join(c)).as_deref()).expect_err("refused").to_string();
    assert!(refused("origin.pem", "other.key", None).contains("other.key: not the key of the certificate in"), "said plainly");
    assert!(refused("origin.pem", "nowhere.key", None).contains("nowhere.key: I/O error"));
    assert!(refused("nowhere.pem", "origin.key", None).contains("nowhere.pem: I/O error"));
    assert!(refused("origin.key", "origin.key", None).contains("origin.key: no certificate in it"));
    assert!(refused("origin.pem", "origin.pem", None).contains("origin.pem: no private key in it"));
    assert!(refused("origin.pem", "origin.key", Some("origin.key")).contains("origin.key: no certificate in it"), "a client CA file with none");
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.unwrap().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&data);
}
