//! The desktop's HTTP, for pictures (docs/SYNC.md §6): a picture's sealed
//! bytes put to, or fetched from, the short-lived link the server gave
//! ([`Transfer`]). A thread of its own, as the socket has, and nothing
//! else: what the bytes are is the session's business. And now and then
//! a question of the app's own to its server, apart from the socket
//! ([`ask`]): whether it takes signups, an invite asked for.
//!
//! HTTP/1.1, one request to a connection, over the socket's own TLS (the
//! web's roots, and a self-hosted server's certificate besides). A put
//! says its length; an answer is read by its length, in chunks, or to its
//! end. That's all a link needs, this server's or an object store's.
//!
//! A link the session found fair (`link::fair`) names its host as
//! written; a public-looking name can still be given an address on this
//! computer or its network by the DNS. So a picture's host is looked up
//! once, every address it has checked as `link` checks one written out,
//! and the connection made to those same addresses: a second lookup can't
//! answer otherwise. An address on this computer or its network is only
//! ever the server's own host's, and only when the socket itself is
//! connected to one such: then the connection goes to the very address
//! the socket is, so the DNS, the server's to answer for its own name,
//! can't send it elsewhere.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc as tmpsc;

use crate::crypto::{BlobId, MAX_BLOB};
use crate::link::{Url, inner, inner_ip, same_host};
use crate::session::Transfer;

/// How long a transfer may take, all of it.
const TIMEOUT: Duration = Duration::from_secs(120);

/// The most an answer may be: the largest picture, sealed.
const MOST: usize = MAX_BLOB + 64;

/// How a transfer went: for a fetch, the sealed bytes. For
/// [`Session::transferred`](crate::session::Session::transferred).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    pub id: BlobId,
    pub put: bool,
    pub result: Result<Vec<u8>, String>,
}

/// Transfers, done on a thread of their own, several at once.
pub struct Http {
    jobs: tmpsc::UnboundedSender<(Transfer, Option<Server>)>,
    done: mpsc::Receiver<Done>,
}

impl Http {
    /// The thread, trusting these certificates (DER) besides the web's
    /// roots; `wake` is called when a transfer is done.
    pub fn start(trusted: Vec<Vec<u8>>, wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Self> {
        let (jobs, mut rx) = tmpsc::unbounded_channel::<(Transfer, Option<Server>)>();
        let (tx, done) = mpsc::channel::<Done>();
        let wake = Arc::new(wake);
        std::thread::Builder::new().name("carnet-http".into()).spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            let config = crate::net::client_config(&trusted);
            rt.block_on(async move {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async move {
                        while let Some((t, server)) = rx.recv().await {
                            let (config, tx, wake) = (config.clone(), tx.clone(), wake.clone());
                            tokio::task::spawn_local(async move {
                                let (id, put) = (t.id(), matches!(t, Transfer::Put { .. }));
                                let result = match tokio::time::timeout(TIMEOUT, transfer(config, &t, server.as_ref())).await {
                                    Ok(r) => r,
                                    Err(_) => Err("timed out".into()),
                                };
                                if tx.send(Done { id, put, result }).is_ok() {
                                    wake();
                                }
                            });
                        }
                    })
                    .await;
            });
        })?;
        Ok(Http { jobs, done })
    }

    /// A transfer for the server at `origin`, its socket connected to
    /// `peer` (`Link::peer`): its link's host may give no address on this
    /// computer or its network unless it's the server's own host, and the
    /// socket is to such an address.
    pub fn send_for(&self, origin: &str, peer: Option<IpAddr>, t: Transfer) {
        let _ = self.jobs.send((t, Some(Server { origin: origin.to_string(), peer })));
    }

    /// A transfer to wherever its link's host is, unchecked: for a link
    /// made, not given (a test's server on this computer).
    pub fn send(&self, t: Transfer) {
        let _ = self.jobs.send((t, None));
    }

    /// Transfers done since last asked.
    pub fn poll(&self) -> Vec<Done> {
        self.done.try_iter().collect()
    }
}

/// The server a link came from: its origin, and the address its socket
/// is connected to, when known.
#[derive(Clone, Debug)]
struct Server {
    origin: String,
    peer: Option<IpAddr>,
}

async fn transfer(config: Arc<rustls::ClientConfig>, t: &Transfer, server: Option<&Server>) -> Result<Vec<u8>, String> {
    let answer = match t {
        Transfer::Put { url, sealed, .. } => request(config, "PUT", url, Some(("application/octet-stream", sealed)), server).await?,
        Transfer::Get { url, .. } => request(config, "GET", url, None, server).await?,
    };
    if !(200..300).contains(&answer.status) {
        return Err(format!("HTTP {}", answer.status));
    }
    Ok(answer.body)
}

