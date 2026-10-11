//! An account as rows, whichever store keeps it: what `migrate` reads
//! from the SQLite files, writes to Postgres, and reads back from both to
//! compare. Each list is in one order on every side (by id, by seq, as
//! logged), so two stores that hold the same have equal rows, and equal
//! [`AccountRows::sum`]s.

use sha2::{Digest, Sha256};

use crate::storage::Logged;

/// A device's row: [`Device`](crate::storage::Device), with its id and
/// when it was enrolled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: Vec<u8>,
    pub public: Vec<u8>,
    pub revoked: bool,
    pub created: u64,
    pub card: Vec<u8>,
    pub seen: u64,
}

/// A key epoch's key sealed to one device (empty: the recovery code).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RekeyRow {
    pub device: Vec<u8>,
    pub epoch: u32,
    pub sealed: Vec<u8>,
}

/// A delta record, live (`shelved_by` 0) or set aside by that base.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecordRow {
    pub seq: u64,
    pub room: String,
    pub peer: u64,
    pub start: u64,
    pub end: u64,
    pub bytes: Vec<u8>,
    pub shelved_by: u64,
    pub shelved_at: u64,
}

/// A base, live or set aside; `vv` as its header encodes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseRow {
    pub seq: u64,
    pub room: String,
    pub vv: Vec<u8>,
    pub bytes: Vec<u8>,
    pub by: Vec<u8>,
    pub at: u64,
    pub verified: bool,
    pub undoable: bool,
    pub shelved_by: u64,
    pub shelved_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryRow {
    pub public: Vec<u8>,
    pub wrapped: Vec<u8>,
    pub created: u64,
    pub card: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PasswordRow {
    pub id: String,
    pub record: Vec<u8>,
    pub public: Vec<u8>,
    pub wrapped: Vec<u8>,
    pub card: Vec<u8>,
}

/// A passkey's slot, named by its signing key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PasskeyRow {
    pub public: Vec<u8>,
    pub wrapped: Vec<u8>,
    pub card: Vec<u8>,
    pub made: u64,
}

/// Everything one account keeps, but its pictures.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountRows {
    pub id: Vec<u8>,
    pub owner: bool,
    pub created: u64,
    pub seq: u64,
    pub epoch: u32,
    /// By id.
    pub devices: Vec<DeviceRow>,
    /// By device, then epoch.
    pub rekeys: Vec<RekeyRow>,
    /// By seq.
    pub records: Vec<RecordRow>,
    /// By seq.
    pub bases: Vec<BaseRow>,
    /// Rooms removed, and when; by room.
    pub removed: Vec<(String, u64)>,
    /// The oldest first.
    pub log: Vec<Logged>,
    pub recovery: Option<RecoveryRow>,
    pub password: Option<PasswordRow>,
    /// The oldest first.
    pub passkeys: Vec<PasskeyRow>,
}

/// An invite not used yet: its code's hash, whether it's the owner's,
/// when it stops being good.
pub type InviteRow = ([u8; 32], bool, u64);

/// Bytes fed to a hash, each piece with its length first: no two
/// different rows read the same.
struct Fed(Sha256);

impl Fed {
    fn n(&mut self, n: u64) {
        self.0.update(n.to_be_bytes());
    }
    fn b(&mut self, b: &[u8]) {
        self.n(b.len() as u64);
        self.0.update(b);
    }
}

