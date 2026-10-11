//! `%ELO` records: a doc's changes, sealed, with the plaintext header the
//! server indexes by (docs/SYNC.md §6). The layout is the Loro Protocol's
//! (its `elo` module reads it); writing it, and sealing, is ours.
//!
//! ```text
//! delta     0x00, peer (var bytes), start, end (uleb), key id (var str),
//!           IV (var bytes, 12), then the sealed bytes (var bytes)
//! snapshot  0x01, count, (peer, counter)…, key id, IV, sealed bytes
//! ```
//!
//! Everything before the sealed bytes is the header, and the header is the
//! AES-GCM associated data: the server reads it, and cannot change it.
//! A peer is written as its decimal digits, as the Loro Protocol's other
//! clients write it. The key id names the key's epoch: `e0`, `e1`, ….

use loro_protocol::{BytesReader, BytesWriter};
use loro_protocol::elo::{EloHeader, decode_elo_container, parse_elo_record_header};

use crate::crypto::{Random, Ring, VaultKey};

/// What a record holds, as its header says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Header {
    /// `peer`'s operations `start..end`.
    Delta { peer: u64, start: u64, end: u64, epoch: u32 },
    /// The whole doc as of `vv`.
    Snapshot { vv: Vec<(u64, u64)>, epoch: u32 },
}

/// A record that is not one, or that does not open.
#[derive(Debug, PartialEq, Eq)]
pub struct BadRecord(pub String);

impl std::fmt::Display for BadRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bad record: {}", self.0)
    }
}

impl std::error::Error for BadRecord {}

fn key_id(epoch: u32) -> String {
    format!("e{epoch}")
}

fn epoch_of(key_id: &str) -> Result<u32, BadRecord> {
    key_id.strip_prefix('e').and_then(|n| n.parse().ok()).ok_or_else(|| BadRecord(format!("key id {key_id:?}")))
}

fn peer_of(bytes: &[u8]) -> Result<u64, BadRecord> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| BadRecord("peer".into()))
}

/// `plain` (a doc's changes) sealed as a record with `header`, under the
/// key of `room` at the header's epoch.
pub fn seal(vault: &VaultKey, room: &str, header: &Header, plain: &[u8], rng: &mut impl Random) -> Vec<u8> {
    let iv: [u8; 12] = rng.bytes();
    let mut w = BytesWriter::new();
    let epoch = match header {
        Header::Delta { peer, start, end, epoch } => {
            w.push_byte(0x00);
            w.push_var_bytes(peer.to_string().as_bytes());
            w.push_uleb128(*start);
            w.push_uleb128(*end);
            *epoch
        }
        Header::Snapshot { vv, epoch } => {
            w.push_byte(0x01);
            w.push_uleb128(vv.len() as u64);
            // The Loro Protocol wants a snapshot's peers sorted as they are
            // written, by their digits' bytes: 10 before 9.
            let mut written: Vec<(String, u64)> = vv.iter().map(|(p, c)| (p.to_string(), *c)).collect();
            written.sort();
            for (peer, counter) in written {
                w.push_var_bytes(peer.as_bytes());
                w.push_uleb128(counter);
            }
            *epoch
        }
    };
    w.push_var_string(&key_id(epoch));
    w.push_var_bytes(&iv);
    let aad = w.finalize();
    let sealed = vault.doc_key(epoch, room).seal(iv, &aad, plain);
    let mut w = BytesWriter::new();
    w.push_bytes(&aad);
    w.push_var_bytes(&sealed);
    w.finalize()
}

/// A snapshot's header with a plausible count: no more peers than bytes
/// for them (two each at least). See `wire::plausible`.
fn plausible_record(record: &[u8]) -> Result<(), BadRecord> {
    let mut r = BytesReader::new(record);
    if r.read_byte().map_err(BadRecord)? == 0x01 {
        crate::wire::plausible(&mut r, 2).map_err(BadRecord)?;
    }
    Ok(())
}

/// A record's header, read without a key: what the server knows of it.
pub fn header(record: &[u8]) -> Result<Header, BadRecord> {
    plausible_record(record)?;
    let parsed = parse_elo_record_header(record).map_err(BadRecord)?;
    Ok(match parsed.header {
        EloHeader::Delta(h) => Header::Delta { peer: peer_of(&h.peer_id)?, start: h.start, end: h.end, epoch: epoch_of(&h.key_id)? },
        EloHeader::Snapshot(h) => Header::Snapshot {
            // Back in peer order, as a doc's own version vector is.
            vv: {
                let mut vv = h.vv.iter().map(|(p, c)| Ok((peer_of(p)?, *c))).collect::<Result<Vec<_>, BadRecord>>()?;
                vv.sort_unstable();
                vv
            },
            epoch: epoch_of(&h.key_id)?,
        },
    })
}

