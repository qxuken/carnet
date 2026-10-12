//! End-to-end encryption and device auth (docs/SYNC.md §7).
//!
//! Two keys, two questions:
//! - a device's **signing key** (Ed25519) answers "is this an enrolled
//!   device?" — it signs the server's login challenge;
//! - the **vault key** (256 bits) answers "can it read the notes?" — every
//!   record is sealed with a key derived from it, and it never leaves a
//!   device in the clear.
//!
//! Removing a device makes a new vault key (the next **epoch**), sent to
//! each device kept in a **sealed box** only it opens (X25519), signed by
//! the device that made it (`rekey.rs`), so the one removed reads nothing
//! written after. A device keeps every epoch's key,
//! its [`Ring`], so what was written before still reads. Each device's
//! **card** (its name and keys) is sealed under the vault key: the server
//! keeps it and cannot read it, nor make one up.
//!
//! Randomness comes in through [`Random`]: the OS's on a device, a seeded
//! one in the simulation, so a run of it can be played again exactly.

use std::collections::BTreeMap;
use std::fmt;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use curve25519_dalek::montgomery::MontgomeryPoint;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use sha2::Sha256;

/// Where random bytes come from.
pub trait Random {
    fn fill(&mut self, out: &mut [u8]);

    fn bytes<const N: usize>(&mut self) -> [u8; N]
    where
        Self: Sized,
    {
        let mut b = [0; N];
        self.fill(&mut b);
        b
    }
}

/// The operating system's randomness: what a device and the server use.
/// (The PWA brings WebCrypto's, through the same trait.)
#[cfg(not(target_arch = "wasm32"))]
pub struct OsRandom;

#[cfg(not(target_arch = "wasm32"))]
impl Random for OsRandom {
    fn fill(&mut self, out: &mut [u8]) {
        getrandom::fill(out).expect("the operating system gives random bytes");
    }
}

/// Sealing or opening failed: a wrong key, or bytes changed on the way.
#[derive(Debug, PartialEq, Eq)]
pub struct Sealed;

impl fmt::Display for Sealed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("does not open with this key, or was changed")
    }
}

impl std::error::Error for Sealed {}

/// Thirty-two bytes from HKDF-SHA256 of `ikm`, for `info`.
fn derive(ikm: &[u8], info: &[&[u8]]) -> [u8; 32] {
    let mut out = [0; 32];
    Hkdf::<Sha256>::new(None, ikm)
        .expand_multi_info(info, &mut out)
        .expect("32 bytes is a length HKDF-SHA256 gives");
    out
}

/// AES-256-GCM: `plain` sealed under `key` with `aad` bound to it.
fn seal(key: &[u8; 32], iv: [u8; 12], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("a 32-byte key");
    cipher
        .encrypt(&Nonce::from(iv), Payload { msg: plain, aad })
        .expect("sealing into memory does not fail")
}

fn open(key: &[u8; 32], iv: [u8; 12], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, Sealed> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("a 32-byte key");
    cipher.decrypt(&Nonce::from(iv), Payload { msg: sealed, aad }).map_err(|_| Sealed)
}

/// The key everything in a vault is sealed under, by way of the keys
/// derived from it. Made once, on the first device; then only ever moved
/// sealed (to a paired device, under the recovery code).
#[derive(Clone, PartialEq, Eq)]
pub struct VaultKey([u8; 32]);

impl fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VaultKey(…)")
    }
}

