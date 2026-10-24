//! `migrate` (`migrate.rs`): a data folder copied to Postgres and a
//! bucket, whole, checked, and again with nothing more to do; a folder in
//! use, or a database or bucket a server ran on, refused. And the whole
//! way end to end: a server on SQLite filled over its sockets, moved,
//! started on Postgres and a bucket, and serving what it kept as before
//! (notes, a deleted one refused, a file by the bucket's link). The
//! apps know nothing of it: to them it's the same server.
//!
//! With the `postgres` feature (and `s3` for the files), against a
//! real database (`CARNET_TEST_POSTGRES`) and bucket (`CARNET_TEST_S3`,
//! as in `tests/s3.rs`). Without them, each test says so and passes.
#![cfg(feature = "postgres")]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use carnet_server::blobs::BlobFiles;
use carnet_server::migrate::{self, Plan};
use carnet_server::postgres::Postgres;
use carnet_server::storage::{Logged, PasskeySlot, PasswordSlot};
use carnet_server::{Sharded, Storage};
use common::database;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("carnet-migrate-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

const R: &str = "000000000000000000000000000000r1";

/// A folder with a bit of everything a server keeps: two accounts,
/// devices (one removed by the other, its keys sealed again), records
/// live and set aside by a base, a room removed, the log, a recovery
/// code, a password, two passkeys, invites, files on disk (one marked to go) and
/// one still in an account's database.
fn fill(data: &Path) {
    let mut s = Sharded::open(data).unwrap();
    let (alice, bob) = (b"alice".as_slice(), b"bob".as_slice());
    s.add_account(alice, true).unwrap();
    s.add_account(bob, false).unwrap();
    s.add_device(alice, b"d1", [1; 32]).unwrap();
    s.add_device(alice, b"d2", [2; 32]).unwrap();
    s.set_card(alice, b"d1", vec![9, 9]).unwrap();
    s.set_seen(alice, b"d1", 77).unwrap();
    s.put(alice, R, 1, 0, 5, b"a".to_vec()).unwrap();
    s.put(alice, R, 2, 0, 3, b"b".to_vec()).unwrap();
    s.put(alice, R, 1, 5, 8, b"c".to_vec()).unwrap();
    let seq = s.set_base(alice, R, vec![(1, 5), (2, 3)], b"S".to_vec(), b"d1", 100).unwrap();
    s.verify_base(alice, R, seq).unwrap();
    s.put(alice, "gone", 1, 0, 2, b"x".to_vec()).unwrap();
    s.remove_room(alice, "gone", 150).unwrap();
    s.set_recovery(alice, [5; 32], vec![1, 2, 3], vec![4]).unwrap();
    s.set_password(alice, PasswordSlot { id: "ann@example.com".into(), record: vec![8; 40], public: [6; 32], wrapped: vec![7; 20], card: vec![3] }).unwrap();
    s.add_passkey(alice, PasskeySlot { public: [9; 32], wrapped: vec![1; 8], card: vec![2], made: 60 }).unwrap();
    s.add_passkey(alice, PasskeySlot { public: [10; 32], wrapped: vec![3; 8], card: vec![4], made: 50 }).unwrap();
    let passkey = carnet_sync::passkey::slot(&[9; 32]);
    s.rotate(
        alice,
        b"d2",
        1,
        vec![(b"d1".to_vec(), vec![11]), (Vec::new(), vec![12]), (carnet_sync::password::SLOT.to_vec(), vec![13]), (passkey.clone(), vec![16])],
        vec![(b"d1".to_vec(), vec![14]), (passkey, vec![15])],
    )
    .unwrap();
    s.log(alice, Logged { at: 120, device: b"d1".to_vec(), room: R.into(), what: "rolled back".into() }).unwrap();
    s.add_invite([7; 32], false, 9_999_999_999).unwrap();
    s.add_invite([8; 32], true, 5).unwrap();
    s.put(bob, R, 3, 0, 1, b"bob's".to_vec()).unwrap();
    drop(s);
    let files = BlobFiles::open(data).unwrap();
    files.keep(alice, &[1; 32], b"FILE-ONE").unwrap();
    files.keep(alice, &[2; 32], b"FILE-TWO").unwrap();
    files.mark(alice, &[2; 32], 1234).unwrap();
    // One a server before kept in the database, not moved out yet.
    let db = rusqlite::Connection::open(Sharded::account_path(data, bob).unwrap()).unwrap();
    db.execute("INSERT INTO blobs (id, bytes) VALUES (?1, ?2)", rusqlite::params![[3u8; 32].as_slice(), b"FILE-OLD".as_slice()]).unwrap();
}

/// Every answer the server asks of a store, the same from both.
fn same_answers(a: &dyn Storage, b: &dyn Storage) {
    assert_eq!(a.account_list(), b.account_list());
    assert_eq!(a.invites(), b.invites());
    assert_eq!(a.accounts(), b.accounts());
    for (acct, ..) in a.account_list() {
        let acct = acct.as_slice();
        assert_eq!(a.devices(acct), b.devices(acct));
        for (d, _) in a.devices(acct) {
            assert_eq!(a.rekeys(acct, &d), b.rekeys(acct, &d));
            assert_eq!(a.device(acct, &d), b.device(acct, &d));
        }
        for d in [b"".to_vec(), carnet_sync::password::SLOT.to_vec(), carnet_sync::passkey::slot(&[9; 32])] {
            assert_eq!(a.rekeys(acct, &d), b.rekeys(acct, &d));
        }
        assert_eq!(a.passkeys(acct), b.passkeys(acct));
        assert_eq!((a.epoch(acct), a.seq(acct), a.is_owner(acct)), (b.epoch(acct), b.seq(acct), b.is_owner(acct)));
        assert_eq!(a.rooms(acct), b.rooms(acct));
        // What a write is judged by, counted on the way in.
        assert_eq!(a.room_count(acct), b.room_count(acct));
        for room in [R, "gone"] {
            assert_eq!(a.live(acct, room), b.live(acct, room));
            assert_eq!(a.spans(acct, room), b.spans(acct, room));
            assert_eq!(a.base(acct, room), b.base(acct, room));
            assert_eq!(a.shelf(acct, room), b.shelf(acct, room));
            assert_eq!(a.is_removed(acct, room), b.is_removed(acct, room));
        }
        assert_eq!(a.changed_since(acct, 0), b.changed_since(acct, 0));
        assert_eq!(a.usage(acct), b.usage(acct));
        assert_eq!(a.logged(acct), b.logged(acct));
        assert_eq!(a.recovery_card(acct), b.recovery_card(acct));
        assert_eq!(a.password_card(acct), b.password_card(acct));
    }
    assert_eq!(a.recovery(&[5; 32]), b.recovery(&[5; 32]));
    assert_eq!(a.password_of("ann@example.com"), b.password_of("ann@example.com"));
    assert_eq!(a.password_slot(&[6; 32]), b.password_slot(&[6; 32]));
    for k in [[9; 32], [10; 32]] {
        assert_eq!(a.passkey_slot(&k), b.passkey_slot(&k));
    }
}

/// Every file of the folder and its bytes, but the WAL's scratch files
/// SQLite keeps beside a database while it's open.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(p) = todo.pop() {
        if p.is_dir() {
            todo.extend(std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()));
        } else if !p.to_string_lossy().ends_with("-shm") && !p.to_string_lossy().ends_with("-wal") {
            out.insert(p.clone(), std::fs::read(&p).unwrap());
        }
    }
    out
}

