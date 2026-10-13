//! A server beside pictures it has no records for (the deploy drill's
//! hazard): its records not put back (a new data folder, an empty
//! database), or put back from before some accounts were made, on a
//! bucket a server kept pictures in. The sweep takes the pictures of
//! accounts removed here (recorded so in the bucket, `gone/`), and no
//! others; a store with no account refuses to start beside others'
//! pictures unless told to.
//!
//! With the `s3` feature, against a real bucket (`CARNET_TEST_S3`, as in
//! `tests/s3.rs`), and Postgres (`CARNET_TEST_POSTGRES`, with the
//! `postgres` feature) for a database put back from an older backup.
//! Without them, each test says so and passes.
#![cfg(feature = "s3")]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use carnet_server::Storage;
use carnet_server::blobs::{Blobs, Job, Unowned, Worker};
use carnet_server::s3::{S3Blobs, S3Config};
use carnet_server::server::{self, Config, Host, Start};
use common::{bucket, empty};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("carnet-old-pictures-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A bucket's removal records: put by `forget`, before its pictures go;
/// not an account folder; and the sweep takes what's put after, of that
/// account alone.
#[test]
fn a_bucket_records_a_removal_and_the_sweep_keeps_strangers() {
    let Some((s3, config)) = bucket("removals") else { return };
    let mut w = Worker::new(s3.clone());
    for a in [&b"here"[..], b"removed", b"stranger"] {
        s3.put(&s3.key(a, &[1; 32]), b"sealed").unwrap();
    }
    w.run(Job::Gone { account: b"removed".to_vec() }, &mut |_| {});
    assert_eq!(w.blobs.held(b"removed"), [], "its pictures gone");
    assert_eq!(w.blobs.removed(), [b"removed".to_vec()].into(), "its removal recorded");
    let mut owners = w.blobs.owners();
    owners.sort();
    assert_eq!(owners, [b"here".to_vec(), b"stranger".to_vec()], "the record isn't an account's folder");
    // A put by a link given before, after: a bucket can't refuse it.
    s3.put(&s3.key(b"removed", &[2; 32]), b"late").unwrap();
    let swept = w.sweep_unowned(|owners| owners.into_iter().filter(|a| a != b"here").collect());
    assert_eq!(swept, Unowned { gone: 1, kept: 1 });
    assert_eq!(w.blobs.held(b"removed"), []);
    assert_eq!(w.blobs.held(b"stranger").len(), 1, "an account the store doesn't know, never removed here: kept");
    assert_eq!(w.blobs.removed().len(), 1, "the record stays, for a later put");
    empty(&s3, &config);
}

/// The bucket's pictures: one of an account `old`, never removed here,
/// and one of `removed`, removed (recorded), put after.
fn fill(s3: &mut S3Blobs) {
    s3.put(&s3.key(b"old", &[1; 32]), b"sealed").unwrap();
    s3.forget(b"removed");
    s3.put(&s3.key(b"removed", &[1; 32]), b"late").unwrap();
}

type Running = (oneshot::Sender<()>, tokio::task::JoinHandle<std::io::Result<()>>);

/// A server on the bucket and `store` (SQLite in `data` if none): how its
/// start went, and it running, if it did.
async fn serve(data: &Path, config: &S3Config, store: Option<server::Store>, start: Start) -> (std::io::Result<()>, Option<Running>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (ready_tx, ready) = oneshot::channel();
    let host = Host {
        store,
        blobs: Some(Box::new(S3Blobs::new(config).unwrap())),
        ready: Some(Box::new(move |_| {
            let _ = ready_tx.send(());
        })),
        ..Host::default()
    };
    let (stop, rx) = oneshot::channel::<()>();
    let server = Config { data: data.to_path_buf(), origin: format!("http://{addr}"), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None };
    let done = tokio::spawn(server::serve_started(server, listener, None, host, server::SocketLimits::default(), start, async {
        let _ = rx.await;
    }));
    // Ready once the hub runs; dropped unsent if the start is refused.
    match ready.await {
        Ok(()) => (Ok(()), Some((stop, done))),
        Err(_) => (tokio::time::timeout(Duration::from_secs(10), done).await.expect("ends").unwrap(), None),
    }
}

async fn stop((stop, done): Running) {
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.expect("stops").unwrap().unwrap();
}

/// How many pictures each account has in the bucket (off the async
/// threads: the bucket's client blocks).
async fn held(s3: &S3Blobs, accounts: &'static [&'static [u8]]) -> Vec<usize> {
    let mut b = s3.clone();
    tokio::task::spawn_blocking(move || accounts.iter().map(|a| b.held(a).len()).collect()).await.unwrap()
}

