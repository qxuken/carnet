//! The web app's "Show a code" through pairing, login, note sync and reload.
//! Two independent browser vaults communicate only through the real hub.

use carnet_server::{Hub, Memory, Out, Storage};
use carnet_sync::crypto::{DeviceKey, Random, Ring, VaultKey};
use carnet_sync::pair::{Joiner, Joining, PairCode};
use carnet_web::notes::{Me, Notes, Said};

const ORIGIN: &str = "https://notes.example";
const ACCOUNT: &[u8] = b"owner-account";

struct Seeded(u64);
impl Random for Seeded {
    fn fill(&mut self, out: &mut [u8]) {
        for b in out {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
            *b = (self.0 >> 32) as u8;
        }
    }
}

struct Browsers {
    hub: Hub<Memory>,
    web: Notes,
    pwa: Option<Notes>,
    joiner: Option<Joiner>,
    rng: Seeded,
}

impl Browsers {
    fn new(seed: u64) -> Self {
        let mut rng = Seeded(seed);
        let mut hub = Hub::new(ORIGIN, Memory::default());
        hub.store_mut().add_account(ACCOUNT, true).unwrap();
        let key = DeviceKey::generate(&mut rng);
        hub.store_mut().add_device(ACCOUNT, b"web", key.public()).unwrap();
        let me = Me { account: ACCOUNT.to_vec(), device: b"web".to_vec(), key, ring: Ring::new(VaultKey::generate(&mut rng)) };
        let web = Notes::open(ORIGIN, me, "Safari", &[], 1).unwrap();
        let mut net = Self { hub, web, pwa: None, joiner: None, rng };
        net.web.session.connected();
        let out = net.hub.connect(1, &mut net.rng);
        net.deliver(out);
        net.pump();
        assert!(net.web.session.is_ready());
        net
    }

    fn deliver(&mut self, out: Vec<Out>) {
        for o in out {
            match o {
                Out::Send(1, b) => self.web.receive(&b, &mut self.rng),
                Out::Send(2, b) => {
                    if let Some(pwa) = &mut self.pwa {
                        pwa.receive(&b, &mut self.rng);
                    } else {
                        self.joiner.as_mut().expect("a joining browser").receive(&b, &mut self.rng);
                    }
                }
                other => panic!("unexpected hub action: {other:?}"),
            }
        }
    }

    fn pump(&mut self) {
        for _ in 0..100 {
            let mut frames: Vec<_> = self.web.session.take_out().into_iter().map(|b| (1, b)).collect();
            if let Some(pwa) = &mut self.pwa {
                frames.extend(pwa.session.take_out().into_iter().map(|b| (2, b)));
            } else if let Some(joiner) = &mut self.joiner {
                frames.extend(joiner.take_out().into_iter().map(|b| (2, b)));
            }
            if frames.is_empty() {
                return;
            }
            for (conn, b) in frames {
                let out = self.hub.receive(conn, &b, &mut self.rng);
                self.deliver(out);
            }
            self.web.take_changed();
            if let Some(pwa) = &mut self.pwa {
                pwa.take_changed();
            }
        }
        panic!("pairing did not settle");
    }

    fn show_code(&mut self) -> String {
        self.web.said.clear();
        self.web.pair_open(&mut self.rng);
        self.pump();
        self.web
            .said
            .iter()
            .find_map(|s| match s {
                Said::PairCode(c) => Some(c.clone()),
                _ => None,
            })
            .expect("Show a code produces a code")
    }

    fn join(&mut self, code: PairCode) -> DeviceKey {
        let key = DeviceKey::generate(&mut self.rng);
        self.joiner = Some(Joiner::new(code, b"pwa".to_vec(), key.clone()));
        let out = self.hub.connect(2, &mut self.rng);
        self.deliver(out);
        self.pump();
        key
    }

    fn finish(&mut self, key: DeviceKey) -> Vec<u8> {
        let state = self.joiner.take().unwrap().state;
        let Joining::In(admitted) = state else {
            panic!("pairing failed: {state:?}");
        };
        assert_eq!(admitted.account, ACCOUNT);
        // Same identity serialization and challenge handover as Join.identity
        // and sync.svelte.ts use when replacing the joiner with a vault.
        let identity = Me { account: admitted.account, device: b"pwa".to_vec(), key, ring: admitted.ring }.to_bytes();
        let mut pwa = Notes::open(ORIGIN, Me::from_bytes(&identity).unwrap(), "Home Screen", &[], 2).unwrap();
        pwa.session.connected();
        pwa.receive(&admitted.challenge, &mut self.rng);
        self.pwa = Some(pwa);
        self.pump();
        assert!(self.pwa.as_ref().unwrap().session.is_ready());
        identity
    }
}