fn plan(data: &Path, url: &str) -> Plan {
    Plan { from: data.to_path_buf(), postgres: Some(url.to_string()), ..Plan::default() }
}

/// Copied whole, row for row, every answer the same; again, nothing to
/// do; a change in the folder, that account again; a change on the other
/// side, found by the check and put right by a run. The folder never
/// changes.
#[test]
fn migrate_copies_a_folder_whole_and_a_rerun_finds_nothing() {
    let Some((url, _schema)) = database("whole") else { return };
    let data = temp("whole");
    fill(&data);
    let before = snapshot(&data);
    #[allow(unused_mut)]
    let mut plan = plan(&data, &url);
    #[cfg(feature = "s3")]
    let bucket = common::bucket("migrate-whole");
    #[cfg(feature = "s3")]
    {
        plan.s3 = bucket.as_ref().map(|(_, c)| c.clone());
    }
    let mut said = Vec::new();
    let done = migrate::run(&plan, &mut |l| said.push(l.to_string())).unwrap_or_else(|e| panic!("{e}: {said:#?}"));
    assert_eq!(snapshot(&data), before, "the folder as it was, byte for byte");
    assert_eq!((done.copied, done.skipped, done.rows.accounts, done.invites), (2, 0, 2, 2), "{said:#?}");
    assert_eq!((done.rows.devices, done.rows.records, done.rows.bases, done.rows.removed, done.rows.passwords, done.rows.recovery, done.rows.passkeys), (2, 4, 1, 1, 1, 1, 2));
    let pg = Postgres::target(&url).unwrap();
    let keys = pg.passkeys(b"alice");
    assert_eq!(keys.iter().map(|k| (k.public[0], k.card.clone())).collect::<Vec<_>>(), [(10, vec![4]), (9, vec![15])], "the oldest first, the card sealed anew");
    drop(pg);
    assert_eq!(done.sums.len(), 2);
    let pg = Postgres::target(&url).unwrap();
    same_answers(&Sharded::open(&data).unwrap(), &pg);
    assert!(!pg.served().unwrap(), "migrate isn't a server");
    #[cfg(feature = "s3")]
    if let Some((s3, _)) = &bucket {
        assert_eq!((done.files, done.put, done.marks), (3, 3, 1), "two files and one from a database; one marked");
        assert_eq!(s3.get(&s3.key(b"bob", &[3; 32])).unwrap().as_deref(), Some(b"FILE-OLD".as_slice()));
        assert_eq!(s3.marked(b"alice", &[2; 32]).unwrap(), Some(1234));
    }

    // Again: the same, so nothing copied.
    let again = migrate::run(&plan, &mut |_| {}).unwrap();
    assert_eq!((again.copied, again.skipped, again.put), (0, 2, 0));
    assert_eq!(again.sums, done.sums);

    // The old server ran again meanwhile: bob's account changed, copied again.
    let mut s = Sharded::open(&data).unwrap();
    s.put(b"bob", R, 3, 1, 2, b"more".to_vec()).unwrap();
    s.drop_invite([8; 32]).unwrap();
    drop(s);
    let before = {
        let mut b = before;
        b.retain(|p, _| !p.starts_with(data.join("accounts")) && p != &Sharded::directory_path(&data));
        b
    };
    let third = migrate::run(&plan, &mut |_| {}).unwrap();
    assert_eq!((third.copied, third.skipped, third.invites), (1, 1, 1));
    same_answers(&Sharded::open(&data).unwrap(), &pg);

    // Changed on the other side: the check finds it; a run puts it right.
    let mut c = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    c.execute("UPDATE records SET bytes = '\\x00' WHERE account = 'alice'::bytea AND seq = 3", &[]).unwrap();
    let mut said = Vec::new();
    let checked = migrate::run(&Plan { verify_only: true, ..plan.clone() }, &mut |l| said.push(l.to_string()));
    assert!(checked.is_err(), "{said:#?}");
    assert!(said.iter().any(|l| l.contains(&"alice".bytes().map(|b| format!("{b:02x}")).collect::<String>()) && l.contains("differs")), "{said:#?}");
    let fixed = migrate::run(&plan, &mut |_| {}).unwrap();
    assert_eq!(fixed.copied, 1);
    #[cfg(feature = "s3")]
    if let Some((s3, config)) = &bucket {
        // A file gone from the bucket is put again; one only there goes.
        s3.delete(&s3.key(b"alice", &[1; 32])).unwrap();
        s3.put(&s3.key(b"alice", &[9; 32]), b"STRAY").unwrap();
        let mended = migrate::run(&plan, &mut |_| {}).unwrap();
        assert_eq!((mended.put, mended.files_skipped, mended.files), (1, 2, 3));
        assert!(s3.head(&s3.key(b"alice", &[9; 32])).unwrap().is_none());
        common::empty(s3, config);
    }

    // The files on disk and what isn't a database: as they were. (The
    // databases are compared by their rows above: SQLite may fold a WAL
    // into its file as any open does.)
    let after = snapshot(&data);
    for (p, bytes) in &before {
        assert_eq!(after.get(p), Some(bytes), "{} changed", p.display());
    }
    let _ = std::fs::remove_dir_all(&data);
}

