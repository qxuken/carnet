//! Carnet's notes in the browser (docs/SYNC.md §8): the PWA's Rust, bound
//! for JS. Everything here is sans-I/O: frames in and out, bytes to keep
//! and bytes kept. web/ moves them over the WebSocket and into IndexedDB.

pub mod editor;
pub mod notes;
pub mod relink;
pub mod zip;

use carnet_core::{inline, page};
use carnet_sync::crypto::{DeviceKey, Random, RecoveryCode};
use carnet_sync::invite::InviteCode;
use carnet_sync::pair::{Enroller, Joiner, Joining, NO_SLOT, PairCode, PasswordJoiner, Recoverer};
use carnet_sync::Origin;
use wasm_bindgen::prelude::*;

use editor::PicTo;
use notes::{Me, Notes, Said, hex, unhex};

#[wasm_bindgen]
extern "C" {
    /// The browser's randomness (`crypto.getRandomValues`), in a page or
    /// a worker alike.
    #[wasm_bindgen(js_namespace = crypto, js_name = getRandomValues)]
    fn get_random_values(buf: &mut [u8]);
}

/// The browser's randomness, as the sync crates take it.
pub struct WebRandom;

impl Random for WebRandom {
    fn fill(&mut self, out: &mut [u8]) {
        // getRandomValues takes at most 65,536 bytes at a time.
        for chunk in out.chunks_mut(65_536) {
            get_random_values(chunk);
        }
    }
}

fn frames(out: Vec<Vec<u8>>) -> js_sys::Array {
    out.into_iter().map(|b| js_sys::Uint8Array::from(&b[..])).collect()
}

enum Way {
    Pair(Joiner),
    Recover(Recoverer),
    Enroll(Enroller),
    // Its OPAQUE state is large; the others are not.
    Password(Box<PasswordJoiner>),
    /// A recovery, by what a passkey's PRF output comes to.
    Passkey(Recoverer),
}

/// A browser coming into an account: by a code from a device already in,
/// an invite, the recovery words, an email and password, or a passkey.
#[wasm_bindgen]
pub struct Join {
    way: Way,
    device: Vec<u8>,
    key: DeviceKey,
}

#[wasm_bindgen]
impl Join {
    /// By a pairing code (`47-381095`).
    pub fn pair(code: &str) -> Result<Join, JsError> {
        let code = PairCode::parse(code).ok_or_else(|| JsError::new("That isn't a code from another device: it looks like 47-381095"))?;
        let device = WebRandom.bytes::<8>().to_vec();
        let key = DeviceKey::generate(&mut WebRandom);
        Ok(Join { way: Way::Pair(Joiner::new(code, device.clone(), key.clone())), device, key })
    }

    /// By the twelve words, at the server at `origin`.
    pub fn recover(origin: &str, words: &str) -> Result<Join, JsError> {
        let code = RecoveryCode::parse(words).ok_or_else(|| JsError::new(&RecoveryCode::problem(words)))?;
        let device = WebRandom.bytes::<8>().to_vec();
        let key = DeviceKey::generate(&mut WebRandom);
        Ok(Join { way: Way::Recover(Recoverer::new(origin, code, device.clone(), key.clone())), device, key })
    }

    /// By an invite (`ABCD-EFGH-JKMN-PQRS`): a new account, this browser
    /// its first device, the vault key made here.
    pub fn enroll(invite: &str) -> Result<Join, JsError> {
        let code = InviteCode::parse(invite).ok_or_else(|| JsError::new("That isn't an invite: it looks like ABCD-EFGH-JKMN-PQRS"))?;
        let device = WebRandom.bytes::<8>().to_vec();
        let key = DeviceKey::generate(&mut WebRandom);
        Ok(Join { way: Way::Enroll(Enroller::new(code.text(), device.clone(), key.clone(), &mut WebRandom)), device, key })
    }

    /// By an email and password, at the server at `origin`.
    pub fn password(origin: &str, email: &str, password: &str) -> Result<Join, JsError> {
        if !email.contains('@') {
            return Err(JsError::new("That doesn't look like an email address"));
        }
        let device = WebRandom.bytes::<8>().to_vec();
        let key = DeviceKey::generate(&mut WebRandom);
        Ok(Join { way: Way::Password(Box::new(PasswordJoiner::new(origin, email, password, device.clone(), key.clone()))), device, key })
    }

    /// By a passkey: its PRF output, as the browser gave it for
    /// [`passkey_salt`], at the server at `origin`. Taken whole, so it is
    /// wiped here once used, not left in memory freed.
    pub fn passkey(origin: &str, prf: Vec<u8>) -> Result<Join, JsError> {
        let prf = zeroize::Zeroizing::new(prf);
        if prf.len() != carnet_sync::passkey::PRF_LEN {
            return Err(JsError::new("That passkey didn't give what Carnet needs to unlock your notes"));
        }
        let device = WebRandom.bytes::<8>().to_vec();
        let key = DeviceKey::generate(&mut WebRandom);
        Ok(Join { way: Way::Passkey(Recoverer::passkey(origin, &prf, device.clone(), key.clone())), device, key })
    }

    pub fn receive(&mut self, frame: &[u8]) {
        match &mut self.way {
            Way::Pair(j) => j.receive(frame, &mut WebRandom),
            Way::Recover(r) | Way::Passkey(r) => r.receive(frame),
            Way::Enroll(e) => e.receive(frame),
            Way::Password(p) => p.receive(frame, &mut WebRandom),
        }
    }

    pub fn take_out(&mut self) -> js_sys::Array {
        frames(match &mut self.way {
            Way::Pair(j) => j.take_out(),
            Way::Recover(r) | Way::Passkey(r) => r.take_out(),
            Way::Enroll(e) => e.take_out(),
            Way::Password(p) => p.take_out(),
        })
    }

    fn state(&self) -> &Joining {
        match &self.way {
            Way::Pair(j) => &j.state,
            Way::Recover(r) | Way::Passkey(r) => &r.state,
            Way::Enroll(e) => &e.state,
            Way::Password(p) => p.state(),
        }
    }

    /// A new account's first device: the twelve words are to be made now.
    pub fn is_new_account(&self) -> bool {
        matches!(self.way, Way::Enroll(_))
    }

    /// Why it failed, if it did.
    pub fn failed(&self) -> Option<String> {
        match (self.state(), &self.way) {
            (Joining::Failed(why), Way::Passkey(_)) if why == NO_SLOT => Some(PASSKEY_GONE.into()),
            (Joining::Failed(why), _) => Some(why.clone()),
            _ => None,
        }
    }