impl VaultKey {
    pub fn generate(rng: &mut impl Random) -> Self {
        VaultKey(rng.bytes())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        VaultKey(bytes)
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    /// The key for one doc (a room) at one epoch. Each doc its own, so a
    /// record moved by the server from one room to another does not open
    /// there; each epoch its own, so a device revoked does not read what
    /// comes after.
    pub fn doc_key(&self, epoch: u32, room: &str) -> DocKey {
        DocKey(derive(&self.0, &[b"carnet doc v1", &epoch.to_be_bytes(), room.as_bytes()]))
    }

    /// This key sealed under `wrapping` (a recovery code's, a pairing's),
    /// for keeping on the server or sending: IV, then sealed bytes.
    pub fn wrap(&self, wrapping: &[u8; 32], rng: &mut impl Random) -> Vec<u8> {
        let iv: [u8; 12] = rng.bytes();
        let mut out = iv.to_vec();
        out.extend(seal(wrapping, iv, b"carnet vault key v1", &self.0));
        out
    }

    pub fn unwrap(wrapped: &[u8], wrapping: &[u8; 32]) -> Result<Self, Sealed> {
        let (iv, sealed) = wrapped.split_at_checked(12).ok_or(Sealed)?;
        let plain = open(wrapping, iv.try_into().map_err(|_| Sealed)?, b"carnet vault key v1", sealed)?;
        Ok(VaultKey(plain.try_into().map_err(|_| Sealed)?))
    }
}

/// Every epoch's vault key a device has: records are sealed under the
/// newest, and opened under whichever their header names.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Ring(BTreeMap<u32, VaultKey>);

impl fmt::Debug for Ring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ring(epochs {:?})", self.0.keys().collect::<Vec<_>>())
    }
}

/// One epoch and its key, as a ring is written: 4 bytes and 32.
const RING_ENTRY: usize = 36;

impl Ring {
    /// A new account's ring: one key, epoch 0.
    pub fn new(vault: VaultKey) -> Self {
        Ring(BTreeMap::from([(0, vault)]))
    }

    /// `epoch`'s key, added. An epoch it has keeps the key it has: a key
    /// is never replaced (false).
    pub fn insert(&mut self, epoch: u32, key: VaultKey) -> bool {
        match self.0.entry(epoch) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(key);
                true
            }
            std::collections::btree_map::Entry::Occupied(_) => false,
        }
    }

    pub fn get(&self, epoch: u32) -> Option<&VaultKey> {
        self.0.get(&epoch)
    }

    /// The newest epoch and its key: what new records are sealed under.
    pub fn current(&self) -> (u32, &VaultKey) {
        self.0.iter().next_back().map(|(e, k)| (*e, k)).expect("a ring has a key")
    }

    pub fn epochs(&self) -> Vec<u32> {
        self.0.keys().copied().collect()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.iter().flat_map(|(e, k)| e.to_be_bytes().into_iter().chain(k.0)).collect()
    }

    /// `None` unless whole entries, at least one.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || !bytes.len().is_multiple_of(RING_ENTRY) {
            return None;
        }
        let ring = bytes
            .chunks(RING_ENTRY)
            .map(|c| (u32::from_be_bytes(c[..4].try_into().expect("4")), VaultKey(c[4..].try_into().expect("32"))))
            .collect();
        Some(Ring(ring))
    }

    /// Sealed under `wrapping` (a recovery code's, a pairing's): IV, then
    /// sealed bytes.
    pub fn wrap(&self, wrapping: &[u8; 32], rng: &mut impl Random) -> Vec<u8> {
        let iv: [u8; 12] = rng.bytes();
        let mut out = iv.to_vec();
        out.extend(seal(wrapping, iv, b"carnet ring v1", &self.to_bytes()));
        out
    }

    pub fn unwrap(wrapped: &[u8], wrapping: &[u8; 32]) -> Result<Self, Sealed> {
        let (iv, sealed) = wrapped.split_at_checked(12).ok_or(Sealed)?;
        let plain = open(wrapping, iv.try_into().map_err(|_| Sealed)?, b"carnet ring v1", sealed)?;
        Ring::from_bytes(&plain).ok_or(Sealed)
    }
}

/// The secret half of a key that sealed boxes are sent to (X25519). A
/// device's comes from its signing key, the recovery code's from the code:
/// nothing more to keep.
#[derive(Clone)]
pub struct BoxSecret([u8; 32]);

impl fmt::Debug for BoxSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BoxSecret(…)")
    }
}

