//! A deterministic simulation of sync (docs/SYNC.md §12): devices of one
//! account editing notes, the hub, and a network between them that
//! delivers one message at a time, in a random order across sockets (a
//! socket keeps its own order, as TCP does), and drops sockets at random,
//! with whatever was in flight on them. Everything random comes from one
//! seed: a failure names its seed, and that seed plays it again exactly.
//!
//! Now and then one device takes the last one out of the account (§7),
//! wherever the network is at the time. Rooms are compacted (§6) at a low
//! limit, so snapshots are written, checked and stood on all through; the
//! clock moves ten seconds a step.
//!
//! When the dust settles — every device back, every message delivered —
//! what must hold:
//! - every device has every note, and the same text in each;
//! - every word anyone typed is in it (no one deletes here);
//! - no word is anywhere in what the server stored: it held ciphertext;
//! - a device taken out holds no word typed after the server took it out,
//!   and opens nothing sealed under a key made since.

use std::collections::{BTreeMap, HashMap, VecDeque};

use carnet_server::blobs::{MemoryBlobs, Worker};
use carnet_server::{ConnId, Hub, Memory, Out, Sharded, Storage};
use carnet_sync::crypto::{Card, DeviceKey, Random, Ring, VaultKey};
use carnet_sync::elo::{self, Header};
use carnet_sync::session::{Docs, Event, Identity, Session};
use carnet_sync::{NoteDoc, Origin};

/// Postgres, when the tests are given one (`CARNET_TEST_POSTGRES`, and
/// the `postgres` feature): a schema of its own, dropped after.
fn postgres() -> Option<Store> {
    #[cfg(feature = "postgres")]
    if let Some(url) = std::env::var("CARNET_TEST_POSTGRES").ok().filter(|u| !u.is_empty()) {
        return Some(Box::new(carnet_server::postgres::Postgres::scratch(&url).unwrap()));
    }
    None
}

const ORIGIN: &str = "https://sim.example";
const START: u64 = 1_760_000_000;
const ACCOUNT: &[u8] = b"account-1";

/// splitmix64: every choice the simulation makes, and every key and IV.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

impl Random for Seeded {
    fn fill(&mut self, out: &mut [u8]) {
        for b in out {
            *b = self.next() as u8;
        }
    }
}

/// A device's notes: one doc per room, written as this device's peer.
struct Notes {
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

struct Device {
    identity: Identity,
    session: Session,
    notes: Notes,
    conn: Option<ConnId>,
}

/// One socket: what is on its way each way, in order.
#[derive(Default)]
struct Socket {
    up: VecDeque<Vec<u8>>,
    down: VecDeque<Vec<u8>>,
    /// The server closed it: what it sent first still arrives, then the
    /// device sees it close; nothing more goes up.
    closing: bool,
}

/// Where the hub keeps things: memory, or SQLite with a database per
/// account — the same runs on both, so the binary's storage is held to
/// what memory does.
type Store = Box<dyn Storage>;

struct World {
    rng: Seeded,
    hub: Hub<Store>,
    devices: Vec<Device>,
    sockets: HashMap<ConnId, (usize, Socket)>,
    next_conn: ConnId,
    /// Every word typed, by room.
    typed: BTreeMap<String, Vec<String>>,
    words: usize,
    /// Devices turned away, in order.
    denied: Vec<usize>,
    /// Devices sent a record they could not open.
    unreadable: Vec<usize>,
    /// Who typed each word, in order.
    by: Vec<(usize, String)>,
    /// How many words had been typed when the server took the last device
    /// out, if it did.
    revoked_at: Option<usize>,
    /// A device that means to take the last one out: it tries whenever it
    /// is online, until the server has done it.
    remover: Option<usize>,
    now: u64,
    /// The picture worker, as the server runs one beside the hub.
    worker: Worker<MemoryBlobs>,
}

const ROOMS: [&str; 3] = ["0000000000000000000000000000000a", "0000000000000000000000000000000b", "0000000000000000000000000000000c"];

impl World {
    fn new(seed: u64, devices: usize) -> Self {
        // Every other seed on SQLite; every third on Postgres, given one.
        let store: Store = match postgres() {
            Some(pg) if seed % 3 == 2 => pg,
            _ if seed.is_multiple_of(2) => Box::new(Memory::default()),
            _ => Box::new(Sharded::memory().unwrap()),
        };
        Self::with(seed, devices, store)
    }

