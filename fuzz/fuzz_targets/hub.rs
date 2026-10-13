//! The server's hub fed any frames from any of a few sockets, opened and
//! closed at any time, with one real device among them (it logs in when
//! told to), and each socket said to have written what it was sent at any
//! time, a room's records a small part at a time (`drained`,
//! `take_more`): nothing may panic, whatever a client sends or in
//! whatever order.
#![no_main]

use std::collections::HashMap;

use arbitrary::Arbitrary;
use carnet_server::{Hub, Memory, Out, Storage};
use carnet_sync::crypto::{DeviceKey, Random, login_message};
use carnet_sync::wire::{self, Carnet, Frame};
use libfuzzer_sys::fuzz_target;

struct Counter(u64);

impl Random for Counter {
    fn fill(&mut self, out: &mut [u8]) {
        for b in out {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
            *b = (self.0 >> 33) as u8;
        }
    }
}

#[derive(Arbitrary, Debug)]
enum Op {
    Connect(u8),
    Disconnect(u8),
    Send(u8, Vec<u8>),
    /// The real device logs in on the socket, signing its challenge.
    Login(u8),
    /// The socket has written most of what it was sent, asked or not.
    Drained(u8),
    /// Every socket with more to come, drained.
    More,
}

const ORIGIN: &str = "https://fuzz.example";

fuzz_target!(|ops: Vec<Op>| {
    let mut rng = Counter(7);
    let key = DeviceKey::from_secret([1; 32]);
    let mut hub = Hub::new(ORIGIN, Memory::default());
    hub.store_mut().add_account(b"acct", true).unwrap();
    hub.store_mut().add_device(b"acct", b"dev", key.public()).unwrap();
    // A room with more than a part of records, so its backfill goes in
    // several; the bytes needn't open, the hub never reads them.
    for i in 0..8 {
        hub.store_mut().put(b"acct", "r", 1, i, i + 1, vec![i as u8; 300]).unwrap();
    }
    hub.backfill_part(512);
    // Each socket's challenge, as the hub sent it.
    let mut nonces: HashMap<u64, [u8; 32]> = HashMap::new();
    for op in ops.into_iter().take(64) {
        let outs = match op {
            Op::Connect(c) => hub.connect(u64::from(c % 4), &mut rng),
            Op::Disconnect(c) => hub.disconnect(u64::from(c % 4)),
            Op::Send(c, bytes) => hub.receive(u64::from(c % 4), &bytes, &mut rng),
            Op::Login(c) => {
                let conn = u64::from(c % 4);
                let nonce = nonces.get(&conn).copied().unwrap_or_default();
                let signature = key.sign(&login_message(ORIGIN, b"acct", b"dev", &nonce));
                let hello = Frame::Carnet(Carnet::Hello { account: b"acct".to_vec(), device: b"dev".to_vec(), signature, since: 0 });
                hub.receive(conn, &wire::encode(&hello), &mut rng)
            }
            Op::Drained(c) => hub.drained(u64::from(c % 4)),
            Op::More => hub.take_more().into_iter().flat_map(|c| hub.drained(c)).collect(),
        };
        for o in outs {
            if let Out::Send(c, b) = o
                && let Ok(Frame::Carnet(Carnet::Challenge { nonce, .. })) = wire::decode(&b)
            {
                nonces.insert(c, nonce);
            }
        }
    }
});