/// The key for a box, from the shared secret and both public keys, so a
/// box is bound to the key it was sent to.
fn box_key(shared: &[u8; 32], from: &[u8; 32], to: &[u8; 32], label: &[u8]) -> Result<[u8; 32], Sealed> {
    // A public key of small order makes a shared secret of zeros: anyone
    // could compute it. Refused.
    if shared == &[0; 32] {
        return Err(Sealed);
    }
    Ok(derive(shared, &[b"carnet box v1", label, from, to]))
}

impl BoxSecret {
    pub fn public(&self) -> [u8; 32] {
        MontgomeryPoint::mul_base_clamped(self.0).to_bytes()
    }

    /// A box sealed to `public` by [`seal_to`], opened: its plain bytes.
    pub fn open(&self, label: &[u8], boxed: &[u8]) -> Result<Vec<u8>, Sealed> {
        let (from, rest) = boxed.split_at_checked(32).ok_or(Sealed)?;
        let (iv, sealed) = rest.split_at_checked(12).ok_or(Sealed)?;
        let from: [u8; 32] = from.try_into().map_err(|_| Sealed)?;
        let shared = MontgomeryPoint(from).mul_clamped(self.0).to_bytes();
        let key = box_key(&shared, &from, &self.public(), label)?;
        open(&key, iv.try_into().map_err(|_| Sealed)?, label, sealed)
    }
}

/// `plain` sealed so that only the holder of `to`'s secret opens it: a
/// key made for this box alone, agreed with `to`. Its public half, the IV,
/// then the sealed bytes. `label` says what it is, and is bound to it.
pub fn seal_to(to: &[u8; 32], label: &[u8], plain: &[u8], rng: &mut impl Random) -> Vec<u8> {
    let once = BoxSecret(rng.bytes());
    let from = once.public();
    let shared = MontgomeryPoint(*to).mul_clamped(once.0).to_bytes();
    // A key of small order: the box is sealed to nobody (it won't open).
    let key = box_key(&shared, &from, to, label).unwrap_or_else(|_| rng.bytes());
    let iv: [u8; 12] = rng.bytes();
    let mut out = from.to_vec();
    out.extend(iv);
    out.extend(seal(&key, iv, label, plain));
    out
}

/// A device as the account's other devices know it: its id, its keys, and
/// the name its owner sees. Sealed under the vault key, so the server keeps
/// it without reading the name, and cannot slip in a device of its own. The
/// recovery code has one too, with an empty id: what a new epoch's key is
/// sealed to for it; so do the password and each passkey, in slots of
/// their own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub device: Vec<u8>,
    /// Its signing key: the one the server logs it in with.
    pub public: [u8; 32],
    /// Its box key: what a new epoch's vault key is sealed to.
    pub boxed: [u8; 32],
    pub name: String,
    /// A passkey's credential id, as the browser knows it; empty for
    /// anything else.
    pub credential: Vec<u8>,
}

impl Card {
    fn key(vault: &VaultKey, epoch: u32) -> [u8; 32] {
        derive(&vault.0, &[b"carnet card v1", &epoch.to_be_bytes()])
    }

    /// Sealed under the ring's newest key: the epoch, the IV, the sealed
    /// bytes.
    pub fn seal(&self, ring: &Ring, rng: &mut impl Random) -> Vec<u8> {
        let (epoch, vault) = ring.current();
        let mut plain = Vec::new();
        // A fifth part only when there is one: a card without stays as
        // cards were before passkeys.
        let credential = (!self.credential.is_empty()).then_some(self.credential.as_slice());
        for part in [self.device.as_slice(), &self.public, &self.boxed, self.name.as_bytes()].into_iter().chain(credential) {
            plain.extend((part.len() as u32).to_be_bytes());
            plain.extend(part);
        }
        let iv: [u8; 12] = rng.bytes();
        let mut out = epoch.to_be_bytes().to_vec();
        out.extend(iv);
        out.extend(seal(&Card::key(vault, epoch), iv, b"carnet card v1", &plain));
        out
    }