    fn with(seed: u64, devices: usize, store: Store) -> Self {
        let mut rng = Seeded(seed);
        let vault = VaultKey::generate(&mut rng);
        let mut hub = Hub::new(ORIGIN, store);
        hub.tick(START);
        hub.compact_when(3, 1 << 30);
        hub.store_mut().add_account(ACCOUNT, true).unwrap();
        let devices = (0..devices)
            .map(|i| {
                let key = DeviceKey::generate(&mut rng);
                let id = format!("device-{i}").into_bytes();
                hub.store_mut().add_device(ACCOUNT, &id, key.public()).unwrap();
                // Each has been online before the run starts: its card is
                // there, as its first login would have left it.
                let card = Card { device: id.clone(), public: key.public(), boxed: key.box_secret().public(), name: format!("{i}"), credential: Vec::new() };
                hub.store_mut().set_card(ACCOUNT, &id, card.seal(&Ring::new(vault.clone()), &mut rng)).unwrap();
                let identity = Identity { origin: ORIGIN.into(), account: ACCOUNT.to_vec(), device: id, key, ring: Ring::new(vault.clone()), name: String::new() };
                Device {
                    session: Session::new(identity.clone(), 0),
                    identity,
                    // Random, as the app's are: peers of every length, so
                    // whatever sorts them meets 10 before 9.
                    notes: Notes { peer: rng.next() >> (rng.next() % 60), docs: BTreeMap::new() },
                    conn: None,
                }
            })
            .collect();
        World {
            rng,
            hub,
            devices,
            sockets: HashMap::new(),
            worker: Worker::memory(),
            next_conn: 1,
            typed: BTreeMap::new(),
            words: 0,
            denied: Vec::new(),
            unreadable: Vec::new(),
            by: Vec::new(),
            revoked_at: None,
            remover: None,
            now: START,
        }
    }

    /// The device that may be taken out: the last.
    fn target(&self) -> usize {
        self.devices.len() - 1
    }

    fn target_id(&self) -> Vec<u8> {
        format!("device-{}", self.target()).into_bytes()
    }

    fn is_out(&self, d: usize) -> bool {
        self.revoked_at.is_some() && d == self.target()
    }

    /// What the device's session said, as the app takes it: a new key is
    /// kept (so a restart has it), the rest noted for the checks.
    fn events(&mut self, d: usize) {
        for e in self.devices[d].session.take_events() {
            match e {
                Event::Denied(_) | Event::Removed { .. } => self.denied.push(d),
                Event::Unreadable(_) => self.unreadable.push(d),
                Event::Rekeyed(_) => self.devices[d].identity.ring = self.devices[d].session.ring().clone(),
                _ => {}
            }
        }
    }

    fn connect(&mut self, d: usize) {
        if self.devices[d].conn.is_some() {
            return;
        }
        let conn = self.next_conn;
        self.next_conn += 1;
        self.sockets.insert(conn, (d, Socket::default()));
        self.devices[d].conn = Some(conn);
        self.devices[d].session.connected();
        let outs = self.hub.connect(conn, &mut self.rng);
        self.route(outs);
    }

    fn disconnect(&mut self, d: usize) {
        if let Some(conn) = self.devices[d].conn.take() {
            self.sockets.remove(&conn);
            let outs = self.hub.disconnect(conn);
            self.devices[d].session.disconnected();
            self.route(outs);
        }
    }

