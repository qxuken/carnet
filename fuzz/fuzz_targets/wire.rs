//! Any bytes as what comes off a socket: a frame, a record, a container
//! of records, a version vector. Nothing may panic; what decodes encodes
//! back to bytes that decode the same.
#![no_main]

use carnet_sync::{elo, wire};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(frame) = wire::decode(bytes) {
        assert_eq!(wire::decode(&wire::encode(&frame)), Ok(frame), "a frame that does not come back");
    }
    let _ = elo::header(bytes);
    let _ = elo::records(bytes);
    let _ = wire::decode_vv(bytes);
});
