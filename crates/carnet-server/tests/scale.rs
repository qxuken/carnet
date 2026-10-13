//! What a write costs the hub as a room grows (docs/SYNC.md §6): the
//! same at its ten-thousandth record as at its first. What it's judged
//! by, a room's live records and bytes and whether it has a base, and how
//! many rooms hold anything, is kept counted by each store, and the count
//! stays what the records say through everything done to them.
//!
//! The timing is a measurement, not a check, so it's ignored unless asked
//! for, on SQLite on disk (Postgres too, given `CARNET_TEST_POSTGRES`):
//!
//! ```text
//! CARNET_SCALE=10000 cargo test --release -p carnet-server --test scale -- --ignored --nocapture
//! ```

mod common;

use std::time::Instant;

use carnet_server::storage::Live;
use carnet_server::{ConnId, Memory, Sharded, Storage};
use carnet_sync::crypto::{Random, VaultKey};
use carnet_sync::elo::{self, Header};
use carnet_sync::wire::{self, BatchId, CrdtType, Frame};
use common::*;
use loro_protocol::ProtocolMessage;

const NOW: u64 = 1_760_000_000;

/// A delta of `peer`'s operation `i`, as a device sends it: one record
/// of about a save's size, in an update of its own.
fn delta(vault: &VaultKey, rng: &mut Seeded, room: &str, peer: u64, i: u64) -> Vec<u8> {
    let r = elo::seal(vault, room, &Header::Delta { peer, start: i, end: i + 1, epoch: 0 }, &[b'x'; 200], rng);
    let mut id = [0; 8];
    id.copy_from_slice(&i.to_le_bytes());
    wire::encode(&Frame::Room(ProtocolMessage::DocUpdate {
        crdt: CrdtType::Elo,
        room_id: room.into(),
        updates: vec![elo::container(&[r])],
        batch_id: BatchId(id),
    }))
}

/// A hub on `store` with a device in `ROOM`, no room compacted and none
/// refused (another device online, writing too, holds compaction off).
fn joined(store: Box<dyn Storage>) -> (Net, VaultKey, ConnId) {
    let mut net = Net::new(7, store);
    net.hub.tick(NOW);
    net.hub.compact_when(1 << 40, 1 << 50);
    let vault = VaultKey::generate(&mut net.rng);
    let a = net.first_device(&vault);
    net.write(a, "joined\n");
    (net, vault, a)
}

