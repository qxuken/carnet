//! `carnet-server`: serve, print an invite, back up, or move to a
//! database server and a bucket.
//!
//! ```text
//! carnet-server [--data DIR] [--listen ADDR] [--origin URL] [--web DIR] [--metrics ADDR] [--trusted-proxy LIST] [--client-header cf|xff]
//!               [--tls-cert FILE --tls-key FILE [--tls-client-ca FILE]] [--passwords] [--compact-after N] [--postgres URL] [--s3 URL [--s3-region R] [--s3-virtual-host]]
//!               [--keep-untracked-pictures]
//! carnet-server invite [--data DIR] [--postgres URL]
//! carnet-server backup DIR [--data DIR]
//! carnet-server health [--listen ADDR | --metrics ADDR]
//! carnet-server migrate [--from DIR] [--to-postgres URL] [--to-s3 URL [--s3-region R]] [--verify-only | --pictures-first]
//! ```
//!
//! Each flag has an environment variable for a container: `CARNET_DATA`
//! (default `./data`), `CARNET_LISTEN` (default `127.0.0.1:8080`; in a
//! container, `0.0.0.0:8080`), `CARNET_ORIGIN` (default `http://` and the
//! listen address; behind a proxy, the public `https://…` devices use),
//! `CARNET_COMPACT_AFTER` (default 200), `CARNET_WEB` (the PWA's build,
//! `web/dist`, served at `/`; none by default), `CARNET_METRICS` (an
//! address for `/metrics`, its own listener; none by default),
//! `CARNET_TRUSTED_PROXY` (the proxies in front, addresses or ranges, comma
//! separated, whose word on who a request is from is believed; none by
//! default), `CARNET_CLIENT_HEADER` (the header they say it in: `xff`,
//! `X-Forwarded-For`, by default; `cf`, Cloudflare's `CF-Connecting-IP`,
//! straight behind Cloudflare), `CARNET_TLS_CERT`, `CARNET_TLS_KEY` and
//! `CARNET_TLS_CLIENT_CA` (TLS on the listener itself; none by default),
//! `CARNET_POSTGRES` and `CARNET_S3` (with the features
//! built in; none by default), `CARNET_S3_REGION`, `CARNET_S3_VIRTUAL_HOST`,
//! `CARNET_S3_CA`, `CARNET_KEEP_UNTRACKED_PICTURES`.
//! A bucket's keys come only from the environment: `CARNET_S3_ACCESS_KEY`
//! and `CARNET_S3_SECRET_KEY` (or AWS's own two names).

use std::path::PathBuf;
use std::process::ExitCode;

use carnet_server::{Sharded, Storage};
use carnet_server::server::{self, Config, INVITE_DAYS, OWNER_INVITE_DAYS};

const HELP: &str = "carnet-server: the sync server for Carnet notes