    fn route(&mut self, outs: Vec<Out>) {
        for o in outs {
            match o {
                Out::Send(conn, bytes) => {
                    if let Some((_, s)) = self.sockets.get_mut(&conn) {
                        s.down.push_back(bytes);
                    }
                }
                Out::Close(conn) => {
                    if let Some((_, s)) = self.sockets.get_mut(&conn) {
                        s.closing = true;
                        s.up.clear();
                    }
                }
                Out::Blob(job) => {
                    let sockets = &mut self.sockets;
                    self.worker.run(job, &mut |o| {
                        if let Out::Send(conn, bytes) = o
                            && let Some((_, s)) = sockets.get_mut(&conn)
                        {
                            s.down.push_back(bytes);
                        }
                    });
                }
            }
        }
    }

    /// What a device's session has to send, onto its socket.
    fn flush(&mut self, d: usize) {
        let out = self.devices[d].session.take_out();
        self.events(d);
        if let Some(conn) = self.devices[d].conn
            && let Some((_, s)) = self.sockets.get_mut(&conn)
        {
            s.up.extend(out);
        }
    }

    /// A device types a word somewhere into a note, and saves. Returns it.
    fn type_word(&mut self, d: usize) -> String {
        let room = ROOMS[self.rng.below(ROOMS.len())];
        self.words += 1;
        let word = format!("w{:05}x", self.words);
        let dev = &mut self.devices[d];
        let doc = dev.notes.doc(room);
        let text = doc.text();
        // Between words, never inside one: a word typed into another
        // would not be found whole, and it is wholes that are checked.
        let gaps: Vec<usize> = std::iter::once(0)
            .chain(text.char_indices().filter(|&(_, c)| c == ' ').map(|(i, _)| i))
            .chain(std::iter::once(text.len()))
            .collect();
        let at = gaps[self.rng.below(gaps.len())];
        let new = format!("{} {word} {}", &text[..at], &text[at..]);
        doc.record(&new, Origin::Save);
        self.typed.entry(room.to_string()).or_default().push(word.clone());
        self.by.push((d, word));
        let World { devices, rng, .. } = self;
        let dev = &mut devices[d];
        dev.session.saved(room, &mut dev.notes, rng);
        self.flush(d);
        self.typed[room].last().cloned().expect("typed")
    }

    /// One message, on a socket chosen at random, the way chosen at random.
    fn deliver_one(&mut self) -> bool {
        let mut busy: Vec<(ConnId, bool)> = Vec::new();
        let mut conns: Vec<&ConnId> = self.sockets.keys().collect();
        conns.sort_unstable();
        for &c in conns {
            let s = &self.sockets[&c].1;
            if !s.up.is_empty() {
                busy.push((c, true));
            }
            if !s.down.is_empty() {
                busy.push((c, false));
            }
        }
        if busy.is_empty() {
            return false;
        }
        let (conn, up) = busy[self.rng.below(busy.len())];
        let (d, s) = self.sockets.get_mut(&conn).expect("busy");
        let d = *d;
        if up {
            let bytes = s.up.pop_front().expect("busy");
            let outs = self.hub.receive(conn, &bytes, &mut self.rng);
            self.route(outs);
            if self.revoked_at.is_none() && self.hub.store().device(ACCOUNT, &self.target_id()).is_some_and(|d| d.revoked) {
                self.revoked_at = Some(self.words);
            }
        } else {
            let bytes = s.down.pop_front().expect("busy");
            let closed = s.closing && s.down.is_empty();
            let World { devices, rng, .. } = self;
            let dev = &mut devices[d];
            dev.session.receive(&bytes, &mut dev.notes, rng);
            self.flush(d);
            if closed {
                self.disconnect(d);
            }
        }
        true
    }

    /// The app quits and starts again: its notes are on disk, its session
    /// starts afresh from the cursor it saved.
    fn restart_device(&mut self, d: usize) {
        self.disconnect(d);
        let cursor = self.devices[d].session.cursor();
        let id = self.devices[d].identity.clone();
        self.devices[d].session = Session::new(id, cursor);
    }