    pub fn open(sealed: &[u8], ring: &Ring) -> Result<Card, Sealed> {
        let (epoch, rest) = sealed.split_at_checked(4).ok_or(Sealed)?;
        let (iv, sealed) = rest.split_at_checked(12).ok_or(Sealed)?;
        let vault = ring.get(u32::from_be_bytes(epoch.try_into().map_err(|_| Sealed)?)).ok_or(Sealed)?;
        let epoch = u32::from_be_bytes(epoch.try_into().map_err(|_| Sealed)?);
        let plain = open(&Card::key(vault, epoch), iv.try_into().map_err(|_| Sealed)?, b"carnet card v1", sealed)?;
        let mut parts = Vec::new();
        let mut rest = plain.as_slice();
        while !rest.is_empty() {
            let (n, r) = rest.split_at_checked(4).ok_or(Sealed)?;
            let n = u32::from_be_bytes(n.try_into().map_err(|_| Sealed)?) as usize;
            let (part, r) = r.split_at_checked(n).ok_or(Sealed)?;
            parts.push(part.to_vec());
            rest = r;
        }
        if parts.len() == 4 {
            parts.push(Vec::new());
        }
        let [device, public, boxed, name, credential] = <[Vec<u8>; 5]>::try_from(parts).map_err(|_| Sealed)?;
        Ok(Card {
            device,
            public: public.try_into().map_err(|_| Sealed)?,
            boxed: boxed.try_into().map_err(|_| Sealed)?,
            name: String::from_utf8(name).map_err(|_| Sealed)?,
            credential,
        })
    }
}

/// A picture's id: a key from the vault's first epoch over the picture's
/// hash, so the same picture is one blob in a vault, and the id says
/// nothing about it to anyone without the key. (The first epoch, not the
/// newest, so a picture keeps its id when keys rotate; a device removed
/// since could tell whether a picture it has is in the vault, no more.)
pub type BlobId = [u8; 32];

/// The most a picture may weigh.
pub const MAX_BLOB: usize = 32 * 1024 * 1024;

const BLOB_MAGIC: &[u8; 4] = b"CRNB";

/// What sealing adds to a picture: the magic, the epoch, the IV, the tag.
const BLOB_OVER: u64 = 4 + 4 + 12 + 16;

/// How long a picture of `plain` bytes is once sealed ([`seal_blob`]):
/// what a device says it will put, and the server holds it to.
pub fn sealed_len(plain: usize) -> u64 {
    plain as u64 + BLOB_OVER
}

pub fn blob_id(ring: &Ring, plain: &[u8]) -> BlobId {
    use sha2::Digest;
    let first = ring.0.values().next().expect("a ring has a key");
    let hash = Sha256::digest(plain);
    derive(&first.0, &[b"carnet blob id v1", hash.as_slice()])
}

/// A picture sealed for the server: the magic, the epoch it is sealed
/// under, an IV, and the sealed bytes, with the magic, epoch and id bound
/// in as associated data.
pub fn seal_blob(ring: &Ring, id: &BlobId, plain: &[u8], rng: &mut impl Random) -> Vec<u8> {
    let (epoch, vault) = ring.current();
    let key = derive(&vault.0, &[b"carnet blob v1", &epoch.to_be_bytes(), id]);
    let iv: [u8; 12] = rng.bytes();
    let mut head = BLOB_MAGIC.to_vec();
    head.extend(epoch.to_be_bytes());
    let aad = [head.as_slice(), id].concat();
    let mut out = head;
    out.extend(iv);
    out.extend(seal(&key, iv, &aad, plain));
    out
}