/// An answer, whatever its status: what a question of the app's own
/// ([`ask`]) reads, a refusal's words among them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

/// How long a question may take, all of it.
const ASK_TIMEOUT: Duration = Duration::from_secs(20);

/// A question of the app's own to its server, on a thread of its own and
/// over the socket's TLS (the web's roots): a GET, or with `json` a POST
/// of it. Its answer comes on the channel, whatever its status, and
/// `wake` is called then. Asked now and then (whether a server takes
/// signups, an invite asked for), so a thread each is enough.
pub fn ask(url: String, json: Option<Vec<u8>>, wake: impl Fn() + Send + 'static) -> std::io::Result<mpsc::Receiver<Result<Answer, String>>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new().name("carnet-ask".into()).spawn(move || {
        let result = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt.block_on(async {
                let config = crate::net::client_config(&[]);
                let body = json.as_deref().map(|b| ("application/json", b));
                let method = if body.is_some() { "POST" } else { "GET" };
                match tokio::time::timeout(ASK_TIMEOUT, request(config, method, &url, body, None)).await {
                    Ok(r) => r,
                    Err(_) => Err("timed out".into()),
                }
            }),
            Err(e) => Err(e.to_string()),
        };
        if tx.send(result).is_ok() {
            wake();
        }
    })?;
    Ok(rx)
}

/// One request, with a body of its type if any, and its answer. Given
/// the server, the link is one it gave, and is checked where it leads
/// ([`addresses`]).
async fn request(config: Arc<rustls::ClientConfig>, method: &str, url: &str, body: Option<(&str, &[u8])>, server: Option<&Server>) -> Result<Answer, String> {
    // Read strictly (`link`): nothing in the request line but the link's
    // printable ASCII.
    let u = Url::parse(url)?;
    let at = addresses(&u, server).await?;
    let tcp = tokio::net::TcpStream::connect(&at[..]).await.map_err(|e| e.to_string())?;
    if u.tls {
        let name = rustls::pki_types::ServerName::try_from(u.host.clone()).map_err(|e| e.to_string())?;
        let tls = tokio_rustls::TlsConnector::from(config).connect(name, tcp).await.map_err(|e| e.to_string())?;
        exchange(tls, &u, method, body).await
    } else {
        exchange(tcp, &u, method, body).await
    }
}

/// Where `u`'s host is, looked up once. For a link a server gave, none
/// of its addresses may be on this computer or its network ([`inner_ip`])
/// unless it's the server's own host and the server is on one itself
/// ([`server_inner`]); and then it's the address the socket is connected
/// to, on the link's port, whatever the DNS says now.
async fn addresses(u: &Url, server: Option<&Server>) -> Result<Vec<SocketAddr>, String> {
    let at: Vec<SocketAddr> = tokio::net::lookup_host((u.host.as_str(), u.port)).await.map_err(|e| e.to_string())?.collect();
    if at.is_empty() {
        return Err(format!("{}: no address", u.host));
    }
    let Some(server) = server else { return Ok(at) };
    if !at.iter().any(|a| inner_ip(a.ip())) {
        return Ok(at);
    }
    let own = Url::parse(&server.origin).is_ok_and(|s| same_host(&s.host, &u.host));
    if !own || !server_inner(server).await {
        return Err(format!("{}: an address on this computer or its network, not the server's own", u.host));
    }
    Ok(match server.peer {
        // The socket's address first; and, the socket on this computer,
        // this computer's other loopback (`localhost` is `::1` and
        // `127.0.0.1` both, and a store beside it may listen on one).
        Some(ip) => {
            let mut to = vec![SocketAddr::new(ip, u.port)];
            to.extend(at.into_iter().filter(|a| a.ip() != ip && a.ip().is_loopback() && ip.is_loopback()));
            to
        }
        None => at,
    })
}

/// Whether the server is on this computer or its network: the address
/// its socket is connected to, when known; else its host, written so or
/// looked up so, every address of it. Any address public, and it isn't:
/// what a name looks up to is the server's own DNS to say, and a hostile
/// one could add `127.0.0.1` to its answer.
async fn server_inner(server: &Server) -> bool {
    if let Some(ip) = server.peer {
        return inner_ip(ip);
    }
    let Ok(s) = Url::parse(&server.origin) else { return false };
    if let Ok(ip) = s.host.parse::<IpAddr>() {
        return inner_ip(ip);
    }
    if inner(&s.host) {
        return true;
    }
    match tokio::net::lookup_host((s.host.as_str(), s.port)).await {
        Ok(at) => all_inner(&at.collect::<Vec<_>>()),
        Err(_) => false,
    }
}