    /// The server goes down and comes back: every socket drops, the
    /// storage stays.
    fn restart_server(&mut self) {
        for d in 0..self.devices.len() {
            self.disconnect(d);
        }
        let placeholder: Store = Box::<Memory>::default();
        let hub = std::mem::replace(&mut self.hub, Hub::new(ORIGIN, placeholder));
        self.hub = Hub::new(ORIGIN, hub.into_store());
        self.hub.tick(self.now);
        self.hub.compact_when(3, 1 << 30);
    }

    fn step(&mut self) {
        self.now += 10;
        self.hub.tick(self.now);
        // The remover, online, tries: by the list it last read, whatever
        // has happened since (one not read yet, it asks for).
        if let Some(r) = self.remover
            && self.revoked_at.is_none()
            && self.devices[r].session.is_ready()
            && self.rng.below(2) == 0
        {
            let target = self.target_id();
            let World { devices, rng, .. } = self;
            if devices[r].session.revoke(&target, rng).is_err() {
                devices[r].session.list_devices();
            }
            self.flush(r);
        }
        let d = self.rng.below(self.devices.len());
        match self.rng.below(42) {
            0..10 => {
                self.type_word(d);
            }
            10..34 => {
                self.deliver_one();
            }
            34 | 35 => self.disconnect(d),
            36 => self.restart_device(d),
            37 => self.restart_server(),
            38 => {
                self.devices[d].session.list_devices();
                self.flush(d);
            }
            // Now and then, a device sets out to take the last one out.
            39 if d != self.target() && self.remover.is_none() => self.remover = Some(d),
            _ => self.connect(d),
        }
    }

    /// Everyone back, everything delivered.
    fn settle(&mut self) {
        for d in 0..self.devices.len() {
            if !self.is_out(d) {
                self.connect(d);
            }
        }
        for _ in 0..200_000 {
            if !self.deliver_one() && (0..self.devices.len()).all(|d| self.is_out(d) || self.devices[d].session.is_idle()) {
                return;
            }
        }
        panic!("did not settle");
    }

