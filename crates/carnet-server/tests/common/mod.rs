//! A small world for the hub's tests: devices, joiners and recoverers on
//! sockets to one hub, messages delivered in order (shuffling them is the
//! simulation's job), on memory or SQLite alike.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, VecDeque};

use carnet_server::blobs::{MemoryBlobs, Worker};
use carnet_server::{ConnId, Hub, Memory, Out, Sharded, Storage};
use carnet_sync::crypto::{DeviceKey, Random, Ring, VaultKey};
use carnet_sync::pair::{Enroller, Joiner, Joining, PairCode, PasswordJoiner, Recoverer};
use carnet_sync::session::{DeviceInfo, Docs, Event, Identity, Session, Transfer};
use carnet_sync::wire::{self, Carnet, Frame};
use carnet_sync::{NoteDoc, Origin};

pub const ORIGIN: &str = "https://pair.example";
pub const ACCOUNT: &[u8] = b"account-p";
pub const ROOM: &str = "0000000000000000000000000000beef";

pub struct Seeded(pub u64);

impl Random for Seeded {
    fn fill(&mut self, out: &mut [u8]) {
        for b in out {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            *b = (z ^ (z >> 31)) as u8;
        }
    }
}

pub struct Notes {
    pub peer: u64,
    pub docs: BTreeMap<String, NoteDoc>,
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

#[allow(clippy::large_enum_variant)]
pub enum Peer {
    Device(Device),
    Joiner(Joiner),
    Recoverer(Recoverer),
    Password(PasswordJoiner),
    Enroller(Enroller),
}

/// Frames from the new device that the server alters before passing on:
/// the server in the middle, meddling.
pub type Meddle = Box<dyn FnMut(&mut Vec<u8>)>;

pub struct Net {
    pub rng: Seeded,
    pub hub: Hub<Box<dyn Storage>>,
    pub peers: HashMap<ConnId, Peer>,
    pub down: VecDeque<(ConnId, Vec<u8>)>,
    pub next: ConnId,
    pub meddle: Option<(ConnId, Meddle)>,
    pub denied: Vec<(ConnId, String)>,
    /// The picture worker, as the server runs one beside the hub.
    pub worker: Worker<MemoryBlobs>,
}

impl Net {
    pub fn new(seed: u64, store: Box<dyn Storage>) -> Self {
        let mut hub = Hub::new(ORIGIN, store);
        hub.store_mut().add_account(ACCOUNT, true).unwrap();
        Net { rng: Seeded(seed), hub, peers: HashMap::new(), down: VecDeque::new(), next: 1, meddle: None, denied: Vec::new(), worker: worker() }
    }

    pub fn connect(&mut self, peer: Peer) -> ConnId {
        let conn = self.next;
        self.next += 1;
        self.peers.insert(conn, peer);
        let outs = self.hub.connect(conn, &mut self.rng);
        self.route(outs);
        conn
    }

    pub fn disconnect(&mut self, conn: ConnId) {
        self.peers.remove(&conn);
        let outs = self.hub.disconnect(conn);
        self.route(outs);
    }

    pub fn route(&mut self, outs: Vec<Out>) {
        for o in outs {
            match o {
                Out::Send(c, b) => self.down.push_back((c, b)),
                Out::Blob(job) => {
                    let down = &mut self.down;
                    self.worker.run(job, &mut |o| {
                        if let Out::Send(c, b) = o {
                            down.push_back((c, b));
                        }
                    });
                }
                Out::Close(_) => {}
            }
        }
    }

    /// Everything delivered, both ways, in order; pictures' HTTP done as
    /// the server's links would have it.
    pub fn pump(&mut self) {
        for _ in 0..10_000 {
            let mut moved = false;
            for p in self.peers.values_mut() {
                let Peer::Device(d) = p else { continue };
                for t in d.session.take_transfers() {
                    moved = true;
                    let put = matches!(t, Transfer::Put { .. });
                    let result = self.worker.blobs.serve(&t);
                    d.session.transferred(t.id(), put, result);
                    d.events.extend(d.session.take_events());
                }
            }
            let mut up: Vec<(ConnId, Vec<u8>)> = Vec::new();
            for (&c, p) in self.peers.iter_mut() {
                let out = match p {
                    Peer::Device(d) => d.session.take_out(),
                    Peer::Joiner(j) => j.take_out(),
                    Peer::Recoverer(r) => r.take_out(),
                    Peer::Password(p) => p.take_out(),
                    Peer::Enroller(e) => e.take_out(),
                };
                up.extend(out.into_iter().map(|b| (c, b)));
            }
            up.sort_by_key(|(c, _)| *c);
            if up.is_empty() && self.down.is_empty() && !moved {
                return;
            }
            for (c, mut b) in up {
                if let Some((m, f)) = self.meddle.as_mut()
                    && *m == c
                {
                    f(&mut b);
                }
                let outs = self.hub.receive(c, &b, &mut self.rng);
                self.route(outs);
            }
            while let Some((c, b)) = self.down.pop_front() {
                if let Ok(Frame::Carnet(Carnet::Denied { reason, .. })) = wire::decode(&b) {
                    self.denied.push((c, reason));
                }
                let Net { peers, rng, .. } = self;
                match peers.get_mut(&c) {
                    Some(Peer::Device(d)) => {
                        d.session.receive(&b, &mut d.notes, rng);
                        d.events.extend(d.session.take_events());
                    }
                    Some(Peer::Joiner(j)) => j.receive(&b, rng),
                    Some(Peer::Recoverer(r)) => r.receive(&b),
                    Some(Peer::Password(p)) => p.receive(&b, rng),
                    Some(Peer::Enroller(e)) => e.receive(&b),
                    None => {}
                }
            }
        }
        panic!("did not settle");
    }