/// Waits (up to five seconds) for the account's pictures to be gone.
async fn goes(s3: &S3Blobs, account: &'static [u8]) -> bool {
    for _ in 0..50 {
        let mut b = s3.clone();
        if tokio::task::spawn_blocking(move || b.held(account).is_empty()).await.unwrap() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// A new data folder on a bucket a server kept pictures in: refused, as
/// it says, nothing made or deleted. Told to start anyway, it does: the
/// removed account's late picture goes, the other is kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_data_folder_on_a_used_bucket_refuses_to_start() {
    let Some((s3, config)) = bucket("new-folder") else { return };
    let data = temp("new-folder");
    let mut b = s3.clone();
    tokio::task::spawn_blocking(move || fill(&mut b)).await.unwrap();

    let (refused, running) = serve(&data, &config, None, Start::default()).await;
    assert!(running.is_none());
    let said = refused.expect_err("refused").to_string();
    assert!(said.contains("The bucket holds pictures of 1 account, and there are no accounts here"), "{said}");
    assert!(said.contains("another bucket (or another folder in it)") && said.contains("--beside-old-pictures"), "{said}");
    assert!(carnet_server::Sharded::open(&data).unwrap().invites().is_empty(), "no invite made");
    assert_eq!(held(&s3, &[b"old", b"removed"]).await, [1, 1], "nothing deleted");

    let (started, running) = serve(&data, &config, None, Start { beside_old_pictures: true }).await;
    started.unwrap();
    assert!(goes(&s3, b"removed").await, "the removed account's late picture: gone");
    assert_eq!(held(&s3, &[b"old"]).await, [1], "the other kept");
    stop(running.unwrap()).await;
    empty(&s3, &config);
    let _ = std::fs::remove_dir_all(&data);
}

/// Postgres: an empty database on a used bucket refuses to start, as a
/// new data folder does; one put back from an older backup (it has
/// accounts, not every one the bucket has) starts, and its sweep takes
/// the removed account's late picture and keeps those of the account
/// made after the backup.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_database_put_back_from_before_keeps_the_newer_accounts_pictures() {
    use carnet_server::postgres::Postgres;
    let Some((url, _schema)) = common::database("old_pictures") else { return };
    let Some((s3, config)) = bucket("restored") else { return };
    let data = temp("restored");
    let mut b = s3.clone();
    tokio::task::spawn_blocking(move || fill(&mut b)).await.unwrap();

    let open = |url: String| tokio::task::spawn_blocking(move || Postgres::open(&url).unwrap());
    let pg = open(url.clone()).await.unwrap();
    let (refused, running) = serve(&data, &config, Some(Box::new(pg)), Start::default()).await;
    assert!(running.is_none());
    let said = refused.expect_err("refused").to_string();
    assert!(said.contains("The bucket holds pictures of 1 account"), "{said}");

    // The backup's: an account from before, with a picture of its own.
    let mut pg = open(url.clone()).await.unwrap();
    let pg = tokio::task::spawn_blocking(move || {
        pg.add_account(b"before", true).unwrap();
        pg.add_device(b"before", b"d", [1; 32]).unwrap();
        pg
    })
    .await
    .unwrap();
    let b = s3.clone();
    tokio::task::spawn_blocking(move || b.put(&b.key(b"before", &[1; 32]), b"sealed").unwrap()).await.unwrap();
    let (started, running) = serve(&data, &config, Some(Box::new(pg)), Start::default()).await;
    started.unwrap();
    assert!(goes(&s3, b"removed").await, "the removed account's late picture: gone");
    assert_eq!(held(&s3, &[b"before", b"old"]).await, [1, 1], "the account's own, and the newer account's, kept");
    stop(running.unwrap()).await;
    empty(&s3, &config);
    let _ = std::fs::remove_dir_all(&data);
}