/// A folder a server has open is refused, not waited for; a database or
/// bucket a server has run on isn't copied over.
#[test]
fn migrate_refuses_a_folder_or_database_in_use() {
    let Some((url, _schema)) = database("refused") else { return };
    let data = temp("refused");
    fill(&data);
    {
        let open = Sharded::open(&data).unwrap();
        let _ = open.accounts();
        let e = migrate::run(&plan(&data, &url), &mut |_| {}).unwrap_err();
        assert!(e.0.contains("in use"), "{e}");
    }
    assert!(migrate::run(&Plan { from: data.clone(), ..Plan::default() }, &mut |_| {}).is_err(), "nowhere to go");

    // A server ran on it: it has what the folder hasn't.
    drop(Postgres::open(&url).unwrap());
    let e = migrate::run(&plan(&data, &url), &mut |_| {}).unwrap_err();
    assert!(e.0.contains("a server has run"), "{e}");
    #[cfg(feature = "s3")]
    if let Some((s3, config)) = common::bucket("migrate-refused") {
        s3.serving().unwrap();
        let p = Plan { from: data.clone(), s3: Some(config.clone()), ..Plan::default() };
        let e = migrate::run(&p, &mut |_| {}).unwrap_err();
        assert!(e.0.contains("a server has run"), "{e}");
        common::empty(&s3, &config);
    }
    let _ = std::fs::remove_dir_all(&data);
}

