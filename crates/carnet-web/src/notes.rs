//! The viewer's state, in plain Rust (so it is tested natively): this
//! device's identity, its session, and every doc it has: the notes and the
//! vault. What lib.rs binds for the browser.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use carnet_sync::crypto::BlobId;

use carnet_core::export::Picture;
use carnet_core::{compare, doc, name};
use carnet_sync::crypto::{DeviceKey, Random, RecoveryCode, Ring};
use carnet_sync::invite::InviteCode;
use carnet_sync::session::{DeviceInfo, Docs, Event, Identity, PasskeyInfo, Session, Transfer};
use carnet_sync::wire::{AccountRow, InviteRow};
use carnet_sync::vault::{self, Place};
use carnet_sync::{Key, NoteDoc, Origin, Version};

/// Who this browser is: what it logs in with.
#[derive(Clone)]
pub struct Me {
    pub account: Vec<u8>,
    pub device: Vec<u8>,
    pub key: DeviceKey,
    pub ring: Ring,
}

fn put(out: &mut Vec<u8>, b: &[u8]) {
    out.extend((b.len() as u32).to_be_bytes());
    out.extend(b);
}

fn take<'a>(b: &mut &'a [u8]) -> Option<&'a [u8]> {
    let (n, rest) = b.split_at_checked(4)?;
    let n = u32::from_be_bytes(n.try_into().ok()?) as usize;
    let (v, rest) = rest.split_at_checked(n)?;
    *b = rest;
    Some(v)
}

impl Me {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = b"CRW1".to_vec();
        for part in [&self.account[..], &self.device, &self.key.secret(), &self.ring.to_bytes()] {
            put(&mut out, part);
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Me> {
        let mut b = b.strip_prefix(b"CRW1".as_slice())?;
        let account = take(&mut b)?.to_vec();
        let device = take(&mut b)?.to_vec();
        let key = DeviceKey::from_secret(take(&mut b)?.try_into().ok()?);
        let ring = Ring::from_bytes(take(&mut b)?)?;
        Some(Me { account, device, key, ring })
    }
}

/// The docs this browser has: by room, each made as it is first asked for.
pub struct Rooms {
    pub peer: u64,
    pub docs: BTreeMap<String, NoteDoc>,
    /// This browser's device id: each change written here names it.
    pub author: Vec<u8>,
}

impl Rooms {
    fn made(&self) -> NoteDoc {
        let d = NoteDoc::new(self.peer);
        d.set_author(Some(&self.author));
        d
    }
}

impl Docs for Rooms {
    fn rooms(&self) -> Vec<String> {
        let mut r: Vec<String> = self.docs.keys().cloned().collect();
        // The vault first, as the desktop joins it.
        r.sort_by_key(|k| k != vault::ROOM);
        r
    }

    fn doc(&mut self, room: &str) -> &NoteDoc {
        if !self.docs.contains_key(room) {
            let d = self.made();
            self.docs.insert(room.to_string(), d);
        }
        &self.docs[room]
    }
}

/// A note as the library lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub key: String,
    pub title: String,
    pub folder: String,
    pub pinned: bool,
    pub trashed: bool,
    pub snippet: String,
    /// Checkboxes ticked, and in all.
    pub done: usize,
    pub todos: usize,
    /// Its last change, in seconds since 1970.
    pub modified: i64,
}

/// What happened beside the notes, for the page to show.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// The account's devices, as asked for: this one first.
    Devices(Vec<DeviceInfo>),
    /// A pairing opened: the code for the new device.
    PairCode(String),
    /// A new device came in by the code: its id.
    Paired(Vec<u8>),
    /// The pairing ended with no device let in, and why.
    PairFailed(String),
    /// The recovery code is set on the server.
    RecoverySet,
    /// A new key came (a device was removed): the identity is to be kept
    /// again, or a reload would lose it.
    Rekeyed,
    /// This browser was removed from the account, on another device; or
    /// (`account`) the whole account, by the server's owner.
    Removed { account: bool },
    /// The browser's app and the server don't get on (`compat`): the
    /// words to show, and whether syncing goes on meanwhile.
    Outdated { message: &'static str, syncing: bool },
    /// Whether this account is the server's owner's, and if it is, the
    /// invites waiting (with the code of each made here) and every account.
    Invites { owner: bool, waiting: Vec<(InviteRow, Option<String>)>, accounts: Vec<AccountRow> },
    /// A picture put in here is on the server, and in the vault: its id.
    Uploaded(String),
    /// What the account keeps on the server, and may, as asked.
    Usage(carnet_sync::session::Usage),
    /// With the devices: whether the server takes passwords, the email the
    /// account's signs in as, and the only one it may (a hosted service:
    /// the email signed up with), if the server holds it to one.
    Password { allowed: bool, id: Option<String>, only: Option<String> },
    /// The password is set, changed, or gone, as asked.
    PasswordSet,
    /// With the devices: the account's passkeys, the oldest first.
    Passkeys(Vec<PasskeyInfo>),
    /// A passkey is added, or taken away, as asked.
    PasskeySet,
}