/// Whether there are addresses, and every one is on this computer or its
/// network.
fn all_inner(at: &[SocketAddr]) -> bool {
    !at.is_empty() && at.iter().all(|a| inner_ip(a.ip()))
}

async fn exchange(mut s: impl AsyncRead + AsyncWrite + Unpin, u: &Url, method: &str, body: Option<(&str, &[u8])>) -> Result<Answer, String> {
    let mut head = format!("{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: Carnet\r\n", u.path, u.host_header());
    if let Some((kind, b)) = body {
        head.push_str(&format!("Content-Type: {kind}\r\nContent-Length: {}\r\n", b.len()));
    }
    let body = body.map_or(&[][..], |(_, b)| b);
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).await.map_err(|e| e.to_string())?;
    s.write_all(body).await.map_err(|e| e.to_string())?;
    s.flush().await.map_err(|e| e.to_string())?;

    // The answer: its head, then its body by length, in chunks, or to the
    // end. Read as it comes; never past what a picture may be.
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let (status, length, chunked, start) = loop {
        let n = s.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("the server closed before answering".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut res = httparse::Response::new(&mut headers);
        match res.parse(&buf).map_err(|e| e.to_string())? {
            httparse::Status::Complete(at) => {
                let header = |name: &str| res.headers.iter().find(|h| h.name.eq_ignore_ascii_case(name)).and_then(|h| std::str::from_utf8(h.value).ok());
                let length = header("content-length").and_then(|v| v.trim().parse::<usize>().ok());
                let chunked = header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
                break (res.code.unwrap_or(0), length, chunked, at);
            }
            httparse::Status::Partial if buf.len() > 64 * 1024 => return Err("an answer's head past its size".into()),
            httparse::Status::Partial => {}
        }
    };
    let mut rest = buf.split_off(start);
    loop {
        if length.is_some_and(|l| rest.len() >= l) || rest.len() > MOST * 2 {
            break;
        }
        let n = s.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        rest.extend_from_slice(&chunk[..n]);
    }
    let body = if chunked { unchunk(&rest)? } else { rest[..length.unwrap_or(rest.len()).min(rest.len())].to_vec() };
    if length.is_some_and(|l| body.len() < l) {
        return Err("the answer stopped short".into());
    }
    if body.len() > MOST {
        return Err("an answer past what a picture may be".into());
    }
    Ok(Answer { status, body })
}