/// A database from a newer server is refused; a connection that drops is
/// made again, and what comes after goes through.
#[test]
fn postgres_refuses_a_newer_schema_and_reconnects() {
    let Some((url, _schema)) = database("conn") else { return };
    // Named, so only this test's connection is cut (others run beside it).
    let url = format!("{url}&application_name=carnet_reconnect_{}", std::process::id());
    let mut pg = Postgres::open(&url).unwrap();
    pg.add_account(b"a", true).unwrap();
    let mut c = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    c.batch_execute(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid <> pg_backend_pid() AND application_name = 'carnet_reconnect_{}'",
        std::process::id()
    ))
    .unwrap();
    // The first ask after may fail with the connection; the next makes a
    // new one.
    let _ = pg.add_device(b"a", b"d", [1; 32]);
    pg.add_device(b"a", b"d", [1; 32]).unwrap();
    assert_eq!(pg.devices(b"a").len(), 1);
    c.batch_execute("UPDATE carnet_schema SET version = 99").unwrap();
    let e = Postgres::open(&url).err().expect("refused");
    assert!(e.0.contains("newer"), "{e}");
}

/// A server of one's own on Postgres, as the binary starts it: from an
/// async caller, its first invite made in the database, and well.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_starts_on_postgres_with_its_first_invite() {
    use carnet_server::server::{self, Config, Host};
    let Some((url, _schema)) = database("start") else { return };
    let data = temp("start");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let host = Host { store: Some(Box::new(Postgres::open(&url).unwrap())), ..Host::default() };
    let config = Config { data: data.clone(), origin: format!("http://{addr}"), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None };
    let done = tokio::spawn(server::serve_hosted(config, listener, None, host, async {
        let _ = rx.await;
    }));
    let mut well = false;
    for _ in 0..50 {
        if let Ok(r) = reqwest_free_get(addr, "/v1/health").await
            && r.starts_with("HTTP/1.1 200")
        {
            well = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(well, "the server answers");
    // (The database's client blocks: off this async test's thread.)
    let invites = tokio::task::spawn_blocking(move || Postgres::target(&url).unwrap().invites()).await.unwrap();
    assert_eq!(invites.len(), 1, "the owner's invite, in the database");
    assert!(invites[0].1);
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done).await.unwrap().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&data);
}

/// A GET by hand: what came back, as text.
async fn reqwest_free_get(addr: std::net::SocketAddr, path: &str) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await?;
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
    let mut out = String::new();
    s.read_to_string(&mut out).await?;
    Ok(out)
}