/// A version of a note, as its history lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Past {
    /// Its [`Version`], in hex: what the page names it by.
    pub version: String,
    /// When, in seconds since 1970.
    pub time: i64,
    pub origin: Option<Origin>,
    /// The device that made it, when it said; and whether that's this one.
    pub by: Option<Vec<u8>>,
    pub mine: bool,
    /// Words it added and took away, against the version before.
    pub added: usize,
    pub removed: usize,
}

/// What the library shows of a note, apart from its place: worked out
/// from its text, and kept until the note changes.
#[derive(Clone)]
struct Summary {
    title: Option<String>,
    snippet: String,
    done: usize,
    todos: usize,
    modified: i64,
    empty: bool,
}

impl Summary {
    fn of(d: &NoteDoc) -> Summary {
        let text = d.text();
        let (title, body) = doc::from_markdown(&text);
        let blocks = body.blocks();
        // As the desktop's library has it: the first words, plain.
        let snippet = body.snippet();
        let (mut done, mut todos) = (0, 0);
        for b in blocks {
            if let doc::Kind::Item { style: doc::ListStyle::Todo, done: d, .. } = b.kind {
                todos += 1;
                done += usize::from(d);
            }
        }
        Summary { title: title.filter(|t| !t.trim().is_empty()), snippet, done, todos, modified: d.modified(), empty: text.is_empty() }
    }

    /// The note's title: its words', or else the name it has in the vault
    /// (a desktop note's file name); none for one untitled.
    fn named<'a>(&'a self, place: &'a Place) -> Option<&'a str> {
        self.title.as_deref().or((!place.name.is_empty()).then_some(place.name.as_str()))
    }
}

pub struct Notes {
    pub me: Me,
    pub session: Session,
    pub rooms: Rooms,
    /// Rooms changed since last asked: the page draws them again.
    pub changed: Vec<String>,
    /// Pictures come, by id: the page shows them.
    pub pictures: Vec<([u8; 32], Vec<u8>)>,
    /// Pictures asked for that didn't come (the server hasn't one, or it
    /// didn't open, or the fetch failed): the page says so, and asks
    /// again when the notes change.
    pub missing: Vec<[u8; 32]>,
    /// What went wrong, for the page to say.
    pub errors: Vec<String>,
    /// What else happened, for the page to show.
    pub said: Vec<Said>,
    /// Each note's [`Summary`], by room, and the version it was made at:
    /// a change in one note works out only that one again.
    summaries: RefCell<HashMap<String, (Version, Summary)>>,
    /// Each doc as last kept, and its version then: [`Notes::save`]
    /// exports only the docs that changed since.
    kept: RefCell<HashMap<String, (Version, Vec<u8>)>>,
    /// Notes made here and not yet written, by room: the folder each goes
    /// in. Such a note reaches the vault (and the other devices) with its
    /// first words; left blank, it's dropped.
    fresh: HashMap<String, String>,
    /// Pictures put in here and not yet on the server, by id: where each
    /// goes (`attachments/…`) and its bytes. Offered once logged in, sent
    /// if the server lacks it, then put in the vault, so no device asks
    /// for a picture the server hasn't got.
    uploads: HashMap<BlobId, (String, Vec<u8>)>,
    /// Invites made here: the server keeps only their hashes, so only
    /// here can their codes be shown again.
    made: Vec<InviteCode>,
    /// Dropped pictures shown again, asked for by the id they had: the
    /// paths each is named at again when it comes.
    reviving: HashMap<BlobId, Vec<String>>,
}

impl Notes {
    /// With `state` as [`Notes::save`] left it (or none, the first time),
    /// brought up to this build's [`STATE`]. A newer build's is refused,
    /// for the page to say so rather than start again over it.
    pub fn open(origin: &str, me: Me, name: &str, state: &[u8], peer: u64) -> Result<Notes, Newer> {
        let body = state_body(state)?;
        let mut rooms = Rooms { peer, docs: BTreeMap::new(), author: me.device.clone() };
        let mut kept = HashMap::new();
        let mut cursor = 0;
        let mut forgotten = String::new();
        let mut b = body.as_deref().unwrap_or(&[]);
        if let Some(c) = take(&mut b) {
            cursor = c.try_into().map(u64::from_be_bytes).unwrap_or(0);
            while let (Some(room), Some(bytes)) = (take(&mut b), take(&mut b)) {
                if room == FORGOTTEN.as_bytes() {
                    forgotten = String::from_utf8_lossy(bytes).into_owned();
                    continue;
                }
                if let (Ok(room), Ok(doc)) = (std::str::from_utf8(room), NoteDoc::load(bytes, peer)) {
                    doc.set_author(Some(&me.device));
                    kept.insert(room.to_string(), (doc.version(), bytes.to_vec()));
                    rooms.docs.insert(room.to_string(), doc);
                }
            }
        }
        let id = Identity {
            origin: origin.to_string(),
            account: me.account.clone(),
            device: me.device.clone(),
            key: me.key.clone(),
            ring: me.ring.clone(),
            name: name.to_string(),
        };
        let mut notes = Notes { session: Session::new(id, cursor), me, rooms, changed: Vec::new(), pictures: Vec::new(), missing: Vec::new(), errors: Vec::new(), said: Vec::new(), summaries: RefCell::new(HashMap::new()), kept: RefCell::new(kept), fresh: HashMap::new(), uploads: HashMap::new(), made: Vec::new(), reviving: HashMap::new() };
        // Deleted forever, and pictures dropped, the server hasn't said
        // done: said again once in.
        for line in forgotten.lines() {
            match line.split_once(' ') {
                Some(("room", room)) => notes.session.remove_room(room),
                Some(("picture", id)) => {
                    if let Some(id) = unhex(id).and_then(|b| <[u8; 32]>::try_from(b).ok()) {
                        notes.session.drop_blobs(&[id]);
                    }
                }
                _ => {}
            }
        }
        notes.forget_gone();
        Ok(notes)
    }

