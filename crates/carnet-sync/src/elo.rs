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

use loro_protocol::{BatchId, BytesReader, BytesWriter, CrdtType, ProtocolMessage};
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

/// The most a snapshot record may weigh, sealed: the whole doc, history
/// and all, as compaction writes it (§6). Past [`MAX_PLAIN`] it can't be a
/// delta's record, nor go in one message: it goes in fragments
/// ([`messages`]). A doc whose snapshot is past this isn't compacted, and
/// its owner is told.
pub const MAX_SNAPSHOT: usize = 32 << 20;

/// How much of an update goes in one of the Loro Protocol's fragments.
pub const FRAGMENT: usize = 192 * 1024;

/// What a message adds to its update, at most: the magic, the room id
/// ([`MAX_ROOM`] bytes at most, and its length), the type, the count and
/// the update's length, the batch id.
const MESSAGE_OVER: usize = 4 + 2 + MAX_ROOM + 1 + 2 * 10 + 8;

/// The longest room id a message is made for here: a note's is 32 hex
/// digits, the vault's `vault`; the server takes none past 64 bytes.
const MAX_ROOM: usize = 128;

/// One update for `room` (an `%ELO` container, [`container`]) as the
/// messages it goes in: one `DocUpdate` if it fits a message, else the
/// Loro Protocol's fragments: a `DocUpdateFragmentHeader` saying how many
/// and how long in all, then each `DocUpdateFragment` in order, all under
/// `batch_id`. Put back together ([`Pieces`]), they are the update.
pub fn messages(room: &str, update: Vec<u8>, batch_id: BatchId) -> Vec<ProtocolMessage> {
    if update.len() + MESSAGE_OVER <= loro_protocol::protocol::MAX_MESSAGE_SIZE {
        return vec![ProtocolMessage::DocUpdate { crdt: CrdtType::Elo, room_id: room.to_string(), updates: vec![update], batch_id }];
    }
    let mut out = vec![ProtocolMessage::DocUpdateFragmentHeader {
        crdt: CrdtType::Elo,
        room_id: room.to_string(),
        batch_id,
        fragment_count: update.len().div_ceil(FRAGMENT) as u64,
        total_size_bytes: update.len() as u64,
    }];
    for (index, fragment) in update.chunks(FRAGMENT).enumerate() {
        out.push(ProtocolMessage::DocUpdateFragment {
            crdt: CrdtType::Elo,
            room_id: room.to_string(),
            batch_id,
            index: index as u64,
            fragment: fragment.to_vec(),
        });
    }
    out
}

/// An update coming in fragments, put back together: what a
/// `DocUpdateFragmentHeader` promised, then each fragment in order.
#[derive(Debug)]
pub struct Pieces {
    pub room: String,
    pub batch_id: BatchId,
    count: u64,
    total: usize,
    next: u64,
    bytes: Vec<u8>,
}

/// The most an update in fragments may weigh: a snapshot at its largest,
/// in its container.
pub const MAX_PIECES: usize = MAX_SNAPSHOT + 16;

impl Pieces {
    /// What a header says is coming, if it can be: no more than
    /// [`MAX_PIECES`] bytes, in at least one fragment and no more
    /// fragments than bytes. Nothing is set aside for it until it comes.
    pub fn start(room: &str, batch_id: BatchId, count: u64, total: u64) -> Option<Pieces> {
        let total = usize::try_from(total).ok().filter(|&t| t <= MAX_PIECES)?;
        if count == 0 || count > total as u64 {
            return None;
        }
        Some(Pieces { room: room.to_string(), batch_id, count, total, next: 0, bytes: Vec::new() })
    }

    /// The next fragment: `Err` if it isn't the next of this update, or
    /// goes past what was said; `Ok(Some(update))` once the last has come,
    /// and is exactly as long as was said.
    pub fn add(&mut self, room: &str, batch_id: BatchId, index: u64, fragment: &[u8]) -> Result<Option<Vec<u8>>, BadRecord> {
        if room != self.room || batch_id != self.batch_id || index != self.next || self.bytes.len() + fragment.len() > self.total {
            return Err(BadRecord("a fragment out of place".into()));
        }
        self.bytes.extend_from_slice(fragment);
        self.next += 1;
        if self.next < self.count {
            return Ok(None);
        }
        if self.bytes.len() != self.total {
            return Err(BadRecord("fragments short of what was said".into()));
        }
        Ok(Some(std::mem::take(&mut self.bytes)))
    }
}

/// Records in batches, none over [`MAX_BATCH`] unless one record alone is.
pub fn batches(records: Vec<Vec<u8>>) -> Vec<Vec<Vec<u8>>> {
    batches_under(records, MAX_BATCH)
}

/// Records in batches, none over `max` bytes unless one record alone is.
pub fn batches_under(records: Vec<Vec<u8>>, max: usize) -> Vec<Vec<Vec<u8>>> {
    let mut out: Vec<Vec<Vec<u8>>> = Vec::new();
    let mut size = 0;
    for r in records {
        if out.is_empty() || size + r.len() > max {
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

    /// An update too big for a message goes in fragments, each one a
    /// message, and comes back whole; one that fits goes as it is. Out of
    /// order, short, or past what was said: refused.
    #[test]
    fn a_big_update_goes_in_fragments_and_comes_back_whole() {
        let room = "0000000000000000000000000000beef";
        let small = vec![7; MAX_BATCH];
        assert!(matches!(&messages(room, small.clone(), BatchId([1; 8]))[..], [ProtocolMessage::DocUpdate { updates, .. }] if updates[0] == small));
        let big: Vec<u8> = (0..FRAGMENT * 3 + 5).map(|i| i as u8).collect();
        let sent = messages(room, big.clone(), BatchId([2; 8]));
        assert_eq!(sent.len(), 5);
        for m in &sent {
            assert!(loro_protocol::encode(m).is_ok(), "each fits a message");
        }
        let ProtocolMessage::DocUpdateFragmentHeader { fragment_count, total_size_bytes, .. } = sent[0] else { panic!() };
        let fragments: Vec<(u64, Vec<u8>)> = sent[1..]
            .iter()
            .map(|m| match m {
                ProtocolMessage::DocUpdateFragment { index, fragment, .. } => (*index, fragment.clone()),
                _ => panic!(),
            })
            .collect();
        let mut p = Pieces::start(room, BatchId([2; 8]), fragment_count, total_size_bytes).unwrap();
        let mut whole = None;
        for (i, f) in &fragments {
            whole = p.add(room, BatchId([2; 8]), *i, f).unwrap();
        }
        assert_eq!(whole, Some(big));

        let mut p = Pieces::start(room, BatchId([2; 8]), fragment_count, total_size_bytes).unwrap();
        assert!(p.add(room, BatchId([2; 8]), 1, &fragments[1].1).is_err(), "out of order");
        let mut p = Pieces::start(room, BatchId([2; 8]), 1, 10).unwrap();
        assert!(p.add(room, BatchId([2; 8]), 0, &[0; 11]).is_err(), "past what was said");
        let mut p = Pieces::start(room, BatchId([2; 8]), 1, 10).unwrap();
        assert!(p.add(room, BatchId([2; 8]), 0, &[0; 9]).is_err(), "short of it");
        assert!(Pieces::start(room, BatchId([2; 8]), 1, MAX_PIECES as u64 + 1).is_none());
        assert!(Pieces::start(room, BatchId([2; 8]), 0, 10).is_none());
        assert!(Pieces::start(room, BatchId([2; 8]), 11, 10).is_none());
    }
}