#[test]
#[ignore = "a measurement: CARNET_SCALE=10000 … -- --ignored --nocapture"]
fn a_write_costs_the_same_however_many_records_its_room_has() {
    let n: u64 = std::env::var("CARNET_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let window = (n / 20).clamp(1, 500);
    let dir = std::env::temp_dir().join(format!("carnet-scale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut stores: Vec<(&str, Box<dyn Storage>)> = vec![("sqlite", Box::new(Sharded::open(&dir).unwrap()))];
    stores.extend(postgres().map(|p| ("postgres", p)));
    for (name, store) in stores {
        let (mut net, vault, a) = joined(store);
        let mut rng = Seeded(9);
        let frames: Vec<Vec<u8>> = (0..n).map(|i| delta(&vault, &mut rng, ROOM, 77, i)).collect();
        let started = Instant::now();
        let mut per = Vec::with_capacity(n as usize);
        for f in &frames {
            let t = Instant::now();
            let outs = net.hub.receive(a, f, &mut net.rng);
            per.push(t.elapsed());
            drop(outs);
        }
        let total = started.elapsed();
        let avg = |s: &[std::time::Duration]| s.iter().sum::<std::time::Duration>().as_secs_f64() * 1e6 / s.len() as f64;
        let w = window as usize;
        println!("{name}: {n} deltas to one room in {:.2} s", total.as_secs_f64());
        println!("  first {w}: {:.0} µs each", avg(&per[..w]));
        let mut points = vec![1_000, 10_000, n];
        points.retain(|&at| at <= n && at >= window);
        points.dedup();
        for at in points {
            let at = at as usize;
            println!("  updates {}..{}: {:.0} µs each", at - w, at, avg(&per[at - w..at]));
        }
        assert_eq!(net.hub.store().live(ACCOUNT, ROOM).records, n + 1);
    }
    if let Some(path) = Sharded::account_path(&dir, ACCOUNT) {
        explain(&path);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// How SQLite answers what a write asks of an account's file.
fn explain(path: &std::path::Path) {
    let db = rusqlite::Connection::open(path).unwrap();
    let room = format!("'{ROOM}'");
    for sql in [
        format!("SELECT vv FROM bases WHERE room = {room} AND shelved_by = 0"),
        format!("SELECT EXISTS (SELECT 1 FROM records WHERE room = {room} AND peer = 77 AND shelved_by = 0 AND span_end >= 6 AND span_start <= 5)"),
        format!("DELETE FROM records WHERE room = {room} AND peer = 77 AND shelved_by = 0 AND span_end >= 5 AND span_end <= 6 AND span_start >= 5"),
        format!("SELECT records, bytes, bases > 0 FROM rooms WHERE room = {room}"),
        "SELECT rooms FROM meta".into(),
    ] {
        let Ok(mut st) = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")) else {
            continue;
        };
        let rows: Vec<String> = st.query_map([], |r| r.get::<_, String>(3)).unwrap().flatten().collect();
        println!("{}\n    {}", sql.split_whitespace().collect::<Vec<_>>().join(" "), rows.join("\n    "));
    }
}

/// What `room` holds, as its records and base say, counted here.
fn counted(store: &dyn Storage, account: &[u8], room: &str) -> Live {
    let spans = store.spans(account, room);
    Live { records: spans.len() as u64, bytes: spans.iter().map(|s| s.record.len() as u64).sum(), based: store.base(account, room).is_some() }
}

fn check(store: &dyn Storage, account: &[u8], rooms: &[String], step: &str) {
    for room in rooms {
        assert_eq!(store.live(account, room), counted(store, account, room), "{room}, after {step}");
    }
    assert_eq!(store.room_count(account), store.rooms(account).len() as u64, "rooms, after {step}");
}

/// A store as a test has it: made, and (on disk) opened again, as after
/// a restart; given, or not, as a file of the version before the count
/// was kept.
struct Kind {
    name: &'static str,
    make: Box<dyn Fn() -> Box<dyn Storage>>,
    reopen: Option<Reopen>,
}

/// The store opened again, from a file of the version before if asked.
type Reopen = Box<dyn Fn(bool) -> Box<dyn Storage>>;

fn kinds(dir: &std::path::Path) -> Vec<Kind> {
    let disk = dir.to_path_buf();
    let again = dir.to_path_buf();
    let mut all = vec![
        Kind { name: "memory", make: Box::new(|| Box::new(Memory::default())), reopen: None },
        Kind { name: "sqlite in memory", make: Box::new(|| Box::new(Sharded::memory().unwrap())), reopen: None },
        Kind {
            name: "sqlite",
            make: Box::new(move || Box::new(Sharded::open(&disk).unwrap())),
            reopen: Some(Box::new(move |older| {
                if older {
                    to_schema_8(&again);
                }
                Box::new(Sharded::open(&again).unwrap())
            })),
        },
    ];
    if postgres().is_some() {
        all.push(Kind { name: "postgres", make: Box::new(|| postgres().unwrap()), reopen: None });
    }
    all
}

/// Every account file in `data` as schema 8 had it: no count kept.
fn to_schema_8(data: &std::path::Path) {
    for e in std::fs::read_dir(data.join("accounts")).unwrap().flatten() {
        if e.path().extension().is_none_or(|x| x != "db") {
            continue;
        }
        let db = rusqlite::Connection::open(e.path()).unwrap();
        db.execute_batch(
            "DROP TRIGGER records_in; DROP TRIGGER records_out; DROP TRIGGER records_back; DROP TRIGGER records_aside;
             DROP TRIGGER bases_in; DROP TRIGGER bases_out; DROP TRIGGER bases_back; DROP TRIGGER bases_aside;
             DROP TABLE rooms; DROP INDEX records_reach; ALTER TABLE meta DROP COLUMN rooms; PRAGMA user_version = 8;",
        )
        .unwrap();
    }
}

/// Records come, cover each other and go; bases set them aside and are
/// rolled back, purged, replaced; rooms are removed; the account goes and
/// comes back; the server restarts, once on files from before the count
/// was kept. After each, the count is what the records say, on every
/// store.
#[test]
fn the_count_a_write_is_judged_by_stays_exact() {
    let dir = std::env::temp_dir().join(format!("carnet-counts-{}", std::process::id()));
    for kind in kinds(&dir) {
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = (kind.make)();
        let a: &[u8] = b"acct";
        store.add_account(a, true).unwrap();
        store.add_device(a, b"dev", [1; 32]).unwrap();
        let rooms: Vec<String> = (0..4).map(|r| format!("{r:032x}")).chain(["vault".to_string()]).collect();
        let mut rng = Seeded(5);
        let mut pick = |n: u64| u64::from_le_bytes(rng.bytes()) % n;
        let mut now = 1_000;
        for step in 0..1500 {
            now += 1;
            let room = rooms[pick(rooms.len() as u64) as usize].clone();
            let what = match if step == 500 { 96 } else { pick(100) } {
                0..70 => {
                    let peer = 1 + pick(3);
                    let start = pick(40);
                    let end = start + 1 + pick(6);
                    store.put(a, &room, peer, start, end, vec![b'r'; 1 + pick(90) as usize]).unwrap();
                    "a record"
                }
                70..80 => {
                    let vv: Vec<(u64, u64)> = (1..=3).map(|p| (p, pick(45))).collect();
                    store.set_base(a, &room, vv, vec![b'b'; 30], b"dev", now).unwrap();
                    "a base"
                }
                80..88 => {
                    store.roll_back(a, &room).unwrap();
                    "a roll back"
                }
                88..92 => {
                    store.purge(now - pick(200)).unwrap();
                    "a purge"
                }
                92..94 => {
                    if let Some(b) = store.base(a, &room) {
                        store.verify_base(a, &room, b.seq).unwrap();
                    }
                    "a check"
                }
                94..96 => {
                    // (The hub takes nothing more of a room removed; a store
                    // takes what it's given, counted from nothing.)
                    store.remove_room(a, &room, now).unwrap();
                    "a removal"
                }
                96..98 => {
                    if let Some(reopen) = &kind.reopen {
                        drop(store);
                        store = reopen(step == 500);
                    }
                    "a restart"
                }
                _ => {
                    if step % 3 == 0 {
                        store.remove_account(a).unwrap();
                        assert_eq!(store.room_count(a), 0);
                        store.add_account(a, true).unwrap();
                        store.add_device(a, b"dev", [1; 32]).unwrap();
                    }
                    "the account removed"
                }
            };
            check(&*store, a, &rooms, &format!("{what}, step {step}, {}", kind.name));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A Postgres database from before the count was kept: counted once as
/// it's opened, then kept on.
#[cfg(feature = "postgres")]
#[test]
fn a_postgres_database_from_before_the_count_is_counted_when_opened() {
    use carnet_server::postgres::Postgres;
    let Some((url, _schema)) = database("count") else {
        return;
    };
    let a: &[u8] = b"acct";
    let rooms: Vec<String> = (0..3).map(|r| format!("{r:032x}")).collect();
    {
        let mut pg = Postgres::open(&url).unwrap();
        pg.add_account(a, true).unwrap();
        for (i, room) in rooms.iter().enumerate() {
            for n in 0..(5 + i as u64 * 3) {
                pg.put(a, room, 1, n, n + 1, vec![b'r'; 10 + n as usize]).unwrap();
            }
        }
        pg.set_base(a, &rooms[1], vec![(1, 4)], b"S".to_vec(), b"dev", 10).unwrap();
        pg.set_base(a, &rooms[2], vec![(1, 100)], b"S".to_vec(), b"dev", 10).unwrap();
    }
    sql(
        &url,
        "DROP TRIGGER rooms_kept ON rooms; DROP FUNCTION carnet_count_rooms;
         DROP TRIGGER records_in ON records; DROP TRIGGER records_out ON records; DROP TRIGGER records_moved ON records;
         DROP TRIGGER bases_in ON bases; DROP TRIGGER bases_out ON bases; DROP TRIGGER bases_moved ON bases;
         DROP FUNCTION carnet_count_live; DROP TABLE rooms; DROP INDEX records_reach; ALTER TABLE accounts DROP COLUMN rooms;
         UPDATE carnet_schema SET version = 2;",
    )
    .unwrap();
    let mut pg = Postgres::open(&url).unwrap();
    check(&pg, a, &rooms, "opened");
    assert_eq!(pg.room_count(a), 3);
    assert_eq!(pg.live(a, &rooms[1]), Live { records: 4, bytes: 4 * 10 + 4 + 5 + 6 + 7, based: true });
    pg.put(a, &rooms[0], 2, 0, 1, b"more".to_vec()).unwrap();
    pg.roll_back(a, &rooms[2]).unwrap();
    check(&pg, a, &rooms, "written after");
    pg.remove_account(a).unwrap();
    assert_eq!(pg.room_count(a), 0);
}