/// A picture as [`seal_blob`] made it, opened: and only if it is the
/// picture the id names.
pub fn open_blob(ring: &Ring, id: &BlobId, sealed: &[u8]) -> Result<Vec<u8>, Sealed> {
    let rest = sealed.strip_prefix(BLOB_MAGIC.as_slice()).ok_or(Sealed)?;
    let (epoch, rest) = rest.split_at_checked(4).ok_or(Sealed)?;
    let (iv, ct) = rest.split_at_checked(12).ok_or(Sealed)?;
    let epoch_n = u32::from_be_bytes(epoch.try_into().map_err(|_| Sealed)?);
    let vault = ring.get(epoch_n).ok_or(Sealed)?;
    let key = derive(&vault.0, &[b"carnet blob v1", epoch, id]);
    let aad = [BLOB_MAGIC.as_slice(), epoch, id].concat();
    let plain = open(&key, iv.try_into().map_err(|_| Sealed)?, &aad, ct)?;
    if &blob_id(ring, &plain) != id {
        return Err(Sealed);
    }
    Ok(plain)
}

/// One doc's key at one epoch.
#[derive(Clone, PartialEq, Eq)]
pub struct DocKey([u8; 32]);

impl fmt::Debug for DocKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DocKey(…)")
    }
}

impl DocKey {
    /// `plain` sealed with a fresh random IV, `aad` (the record's plaintext
    /// header) bound to it: the server can read the header, and cannot
    /// change it unnoticed.
    pub fn seal(&self, iv: [u8; 12], aad: &[u8], plain: &[u8]) -> Vec<u8> {
        seal(&self.0, iv, aad, plain)
    }

    pub fn open(&self, iv: [u8; 12], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, Sealed> {
        open(&self.0, iv, aad, sealed)
    }
}

/// A device's signing key: it proves the device is one of the account's
/// (by signing the login challenge, §7), and, later, vouches for a device
/// it pairs.
#[derive(Clone)]
pub struct DeviceKey(SigningKey);

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceKey({:02x?}…)", &self.public()[..4])
    }
}

impl DeviceKey {
    pub fn generate(rng: &mut impl Random) -> Self {
        DeviceKey(SigningKey::from_bytes(&rng.bytes()))
    }

    pub fn from_secret(secret: [u8; 32]) -> Self {
        DeviceKey(SigningKey::from_bytes(&secret))
    }

    /// The secret, for the OS's keychain.
    pub fn secret(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// What the server keeps.
    pub fn public(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.0.sign(message).to_bytes()
    }

    /// The device's box key, from its signing key: what a new epoch's
    /// vault key is sealed to for it.
    pub fn box_secret(&self) -> BoxSecret {
        BoxSecret(derive(&self.0.to_bytes(), &[b"carnet device box v1"]))
    }
}

/// Whether `signature` is `public`'s over `message`. Strict: a signature
/// another encoding of which would also pass is refused.
pub fn verify(public: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    VerifyingKey::from_bytes(public).is_ok_and(|k| k.verify_strict(message, &Signature::from_bytes(signature)).is_ok())
}

/// What a device signs to log in (§7): a fixed prefix, so the signature is
/// good for nothing else the key signs; the server's origin, so another
/// server cannot pass a real one's challenge on; who claims to log in; and
/// the server's nonce, used once.
pub fn login_message(origin: &str, account: &[u8], device: &[u8], nonce: &[u8; 32]) -> Vec<u8> {
    let mut m = b"carnet-auth-v1".to_vec();
    for part in [origin.as_bytes(), account, device] {
        m.extend((part.len() as u32).to_be_bytes());
        m.extend(part);
    }
    m.extend(nonce);
    m
}

/// What the recovery key signs to let a new device in: like a login, for
/// this server and this nonce, and naming the device it lets in, so the
/// signature lets in that device and no other.
pub fn recovery_message(origin: &str, recovery: &[u8; 32], device: &[u8], device_public: &[u8; 32], nonce: &[u8; 32]) -> Vec<u8> {
    let mut m = b"carnet-recover-v1".to_vec();
    for part in [origin.as_bytes(), recovery.as_slice(), device, device_public.as_slice()] {
        m.extend((part.len() as u32).to_be_bytes());
        m.extend(part);
    }
    m.extend(nonce);
    m
}

/// The recovery code: 128 random bits, written as 12 BIP-39 words (the
/// 4 bits past 128 are a checksum that catches most typos). One secret,
/// not shares (§7). Two keys come from it: one that wraps a copy of the
/// vault key kept on the server, one that signs in when every device is
/// gone. No slow hash: 128 random bits cannot be guessed.
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryCode([u8; 16]);

impl fmt::Debug for RecoveryCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryCode(…)")
    }
}