#[test]
fn a_shown_code_pairs_an_independent_browser_and_its_notes_survive_reload() {
    for seed in 1..=8 {
        for linked in [false, true] {
            let mut net = Browsers::new(seed);
            let note = net.web.create("");
            let (_, base) = net.web.source(&note).unwrap();
            net.web.edit(&note, &base, "# Before pairing\n\nAlready encrypted.\n", &mut net.rng).unwrap();
            net.pump();
            let shown = net.show_code();
            let input = if linked { PairCode::parse(&shown).unwrap().link(ORIGIN) } else { format!("  {}  ", shown.replace('-', " - ")) };
            let key = net.join(PairCode::from_link(&input).unwrap());
            let identity = net.finish(key);
            assert!(net.web.said.iter().any(|s| matches!(s, Said::Paired(d) if d == b"pwa")));
            let pwa = net.pwa.as_mut().unwrap();
            assert_eq!(pwa.source(&note).unwrap().0, "# Before pairing\n\nAlready encrypted.\n");
            let (_, base) = pwa.source(&note).unwrap();
            pwa.edit(&note, &base, "# Before pairing\n\nEdited in the PWA.\n", &mut net.rng).unwrap();
            net.pump();
            assert_eq!(net.web.source(&note), net.pwa.as_ref().unwrap().source(&note));

            let state = net.pwa.as_ref().unwrap().save();
            let out = net.hub.disconnect(2);
            net.deliver(out);
            let mut pwa = Notes::open(ORIGIN, Me::from_bytes(&identity).unwrap(), "Home Screen", &state, 2).unwrap();
            pwa.session.connected();
            net.pwa = Some(pwa);
            let out = net.hub.connect(2, &mut net.rng);
            net.deliver(out);
            net.pump();
            assert!(net.pwa.as_ref().unwrap().session.is_ready());
            assert_eq!(net.pwa.as_ref().unwrap().source(&note).unwrap().0, "# Before pairing\n\nEdited in the PWA.\n");
        }
    }
}

#[test]
fn a_wrong_code_can_be_replaced_with_a_fresh_code_and_then_pair() {
    let mut net = Browsers::new(17);
    let code = PairCode::parse(&net.show_code()).unwrap();
    net.join(PairCode { secret: (code.secret + 1) % 1_000_000, ..code });
    assert!(matches!(net.joiner.as_ref().unwrap().state, Joining::Failed(_)));
    assert!(net.hub.store().device(ACCOUNT, b"pwa").is_none());
    assert!(net.web.said.iter().any(|s| matches!(s, Said::PairFailed(_))));
    let out = net.hub.disconnect(2);
    net.deliver(out);
    let fresh = PairCode::parse(&net.show_code()).unwrap();
    let key = net.join(fresh);
    net.finish(key);
}

#[test]
fn disconnecting_the_browser_invalidates_its_code_but_reconnecting_can_make_a_new_one() {
    let mut net = Browsers::new(29);
    let old = PairCode::parse(&net.show_code()).unwrap();
    let out = net.hub.disconnect(1);
    net.deliver(out);
    net.web.session.disconnected();
    net.join(old);
    assert!(matches!(net.joiner.as_ref().unwrap().state, Joining::Failed(_)));
    let out = net.hub.disconnect(2);
    net.deliver(out);
    net.web.session.connected();
    let out = net.hub.connect(1, &mut net.rng);
    net.deliver(out);
    net.pump();
    let fresh = PairCode::parse(&net.show_code()).unwrap();
    let key = net.join(fresh);
    net.finish(key);
}

#[test]
fn pressing_show_a_code_twice_before_the_reply_still_shows_a_usable_code() {
    let mut net = Browsers::new(41);
    // Both clicks happen before the WebSocket can deliver PairOpened.
    net.web.pair_open(&mut net.rng);
    net.web.pair_open(&mut net.rng);
    net.pump();
    let shown = net
        .web
        .said
        .iter()
        .rev()
        .find_map(|s| match s {
            Said::PairCode(code) => Some(code.clone()),
            _ => None,
        })
        .expect("a code shown after two clicks");
    let key = net.join(PairCode::parse(&shown).unwrap());
    net.finish(key);
}

#[test]
fn replacing_a_shown_code_pairs_with_the_new_code() {
    let mut net = Browsers::new(53);
    let old = net.show_code();
    let fresh = net.show_code();
    assert_ne!(old, fresh);
    let key = net.join(PairCode::parse(&fresh).unwrap());
    net.finish(key);
}