/// A record of `room`, opened: its header and the changes in it.
/// A record opened under the key its header names, if the ring has it.
pub fn open(ring: &Ring, room: &str, record: &[u8]) -> Result<(Header, Vec<u8>), BadRecord> {
    plausible_record(record)?;
    let parsed = parse_elo_record_header(record).map_err(BadRecord)?;
    let (key_id, iv) = match &parsed.header {
        EloHeader::Delta(h) => (&h.key_id, h.iv),
        EloHeader::Snapshot(h) => (&h.key_id, h.iv),
    };
    let epoch = epoch_of(key_id)?;
    let key = ring.get(epoch).ok_or_else(|| BadRecord(format!("no key for epoch {epoch}")))?.doc_key(epoch, room);
    let plain = key.open(iv, parsed.aad, parsed.ct).map_err(|e| BadRecord(e.to_string()))?;
    Ok((header(record)?, plain))
}

/// The most a record's changes may weigh, sealed: past it the server
/// refuses a record, so a device cuts a span that big in pieces.
pub const MAX_PLAIN: usize = 192 * 1024;

/// The most one `DocUpdate` carries: records are sent in batches under it
/// (the Loro Protocol's own limit is 256 KiB a message).
pub const MAX_BATCH: usize = 240 * 1024;

/// Records in batches, none over [`MAX_BATCH`] unless one record alone is.
pub fn batches(records: Vec<Vec<u8>>) -> Vec<Vec<Vec<u8>>> {
    let mut out: Vec<Vec<Vec<u8>>> = Vec::new();
    let mut size = 0;
    for r in records {
        if out.is_empty() || size + r.len() > MAX_BATCH {
            out.push(Vec::new());
            size = 0;
        }
        size += r.len();
        out.last_mut().expect("pushed").push(r);
    }
    out
}

/// Records as one `DocUpdate`'s payload: a count, then each record.
pub fn container(records: &[impl AsRef<[u8]>]) -> Vec<u8> {
    let mut w = BytesWriter::new();
    w.push_uleb128(records.len() as u64);
    for r in records {
        w.push_var_bytes(r.as_ref());
    }
    w.finalize()
}

pub fn records(container: &[u8]) -> Result<Vec<&[u8]>, BadRecord> {
    crate::wire::plausible(&mut BytesReader::new(container), 1).map_err(BadRecord)?;
    decode_elo_container(container).map_err(BadRecord)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::tests::Seeded;

    #[test]
    fn a_record_says_what_it_holds_and_opens_in_its_room_only() {
        let mut rng = Seeded(7);
        let vault = VaultKey::generate(&mut rng);
        let h = Header::Delta { peer: 1234567890123, start: 3, end: 9, epoch: 0 };
        let r = seal(&vault, "note-a", &h, b"changes", &mut rng);
        let mut ring = Ring::new(vault.clone());
        assert_eq!(header(&r).unwrap(), h);
        assert_eq!(open(&ring, "note-a", &r).unwrap(), (h, b"changes".to_vec()));
        assert!(open(&ring, "note-b", &r).is_err(), "moved to another room, it does not open");
        assert!(open(&Ring::new(VaultKey::generate(&mut rng)), "note-a", &r).is_err());
        // The header is bound to the sealed bytes: change the span, and
        // it does not open.
        let mut bad = r.clone();
        let at = bad.iter().position(|&b| b == 3).unwrap();
        bad[at] = 2;
        assert!(open(&ring, "note-a", &bad).is_err());

        // Peers whose digits sort apart from their numbers (10 before 9 as
        // text): written as the Loro Protocol wants, read back in order.
        let s = Header::Snapshot { vv: vec![(9, 4), (10, 2), (99, 2)], epoch: 2 };
        let r2 = seal(&vault, "note-a", &s, b"whole", &mut rng);
        assert!(open(&ring, "note-a", &r2).is_err(), "a ring without epoch 2");
        ring.insert(2, vault.clone());
        assert_eq!(open(&ring, "note-a", &r2).unwrap().0, s);
        let c = container(&[&r, &r2]);
        assert_eq!(records(&c).unwrap(), vec![&r[..], &r2[..]]);
        assert!(!r.windows(7).any(|w| w == b"changes"), "the changes are not there in the clear");
        // Counts past what follows are refused before room is made for
        // them (found by fuzzing: these four bytes asked for 4 GB).
        assert!(records(&[0xff, 0xff, 0xff, 0x7a]).is_err());
        assert!(header(&[0x01, 0xff, 0xff, 0xff, 0x7a]).is_err());
    }
}
