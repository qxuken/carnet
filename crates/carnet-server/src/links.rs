//! Signed links to a file (docs/SYNC.md §6): what lets a device `GET`
//! or `PUT` one sealed blob over HTTP, for a few minutes, with no session
//! and nothing stored. The socket, logged in, is what asks for one.
//!
//! Not a JWT: nobody but this server reads them, and a fixed form with one
//! algorithm leaves nothing to negotiate or confuse. A link is, in hex:
//!
//! ```text
//! version (1) · put (1) · expires (8) · bytes (8) · file id (32) · account (1..64)
//! · HMAC-SHA256 of all that, under the server's key (32)
//! ```
//!
//! The key is made on the first start (`data/links.key`) and never leaves
//! the server. A link names one account's one file, one way, of so
//! many bytes, until a time: lost, it opens that file's sealed bytes for
//! minutes, no more (and they open only with the account's keys).

use std::path::Path;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// How long a link is good for: a request by it must begin by then.
pub const LIFETIME: u64 = 5 * 60;

/// How long a put by a link may go on once it has begun: on this server,
/// a file's request has as long (`server::BLOB_TIMEOUT`), so a put
/// begun as its link runs out is over [`LIFETIME`] and this after the
/// link was given, at the latest. Until then, what it may put still
/// counts against its account's room (`blobs::Worker`).
pub const PUT_TAKES: u64 = 10 * 60;

const VERSION: u8 = 1;
const MAC: usize = 32;

/// What a link allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub account: Vec<u8>,
    pub id: [u8; 32],
    pub put: bool,
    /// What a put must send, exactly (the length the device said when it
    /// offered the file); for a get, 0.
    pub max: u64,
    pub expires: u64,
}

/// The server's key for links.
pub struct Signer {
    key: [u8; 32],
}

impl Signer {
    pub fn new(key: [u8; 32]) -> Self {
        Signer { key }
    }

    /// The key kept in `data/links.key`, made with `random` the first
    /// time.
    pub fn open(data: &Path, random: impl FnOnce() -> [u8; 32]) -> std::io::Result<Self> {
        let path = data.join("links.key");
        match std::fs::read(&path) {
            Ok(k) if k.len() == 32 => Ok(Signer::new(k.try_into().expect("32"))),
            Ok(_) => Err(std::io::Error::other(format!("{}: not a key", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = random();
                let tmp = path.with_extension("tmp");
                std::fs::write(&tmp, key)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
                }
                std::fs::rename(&tmp, &path)?;
                Ok(Signer::new(key))
            }
            Err(e) => Err(e),
        }
    }

    fn mac(&self) -> Hmac<Sha256> {
        <Hmac<Sha256> as KeyInit>::new_from_slice(&self.key).expect("any key length")
    }

    /// The link's token, for the URL's last part.
    pub fn sign(&self, g: &Grant) -> String {
        let mut payload = vec![VERSION, u8::from(g.put)];
        payload.extend(g.expires.to_be_bytes());
        payload.extend(g.max.to_be_bytes());
        payload.extend(g.id);
        payload.extend(&g.account);
        let mut mac = self.mac();
        mac.update(&payload);
        payload.extend(mac.finalize().into_bytes());
        payload.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// What a token allows at `now`, if it's one this server signed and
    /// still good.
    pub fn check(&self, token: &str, now: u64) -> Result<Grant, &'static str> {
        let bytes = unhex(token).ok_or("not a link")?;
        if bytes.len() < 2 + 8 + 8 + 32 + 1 + MAC || bytes[0] != VERSION {
            return Err("not a link");
        }
        let (payload, tag) = bytes.split_at(bytes.len() - MAC);
        let mut mac = self.mac();
        mac.update(payload);
        mac.verify_slice(tag).map_err(|_| "not a link")?;
        let account = payload[50..].to_vec();
        if account.len() > 64 {
            return Err("not a link");
        }
        let g = Grant {
            put: payload[1] == 1,
            expires: u64::from_be_bytes(payload[2..10].try_into().expect("8")),
            max: u64::from_be_bytes(payload[10..18].try_into().expect("8")),
            id: payload[18..50].try_into().expect("32"),
            account,
        };
        if now >= g.expires {
            return Err("this link has expired");
        }
        Ok(g)
    }
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len().is_multiple_of(2) && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_good_for_what_it_says_until_it_says() {
        let s = Signer::new([7; 32]);
        let g = Grant { account: b"acct".to_vec(), id: [3; 32], put: true, max: 1000, expires: 500 };
        let t = s.sign(&g);
        assert_eq!(s.check(&t, 499), Ok(g.clone()));
        assert_eq!(s.check(&t, 500), Err("this link has expired"));
        // Another server's key; a byte changed anywhere; cut short.
        assert!(Signer::new([8; 32]).check(&t, 0).is_err());
        for i in [0, 3, 40, 100, t.len() - 1] {
            let mut bad = t.clone().into_bytes();
            bad[i] = if bad[i] == b'0' { b'1' } else { b'0' };
            assert!(s.check(std::str::from_utf8(&bad).unwrap(), 0).is_err(), "byte {i}");
        }
        assert!(s.check(&t[..t.len() - 2], 0).is_err());
        assert!(s.check("zz", 0).is_err() && s.check("", 0).is_err());
        // A get can't be made a put: the flag is signed.
        let get = s.sign(&Grant { put: false, max: 0, ..g });
        assert!(!s.check(&get, 0).unwrap().put);
    }
}