    fn check(&self, seed: u64) {
        assert!(self.denied.iter().all(|&d| self.is_out(d)), "seed {seed}: a device in the account was turned away");
        assert!(self.unreadable.is_empty(), "seed {seed}: a device was sent a record it could not open");
        // Words the device taken out typed may never have left it.
        let out_words: Vec<&String> = self.by.iter().filter(|(d, _)| self.is_out(*d)).map(|(_, w)| w).collect();
        for room in ROOMS {
            let texts: Vec<Option<String>> = self
                .devices
                .iter()
                .enumerate()
                .map(|(i, d)| if self.is_out(i) { None } else { d.notes.docs.get(room).map(NoteDoc::text) })
                .collect();
            let Some(typed) = self.typed.get(room) else { continue };
            let typed: Vec<&String> = typed.iter().filter(|w| !out_words.contains(w)).collect();
            let first = texts.iter().flatten().next().cloned();
            for (i, t) in texts.iter().enumerate() {
                if self.is_out(i) {
                    continue;
                }
                let t = t.as_ref().unwrap_or_else(|| panic!("seed {seed}: device {i} has no {room}"));
                assert_eq!(Some(t), first.as_ref(), "seed {seed}: device {i} differs in {room}");
                for w in &typed {
                    if !t.contains(w.as_str()) {
                        let spans: Vec<(u64, u64, u64)> =
                            self.hub.store().spans(ACCOUNT, room).iter().map(|s| (s.peer, s.start, s.end)).collect();
                        let vvs: Vec<Vec<(u64, u64)>> =
                            self.devices.iter().map(|d| d.notes.docs.get(room).map(NoteDoc::version_vector).unwrap_or_default()).collect();
                        panic!("seed {seed}: {w} lost from {room}\ntexts: {texts:?}\nserver spans: {spans:?}\ndevice vvs: {vvs:?}");
                    }
                }
            }
        }
        // The device taken out: nothing typed after, nothing sealed since.
        if let Some(at) = self.revoked_at {
            let out = &self.devices[self.target()];
            let held: String = out.notes.docs.values().map(NoteDoc::text).collect();
            for (d, w) in &self.by[at..] {
                assert!(*d == self.target() || !held.contains(w.as_str()), "seed {seed}: the device taken out has {w}, typed after");
            }
            for room in ROOMS {
                for s in self.hub.store().spans(ACCOUNT, room) {
                    if matches!(elo::header(&s.record), Ok(Header::Delta { epoch, .. }) if epoch > 0) {
                        assert!(elo::open(out.session.ring(), room, &s.record).is_err(), "seed {seed}: the device taken out opens a new record");
                    }
                }
            }
        }
        let stored: Vec<Vec<u8>> = ROOMS.iter().flat_map(|r| self.hub.store().spans(ACCOUNT, r)).map(|s| s.record).collect();
        for record in &stored {
            for words in self.typed.values() {
                for w in words {
                    assert!(!record.windows(w.len()).any(|x| x == w.as_bytes()), "seed {seed}: the server holds {w} in the clear");
                }
            }
        }
    }
}

/// What a run did, so a run that did nothing cannot pass for one that
/// did everything.
#[derive(Default, Debug)]
struct Stats {
    words: usize,
    records: usize,
    rooms_everywhere: usize,
    /// Runs in which a device was taken out.
    revoked: usize,
    /// Rooms standing on a base at the end.
    bases: usize,
}

fn run(seed: u64, devices: usize, steps: usize) -> Stats {
    let mut w = World::new(seed, devices);
    for _ in 0..steps {
        w.step();
    }
    w.settle();
    w.check(seed);
    Stats {
        words: w.words,
        records: ROOMS.iter().map(|r| w.hub.store().spans(ACCOUNT, r).len()).sum(),
        rooms_everywhere: ROOMS
            .iter()
            .filter(|r| (0..w.devices.len()).all(|d| w.is_out(d) || w.devices[d].notes.docs.contains_key(**r)))
            .count(),
        revoked: usize::from(w.revoked_at.is_some()),
        bases: ROOMS.iter().filter(|r| w.hub.store().base(ACCOUNT, r).is_some()).count(),
    }
}

#[test]
fn devices_converge_and_the_server_reads_nothing() {
    let seeds: u64 = std::env::var("SIM_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let mut total = Stats::default();
    for seed in 0..seeds {
        let s = run(seed, 4, 300);
        total.words += s.words;
        total.records += s.records;
        total.rooms_everywhere += s.rooms_everywhere;
        total.revoked += s.revoked;
        total.bases += s.bases;
    }
    // Rooms compacted, in most runs.
    assert!(total.bases > seeds as usize, "{total:?}");
    // A device taken out in a good share of runs, not in all.
    assert!(total.revoked > seeds as usize / 10 && total.revoked < seeds as usize, "{total:?}");
    // About 75 words a run, through records the server kept, into notes
    // that reached every device.
    assert!(total.words > seeds as usize * 50, "{total:?}");
    assert!(total.records > seeds as usize * 10, "{total:?}");
    assert!(total.rooms_everywhere > seeds as usize * 2, "{total:?}");
    if std::env::var_os("SIM_STATS").is_some() {
        println!("{seeds} runs: {total:?}");
    }
}

#[test]
fn a_device_not_enrolled_or_revoked_is_turned_away() {
    let mut w = World::new(9, 3);
    w.hub.store_mut().revoke_device(ACCOUNT, b"device-1").unwrap();
    // Device 2 signs with a key the server never enrolled.
    w.devices[2].session = Session::new(
        Identity {
            origin: ORIGIN.into(),
            account: ACCOUNT.to_vec(),
            device: b"device-2".to_vec(),
            key: DeviceKey::generate(&mut w.rng),
            ring: Ring::new(VaultKey::generate(&mut w.rng)),
            name: String::new(),
        },
        0,
    );
    for d in 0..3 {
        w.connect(d);
    }
    while w.deliver_one() {}
    w.denied.sort_unstable();
    assert_eq!(w.denied, [1, 2]);
    assert!(w.devices[0].session.is_ready());
    assert!(w.devices[1].conn.is_none() && w.devices[2].conn.is_none(), "their sockets closed");
}

/// A paste far past what one record or one message may hold goes in
/// pieces, and comes out whole on the other side.
#[test]
fn a_big_paste_goes_in_pieces() {
    let mut w = World::new(4, 2);
    w.connect(0);
    w.connect(1);
    while w.deliver_one() {}
    let big: String = (0..40_000).map(|i| format!("w{i:05}x ")).collect();
    assert!(big.len() > 300_000);
    {
        let World { devices, rng, .. } = &mut w;
        let dev = &mut devices[0];
        dev.notes.doc(ROOMS[0]).record(&big, Origin::Save);
        dev.session.saved(ROOMS[0], &mut dev.notes, rng);
    }
    w.flush(0);
    w.settle();
    assert_eq!(w.devices[1].notes.docs.get(ROOMS[0]).map(NoteDoc::text).as_deref(), Some(big.as_str()));
    let spans = w.hub.store().spans(ACCOUNT, ROOMS[0]);
    assert!(spans.len() > 1, "cut in pieces");
    assert!(spans.iter().all(|s| s.record.len() <= carnet_server::hub::MAX_RECORD));
}

/// Two accounts on one server, typing into notes with the same ids: a
/// device of one never takes in a record of the other's (not even one it
/// could not open), and each account's notes hold its own words only. On
/// SQLite, whose files keep accounts apart; and on Postgres, given one,
/// where only each query's account does.
#[test]
fn accounts_never_meet() {
    for seed in 0..40 {
        let store = match postgres() {
            Some(pg) if seed % 2 == 1 => pg,
            _ => Box::new(Sharded::memory().unwrap()),
        };
        let mut w = World::with(seed, 3, store);
        // Device 2 is another account's, with its own vault.
        let other = b"account-2".to_vec();
        let key = DeviceKey::generate(&mut w.rng);
        w.hub.store_mut().add_account(&other, false).unwrap();
        w.hub.store_mut().add_device(&other, b"device-2", key.public()).unwrap();
        let identity = Identity {
            origin: ORIGIN.into(),
            account: other.clone(),
            device: b"device-2".to_vec(),
            key,
            ring: Ring::new(VaultKey::generate(&mut w.rng)),
            name: String::new(),
        };
        w.devices[2].identity = identity.clone();
        w.devices[2].session = Session::new(identity, 0);
        let (mut mine, mut theirs) = (Vec::new(), Vec::new());
        for _ in 0..400 {
            match w.rng.below(8) {
                0 => {
                    let d = w.rng.below(2);
                    mine.push(w.type_word(d));
                }
                1 => theirs.push(w.type_word(2)),
                2 => {
                    let d = w.rng.below(3);
                    w.connect(d);
                }
                _ => {
                    w.deliver_one();
                }
            }
        }
        w.settle();
        assert!(w.unreadable.is_empty(), "seed {seed}: a device was sent another account's records");
        assert!(!mine.is_empty() && !theirs.is_empty(), "seed {seed}: both typed");
        let words = |d: usize| -> Vec<String> {
            w.devices[d].notes.docs.values().flat_map(|doc| doc.text().split_whitespace().map(str::to_string).collect::<Vec<_>>()).collect()
        };
        for word in words(2) {
            assert!(theirs.contains(&word), "seed {seed}: account 2 holds account 1's {word}");
        }
        for d in 0..2 {
            for word in words(d) {
                assert!(mine.contains(&word), "seed {seed}: account 1 holds account 2's {word}");
            }
        }
        for word in &theirs {
            assert!(words(2).contains(word), "seed {seed}: account 2 lost {word}");
        }
        assert_eq!(words(0).len(), mine.len(), "seed {seed}: account 1 has all of its own");
    }
}
