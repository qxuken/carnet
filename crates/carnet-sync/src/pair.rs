//! Letting a device in (docs/SYNC.md §7): by pairing with one already in,
//! or by the recovery code when none is left. Both are state machines
//! with no I/O, like the session: frames in, frames out.
//!
//! **Pairing.** The device in opens a pairing; the server gives it a
//! nameplate, a small number, and the device picks six random digits: the
//! code is both, `47-381095`. The new device types it. Each side starts
//! SPAKE2 with the whole code as its password, and the server relays their
//! messages. From those it learns nothing it can use: the two sides end
//! with one key only if their codes matched, and a server guessing the
//! code gets one try (the nameplate is spent on the first join).
//!
//! ```text
//! new device                     server                     device in
//!                                  ← PairOpen
//!                                  PairOpened{47} →     shows 47-381095
//! PairJoin{47, pake_n} →           → PairJoined{pake_n}
//!                                  ← PairStep{pake_i}
//! ← PairStep{pake_i}
//!   both now hold K
//! PairStep{seal_K(device, key)} →  →                    opens: right code
//!                                  ← PairApprove{device, key}
//!                                  ← PairStep{seal_K(ring, account)}
//! ← PairStep{…}                       (the server lets the device in)
//!   logs in with Hello, as any device
//! ```
//!
//! The new device's key goes sealed under K, so the server cannot put its
//! own in its place; the key ring (every epoch's vault key) comes back
//! sealed under K, so the server never sees it.
//!
//! **Recovery.** The recovery code's signing key signs the login
//! challenge, naming a new device; the server lets the device in and
//! hands back the key ring as it was when the code was made, wrapped under
//! the code's wrapping key, and each epoch's key made since, sealed to the
//! code's box key. A password and a passkey each come to a secret that
//! recovers so, in a slot of its own.

use rand_core::{CryptoRng, RngCore};
use spake2::{Ed25519Group, Identity as SpakeId, Password, Spake2};

use crate::crypto::{DeviceKey, Random, RecoveryCode, Ring, Sealed, VaultKey, open_rekey, recovery_message};
use crate::wire::{self, Carnet, Frame};

/// The code a new device types: a nameplate and six secret digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairCode {
    pub nameplate: u32,
    pub secret: u32,
}

impl PairCode {
    /// `47-381095`.
    pub fn text(&self) -> String {
        format!("{}-{:06}", self.nameplate, self.secret)
    }

    /// A code as someone typed it: spaces and dashes anywhere after the
    /// nameplate's dash forgiven.
    pub fn parse(typed: &str) -> Option<Self> {
        let typed: String = typed.chars().filter(|c| !c.is_whitespace()).collect();
        let (n, s) = typed.split_once('-')?;
        let s: String = s.chars().filter(|c| *c != '-').collect();
        if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(PairCode { nameplate: n.parse().ok()?, secret: s.parse().ok()? })
    }

    /// The code as a link to the web viewer on the server at `origin`:
    /// what a QR code carries, so a phone's camera opens the viewer with
    /// the code in it.
    pub fn link(&self, origin: &str) -> String {
        format!("{}/#pair={}", origin.trim_end_matches('/'), self.text())
    }

    /// A code from what a QR code carried, or a link pasted: a
    /// [`link`](PairCode::link), or the code itself.
    pub fn from_link(text: &str) -> Option<Self> {
        let text = text.trim();
        Self::parse(text.rsplit_once("#pair=").map_or(text, |(_, code)| code))
    }
}

/// Our `Random`, as SPAKE2's (and OPAQUE's) `rand_core` 0.6 wants it.
pub(crate) struct Rng<'a, R: Random>(pub(crate) &'a mut R);

impl<R: Random> RngCore for Rng<'_, R> {
    fn next_u32(&mut self) -> u32 {
        u32::from_le_bytes(self.0.bytes())
    }
    fn next_u64(&mut self) -> u64 {
        u64::from_le_bytes(self.0.bytes())
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill(dest);
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.0.fill(dest);
        Ok(())
    }
}

impl<R: Random> CryptoRng for Rng<'_, R> {}

fn pake(code: &PairCode, rng: &mut impl Random) -> (Spake2<Ed25519Group>, Vec<u8>) {
    Spake2::<Ed25519Group>::start_symmetric_with_rng(
        &Password::new(code.text().as_bytes()),
        &SpakeId::new(b"carnet pairing v1"),
        Rng(rng),
    )
}

/// The two keys a pairing's shared secret gives: one for each way.
struct Keys {
    /// New device → device in: the new device's id and key.
    to_in: VaultKey,
    /// Device in → new device: the key ring, the account.
    to_new: VaultKey,
}