impl RecoveryCode {
    pub fn generate(rng: &mut impl Random) -> Self {
        RecoveryCode(rng.bytes())
    }

    /// The 12 words, lowercase, one space apart.
    pub fn words(&self) -> String {
        let m = bip39::Mnemonic::from_entropy(&self.0).expect("16 bytes is a BIP-39 length");
        m.words().collect::<Vec<_>>().join(" ")
    }

    /// The code from its words as someone typed them: any case, any
    /// spacing. `None` for a word not on the list, or a checksum that
    /// does not match.
    pub fn parse(typed: &str) -> Option<Self> {
        let words = typed.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>().join(" ");
        let m = bip39::Mnemonic::parse_normalized(&words).ok()?;
        let (bytes, len) = m.to_entropy_array();
        (len == 16).then(|| RecoveryCode(bytes[..16].try_into().expect("16")))
    }

    /// What is wrong with words [`parse`](RecoveryCode::parse) refused,
    /// in words a person can act on: how many there are, which is not on
    /// the list, or that they don't fit together.
    pub fn problem(typed: &str) -> String {
        let words: Vec<String> = typed.split_whitespace().map(str::to_lowercase).collect();
        if words.len() != 12 {
            return format!("That's {} words; the recovery code is twelve", words.len());
        }
        if let Some((i, w)) = words.iter().enumerate().find(|(_, w)| bip39::Language::English.find_word(w).is_none()) {
            return format!("Word {}, \u{201c}{w}\u{201d}, isn't one of the recovery words: check its spelling", i + 1);
        }
        "Each word is on the list, but together they aren't a recovery code: one may be another word from the list, or two may be swapped".into()
    }

    /// The secret an email and password come to (`password.rs`): one
    /// more way in, working as the words do, in a slot of its own.
    pub fn from_secret(secret: &[u8]) -> Self {
        RecoveryCode(derive(secret, &[b"carnet password slot v1"])[..16].try_into().expect("16"))
    }

    /// The secret a passkey's PRF output comes to (`passkey.rs`): working
    /// as the words do, in a slot of the passkey's own. Another label than
    /// the password's, so the two can never be one.
    pub fn from_passkey(prf: &[u8]) -> Self {
        RecoveryCode(derive(prf, &[b"carnet passkey slot v1"])[..16].try_into().expect("16"))
    }

    /// What wraps the server's copy of the vault key.
    pub fn wrapping_key(&self) -> [u8; 32] {
        derive(&self.0, &[b"carnet recovery wrap v1"])
    }

    /// What signs in with the code when no device is left.
    pub fn signing_key(&self) -> DeviceKey {
        DeviceKey::from_secret(derive(&self.0, &[b"carnet recovery sign v1"]))
    }