usage:
  carnet-server [--data DIR] [--listen ADDR] [--origin URL] [--web DIR]
                [--metrics ADDR] [--trusted-proxy LIST] [--client-header cf|xff]
                [--tls-cert FILE --tls-key FILE [--tls-client-ca FILE]]
                [--passwords] [--compact-after N] [--postgres URL] [--s3 URL]
                [--keep-untracked-pictures]
  carnet-server invite [--data DIR] [--postgres URL]
  carnet-server backup DIR [--data DIR]
  carnet-server health [--listen ADDR | --metrics ADDR]
  carnet-server migrate [--from DIR] [--to-postgres URL] [--to-s3 URL]
                [--verify-only | --pictures-first]

  --data DIR      where the databases are kept          (CARNET_DATA, ./data)
  --listen ADDR   the address to listen on              (CARNET_LISTEN, 127.0.0.1:8080)
  --origin URL    the URL devices reach this server at  (CARNET_ORIGIN, http://ADDR)
                  behind a proxy: its public https:// address
  --web DIR       the web viewer's build, served at /   (CARNET_WEB, none)
  --metrics ADDR  serve /metrics (Prometheus) there, a listener of its
                  own: keep it private, e.g. 127.0.0.1:9090
                                                        (CARNET_METRICS, none)
  --trusted-proxy LIST  the proxies in front, whose word on who a request
                  is from is taken: addresses or ranges, comma separated,
                  e.g. 127.0.0.1 or 172.16.0.0/12; what the limits on
                  tries and sockets count by    (CARNET_TRUSTED_PROXY, none)
  --client-header cf|xff  the header they say it in: xff, X-Forwarded-For
                  (nginx, Caddy); cf, Cloudflare's CF-Connecting-IP, only
                  when Cloudflare (a Tunnel too) is the proxy named
                                                        (CARNET_CLIENT_HEADER, xff)
  --tls-cert FILE, --tls-key FILE  TLS on the listener itself, with this
                  certificate (PEM, its chain) and key: when nothing in
                  front ends it, as with Cloudflare proxying to an Origin
                  certificate             (CARNET_TLS_CERT, CARNET_TLS_KEY)
  --tls-client-ca FILE  and only clients showing a certificate this CA
                  signed (Cloudflare's Authenticated Origin Pulls)
                                                        (CARNET_TLS_CLIENT_CA)
  --passwords     let devices set a password, and sign in by email and
                  password (OPAQUE: the password never reaches the
                  server)                      (CARNET_PASSWORDS=1, off)
  --compact-after N  a note kept as N records is asked for as one
                  snapshot                              (CARNET_COMPACT_AFTER, 200)
  --postgres URL  keep the records in Postgres instead of the SQLite
                  files: postgres://user:password@host/db (sslmode=require
                  for TLS); built with --features postgres
                                                        (CARNET_POSTGRES, none)
  --s3 URL        keep the pictures in an S3 bucket instead of files:
                  https://host/bucket[/folder]; its keys from
                  CARNET_S3_ACCESS_KEY and CARNET_S3_SECRET_KEY; built
                  with --features s3                    (CARNET_S3, none)
  --s3-region R   the bucket's region         (CARNET_S3_REGION, us-east-1)
  --s3-virtual-host  the bucket as the host's first name, not the path's
                                               (CARNET_S3_VIRTUAL_HOST=1, off)
  --s3-ca FILE    the CA (PEM) the bucket's certificate is signed by, for
                  a bucket of one's own: trusted instead of the web's roots
                                                        (CARNET_S3_CA, none)
  --keep-untracked-pictures  start with no accounts beside pictures of
                  accounts this server has no record of: refused otherwise, as
                  their records were likely not put back (a new data
                  folder, an empty database). The pictures are kept, unused
                                           (CARNET_KEEP_UNTRACKED_PICTURES=1, off)

  invite          print a new invite code: one account, good for a week
                  (with no account yet, the owner's: good for a day)
  backup DIR      copy every database into DIR (new, or empty), safely
                  while it serves, and the pictures unless a bucket keeps
                  them; all of it is sealed or public, so it can go
                  anywhere (the SQLite files only: Postgres has pg_dump)
  health          ask the server listening at ADDR whether it's well:
                  exit 0 if so, 1 if not (a container's health check)
  migrate         copy the data folder (--from, else --data) to Postgres
                  (--to-postgres URL) and its pictures to a bucket
                  (--to-s3 URL), with the server stopped; run it again
                  to go on, it skips what's copied; it checks all of it
                  after, and never changes the folder. --verify-only:
                  only check. --pictures-first: only the pictures, to
                  the bucket, while the server still runs (no lock), so
                  the stopped run after has little left to copy
";

#[derive(Default)]
struct Args {
    invite: bool,
    health: bool,
    migrate: bool,
    verify_only: bool,
    pictures_first: bool,
    backup: Option<PathBuf>,
    data: PathBuf,
    from: Option<PathBuf>,
    listen: String,
    origin: Option<String>,
    compact_after: usize,
    web: Option<PathBuf>,
    metrics: Option<String>,
    trusted: Vec<carnet_server::limits::Network>,
    client_header: carnet_server::limits::ClientHeader,
    passwords: bool,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
    tls_client_ca: Option<PathBuf>,
    postgres: Option<String>,
    s3: Option<String>,
    to_postgres: Option<String>,
    to_s3: Option<String>,
    s3_region: String,
    s3_virtual_host: bool,
    s3_ca: Option<PathBuf>,
    keep_untracked_pictures: bool,
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn args() -> Result<Args, String> {
    let on = |k: &str| env(k).is_some_and(|v| v == "1" || v == "true");
    let mut a = Args {
        data: env("CARNET_DATA").map_or_else(|| PathBuf::from("data"), PathBuf::from),
        listen: env("CARNET_LISTEN").unwrap_or_else(|| "127.0.0.1:8080".into()),
        origin: env("CARNET_ORIGIN"),
        compact_after: server::COMPACT_AFTER,
        web: env("CARNET_WEB").map(PathBuf::from),
        metrics: env("CARNET_METRICS"),
        passwords: on("CARNET_PASSWORDS"),
        tls_cert: env("CARNET_TLS_CERT").map(PathBuf::from),
        tls_key: env("CARNET_TLS_KEY").map(PathBuf::from),
        tls_client_ca: env("CARNET_TLS_CLIENT_CA").map(PathBuf::from),
        postgres: env("CARNET_POSTGRES"),
        s3: env("CARNET_S3"),
        s3_region: env("CARNET_S3_REGION").unwrap_or_else(|| "us-east-1".into()),
        s3_virtual_host: on("CARNET_S3_VIRTUAL_HOST"),
        s3_ca: env("CARNET_S3_CA").map(PathBuf::from),
        keep_untracked_pictures: on("CARNET_KEEP_UNTRACKED_PICTURES"),
        ..Args::default()
    };
    if let Some(list) = env("CARNET_TRUSTED_PROXY") {
        a.trusted = carnet_server::limits::networks(&list).map_err(|e| format!("CARNET_TRUSTED_PROXY: {e}"))?;
    }
    if let Some(h) = env("CARNET_CLIENT_HEADER") {
        a.client_header = h.parse().map_err(|e| format!("CARNET_CLIENT_HEADER: {e}"))?;
    }
    if let Some(n) = env("CARNET_COMPACT_AFTER") {
        a.compact_after = n.parse().map_err(|_| format!("CARNET_COMPACT_AFTER={n}: not a number"))?;
    }
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "invite" => a.invite = true,
            "health" => a.health = true,
            "migrate" => a.migrate = true,
            "backup" => a.backup = Some(value("backup")?.into()),
            "--data" => a.data = value("--data")?.into(),
            "--from" => a.from = Some(value("--from")?.into()),
            "--listen" => a.listen = value("--listen")?,
            "--origin" => a.origin = Some(value("--origin")?),
            "--web" => a.web = Some(value("--web")?.into()),
            "--metrics" => a.metrics = Some(value("--metrics")?),
            "--trusted-proxy" => a.trusted = carnet_server::limits::networks(&value("--trusted-proxy")?).map_err(|e| format!("--trusted-proxy: {e}"))?,
            "--client-header" => a.client_header = value("--client-header")?.parse().map_err(|e| format!("--client-header: {e}"))?,
            "--passwords" => a.passwords = true,
            "--tls-cert" => a.tls_cert = Some(value("--tls-cert")?.into()),
            "--tls-key" => a.tls_key = Some(value("--tls-key")?.into()),
            "--tls-client-ca" => a.tls_client_ca = Some(value("--tls-client-ca")?.into()),
            "--postgres" => a.postgres = Some(value("--postgres")?),
            "--s3" => a.s3 = Some(value("--s3")?),
            "--s3-region" => a.s3_region = value("--s3-region")?,
            "--s3-virtual-host" => a.s3_virtual_host = true,
            "--s3-ca" => a.s3_ca = Some(value("--s3-ca")?.into()),
            "--keep-untracked-pictures" => a.keep_untracked_pictures = true,
            "--to-postgres" => a.to_postgres = Some(value("--to-postgres")?),
            "--to-s3" => a.to_s3 = Some(value("--to-s3")?),
            "--verify-only" => a.verify_only = true,
            "--pictures-first" => a.pictures_first = true,
            "--compact-after" => {
                let n = value("--compact-after")?;
                a.compact_after = n.parse().ok().filter(|&n: &usize| n > 0).ok_or(format!("--compact-after {n}: not a number of records"))?;
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("{other}: not an option")),
        }
    }
    Ok(a)
}

/// A bucket's place and keys: the URL given, the keys from the
/// environment (never a flag: a command line is seen by every user of
/// the machine).
#[cfg(feature = "s3")]
fn s3_config(url: &str, a: &Args) -> Result<carnet_server::s3::S3Config, String> {
    let key = env("CARNET_S3_ACCESS_KEY").or_else(|| env("AWS_ACCESS_KEY_ID")).ok_or("a bucket needs CARNET_S3_ACCESS_KEY (or AWS_ACCESS_KEY_ID)")?;
    let secret = env("CARNET_S3_SECRET_KEY").or_else(|| env("AWS_SECRET_ACCESS_KEY")).ok_or("a bucket needs CARNET_S3_SECRET_KEY (or AWS_SECRET_ACCESS_KEY)")?;
    Ok(carnet_server::s3::S3Config { url: url.to_string(), region: a.s3_region.clone(), key, secret, virtual_host: a.s3_virtual_host, ca: a.s3_ca.clone() })
}

/// The store the server runs on: Postgres if one is given.
fn store(a: &Args) -> Result<Option<server::Store>, String> {
    let Some(url) = &a.postgres else { return Ok(None) };
    #[cfg(feature = "postgres")]
    {
        let pg = carnet_server::postgres::Postgres::open(url).map_err(|e| format!("--postgres: {e}"))?;
        Ok(Some(Box::new(pg)))
    }
    #[cfg(not(feature = "postgres"))]
    {
        let _ = url;
        Err("--postgres: this carnet-server was built without Postgres (cargo build --features postgres)".into())
    }
}

/// Where pictures go: a bucket if one is given.
fn blobs(a: &Args) -> Result<Option<Box<dyn carnet_server::blobs::Blobs>>, String> {
    let Some(url) = &a.s3 else { return Ok(None) };
    #[cfg(feature = "s3")]
    {
        let s3 = carnet_server::s3::S3Blobs::new(&s3_config(url, a)?)?;
        s3.check()?;
        s3.serving()?;
        Ok(Some(Box::new(s3)))
    }
    #[cfg(not(feature = "s3"))]
    {
        let _ = url;
        Err("--s3: this carnet-server was built without S3 (cargo build --features s3)".into())
    }
}

fn migrate(a: &Args) -> Result<(), String> {
    #[cfg(feature = "postgres")]
    {
        let plan = carnet_server::migrate::Plan {
            from: a.from.clone().unwrap_or_else(|| a.data.clone()),
            postgres: a.to_postgres.clone(),
            #[cfg(feature = "s3")]
            s3: a.to_s3.as_deref().map(|u| s3_config(u, a)).transpose()?,
            verify_only: a.verify_only,
            pictures_first: a.pictures_first,
        };
        #[cfg(not(feature = "s3"))]
        if a.to_s3.is_some() {
            return Err("--to-s3: this carnet-server was built without S3 (cargo build --features s3)".into());
        }
        let done = carnet_server::migrate::run(&plan, &mut |line| eprintln!("carnet-server: {line}")).map_err(|e| e.0)?;
        if a.pictures_first {
            eprintln!(
                "carnet-server: pictures copied first: {} put now ({} bytes), {} there already. The folder is as it was; stop the server, then migrate.",
                done.put, done.put_bytes, done.pictures_skipped
            );
            return Ok(());
        }
        let what = if a.verify_only { "checked" } else { "migrated and checked" };
        eprintln!(
            "carnet-server: {what}: {} accounts ({} copied now, {} the same already), {} pictures ({} put now, {} there already). The folder is as it was.",
            done.rows.accounts, done.copied, done.skipped, done.pictures, done.put, done.pictures_skipped
        );
        Ok(())
    }
    #[cfg(not(feature = "postgres"))]
    {
        let _ = a;
        Err("migrate: this carnet-server was built without Postgres (cargo build --features postgres,s3)".into())
    }
}

fn main() -> ExitCode {
    let a = match args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("carnet-server: {e}\n");
            }
            eprint!("{HELP}");
            return if e.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(2) };
        }
    };
    let fail = |e: String| {
        eprintln!("carnet-server: {e}");
        ExitCode::FAILURE
    };
    if a.health {
        // With TLS, the public listener may ask a client certificate the
        // check hasn't: the metrics address answers it in plain HTTP.
        let at = match (&a.tls_cert, &a.metrics) {
            (None, _) => &a.listen,
            (Some(_), Some(m)) => m,
            (Some(_), None) => return fail("with TLS, health is asked on the metrics address: give --metrics (CARNET_METRICS)".into()),
        };
        return match health(at) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(e),
        };
    }
    if a.migrate {
        return match migrate(&a) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(e),
        };
    }
    if let Some(to) = &a.backup {
        if a.postgres.is_some() {
            return fail("backup copies the SQLite files; with Postgres, back up with pg_dump (and the bucket by its own means)".into());
        }
        // With a bucket, the pictures are its to back up: a folder of
        // them here is from before it, and left out.
        return match Sharded::open(&a.data).and_then(|s| s.backup_with(to, a.s3.is_none())) {
            Ok(n) => {
                let accounts = if n == 1 { "account" } else { "accounts" };
                eprintln!("carnet-server: backed up {n} {accounts} to {}", to.display());
                ExitCode::SUCCESS
            }
            Err(e) => fail(e.to_string()),
        };
    }
    if a.invite {
        // With no account yet, it's the owner's: good for a day.
        let made = match store(&a) {
            Ok(Some(mut s)) => {
                let owner = s.accounts() == 0;
                server::new_invite(&mut s).map(|code| (code, owner))
            }
            Ok(None) => Sharded::open(&a.data).and_then(|mut s| {
                let owner = s.accounts() == 0;
                server::new_invite(&mut s).map(|code| (code, owner))
            }),
            Err(e) => return fail(e),
        };
        return match made {
            Ok((code, owner)) => {
                println!("{}", code.text());
                if owner {
                    eprintln!("carnet-server: for the first account (owner); single use, good for {OWNER_INVITE_DAYS} day: if it has run out, run this again for a new one");
                } else {
                    eprintln!("carnet-server: single use, good for {INVITE_DAYS} days");
                }
                ExitCode::SUCCESS
            }
            Err(e) => fail(e.to_string()),
        };
    }
    let (store, blobs) = match (store(&a), blobs(&a)) {
        (Ok(s), Ok(b)) => (s, b),
        (Err(e), _) | (_, Err(e)) => return fail(e),
    };
    let tls = match (&a.tls_cert, &a.tls_key) {
        (Some(cert), Some(key)) => match carnet_server::tls::Tls::load(cert, key, a.tls_client_ca.as_deref()) {
            Ok(t) => Some(t),
            Err(e) => return fail(e.to_string()),
        },
        (None, None) if a.tls_client_ca.is_none() => None,
        _ => return fail("TLS needs --tls-cert and --tls-key both (a client CA only with them)".into()),
    };
    let scheme = if tls.is_some() { "https" } else { "http" };
    let origin = a.origin.unwrap_or_else(|| format!("{scheme}://{}", a.listen));
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => return fail(e.to_string()),
    };
    let served = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(&a.listen).await?;
        let metrics = match &a.metrics {
            Some(addr) => Some(tokio::net::TcpListener::bind(addr).await?),
            None => None,
        };
        let stop = async {
            stopped().await;
            println!("carnet-server: stopping");
        };
        let host = server::Host { passwords: a.passwords, store, blobs, ..server::Host::default() };
        let config = Config { data: a.data, origin, compact_after: a.compact_after, web: a.web, trusted: a.trusted, client_header: a.client_header, tls };
        let start = server::Start { keep_untracked_pictures: a.keep_untracked_pictures };
        server::serve_started(config, listener, metrics, host, server::SocketLimits::default(), start, stop).await
    });
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e.to_string()),
    }
}

/// Ctrl+C, or SIGTERM where there is one: what `docker stop` and systemd
/// send, and wait a while for before they kill.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// `GET /v1/health` of the server listening at `listen`, by hand: the
/// image has no HTTP client, and this needs one request. A server on every
/// address (`0.0.0.0`) is asked on this machine's own.
fn health(listen: &str) -> Result<(), String> {
    use std::io::{Read, Write};
    let addr: std::net::SocketAddr = listen.parse().map_err(|_| format!("{listen}: not an address"))?;
    let addr = if addr.ip().is_unspecified() { std::net::SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), addr.port()) } else { addr };
    let timeout = std::time::Duration::from_secs(5);
    let mut s = std::net::TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("{addr}: {e}"))?;
    s.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    write!(s, "GET /v1/health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").map_err(|e| e.to_string())?;
    let mut reply = String::new();
    s.read_to_string(&mut reply).map_err(|e| e.to_string())?;
    if reply.starts_with("HTTP/1.1 200") { Ok(()) } else { Err(format!("not well: {}", reply.lines().next().unwrap_or("no answer"))) }
}