fn keys(shared: &[u8]) -> Keys {
    use hkdf::Hkdf;
    use sha2::Sha256;
    let mut a = [0; 32];
    let mut b = [0; 32];
    let h = Hkdf::<Sha256>::new(None, shared);
    h.expand(b"carnet pairing v1 to device in", &mut a).expect("32 bytes");
    h.expand(b"carnet pairing v1 to new device", &mut b).expect("32 bytes");
    Keys { to_in: VaultKey::from_bytes(a), to_new: VaultKey::from_bytes(b) }
}

/// A pairing step's bytes: what kind, then what.
const STEP_PAKE: u8 = 1;
const STEP_DEVICE: u8 = 2;
const STEP_KEY: u8 = 3;

fn step(kind: u8, data: &[u8]) -> Vec<u8> {
    wire::encode(&Frame::Carnet(Carnet::PairStep { data: [&[kind][..], data].concat() }))
}

/// Length-prefixed parts, sealed under `key` (wrapped as a vault key
/// would be: IV, then sealed bytes).
fn seal_parts(key: &VaultKey, parts: &[&[u8]], rng: &mut impl Random) -> Vec<u8> {
    let mut plain = Vec::new();
    for p in parts {
        plain.extend((p.len() as u32).to_be_bytes());
        plain.extend(*p);
    }
    let k = key.doc_key(0, "pairing");
    let iv: [u8; 12] = rng.bytes();
    let mut out = iv.to_vec();
    out.extend(k.seal(iv, b"carnet pairing v1", &plain));
    out
}

fn open_parts(key: &VaultKey, sealed: &[u8]) -> Result<Vec<Vec<u8>>, Sealed> {
    let (iv, ct) = sealed.split_at_checked(12).ok_or(Sealed)?;
    let plain = key.doc_key(0, "pairing").open(iv.try_into().map_err(|_| Sealed)?, b"carnet pairing v1", ct)?;
    let mut parts = Vec::new();
    let mut rest = &plain[..];
    while !rest.is_empty() {
        let (len, r) = rest.split_at_checked(4).ok_or(Sealed)?;
        let len = u32::from_be_bytes(len.try_into().map_err(|_| Sealed)?) as usize;
        let (p, r) = r.split_at_checked(len).ok_or(Sealed)?;
        parts.push(p.to_vec());
        rest = r;
    }
    Ok(parts)
}

/// The device in's side of a pairing, kept by its session.
pub(crate) struct Opener {
    secret: u32,
    state: OpenerState,
}

enum OpenerState {
    /// The server's nameplate awaited.
    Asked,
    /// Shown; a device's join awaited.
    Open { pake: Spake2<Ed25519Group>, mine: Vec<u8> },
    /// Joined: the new device's key awaited.
    Keyed(Keys),
    Done,
}

/// What a pairing step asks of the session.
pub(crate) enum OpenerOut {
    /// Frames to send.
    Send(Vec<Vec<u8>>),
    /// The code to show.
    Code(PairCode),
    /// Paired: frames to send, and the device let in.
    Paired(Vec<Vec<u8>>, Vec<u8>),
    /// Over, and why, with what to send to close it.
    Failed(String, Vec<Vec<u8>>),
    Nothing,
}

impl Opener {
    pub(crate) fn new(rng: &mut impl Random) -> Self {
        let secret = u32::from_le_bytes(rng.bytes()) % 1_000_000;
        Opener { secret, state: OpenerState::Asked }
    }

    pub(crate) fn receive(
        &mut self,
        frame: &Carnet,
        ring: &Ring,
        account: &[u8],
        rng: &mut impl Random,
    ) -> OpenerOut {
        match (std::mem::replace(&mut self.state, OpenerState::Done), frame) {
            (OpenerState::Asked, Carnet::PairOpened { nameplate }) => {
                let code = PairCode { nameplate: *nameplate, secret: self.secret };
                let (pake, mine) = pake(&code, rng);
                self.state = OpenerState::Open { pake, mine };
                OpenerOut::Code(code)
            }
            (OpenerState::Open { pake, mine, .. }, Carnet::PairJoined { pake: theirs }) => match pake.finish(theirs) {
                Ok(shared) => {
                    self.state = OpenerState::Keyed(keys(&shared));
                    OpenerOut::Send(vec![step(STEP_PAKE, &mine)])
                }
                Err(_) => close("the new device's message was not one"),
            },
            (OpenerState::Keyed(k), Carnet::PairStep { data }) if data.first() == Some(&STEP_DEVICE) => {
                // A wrong code, or the server meddling: the one try is spent,
                // and the pairing closed.
                let Ok(parts) = open_parts(&k.to_in, &data[1..]) else { return close("the codes did not match") };
                let [device, public] = &parts[..] else { return close("a device that said too much") };
                let Ok(public) = <[u8; 32]>::try_from(public.as_slice()) else { return close("a key that is not one") };
                let approve = wire::encode(&Frame::Carnet(Carnet::PairApprove { device: device.clone(), public }));
                let sealed = seal_parts(&k.to_new, &[&ring.to_bytes(), account], rng);
                OpenerOut::Paired(vec![approve, step(STEP_KEY, &sealed)], device.clone())
            }
            (_, Carnet::PairClose { reason }) => OpenerOut::Failed(reason.clone(), Vec::new()),
            (state, _) => {
                self.state = state;
                OpenerOut::Nothing
            }
        }
    }
}

