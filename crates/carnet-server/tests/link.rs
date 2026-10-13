//! The desktop's own pieces against the binary: `Link` threads for the
//! sockets, an `Enroller` making the account by an invite, a `Joiner`
//! pairing a second device, and their `Session`s syncing a note.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use carnet_server::Sharded;
use carnet_server::server::{self, Config};
use carnet_sync::crypto::{DeviceKey, OsRandom};
use carnet_sync::net::{Link, NetEvent, sync_url};
use carnet_sync::pair::{Admitted, Enroller, Joiner, Joining, PairCode};
use carnet_sync::session::{Docs, Event, Identity, Session};
use carnet_sync::{NoteDoc, Origin};
use tokio::net::TcpListener;

const ROOM: &str = "000000000000000000000000000000aa";

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

/// Polls `link` until `done`, or ten seconds.
fn until(mut step: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(10) {
        if step() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out");
}

struct Device {
    link: Link,
    session: Session,
    notes: Notes,
    events: Vec<Event>,
}

impl Device {
    fn admitted(link: Link, origin: &str, device: &[u8], key: DeviceKey, a: Admitted, peer: u64) -> Self {
        let id = Identity { origin: origin.into(), account: a.account, device: device.to_vec(), key, ring: a.ring, name: String::new() };
        let mut d = Device { link, session: Session::new(id, 0), notes: Notes { peer, docs: BTreeMap::new() }, events: Vec::new() };
        d.session.connected();
        d.session.receive(&a.challenge, &mut d.notes, &mut OsRandom);
        d.flush();
        d
    }

    fn flush(&mut self) {
        for b in self.session.take_out() {
            self.link.send(b);
        }
        self.events.extend(self.session.take_events());
    }

    /// Takes in what came, sends what is to go.
    fn step(&mut self) {
        for e in self.link.poll() {
            match e {
                NetEvent::Connected => self.session.connected(),
                NetEvent::Frame(b) => self.session.receive(&b, &mut self.notes, &mut OsRandom),
                NetEvent::Disconnected(_) => self.session.disconnected(),
            }
        }
        self.flush();
    }
}

#[test]
fn a_desktop_enrols_pairs_and_syncs_over_its_own_sockets() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let data = std::env::temp_dir().join(format!("carnet-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let code = server::new_invite(&mut Sharded::open(&data).unwrap()).unwrap();
    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = rt.spawn(server::serve(Config { data: data.clone(), origin: origin.clone(), compact_after: server::COMPACT_AFTER, web: None, trusted: Vec::new(), client_header: Default::default() }, listener, async {
        let _ = stopped.await;
    }));
    let url = sync_url(&origin, "test", "0").unwrap();

    // A: the account made by the invite, over its socket.
    let key_a = DeviceKey::generate(&mut OsRandom);
    let link_a = Link::start(url.clone(), || {}).unwrap();
    let mut enroller = Enroller::new(code.text(), b"desk".to_vec(), key_a.clone(), &mut OsRandom);
    until(|| {
        for e in link_a.poll() {
            if let NetEvent::Frame(b) = e {
                enroller.receive(&b);
            }
        }
        for b in enroller.take_out() {
            link_a.send(b);
        }
        !matches!(enroller.state, Joining::Going)
    });
    let Joining::In(admitted) = enroller.state else { panic!("{:?}", enroller.state) };
    let mut a = Device::admitted(link_a, &origin, b"desk", key_a, admitted, 1);
    until(|| {
        a.step();
        a.session.is_ready()
    });
    a.notes.doc(ROOM).record("# Plans\n\n- [ ] book the train\n", Origin::Save);
    a.session.saved(ROOM, &mut a.notes, &mut OsRandom);
    a.flush();
    until(|| {
        a.step();
        a.session.is_idle()
    });

    // B: paired from A's code, over its own socket.
    a.session.pair_open(&mut OsRandom);
    a.flush();
    let mut shown = None;
    until(|| {
        a.step();
        shown = a.events.iter().find_map(|e| match e {
            Event::PairCode(c) => PairCode::parse(c),
            _ => None,
        });
        shown.is_some()
    });
    let key_b = DeviceKey::generate(&mut OsRandom);
    let link_b = Link::start(url, || {}).unwrap();
    let mut joiner = Joiner::new(shown.unwrap(), b"laptop".to_vec(), key_b.clone());
    until(|| {
        a.step();
        for e in link_b.poll() {
            if let NetEvent::Frame(b) = e {
                joiner.receive(&b, &mut OsRandom);
            }
        }
        for b in joiner.take_out() {
            link_b.send(b);
        }
        !matches!(joiner.state, Joining::Going)
    });
    let Joining::In(admitted) = joiner.state else { panic!("{:?}", joiner.state) };
    let mut b = Device::admitted(link_b, &origin, b"laptop", key_b, admitted, 2);
    until(|| {
        a.step();
        b.step();
        b.notes.docs.get(ROOM).is_some_and(|d| d.text().contains("book the train"))
    });

    // And back: B writes, A has it.
    b.notes.doc(ROOM).record("# Plans\n\n- [x] book the train\n", Origin::Save);
    b.session.saved(ROOM, &mut b.notes, &mut OsRandom);
    b.flush();
    until(|| {
        a.step();
        b.step();
        a.notes.docs[ROOM].text().contains("[x] book")
    });

    drop(a);
    drop(b);
    let _ = stop.send(());
    rt.block_on(served).unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&data);
}