    /// What a new epoch's vault key is sealed to for the code, so the
    /// words still let a device in after one is removed.
    pub fn box_secret(&self) -> BoxSecret {
        BoxSecret(derive(&self.0, &[b"carnet recovery box v1"]))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// splitmix64: plenty for tests, and the same every run.
    pub struct Seeded(pub u64);

    impl Random for Seeded {
        fn fill(&mut self, out: &mut [u8]) {
            for b in out {
                self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
                *b = (z ^ (z >> 31)) as u8;
            }
        }
    }

    #[test]
    fn a_record_opens_with_its_doc_key_only() {
        let mut rng = Seeded(1);
        let vault = VaultKey::generate(&mut rng);
        let k = vault.doc_key(0, "room-a");
        let iv = rng.bytes();
        let sealed = k.seal(iv, b"header", b"- [ ] milk");
        assert_eq!(k.open(iv, b"header", &sealed).unwrap(), b"- [ ] milk");
        // Another room, another epoch, another header, a flipped bit: no.
        assert_eq!(vault.doc_key(0, "room-b").open(iv, b"header", &sealed), Err(Sealed));
        assert_eq!(vault.doc_key(1, "room-a").open(iv, b"header", &sealed), Err(Sealed));
        assert_eq!(k.open(iv, b"headex", &sealed), Err(Sealed));
        let mut bad = sealed.clone();
        bad[0] ^= 1;
        assert_eq!(k.open(iv, b"header", &bad), Err(Sealed));
    }

    #[test]
    fn a_box_opens_for_whom_it_was_sealed_to_only() {
        let mut rng = Seeded(4);
        let a = DeviceKey::generate(&mut rng).box_secret();
        let b = DeviceKey::generate(&mut rng).box_secret();
        let key = VaultKey::generate(&mut rng).to_bytes();
        let boxed = seal_to(&a.public(), b"carnet test v1", &key, &mut rng);
        assert_eq!(a.open(b"carnet test v1", &boxed), Ok(key.to_vec()));
        assert_eq!(b.open(b"carnet test v1", &boxed), Err(Sealed), "not for b");
        let mut bad = boxed.clone();
        bad[40] ^= 1;
        assert_eq!(a.open(b"carnet test v1", &bad), Err(Sealed));
        // Another label: not what was sealed.
        assert_eq!(a.open(b"carnet other v1", &boxed), Err(Sealed));
        // A key of small order (zero) gets a box nobody opens.
        let lost = seal_to(&[0; 32], b"carnet test v1", &key, &mut rng);
        assert!(a.open(b"carnet test v1", &lost).is_err());
        // The box key comes from the signing key: the same each time.
        let d = DeviceKey::generate(&mut rng);
        assert_eq!(d.box_secret().public(), DeviceKey::from_secret(d.secret()).box_secret().public());
    }

    #[test]
    fn a_ring_keeps_every_epoch_and_seals_under_the_newest() {
        let mut rng = Seeded(5);
        let (k0, k1) = (VaultKey::generate(&mut rng), VaultKey::generate(&mut rng));
        let mut ring = Ring::new(k0.clone());
        assert!(ring.insert(1, k1.clone()));
        assert_eq!(ring.current(), (1, &k1));
        // A key it has is never replaced.
        assert!(!ring.insert(1, k0.clone()));
        assert_eq!(ring.get(1), Some(&k1));
        assert_eq!(ring.get(0), Some(&k0));
        assert_eq!(Ring::from_bytes(&ring.to_bytes()), Some(ring.clone()));
        assert_eq!(Ring::from_bytes(&[1; 35]), None);
        let wrapping = rng.bytes();
        assert_eq!(Ring::unwrap(&ring.wrap(&wrapping, &mut rng), &wrapping), Ok(ring.clone()));
        assert!(Ring::unwrap(&ring.wrap(&wrapping, &mut rng), &rng.bytes()).is_err());

        // A card opens with a ring that has its epoch, and only then.
        let card = Card { device: b"dev".to_vec(), public: [1; 32], boxed: [2; 32], name: "Viktor's MacBook".into(), credential: Vec::new() };
        let sealed = card.seal(&ring, &mut rng);
        assert_eq!(Card::open(&sealed, &ring), Ok(card.clone()));
        // A passkey's, with its credential id.
        let passkey = Card { credential: vec![9; 20], name: "Passkey from Chrome on Mac".into(), ..card.clone() };
        assert_eq!(Card::open(&passkey.seal(&ring, &mut rng), &ring), Ok(passkey));
        assert_eq!(Card::open(&sealed, &Ring::new(k0)), Err(Sealed), "a ring without epoch 1");
        let mut bad = sealed;
        bad[20] ^= 1;
        assert_eq!(Card::open(&bad, &ring), Err(Sealed));
    }

    #[test]
    fn a_picture_opens_as_itself_only() {
        let mut rng = Seeded(9);
        let mut ring = Ring::new(VaultKey::generate(&mut rng));
        let picture = vec![7u8; 5000];
        let id = blob_id(&ring, &picture);
        assert_eq!(id, blob_id(&ring, &picture), "the same picture, the same id");
        assert_ne!(id, blob_id(&ring, &[7u8; 4999]));
        let sealed = seal_blob(&ring, &id, &picture, &mut rng);
        assert_eq!(sealed.len() as u64, sealed_len(picture.len()), "as long as the device says");
        assert!(!sealed.windows(64).any(|w| w == &picture[..64]), "not in the clear");
        assert_eq!(open_blob(&ring, &id, &sealed), Ok(picture.clone()));
        // Under another id (the server swapping blobs): no.
        let other = blob_id(&ring, b"other");
        assert_eq!(open_blob(&ring, &other, &sealed), Err(Sealed));
        let mut bad = sealed.clone();
        bad[30] ^= 1;
        assert_eq!(open_blob(&ring, &id, &bad), Err(Sealed));
        // Keys rotate: the id stays, and the old blob still opens.
        ring.insert(1, VaultKey::generate(&mut rng));
        assert_eq!(blob_id(&ring, &picture), id);
        assert_eq!(open_blob(&ring, &id, &sealed), Ok(picture.clone()));
        let newer = seal_blob(&ring, &id, &picture, &mut rng);
        assert_eq!(open_blob(&ring, &id, &newer), Ok(picture));
        // Another vault's: a different id.
        assert_ne!(blob_id(&Ring::new(VaultKey::generate(&mut rng)), b"x"), blob_id(&ring, b"x"));
    }

    #[test]
    fn a_login_signature_is_for_that_server_device_and_nonce() {
        let mut rng = Seeded(2);
        let d = DeviceKey::generate(&mut rng);
        let nonce = rng.bytes();
        let m = login_message("https://notes.example.com", b"acct", b"dev", &nonce);
        let sig = d.sign(&m);
        assert!(verify(&d.public(), &m, &sig));
        let other = login_message("https://evil.example.com", b"acct", b"dev", &nonce);
        assert!(!verify(&d.public(), &other, &sig));
        assert!(!verify(&DeviceKey::generate(&mut rng).public(), &m, &sig));
        assert_eq!(DeviceKey::from_secret(d.secret()).public(), d.public());
    }

    #[test]
    fn the_recovery_code_is_twelve_words_that_come_back() {
        let mut rng = Seeded(3);
        let code = RecoveryCode::generate(&mut rng);
        let words = code.words();
        assert_eq!(words.split(' ').count(), 12);
        let typed = format!("  {}  ", words.to_uppercase().replace(' ', "   "));
        assert_eq!(RecoveryCode::parse(&typed), Some(code.clone()));
        // A word swapped for another breaks the checksum, mostly; a word
        // off the list always does.
        let mut w: Vec<&str> = words.split(' ').collect();
        w[3] = "notaword";
        assert_eq!(RecoveryCode::parse(&w.join(" ")), None);
        // And what is wrong is said: the word, or the count.
        assert!(RecoveryCode::problem(&w.join(" ")).starts_with("Word 4, \u{201c}notaword\u{201d}"));
        assert!(RecoveryCode::problem(&w[..11].join(" ")).starts_with("That's 11 words"));
        let mut swapped: Vec<&str> = words.split(' ').collect();
        swapped.swap(0, 11);
        if RecoveryCode::parse(&swapped.join(" ")).is_none() {
            assert!(RecoveryCode::problem(&swapped.join(" ")).contains("swapped"));
        }

        let vault = VaultKey::generate(&mut rng);
        let wrapped = vault.wrap(&code.wrapping_key(), &mut rng);
        assert_eq!(VaultKey::unwrap(&wrapped, &code.wrapping_key()).unwrap(), vault);
        let other = RecoveryCode::generate(&mut rng);
        assert_eq!(VaultKey::unwrap(&wrapped, &other.wrapping_key()), Err(Sealed));
        assert_eq!(code.signing_key().public(), RecoveryCode::parse(&words).unwrap().signing_key().public());
    }
}