/// The pairing closed, at the server too, for `reason`.
fn close(reason: &str) -> OpenerOut {
    let frame = wire::encode(&Frame::Carnet(Carnet::PairClose { reason: reason.into() }));
    OpenerOut::Failed(reason.into(), vec![frame])
}

/// What a new device ends with: what it logs in with, and the challenge
/// already on its socket, to answer with its new session.
#[derive(Debug)]
pub struct Admitted {
    pub account: Vec<u8>,
    pub ring: Ring,
    /// The server's challenge frame, as it came: hand it to the new
    /// session after `connected()`.
    pub challenge: Vec<u8>,
}

/// Where a joining or recovering device is.
#[derive(Debug)]
pub enum Joining {
    Going,
    In(Admitted),
    Failed(String),
}

/// The new device's side of a pairing.
pub struct Joiner {
    code: PairCode,
    device: Vec<u8>,
    key: DeviceKey,
    pake: Option<Spake2<Ed25519Group>>,
    keys: Option<Keys>,
    challenge: Option<Vec<u8>>,
    out: Vec<Vec<u8>>,
    pub state: Joining,
}

impl Joiner {
    pub fn new(code: PairCode, device: Vec<u8>, key: DeviceKey) -> Self {
        Joiner { code, device, key, pake: None, keys: None, challenge: None, out: Vec::new(), state: Joining::Going }
    }

    pub fn take_out(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.out)
    }

    pub fn receive(&mut self, bytes: &[u8], rng: &mut impl Random) {
        let Ok(Frame::Carnet(frame)) = wire::decode(bytes) else { return };
        match frame {
            Carnet::Challenge { server, .. } if self.challenge.is_none() => {
                if let Some(why) = server.fit().refusal() {
                    self.state = Joining::Failed(why.into());
                    return;
                }
                self.challenge = Some(bytes.to_vec());
                let (pake, mine) = pake(&self.code, rng);
                self.pake = Some(pake);
                self.out.push(wire::encode(&Frame::Carnet(Carnet::PairJoin { nameplate: self.code.nameplate, pake: mine })));
            }
            Carnet::PairStep { data } if data.first() == Some(&STEP_PAKE) => {
                let Some(pake) = self.pake.take() else { return };
                let Ok(shared) = pake.finish(&data[1..]) else {
                    self.state = Joining::Failed("the other device's message was not one".into());
                    return;
                };
                let k = keys(&shared);
                let sealed = seal_parts(&k.to_in, &[&self.device, &self.key.public()], rng);
                self.out.push(step(STEP_DEVICE, &sealed));
                self.keys = Some(k);
            }
            Carnet::PairStep { data } if data.first() == Some(&STEP_KEY) => {
                let Some(k) = &self.keys else { return };
                let opened = open_parts(&k.to_new, &data[1..]).ok().and_then(|parts| match &parts[..] {
                    [ring, account] => Some((Ring::from_bytes(ring)?, account.clone())),
                    _ => None,
                });
                self.state = match (opened, self.challenge.take()) {
                    (Some((ring, account)), Some(challenge)) => Joining::In(Admitted { account, ring, challenge }),
                    _ => Joining::Failed("the vault key did not open".into()),
                };
            }
            Carnet::PairClose { reason } => self.state = Joining::Failed(reason),
            Carnet::Denied { reason, .. } => self.state = Joining::Failed(reason),
            _ => {}
        }
    }
}

/// What the server says to a recovery whose key no slot has: words
/// replaced, a password changed, a passkey removed, or never any.
pub const NO_SLOT: &str = "no account has that recovery code";

