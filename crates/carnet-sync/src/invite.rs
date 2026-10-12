//! Invite codes (docs/SYNC.md §6): what makes a new account on a server
//! that does not take anyone who asks. 80 random bits, written as 16
//! Crockford base32 characters in fours (`K7QF-2M9D-XW4H-P3TA`): no I, L,
//! O or U, so a code read aloud or copied by hand comes back right, and
//! reading one forgives case, dashes, spaces, and I, L and O typed for 1
//! and 0. The server keeps only its hash, so a code read off the server
//! lets no one in; the code itself is kept in the account's vault
//! (`vault.rs`), sealed, for each of the account's devices to show again.

use sha2::{Digest, Sha256};

use crate::crypto::Random;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InviteCode([u8; 10]);

impl std::fmt::Debug for InviteCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InviteCode(…)")
    }
}

impl InviteCode {
    pub fn generate(rng: &mut impl Random) -> Self {
        InviteCode(rng.bytes())
    }

    /// The code as it is shown: four groups of four.
    pub fn text(&self) -> String {
        let bits = self.0.iter().fold(0u128, |acc, &b| (acc << 8) | u128::from(b));
        let chars: Vec<u8> = (0..16).rev().map(|i| ALPHABET[((bits >> (i * 5)) & 31) as usize]).collect();
        chars.chunks(4).map(|c| std::str::from_utf8(c).expect("ASCII")).collect::<Vec<_>>().join("-")
    }

    /// A code as someone typed it; `None` if it is not one.
    pub fn parse(typed: &str) -> Option<Self> {
        let mut bits: u128 = 0;
        let mut n = 0;
        for c in typed.chars().filter(|c| !matches!(c, '-' | ' ' | '\t')) {
            let c = match c.to_ascii_uppercase() {
                'I' | 'L' => '1',
                'O' => '0',
                c => c,
            };
            let v = ALPHABET.iter().position(|&a| a as char == c)?;
            bits = (bits << 5) | v as u128;
            n += 1;
            if n > 16 {
                return None;
            }
        }
        (n == 16).then(|| {
            let mut out = [0u8; 10];
            for (i, b) in out.iter_mut().enumerate() {
                *b = (bits >> (8 * (9 - i))) as u8;
            }
            InviteCode(out)
        })
    }

    /// What the server keeps: the code's SHA-256.
    pub fn hash(&self) -> [u8; 32] {
        Sha256::digest(self.0).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::tests::Seeded;

    #[test]
    fn a_code_reads_back_however_it_was_typed() {
        let mut rng = Seeded(5);
        for _ in 0..100 {
            let c = InviteCode::generate(&mut rng);
            let t = c.text();
            assert_eq!(t.len(), 19);
            assert!(!t.contains(['I', 'L', 'O', 'U']));
            assert_eq!(InviteCode::parse(&t), Some(c));
            assert_eq!(InviteCode::parse(&t.to_lowercase().replace('-', " ")), Some(c));
        }
        let c = InviteCode::parse("K7QF-2M9D-XW4H-P3TA").unwrap();
        assert_eq!(InviteCode::parse("k7qf 2m9d xw4h p3ta"), Some(c));
        assert_eq!(InviteCode::parse("10IL-0000-0000-0000"), InviteCode::parse("1011-0000-0000-0000"));
        assert_eq!(InviteCode::parse("K7QF-2M9D-XW4H"), None, "too short");
        assert_eq!(InviteCode::parse("K7QF-2M9D-XW4H-P3TU"), None, "U is not in it");
        assert_ne!(c.hash(), InviteCode::parse("K7QF-2M9D-XW4H-P3TB").unwrap().hash());
    }
}