/// A chunked body, put back together.
fn unchunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line = b.windows(2).position(|w| w == b"\r\n").ok_or("a chunk cut short")?;
        let size = std::str::from_utf8(&b[..line]).ok().and_then(|s| usize::from_str_radix(s.split(';').next()?.trim(), 16).ok()).ok_or("not a chunk")?;
        b = &b[line + 2..];
        if size == 0 {
            return Ok(out);
        }
        // A size past any there could be is a bad answer, not a sum
        // that wraps.
        let end = size.checked_add(2).ok_or("not a chunk")?;
        if b.len() < end {
            return Err("a chunk cut short".into());
        }
        out.extend_from_slice(&b[..size]);
        b = &b[end..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that answers one request with `answer`, and says what it
    /// was asked.
    async fn one_answer(answer: Vec<u8>) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            // The head, and as much body as it says.
            loop {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                if let Some(at) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&got[..at]).to_lowercase();
                    let len = head.lines().find_map(|l| l.strip_prefix("content-length: ")?.parse::<usize>().ok()).unwrap_or(0);
                    if got.len() >= at + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            s.write_all(&answer).await.unwrap();
            got
        });
        (format!("http://{addr}"), task)
    }

    fn config() -> Arc<rustls::ClientConfig> {
        crate::net::client_config(&[])
    }

    /// The server at `origin`, its socket's address not known.
    fn at(origin: &str) -> Server {
        Server { origin: origin.into(), peer: None }
    }

    fn at_peer(origin: &str, peer: &str) -> Server {
        Server { origin: origin.into(), peer: Some(peer.parse().unwrap()) }
    }

    #[test]
    fn chunks_are_read_as_they_say() {
        assert_eq!(unchunk(b"3\r\nabc\r\n2;x=1\r\nde\r\n0\r\n\r\n").unwrap(), b"abcde");
        assert!(unchunk(b"5\r\nab").is_err());
        // A size as big as can be, or past it: a bad answer, said so.
        assert_eq!(unchunk(b"ffffffffffffffff\r\nab\r\n0\r\n\r\n"), Err("not a chunk".into()));
        assert_eq!(unchunk(b"fffffffffffffffd\r\nab\r\n0\r\n\r\n"), Err("a chunk cut short".into()));
        assert!(unchunk(b"10000000000000000\r\nab\r\n").is_err());
    }

    /// A link with CR or LF in it is never sent: no second request, no
    /// header of the server's making.
    #[test]
    fn a_link_that_reads_two_ways_is_not_asked() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (base, _) = one_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()).await;
            let url = format!("{base}/x HTTP/1.1\r\nHost: evil\r\n\r\nGET /y");
            assert!(transfer(config(), &Transfer::Get { id: [1; 32], url }, Some(&at(&base))).await.is_err());
        });
    }

    /// A link whose name the DNS gives an address on this computer is not
    /// followed for a server elsewhere, nor one given as an address that
    /// is; for a server here, it is. Checked on the addresses looked up,
    /// not the name as written.
    #[test]
    fn a_name_that_leads_here_is_not_followed_for_a_server_elsewhere() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (base, _) = one_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()).await;
            let port = base.rsplit(':').next().unwrap();
            let url = format!("http://localhost:{port}/x");
            for elsewhere in ["https://notes.example.com", "https://203.0.113.9"] {
                let got = transfer(config(), &Transfer::Get { id: [1; 32], url: url.clone() }, Some(&at(elsewhere))).await;
                assert!(got.unwrap_err().contains("on this computer or its network"), "{elsewhere}");
            }
            assert_eq!(transfer(config(), &Transfer::Get { id: [1; 32], url }, Some(&at("http://localhost:8787"))).await.unwrap(), b"ok");
        });
        // The addresses a name may be given, as checked.
        for here in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.9",
            "172.20.0.1",
            "169.254.169.254",
            "100.100.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "fe80::1",
            "fd12::1",
            "::ffff:10.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::c0a8:101",
            "64:ff9b:1::808:808",
        ] {
            assert!(inner_ip(here.parse().unwrap()), "{here}");
        }
        for public in ["203.0.113.9", "8.8.8.8", "2606:4700::1111", "100.128.0.1", "64:ff9b::808:808"] {
            assert!(!inner_ip(public.parse().unwrap()), "{public}");
        }
    }

    /// A server's name the DNS gives an address here and one elsewhere
    /// is not counted as here: that answer is the server's own to give.
    /// Only every address here is.
    #[test]
    fn a_server_is_here_only_if_all_its_addresses_are() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(all_inner(&[a("127.0.0.1:80"), a("[::1]:80")]));
        assert!(all_inner(&[a("192.168.1.5:443")]));
        assert!(!all_inner(&[a("127.0.0.1:80"), a("203.0.113.9:80")]));
        assert!(!all_inner(&[a("203.0.113.9:80")]));
        assert!(!all_inner(&[]));
    }

    /// A hostile server, its socket somewhere public, whose links lead
    /// here (to Docker's port, say): never followed, by a name of its
    /// own looked up here, a name of the server's own looked up anew (the
    /// DNS turned since the socket connected), nor another host. A server
    /// whose socket is here: its own host, on any port, at the socket's
    /// very address; another host here, never.
    #[test]
    fn a_hostile_server_cannot_lead_here() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (base, _) = one_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()).await;
            let port = base.rsplit(':').next().unwrap();
            let get = |url: String| Transfer::Get { id: [1; 32], url };
            // The socket to a public address: nothing here, its own name
            // included.
            for (origin, url) in [
                ("http://localhost:8787", format!("http://localhost:{port}/x")),
                ("http://notes.example.com", format!("http://localhost:{port}/x")),
                ("http://127.0.0.1:8787", format!("http://127.0.0.1:{port}/x")),
            ] {
                let got = transfer(config(), &get(url), Some(&at_peer(origin, "203.0.113.9"))).await;
                assert!(got.unwrap_err().contains("not the server's own"), "{origin}");
            }
            // Here, but another host: refused.
            let got = transfer(config(), &get(format!("http://localhost:{port}/x")), Some(&at_peer("http://127.0.0.1:8787", "127.0.0.1"))).await;
            assert!(got.unwrap_err().contains("not the server's own"));
            let got = transfer(config(), &get(format!("http://localhost:{port}/x")), Some(&at("http://127.0.0.1:8787"))).await;
            assert!(got.unwrap_err().contains("not the server's own"));
            // Here, its own host, another port: followed, to the socket's
            // address (or this computer's other loopback).
            let got = transfer(config(), &get(format!("http://localhost:{port}/x")), Some(&at_peer("http://localhost:8787", "::1"))).await;
            assert_eq!(got.unwrap(), b"ok");
        });
    }

    #[test]
    fn a_get_and_a_put_over_http() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            // A fetch, its body by length.
            let (base, server) = one_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".to_vec()).await;
            let got = transfer(config(), &Transfer::Get { id: [1; 32], url: format!("{base}/v1/blobs/t?q=1") }, Some(&at(&base))).await;
            assert_eq!(got.unwrap(), b"hello");
            let asked = String::from_utf8(server.await.unwrap()).unwrap();
            assert!(asked.starts_with("GET /v1/blobs/t?q=1 HTTP/1.1\r\n"), "{asked}");
            // In chunks.
            let (base, _) = one_answer(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhel\r\n2\r\nlo\r\n0\r\n\r\n".to_vec()).await;
            assert_eq!(transfer(config(), &Transfer::Get { id: [1; 32], url: base.clone() }, Some(&at(&base))).await.unwrap(), b"hello");
            // A put says its length, and sends its bytes.
            let (base, server) = one_answer(b"HTTP/1.1 204 No Content\r\n\r\n".to_vec()).await;
            let put = Transfer::Put { id: [1; 32], url: format!("{base}/v1/blobs/p"), sealed: b"sealed bytes".to_vec() };
            assert_eq!(transfer(config(), &put, Some(&at(&base))).await.unwrap(), b"");
            let asked = String::from_utf8(server.await.unwrap()).unwrap();
            assert!(asked.starts_with("PUT /v1/blobs/p HTTP/1.1\r\n") && asked.contains("Content-Length: 12\r\n") && asked.ends_with("\r\n\r\nsealed bytes"), "{asked}");
            // Refused: said so.
            let (base, _) = one_answer(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 4\r\n\r\nnope".to_vec()).await;
            assert_eq!(transfer(config(), &Transfer::Get { id: [1; 32], url: base.clone() }, Some(&at(&base))).await, Err("HTTP 403".into()));
            // Cut short.
            let (base, _) = one_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nhal".to_vec()).await;
            assert!(transfer(config(), &Transfer::Get { id: [1; 32], url: base.clone() }, Some(&at(&base))).await.is_err());
        });
    }

    /// A question of the app's own: JSON posted as it is, and a refusal
    /// read with its words, not just its status.
    #[test]
    fn a_question_posts_json_and_reads_any_answer() {
        // The server answers on a thread of the runtime's while this one
        // waits for the question's.
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let (base, server) = rt.block_on(one_answer(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 21\r\n\r\n{\"error\":\"slow down\"}".to_vec()));
        let woke = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let w = woke.clone();
        let rx = ask(format!("{base}/v1/signup"), Some(br#"{"email":"ann@example.com"}"#.to_vec()), move || w.store(true, std::sync::atomic::Ordering::SeqCst)).unwrap();
        let got = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!(got, Answer { status: 429, body: br#"{"error":"slow down"}"#.to_vec() });
        // Woken just after the answer is sent: given a moment.
        let by = std::time::Instant::now() + Duration::from_secs(5);
        while !woke.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < by {
            std::thread::yield_now();
        }
        assert!(woke.load(std::sync::atomic::Ordering::SeqCst));
        let asked = String::from_utf8(rt.block_on(server).unwrap()).unwrap();
        assert!(asked.starts_with("POST /v1/signup HTTP/1.1\r\n"), "{asked}");
        assert!(asked.contains("Content-Type: application/json\r\nContent-Length: 27\r\n") && !asked.to_lowercase().contains("origin:"), "{asked}");
        assert!(asked.ends_with("\r\n\r\n{\"email\":\"ann@example.com\"}"), "{asked}");
        // A GET without a body; nobody there: said.
        let (base, server) = rt.block_on(one_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec()));
        let got = ask(format!("{base}/v1/info"), None, || {}).unwrap().recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!(got.status, 200);
        assert!(String::from_utf8(rt.block_on(server).unwrap()).unwrap().starts_with("GET /v1/info HTTP/1.1\r\n"));
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        assert!(ask(format!("http://{closed}/v1/info"), None, || {}).unwrap().recv_timeout(Duration::from_secs(10)).unwrap().is_err());
    }
}