    /// Everything kept: the cursor and every doc, after the header.
    pub fn save(&self) -> Vec<u8> {
        let mut out = header(STATE).to_vec();
        put(&mut out, &self.session.cursor().to_be_bytes());
        let mut kept = self.kept.borrow_mut();
        for (room, doc) in &self.rooms.docs {
            let v = doc.version();
            if kept.get(room).is_none_or(|(at, _)| *at != v) {
                kept.insert(room.clone(), (v, doc.to_bytes()));
            }
            put(&mut out, room.as_bytes());
            put(&mut out, &kept[room].1);
        }
        let (rooms, ids) = self.session.pending_removals();
        if !rooms.is_empty() || !ids.is_empty() {
            let text: String = rooms.iter().map(|r| format!("room {r}\n")).chain(ids.iter().map(|id| format!("picture {}\n", hex(id)))).collect();
            put(&mut out, FORGOTTEN.as_bytes());
            put(&mut out, text.as_bytes());
        }
        out
    }

    pub fn receive(&mut self, frame: &[u8], rng: &mut impl Random) {
        self.session.receive(frame, &mut self.rooms, rng);
        self.take_events(rng);
    }

    /// Pictures to put or fetch over HTTP, by the links the server gave.
    pub fn take_transfers(&mut self) -> Vec<Transfer> {
        self.session.take_transfers()
    }

    /// How a transfer went: a fetch's sealed bytes, or why not.
    pub fn transferred(&mut self, id: BlobId, put: bool, result: Result<Vec<u8>, String>, rng: &mut impl Random) {
        self.session.transferred(id, put, result);
        self.take_events(rng);
    }

    fn take_events(&mut self, rng: &mut impl Random) {
        for e in self.session.take_events() {
            match e {
                Event::Changed(room) => {
                    if room == vault::ROOM {
                        self.forget_gone();
                    }
                    if !self.changed.contains(&room) {
                        self.changed.push(room);
                    }
                }
                Event::BlobGot(id, bytes) => {
                    // Brought back: named again, for every device.
                    let paths = self.reviving.remove(&id).unwrap_or_default();
                    let vault = self.rooms.doc(vault::ROOM);
                    let mut changed = false;
                    for p in paths {
                        changed |= vault.set_picture(&p, &hex(&id));
                    }
                    if changed {
                        self.session.saved(vault::ROOM, &mut self.rooms, rng);
                    }
                    self.pictures.push((id, bytes));
                }
                Event::Rekeyed(_) => {
                    self.me.ring = self.session.ring().clone();
                    self.said.push(Said::Rekeyed);
                }
                // Taken out of the account elsewhere (as the desktop
                // reads it): its keys are no use now.
                Event::Removed { account } => self.said.push(Said::Removed { account }),
                Event::Outdated(fit) => match fit.refusal() {
                    Some(message) => self.said.push(Said::Outdated { message, syncing: false }),
                    None => self.said.push(Said::Outdated { message: carnet_sync::compat::UPDATE_SOON, syncing: true }),
                },
                Event::Invites { owner, invites, accounts } => {
                    let waiting = invites
                        .into_iter()
                        .map(|i| {
                            let code = self.made.iter().find(|c| c.hash() == i.hash).map(InviteCode::text);
                            (i, code)
                        })
                        .collect();
                    self.said.push(Said::Invites { owner, waiting, accounts });
                }
                Event::Denied(why) => self.errors.push(format!("The server turned this browser away: {why}")),
                Event::Refused(why) => self.errors.push(why),
                Event::Usage(u) => self.said.push(Said::Usage(u)),
                Event::Password { allowed, id, only } => self.said.push(Said::Password { allowed, id, only }),
                Event::PasswordSet => self.said.push(Said::PasswordSet),
                Event::Passkeys(list) => self.said.push(Said::Passkeys(list)),
                Event::PasskeySet => self.said.push(Said::PasskeySet),
                Event::ReadOnly => self.errors.push("This account is read only for now: what's written here stays here, and syncs once the account can write again.".into()),
                Event::Devices(mut list) => {
                    list.sort_by(|a, b| b.you.cmp(&a.you).then_with(|| a.name.cmp(&b.name)));
                    self.said.push(Said::Devices(list));
                }
                Event::PairCode(code) => self.said.push(Said::PairCode(code)),
                Event::Paired(device) => self.said.push(Said::Paired(device)),
                Event::PairFailed(why) => self.said.push(Said::PairFailed(why)),
                Event::RecoverySet => self.said.push(Said::RecoverySet),
                Event::LoggedIn => {
                    let ids: Vec<BlobId> = self.uploads.keys().copied().collect();
                    self.session.offer_blobs(ids);
                }
                Event::BlobWanted(id) => {
                    if let Some((_, bytes)) = self.uploads.get(&id) {
                        self.session.put_blob(bytes, rng);
                    }
                }
                Event::BlobStored(id) => {
                    if let Some((path, _)) = self.uploads.remove(&id) {
                        let vault = self.rooms.doc(vault::ROOM);
                        let (changed, unused) = vault.put_picture(&path, &hex(&id));
                        if changed {
                            self.session.saved(vault::ROOM, &mut self.rooms, rng);
                        }
                        let unused: Vec<BlobId> = unused.and_then(|u| unhex(&u)).and_then(|b| b.try_into().ok()).into_iter().collect();
                        self.session.drop_blobs(&unused);
                        self.said.push(Said::Uploaded(hex(&id)));
                    }
                }
                // One being put is offered again at the next login; one
                // asked for, the page is told of.
                Event::BlobFailed(id) if !self.uploads.contains_key(&id) => {
                    self.reviving.remove(&id);
                    self.missing.push(id);
                }
                _ => {}
            }
        }
    }