/// The whole way: a server on SQLite, filled by devices over its sockets
/// (notes, a second device and then not, a password, a file, a note
/// deleted for good); stopped; migrated; started on Postgres and a
/// bucket. A device logs in and has every note, fetches the file by
/// the bucket's link; the password lets a new device in; the deleted
/// note stays deleted. And `migrate` won't run over it again.
#[cfg(feature = "s3")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_to_postgres_and_a_bucket_serves_what_it_kept() {
    use carnet_server::s3::S3Blobs;
    use carnet_server::server::{self, Host};
    use carnet_sync::crypto::{DeviceKey, OsRandom, Ring, VaultKey};
    use carnet_sync::pair::{Joiner, Joining, PairCode, PasswordJoiner};
    use carnet_sync::session::{Event, Identity, Transfer};
    use device::{Device, enroll};
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let Some((url, _schema)) = database("e2e") else { return };
    let Some((s3, s3_config)) = common::bucket("migrate-e2e") else { return };
    let data = temp("e2e");
    std::fs::create_dir_all(&data).unwrap();
    let code = server::new_invite(&mut Sharded::open(&data).unwrap()).unwrap();

    // The server as it was: SQLite and files, passwords on.
    let first = device::start(&data, None, Host { passwords: true, ..Host::default() }).await;
    let origin = format!("http://{}", first.addr);
    let a_key = DeviceKey::generate(&mut OsRandom);
    let account = enroll(first.addr, &code.text(), b"\x0a", a_key.public()).await.unwrap();
    let vault = VaultKey::generate(&mut OsRandom);
    let identity = Identity { origin: origin.clone(), account: account.clone(), device: b"\x0a".to_vec(), key: a_key, ring: Ring::new(vault), name: String::new() };
    let mut a = Device::new(identity.clone(), 1);
    let mut ws = a.connect(first.addr).await;
    a.settle(&mut ws).await;
    a.write(device::ROOM, "# Shop\n\n- [ ] milk\n");
    a.write(device::OTHER, "a note to delete");
    a.settle(&mut ws).await;
    a.session.remove_room(device::OTHER);
    a.settle(&mut ws).await;
    assert!(a.events.contains(&Event::RoomRemoved(device::OTHER.into())), "{:?}", a.events);

    // A second device, paired, then removed by the first: a new key epoch.
    a.session.pair_open(&mut OsRandom);
    a.settle(&mut ws).await;
    let pair = a.events.iter().find_map(|e| match e {
        Event::PairCode(c) => PairCode::parse(c),
        _ => None,
    });
    let mut joiner = Joiner::new(pair.unwrap(), b"\x0b".to_vec(), DeviceKey::generate(&mut OsRandom));
    let (mut ws_b, _) = tokio_tungstenite::connect_async(format!("ws://{}/v1/sync?protocol={}", first.addr, carnet_sync::compat::PROTOCOL)).await.unwrap();
    for _ in 0..200 {
        if !matches!(joiner.state, Joining::Going) {
            break;
        }
        for out in a.session.take_out() {
            ws.send(Message::Binary(out.into())).await.unwrap();
        }
        for out in joiner.take_out() {
            ws_b.send(Message::Binary(out.into())).await.unwrap();
        }
        tokio::select! {
            Some(Ok(Message::Binary(m))) = ws.next() => {
                a.session.receive(&m, &mut a.notes, &mut OsRandom);
                a.events.extend(a.session.take_events());
            }
            Some(Ok(Message::Binary(m))) = ws_b.next() => joiner.receive(&m, &mut OsRandom),
            _ = tokio::time::sleep(Duration::from_millis(300)) => {}
        }
    }
    assert!(matches!(joiner.state, Joining::In(_)), "{:?}", joiner.state);
    drop(ws_b);
    a.session.list_devices();
    a.settle(&mut ws).await;
    a.session.revoke(b"\x0b", &mut OsRandom).unwrap();
    a.settle(&mut ws).await;
    a.write(device::ROOM, "# Shop\n\n- [ ] milk\n- [ ] eggs\n");
    a.settle(&mut ws).await;

    // A password, and a file put at the server's own link.
    a.session.set_password("ann@example.com", "correct horse battery", &mut OsRandom).unwrap();
    a.settle(&mut ws).await;
    assert!(a.events.contains(&Event::PasswordSet), "{:?}", a.events);
    let pic: Vec<u8> = (0..200_000u32).map(|i| (i % 249) as u8).collect();
    let id = a.session.blob_id(&pic);
    a.session.offer_blobs(vec![(id, pic.len())]);
    a.settle(&mut ws).await;
    assert!(a.session.put_blob(&pic, &mut OsRandom));
    let http = carnet_sync::http::Http::start(Vec::new(), || {}).unwrap();
    let transfers = a.session.take_transfers();
    assert!(matches!(&transfers[..], [Transfer::Put { url, .. }] if url.starts_with(&origin)));
    let done = device::transfer(&http, transfers).await;
    a.session.transferred(done.id, done.put, done.result);
    assert!(a.session.take_events().contains(&Event::BlobStored(id)));
    drop(ws);
    first.stop().await;

    // Moved.
    let plan = Plan { from: data.clone(), postgres: Some(url.clone()), s3: Some(s3_config.clone()), ..Plan::default() };
    let mut said = Vec::new();
    let done = migrate::run(&plan, &mut |l| said.push(l.to_string())).unwrap_or_else(|e| panic!("{e}: {said:#?}"));
    assert_eq!((done.rows.accounts, done.files, done.put), (1, 1, 1), "{said:#?}");

    // Started on Postgres and the bucket, the same data folder beside them
    // for its keys (the OPAQUE key above all), at the same origin.
    let host = Host {
        passwords: true,
        first_invite: false,
        store: Some(Box::new(Postgres::open(&url).unwrap())),
        blobs: Some(Box::new({
            let s = S3Blobs::new(&s3_config).unwrap();
            s.serving().unwrap();
            s
        })),
        ..Host::default()
    };
    let second = device::start(&data, Some(&origin), host).await;

    // A device of the account, from nothing but its keys: every note but
    // the one deleted, and the file by the bucket's link.
    let mut again = Device::new(Identity { ring: a.session.ring().clone(), ..identity.clone() }, 3);
    let mut ws = again.connect(second.addr).await;
    again.settle(&mut ws).await;
    assert!(again.events.contains(&Event::LoggedIn), "{:?}", again.events);
    assert_eq!(again.text(device::ROOM).as_deref(), Some("# Shop\n\n- [ ] milk\n- [ ] eggs\n"));
    assert_eq!(again.text(device::OTHER), None, "deleted for good");
    again.session.get_blob(id);
    again.settle(&mut ws).await;
    let transfers = again.session.take_transfers();
    let [Transfer::Get { url: link, .. }] = &transfers[..] else { panic!("a fetch: {transfers:?}") };
    assert!(link.starts_with(s3_config.url.split("/migrate-e2e").next().unwrap()) && link.contains("X-Amz-Signature"), "the bucket's link: {link}");
    let done = device::transfer(&http, transfers).await;
    again.session.transferred(done.id, done.put, done.result);
    assert!(again.session.take_events().contains(&Event::BlobGot(id, pic.clone())));
    // Writing to the deleted note: refused still.
    again.write(device::OTHER, "back from the dead?");
    again.settle(&mut ws).await;
    assert!(again.events.contains(&Event::RoomRemoved(device::OTHER.into())), "{:?}", again.events);

    // The password lets a new device in, with every epoch's key.
    let key = DeviceKey::generate(&mut OsRandom);
    let mut joiner = PasswordJoiner::new(origin.clone(), "ann@example.com", "correct horse battery", b"\x0c".to_vec(), key.clone());
    let (mut ws_c, _) = tokio_tungstenite::connect_async(format!("ws://{}/v1/sync?protocol={}", second.addr, carnet_sync::compat::PROTOCOL)).await.unwrap();
    for _ in 0..100 {
        if !matches!(joiner.state(), Joining::Going) {
            break;
        }
        for out in joiner.take_out() {
            ws_c.send(Message::Binary(out.into())).await.unwrap();
        }
        if let Ok(Some(Ok(Message::Binary(m)))) = tokio::time::timeout(Duration::from_millis(300), ws_c.next()).await {
            joiner.receive(&m, &mut OsRandom);
        }
    }
    let Joining::In(admitted) = joiner.into_state() else { panic!("not let in by the password") };
    let mut c = Device::new(Identity { origin: origin.clone(), account: admitted.account, device: b"\x0c".to_vec(), key, ring: admitted.ring, name: String::new() }, 4);
    c.session.connected();
    c.session.receive(&admitted.challenge, &mut c.notes, &mut OsRandom);
    c.settle(&mut ws_c).await;
    assert_eq!(c.text(device::ROOM).as_deref(), Some("# Shop\n\n- [ ] milk\n- [ ] eggs\n"), "signed in by password, the notes too");

    // A server has run on them now: `migrate` copies over neither.
    let e = migrate::run(&plan, &mut |_| {}).unwrap_err();
    assert!(e.0.contains("a server has run"), "{e}");

    drop(ws);
    drop(ws_c);
    second.stop().await;
    common::empty(&s3, &s3_config);
    let _ = std::fs::remove_dir_all(&data);
}

