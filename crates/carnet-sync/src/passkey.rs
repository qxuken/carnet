//! Passkeys (docs/SYNC.md §7), on the web: one more way in, beside a code
//! from another device, the recovery words and a password.
//!
//! A passkey made in the browser with WebAuthn's PRF extension gives, each
//! time it's used, the same 32 secret bytes for the same salt ([`salt`]):
//! the passkey's own, made inside the authenticator, never seen by the
//! server. From them comes a secret that works as the recovery words do
//! ([`RecoveryCode::from_passkey`]): it wraps a copy of the vault key kept
//! on the server, signs the recovery that lets a new device in, and is
//! what a new epoch's key is sealed to when a device is removed. Each
//! passkey has a slot of its own on the server ([`slot`]), found by the
//! slot's signing key.
//!
//! The server never checks a WebAuthn assertion: what proves the passkey
//! is the signature made with what its PRF output comes to, as a
//! recovery's is. The passkey's user id is the account's, so signing in
//! needs nothing typed; the browser asks which passkey.

use sha2::{Digest, Sha256};

use crate::crypto::RecoveryCode;

/// The most passkeys one account has.
pub const MAX: usize = 20;

/// The PRF output's length: what a passkey gives, and what's taken.
pub const PRF_LEN: usize = 32;

/// The longest name a passkey keeps, in letters.
pub const NAME_LEN: usize = 64;

/// What the browser asks the passkey's PRF for, the same every time:
/// SHA-256 of "carnet passkey v1".
pub fn salt() -> [u8; 32] {
    Sha256::digest(b"carnet passkey v1").into()
}

/// What a passkey's PRF output comes to: the slot's secret.
pub fn secret(prf: &[u8]) -> RecoveryCode {
    RecoveryCode::from_passkey(prf)
}

/// The id a passkey's slot has where a device's would (a new epoch's keys
/// and cards are sent by it): 65 bytes, so never a device's (at most 64),
/// starting 0xfe, so never the password's ([`crate::password::SLOT`]), and
/// ending in the slot's signing key, so each passkey's is its own.
pub fn slot(public: &[u8; 32]) -> Vec<u8> {
    let mut id = vec![0xfe; 33];
    id.extend(public);
    id
}

/// The signing key a passkey's slot id ends in, if `id` is one.
pub fn of_slot(id: &[u8]) -> Option<[u8; 32]> {
    (id.len() == 65 && id[..33].iter().all(|&b| b == 0xfe)).then(|| id[33..].try_into().expect("32"))
}

/// A passkey's name as it's kept: spaces made one, at most [`NAME_LEN`]
/// letters.
pub fn name(typed: &str) -> String {
    typed.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(NAME_LEN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_passkey_slot_is_never_a_device_nor_the_password() {
        let id = slot(&[7; 32]);
        assert_eq!(id.len(), 65, "past a device id's 64");
        assert_ne!(id, crate::password::SLOT);
        assert_eq!(of_slot(&id), Some([7; 32]));
        assert_eq!(of_slot(crate::password::SLOT), None);
        assert_eq!(of_slot(&[0xfe; 64]), None);
        assert_ne!(slot(&[8; 32]), id);
    }

    /// The same PRF output, the same secret; as a password's, never.
    #[test]
    fn a_prf_output_comes_to_its_own_secret() {
        let prf = [3; 32];
        assert_eq!(secret(&prf).signing_key().public(), secret(&prf).signing_key().public());
        assert_ne!(secret(&prf).signing_key().public(), secret(&[4; 32]).signing_key().public());
        assert_ne!(secret(&prf).signing_key().public(), RecoveryCode::from_secret(&prf).signing_key().public());
        // Fixed for good: another salt and every passkey made gives another
        // secret, and opens nothing.
        assert_eq!(salt()[..4], [0x5c, 0x01, 0x9d, 0x83]);
        assert_eq!(name("  Passkey   from\tChrome "), "Passkey from Chrome");
        assert_eq!(name(&"x".repeat(100)).len(), NAME_LEN);
    }
}