    fn place(&self, key: &str) -> Option<Place> {
        self.rooms.docs.get(vault::ROOM)?.place(Key::parse(key)?)
    }

    /// Each note's [`Summary`], by room: made again only for a note whose
    /// version moved since.
    fn summaries(&self) -> std::cell::RefMut<'_, HashMap<String, (Version, Summary)>> {
        let mut summaries = self.summaries.borrow_mut();
        for (room, d) in &self.rooms.docs {
            if Key::parse(room).is_none() {
                continue;
            }
            let v = d.version();
            if summaries.get(room).is_none_or(|(at, _)| *at != v) {
                summaries.insert(room.clone(), (v, Summary::of(d)));
            }
        }
        summaries
    }

    /// Every note this browser has: title, folder, pinned and so on, as the
    /// library lists them; those in the trash too, marked.
    pub fn notes(&self) -> Vec<Listed> {
        let places = self.rooms.docs.get(vault::ROOM).map(|v| v.layout().notes).unwrap_or_default();
        let summaries = self.summaries();
        let mut out = Vec::new();
        for room in self.rooms.docs.keys() {
            let Some(key) = Key::parse(room) else { continue };
            let s = &summaries[room].1;
            if s.empty {
                continue;
            }
            let place = places.get(&key).cloned().unwrap_or_default();
            let title = s.named(&place).unwrap_or("Untitled").to_string();
            out.push(Listed {
                key: room.clone(),
                title,
                folder: place.folder,
                pinned: place.pinned,
                trashed: place.trashed,
                snippet: s.snippet.clone(),
                done: s.done,
                todos: s.todos,
                modified: s.modified,
            });
        }
        out
    }

    /// The folders, as the vault has them, in the desktop's order.
    pub fn folders(&self) -> Vec<String> {
        let mut folders: Vec<String> = self.rooms.docs.get(vault::ROOM).map(|v| v.folders().into_iter().collect()).unwrap_or_default();
        // As the desktop lists them: by name, case aside.
        folders.sort_by_key(|f| f.to_lowercase());
        folders
    }

    /// A note as the page shows it. A picture the page fetches itself has
    /// no `src` but `data-picture="<path from the notes folder>"`: the
    /// browser asks for nothing before the page has it (pictures.ts).
    pub fn html(&self, key: &str) -> String {
        let Some(d) = self.rooms.docs.get(key) else { return String::new() };
        self.draw(key, &d.text())
    }

    /// `md`, a version of the note at `key`, as the page shows it.
    fn draw(&self, key: &str, md: &str) -> String {
        let folder = self.place(key).map(|p| p.folder).unwrap_or_default();
        let files = self.picture_files();
        carnet_core::export::viewer(md, |src, wiki| match find(&folder, src, wiki, &files) {
            Some(path) => Picture::Fetch(path),
            // On the web: as it is.
            None if ["http://", "https://", "data:"].iter().any(|p| src.starts_with(p)) => Picture::Url(src.to_string()),
            // A file outside the notes folder: only on the device that has
            // it, which the page says (pictures.ts), as it does of one the
            // vault doesn't name.
            None => Picture::Fetch(src.to_string()),
        })
    }

    /// The pictures the vault names: what an Obsidian embed's picture
    /// (`![[photo.png]]`) is looked for among.
    fn picture_files(&self) -> doc::Files {
        doc::Files::new(self.rooms.docs.get(vault::ROOM).map(|v| v.pictures().into_keys().collect::<Vec<_>>()).unwrap_or_default())
    }

    /// Where a picture is, as a path from the notes folder, for a note in
    /// `folder` ([`find`]).
    pub fn picture_path(&self, folder: &str, src: &str, wiki: bool) -> Option<String> {
        find(folder, src, wiki, &self.picture_files())
    }

    /// The note's `n`th checkbox ticked or unticked: a one-character edit,
    /// sent at once.
    pub fn toggle_task(&mut self, key: &str, n: usize, rng: &mut impl Random) -> bool {
        let Some(d) = self.rooms.docs.get(key) else { return false };
        let Some(text) = doc::toggle_task(&d.text(), n) else { return false };
        d.record(&text, carnet_sync::Origin::Save);
        self.session.saved(key, &mut self.rooms, rng);
        true
    }

    /// Notes whose title or words have `query` in them, without case, as
    /// the desktop's search has it: the title the library shows (a note
    /// from the desktop is titled by its file's name, which isn't in its
    /// words) and the words.
    pub fn search(&self, query: &str) -> Vec<String> {
        let q = query.to_lowercase();
        let places = self.rooms.docs.get(vault::ROOM).map(|v| v.layout().notes).unwrap_or_default();
        let summaries = self.summaries();
        self.rooms
            .docs
            .iter()
            .filter(|(r, d)| {
                let Some(key) = Key::parse(r) else { return false };
                let place = places.get(&key).cloned().unwrap_or_default();
                summaries[*r].1.named(&place).is_some_and(|t| t.to_lowercase().contains(&q)) || d.text().to_lowercase().contains(&q)
            })
            .map(|(r, _)| r.clone())
            .collect()
    }

    /// A note's Markdown, for the editor, and its version (in hex): what
    /// the next [`Notes::edit`] is made from.
    pub fn source(&self, key: &str) -> Option<(String, String)> {
        let d = self.rooms.docs.get(key)?;
        Some((d.text(), hex(&d.version().to_bytes())))
    }

    /// `text`, as typed from the note at `base` (a version from
    /// [`Notes::source`] or the last edit): what came from elsewhere since
    /// stays beside it, and the merged text comes back with its version,
    /// for the editor. Sent at once. A new title renames the note on every
    /// device; a new note goes into the vault with its first words.
    pub fn edit(&mut self, key: &str, base: &str, text: &str, rng: &mut impl Random) -> Option<(String, String)> {
        let k = Key::parse(key)?;
        let d = self.rooms.docs.get(key)?;
        let base = unhex(base).and_then(|b| Version::from_bytes(&b))?;
        let before = d.text();
        let merged = if d.version() == base {
            d.record(text, Origin::Save);
            d.text()
        } else {
            d.apply_from(&base, text, Origin::Save).ok()?
        };
        let version = hex(&d.version().to_bytes());
        let changed = merged != before;
        // Where it goes: placed with its first words, renamed with its title.
        let vault = self.rooms.doc(vault::ROOM);
        let wanted = name::for_text(&merged);
        let placed = match vault.place(k) {
            Some(mut p) if name::for_text(&before) != wanted => {
                p.name = wanted;
                vault.set_place(k, &p)
            }
            Some(_) => false,
            // Words, not a bare `# ` a new note starts with.
            None if !merged.trim().trim_start_matches('#').trim().is_empty() => match self.fresh.remove(key) {
                Some(folder) => vault.set_place(k, &Place { folder, name: wanted, pinned: false, trashed: false }),
                None => false,
            },
            None => false,
        };
        // The vault first: the other devices learn where it goes before
        // its words come.
        if placed {
            self.session.saved(vault::ROOM, &mut self.rooms, rng);
        }
        if changed {
            self.session.saved(key, &mut self.rooms, rng);
        }
        Some((merged, version))
    }

    /// A new note, blank, in `folder`: its key. Nothing is sent until it
    /// has words ([`Notes::edit`]); [`Notes::discard`] drops it before.
    pub fn create(&mut self, folder: &str) -> String {
        let key = Key::random().to_string();
        let d = self.rooms.made();
        self.rooms.docs.insert(key.clone(), d);
        self.fresh.insert(key.clone(), folder.to_string());
        key
    }

    /// A note made here and never written, dropped. False for any other.
    pub fn discard(&mut self, key: &str) -> bool {
        if self.fresh.remove(key).is_none() {
            return false;
        }
        self.rooms.docs.remove(key);
        true
    }

    /// Notes another device deleted forever, gone here too.
    fn forget_gone(&mut self) {
        let Some(vault) = self.rooms.docs.get(vault::ROOM) else { return };
        for key in vault.gone() {
            let room = key.to_string();
            if self.rooms.docs.remove(&room).is_some() {
                self.summaries.borrow_mut().remove(&room);
                self.kept.borrow_mut().remove(&room);
                self.fresh.remove(&room);
                self.session.remove_room(&room);
                if !self.changed.contains(&room) {
                    self.changed.push(room);
                }
            }
        }
    }

    /// The pictures the notes show, as paths from the notes folder, those
    /// in Deleted too.
    fn pictures_in_use(&self) -> std::collections::BTreeSet<String> {
        let mut used = std::collections::BTreeSet::new();
        let files = self.picture_files();
        for (room, d) in &self.rooms.docs {
            if Key::parse(room).is_none() {
                continue;
            }
            let folder = self.place(room).map(|p| p.folder).unwrap_or_default();
            used.extend(doc::picture_paths(&folder, &d.text(), &files));
        }
        used
    }

    /// Of `paths`, the pictures some version of a note here shows: one
    /// restored would show them again. Only notes whose history may hold
    /// the picture's name are read version by version.
    fn shown_in_history(&self, paths: &std::collections::BTreeSet<String>) -> std::collections::BTreeSet<String> {
        let mut shown = std::collections::BTreeSet::new();
        if paths.is_empty() {
            return shown;
        }
        let names: Vec<&str> = paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p)).collect();
        let files = self.picture_files();
        for (room, d) in &self.rooms.docs {
            if Key::parse(room).is_none() {
                continue;
            }
            let bytes = d.to_bytes();
            if !names.iter().any(|n| bytes.windows(n.len()).any(|w| w == n.as_bytes())) {
                continue;
            }
            let folder = self.place(room).map(|p| p.folder).unwrap_or_default();
            for r in d.history() {
                if let Ok(text) = d.text_at(&r.version) {
                    shown.extend(doc::picture_paths(&folder, &text, &files).into_iter().filter(|p| paths.contains(p)));
                }
            }
        }
        shown
    }

    /// A note in Deleted, deleted forever: its history, here and on every
    /// device, and its room on the server. The pictures it showed, in any
    /// version, that no other note shows now are dropped, for the server
    /// to drop. False for a note not in Deleted.
    pub fn delete_forever(&mut self, key: &str, rng: &mut impl Random) -> bool {
        let Some(k) = Key::parse(key) else { return false };
        let Some(place) = self.place(key).filter(|p| p.trashed) else { return false };
        let mut shown = std::collections::BTreeSet::new();
        let files = self.picture_files();
        if let Some(d) = self.rooms.docs.get(key) {
            shown.extend(doc::picture_paths(&place.folder, &d.text(), &files));
            for r in d.history() {
                if let Ok(old) = d.text_at(&r.version) {
                    shown.extend(doc::picture_paths(&place.folder, &old, &files));
                }
            }
        }
        self.rooms.doc(vault::ROOM).set_gone(k);
        self.rooms.docs.remove(key);
        self.summaries.borrow_mut().remove(key);
        self.kept.borrow_mut().remove(key);
        self.session.remove_room(key);
        let mut used = self.pictures_in_use();
        let rest: std::collections::BTreeSet<String> = shown.difference(&used).cloned().collect();
        used.extend(self.shown_in_history(&rest));
        let vault = self.rooms.doc(vault::ROOM);
        let held = vault.pictures();
        let mut unused = Vec::new();
        for p in shown.difference(&used).filter(|p| held.contains_key(*p)) {
            if let (_, Some(id)) = vault.drop_picture(p)
                && let Some(id) = unhex(&id).and_then(|b| <[u8; 32]>::try_from(b).ok())
            {
                unused.push(id);
            }
        }
        self.session.drop_blobs(&unused);
        self.session.saved(vault::ROOM, &mut self.rooms, rng);
        self.changed.push(key.to_string());
        true
    }

    /// The note moved to Deleted, or put back: on every device.
    pub fn set_trashed(&mut self, key: &str, trashed: bool, rng: &mut impl Random) -> bool {
        let Some(k) = Key::parse(key) else { return false };
        let vault = self.rooms.doc(vault::ROOM);
        let Some(mut p) = vault.place(k) else { return false };
        p.trashed = trashed;
        if !vault.set_place(k, &p) {
            return false;
        }
        self.session.saved(vault::ROOM, &mut self.rooms, rng);
        true
    }

    /// A name for a new picture in the attachments folder (`stem.ext`,
    /// numbered past one the vault or an upload has already).
    pub fn picture_free(&self, stem: &str, ext: &str) -> String {
        let taken = |p: &str| self.picture_id(p).is_some() || self.uploads.values().any(|(q, _)| q == p);
        let mut n = 1;
        loop {
            let name = if n == 1 { format!("attachments/{stem}.{ext}") } else { format!("attachments/{stem}-{n}.{ext}") };
            if !taken(&name) {
                return name;
            }
            n += 1;
        }
    }

    /// A picture put in here at `path`, to go to the server and then the
    /// vault: its id, in hex. Nothing to do for one the vault has there
    /// already. `None` for one too big to send.
    pub fn upload(&mut self, path: &str, bytes: Vec<u8>) -> Option<String> {
        if bytes.len() > carnet_sync::crypto::MAX_BLOB {
            return None;
        }
        let id = self.session.blob_id(&bytes);
        let hexed = hex(&id);
        if self.picture_id(path).as_deref() == Some(hexed.as_str()) {
            return Some(hexed);
        }
        self.uploads.insert(id, (path.to_string(), bytes));
        self.session.offer_blobs(vec![id]);
        Some(hexed)
    }

    /// The id, in hex, of the picture at `path` (from the notes folder).
    pub fn picture_id(&self, path: &str) -> Option<String> {
        self.rooms.docs.get(vault::ROOM)?.pictures().get(path).cloned()
    }

    /// The picture at `path` (from the notes folder) asked for; it comes
    /// in [`Notes::pictures`]. Its id, in hex, or `None` if the vault
    /// doesn't have it. One dropped since (a version brought back shows
    /// it) is asked for by the id it had, and named again when it comes.
    pub fn get_picture(&mut self, path: &str) -> Option<String> {
        let id = match self.picture_id(path) {
            Some(id) => id,
            None => {
                let id = self.rooms.docs.get(vault::ROOM)?.dropped_id(path)?;
                let blob: BlobId = unhex(&id)?.try_into().ok()?;
                self.reviving.entry(blob).or_default().push(path.to_string());
                id
            }
        };
        let bytes: Vec<u8> = (0..32).map(|i| u8::from_str_radix(id.get(2 * i..2 * i + 2).unwrap_or("00"), 16).unwrap_or(0)).collect();
        self.session.get_blob(bytes.try_into().ok()?);
        Some(id)
    }

    /// Every picture the vault names, by id in hex: what is worth keeping.
    pub fn picture_ids(&self) -> Vec<String> {
        self.rooms.docs.get(vault::ROOM).map(|v| v.pictures().into_values().collect()).unwrap_or_default()
    }

    /// Asks for the account's devices: they come as [`Said::Devices`].
    pub fn list_devices(&mut self) {
        self.session.list_devices();
    }

    /// A passkey made in this browser added to the account (§7), by its
    /// PRF output: [`Said::PasskeySet`] once the server keeps it.
    pub fn add_passkey(&mut self, name: &str, credential: &[u8], prf: &[u8], rng: &mut impl Random) -> Result<(), String> {
        self.session.add_passkey(name, credential, prf, rng)
    }

    /// The passkey `id` names taken away: it lets no device in from now.
    pub fn remove_passkey(&mut self, id: &[u8; 32]) {
        self.session.remove_passkey(id);
    }

    /// Asks what the account keeps, and may: it comes as [`Said::Usage`].
    pub fn usage(&mut self) {
        self.session.usage();
    }

    /// Takes `device` out of the account. Another device: a new key goes
    /// to every device kept, so it reads nothing written from now on.
    /// This browser: it leaves.
    pub fn remove_device(&mut self, device: &[u8], rng: &mut impl Random) -> Result<(), String> {
        self.session.revoke(device, rng)
    }

    /// What this browser is called in the others' lists, changed now; the
    /// list comes again.
    pub fn rename(&mut self, name: &str, rng: &mut impl Random) {
        self.session.rename(name, rng);
    }

    /// Whether this account is the server's owner's, and its invites and
    /// accounts: they come as [`Said::Invites`].
    pub fn list_invites(&mut self) {
        self.session.list_invites();
    }

    /// A new invite, for someone's own account on this server: its code,
    /// to give them. The list comes again. `None` when not connected.
    pub fn make_invite(&mut self, rng: &mut impl Random) -> Option<String> {
        let code = self.session.invite(rng)?;
        let text = code.text();
        self.made.push(code);
        Some(text)
    }

    /// An invite not used yet, taken back.
    pub fn drop_invite(&mut self, hash: [u8; 32]) {
        self.session.drop_invite(hash);
    }

    /// Another person's account taken off the server, with all it keeps.
    pub fn remove_account(&mut self, account: &[u8]) {
        self.session.remove_account(account);
    }

    /// Opens a pairing: the code comes as [`Said::PairCode`].
    pub fn pair_open(&mut self, rng: &mut impl Random) {
        self.session.pair_open(rng);
    }

    pub fn pair_cancel(&mut self) {
        self.session.pair_cancel();
    }

    /// A new recovery code, set on the server: its twelve words, shown
    /// once and kept nowhere here. `None` when not connected.
    pub fn make_recovery(&mut self, rng: &mut impl Random) -> Option<String> {
        if !self.session.is_ready() {
            return None;
        }
        let code = RecoveryCode::generate(rng);
        self.session.set_recovery(&code, rng);
        Some(code.words())
    }

    /// A note's versions, newest first.
    pub fn history(&self, key: &str) -> Vec<Past> {
        let Some(d) = self.rooms.docs.get(key) else { return Vec::new() };
        let history = d.history();
        let texts: Vec<String> = history.iter().map(|r| d.text_at(&r.version).unwrap_or_default()).collect();
        history
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let (added, removed) = compare::word_delta(texts.get(i + 1).map_or("", String::as_str), &texts[i]);
                let mine = r.device.as_deref() == Some(self.me.device.as_slice());
                Past { version: hex(&r.version.to_bytes()), time: r.time, origin: r.origin, by: r.device.clone(), mine, added, removed }
            })
            .collect()
    }

    /// A note as it was at `version`, what changed since the version
    /// before it marked as the desktop's history marks it. The first
    /// version is shown as it was.
    pub fn past_html(&self, key: &str, version: &str) -> Option<String> {
        let d = self.rooms.docs.get(key)?;
        let history = d.history();
        let at = history.iter().position(|r| hex(&r.version.to_bytes()) == version)?;
        let new = d.text_at(&history[at].version).ok()?;
        let md = match history.get(at + 1).and_then(|r| d.text_at(&r.version).ok()) {
            Some(old) => {
                let (title, marked) = compare::marked(&old, &new);
                doc::to_markdown(title.as_deref(), &marked)
            }
            None => new,
        };
        Some(self.draw(key, &md))
    }

    /// The note made what it was at `version`, by a new change (history
    /// only grows), sent at once. False if it already was, or there is
    /// no such version.
    pub fn restore(&mut self, key: &str, version: &str, rng: &mut impl Random) -> bool {
        let Some(d) = self.rooms.docs.get(key) else { return false };
        let Some(v) = unhex(version).and_then(|b| Version::from_bytes(&b)) else { return false };
        if !matches!(d.restore(&v), Ok(true)) {
            return false;
        }
        self.session.saved(key, &mut self.rooms, rng);
        true
    }
}