/// A new device let in by the recovery code.
pub struct Recoverer {
    origin: String,
    code: RecoveryCode,
    device: Vec<u8>,
    key: DeviceKey,
    found: Option<(Vec<u8>, Ring)>,
    asked: bool,
    out: Vec<Vec<u8>>,
    pub state: Joining,
}

impl Recoverer {
    pub fn new(origin: impl Into<String>, code: RecoveryCode, device: Vec<u8>, key: DeviceKey) -> Self {
        Recoverer { origin: origin.into(), code, device, key, found: None, asked: false, out: Vec::new(), state: Joining::Going }
    }

    /// A new device let in by a passkey (§7, `passkey.rs`): what its PRF
    /// output comes to recovers, in the passkey's slot.
    pub fn passkey(origin: impl Into<String>, prf: &[u8], device: Vec<u8>, key: DeviceKey) -> Self {
        Self::new(origin, crate::passkey::secret(prf), device, key)
    }

    pub fn take_out(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.out)
    }

    pub fn receive(&mut self, bytes: &[u8]) {
        let Ok(Frame::Carnet(frame)) = wire::decode(bytes) else { return };
        match frame {
            // The first challenge: answered with the recovery key.
            Carnet::Challenge { nonce, server } if !self.asked => {
                if let Some(why) = server.fit().refusal() {
                    self.state = Joining::Failed(why.into());
                    return;
                }
                self.asked = true;
                let signer = self.code.signing_key();
                let public = signer.public();
                let m = recovery_message(&self.origin, &public, &self.device, &self.key.public(), &nonce);
                self.out.push(wire::encode(&Frame::Carnet(Carnet::Recover {
                    public,
                    signature: signer.sign(&m),
                    device: self.device.clone(),
                    device_public: self.key.public(),
                })));
            }
            Carnet::Recovered { account, wrapped, rekeys } => match Ring::unwrap(&wrapped, &self.code.wrapping_key()) {
                Ok(mut ring) => {
                    // Each epoch since the code was made: one that does not
                    // open is left out (records under it won't read).
                    let secret = self.code.box_secret();
                    for (epoch, key) in rekeys.iter().filter_map(|r| open_rekey(&secret, r).ok()) {
                        ring.insert(epoch, key);
                    }
                    self.found = Some((account, ring));
                }
                Err(_) => self.state = Joining::Failed("the vault key did not open with these words".into()),
            },
            // The second: the one the new device logs in with.
            Carnet::Challenge { .. } => {
                if let Some((account, ring)) = self.found.take() {
                    self.state = Joining::In(Admitted { account, ring, challenge: bytes.to_vec() });
                }
            }
            Carnet::Denied { reason, .. } => self.state = Joining::Failed(reason),
            _ => {}
        }
    }
}

/// A new device let in by an email and password (§7, `password.rs`):
/// OPAQUE first, which gives what the password comes to; from there, as a
/// recovery, in the password's slot.
pub struct PasswordJoiner {
    origin: String,
    id: String,
    password: String,
    device: Vec<u8>,
    key: DeviceKey,
    /// The first challenge, kept for the recovery that follows.
    challenge: Option<Vec<u8>>,
    login: Option<opaque_ke::ClientLogin<crate::password::Suite>>,
    then: Option<Recoverer>,
    out: Vec<Vec<u8>>,
    state: Joining,
}

impl PasswordJoiner {
    /// Where it is: the recovery's, once OPAQUE is done.
    pub fn state(&self) -> &Joining {
        self.then.as_ref().map_or(&self.state, |r| &r.state)
    }

    pub fn into_state(self) -> Joining {
        match self.then {
            Some(r) => r.state,
            None => self.state,
        }
    }

    pub fn new(origin: impl Into<String>, email: &str, password: impl Into<String>, device: Vec<u8>, key: DeviceKey) -> Self {
        PasswordJoiner {
            origin: origin.into(),
            id: crate::password::id(email),
            password: password.into(),
            device,
            key,
            challenge: None,
            login: None,
            then: None,
            out: Vec::new(),
            state: Joining::Going,
        }
    }

    pub fn take_out(&mut self) -> Vec<Vec<u8>> {
        let mut out = std::mem::take(&mut self.out);
        if let Some(r) = self.then.as_mut() {
            out.extend(r.take_out());
        }
        out
    }

