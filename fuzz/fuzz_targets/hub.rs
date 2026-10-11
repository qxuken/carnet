//! The server's hub fed any frames from any of a few sockets, opened and
//! closed at any time, with one real device among them: nothing may
//! panic, whatever a client sends or in whatever order.
#![no_main]

use arbitrary::Arbitrary;
use carnet_server::{Hub, Memory, Storage};
use carnet_sync::crypto::{DeviceKey, Random};
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
}

fuzz_target!(|ops: Vec<Op>| {
    let mut rng = Counter(7);
    let mut hub = Hub::new("https://fuzz.example", Memory::default());
    hub.store_mut().add_account(b"acct", true).unwrap();
    hub.store_mut().add_device(b"acct", b"dev", DeviceKey::from_secret([1; 32]).public()).unwrap();
    for op in ops.into_iter().take(64) {
        let _ = match op {
            Op::Connect(c) => hub.connect(u64::from(c % 4), &mut rng),
            Op::Disconnect(c) => hub.disconnect(u64::from(c % 4)),
            Op::Send(c, bytes) => hub.receive(u64::from(c % 4), &bytes, &mut rng),
        };
    }
});
