//! A new epoch's vault key, from the device that made it (docs/SYNC.md §7,
//! "Devices, removal and key epochs").
//!
//! A device removing another makes the next epoch's key and sends it,
//! through the server, to each device and slot kept, in a box only that
//! one opens. A box alone says nothing of who sealed it, and every device
//! ever in the account has every box key (they're on the cards). So a
//! device removed, with the server's help, could seal a key of its own to
//! each device kept, and read all they write after. Each key therefore
//! goes signed by the device that made it, with its card:
//!
//! ```text
//! signed:  "carnet-rekey-v2" ‖ account ‖ signer's id ‖ box key it's for
//!          ‖ the epoch it follows ‖ its epoch ‖ the box        (each length-prefixed)
//! sent:    signer's id ‖ its card, under the epoch it follows ‖ the box ‖ the signature
//! ```
//!
//! The server keeps and relays what's sent as it did the box alone: it
//! reads none of it. A device takes the key ([`take`]) only if:
//!
//! - the box opens for it, and says the epoch signed;
//! - it has no key for that epoch yet: it never replaces one;
//! - the card opens under the newest key the device has, names the signer,
//!   a device (not a slot), and the signature is that card's key's. So the
//!   signer was in the account at that epoch; a device removed before it
//!   never was, and has no key to seal such a card under;
//! - the new epoch is the one right after that, so it's past every one the
//!   device has: a key can't skip ahead of the one it should follow.
//!
//! One that follows an epoch the device has no key for yet (it was away
//! across two removals, and the later came first) is held, and tried
//! again once it has ([`Took::Later`], [`take_all`]).

use crate::crypto::{BoxSecret, Card, DeviceKey, Random, Ring, VaultKey, seal_to, verify};

/// What the box is: bound to it.
const BOX: &[u8] = b"carnet rekey v2";
/// What the signature is over, first: so it's good for nothing else the
/// key signs (a login, `carnet-auth-v1`).
const SIGNED: &[u8] = b"carnet-rekey-v2";
/// A device's id is at most this long (the server holds it so); a slot's
/// (the password's, a passkey's) is longer, and the recovery words' empty.
const MAX_DEVICE_ID: usize = 64;
/// Held at most, waiting for the key they follow: past it the oldest go.
/// The server sends them all again at the next login.
pub const MAX_HELD: usize = 64;

/// The device making a new epoch, as it signs the key it sends each device
/// and slot kept.
pub struct Signer<'a> {
    account: &'a [u8],
    device: &'a [u8],
    key: &'a DeviceKey,
    /// The newest epoch it has: the one the new key follows.
    after: u32,
    /// Its card under that epoch's key, without its name: what shows the
    /// device taking the key that it was in the account then.
    card: Vec<u8>,
}

impl<'a> Signer<'a> {
    /// `device`, with `key`, in `account`; `ring` as it is before the new
    /// epoch.
    pub fn new(account: &'a [u8], device: &'a [u8], key: &'a DeviceKey, ring: &Ring, rng: &mut impl Random) -> Self {
        let card = Card { device: device.to_vec(), public: key.public(), boxed: key.box_secret().public(), name: String::new(), credential: Vec::new() }
            .seal(ring, rng);
        Signer { account, device, key, after: ring.current().0, card }
    }

    /// `vault`, `epoch`'s key, sealed to the box key `to` and signed.
    pub fn seal(&self, to: &[u8; 32], epoch: u32, vault: &VaultKey, rng: &mut impl Random) -> Vec<u8> {
        let mut plain = epoch.to_be_bytes().to_vec();
        plain.extend(vault.to_bytes());
        let boxed = seal_to(to, BOX, &plain, rng);
        let signature = self.key.sign(&message(self.account, self.device, to, self.after, epoch, &boxed));
        join(&[self.device, &self.card, &boxed, &signature])
    }
}

/// What a [`Signer`] signs.
fn message(account: &[u8], signer: &[u8], to: &[u8; 32], after: u32, epoch: u32, boxed: &[u8]) -> Vec<u8> {
    join(&[SIGNED, account, signer, to, &after.to_be_bytes(), &epoch.to_be_bytes(), boxed])
}

/// Each part, its length first.
fn join(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend((p.len() as u32).to_be_bytes());
        out.extend(*p);
    }
    out
}

/// `N` parts as [`join`] wrote them, and nothing after.
fn split<const N: usize>(mut bytes: &[u8]) -> Option<[&[u8]; N]> {
    let mut parts = [&[][..]; N];
    for part in &mut parts {
        let (n, rest) = bytes.split_at_checked(4)?;
        let n = u32::from_be_bytes(n.try_into().ok()?) as usize;
        (*part, bytes) = rest.split_at_checked(n)?;
    }
    bytes.is_empty().then_some(parts)
}

