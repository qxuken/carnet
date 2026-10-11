//! Email and password (docs/SYNC.md §7), for a hosted service: one more
//! way in, beside a code from another device and the recovery words.
//!
//! OPAQUE (RFC 9807, `opaque-ke`): the password never reaches the server,
//! not even hashed, and a stolen database allows no precomputed guessing:
//! each guess needs the server's own key and Argon2id, one account at a
//! time. What OPAQUE gives the device, its `export_key`, the server never
//! sees; from it comes a secret that works as the recovery words do
//! ([`RecoveryCode::from_secret`]): it wraps a copy of the vault key kept
//! on the server, signs the login that lets a new device in, and is what
//! a new epoch's key is sealed to when a device is removed. A slot of its
//! own on the server ([`SLOT`]), beside the words'.
//!
//! The server only relays OPAQUE's messages and keeps its record; whether
//! the password was right is proved by the signature that follows, as a
//! recovery's is.

use argon2::Argon2;
use opaque_ke::{
    CipherSuite, ClientLogin, ClientLoginFinishParameters, ClientRegistration, ClientRegistrationFinishParameters, CredentialFinalization,
    CredentialRequest, CredentialResponse, Identifiers, RegistrationRequest, RegistrationResponse, RegistrationUpload, ServerLogin,
    ServerLoginStartParameters, ServerRegistration, ServerSetup,
};

use crate::crypto::{Random, RecoveryCode};
use crate::pair::Rng;

/// Where a password's keys are kept on the server, as a device id the
/// devices' own can't be (they are at most 64 bytes).
pub const SLOT: &[u8] = &[0xff; 65];

/// What the server knows itself by, in every OPAQUE exchange.
const SERVER: &[u8] = b"carnet";

pub struct Suite;

impl CipherSuite for Suite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeGroup = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::key_exchange::tripledh::TripleDh;
    type Ksf = Argon2<'static>;
}

/// An email as it's kept and looked up: trimmed, lowercased.
pub fn id(email: &str) -> String {
    email.trim().to_lowercase()
}

fn identifiers(id: &str) -> Identifiers<'_> {
    Identifiers { client: Some(id.as_bytes()), server: Some(SERVER) }
}

/// The device setting a password: its first message, and what it keeps
/// for the answer.
pub fn register_start(password: &str, rng: &mut impl Random) -> Result<(Vec<u8>, ClientRegistration<Suite>), String> {
    let r = ClientRegistration::<Suite>::start(&mut Rng(rng), password.as_bytes()).map_err(|e| e.to_string())?;
    Ok((r.message.serialize().to_vec(), r.state))
}

/// The server's answer taken: the record for the server to keep, and the
/// secret the password comes to.
pub fn register_finish(state: ClientRegistration<Suite>, id: &str, password: &str, response: &[u8], rng: &mut impl Random) -> Result<(Vec<u8>, RecoveryCode), String> {
    let response = RegistrationResponse::<Suite>::deserialize(response).map_err(|e| e.to_string())?;
    let params = ClientRegistrationFinishParameters::new(identifiers(id), None);
    let r = state.finish(&mut Rng(rng), password.as_bytes(), response, params).map_err(|e| e.to_string())?;
    Ok((r.message.serialize().to_vec(), RecoveryCode::from_secret(&r.export_key)))
}

/// A new device signing in: its first message, and what it keeps.
pub fn login_start(password: &str, rng: &mut impl Random) -> Result<(Vec<u8>, ClientLogin<Suite>), String> {
    let r = ClientLogin::<Suite>::start(&mut Rng(rng), password.as_bytes()).map_err(|e| e.to_string())?;
    Ok((r.message.serialize().to_vec(), r.state))
}

/// The server's answer taken: the secret, if the password was the one
/// set (and the record this email's: an unknown email's answer is made
/// up, and fails here alike).
pub fn login_finish(state: ClientLogin<Suite>, id: &str, password: &str, response: &[u8]) -> Result<RecoveryCode, String> {
    let response = CredentialResponse::<Suite>::deserialize(response).map_err(|_| WRONG.to_string())?;
    let params = ClientLoginFinishParameters::new(None, identifiers(id), None);
    let r = state.finish(password.as_bytes(), response, params).map_err(|_| WRONG.to_string())?;
    // OPAQUE's own last message isn't needed: the signature that follows
    // proves the secret.
    let _: CredentialFinalization<Suite> = r.message;
    Ok(RecoveryCode::from_secret(&r.export_key))
}