/// `run` off the async threads (its clients block): what it did, and said.
#[cfg(feature = "s3")]
async fn migrated(plan: &Plan) -> (Result<migrate::Done, carnet_server::storage::Failed>, Vec<String>) {
    let plan = plan.clone();
    tokio::task::spawn_blocking(move || {
        let mut said = Vec::new();
        (migrate::run(&plan, &mut |l| said.push(l.to_string())), said)
    })
    .await
    .unwrap()
}

/// The files go first, the server running on the folder, a file
/// half put meanwhile: no lock (the server carries on, and the put ends
/// well), only finished files copied, and no records. Then the stop: one
/// file swept since, and the run after copies the records and only
/// the file since, finds the others there already, takes the swept
/// one from the bucket, and its check passes. Again on the stopped
/// folder, the folder is as it was; on a bucket a server has run on,
/// refused.
#[cfg(feature = "s3")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_files_first_while_the_server_runs() {
    use carnet_server::server::{self, Host};
    use carnet_sync::crypto::{DeviceKey, OsRandom, Ring, VaultKey};
    use carnet_sync::session::{Event, Identity, Transfer};
    use device::{Device, Socket, enroll};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let Some((url, _schema)) = database("first") else { return };
    let Some((s3, s3_config)) = common::bucket("migrate-first") else { return };
    let data = temp("first");
    std::fs::create_dir_all(&data).unwrap();
    let code = server::new_invite(&mut Sharded::open(&data).unwrap()).unwrap();
    let running = device::start(&data, None, Host::default()).await;
    let origin = format!("http://{}", running.addr);
    let key = DeviceKey::generate(&mut OsRandom);
    let account = enroll(running.addr, &code.text(), b"\x0a", key.public()).await.unwrap();
    let identity = Identity { origin, account: account.clone(), device: b"\x0a".to_vec(), key, ring: Ring::new(VaultKey::generate(&mut OsRandom)), name: String::new() };
    let mut a = Device::new(identity, 1);
    let mut ws = a.connect(running.addr).await;
    a.settle(&mut ws).await;
    a.write(device::ROOM, "# Files\n");
    a.settle(&mut ws).await;

    // A file offered, and its put to do.
    async fn offer(a: &mut Device, ws: &mut Socket, file: &[u8]) -> ([u8; 32], String, Vec<u8>) {
        let id = a.session.blob_id(file);
        a.session.offer_blobs(vec![(id, file.len())]);
        a.settle(ws).await;
        assert!(a.session.put_blob(file, &mut OsRandom));
        let [Transfer::Put { url, sealed, .. }] = &a.session.take_transfers()[..] else { panic!("a put") };
        (id, url.clone(), sealed.clone())
    }
    let http = carnet_sync::http::Http::start(Vec::new(), || {}).unwrap();
    let mut ids = Vec::new();
    for n in 1..=3u8 {
        let (id, link, sealed) = offer(&mut a, &mut ws, &vec![n; 40_000 * usize::from(n)]).await;
        let done = device::transfer(&http, vec![Transfer::Put { id, url: link, sealed }]).await;
        a.session.transferred(done.id, done.put, done.result);
        assert!(a.session.take_events().contains(&Event::BlobStored(id)));
        ids.push(id);
    }
    // The third, marked to go by the device's word.
    a.session.drop_blobs(&[ids[2]]);
    a.settle(&mut ws).await;
    let files = BlobFiles { root: data.join("blobs"), spool: data.join("spool") };
    assert!(files.path(&account, &ids[2]).with_extension("drop").exists(), "marked");

    // A fourth, half put: its length said, half its bytes sent.
    let (id4, link, sealed) = offer(&mut a, &mut ws, &[4; 200_000]).await;
    let (host, path) = link.strip_prefix("http://").unwrap().split_once('/').unwrap();
    let mut put = tokio::net::TcpStream::connect(host).await.unwrap();
    put.write_all(format!("PUT /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", sealed.len()).as_bytes()).await.unwrap();
    let half = sealed.len() / 2;
    put.write_all(&sealed[..half]).await.unwrap();
    put.flush().await.unwrap();
    let mut coming = false;
    for _ in 0..100 {
        coming = std::fs::read_dir(&files.spool).unwrap().flatten().any(|e| e.metadata().is_ok_and(|m| m.len() > 0));
        if coming {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(coming, "half of it in the spool");

    // The files first, beside the running server.
    let first = Plan { from: data.clone(), postgres: Some(url.clone()), s3: Some(s3_config.clone()), files_first: true, ..Plan::default() };
    let (done, said) = migrated(&first).await;
    let done = done.unwrap_or_else(|e| panic!("{e}: {said:#?}"));
    assert_eq!((done.files, done.put, done.files_skipped), (3, 3, 0), "{said:#?}");
    let bytes: u64 = ids.iter().map(|id| std::fs::metadata(files.path(&account, id)).unwrap().len()).sum();
    assert_eq!((done.file_bytes, done.put_bytes), (bytes, bytes));
    assert!(said.last().unwrap().contains("3 put now") && said.last().unwrap().contains("stop the server"), "{said:#?}");
    for id in &ids {
        assert_eq!(s3.get(&s3.key(&account, id)).unwrap(), Some(std::fs::read(files.path(&account, id)).unwrap()), "whole, as the folder has it");
    }
    assert!(s3.head(&s3.key(&account, &id4)).unwrap().is_none(), "one still coming in isn't copied");
    assert!(s3.marks().unwrap().is_empty(), "marks wait for the stopped run");
    let pg_url = url.clone();
    let records = tokio::task::spawn_blocking(move || Postgres::target(&pg_url).unwrap().account_ids().unwrap()).await.unwrap();
    assert!(records.is_empty(), "no records: {records:?}");

    // The server carried on: the put ends well, and is kept.
    put.write_all(&sealed[half..]).await.unwrap();
    let mut reply = String::new();
    put.read_to_string(&mut reply).await.unwrap();
    assert!(reply.starts_with("HTTP/1.1 204"), "{reply}");
    a.session.transferred(id4, true, Ok(Vec::new()));
    assert_eq!(std::fs::read(files.path(&account, &id4)).unwrap(), sealed);
    drop(ws);
    running.stop().await;

    // Swept since (as the server's sweep does, 30 days on).
    files.mark(&account, &ids[1], 1).unwrap();
    assert_eq!(files.sweep(2), 1);

    // Stopped: the run copies the records, the file since and the
    // mark; the two still in the folder are there already; the swept one
    // goes from the bucket.
    let plan = Plan { files_first: false, ..first.clone() };
    let (done, said) = migrated(&plan).await;
    let done = done.unwrap_or_else(|e| panic!("{e}: {said:#?}"));
    assert_eq!((done.rows.accounts, done.files, done.put, done.files_skipped, done.marks), (1, 3, 1, 2, 1), "{said:#?}");
    assert!(s3.head(&s3.key(&account, &ids[1])).unwrap().is_none(), "swept: gone from the bucket too");
    assert!(s3.marked(&account, &ids[2]).unwrap().is_some());
    let (checked, said) = migrated(&Plan { verify_only: true, ..plan.clone() }).await;
    assert!(checked.is_ok(), "{said:#?}");

    // Again first, on the stopped folder: nothing to put, nothing written.
    let before = snapshot(&data);
    let (again, said) = migrated(&first).await;
    let again = again.unwrap_or_else(|e| panic!("{e}: {said:#?}"));
    assert_eq!((again.files, again.put, again.files_skipped), (3, 0, 3));
    assert_eq!(snapshot(&data), before, "the folder as it was");

    // Without a bucket, or only to check: refused. A bucket a server has
    // run on: refused.
    let (e, _) = migrated(&Plan { s3: None, ..first.clone() }).await;
    assert!(e.unwrap_err().0.contains("give a bucket"));
    let (e, _) = migrated(&Plan { verify_only: true, ..first.clone() }).await;
    assert!(e.is_err());
    s3.serving().unwrap();
    let (e, _) = migrated(&Plan { postgres: None, ..first.clone() }).await;
    assert!(e.unwrap_err().0.contains("a server has run"));
    common::empty(&s3, &s3_config);
    let _ = std::fs::remove_dir_all(&data);
}

/// A device over a real socket, as `tests/server.rs` drives one.
#[cfg(feature = "s3")]
mod device {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::time::Duration;

    use carnet_server::server::{self, Config, Host};
    use carnet_sync::crypto::OsRandom;
    use carnet_sync::session::{Docs, Event, Identity, Session, Transfer};
    use carnet_sync::{NoteDoc, Origin};
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::oneshot;
    use tokio_tungstenite::tungstenite::Message;

    pub const ROOM: &str = "00000000000000000000000000000abc";
    pub const OTHER: &str = "00000000000000000000000000000def";

    pub struct Running {
        pub addr: std::net::SocketAddr,
        stop: oneshot::Sender<()>,
        done: tokio::task::JoinHandle<std::io::Result<()>>,
    }

    pub async fn start(data: &Path, origin: Option<&str>, host: Host) -> Running {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, rx) = oneshot::channel::<()>();
        let config = Config { data: data.to_path_buf(), origin: origin.map_or_else(|| format!("http://{addr}"), str::to_string), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new(), client_header: Default::default(), tls: None };
        let done = tokio::spawn(server::serve_hosted(config, listener, None, host, async {
            let _ = rx.await;
        }));
        Running { addr, stop, done }
    }

    impl Running {
        pub async fn stop(self) {
            let _ = self.stop.send(());
            tokio::time::timeout(Duration::from_secs(10), self.done).await.expect("stops").unwrap().unwrap();
        }
    }

    pub async fn enroll(addr: std::net::SocketAddr, invite: &str, device: &[u8], public: [u8; 32]) -> Result<Vec<u8>, String> {
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

    /// One transfer, done by the desktop's own HTTP.
    pub async fn transfer(http: &carnet_sync::http::Http, transfers: Vec<Transfer>) -> carnet_sync::http::Done {
        for t in transfers {
            http.send(t);
        }
        for _ in 0..200 {
            if let Some(d) = http.poll().pop() {
                return d;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no transfer done");
    }

    pub struct Notes {
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

    pub struct Device {
        pub session: Session,
        pub notes: Notes,
        pub events: Vec<Event>,
    }

    pub type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

    impl Device {
        pub fn new(identity: Identity, peer: u64) -> Self {
            Device { session: Session::new(identity, 0), notes: Notes { peer, docs: BTreeMap::new() }, events: Vec::new() }
        }

        pub async fn connect(&mut self, addr: std::net::SocketAddr) -> Socket {
            let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/sync?protocol={}", carnet_sync::compat::PROTOCOL)).await.unwrap();
            self.session.disconnected();
            self.session.connected();
            ws
        }

        /// Frames both ways until the session is idle and the socket quiet.
        pub async fn settle(&mut self, ws: &mut Socket) {
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

        pub fn write(&mut self, room: &str, text: &str) {
            self.notes.doc(room).record(text, Origin::Save);
            self.session.saved(room, &mut self.notes, &mut OsRandom);
        }

        pub fn text(&self, room: &str) -> Option<String> {
            self.notes.docs.get(room).map(NoteDoc::text)
        }
    }
}

/// An account copied in whose recovery key or passkey another account
/// there has already is refused, as a device setting it is: never taken
/// from that account, and nothing of the account copied changed.
#[test]
fn migrate_import_never_takes_a_key_from_another_account() {
    let Some((url, _schema)) = database("conflict") else { return };
    let data = temp("conflict");
    fill(&data);
    migrate::run(&plan(&data, &url), &mut |_| {}).unwrap();
    let pg = Postgres::target(&url).unwrap();
    let alice = pg.rows(b"alice").unwrap().unwrap();
    let before = pg.rows(b"bob").unwrap().unwrap();
    // Bob's rows, with alice's recovery key: refused, alice's kept.
    let mut taking = before.clone();
    taking.recovery = alice.recovery.clone();
    let e = pg.put_rows(&taking).unwrap_err();
    assert!(e.0.contains("recovery") && e.0.contains(&"alice".bytes().map(|b| format!("{b:02x}")).collect::<String>()), "{e}");
    assert_eq!(pg.recovery(&[5; 32]).map(|(a, _)| a), Some(b"alice".to_vec()), "alice's still");
    assert_eq!(pg.rows(b"bob").unwrap().unwrap(), before, "bob's as they were");
    // With one of alice's passkeys: the same.
    let mut taking = before.clone();
    taking.passkeys = alice.passkeys[..1].to_vec();
    assert!(pg.put_rows(&taking).unwrap_err().0.contains("passkeys"));
    assert_eq!(pg.passkeys(b"alice").len(), 2, "alice's still");
    assert_eq!(pg.rows(b"bob").unwrap().unwrap(), before);
    let _ = std::fs::remove_dir_all(&data);
}