/// How [`Notes::save`]'s bytes are laid out, named in their header:
/// `CRS`, then the layout as a digit's byte (`b'0' + n`), so layout 1 is
/// the `CRS1` every build before there were layouts wrote. Raised when a
/// release changes what follows, with a step in [`STATE_UPGRADES`] from
/// the one before.
pub const STATE: u32 = 1;

/// Where a picture is ([`doc::find_picture`]), spelled as the vault
/// spells it: a desktop on a disk that doesn't mind case shows
/// `Hill.JPG` for `hill.jpg`, and so does this.
fn find(folder: &str, src: &str, wiki: bool, files: &doc::Files) -> Option<String> {
    let path = doc::find_picture(folder, src, wiki, files)?;
    Some(files.get(&path).map_or(path, str::to_string))
}

/// A step from one layout to the next: what follows the header in one,
/// as the next has it, or `None` if it doesn't read.
type StateUpgrade = fn(Vec<u8>) -> Option<Vec<u8>>;

/// The steps, oldest first: `STATE_UPGRADES[n - 1]` takes layout n to
/// n + 1. The page keeps the result at its next save.
const STATE_UPGRADES: &[StateUpgrade] = &[];
const _: () = assert!(STATE_UPGRADES.len() + 1 == STATE as usize && STATE < 200, "a step per layout, a byte each");