    /// Whether it failed because the server has no slot for this passkey:
    /// the browser may forget the passkey.
    pub fn passkey_gone(&self) -> bool {
        matches!((self.state(), &self.way), (Joining::Failed(why), Way::Passkey(_)) if why == NO_SLOT)
    }

    /// In: who this browser is now, as bytes to keep (sealed by the page
    /// under a key the browser won't hand out).
    pub fn identity(&self) -> Option<Vec<u8>> {
        match self.state() {
            Joining::In(a) => Some(Me { account: a.account.clone(), device: self.device.clone(), key: self.key.clone(), ring: a.ring.clone() }.to_bytes()),
            _ => None,
        }
    }

    /// In: the challenge already on the socket, for the session to answer.
    pub fn challenge(&self) -> Option<Vec<u8>> {
        match self.state() {
            Joining::In(a) => Some(a.challenge.clone()),
            _ => None,
        }
    }
}

/// What a browser signing in by a passkey is told when no account here
/// has it.
const PASSKEY_GONE: &str = "That passkey doesn't open an account here any more: it was removed. Come in with a code from another device, or the recovery words";

/// What a passkey's PRF is asked for, every time (`passkey::salt`).
#[wasm_bindgen]
pub fn passkey_salt() -> Vec<u8> {
    carnet_sync::passkey::salt().to_vec()
}

/// The most a picture's sealed bytes may be: the largest picture
/// (`MAX_BLOB`), sealed. A fetch stops past it.
#[wasm_bindgen]
pub fn max_sealed() -> f64 {
    carnet_sync::crypto::sealed_len(carnet_sync::crypto::MAX_BLOB) as f64
}

/// The protocol this build speaks (`compat`): what the socket's address
/// says.
#[wasm_bindgen]
pub fn protocol() -> u32 {
    carnet_sync::compat::PROTOCOL
}

/// This build's version, for the socket's address: which app versions
/// are about, as the server counts them.
#[wasm_bindgen]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Whether a server whose `/v1/info` says `protocol` and `oldest` talks
/// to this build: `""`, or why not, to show before signing in.
#[wasm_bindgen]
pub fn server_fit(protocol: u32, oldest: u32, deprecated: u32) -> String {
    let g = carnet_sync::compat::Greeting { protocol, oldest, deprecated, features: Vec::new() };
    g.fit().refusal().unwrap_or_default().to_string()
}

/// Why state kept by [`Vault::save`] won't open here, in words for the
/// page: saved by a newer build. Empty when it opens (or starts again).
#[wasm_bindgen]
pub fn state_refusal(state: &[u8]) -> String {
    notes::state_body(state).err().map(|e| e.to_string()).unwrap_or_default()
}

/// This browser's notes, and its side of sync.
#[wasm_bindgen]
pub struct Vault {
    notes: Notes,
}

