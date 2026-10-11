//! The desktop's HTTP, for pictures (docs/SYNC.md §6): a picture's sealed
//! bytes put to, or fetched from, the short-lived link the server gave
//! ([`Transfer`]). A thread of its own, as the socket has, and nothing
//! else: what the bytes are is the session's business.
//!
//! HTTP/1.1, one request to a connection, over the socket's own TLS (the
//! web's roots, and a self-hosted server's certificate besides). A put
//! says its length; an answer is read by its length, in chunks, or to its
//! end. That's all a link needs, this server's or an object store's.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc as tmpsc;

use crate::crypto::{BlobId, MAX_BLOB};
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
    jobs: tmpsc::UnboundedSender<Transfer>,
    done: mpsc::Receiver<Done>,
}

impl Http {
    /// The thread, trusting these certificates (DER) besides the web's
    /// roots; `wake` is called when a transfer is done.
    pub fn start(trusted: Vec<Vec<u8>>, wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Self> {
        let (jobs, mut rx) = tmpsc::unbounded_channel::<Transfer>();
        let (tx, done) = mpsc::channel::<Done>();
        let wake = Arc::new(wake);
        std::thread::Builder::new().name("carnet-http".into()).spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            let config = crate::net::client_config(&trusted);
            rt.block_on(async move {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async move {
                        while let Some(t) = rx.recv().await {
                            let (config, tx, wake) = (config.clone(), tx.clone(), wake.clone());
                            tokio::task::spawn_local(async move {
                                let (id, put) = (t.id(), matches!(t, Transfer::Put { .. }));
                                let result = match tokio::time::timeout(TIMEOUT, transfer(config, &t)).await {
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

    pub fn send(&self, t: Transfer) {
        let _ = self.jobs.send(t);
    }

    /// Transfers done since last asked.
    pub fn poll(&self) -> Vec<Done> {
        self.done.try_iter().collect()
    }
}

/// Where a link points: secure or not, host, port, and the path with its
/// query, as sent.
struct Url {
    tls: bool,
    host: String,
    port: u16,
    path: String,
}

fn parse(url: &str) -> Result<Url, String> {
    let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("{url}: not an http(s) link"));
    };
    let (authority, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || p.parse::<u16>().is_ok() => (h, p.parse().map_err(|_| format!("{url}: not a port"))?),
        _ => (authority, if tls { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(format!("{url}: no host"));
    }
    Ok(Url { tls, host: host.trim_start_matches('[').trim_end_matches(']').to_string(), port, path: path.to_string() })
}

async fn transfer(config: Arc<rustls::ClientConfig>, t: &Transfer) -> Result<Vec<u8>, String> {
    let (url, method, body): (&str, &str, &[u8]) = match t {
        Transfer::Put { url, sealed, .. } => (url, "PUT", sealed),
        Transfer::Get { url, .. } => (url, "GET", &[]),
    };
    let u = parse(url)?;
    let tcp = tokio::net::TcpStream::connect((u.host.as_str(), u.port)).await.map_err(|e| e.to_string())?;
    if u.tls {
        let name = rustls::pki_types::ServerName::try_from(u.host.clone()).map_err(|e| e.to_string())?;
        let tls = tokio_rustls::TlsConnector::from(config).connect(name, tcp).await.map_err(|e| e.to_string())?;
        exchange(tls, &u, method, body).await
    } else {
        exchange(tcp, &u, method, body).await
    }
}

async fn exchange(mut s: impl AsyncRead + AsyncWrite + Unpin, u: &Url, method: &str, body: &[u8]) -> Result<Vec<u8>, String> {
    let host = if (u.tls && u.port == 443) || (!u.tls && u.port == 80) { u.host.clone() } else { format!("{}:{}", u.host, u.port) };
    let mut head = format!("{method} {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: Carnet\r\n", u.path);
    if method == "PUT" {
        head.push_str(&format!("Content-Type: application/octet-stream\r\nContent-Length: {}\r\n", body.len()));
    }
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
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    if body.len() > MOST {
        return Err("an answer past what a picture may be".into());
    }
    Ok(body)
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
        if b.len() < size + 2 {
            return Err("a chunk cut short".into());
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
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

    #[test]
    fn links_are_read_as_they_say() {
        let u = parse("https://bucket.example/acct/id?X-Amz-Signature=ab%2Fcd&x=1").unwrap();
        assert_eq!((u.tls, u.host.as_str(), u.port, u.path.as_str()), (true, "bucket.example", 443, "/acct/id?X-Amz-Signature=ab%2Fcd&x=1"));
        let u = parse("http://127.0.0.1:8787/v1/blobs/ab").unwrap();
        assert_eq!((u.tls, u.host.as_str(), u.port), (false, "127.0.0.1", 8787));
        let u = parse("http://[::1]:9000/x").unwrap();
        assert_eq!((u.host.as_str(), u.port), ("::1", 9000));
        assert!(parse("ftp://x/y").is_err() && parse("https:///x").is_err());
        assert_eq!(unchunk(b"3\r\nabc\r\n2;x=1\r\nde\r\n0\r\n\r\n").unwrap(), b"abcde");
        assert!(unchunk(b"5\r\nab").is_err());
    }

    #[test]
    fn a_get_and_a_put_over_http() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            // A fetch, its body by length.
            let (base, server) = one_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".to_vec()).await;
            let got = transfer(config(), &Transfer::Get { id: [1; 32], url: format!("{base}/v1/blobs/t?q=1") }).await;
            assert_eq!(got.unwrap(), b"hello");
            let asked = String::from_utf8(server.await.unwrap()).unwrap();
            assert!(asked.starts_with("GET /v1/blobs/t?q=1 HTTP/1.1\r\n"), "{asked}");
            // In chunks.
            let (base, _) = one_answer(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhel\r\n2\r\nlo\r\n0\r\n\r\n".to_vec()).await;
            assert_eq!(transfer(config(), &Transfer::Get { id: [1; 32], url: base }).await.unwrap(), b"hello");
            // A put says its length, and sends its bytes.
            let (base, server) = one_answer(b"HTTP/1.1 204 No Content\r\n\r\n".to_vec()).await;
            let put = Transfer::Put { id: [1; 32], url: format!("{base}/v1/blobs/p"), sealed: b"sealed bytes".to_vec() };
            assert_eq!(transfer(config(), &put).await.unwrap(), b"");
            let asked = String::from_utf8(server.await.unwrap()).unwrap();
            assert!(asked.starts_with("PUT /v1/blobs/p HTTP/1.1\r\n") && asked.contains("Content-Length: 12\r\n") && asked.ends_with("\r\n\r\nsealed bytes"), "{asked}");
            // Refused: said so.
            let (base, _) = one_answer(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 4\r\n\r\nnope".to_vec()).await;
            assert_eq!(transfer(config(), &Transfer::Get { id: [1; 32], url: base }).await, Err("HTTP 403".into()));
            // Cut short.
            let (base, _) = one_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nhal".to_vec()).await;
            assert!(transfer(config(), &Transfer::Get { id: [1; 32], url: base }).await.is_err());
        });
    }
}