/// What became of a key sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Took {
    /// Taken in: the ring has this epoch now.
    Taken(u32),
    /// The ring has it already, the same key: nothing to do.
    Had,
    /// It follows an epoch the ring has no key for yet: to try again once
    /// it has.
    Later,
    /// Not from a device in the account at the ring's newest epoch, not
    /// the epoch right after it, another key for an epoch the ring has, or
    /// not one at all: left out.
    Refused,
}

/// `sent` (a [`Signer::seal`]) for the holder of `to`, in `account`: taken
/// into `ring` if it's to be (see the module).
pub fn take(ring: &mut Ring, account: &[u8], to: &BoxSecret, sent: &[u8]) -> Took {
    let Some([signer, card, boxed, signature]) = split::<4>(sent) else { return Took::Refused };
    let Ok(plain) = to.open(BOX, boxed) else { return Took::Refused };
    let Ok(plain) = <[u8; 36]>::try_from(plain.as_slice()) else { return Took::Refused };
    let epoch = u32::from_be_bytes(plain[..4].try_into().expect("4"));
    let vault = VaultKey::from_bytes(plain[4..].try_into().expect("32"));
    match ring.get(epoch) {
        Some(k) if *k == vault => return Took::Had,
        // Never another key for an epoch the ring has.
        Some(_) => return Took::Refused,
        None => {}
    }
    // The epoch its card is sealed under, the one it follows: in the clear,
    // ahead of the card (`Card::seal`).
    let Some(after) = card.get(..4).map(|b| u32::from_be_bytes(b.try_into().expect("4"))) else { return Took::Refused };
    // The epoch right after it, and no other (epochs go up one at a time,
    // as the server holds them): a key can't skip ahead of the one between.
    if after.checked_add(1) != Some(epoch) {
        return Took::Refused;
    }
    let (newest, _) = ring.current();
    if after > newest {
        return Took::Later;
    }
    // From a device in the account at this ring's newest epoch, or nothing:
    // a card under an older one is any device's that was in the account
    // then, the one removed too.
    if after != newest {
        return Took::Refused;
    }
    let Ok(c) = Card::open(card, ring) else { return Took::Refused };
    if c.device != signer || signer.is_empty() || signer.len() > MAX_DEVICE_ID {
        return Took::Refused;
    }
    let Ok(signature) = <[u8; 64]>::try_from(signature) else { return Took::Refused };
    if !verify(&c.public, &message(account, signer, &to.public(), after, epoch, boxed), &signature) {
        return Took::Refused;
    }
    ring.insert(epoch, vault);
    Took::Taken(epoch)
}