impl AccountRows {
    /// SHA-256 over every row, in order: equal for equal rows.
    pub fn sum(&self) -> [u8; 32] {
        let mut f = Fed(Sha256::new());
        f.b(&self.id);
        f.n(u64::from(self.owner));
        f.n(self.created);
        f.n(self.seq);
        f.n(u64::from(self.epoch));
        f.n(self.devices.len() as u64);
        for d in &self.devices {
            f.b(&d.id);
            f.b(&d.public);
            f.n(u64::from(d.revoked));
            f.n(d.created);
            f.b(&d.card);
            f.n(d.seen);
        }
        f.n(self.rekeys.len() as u64);
        for k in &self.rekeys {
            f.b(&k.device);
            f.n(u64::from(k.epoch));
            f.b(&k.sealed);
        }
        f.n(self.records.len() as u64);
        for r in &self.records {
            f.n(r.seq);
            f.b(r.room.as_bytes());
            f.n(r.peer);
            f.n(r.start);
            f.n(r.end);
            f.b(&r.bytes);
            f.n(r.shelved_by);
            f.n(r.shelved_at);
        }
        f.n(self.bases.len() as u64);
        for b in &self.bases {
            f.n(b.seq);
            f.b(b.room.as_bytes());
            f.b(&b.vv);
            f.b(&b.bytes);
            f.b(&b.by);
            f.n(b.at);
            f.n(u64::from(b.verified));
            f.n(u64::from(b.undoable));
            f.n(b.shelved_by);
            f.n(b.shelved_at);
        }
        f.n(self.removed.len() as u64);
        for (room, at) in &self.removed {
            f.b(room.as_bytes());
            f.n(*at);
        }
        f.n(self.log.len() as u64);
        for l in &self.log {
            f.n(l.at);
            f.b(&l.device);
            f.b(l.room.as_bytes());
            f.b(l.what.as_bytes());
        }
        match &self.recovery {
            Some(r) => {
                f.n(1);
                f.b(&r.public);
                f.b(&r.wrapped);
                f.n(r.created);
                f.b(&r.card);
            }
            None => f.n(0),
        }
        match &self.password {
            Some(p) => {
                f.n(1);
                f.b(p.id.as_bytes());
                f.b(&p.record);
                f.b(&p.public);
                f.b(&p.wrapped);
                f.b(&p.card);
            }
            None => f.n(0),
        }
        f.n(self.passkeys.len() as u64);
        for k in &self.passkeys {
            f.b(&k.public);
            f.b(&k.wrapped);
            f.b(&k.card);
            f.n(k.made);
        }
        f.0.finalize().into()
    }

    /// How many of each: what a migration says it copied.
    pub fn counts(&self) -> Counts {
        Counts {
            accounts: 1,
            devices: self.devices.len() as u64,
            rekeys: self.rekeys.len() as u64,
            records: self.records.len() as u64,
            bases: self.bases.len() as u64,
            removed: self.removed.len() as u64,
            log: self.log.len() as u64,
            recovery: u64::from(self.recovery.is_some()),
            passwords: u64::from(self.password.is_some()),
            passkeys: self.passkeys.len() as u64,
            bytes: self.records.iter().map(|r| r.bytes.len() as u64).sum::<u64>() + self.bases.iter().map(|b| b.bytes.len() as u64).sum::<u64>(),
        }
    }
}

/// Rows of each kind, added up over accounts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub accounts: u64,
    pub devices: u64,
    pub rekeys: u64,
    pub records: u64,
    pub bases: u64,
    pub removed: u64,
    pub log: u64,
    pub recovery: u64,
    pub passwords: u64,
    pub passkeys: u64,
    /// The sealed bytes of the records and bases.
    pub bytes: u64,
}

impl std::ops::AddAssign for Counts {
    fn add_assign(&mut self, o: Counts) {
        self.accounts += o.accounts;
        self.devices += o.devices;
        self.rekeys += o.rekeys;
        self.records += o.records;
        self.bases += o.bases;
        self.removed += o.removed;
        self.log += o.log;
        self.recovery += o.recovery;
        self.passwords += o.passwords;
        self.passkeys += o.passkeys;
        self.bytes += o.bytes;
    }
}

impl std::fmt::Display for Counts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} accounts, {} devices, {} keys sealed to them, {} records and {} bases ({} bytes), {} rooms removed, {} log entries, {} recovery codes, {} passwords, {} passkeys",
            self.accounts, self.devices, self.rekeys, self.records, self.bases, self.bytes, self.removed, self.log, self.recovery, self.passwords, self.passkeys
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_rows_sum_equal_and_any_change_shows() {
        let a = AccountRows {
            id: b"acct".to_vec(),
            records: vec![RecordRow { seq: 1, room: "r".into(), peer: 1, start: 0, end: 3, bytes: b"x".to_vec(), ..Default::default() }],
            log: vec![Logged { at: 1, device: b"d".to_vec(), room: "r".into(), what: "w".into() }],
            ..Default::default()
        };
        assert_eq!(a.sum(), a.clone().sum());
        let mut b = a.clone();
        b.records[0].shelved_at = 9;
        assert_ne!(a.sum(), b.sum());
        // A byte moved between two fields is still a change.
        let mut c = a.clone();
        c.log[0].room = "rw".into();
        c.log[0].what = String::new();
        assert_ne!(a.sum(), c.sum());
        assert_eq!(a.counts().records, 1);
    }
}