#[wasm_bindgen]
impl Vault {
    /// Who it is (from [`Join::identity`]) and what it kept (from
    /// [`Vault::save`], or empty).
    pub fn open(origin: &str, identity: &[u8], name: &str, state: &[u8]) -> Result<Vault, JsError> {
        let me = Me::from_bytes(identity).ok_or_else(|| JsError::new("This browser's keys don't read"))?;
        let peer = u64::from_be_bytes(WebRandom.bytes());
        let notes = Notes::open(origin, me, name, state, peer).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Vault { notes })
    }

    /// The account, short, for the page to show.
    pub fn account(&self) -> String {
        self.notes.me.account.iter().take(4).map(|b| format!("{b:02x}")).collect()
    }

    /// The account's id, whole: what a passkey made here names as its
    /// user.
    pub fn account_id(&self) -> Vec<u8> {
        self.notes.me.account.clone()
    }

    /// Who it is, as it is now (a new key from a removal elsewhere is in
    /// it): for the page to keep again.
    pub fn identity(&self) -> Vec<u8> {
        self.notes.me.to_bytes()
    }

    pub fn connected(&mut self) {
        self.notes.session.connected();
    }

    pub fn disconnected(&mut self) {
        self.notes.session.disconnected();
    }

    pub fn ready(&self) -> bool {
        self.notes.session.is_ready()
    }

    /// How many changes (a note's, or the library's) the server isn't
    /// known to keep yet: offline, read only, or on their way.
    pub fn waiting(&self) -> u32 {
        self.notes.session.waiting() as u32
    }

    /// How many pictures put in here the server hasn't got yet: on their
    /// way, or held back (the account full, or read only).
    pub fn pictures_waiting(&self) -> u32 {
        self.notes.pictures_waiting() as u32
    }

    pub fn receive(&mut self, frame: &[u8]) {
        self.notes.receive(frame, &mut WebRandom);
    }

    pub fn take_out(&mut self) -> js_sys::Array {
        frames(self.notes.session.take_out())
    }

    /// Pictures to put or fetch over HTTP, by the short-lived links the
    /// server gave: each `{id, url, put, body}`, the id in hex, `body` a
    /// put's sealed bytes. Each done is said with [`Vault::transferred`].
    pub fn take_transfers(&mut self) -> js_sys::Array {
        use carnet_sync::session::Transfer;
        use js_sys::{Object, Reflect, Uint8Array};
        let set = |o: &Object, k: &str, v: &JsValue| {
            let _ = Reflect::set(o, &JsValue::from_str(k), v);
        };
        self.notes
            .take_transfers()
            .into_iter()
            .map(|t| {
                let o = Object::new();
                set(&o, "id", &JsValue::from_str(&hex(&t.id())));
                match &t {
                    Transfer::Put { url, sealed, .. } => {
                        set(&o, "url", &JsValue::from_str(url));
                        set(&o, "put", &JsValue::TRUE);
                        set(&o, "body", &Uint8Array::from(&sealed[..]));
                    }
                    Transfer::Get { url, .. } => {
                        set(&o, "url", &JsValue::from_str(url));
                        set(&o, "put", &JsValue::FALSE);
                    }
                }
                JsValue::from(o)
            })
            .collect()
    }

    /// How a transfer went: a fetch's sealed bytes, or `error`: `HTTP`
    /// and the status, `unreachable` for a put that got no answer while
    /// the socket is up ([`notes::UNREACHABLE`]), or any other words.
    pub fn transferred(&mut self, id: &str, put: bool, bytes: Option<Vec<u8>>, error: Option<String>) {
        let Some(id) = unhex(id).and_then(|i| <[u8; 32]>::try_from(i).ok()) else { return };
        let result = match error {
            Some(e) => Err(e),
            None => Ok(bytes.unwrap_or_default()),
        };
        self.notes.transferred(id, put, result, &mut WebRandom);
    }

    /// Whether a room changed since last asked: cheap, after every frame.
    pub fn has_changed(&self) -> bool {
        self.notes.has_changed()
    }

    /// Rooms changed since last asked; those deleted forever on another
    /// device forgotten first, the vault read for them once however many
    /// frames came.
    pub fn take_changed(&mut self) -> Vec<String> {
        self.notes.take_changed()
    }

    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes.errors)
    }

    /// Pictures come since last asked: `[id hex, bytes]` pairs.
    pub fn take_pictures(&mut self) -> js_sys::Array {
        std::mem::take(&mut self.notes.pictures)
            .into_iter()
            .map(|(id, bytes)| {
                let pair = js_sys::Array::new();
                pair.push(&JsValue::from_str(&hex(&id)));
                pair.push(&js_sys::Uint8Array::from(&bytes[..]));
                JsValue::from(pair)
            })
            .collect()
    }

    /// Pictures asked for that didn't come since last asked: their ids,
    /// in hex.
    pub fn take_missing(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes.missing).iter().map(|id| hex(id)).collect()
    }

    /// Every note, as JSON: `[{key, title, folder, pinned, trashed,
    /// snippet, done, todos, modified}]`.
    pub fn notes(&self) -> String {
        let items: Vec<String> = self
            .notes
            .notes()
            .iter()
            .map(|n| {
                format!(
                    r#"{{"key":{},"title":{},"folder":{},"pinned":{},"trashed":{},"snippet":{},"done":{},"todos":{},"modified":{}}}"#,
                    json(&n.key),
                    json(&n.title),
                    json(&n.folder),
                    n.pinned,
                    n.trashed,
                    json(&n.snippet),
                    n.done,
                    n.todos,
                    n.modified
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    pub fn folders(&self) -> Vec<String> {
        self.notes.folders()
    }

    /// The notes the server may not have as they are here: what's waiting
    /// to be sent, and of `before` (this list as the page kept it last,
    /// before a reload, say) what the server hasn't said it keeps since.
    /// For the page to keep, so a reload still knows.
    pub fn unsent(&self, before: Vec<String>) -> Vec<String> {
        self.notes.unsent(&before)
    }

    /// How many notes [`Vault::unsent_zip`] holds for `rooms`.
    pub fn unsent_notes_count(&self, rooms: Vec<String>) -> u32 {
        self.notes.note_files(&rooms).len() as u32
    }

    /// The notes of `rooms` (from [`Vault::unsent`]) as Markdown files, and
    /// the pictures given (their paths, and bytes in the same order), as
    /// one ZIP file: what a browser removed from its account hands over of
    /// what it hadn't sent.
    pub fn unsent_zip(&self, rooms: Vec<String>, picture_paths: Vec<String>, pictures: js_sys::Array) -> Vec<u8> {
        let mut files: Vec<(String, Vec<u8>)> = self.notes.note_files(&rooms).into_iter().map(|(p, t)| (p, t.into_bytes())).collect();
        for (path, bytes) in picture_paths.into_iter().zip(pictures.iter()) {
            files.push((path, js_sys::Uint8Array::new(&bytes).to_vec()));
        }
        crate::zip::zip(&files)
    }

    pub fn html(&self, key: &str) -> String {
        self.notes.html(key)
    }

    pub fn toggle_task(&mut self, key: &str, n: usize) -> bool {
        self.notes.toggle_task(key, n, &mut WebRandom)
    }

    pub fn search(&self, query: &str) -> Vec<String> {
        self.notes.search(query)
    }

    /// A note's Markdown and its version, `[text, version]`, for the
    /// editor; none for a note this browser doesn't have.
    pub fn source(&self, key: &str) -> Option<js_sys::Array> {
        let (text, version) = self.notes.source(key)?;
        Some([JsValue::from_str(&text), JsValue::from_str(&version)].into_iter().collect())
    }

    /// `text`, typed from the note at `base`, merged with what came since
    /// and sent: `[merged text, its version]`.
    pub fn edit(&mut self, key: &str, base: &str, text: &str) -> Option<js_sys::Array> {
        let (text, version) = self.notes.edit(key, base, text, &mut WebRandom)?;
        Some([JsValue::from_str(&text), JsValue::from_str(&version)].into_iter().collect())
    }

    /// A new note in `folder`, blank and not yet sent: its key.
    pub fn create(&mut self, folder: &str) -> String {
        self.notes.create(folder)
    }

    /// A new note left blank, dropped.
    pub fn discard(&mut self, key: &str) -> bool {
        self.notes.discard(key)
    }

    /// The note moved to Deleted, or put back.
    pub fn set_trashed(&mut self, key: &str, trashed: bool) -> bool {
        self.notes.set_trashed(key, trashed, &mut WebRandom)
    }

    /// A note in Deleted, deleted forever, on every device; the pictures
    /// only it showed go too.
    pub fn delete_forever(&mut self, key: &str) -> bool {
        self.notes.delete_forever(key, &mut WebRandom)
    }

    /// A new folder named `name` inside `parent` ("" for the top), on
    /// every device: its path. Refused (the pictures' folder's name) with
    /// the reason, in words.
    pub fn make_folder(&mut self, parent: &str, name: &str) -> Result<String, JsError> {
        self.notes.make_folder(parent, name, &mut WebRandom).map_err(|e| JsError::new(&e))
    }

    /// `folder` renamed `name`, its folders and notes with it: its new path.
    pub fn rename_folder(&mut self, folder: &str, name: &str) -> Result<String, JsError> {
        self.notes.rename_folder(folder, name, &mut WebRandom).map_err(|e| JsError::new(&e))
    }

    /// `folder` into `parent` ("" for the top), its folders and notes with
    /// it: its new path. Never into itself or a folder inside it.
    pub fn move_folder(&mut self, folder: &str, parent: &str) -> Result<String, JsError> {
        self.notes.move_folder(folder, parent, &mut WebRandom).map_err(|e| JsError::new(&e))
    }

    /// Where `folder` may go: "" (the top) first, unless it's there, then
    /// each folder but itself, those inside it and its own.
    pub fn folder_targets(&self, folder: &str) -> Vec<String> {
        self.notes.folder_targets(folder)
    }

    /// Whether `folder` has nothing in it (deleted outright; any other
    /// goes to Deleted whole).
    pub fn folder_is_bare(&self, folder: &str) -> bool {
        self.notes.folder_is_bare(folder)
    }

    /// `folder` and those inside it gone, their notes to Deleted: what
    /// went, as JSON, `{folders, notes}`, for [`Vault::put_back_folder`].
    pub fn delete_folder(&mut self, folder: &str) -> String {
        let (folders, notes) = self.notes.delete_folder(folder, &mut WebRandom);
        let list = |v: &[String]| v.iter().map(|s| json(s)).collect::<Vec<_>>().join(",");
        format!(r#"{{"folders":[{}],"notes":[{}]}}"#, list(&folders), list(&notes))
    }

    /// A folder deleted, put back with its notes.
    pub fn put_back_folder(&mut self, folders: Vec<String>, notes: Vec<String>) -> bool {
        self.notes.put_back_folder(&folders, &notes, &mut WebRandom)
    }

    /// The note into `folder` ("" for the top), its pictures still shown.
    pub fn move_note(&mut self, key: &str, folder: &str) -> bool {
        self.notes.move_note(key, folder, &mut WebRandom)
    }

    /// The note pinned to the top of the list, or not.
    pub fn set_pinned(&mut self, key: &str, pinned: bool) -> bool {
        self.notes.set_pinned(key, pinned, &mut WebRandom)
    }

    /// Where a picture is, as a path from the notes folder, for a note in
    /// `folder`: its `src`, and whether an Obsidian embed (`![[…]]`) names
    /// it, looked for among the vault's pictures. None for a web address,
    /// or one that climbs out.
    pub fn picture_path(&self, folder: &str, src: &str, wiki: bool) -> Option<String> {
        self.notes.picture_path(folder, src, wiki)
    }

    /// The id of the picture at `path`, or none.
    pub fn picture_id(&self, path: &str) -> Option<String> {
        self.notes.picture_id(path)
    }

    /// Asks for the picture at `path`; its id, or none.
    pub fn get_picture(&mut self, path: &str) -> Option<String> {
        self.notes.get_picture(path)
    }

    pub fn save(&self) -> Vec<u8> {
        self.notes.save()
    }

    /// Up to `most` notes changed since last kept made ready to keep, a
    /// few at a time ahead of [`Vault::save`]: how many are still to do.
    pub fn keep_some(&self, most: u32) -> u32 {
        self.notes.keep_some(most as usize) as u32
    }

    /// A name for a new picture in the attachments folder.
    pub fn picture_free(&self, stem: &str, ext: &str) -> String {
        self.notes.picture_free(stem, ext)
    }

    /// A picture put in here at `path`: sent, then put in the vault (said
    /// as `uploaded`). Its id; none for one too big.
    pub fn upload(&mut self, path: &str, bytes: &[u8]) -> Option<String> {
        self.notes.upload(path, bytes.to_vec())
    }

    /// The pictures put in here and not on the server yet, by id.
    pub fn uploading(&self) -> Vec<String> {
        self.notes.uploading()
    }

    /// Why the pictures waiting wait, when it's something that won't mend
    /// by itself, in words; none else.
    pub fn pictures_held(&self) -> Option<String> {
        self.notes.pictures_held().map(str::to_string)
    }

    /// The pictures waiting, offered again.
    pub fn retry_pictures(&mut self) {
        self.notes.retry_pictures();
    }

    /// Every picture the vault names, by id: what is worth keeping here.
    pub fn picture_ids(&self) -> Vec<String> {
        self.notes.picture_ids()
    }

    /// This browser's device id, in hex.
    pub fn device(&self) -> String {
        hex(&self.notes.me.device)
    }

    /// What else happened since last asked, as JSON: `[{"devices":
    /// [{id, name, seen, you, online}]} | {"code": "47-381095"} | {"paired":
    /// id} | {"pairFailed": why} | {"recoverySet": true} | {"rekeyed":
    /// true} | {"removed": true, "account": bool} | {"invites": {owner,
    /// waiting: [{hash, until, code}], accounts: [{id, owner, made, seen,
    /// devices, you}]}} | {"passkeys": [{id, name, credential, made}]} |
    /// {"passkeySet": true}]`, ids and credentials in hex.
    pub fn take_said(&mut self) -> String {
        let items: Vec<String> = std::mem::take(&mut self.notes.said)
            .into_iter()
            .map(|s| match s {
                Said::Devices(list) => {
                    let rows: Vec<String> = list
                        .iter()
                        .map(|d| {
                            format!(
                                r#"{{"id":{},"name":{},"seen":{},"you":{},"online":{}}}"#,
                                json(&hex(&d.id)),
                                d.name.as_deref().map_or("null".into(), json),
                                d.seen,
                                d.you,
                                d.online
                            )
                        })
                        .collect();
                    format!(r#"{{"devices":[{}]}}"#, rows.join(","))
                }
                Said::PairCode(code) => format!(r#"{{"code":{}}}"#, json(&code)),
                Said::Paired(device) => format!(r#"{{"paired":{}}}"#, json(&hex(&device))),
                Said::PairFailed(why) => format!(r#"{{"pairFailed":{}}}"#, json(&why)),
                Said::RecoverySet => r#"{"recoverySet":true}"#.into(),
                Said::Rekeyed => r#"{"rekeyed":true}"#.into(),
                Said::MoreToCome => r#"{"moreToCome":true}"#.into(),
                Said::Removed { account } => format!(r#"{{"removed":true,"account":{account}}}"#),
                Said::Outdated { message, syncing } => format!(r#"{{"outdated":{},"syncing":{syncing}}}"#, json(message)),
                Said::Invites { owner, waiting, accounts } => {
                    let waiting: Vec<String> = waiting
                        .iter()
                        .map(|(i, code)| format!(r#"{{"hash":{},"until":{},"code":{}}}"#, json(&hex(&i.hash)), i.until, code.as_deref().map_or("null".into(), json)))
                        .collect();
                    let accounts: Vec<String> = accounts
                        .iter()
                        .map(|a| {
                            format!(
                                r#"{{"id":{},"owner":{},"made":{},"seen":{},"devices":{},"you":{}}}"#,
                                json(&hex(&a.id)),
                                a.owner,
                                a.made,
                                a.seen,
                                a.devices,
                                a.you
                            )
                        })
                        .collect();
                    format!(r#"{{"invites":{{"owner":{owner},"waiting":[{}],"accounts":[{}]}}}}"#, waiting.join(","), accounts.join(","))
                }
                Said::Uploaded(id) => format!(r#"{{"uploaded":{}}}"#, json(&id)),
                Said::Password { allowed, id, only } => {
                    let or_null = |s: Option<&str>| s.map_or("null".into(), json);
                    format!(r#"{{"password":{{"allowed":{allowed},"id":{},"only":{}}}}}"#, or_null(id.as_deref()), or_null(only.as_deref()))
                }
                Said::PasswordSet => r#"{"passwordSet":true}"#.to_string(),
                Said::Passkeys(list) => {
                    let rows: Vec<String> = list
                        .iter()
                        .map(|p| {
                            format!(
                                r#"{{"id":{},"name":{},"credential":{},"made":{}}}"#,
                                json(&hex(&p.id)),
                                p.name.as_deref().map_or("null".into(), json),
                                json(&hex(&p.credential)),
                                p.made
                            )
                        })
                        .collect();
                    format!(r#"{{"passkeys":[{}]}}"#, rows.join(","))
                }
                Said::PasskeySet => r#"{"passkeySet":true}"#.to_string(),
                Said::Writable => r#"{"writable":true}"#.to_string(),
                Said::ReadOnly => r#"{"readOnly":true}"#.to_string(),
                Said::Usage(u) => format!(
                    r#"{{"usage":{{"notes":{},"pictures":{},"limit":{},"writable":{},"plan":{},"until":{}}}}}"#,
                    u.notes,
                    u.pictures,
                    u.limit.map_or("null".into(), |l| l.to_string()),
                    u.writable,
                    json(&u.plan),
                    u.until.map_or("null".into(), |t| t.to_string())
                ),
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// Sets (or changes) the password, signing in as `email`: said when
    /// done (`passwordSet`), or refused.
    pub fn set_password(&mut self, email: &str, password: &str) -> Result<(), JsError> {
        self.notes.session.set_password(email, password, &mut WebRandom).map_err(|e| JsError::new(&e))
    }

    /// No password any more.
    pub fn drop_password(&mut self) {
        self.notes.session.drop_password();
    }

    /// The whole account deleted: everything the server keeps of it, every
    /// device let go (this one too: said as `removed`).
    pub fn delete_account(&mut self) {
        self.notes.session.delete_account();
    }

    /// A passkey made in this browser added to the account: its name, its
    /// credential id and its PRF output, as the browser gave them. Said
    /// when kept (`passkeySet`), or refused.
    pub fn add_passkey(&mut self, name: &str, credential: &[u8], prf: Vec<u8>) -> Result<(), JsError> {
        let prf = zeroize::Zeroizing::new(prf);
        self.notes.add_passkey(name, credential, &prf, &mut WebRandom).map_err(|e| JsError::new(&e))
    }

    /// The passkey (its id in hex) taken away from the account.
    pub fn remove_passkey(&mut self, id: &str) -> Result<(), JsError> {
        let id = unhex(id).and_then(|h| <[u8; 32]>::try_from(h).ok()).ok_or_else(|| JsError::new("not a passkey's id"))?;
        self.notes.remove_passkey(&id);
        Ok(())
    }

    /// Asks what the account keeps, and may: it comes in [`Vault::take_said`].
    /// Whether the server said it can do `feature` (`compat::feature`).
    pub fn server_has(&self, feature: &str) -> bool {
        self.notes.session.server_has(feature)
    }

    pub fn usage(&mut self) {
        self.notes.usage();
    }

    /// Asks for the account's devices: they come in [`Vault::take_said`].
    pub fn list_devices(&mut self) {
        self.notes.list_devices();
    }

    /// Takes the device (its id in hex) out of the account; this browser's
    /// own id, and it leaves. True if waiting invites were taken back with
    /// it (another device removed had their codes).
    pub fn remove_device(&mut self, device: &str) -> Result<bool, JsError> {
        let id = unhex(device).ok_or_else(|| JsError::new("not a device id"))?;
        self.notes.remove_device(&id, &mut WebRandom).map_err(|e| JsError::new(&e))
    }

    /// What this browser is called in the others' lists, changed now.
    pub fn rename(&mut self, name: &str) {
        self.notes.rename(name, &mut WebRandom);
    }

    /// Whether this account is the server's owner's, and its invites and
    /// accounts: they come in [`Vault::take_said`].
    pub fn list_invites(&mut self) {
        self.notes.list_invites();
    }

    /// A new invite, for someone's own account on this server: its code.
    pub fn make_invite(&mut self) -> Option<String> {
        self.notes.make_invite(&mut WebRandom)
    }

    /// An invite not used yet (its hash in hex), taken back.
    pub fn drop_invite(&mut self, hash: &str) -> Result<(), JsError> {
        let hash = unhex(hash).and_then(|h| <[u8; 32]>::try_from(h).ok()).ok_or_else(|| JsError::new("not an invite's hash"))?;
        self.notes.drop_invite(hash, &mut WebRandom);
        Ok(())
    }

    /// Another person's account (its id in hex) taken off the server.
    pub fn remove_account(&mut self, account: &str) -> Result<(), JsError> {
        let id = unhex(account).ok_or_else(|| JsError::new("not an account id"))?;
        self.notes.remove_account(&id);
        Ok(())
    }

    /// Opens a pairing: the code comes in [`Vault::take_said`].
    pub fn pair_open(&mut self) {
        self.notes.pair_open(&mut WebRandom);
    }

    pub fn pair_cancel(&mut self) {
        self.notes.pair_cancel();
    }

    /// Whether the account has a recovery code, by the last list of
    /// devices; not known yet (no list come): taken to.
    pub fn has_recovery(&self) -> bool {
        self.notes.session.has_recovery() != Some(false)
    }

    /// Whether the account has a recovery code, by the last list of
    /// devices: `undefined` until a list has come.
    pub fn recovery(&self) -> Option<bool> {
        self.notes.session.has_recovery()
    }

    /// A new recovery code: its twelve words, to show once the server says
    /// it keeps it (`recoverySet`).
    pub fn make_recovery(&mut self) -> Option<String> {
        self.notes.make_recovery(&mut WebRandom)
    }

    /// A note's versions, newest first, as JSON: `[{version, time, what,
    /// by, added, removed}]`, `what` as the desktop's history names it,
    /// `by` where (`"here"`, a device's id, or null when it didn't say),
    /// and the words each added and took away.
    pub fn history(&self, key: &str) -> String {
        let items: Vec<String> = self
            .notes
            .history(key)
            .iter()
            .map(|p| {
                // Where: here, or on a device by id, for the page to name
                // from the list of devices.
                let by = match (&p.by, p.mine) {
                    _ if p.origin == Some(Origin::Import) => "null".to_string(),
                    (_, true) => json("here"),
                    (Some(d), false) => json(&hex(d)),
                    (None, false) => "null".to_string(),
                };
                format!(
                    r#"{{"version":{},"time":{},"what":{},"by":{},"added":{},"removed":{}}}"#,
                    json(&p.version),
                    p.time,
                    json(what(p.origin)),
                    by,
                    p.added,
                    p.removed
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// The note at `version`, what changed since the one before marked.
    pub fn past_html(&self, key: &str, version: &str) -> Option<String> {
        self.notes.past_html(key, version)
    }

    /// The note made what it was at `version`, by a new change.
    pub fn restore(&mut self, key: &str, version: &str) -> bool {
        self.notes.restore(key, version, &mut WebRandom)
    }
}

/// A note being written in (the desktop's own page, [`editor::Editor`]):
/// CodeMirror shows its words, and each edit comes here.
#[wasm_bindgen]
pub struct PageEditor {
    e: editor::Editor,
}

#[wasm_bindgen]
impl PageEditor {
    #[wasm_bindgen(constructor)]
    pub fn new(md: &str) -> PageEditor {
        PageEditor { e: editor::Editor::open(md) }
    }

    /// The note as Markdown, as the desktop writes it.
    pub fn markdown(&self) -> String {
        self.e.markdown()
    }

    /// The note became `md` (merged with an edit from elsewhere).
    pub fn reload(&mut self, md: &str) {
        self.e.reload(md);
    }

    /// The words CodeMirror shows.
    pub fn text(&self) -> String {
        self.e.text()
    }

    /// How each line is drawn, as JSON: `[{k: kind, f: first of its block,
    /// i: indent, d: done, n: number, r: [[from, to, flags, mark, ink]],
    /// src, wk: the src an Obsidian embed's, alt, w, fl: a picture's place
    /// (0 its own line, 1 left, 2 right), cells: [[{t: words, r: runs, p:
    /// [[at, src, alt, width, wk]] its pictures, each the letter at `at`
    /// of its words}]], hd: a table's first row its header, ini: a
    /// paragraph's first line whose first letter drops, 1, or 2 when the
    /// letter has a colour of its own}]`. Flags: 1 bold, 2 italic, 4
    /// struck, 8 code, 16 link, 32 a cell's picture; colours 0xRRGGBBAA,
    /// a mark of 1 the plain highlighter.
    pub fn looks(&self) -> String {
        let n = self.e.lines().len();
        let items: Vec<String> = (0..n)
            .map(|i| {
                let l = self.e.look(i);
                let runs: Vec<String> = l
                    .runs
                    .iter()
                    .map(|(a, b, s)| {
                        let flags = u8::from(s.bold) | u8::from(s.italic) << 1 | u8::from(s.strike) << 2 | u8::from(s.code) << 3 | u8::from(s.link) << 4;
                        format!("[{a},{b},{flags},{},{}]", s.mark, s.ink)
                    })
                    .collect();
                let cells: Vec<String> = l
                    .cells
                    .iter()
                    .map(|row| {
                        let cells: Vec<String> = row
                            .iter()
                            .map(|cell| {
                                let runs: Vec<String> = cell
                                    .runs
                                    .iter()
                                    .map(|(a, b, s)| {
                                        let flags = u8::from(s.bold)
                                            | u8::from(s.italic) << 1
                                            | u8::from(s.strike) << 2
                                            | u8::from(s.code) << 3
                                            | u8::from(s.link) << 4
                                            | u8::from(s.picture) << 5;
                                        format!("[{a},{b},{flags},{},{}]", s.mark, s.ink)
                                    })
                                    .collect();
                                let pictures: Vec<String> = cell
                                    .pictures
                                    .iter()
                                    .map(|p| format!("[{},{},{},{},{}]", p.at, json(&p.src), json(&p.alt), p.width.map_or("null".into(), |w| w.to_string()), p.wiki))
                                    .collect();
                                format!(r#"{{"t":{},"r":[{}],"p":[{}]}}"#, json(&cell.words), runs.join(","), pictures.join(","))
                            })
                            .collect();
                        format!("[{}]", cells.join(","))
                    })
                    .collect();
                format!(
                    r#"{{"k":"{}","f":{},"i":{},"d":{},"n":{},"r":[{}],"src":{},"wk":{},"alt":{},"w":{},"fl":{},"cells":[{}],"hd":{},"ini":{}}}"#,
                    l.kind,
                    l.first,
                    l.indent,
                    l.done,
                    l.number,
                    runs.join(","),
                    json(&l.src),
                    l.wiki,
                    json(&l.alt),
                    l.width.map_or("null".into(), |w| w.to_string()),
                    float_number(l.float),
                    cells.join(","),
                    l.head,
                    l.initial
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// The selection: `[anchor line, column, head line, column]`.
    pub fn caret(&self) -> Vec<u32> {
        let (a, b, c, d) = self.e.caret_lc();
        vec![a as u32, b as u32, c as u32, d as u32]
    }

    pub fn select(&mut self, a_line: u32, a_col: u32, h_line: u32, h_col: u32) {
        self.e.select(a_line as usize, a_col as usize, h_line as usize, h_col as usize);
    }

    pub fn input(&mut self, s: &str) -> bool {
        self.e.input(s)
    }

    /// A chosen picture or divider made room for an input method's words.
    pub fn make_room(&mut self) -> bool {
        self.e.make_room()
    }

    pub fn enter(&mut self, soft: bool) -> bool {
        self.e.enter(soft)
    }

    pub fn backspace(&mut self, word: bool) -> bool {
        self.e.backspace(word)
    }

    pub fn delete(&mut self, word: bool) -> bool {
        self.e.delete(word)
    }

    pub fn tab(&mut self, back: bool) -> bool {
        self.e.tab(back)
    }

    /// The caret's blocks turned (0 text, 1–3 headings, 4 bullets, 5
    /// numbers, 6 checklist, 7 quote), or back to text if they are that.
    pub fn turn(&mut self, to: u32) -> bool {
        self.e.turn(u64::from(to))
    }

    pub fn turned_to(&self, to: u32) -> bool {
        self.e.turned_to(u64::from(to))
    }

    /// 0 bold, 1 italic, 2 struck, 3 code.
    pub fn style(&mut self, which: u32) -> bool {
        let kind = [inline::Kind::Bold, inline::Kind::Italic, inline::Kind::Strike, inline::Kind::Code][(which as usize).min(3)];
        self.e.style(kind)
    }

    /// A colour (`ink`) or a highlighter, 0xRRGGBBAA (1: the plain one).
    pub fn tint(&mut self, ink: bool, color: u32) -> bool {
        self.e.tint(ink, color)
    }

    /// The style typing would have at the caret, as `looks` flags, with
    /// 32 for highlighted.
    pub fn style_here(&self) -> u32 {
        let s = self.e.style_here();
        u32::from(s.bold) | u32::from(s.italic) << 1 | u32::from(s.strike) << 2 | u32::from(s.code) << 3 | u32::from(s.link) << 4 | u32::from(s.mark != 0) << 5
    }

    pub fn toggle(&mut self, line: u32) -> bool {
        self.e.toggle(line as usize)
    }

    /// The address of the link the caret is in.
    pub fn link_here(&self) -> Option<String> {
        self.e.link_here()
    }

    /// Where the link the caret is in opens: none for one a page mustn't
    /// follow (`javascript:` and the like).
    pub fn link_to_open(&self) -> Option<String> {
        self.e.link_to_open()
    }

    /// The caret put in a table's cell: CodeMirror's line, the row, the
    /// column, and the selection in the cell's words (UTF-16).
    pub fn cell_select(&mut self, line: u32, r: u32, c: u32, a: u32, h: u32) -> bool {
        self.e.cell_select(line as usize, r as usize, c as usize, a as usize, h as usize)
    }

    /// ← (`right` false) or → in a table's cell beside a picture, or
    /// with one chosen: the page's move, which chooses the picture before
    /// it passes it. False, nothing done, for any other: the browser's.
    pub fn cell_arrow(&mut self, right: bool) -> bool {
        self.e.cell_arrow(right)
    }

    /// Where the caret is, when in a cell: `[line, row, column, anchor,
    /// head]`; none otherwise.
    pub fn cell_caret(&self) -> Option<Vec<u32>> {
        self.e.cell_caret().map(|(l, r, c, a, h)| vec![l as u32, r as u32, c as u32, a as u32, h as u32])
    }

    /// 0 a row below, 1 a column after, 2 the row out, 3 the column out,
    /// 4 the header row out, or back.
    pub fn table_op(&mut self, op: u8) -> bool {
        self.e.table_op(op)
    }

    /// Whether the caret's table has a header row.
    pub fn table_header(&self) -> bool {
        self.e.table_header()
    }

    pub fn remove_table(&mut self) -> bool {
        self.e.remove_table()
    }

    /// The table sorted by the caret's column, A to Z; again, Z to A.
    pub fn sort_table(&mut self) -> bool {
        self.e.sort_table()
    }

    /// How the caret's column was last sorted: 0 not, 1 A to Z, 2 Z to A.
    pub fn sorted(&self) -> u8 {
        match self.e.sorted() {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        }
    }

    pub fn insert_table(&mut self) -> bool {
        self.e.insert_table()
    }

    pub fn insert_divider(&mut self) -> bool {
        self.e.insert_divider()
    }

    /// A picture (`src` as the note links it) on its own line below the
    /// caret's block.
    pub fn insert_picture(&mut self, src: &str) -> bool {
        self.e.insert_picture(src)
    }

    /// The one picture the caret is on, or selects: `[line, place (0 its
    /// own line, 1 left, 2 right), width (0: its own)]`.
    pub fn picture_here(&self) -> Option<Vec<u32>> {
        self.e.picture_here().map(|(line, float, width)| vec![line as u32, float_number(float), width.unwrap_or(0)])
    }

    /// The caret's picture `width` wide (`undefined`: its own).
    pub fn set_picture_width(&mut self, width: Option<u32>) -> bool {
        self.e.set_picture_width(width)
    }

    /// The caret's picture on its own line (0), left (1) or right (2) of
    /// the text; `toggle` (a shortcut), back on its own line if there.
    pub fn set_picture_float(&mut self, to: u32, toggle: bool) -> bool {
        let to = match to {
            1 => carnet_core::doc::Float::Left,
            2 => carnet_core::doc::Float::Right,
            _ => carnet_core::doc::Float::None,
        };
        self.e.set_picture_float(to, toggle)
    }

    pub fn remove_picture(&mut self) -> bool {
        self.e.remove_picture()
    }

    /// The picture on line `from` carried to before the block on line
    /// `before` (past the last line: the end).
    pub fn move_picture_to_line(&mut self, from: u32, before: u32) -> bool {
        self.e.move_picture(from as usize, PicTo::Line(before as usize))
    }

    /// The picture on line `from` carried beside the one on line `to`,
    /// `after` it or before.
    pub fn move_picture_beside(&mut self, from: u32, to: u32, after: bool) -> bool {
        self.e.move_picture(from as usize, PicTo::Beside(to as usize, after))
    }

    /// The picture on line `from` carried into a table's cell (its line,
    /// row, column), at UTF-16 column `col` of its words (`undefined`:
    /// their end): its line gone, its width kept, its side dropped.
    pub fn move_picture_into_cell(&mut self, from: u32, line: u32, r: u32, c: u32, col: Option<u32>) -> bool {
        self.e.move_picture(from as usize, PicTo::Cell(line as usize, r as usize, c as usize, col.map(|n| n as usize)))
    }

    /// The picture whose letter is at UTF-16 column `at` of a cell's words
    /// (its line, row, column) carried into a cell (the same or another),
    /// at UTF-16 column `col` of its words (`undefined`: their end).
    #[allow(clippy::too_many_arguments)]
    pub fn move_cell_picture_to_cell(&mut self, line: u32, r: u32, c: u32, at: u32, to_line: u32, to_r: u32, to_c: u32, col: Option<u32>) -> bool {
        let to = PicTo::Cell(to_line as usize, to_r as usize, to_c as usize, col.map(|n| n as usize));
        self.e.move_cell_picture(line as usize, r as usize, c as usize, at as usize, to)
    }

    /// A cell's picture (as [`Self::move_cell_picture_to_cell`] names it)
    /// carried out of the table: on a line of its own before the block on
    /// line `before`, its width kept.
    pub fn move_cell_picture_out(&mut self, line: u32, r: u32, c: u32, at: u32, before: u32) -> bool {
        self.e.move_cell_picture(line as usize, r as usize, c as usize, at as usize, PicTo::Line(before as usize))
    }

    /// A cell's picture carried out of the table beside the picture on
    /// line `to`, `after` it or before.
    pub fn move_cell_picture_beside(&mut self, line: u32, r: u32, c: u32, at: u32, to: u32, after: bool) -> bool {
        self.e.move_cell_picture(line as usize, r as usize, c as usize, at as usize, PicTo::Beside(to as usize, after))
    }

    /// The caret's link given `url`, or taken off (`undefined`); else the
    /// selection made a link.
    pub fn set_link(&mut self, url: Option<String>) -> bool {
        self.e.set_link(url.as_deref())
    }

    /// The words' colour and the highlighter at the caret (0xRRGGBBAA, 0
    /// none, 1 the plain highlighter): `[ink, mark]`.
    pub fn tint_here(&self) -> Vec<u32> {
        let s = self.e.style_here_full();
        vec![s.ink, s.mark]
    }

    /// The selection as Markdown.
    pub fn copy(&self) -> Option<String> {
        self.e.copy()
    }

    /// The selection as HTML, for pasting into other apps.
    pub fn copy_html(&self) -> Option<String> {
        self.e.copy_for_html().map(|md| carnet_core::export::html(&md, |s, _| s.to_string()))
    }

    pub fn paste(&mut self, s: &str) -> bool {
        self.e.paste(s)
    }

    pub fn undo(&mut self) -> bool {
        self.e.undo()
    }

    pub fn redo(&mut self) -> bool {
        self.e.redo()
    }
}

fn float_number(f: carnet_core::doc::Float) -> u32 {
    match f {
        carnet_core::doc::Float::None => 0,
        carnet_core::doc::Float::Left => 1,
        carnet_core::doc::Float::Right => 2,
    }
}

/// How wide a picture `natural` pixels wide is drawn, as the desktop draws
/// it ([`page::picture_width`]): `[width, the width it has with none
/// given]`; `given` 0 for none.
#[wasm_bindgen]
pub fn picture_width(natural: f32, given: u32, text_w: f32, max: f32, floated: bool) -> Vec<f32> {
    let (w, fit) = page::picture_width(natural, (given > 0).then_some(given), text_w, max, floated);
    vec![w, fit]
}

/// A picture's width while its handle is dragged `dx` outwards.
#[wasm_bindgen]
pub fn dragged_width(from: f32, dx: f32, max: f32) -> f32 {
    page::dragged_width(from, dx, max)
}

/// The width a resize let go at `w` gives a picture (`undefined`: none,
/// back at its own `fit`).
#[wasm_bindgen]
pub fn resized_width(w: f32, fit: f32) -> Option<u32> {
    page::resized_width(w, fit)
}

/// The widths a picture is given from a menu, as JSON: `[[name, percent
/// of the text's width]]`.
#[wasm_bindgen]
pub fn picture_sizes() -> String {
    let all: Vec<String> = page::PICTURE_SIZES.iter().map(|(name, pct)| format!("[{},{pct}]", json(name))).collect();
    format!("[{}]", all.join(","))
}

/// What made a version, as a row in the history says it; where (here, or
/// on which device) the page adds.
fn what(o: Option<Origin>) -> &'static str {
    match o {
        Some(Origin::Import) => "Where its history starts",
        Some(Origin::Save) => "Edited",
        Some(Origin::OtherApp) => "Changed in another app",
        Some(Origin::Restore) => "Restored",
        None => "Changed",
    }
}

/// The pairing code in a link to the viewer at `origin`: what a QR code
/// carries.
#[wasm_bindgen]
pub fn pair_link(origin: &str, code: &str) -> Option<String> {
    PairCode::parse(code).map(|c| c.link(origin))
}

/// The code in a scanned QR code or a pasted link (or the code itself),
/// as it is typed.
#[wasm_bindgen]
pub fn code_from_link(text: &str) -> Option<String> {
    PairCode::from_link(text).map(|c| c.text())
}

/// Whether `text` is an invite (a new account), as typed or in a link
/// (`…/#invite=ABCD-…`): its code, written as it should be.
#[wasm_bindgen]
pub fn invite_from(text: &str) -> Option<String> {
    let code = text.rsplit_once("#invite=").map_or(text, |(_, c)| c);
    InviteCode::parse(code.trim()).map(|c| c.text())
}

/// What's wrong with a password about to be set for `email`, said
/// plainly, before anything is sent (the session refuses the same:
/// `Session::set_password`); `None` when it may be set.
#[wasm_bindgen]
pub fn password_fault(email: &str, password: &str) -> Option<String> {
    let id = carnet_sync::password::id(email);
    if !id.contains('@') || id.len() > 254 {
        return Some("That doesn’t look like an email address.".into());
    }
    let min = carnet_sync::password::MIN_LEN;
    let n = password.chars().count();
    (n < min).then(|| format!("A password needs {min} letters at least: this one has {n}."))
}

/// `text` as a QR code: an SVG drawn in `currentColor`, its quiet zone
/// included.
#[wasm_bindgen]
pub fn qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let n = code.width();
    let q = 4;
    let mut path = String::new();
    for (i, c) in code.to_colors().iter().enumerate() {
        if *c == qrcode::Color::Dark {
            path.push_str(&format!("M{} {}h1v1h-1z", i % n + q, i / n + q));
        }
    }
    let side = n + 2 * q;
    Some(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {side} {side}" shape-rendering="crispEdges"><path fill="currentColor" d="{path}"/></svg>"#))
}


/// A string as JSON.
fn json(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::{json, password_fault};

    #[test]
    fn a_short_password_is_said_before_it_is_sent() {
        assert_eq!(password_fault("me@example.com", "ninechars").as_deref(), Some("A password needs 10 letters at least: this one has 9."));
        assert_eq!(password_fault("me@example.com", "tenletters"), None);
        // Letters, not bytes.
        assert_eq!(password_fault("me@example.com", "ééééééééé").as_deref(), Some("A password needs 10 letters at least: this one has 9."));
        assert_eq!(password_fault("not an email", "tenletters").as_deref(), Some("That doesn’t look like an email address."));
    }

    #[test]
    fn strings_are_json() {
        assert_eq!(json("a \"q\" \\ b\nc\u{1}"), r#""a \"q\" \\ b\nc\u0001""#);
    }
}