    pub fn receive(&mut self, bytes: &[u8], rng: &mut impl Random) {
        if let Some(r) = self.then.as_mut() {
            r.receive(bytes);
            return;
        }
        let Ok(Frame::Carnet(frame)) = wire::decode(bytes) else { return };
        match frame {
            Carnet::Challenge { server, .. } if self.challenge.is_none() => {
                if let Some(why) = server.fit().refusal() {
                    self.state = Joining::Failed(why.into());
                    return;
                }
                self.challenge = Some(bytes.to_vec());
                match crate::password::login_start(&self.password, rng) {
                    Ok((request, state)) => {
                        self.login = Some(state);
                        self.out.push(wire::encode(&Frame::Carnet(Carnet::PasswordLogin { id: self.id.clone(), request })));
                    }
                    Err(e) => self.state = Joining::Failed(e),
                }
            }
            Carnet::PasswordLoginReply { response } => {
                let (Some(state), Some(challenge)) = (self.login.take(), self.challenge.take()) else { return };
                match crate::password::login_finish(state, &self.id, &self.password, &response) {
                    Ok(code) => {
                        let mut r = Recoverer::new(self.origin.clone(), code, self.device.clone(), self.key.clone());
                        r.receive(&challenge);
                        self.then = Some(r);
                    }
                    Err(e) => self.state = Joining::Failed(e),
                }
            }
            Carnet::Denied { reason, .. } => self.state = Joining::Failed(reason),
            _ => {}
        }
    }
}

/// A first device, making a new account by an invite: the vault key is
/// made here, never anywhere else (§7).
pub struct Enroller {
    invite: String,
    device: Vec<u8>,
    key: DeviceKey,
    vault: VaultKey,
    account: Option<Vec<u8>>,
    asked: bool,
    out: Vec<Vec<u8>>,
    pub state: Joining,
}

impl Enroller {
    pub fn new(invite: impl Into<String>, device: Vec<u8>, key: DeviceKey, rng: &mut impl Random) -> Self {
        Enroller {
            invite: invite.into(),
            device,
            key,
            vault: VaultKey::generate(rng),
            account: None,
            asked: false,
            out: Vec::new(),
            state: Joining::Going,
        }
    }

    pub fn take_out(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.out)
    }

    pub fn receive(&mut self, bytes: &[u8]) {
        let Ok(Frame::Carnet(frame)) = wire::decode(bytes) else { return };
        match frame {
            Carnet::Challenge { server, .. } if !self.asked => {
                if let Some(why) = server.fit().refusal() {
                    self.state = Joining::Failed(why.into());
                    return;
                }
                self.asked = true;
                self.out.push(wire::encode(&Frame::Carnet(Carnet::Enroll {
                    invite: self.invite.clone(),
                    device: self.device.clone(),
                    public: self.key.public(),
                })));
            }
            Carnet::Enrolled { account } => self.account = Some(account),
            Carnet::Challenge { .. } => {
                if let Some(account) = self.account.take() {
                    self.state = Joining::In(Admitted { account, ring: Ring::new(self.vault.clone()), challenge: bytes.to_vec() });
                }
            }
            Carnet::Denied { reason, .. } => self.state = Joining::Failed(reason),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::tests::Seeded;

    #[test]
    fn a_code_reads_back_as_typed() {
        let c = PairCode { nameplate: 47, secret: 1095 };
        assert_eq!(c.text(), "47-001095");
        assert_eq!(PairCode::parse(" 47 - 001 095 "), Some(c));
        assert_eq!(PairCode::parse("47-001-095"), Some(c));
        assert_eq!(PairCode::parse("47-1095"), None, "six digits");
        assert_eq!(PairCode::parse("4a-001095"), None);
    }

    #[test]
    fn a_code_goes_into_a_link_and_comes_back_out() {
        let c = PairCode { nameplate: 47, secret: 1095 };
        assert_eq!(c.link("https://notes.example.com/"), "https://notes.example.com/#pair=47-001095");
        assert_eq!(PairCode::from_link(&c.link("http://127.0.0.1:8787")), Some(c));
        assert_eq!(PairCode::from_link(" 47-001095 "), Some(c), "the code alone");
        assert_eq!(PairCode::from_link("https://e.com/#pair=nonsense"), None);
    }

    #[test]
    fn the_same_code_gives_both_sides_one_key_and_another_does_not() {
        let mut rng = Seeded(11);
        let code = PairCode { nameplate: 3, secret: 123456 };
        let (a, ma) = pake(&code, &mut rng);
        let (b, mb) = pake(&code, &mut rng);
        assert_eq!(a.finish(&mb).unwrap(), b.finish(&ma).unwrap());
        let (a, ma) = pake(&code, &mut rng);
        let (b, mb) = pake(&PairCode { secret: 123457, ..code }, &mut rng);
        assert_ne!(a.finish(&mb).unwrap(), b.finish(&ma).unwrap());
    }
}