    pub fn device(&mut self, conn: ConnId) -> &mut Device {
        match self.peers.get_mut(&conn) {
            Some(Peer::Device(d)) => d,
            _ => panic!("not a device"),
        }
    }

    /// A device enrolled in the account, logged in.
    pub fn first_device(&mut self, vault: &VaultKey) -> ConnId {
        let key = DeviceKey::generate(&mut self.rng);
        self.hub.store_mut().add_device(ACCOUNT, b"first", key.public()).unwrap();
        let id = Identity { origin: ORIGIN.into(), account: ACCOUNT.to_vec(), device: b"first".to_vec(), key, ring: Ring::new(vault.clone()), name: "first".into() };
        let mut session = Session::new(id, 0);
        session.connected();
        let conn = self.connect(Peer::Device(Device { session, notes: Notes { peer: 1, docs: BTreeMap::new() }, events: Vec::new() }));
        self.pump();
        assert!(self.device(conn).session.is_ready());
        conn
    }

    /// A joining or recovering device, let in: it becomes a device on the
    /// same socket, answering the challenge it was handed.
    pub fn admit(&mut self, conn: ConnId, device: &[u8], key: DeviceKey, peer: u64) {
        let state = match self.peers.remove(&conn) {
            Some(Peer::Joiner(j)) => j.state,
            Some(Peer::Recoverer(r)) => r.state,
            Some(Peer::Password(p)) => p.into_state(),
            Some(Peer::Enroller(e)) => e.state,
            _ => panic!("not joining"),
        };
        let Joining::In(a) = state else { panic!("not let in: {state:?}") };
        let name = String::from_utf8_lossy(device).into_owned();
        let id = Identity { origin: ORIGIN.into(), account: a.account, device: device.to_vec(), key, ring: a.ring, name };
        let mut d = Device { session: Session::new(id, 0), notes: Notes { peer, docs: BTreeMap::new() }, events: Vec::new() };
        d.session.connected();
        d.session.receive(&a.challenge, &mut d.notes, &mut self.rng);
        self.peers.insert(conn, Peer::Device(d));
        self.pump();
    }

    pub fn write(&mut self, conn: ConnId, text: &str) {
        let Net { peers, rng, .. } = self;
        let Some(Peer::Device(d)) = peers.get_mut(&conn) else { panic!() };
        d.notes.doc(ROOM).record(text, Origin::Save);
        d.session.saved(ROOM, &mut d.notes, rng);
        self.pump();
    }

    /// A new device paired in by `from`, logged in.
    pub fn pair(&mut self, from: ConnId, device: &[u8], peer: u64) -> ConnId {
        let code = self.code(from);
        let key = DeviceKey::generate(&mut self.rng);
        let n = self.connect(Peer::Joiner(Joiner::new(code, device.to_vec(), key.clone())));
        self.pump();
        self.admit(n, device, key, peer);
        assert!(self.device(n).session.is_ready());
        n
    }

    /// The account's devices, as `conn` reads them.
    pub fn list(&mut self, conn: ConnId) -> Vec<DeviceInfo> {
        self.device(conn).session.list_devices();
        self.pump();
        self.device(conn).events.iter().rev().find_map(|e| match e {
            Event::Devices(list) => Some(list.clone()),
            _ => None,
        })
        .expect("a list")
    }

    pub fn revoke(&mut self, conn: ConnId, device: &[u8]) -> Result<(), String> {
        let Net { peers, rng, .. } = self;
        let Some(Peer::Device(d)) = peers.get_mut(&conn) else { panic!() };
        let r = d.session.revoke(device, rng);
        self.pump();
        r
    }

    /// A device off the network: its socket gone, the device kept.
    pub fn take(&mut self, conn: ConnId) -> Device {
        let Some(Peer::Device(mut d)) = self.peers.remove(&conn) else { panic!("not a device") };
        let outs = self.hub.disconnect(conn);
        self.route(outs);
        d.session.disconnected();
        d
    }

    /// The device back, on a new socket: it logs in again.
    pub fn back(&mut self, mut d: Device) -> ConnId {
        d.session.connected();
        let conn = self.connect(Peer::Device(d));
        self.pump();
        conn
    }