fn header(layout: u32) -> [u8; 4] {
    [b'C', b'R', b'S', b'0' + layout as u8]
}

/// Saved state from a newer build than this one, with a layout this one
/// doesn't know: not opened, so not saved over either.
#[derive(Debug, PartialEq)]
pub struct Newer(pub u32);

impl std::fmt::Display for Newer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // As web/src/lib/db.ts says it of the database.
        f.write_str("The notes kept in this browser were saved by a newer Carnet. Reload the page to get it.")
    }
}

/// What follows `state`'s header, in this build's layout: `None` for none
/// (the first time) or bytes that don't read, which start again.
pub fn state_body(state: &[u8]) -> Result<Option<Vec<u8>>, Newer> {
    let Some((&[b'C', b'R', b'S', n], body)) = state.split_first_chunk::<4>() else { return Ok(None) };
    let Some(layout) = n.checked_sub(b'0').map(u32::from).filter(|l| *l >= 1) else { return Ok(None) };
    if layout > STATE {
        return Err(Newer(layout));
    }
    Ok(STATE_UPGRADES[layout as usize - 1..].iter().try_fold(body.to_vec(), |b, step| step(b)))
}

/// Where [`Notes::save`] keeps what the server is still to be told of
/// things gone: beside the docs, under a name no room has.
const FORGOTTEN: &str = "~forgotten";

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    s.len().is_multiple_of(2).then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect())?
}

#[cfg(test)]
mod tests {
    use carnet_core::doc::resolve_picture as resolve;

    #[test]
    fn a_picture_resolves_from_its_notes_folder() {
        assert_eq!(resolve("", "attachments/a.png").as_deref(), Some("attachments/a.png"));
        assert_eq!(resolve("Trips/Lisbon", "../../attachments/a%20b.png").as_deref(), Some("attachments/a b.png"));
        assert_eq!(resolve("", "../x.png"), None, "out of the notes folder");
        assert_eq!(resolve("", "https://e.com/x.png"), None);
    }
}