/// A device's half of setting a password, while the server answers.
pub struct Pending(pub(crate) ClientRegistration<Suite>);

/// What a device is told when the email or the password isn't right.
pub const WRONG: &str = "That email and password don't match: check both, and try again";

/// The server's side: its key, made once and kept (`data/opaque.key`).
pub struct Server {
    setup: ServerSetup<Suite>,
}

impl Server {
    pub fn generate(rng: &mut impl Random) -> Self {
        Server { setup: ServerSetup::<Suite>::new(&mut Rng(rng)) }
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        ServerSetup::<Suite>::deserialize(b).ok().map(|setup| Server { setup })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.setup.serialize().to_vec()
    }

    /// The answer to a device setting a password for `id`.
    pub fn register(&self, id: &str, request: &[u8]) -> Result<Vec<u8>, String> {
        let request = RegistrationRequest::<Suite>::deserialize(request).map_err(|e| e.to_string())?;
        let r = ServerRegistration::<Suite>::start(&self.setup, request, id.as_bytes()).map_err(|e| e.to_string())?;
        Ok(r.message.serialize().to_vec())
    }

    /// The record to keep from a device's upload: what a sign-in is
    /// answered from.
    pub fn record(upload: &[u8]) -> Result<Vec<u8>, String> {
        let upload = RegistrationUpload::<Suite>::deserialize(upload).map_err(|e| e.to_string())?;
        Ok(ServerRegistration::<Suite>::finish(upload).serialize().to_vec())
    }

    /// The answer to a device signing in as `id`, from its `record` (none:
    /// an answer made up, that looks the same and fails on the device).
    pub fn login(&self, id: &str, record: Option<&[u8]>, request: &[u8], rng: &mut impl Random) -> Result<Vec<u8>, String> {
        let request = CredentialRequest::<Suite>::deserialize(request).map_err(|e| e.to_string())?;
        let record = match record {
            Some(r) => Some(ServerRegistration::<Suite>::deserialize(r).map_err(|e| e.to_string())?),
            None => None,
        };
        let params = ServerLoginStartParameters { context: None, identifiers: identifiers(id) };
        let r = ServerLogin::start(&mut Rng(rng), &self.setup, record, request, id.as_bytes(), params).map_err(|e| e.to_string())?;
        Ok(r.message.serialize().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::tests::Seeded;

    /// Set, then signed in with: the same secret both times; a wrong
    /// password, an unknown email or another server's key, no secret at
    /// all, and nothing on the server tells which.
    #[test]
    fn a_password_set_signs_in_and_a_wrong_one_does_not() {
        let mut rng = Seeded(31);
        let server = Server::generate(&mut rng);
        let server = Server::from_bytes(&server.to_bytes()).unwrap();
        let ann = id(" Ann@Example.com ");
        assert_eq!(ann, "ann@example.com");

        let (request, state) = register_start("correct horse", &mut rng).unwrap();
        let response = server.register(&ann, &request).unwrap();
        let (upload, set) = register_finish(state, &ann, "correct horse", &response, &mut rng).unwrap();
        let record = Server::record(&upload).unwrap();
        assert!(!record.windows(13).any(|w| w == b"correct horse"), "the password isn't in the record");

        let sign_in = |password: &str, email: &str, record: Option<&[u8]>, server: &Server, rng: &mut Seeded| {
            let (request, state) = login_start(password, rng).unwrap();
            let response = server.login(email, record, &request, rng).unwrap();
            login_finish(state, email, password, &response)
        };
        let got = sign_in("correct horse", &ann, Some(&record), &server, &mut rng).unwrap();
        assert_eq!(got.signing_key().public(), set.signing_key().public(), "the same secret");
        assert_eq!(got.wrapping_key(), set.wrapping_key());
        assert_eq!(sign_in("wrong horse", &ann, Some(&record), &server, &mut rng).unwrap_err(), WRONG);
        assert_eq!(sign_in("correct horse", "bob@example.com", None, &server, &mut rng).unwrap_err(), WRONG, "no such email: the same");
        let other = Server::generate(&mut rng);
        assert!(sign_in("correct horse", &ann, Some(&record), &other, &mut rng).is_err(), "another server's key");
    }
}