    pub fn code(&mut self, conn: ConnId) -> PairCode {
        let Net { peers, rng, .. } = self;
        let Some(Peer::Device(d)) = peers.get_mut(&conn) else { panic!() };
        d.session.pair_open(rng);
        self.pump();
        let code = self.device(conn).events.iter().rev().find_map(|e| match e {
            Event::PairCode(c) => Some(c.clone()),
            _ => None,
        });
        PairCode::parse(&code.expect("a code shown")).unwrap()
    }
}

/// Every store a test runs on: memory, SQLite, and Postgres when the
/// `postgres` feature is on and `CARNET_TEST_POSTGRES` names a database
/// (`postgres://user:password@127.0.0.1/db`), each test in a schema of its
/// own, dropped after.
pub fn stores() -> Vec<Box<dyn Storage>> {
    let mut all: Vec<Box<dyn Storage>> = vec![Box::new(Memory::default()), Box::new(Sharded::memory().unwrap())];
    all.extend(postgres());
    all
}

/// Postgres, if the tests are given one.
pub fn postgres() -> Option<Box<dyn Storage>> {
    #[cfg(feature = "postgres")]
    if let Some(url) = std::env::var("CARNET_TEST_POSTGRES").ok().filter(|u| !u.is_empty()) {
        return Some(Box::new(carnet_server::postgres::Postgres::scratch(&url).expect("CARNET_TEST_POSTGRES: a database to test on")));
    }
    None
}


/// A database of its own for a test: a schema in `CARNET_TEST_POSTGRES`'s,
/// named in the URL it gives (for what opens it by URL: `migrate`, a
/// server), dropped when the guard goes. `None`, and why said, without one.
#[cfg(feature = "postgres")]
pub fn database(name: &str) -> Option<(String, Schema)> {
    let Some(base) = std::env::var("CARNET_TEST_POSTGRES").ok().filter(|u| !u.is_empty()) else {
        eprintln!("{name}: no CARNET_TEST_POSTGRES, not run");
        return None;
    };
    let schema = format!("carnet_{name}_{}", std::process::id());
    sql(&base, &format!("DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}")).expect("CARNET_TEST_POSTGRES");
    let join = if base.contains('?') { '&' } else { '?' };
    Some((format!("{base}{join}options=-c%20search_path%3D{schema}"), Schema { base, schema }))
}

/// A test's schema, dropped with this.
#[cfg(feature = "postgres")]
pub struct Schema {
    base: String,
    schema: String,
}

#[cfg(feature = "postgres")]
impl Drop for Schema {
    fn drop(&mut self) {
        let _ = sql(&self.base, &format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema));
    }
}

/// Statements run on the database at `url`, on a thread of their own: the
/// client can't block an async test's.
#[cfg(feature = "postgres")]
pub fn sql(url: &str, statements: &str) -> Result<(), postgres::Error> {
    std::thread::scope(|s| {
        s.spawn(|| postgres::Client::connect(url, postgres::NoTls)?.batch_execute(statements)).join().unwrap()
    })
}

/// A folder of its own in the test bucket: `CARNET_TEST_S3` names it
/// (`http://127.0.0.1:9000/carnet-test`, made if it isn't there), its keys
/// in `CARNET_S3_ACCESS_KEY` and `CARNET_S3_SECRET_KEY`. Emptied first;
/// `None`, and why said, without one.
#[cfg(feature = "s3")]
pub fn bucket(name: &str) -> Option<(carnet_server::s3::S3Blobs, carnet_server::s3::S3Config)> {
    use carnet_server::s3::{S3Blobs, S3Config};
    let Some(url) = std::env::var("CARNET_TEST_S3").ok().filter(|u| !u.is_empty()) else {
        eprintln!("{name}: no CARNET_TEST_S3, not run");
        return None;
    };
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("CARNET_TEST_S3 needs {k}"));
    let base = S3Config { url: url.clone(), region: "us-east-1".into(), key: env("CARNET_S3_ACCESS_KEY"), secret: env("CARNET_S3_SECRET_KEY"), virtual_host: false };
    S3Blobs::new(&base).unwrap().make_bucket().unwrap();
    let config = S3Config { url: format!("{}/{name}-{}", url.trim_end_matches('/'), std::process::id()), ..base };
    let s3 = S3Blobs::new(&config).unwrap();
    empty(&s3, &config);
    Some((s3, config))
}

/// Everything in a test's folder of the bucket gone.
#[cfg(feature = "s3")]
pub fn empty(s3: &carnet_server::s3::S3Blobs, config: &carnet_server::s3::S3Config) {
    let folder = config.url.rsplit('/').next().unwrap().to_string() + "/";
    let keys: Vec<String> = s3.list(&folder).unwrap().into_iter().map(|o| o.key).collect();
    s3.delete_all(&keys).unwrap();
}

/// A picture worker on memory, as the server runs one beside the hub.
pub fn worker() -> Worker<MemoryBlobs> {
    Worker::memory()
}