/// Each of `pending` taken that can be, in whatever order they came: one
/// that follows another's epoch is taken after it. Left in `pending`: those
/// that follow a key not come yet. The epochs taken, in order.
pub fn take_all(ring: &mut Ring, account: &[u8], to: &BoxSecret, pending: &mut Vec<Vec<u8>>) -> Vec<u32> {
    let mut taken = Vec::new();
    loop {
        let before = taken.len();
        pending.retain(|sent| match take(ring, account, to, sent) {
            Took::Taken(epoch) => {
                taken.push(epoch);
                false
            }
            Took::Later => true,
            Took::Had | Took::Refused => false,
        });
        if taken.len() == before || pending.is_empty() {
            return taken;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::tests::Seeded;

    const ACCOUNT: &[u8] = b"acct";

    struct Device {
        id: Vec<u8>,
        key: DeviceKey,
    }

    fn device(id: &[u8], rng: &mut Seeded) -> Device {
        Device { id: id.to_vec(), key: DeviceKey::generate(rng) }
    }

    /// `by`, with `ring`, sends `to` the key for `epoch`.
    fn sent(by: &Device, ring: &Ring, to: &Device, epoch: u32, vault: &VaultKey, rng: &mut Seeded) -> Vec<u8> {
        Signer::new(ACCOUNT, &by.id, &by.key, ring, rng).seal(&to.key.box_secret().public(), epoch, vault, rng)
    }

    #[test]
    fn a_key_is_taken_from_a_device_in_the_account_at_the_newest_epoch_only() {
        let mut rng = Seeded(11);
        let (a, b, gone) = (device(b"a", &mut rng), device(b"b", &mut rng), device(b"gone", &mut rng));
        let ring0 = Ring::new(VaultKey::generate(&mut rng));
        let k1 = VaultKey::generate(&mut rng);
        let honest = sent(&a, &ring0, &b, 1, &k1, &mut rng);

        // b takes it; again, it has it.
        let mut ring = ring0.clone();
        assert_eq!(take(&mut ring, ACCOUNT, &b.key.box_secret(), &honest), Took::Taken(1));
        assert_eq!(ring.current(), (1, &k1));
        assert_eq!(take(&mut ring, ACCOUNT, &b.key.box_secret(), &honest), Took::Had);
        // Not for a (the box), not for another account (the signature).
        assert_eq!(take(&mut ring0.clone(), ACCOUNT, &a.key.box_secret(), &honest), Took::Refused);
        assert_eq!(take(&mut ring0.clone(), b"other", &b.key.box_secret(), &honest), Took::Refused);

        // The device removed at 0 → 1 has key 0 only: whatever it sends b
        // after is refused, and b's ring stays as it is.
        let k2 = VaultKey::generate(&mut rng);
        for forged in [sent(&gone, &ring0, &b, 2, &k2, &mut rng), sent(&gone, &ring0, &b, 1, &k2, &mut rng)] {
            assert_eq!(take(&mut ring, ACCOUNT, &b.key.box_secret(), &forged), Took::Refused);
            assert_eq!(ring.epochs(), [0, 1]);
            assert_eq!(ring.current(), (1, &k1));
        }
        // Nor can it borrow a's card under epoch 1 (the server has it): a's
        // key didn't sign.
        let a_card = split::<4>(&sent(&a, &ring, &b, 2, &k2, &mut rng)).unwrap()[1].to_vec();
        let to = b.key.box_secret().public();
        let boxed = seal_to(&to, BOX, &[2u32.to_be_bytes().as_slice(), &k2.to_bytes()].concat(), &mut rng);
        let signature = gone.key.sign(&message(ACCOUNT, b"a", &to, 1, 2, &boxed));
        let borrowed = join(&[b"a", &a_card, &boxed, &signature]);
        assert_eq!(take(&mut ring, ACCOUNT, &b.key.box_secret(), &borrowed), Took::Refused);
        assert_eq!(ring.epochs(), [0, 1]);

        // One skipping an epoch, even a's: refused, not held.
        let ahead = sent(&a, &ring, &b, 3, &k2, &mut rng);
        assert_eq!(take(&mut ring.clone(), ACCOUNT, &b.key.box_secret(), &ahead), Took::Refused);
        // Another key for an epoch b has: refused, even from a.
        let other = sent(&a, &ring0, &b, 1, &k2, &mut rng);
        assert_eq!(take(&mut ring, ACCOUNT, &b.key.box_secret(), &other), Took::Refused);
        assert_eq!(ring.get(1), Some(&k1));
        // Bytes cut short, or one more: nothing.
        let next = sent(&a, &ring, &b, 2, &k2, &mut rng);
        assert_eq!(take(&mut ring.clone(), ACCOUNT, &b.key.box_secret(), &next[..next.len() - 1]), Took::Refused);
        assert_eq!(take(&mut ring.clone(), ACCOUNT, &b.key.box_secret(), &[next.as_slice(), &[0]].concat()), Took::Refused);
        // A slot doesn't sign: only a device.
        let slot = Device { id: crate::password::SLOT.to_vec(), key: DeviceKey::generate(&mut rng) };
        let by_slot = sent(&slot, &ring, &b, 2, &k2, &mut rng);
        assert_eq!(take(&mut ring, ACCOUNT, &b.key.box_secret(), &by_slot), Took::Refused);
        assert_eq!(ring.epochs(), [0, 1]);
        // And a's, the next: taken.
        assert_eq!(take(&mut ring, ACCOUNT, &b.key.box_secret(), &next), Took::Taken(2));
    }

    #[test]
    fn keys_come_in_any_order_and_are_taken_in_theirs() {
        let mut rng = Seeded(12);
        let (a, b) = (device(b"a", &mut rng), device(b"b", &mut rng));
        let mut ring = Ring::new(VaultKey::generate(&mut rng));
        let mut b_ring = ring.clone();
        let (k1, k2) = (VaultKey::generate(&mut rng), VaultKey::generate(&mut rng));
        let first = sent(&a, &ring, &b, 1, &k1, &mut rng);
        ring.insert(1, k1.clone());
        let second = sent(&a, &ring, &b, 2, &k2, &mut rng);

        assert_eq!(take(&mut b_ring, ACCOUNT, &b.key.box_secret(), &second), Took::Later);
        let mut pending = vec![second, first];
        assert_eq!(take_all(&mut b_ring, ACCOUNT, &b.key.box_secret(), &mut pending), [1, 2]);
        assert!(pending.is_empty());
        assert_eq!(b_ring.epochs(), [0, 1, 2]);
        assert_eq!(b_ring.current(), (2, &k2));
    }

    #[test]
    fn the_last_epoch_there_is_does_not_overflow() {
        let mut rng = Seeded(13);
        let (a, b) = (device(b"a", &mut rng), device(b"b", &mut rng));
        let mut ring = Ring::new(VaultKey::generate(&mut rng));
        ring.insert(u32::MAX, VaultKey::generate(&mut rng));
        // Nothing follows u32::MAX.
        let mut b_ring = Ring::new(VaultKey::generate(&mut rng));
        let late = sent(&a, &ring, &b, 7, &VaultKey::generate(&mut rng), &mut rng);
        assert_eq!(take(&mut b_ring, ACCOUNT, &b.key.box_secret(), &late), Took::Refused);
        let mut pending = vec![late];
        assert!(take_all(&mut b_ring, ACCOUNT, &b.key.box_secret(), &mut pending).is_empty());
        assert_eq!(b_ring.epochs(), [0]);
    }
}
