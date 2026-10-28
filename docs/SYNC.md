# Carnet Sync: architecture

> Your notes on every device, merged and never lost. The files stay plain Markdown on your disk. The server is one binary anyone can run, and it only ever sees ciphertext.

This document describes how Carnet's optional sync works: what is synced, how devices merge, what the server stores and sees, how keys are made and moved, and how the pieces are tested. It's written for contributors, for people running their own server, and for anyone checking the security claims. Source comments cite it by section ("docs/SYNC.md §6", or just "§6" inside the sync crates), so the section numbers are stable.

**Terms used throughout:**

| Term | Meaning |
|---|---|
| **notes folder** | The folder of `.md` files a desktop app shows. The files are the truth; sync is extra. |
| **device** | One install that syncs: a desktop app, or a browser running the web app. Each has its own signing key. |
| **account** | One person's notes on a server, and the devices allowed to read them. An account holds one **vault**: the notes, their folders and their files. |
| **doc** | A CRDT document (§3). Each note is one doc; the vault's layout (folders, file names, pins, trash) is one more, the **vault doc**. |
| **room** | A doc as the server knows it: a random id, and the encrypted records written to it. |
| **record** | One encrypted piece of a doc: a **delta** (some of one device's operations) or a **snapshot** (the whole doc as of a version). |
| **vault key** | The symmetric key every record and file is encrypted under, by way of keys derived from it. It changes when a device is removed; each version of it is an **epoch**. |
| **hub** | The part of the server that handles sockets' frames: logins, rooms, relaying. It runs on one thread and does no I/O itself. |
| **replica** | A device's local sync state: every note's doc, and an index of where each note's file is. |

## 1. What it has to be

The typical user is one person with a laptop, a desktop and a phone, often offline on at least one of them. Two people typing into one paragraph at the same moment is not the main case, and isn't optimised for. What is:

- **Merge, never fork.** When a note is changed on two devices offline, both edits survive. No `… (conflict).md` copies of the kind a Dropbox folder produces.
- **The file is still the truth.** Every note stays the same GFM file, readable by any tool. An edit made in another editor is picked up and synced like an edit made in Carnet. If the sync state is deleted, it's rebuilt from the folder (without history).
- **No network on the hot path.** Typing never waits on sync. Sync runs behind the save.
- **Self-hostable in one command.** One static binary or one container, with SQLite and a data folder. Postgres and S3-compatible storage are optional, for larger installs.
- **End-to-end encrypted.** The server stores and relays opaque blobs. A breach of a server leaks sizes and timings, never notes (§7 lists exactly what it sees).
- **A phone can read and write.** The web app (§8) reads every note offline, ticks checklists and edits notes, installable as a PWA (a web app that can be added to the home screen and work offline).
- **Opt-in and silent.** Sync is off until it's turned on in Settings → Sync. The app never shows a dialog, banner or nudge about it.

## 2. The shape

```
 Desktop app                                      Browser (web app, a PWA)
 ┌──────────────────────────────┐                 ┌───────────────────────────────┐
 │ editor ── save ──► store     │                 │ Svelte UI, CodeMirror editor  │
 │           .md files ◄────────┼── other apps    │  ├ service worker (offline)   │
 │ replica (app data folder)    │                 │  ├ IndexedDB (sealed state)   │
 │  ├ a Loro doc per note       │                 │  └ carnet-web (wasm):         │
 │  ├ the vault doc             │                 │     carnet-core + carnet-sync │
 │  └ files index               │                 └───────────────┬───────────────┘
 │ carnet-sync session (sealing)│                                 │
 │ socket thread ───────────────┼────────┐   ┌────────────────────┘
 └──────────────────────────────┘        ▼   ▼   one WebSocket each, plus HTTPS for files
                                ┌──────────────────────────────┐
                                │ carnet-server (axum)         │
                                │  ├ hub: logins, rooms, feed  │   SQLite: a file per account
                                │  ├ file links and storage    │   (or Postgres)
                                │  └ the web app's files at /  │   files: on disk (or S3)
                                └──────────────────────────────┘
```

The server never decrypts and never merges. It authenticates devices, orders, stores and forwards. Every merge happens on a device, in the same Rust code on the desktop and in the browser.

## 3. The CRDT

A CRDT (conflict-free replicated data type) is a data structure that any number of copies can edit independently and that always converges to the same state once every copy has seen every change, in any order. Carnet uses [Loro](https://loro.dev).

### Why Loro

| | Loro | Automerge 3 | Yrs (Yjs) |
|---|---|---|---|
| Core language | Rust | Rust | Rust port of a JS original |
| Web | wasm | wasm | Yjs (JS); Yrs wasm exists |
| Text algorithm | Fugue (minimal interleaving) | RGA-style | YATA |
| "Make the text equal this string" | `LoroText::update` (a character diff) | `update_text` | none built in |
| Fork at a past version | yes (`fork_at`) | yes | limited |
| Ecosystem | youngest | mature | largest |

Loro fits best because:

1. **The editor doesn't change.** Carnet's editor works on block snapshots, not an operation log. `LoroText::update(markdown)` diffs the new text against the doc's and records the difference as operations, so sync hooks in after the save and the editor never learns it exists.
2. **`fork_at` gives a real three-way merge** for a note edited here and in another app at the same time (§5), and lets history previews run on a fork without touching the live note (§9).
3. **It's Rust,** so the same crate compiles into the desktop binary and into the web app's wasm.

Automerge 3 would work almost as well. Everything sits behind `carnet-sync`'s own types, so a switch would stay inside that crate.

**Measured before committing to it** (loro 1.16, Apple-silicon Mac):

- **Every byte survives.** 400 random saves on each of six fixtures (long Markdown documents, an 80 KB note, CRLF line endings with Cyrillic, CJK, emoji and combining accents) gave 0 mismatches, both in the writing copy and in a second copy fed only exported updates.
- **Saves are cheap.** A one-character save on the 80 KB note takes 0.09 ms; a 500-line paste, 0.23 ms.
- **History is small.** A simulated year of writing (3,000 saves) weighs 62 KB of history, less than the 81 KB file. Kept as 3,000 encrypted delta records it's about 500 KB, which is why the server compacts (§6).
- **Merges that come out wrong are rare and lose nothing,** within a text CRDT's known limits (below). Two devices typing at the same spot keep both phrases whole rather than interleaving letters.

### Granularity: the Markdown text, not blocks

Each note is one `LoroText` holding the file's Markdown, exactly as Carnet writes it.

- **Why not a tree of blocks** (a table as rows of cells)? It would merge tables more cleverly, but it would be a second document model beside `carnet-core`'s, its serialiser would have to round-trip byte for byte, and edits from other apps would still arrive as text. Text is what's on disk, so text is what's merged.
- **A character diff, not a line diff.** Loro also offers `update_by_line`, which replaces each changed line whole. When two devices edit one line, both versions then survive: a table row appears twice, or a checklist item twice with its tick on one copy. In testing the character diff merged 7 of 10 concurrent-edit cases as a person would want, against 5 for the line diff, and its history was less than half the size (62 KB against 142 KB for the year above).
- **What text merging costs:** concurrent edits to *different* lines, cells, items or words merge cleanly. Two devices changing one table's column count at once can leave a ragged row (GFM still renders it). Two devices styling overlapping words can leave a stray `**`. The same typo fixed on two offline devices comes out `thee`. An item deleted on one device while edited on another leaves the new words behind. All of these are rare for one person on several devices, visible, and fixed in a keystroke. Nothing is lost.

## 4. What is synced

| Thing | Where it lives in sync | How it merges |
|---|---|---|
| A note's text (title included, as its `# ` line) | a Loro doc per note (`LoroText`) | character by character |
| A note's folder, file name, pinned, trashed | the vault doc: one map entry per note and field (`<key>/folder`, `/name`, `/pinned`, `/trashed`) | last writer wins, per field, so a pin on one device and a move on another both hold |
| Folders, empty ones too | the vault doc: a map of path → present or not | last writer wins, per folder |
| Files in `attachments/` (Carnet's: pictures, and documents a note links to; see below) | sealed blobs by content id (§7); the vault maps path → blob id (a picture's in `pictures`, a document's in `files`) | a blob never changes; a path is last writer wins |
| Any other file a note shows or links to (beside it, elsewhere, outside the notes folder) | not synced: the user's file, on the device that has it | — |
| A trashed note | its doc stays; the vault marks it trashed, and its folder and name say where it came from | restoring clears the mark |
| A note deleted forever | the vault marks it gone (`<key>/gone`), never unmarked; the server deletes its room | gone wins over any other field |
| A file no note shows or links to any more | the vault's path keeps `-<id>` instead of the id; the server may drop the blob after 30 days (§6) | a path put back revives it |
| The codes of invites the account made (the server's owner's, §6) | the vault doc: a map of the code's hash (hex) → `{code, made}` | made once, never changed; forgotten when taken back (all of them when another device is removed), once the server's list leaves it out (a list naming others), or once run out (7 days) |
| Settings | not synced | — |

**Whose a file is.** Carnet owns the files in the notes folder's `attachments/` folder (any case: `Attachments/` is the same folder): a paste, a drop, the paperclip, an import and an export's gathering put them there. Only those sync, and every one of them, of any kind (`files::syncs`): a picture, a `pdf`, a sheet, a recording, a `README`, source code, a page, and a program, a script or a shortcut a note links to as well. Synced files are written without the mode that would make one a program on macOS or Linux, and a copy taken in loses it. A path but a picture's is in the vault's `files` map, not `pictures`: a Carnet from before files lets go of every path in `pictures` it doesn't take for a picture's, which would drop the file on every device. One from when files were only a few kinds (a PDF, a sheet, a recording) reads a path of another kind in `files` as written in the wrong map, so as not there: it neither fetches nor lets it go, nor sweeps it, and the devices that know it keep it. What comes is marked as from elsewhere, as a download is: on macOS `com.apple.quarantine`, so the Finder (and Gatekeeper, for a program) asks before opening it; on Windows the Mark of the Web (a `Zone.Identifier` stream, the Internet's zone), so SmartScreen asks before a program runs and Office opens a document in Protected View. A link in a note opens its file in the system's app for it; one that could run a program as it opens (a program, an installer, a disk image, a script an interpreter runs, such as `.sh`, `.ps1`, `.js` or `.py`, an add-in, or a shortcut, such as `.lnk`, `.url` or `.webloc`, on any system, by its kind, `doc::could_run`; on macOS or Linux also one marked as a program, `store::could_run`) asks first, naming it, Cancel what Enter does. Only those Carnet ever removes. Any other picture a note shows (or file it links to), beside the note (`Home/hill.jpg`), elsewhere in the folder, or outside it (`../`, a full path, `file://`), is the user's: Carnet shows it, faint and marked, never syncs it, and never moves or deletes it on its own. Selected, the picture says so, and offers:

- **Move into Carnet**: the file goes into `attachments/` under a free name (a hard link, then the old name removed, so no note ever shows nothing), and every note that shows it, one in Deleted too, is rewritten to the new path and saved as an edit is, so its history and sync see it. The open notes' undo histories are rewritten likewise, so an undo never brings back a path that leads nowhere. The notice's Undo (or Ctrl+Z while it shows) moves the file back and puts the notes' pictures back as they were; if the vault had named the new path meanwhile, it's dropped there.
- **Copy into Carnet**: a copy in `attachments/`, and this note alone shows it, as one ordinary edit, saved. The notice's Undo (or Ctrl+Z while it shows) takes the edit back and the copy with it, unless a note shows it by then (one in Deleted too): to `.trash/attachments/`, where the sweep would have put it, dropped in the vault; the note's undo history no longer names it, and the version saved with it brings it back from there if restored. An undo after the notice has gone takes the edit back alone, and the copy stays: that version shows it.

A Carnet before this one synced the pictures beside notes too. What it named of those in the vault is dropped there by the first sync round of each run (the server may then drop the blob); the files, wherever they are, stay. The vault's paths outside `attachments/` are never fetched, nor written.

**The sweep.** A file of Carnet's no note shows or links to any more is taken away on the desktop, a while after (`store_owned.rs`): a little after the window opens, then once a day while it's open, and only when sync is off, or on, caught up with the account, and no file is on its way up or down (put off a minute otherwise). A file is unshown when no note shows it or links to it (unsaved edits included), no note in Deleted does, no text in `.trash/` names it (a note put there by hand), no version of any note in the replica shows it, and sync isn't sending it or fetching it back. The first time one is seen so, the time is kept in `unshown.txt` beside `pictures.txt`; shown again, it's forgotten there. Unshown for **7 days**, it's moved to `.trash/attachments/…`, under the same path (or the first free name), where it can be put back by hand, and dropped in the vault: the other devices drop their copies (the drop path above, which keeps one a note there shows), and `BlobDrop` lets the server delete the blob after its own 30 days. A note saved showing it again (a version restored, an undo) puts it back from `.trash/` at once; on another device, it's fetched back while the server keeps it (`want_back`).

So a file taken out of a note by an edit is kept as long as that note's history shows it, which is as long as the note is kept: only deleting the note forever lets it go. The sweep is for files no version ever saved: a paste undone before the note was saved, one dropped then deleted, a copy taken in then undone before a save. A note's link to a file counts as showing it (`doc::linked_paths`).

**Names that stay in the notes folder.** A name, folder or file path in the vault is read only if each of its parts is one plain name on every system (`carnet_core::name::is_plain_path`): not empty, not `.` or `..` (nor only dots and spaces), not hidden (a leading `.`), with no `\`, `:`, `<>"|?*` or control character, and not a name Windows keeps for a device (`CON`, `nul.txt`, `COM1`). Anything else (`../../x`, `/etc`, `C:`, `\\host\share`, `a\b`) is read as not there, on the desktop and the web alike: a note whose place isn't plain arrives at the top, named for its title, and no such folder or file is made. Nor does the vault write one. A place an older or a hostile device wrote so is put right by the next device to write that note's place. Before any write, rename, removal or folder made for sync, the desktop also checks that the path, joined, is still inside the notes folder. A file named as a Mac allows but another system wouldn't (`2:1.md`): its note goes to the vault under its name made plain as a title is (`2-1`), and the file here follows, as one renamed elsewhere would; a file so named in `attachments/` isn't synced, nor swept. A picture's path in a note's text is worked out the same way, `\` taken as `/` as Windows takes it, so `attachments\..\..\x` is outside the notes folder, never Carnet's to remove. A picture on another computer (`\\host\share\…`, `//host/…`, `\\?\…`, `/net/…`) is never looked at, since on Windows looking alone hands that computer the user's login: it shows as not found.

**Note identity.** A note gets a random 128-bit key the first time sync sees it. The mapping key ↔ path lives in the replica, not in the file: no front matter is added. A rename in Carnet updates the mapping directly. A rename made outside Carnet is spotted on rescan: a known path that disappeared is matched to a new file sharing at least half its lines.

**File names.** The vault stores the name a note *wants* (the slug of its title). When several notes want one name in one folder, every device numbers them the same way, by key: `Plan.md`, `Plan 2.md`, … Numbering skips names other notes want outright, so devices don't renumber each other's notes back and forth.

**Where the sync state lives.** In the app's data folder, never inside the notes folder:

| OS | Replica folder |
|---|---|
| macOS | `~/Library/Application Support/Carnet/replica/<id>/` |
| Windows | `%APPDATA%\Carnet\replica\<id>\` |
| Linux | `$XDG_DATA_HOME/carnet/replica/<id>/` (default `~/.local/share/…`) |

`<id>` is a hash of the notes folder's path, so each notes folder has its own. A `.carnet/` inside the notes folder would itself be synced by Dropbox or iCloud and conflict. Running Carnet Sync on a folder that's also in Dropbox still works: a file Dropbox rewrites is just an edit from another app (§5).

```
index.log        a line per change: note key, text hash, version, path
notes/<key>.loro each note's doc, history and all; loaded only when needed
vault.loro       the vault doc
vault.applied    the vault's version last applied to the files here
pictures.txt     path, size, modified time and blob id of each file, and whose ids they are (named from when only pictures synced)
unshown.txt      Carnet's files no note shows, and since when: the sweep's grace period (§4)
forgotten.txt    rooms and files the server still has to be told are gone
version          the layout of this folder (§14)
```

- **The index is an append-only log.** A save appends one short line. When the log opens with more stale lines than live ones, it's rewritten whole.
- **Docs are written by a thread of their own.** A save hands over the bytes and goes on; a doc still waiting to be written is read from memory.
- **Writes are atomic, but not flushed to disk** the way notes are. A replica that's lost or behind is rebuilt from the files, without their history. A doc that doesn't decode is renamed `.loro.bad`, not deleted.
- **Memory:** only the index and up to 32 recently used docs stay loaded. On a folder of 5,000 notes the first open, which makes every doc, costs a few hundred milliseconds more than plain files, once; later opens cost about the same as plain files. A doc sync changed that the store hasn't written yet is written before it leaves memory: a device catching up takes in many notes before it writes one.
- **A room told of is kept.** A note the feed names that has no file here yet (its words not come) still gets a doc file, and the replica lists it as a room from then on, across restarts, since the feed's cursor passes it (§6). One the server says was removed before it came is forgotten.

The replica exists even with sync off: it's what gives every note its history (§9) and lets a save merge with an outside edit (§5).

**The replica outlives an account.** Sync turned off and set up again, with another server or afresh on the same one, the notes keep their keys and their history, and the vault doc its layout: they go up to the new account as they are. A file's id is not the same there, though: it's made under the account's vault key (§7). So `files.txt` says whose ids it holds, and one made under another key is started again: every file hashed anew, offered, put, and named in the vault by its new id.

## 5. The engine on the desktop

**Threads.** The session (the device's side of the protocol, `carnet_sync::session`), the replica and the notes stay on the app's thread. The socket has a thread of its own (`carnet_sync::net`, tokio-tungstenite on a current-thread runtime) that only moves bytes, and wakes the app when some arrive. The session is a state machine with no I/O: bytes in, bytes out, events. The same session runs in the browser and in the test simulation.

**A save here.** The store writes the file as before, then records the new text in the note's doc (`LoroText::update`, one commit). The session sends what's new since the last send. Saves are already debounced, so this is about one commit every couple of seconds of typing, never one per key.

**An edit from another app.** Rescans already notice a changed file. Its new text is recorded in the doc the same way, marked as from another app, and syncs like a local edit.

**Both at once: a merge instead of a conflict copy.** If a note has unsaved edits here and its file changed on disk meanwhile:

1. `base` = the doc's version when the file was last written or read.
2. `theirs` = `fork_at(base)`, then `update(disk text)`.
3. `ours` = `update(editor text)` on the main doc.
4. Importing `theirs` gives one merged text, which is written to the file, and the page reloads from it.

This needs no server. Only when there's no version to merge from (no replica, or a file it never saw) does Carnet keep both, as `… (conflict).md`.

**An update from another device.** It's decrypted and imported, and the store writes the merged text atomically. The outcome is one of: *same* (the file has it already), *updated* (no edits here), *merged* (edits here, both kept, and a notice says so), or *created* (a note new to this device). An open note is redrawn with the caret kept where it can be. The note list doesn't re-sort under the pointer: order is recalculated on navigation.

**The vault doc.** Each tick (at once when the vault changed, otherwise every second, and a second after anything happened: a login, a save, something come or the folder looked at again, though nothing else wakes the app) the store compares three layouts: the files here, the vault, and the vault as last applied here (`vault.applied`, so a restart knows what changed meanwhile). Field by field, what changed here is written to the vault, and the rest is taken from it and done to the files: moved, renamed, pinned, trashed, restored, folders made, empty ones removed. A note moved so keeps its text as it is: a move here rewrites its files' relative links for the new folder, as an edit that syncs, so a device following the move takes that edit rather than rewriting them again, and a note whose text came before its place (put at the top meanwhile) was written for the folder it goes to. A note with unsaved edits waits. Nothing of this device's is written before the account's vault has arrived, or a new device's layout would overwrite the others'.

**Peers.** Each run of the app is a fresh Loro peer, so two runs never write as one. The device that wrote a change is recorded in its commit message instead (§9).

**Offline.** There is no outbox. On joining a room the server says what it holds of it, by peer (a version vector), and the device sends whatever its own doc has past that. A dropped socket loses nothing: the next join works it out again. The only sync state kept beyond the notes is the feed cursor (§6). The socket reconnects after 1 s, 2 s, 4 s … up to a minute, and pings every 30 s so a proxy keeps a quiet socket open.

**What the desktop keeps:**

- `sync.txt`, beside `settings.txt`: the server, the account id, this device's id and name, and the feed cursor. Nothing secret.
- The signing key and the vault key ring: one item in the OS keychain (§7), read on a background thread so the window never waits on a keychain prompt. The item is named for the settings folder, so a second Carnet with its own settings (`CARNET_CONFIG`) is a second device. On Linux with no keychain running, a file only the user can read, and Settings → Sync says so.

**A new recovery code** replaces the one before, so where there is one, Settings asks first ("Replace the recovery code? The words you have now will stop working.", Cancel or Yes, replace). Its words are shown only once the server says it keeps the code (`RecoverySet`): a code it never took is never written down.

**Pictures in notes** are read by a few threads, not one each, within limits: 16384 pixels a side and 256 MiB of memory for one picture; past them, a card says "This picture is too big to show". Only HEIC, TIFF and AVIF, by their file names, go to macOS's ImageIO; anything else is read as its name says, or not at all.

On macOS the keychain ties an item to the program that made it. A locally rebuilt Carnet, or an update not signed with the same Developer ID, is a different program to the keychain, which asks for the login password before handing over the sync keys. If access is denied, sync stays off and Settings says why; the next start asks again.

**Joining an account.** The new device's untouched welcome note, if it's alone in the folder, goes before anything is sent (its file removed, not to Deleted), so the account doesn't end up with two welcome notes. Once the account's notes are all here, it comes back, to sync as any note does, if the account has no welcome note of its own (nor one in Deleted, let go on purpose): the account has one, never two, nor none because a device joined. One the person wrote in always stays.

## 6. The server

### The wire: Loro Protocol rooms, plus a feed of Carnet's own

**Each doc is a room in the Loro Protocol** (MIT), encoded and decoded by the `loro-protocol` crate. It multiplexes rooms on one WebSocket and fragments large updates. Every room, the vault's and each note's, is a `%ELO` room: end-to-end encrypted Loro.

**What the server sees of a `%ELO` record.** Each record has a plaintext header, authenticated as the AES-GCM associated data, and ciphertext after it:

| Record | Plaintext header | Holds |
|---|---|---|
| delta | `peer, start, end, key id, IV` | one peer's operations `start..end` on this doc |
| snapshot | `version vector, key id, IV` | the whole doc as of that version |

The key id is `e<epoch>`; a peer is written as its decimal digits, as other Loro Protocol clients write it. That's enough for the server to sync each room without decrypting anything:

- **Backfill by version vector.** A device joins a room with its version vector. The server answers with the room's base snapshot, if the device isn't past it, plus every delta the device's vector doesn't cover. It keeps a peer's spans unless a longer span from the same peer covers them.
- **A part at a time.** A room can hold more than may wait for a socket (below), so the server sends what a device lacks of it in parts (`hub::BACKFILL_PART`: 2 MiB, or an eighth of what may wait for the socket if that's less), the next once the socket has written all but its pace (`Hub::drained`). A room joined while another's records are still on their way waits its turn; what another device writes meanwhile goes straight on (in any order a doc takes it), but after a base's fragments still waiting, so an update in fragments never meets another of its room. Read from the store a part at a time too (`Storage::spans_after`), a room past the socket's budget reaches it whole, with no reconnecting.
- **Live:** a delta from one device is stored, acknowledged, and relayed to every other device in the room. A batch is kept whole or refused; a record not kept is never acknowledged.
- **Big changes go in pieces.** A span past 192 KiB of plaintext is cut in halves until each piece fits a record, and records travel in updates under 240 KiB. The server refuses a delta's record over 256 KiB and a frame over 1 MiB.
- **A snapshot is one record, however big,** up to 32 MiB sealed (`elo::MAX_SNAPSHOT`): a doc's history can't be cut in halves as a span can. One past what a message holds (the Loro Protocol's 256 KiB) goes in the Loro Protocol's own fragments: a `DocUpdateFragmentHeader` saying how many and how many bytes in all, then each `DocUpdateFragment` in order, 192 KiB each, put back together into the one update (`elo::messages`, `elo::Pieces`). A device compacting sends it so; the server sends a base so, and any update a message can't hold (a delta's record at its largest, passed on). The server takes fragments only for a snapshot, from the lease holder of a room it joined, one at a time, and keeps none it hasn't been promised by a header that fits; a device takes them only as promised, and asks for the room again otherwise.
- **Room ids are short.** A note's room is its key, 32 hex digits; the vault's is `vault`. The server joins, writes and repairs no room id past 64 bytes. A socket may join any room its account has; a room that holds nothing (a note being made, joined before it's written; a first sync joins every note it has before writing any) costs the account nothing, so the hub would hold any number of made-up ids. So an account's sockets, all told, may have at most 256 such joins out (`hub::NEW_ROOMS`), or as many as rooms it may still make (100,000 less what it has) if that's more; past it, `JoinError`. A join written to, left, or of a socket gone counts no more.

The Loro Protocol's own Rust server and client crates aren't used. At the time Carnet's sync was built they spoke an older protocol version; in `%ELO` rooms a device that joined later received nothing (the client sent every update as a snapshot record with an empty version vector, and the server indexed only deltas); the server kept `%ELO` rooms in memory only; and the client's AES-GCM IV defaulted to all zeros unless the caller set one. So the room logic is Carnet's: records in a database, snapshots as bases, and the feed below. Carnet's `%ELO` writer always uses random IVs.

**The vault feed.** The protocol syncs one room at a time, and has no way to tell a device that a room it has never heard of exists: a note made on another device. So the server numbers every record it accepts with a **per-account sequence number**, and Carnet frames (magic `%CRN`, on the same socket; either side tells the two apart by their first four bytes) carry the feed:

```
← Challenge { nonce, greeting }                 see §7 (login) and §14 (greeting)
→ Hello     { account, device, signature, since: <seq> }
← Welcome   { seq }
← Changed   { [(room, seq), …] }   rooms with records after `since`
  … the device joins those rooms with the Loro Protocol and backfills …
← Changed   { [(room, seq)] }      live: a room it hasn't joined got a record
```

On login a device joins every room it has a doc for, the vault's first, and with each join sends what the server lacks (§5). Any room the feed names that it doesn't have yet gets a doc and is joined too, and from then on is joined on every login; but only a note's (its key, 32 lowercase hex digits) or the vault's (`session::note_room`): any other name makes nothing. At most 10,000 rooms new to the device are taken from the feed in a login (`session::MAX_NEW_ROOMS`); past that the cursor stays and the rest come at the next. Once every room taken has answered, if all but a tenth at most brought words the vault's key opens, as an account's own notes do, the device connects again at once for the rest (`Event::MoreToCome`); if more came empty, unreadable or refused, it waits for a login of the person's making and says so (`TOO_MANY_NEW`), so a server can't fill a device with empty notes. A join's answer for a room the device never asked to join is let be. Rooms stay joined for the life of the socket, so live deltas reach the device directly. Its cursor is the highest `seq` the feed has named.

**Every Carnet frame, by family** (`carnet_sync::wire`):

| Family | Frames | Section |
|---|---|---|
| Login | `Challenge`, `Hello`, `Welcome`, `Denied`, `Changed` | §6, §7 |
| New account by invite | `Enroll`, `Enrolled` | §6 |
| Pairing | `PairOpen`, `PairOpened`, `PairJoin`, `PairJoined`, `PairStep`, `PairApprove`, `PairClose` | §7 |
| Recovery | `SetRecovery`, `RecoverySet`, `Recover`, `Recovered` | §7 |
| Devices | `SetCard`, `ListDevices`, `Devices`, `Revoke`, `Rekey`, `Refused` | §7 |
| Password | `PasswordSetStart`/`Reply`/`Finish`, `PasswordSet`, `PasswordDrop`, `PasswordLogin`, `PasswordLoginReply` | §7 |
| Passkeys | `PasskeyAdd`, `PasskeyDrop`, `PasskeySet` | §7 |
| Compaction and repair | `Compact`, `Checked`, `Inspect`, `Inspected`, `RollBack`, `Repair` | §6, §9 |
| Files | `BlobOffer`, `BlobFetch`, `BlobLink`, `BlobStored`, `BlobNone`, `BlobDrop`, `BlobDropped` | §6, §7 |
| Deletion | `RoomRemove`, `RoomRemoved`, `AccountDelete` | §6 |
| Invites (owner) | `Invites`, `InviteList`, `InviteAdd`, `InviteDrop`, `AccountRemove` | §6 |
| Usage | `Usage`, `UsageIs` | §11 |

### Compaction

**Why.** For each room the server holds encrypted deltas, one per save, and at most one encrypted snapshot as the room's **base**. It can't merge them, because it can't decrypt them. A device can, and one snapshot with full history is far smaller than the deltas it replaces (62 KB against about 500 KB for a heavy year of one note).

**When: the server asks, when it's needed and safe.** It can judge both without reading a word:

- **Needed:** the room holds more than 200 deltas since its base (`--compact-after N`), or more than 256 KiB of them.
- **Safe:** all of these hold.
  1. **The device is caught up.** The device is asked right after the server has sent it everything it holds for the room (on one socket, in order), so the snapshot it writes covers everything.
  2. **Nobody else is writing.** The device is the account's only one connected, or the room has had no new record for 5 minutes. Or the room is halfway to what it keeps uncompacted (below: 5,000 records, or 32 MiB): then the device writing to it is asked whoever else is online. A device that writes on and on while another is connected (a big import with a browser tab open, the vault taking a record for every note) would otherwise never be asked, and its room would fill and stop taking writes.
  3. **One lease.** The server sends `Compact { room }` to that one device, which holds the lease for 10 minutes. No other device is asked meanwhile, so two snapshots never race.

  A device is asked when it joins or writes to a room. An edit that lands during compaction is harmless: the snapshot's version vector doesn't cover it, so that delta stays.

**Accepting the snapshot.** The server takes it only from the lease holder, alone in its update, and only if its version vector covers the current base's. The deltas it covers and the old base are then *set aside*, not deleted: new devices no longer download them, but the server keeps them for 30 days so the snapshot can be rolled back (§9). Snapshots are sealed under the newest epoch, so compaction also retires records under keys a removed device knew.

- **The check.** The next device to load the snapshot opens it on its own, before it touches the note, checks that its real version vector matches the header and that it reads as a note, and sends `Checked`.
- **A failed check.** The server rolls back on its own: it drops the snapshot, reinstates the old base and the deltas, and logs which device wrote it. The checking device asks for the room again and gets the records back.
- **After 30 days,** what was set aside is purged (hourly). That applies with or without a second device, so an account with one device still gains from compaction.

A buggy or hostile client therefore can't wipe history by claiming a version it doesn't hold. Snapshots carry full history; nothing on the server trims it.

**What it judges by.** Whether a room is due, or full, is kept counted by the store (`Storage::live`: how many live records, their bytes, whether there's a base; `Storage::room_count`: how many rooms hold anything), in the transaction that changes them, so a write costs the hub the same whatever the room holds: no records counted, none read. On SQLite and Postgres a table (`rooms`) has a row a room, kept by triggers on the records and bases however they change (a write, a snapshot setting them aside, a roll back, a room removed, `migrate` copying them in), and the account its count of rooms; a database from before it is counted once when a server first opens it. A write finds a record of the same peer it covers, or that covers it, by an index on where each live record ends (`records_reach`), not by reading the peer's records. What the hub keeps for compaction goes hourly once it can't matter: a room's last write past the 5 quiet minutes, a lease past its time. A repair is taken only for a room the server keeps.

**What a room and an account may hold** (`hub.rs`), against a device that never compacts (a lease it's given simply runs out), far past what compaction keeps a room to:

- **A room** past 10,000 live records, or 64 MiB of them, takes no more deltas until it's compacted. The write is refused (`PayloadTooLarge`), the device told once a socket why (`quota::ROOM_FULL`), and asked to compact it if it may. A device holds what it writes for that room rather than sending it to be refused again, until the server asks it for the room compacted, takes its snapshot, or it logs in anew; its changes count as waiting meanwhile. A snapshot is always taken from the lease holder.
- **A note too big to compact,** its snapshot past 32 MiB, is the one that can fill its room for good. Asked for it, the device sends nothing and says so, once a session, in its own words (`session::TOO_BIG`: split it into smaller notes), rather than leaving it to stop syncing unsaid.
- **An account** has at most 100,000 rooms: a new room's first write past that is refused. Its notes, every record and base, set aside or not, may take at most `Quota::notes_ceiling` (4 GiB unless the host says otherwise): past it, no more writes (`quota::NOTES_FULL`). That's a ceiling, not a plan: an account at its plan's limit still syncs its notes (§11). The hub counts the account's notes once and adds each write, so a write doesn't sum the account; it counts anew hourly and when anything is removed.

### The binary

`carnet-server` is Rust: axum for HTTP and WebSockets, the hub on a thread of its own fed by every socket through one queue (so it handles one thing at a time, exactly as in the simulation, and a slow disk never stalls a socket), and a picture worker on another. It's a library with a thin `main`, so a hosted service can be built on it (§11). How to run it, behind which proxy, and how to back it up and move it: [`crates/carnet-server/README.md`](../crates/carnet-server/README.md).

```
GET  /              the web app (§8), or a page saying what this is
GET  /v1/info       what the server is: name, version, origin, protocol, features (§14)
GET  /v1/health     200 while the hub answers, 503 if not
GET  /v1/sync       the WebSocket: Carnet frames and Loro Protocol rooms
GET  /v1/blobs/…    a sealed file, by a link the socket gave
PUT  /v1/blobs/…    one file put, the same way
GET  /metrics       Prometheus text format, on a separate listener (--metrics ADDR)
```

**Storage.** Behind the `Storage` trait:

| | Records, devices, keys | Files |
|---|---|---|
| Default | SQLite (WAL), one file per account: `data/carnet.db` is the directory (accounts, invites, and the indexes a sign-in must find), `data/accounts/<id>.db` an account's devices, records, bases and slots | files: `data/blobs/<account>/<id>`, written into `data/spool` first and moved in whole |
| With the `postgres` / `s3` features | Postgres, one schema, every row naming its account | an S3-compatible bucket, links pre-signed there so the bytes never pass through the server |

A file per account means a query in one account's database can't reach another's, whatever it forgets to say; one account's writes don't wait on another's; an account deleted is a file deleted. Reads never create files: a login naming an unknown account leaves nothing behind. Up to 256 account databases stay open, least recently used closed first. Each file carries its schema version and upgrades itself; one from a newer server is refused (§14). `carnet-server migrate` moves a data folder to Postgres and a bucket, checked and resumable.

Other files in `data/`: `links.key` (signs file links) and, with passwords on, `opaque.key` (§7). Both are made on first start, mode `0600`, and must be kept with a backup.

All of `data/` is the server's own user's alone (`private.rs`): the folder and the folders in it `0700`, the databases (and so their WALs) and files `0600`. Sealed or public as it all is, who has an account, how much it keeps and when it last wrote are nobody else's on the machine. A data folder from before that's open to others is closed on start (`0700`), and the log says so once.

**Accounts by invite.** There are two kinds of code, and they shouldn't be confused:

- An **invite code** makes a new *account*: a person with their own vault.
- A **pairing code** (§7) adds a *device* to an account that already exists.

On a start with no accounts, the server prints an invite code to its log, a fresh one each start until an account exists, each good for a day (`OWNER_INVITE_DAYS`), so a code left in an old log lets no one in: the line says that a restart makes a new one. Any of them still good makes the account; then the others are dropped. The device that redeems it (`Enroll`, over the socket) becomes the **owner**. From then on the owner's devices make invites in Settings, on the desktop or in the browser: the device generates the code and sends only its hash, so the server never holds a usable code. The code goes into the account's vault (§4), sealed like everything else there, so each of the owner's devices shows it again, after a restart or a reload too, with Copy beside it. A device that takes an invite back forgets its code there. One the server's list leaves out (used, or taken back elsewhere) is forgotten by the next device to list the invites, once it's an hour old (a list asked for just before the invite was made wouldn't name it either, and clocks differ a little), but only if that list names others: an empty list is no news, as a server could otherwise wipe every code from every device by saying nothing. One run out (made more than 7 days ago) is forgotten whatever the list says. A used one may so stay a while, never shown, as only the invites the server lists are. Removing another device takes back every invite whose code the vault holds, as the device removed had them all (§7). An invite made before codes were kept, or by `carnet-server invite`, shows as "An invite", by its hash alone. Each is single-use and lasts 7 days (`INVITE_DAYS`; the first account's, a day); at most 100 wait at once. An `Enroll` by one run out is denied in words the apps know and show as their own (`invite::RUN_OUT_FIRST`: "That invite has run out. If it was the server's first, restart the server for a new one."; `invite::RUN_OUT` for any other), no wire change: an older app shows them as the server's; a used one, or one never made, is denied as not good. The owner sees the waiting invites and every other account (its device count, when it was last seen, a short id: the server knows nothing more), can take an invite back, and can remove an account. `carnet-server invite` prints one from the shell, for an owner who has lost every device.

Invite codes are 80 random bits as 16 Crockford base32 characters in groups of four (`K7QF-2M9D-XW4H-P3TA`; no I, L, O or U, so it survives being read aloud). The server stores only the hash: whoever reads its database can't use one.

**TLS** is the reverse proxy's job; the server speaks plain HTTP behind it (nginx, Caddy, or Cloudflare, including a Tunnel for a home server with no open port). Cloudflare terminating TLS is harmless here: all it sees is ciphertext.

**Limits** (`limits.rs`), counted per client: an IPv4 address, or an IPv6 address's /64 (a home or a server is handed a whole /64, so each of its addresses counted apart would never run out). An IPv4 address written as IPv6 counts as itself. The address is taken from `X-Forwarded-For` (read from the right, past the trusted hops), or with `--client-header cf` from `CF-Connecting-IP` alone, only when `--trusted-proxy` names the proxy. Each limit keeps 100,000 clients at most; full, a new one is refused while the known keep their buckets:

- sockets: 30 at once, then one every 2 s, and at most 64 open; past that, `429` before the upgrade;
- tries that guess at something or cost work (a failed login, an invite, joining a pairing, a recovery or password sign-in): 10 at once, then one every 6 s; past that, `Denied`. A login that succeeds is never counted, so a reconnecting device never shuts itself out.

A password sign-in is counted by the email it names as well, from any address (`limits::PASSWORD_TRIES`): 20 at once, then one every 3 minutes; past that, `Denied` ("too many sign-ins with this email lately"), its password not tried. With enough addresses, guesses would otherwise come as fast as the addresses allow; now one password takes twenty guesses an hour, however many. It's a delay, not a lock: someone spending an email's tries keeps its owner out of password sign-in for minutes at a time, never for good, and the account's devices, signed in by their keys, not at all. Only emails a password signs in as are counted, so the counts are as many as the passwords, never filled by made-up emails; that turned-away-while-the-address-isn't says an email has a password, after twenty tries, which setting one (refused for an email another account has) says already.

**Each request** (`server.rs`) has its head read within 30 s, and a connection idle between requests is closed after as long, so one that sends slowly holds nothing (`HEAD_TIMEOUT`). A request is answered within 30 s, its body read too, or gets `408` (`REQUEST_TIMEOUT`): the API, the web app's files and a host's routes. A picture's `PUT` or `GET` by its link has 10 minutes (`BLOB_TIMEOUT`): its largest, 32 MiB, at 56 kB/s. The socket's own upgrade is a request like the others; once upgraded it's held to its own limits, below.

**Whose page.** A browser says which page opens a socket (`Origin`). The web app is at the server's own origin (`--origin`), so a socket whose `Origin` is another site's is refused with `403` before it upgrades: a page elsewhere can't use a visitor's browser, or its address, to talk to the server. The apps send no `Origin`, and aren't asked. A server whose origin is on this machine (`localhost`, a loopback address) also takes a page on this machine, as in development behind Vite's proxy; no other site's page can be one.

**Each socket** (`server.rs`, `SocketLimits`):

- **Logging in:** a socket not logged in 30 s after it opened is closed ("took too long to sign in"), whatever it sends meanwhile: the challenge lives no longer (§7). Enrolling, pairing, a recovery or a password sign-in all end in that login.
- **Quiet:** a socket nothing comes from for a minute is sent a WebSocket ping, which a browser answers by itself (the apps also ping every 30 s); nothing for 5 minutes, and it's closed ("nothing heard for too long").
- **At its own pace:** a socket's next frame goes to the hub only once the hub has done the last and what came of it is mostly written (under 1 MiB waiting); one more is held meanwhile, and nothing more read. A device that asks without reading (a room's backfill again and again) is answered as fast as it reads, and the hub's queue holds at most a frame per socket, so the queue needs no bound of its own and the hub never waits on a socket.
- **What waits for it:** at most 16 MiB (16 frames' worth, a message counted with 64 bytes of keeping beside its own), pongs to its pings among it. Past that it isn't reading: it's closed at once, and what waited dropped. A room's records, however many, go a part at a time as it writes them (above), so what waits for a socket that reads stays under its pace, a part and a message.

`/metrics` counts the sockets closed for each: `carnet_sockets_closed_total{reason="budget"}`, `{reason="login_timeout"}`, `{reason="idle"}`.

**Backup:** `carnet-server backup DIR` copies every database (`VACUUM INTO`) and the files into a folder laid out like `data/`, consistent while serving. It's the server's user's alone as `data/` is: its folders `0700`, each file made `0600` before anything is written to it. The notes and files in it are sealed; who has an account, and with passwords on, OPAQUE's records and the emails they sign in as, aren't.

**Health and metrics.** `/v1/health` is what the container's `HEALTHCHECK` asks, through `carnet-server health`. `/metrics` reports counts only: logins and refusals, records and files kept, compactions, storage failures, traffic, how long the hub takes per frame, its queue lag, sockets by protocol and app version (§14), and sockets closed for their limits. Never what anything holds.

### Files over HTTP

Files are too big for the socket, so their bytes go over HTTP, by short-lived links the socket gives (`links.rs`):

1. `BlobOffer { ids, sizes }`: does the server have these? `sizes` says how long each is sealed (its length plus 36 bytes, `crypto::sealed_len`). Each comes back `BlobStored`, or `BlobLink` with a URL to `PUT` exactly that many bytes to; one offered without its size, or past 32 MiB, gets none (`blobs::UNSIZED`). A device sends 64 ids to a frame at most (`wire::MAX_IDS`), and the server answers no more of one; `BlobFetch` and `BlobDrop` likewise.
2. The device puts the sealed bytes. A put of any other length than it said is refused, by this server or the bucket. The server answers `204` only once the file is whole on disk (synced, moved in, its folder synced). Only then does the device write path → id into the vault, so the vault never names a file the server lacks.
3. `BlobFetch { ids }`: links to fetch, each a `BlobLink` or `BlobNone`. The server serves ranges.

A device follows a link only if it's plainly the web's (`carnet_sync::link`): printable ASCII alone (no space, CR or LF to end a request line early), no user in it, `https`, or plain `http` when the server itself is reached by plain `http` (one tried out on this computer, or on a home network, its object store beside it the same way: a link then lowers nothing), and no host on this computer or its network (`localhost`; a loopback, private, link-local or unspecified address, NAT64's `64:ff9b::/96` holding one among them; a name some systems read as one, `127.1`) unless it's the server's own host, on any port (an object store beside it), and that host is on one too. Another host there, never, even for a server that is: a server can't knock on the doors around it (Docker's port, a router's page). Any other is never followed: a fetch fails, a put is offered again later. A bucket's link, on a public host of its own, is fine. On the desktop, a name is also checked as the DNS answers it (`carnet_sync::http`): its host is looked up once, and if any address it gets is on this computer or its network, the link is followed only if it's the server's own host and the address the socket is connected to is on this computer or its network itself (before a socket, every address the server's name looks up to; not just one, since that answer is the server's own DNS's to give). Then the connection goes to the socket's own address, on the link's port, whatever the DNS says now; otherwise to the addresses looked up, so a second lookup can't answer otherwise. In the browser, `fetch` does the lookup, and the browser's own rules for private addresses are all there is.

A link is one account's one file, one direction (`GET` or `PUT`), for five minutes, a put of the length the device offered it as: an HMAC-SHA256 under `data/links.key` over a fixed layout. It's not a JWT: only the server reads it, and one layout with one algorithm leaves nothing to negotiate. A leaked link opens one sealed file for minutes. With S3 storage the links are pre-signed bucket URLs instead, a put's signed with its `Content-Length` and `Content-Type: application/octet-stream` (as both apps send it), so the bucket keeps no page or script on its own origin to serve as one; devices can't tell the difference. (This server ignores a put's type, and serves every file as `application/octet-stream`, `nosniff`, sandboxed.) A put link may put its file again within its five minutes, over the first: harmless, as only the account's own device has it, at the same length, and a file that isn't the one its id names opens nowhere. The hub only decides who may; the file worker makes links and checks what was put, so neither a file nor a bucket's answer holds up the hub.

### Deletion

- **A note deleted forever** (from Deleted, after confirming): the vault marks it gone, and `RoomRemove` has the server delete the room's records and base at once and remember the room's id in a `removed` table. A device that still has the note (offline, or writing without knowing) is told `RoomRemoved` when it joins or writes, and nothing it sends is kept. The vault's own room can't be removed. Any device of the account may remove any of its rooms; the server can't read the notes to judge.
- **A file no note shows any more**, in any version of any remaining note (a note in Deleted counts), once the last note showing it is deleted forever, or a device's sweep has found it unshown for a week (§4); only Carnet's, in `attachments/`: `BlobDrop` marks it, and the server deletes it 30 days later in an hourly sweep, unless a device offers, fetches or puts it meanwhile. Unlike a note, a file is dropped by inference, so it gets the grace period.
- **Until the server confirms,** the rooms and files to remove are kept on the device (`forgotten.txt` on the desktop, the saved state in the browser) and sent again on each login, so a deletion made offline isn't lost.
- **Deleting the account** (`AccountDelete`, from any of its devices, after confirming): everything the server keeps of it goes at once (records, devices, files, slots), and every device is let go (`Denied`), each keeping its local notes. The owner's account can't be deleted this way, since it's what lets everyone else in. A host is told (§11). A link to put a file given before (good for five minutes) puts nothing after: this server refuses it (`403`), the account being gone. A bucket's link the server can't take back, so the account's removal is recorded where its files are (`gone/<account>`, before they're deleted), and the hourly sweep (the first at start) takes the files of any account whose removal is recorded there and that the store doesn't have (the account folders are listed first, then the store asked which are still accounts, so one made meanwhile isn't among them); a file put to this server while its account was being deleted goes the same way. An account the store doesn't have, and whose removal isn't recorded, keeps its files: the store may be older than the files (restored from a backup) or new, and not knowing an account isn't knowing it's gone. A store with no account at all refuses to start beside such files, unless told to (`server::Start`, `--keep-untracked-files`): the records were likely not put back (README, Restoring).

## 7. Auth and encryption

Two questions, answered by two different keys:

| | Question | Answered by | Leaves the device? |
|---|---|---|---|
| **Auth** | "Is this an enrolled device of account A?" | the device's **signing key** (Ed25519), one per device | the public half, once, at enrolment |
| **Encryption** | "Can this device read and write the notes?" | the **vault key** (256-bit, symmetric), one per epoch | never in the clear |

The server checks the first and never holds the second. A stolen server database has device public keys and wrapped vault keys: nothing to log in with, and nothing to read with.

**On the socket, authentication is the login,** once per socket: the device's signature over a fresh nonce and the server's origin. Before it, a socket may only log in, use an invite, join a pairing, recover, or sign in by password or passkey; every other frame is refused. After it, the socket belongs to that account and device, and every frame acts on that account, whatever account it names. Within an account, a frame must also fit its context: a write only to a room the device joined, a snapshot only from the lease holder, a device approved only from the pairing this device opened, invites only from the owner's devices. `crates/carnet-server/tests/auth.rs` sends what an attacker would, frame by frame, and checks each is refused.

**What the server sees,** plainly: which accounts and devices exist, when each device was last online, and, per room, a random id, how many records it has, how big, and when they were written. From `%ELO` headers it also sees which peer wrote how many operations into which room: roughly how much was typed into each note, and when. It never sees text, titles, folder or file names, device names, history, or files; those are all sealed. That's what end-to-end encryption leaves visible to any server that syncs, and worth stating rather than hiding.

### Where the keys live

| | Signing key and vault key ring |
|---|---|
| macOS | Keychain |
| Windows | Credential Manager |
| Linux | Secret Service (GNOME Keyring, KWallet); a `0600` file beside `sync.txt` when there's none, and Settings → Sync says so |
| Browser | IndexedDB, sealed with AES-GCM under a WebCrypto key the browser made non-extractable |

The desktop uses the `keyring` crate. Only on Linux, where a desktop may run no Secret Service, does a key go to a file when the keychain won't take it; a later write that the keychain does take removes the file, and the file is read first, so an older copy left in the keychain never hides a newer one. On macOS and Windows a keychain that won't keep the keys is an error, said in Settings, never a file (one left by an earlier Carnet is moved into the keychain as it's read).

In the browser, everything kept (the identity, the replica state, fetched pictures, the pictures and notes not yet sent) is sealed under that non-extractable key, each record bound to its name, so one can't be passed off as another. Script can't read the key out, but the browser keeps it with the site's data: Chromium and Firefox write it into the profile's own files, beside the database, so a copy of the profile (or of the site's storage in it) opens everything, as anything that can read the user's files can. Safari wraps it with the system's keychain, so a copy of its files alone doesn't open. What the sealing guards against is other sites, and a record read or swapped on its own, not someone with the computer's files. Nothing about sync is written to `settings.txt`.

### Logging in: a signature, not a password or a token

Every connection opens with a challenge-response.

**The nonce** ("number used once") is 32 bytes from the server's CSPRNG, fresh for each connection. It is unpredictable, so a signature can't be made in advance; single-use, forgotten the moment a `Hello` arrives, valid or not, so a captured signature can't be replayed; and short-lived, held only in memory beside the socket for 30 seconds: a socket not logged in by then is closed (§6).

**What's signed:**

```
"carnet-auth-v1" ‖ server origin ‖ account id ‖ device id ‖ nonce      (each part length-prefixed)
```

The fixed prefix keeps this signature from being valid for anything else the key signs. The origin (`https://notes.example.com`, set with `--origin`) stops a malicious server from relaying a challenge from a real one. The account and device ids pin who is logging in.

The server checks the signature against the device's public key and that the device isn't removed, then the socket is logged in. Turned away, a login is told why only as far as its app must act on it: a device removed, that it was (`Deny::DeviceRemoved`); an account not here and a device not in the account it names, the same words and code either way (`Deny::AccountRemoved`: an account deleted while its device was away is the first), so a login tells nobody which accounts there are; a device of the account signed wrongly (another key, another server's origin), only turned away. Everything after goes over that socket, except pictures' bytes (§6). No long-lived token exists to be copied from a disk or a log. Removing a device closes its open sockets at once, and every later challenge fails.

(Encryption has its own random value per message, the AES-GCM **IV**. This document says "nonce" only for the login challenge, so the two don't get mixed up.)

### Getting the vault key onto a new device

1. **The first device** makes the vault key. Settings → Sync offers to make a **recovery code**:
   - **One secret, not shares.** 128 random bits, written as 12 words from the BIP-39 English list (2,048 words, no two sharing their first four letters). 12 words carry 132 bits: 128 bits of key and a 4-bit checksum that catches most typos. Only the wordlist and checksum come from BIP-39; its seed derivation isn't used.
   - **No slow hash.** 128 random bits can't be guessed, so unlike a password they need no Argon2. HKDF derives three keys from the code: a **wrapping key**, which wraps a copy of the key ring kept on the server; a **signing key** (Ed25519), whose public half is registered, so recovery logs in through the same kind of challenge as a device; and a **box key** (X25519), which later epochs are sealed to.
2. **Pairing** (the normal way): Settings → Add a device shows a code and its QR code on a device already in. The code is `47-381095`: a **nameplate** (the smallest number free on the server) and six random digits. The new device scans it, types it, or opens the link the QR code carries (`https://server/#pair=47-381095`). The two run SPAKE2 (a PAKE: password-authenticated key exchange, the `spake2` crate) through the server, keyed by the code, so the server relaying the messages learns nothing and can't tamper unnoticed. Each side derives the same key only if the codes match. Then:
   - the new device sends its id and public key sealed under that key, so the server can't substitute its own;
   - the device in approves it into the account, and sends the key ring and account id sealed under the same key;
   - the new device logs in on the same socket.

   A nameplate is spent on the first join, right code or not, so a guesser gets one try and a second joiner racing the first finds nothing. That's why six digits are enough.
3. **Recovery,** when every device is lost: the new device derives the code's keys from the 12 words, signs the challenge with the recovery signing key (naming the new device, so the signature admits that device and no other), and is let in. The server returns the wrapped key ring and every epoch's key sealed to the code since, each signed by the device that made it and taken as a device takes one (below). Setting a new code replaces the old one, which then finds nothing. A code's key is one slot's: the server refuses one that another account's code, or any password or passkey, already signs with, so no account can claim another's way in. Likewise a device id the account has had is never given again, to another key or back to a removed device.

There's no email in self-hosting, and passwords are optional (below). Losing every device and the recovery code means the notes on the server can't be read. The local `.md` files are untouched, because they never depended on sync.

### Devices, removal and key epochs

- **Cards.** Each device sends a **card** on every login: its id, its signing key, its box key (X25519, derived from its signing key, so nothing more to keep) and its name, sealed under the vault key. The server keeps it and can't read the name. A card is believed only if it names the row it's on, so a server can't pass one device's card off as another's. The recovery code, the password and each passkey have cards too.
- **Only a card under the newest key** (`Card::open_at`) is believed, and so sealed to. Every card is sealed anew under each new epoch: the removal re-seals each device's and slot's, and each device sends its own again as it takes the key. A card under an older epoch could be any device's that was in the account then, the one since removed too: a server colluding with a removed device could hand back a card it sealed before it went, as the recovery code's (even with none set), the password's, a passkey's or a device's, its own box key in it, and the next removal would seal the next key to it, letting it back in for good. Now such a row is one that hasn't said who it is yet, and a removal waits for it ("open Carnet on each of your devices once"); a slot's, one whose card didn't open. A list read before this device took a new key is asked for again before a removal goes by it. The server keeps no card sealed under an epoch before the account's (a device away across a removal sends its old one as it logs in, just before it takes the new key): the one the removal sealed for it stays.
- **Renaming** happens on the device itself; its card is re-sent sealed, and the other devices' lists show the new name.
- **Online now.** The list (`Devices`) says which devices are logged in at the moment it's made (their places in it, added at the end, §14), and the apps say "Online now" for them rather than when their session began. A server from before says none, and the apps show when each was last seen, as before.
- **Kept fresh.** When a device comes online (its card, sent on every login) or goes offline (its last socket closed, which is also when it was last seen), the server sends the list again, unasked, to the account's other devices online. It's the `Devices` frame they already know, so an app from before takes it as it would an answer. The apps also ask again each time Settings opens, which is all a server from before gives them.
- **Removing another device** (`Revoke`): the removing device makes the next epoch's vault key, seals it to each remaining device and to every slot (recovery code, password, passkeys) with a one-off X25519 key agreement then AES-GCM, signs each (below), and re-seals their cards under it. The server accepts only if it covers every device and slot kept, no more and no fewer; otherwise it refuses (`Refused`) and nothing changes. If it accepts, in one transaction: the device is marked removed, its sockets get `Denied` and close, and each remaining device gets its key (`Rekey`) now if online, else at its next login. The removing device takes the new key only from the server's `Rekey`, so a refused removal never leaves it sealing under a key nobody else has. The removed device had every invite code in the vault (§6), so the removing one takes them all back too (`InviteDrop`, `Session::take_back_invites`) and forgets them, and says "Waiting invites were taken back".
- **Signed keys** (`rekey.rs`, protocol 2). A box alone says nothing of who sealed it, and every device that was ever in the account has every box key (they're on the cards, and never change). So a device removed, with the server's help, could seal a key of its own to each device kept at any time after, as the next epoch; each would take it, and everything written from then on, compaction's snapshots and so whole histories included, would be sealed under a key it holds. So each key goes signed by the device that made it:

  ```
  signed:  "carnet-rekey-v2" ‖ account id ‖ signer's device id ‖ the box key it's sealed to
           ‖ the epoch it follows ‖ its epoch ‖ the box                    (each part length-prefixed)
  sent:    signer's device id ‖ its card, sealed under the epoch it follows ‖ the box ‖ the signature
  ```

  The card is the signer's own (its id, signing key and box key; no name). The server keeps and relays what's sent as it did the box alone (`Revoke`'s keys, `Rekey`, `Recovered`), and can read none of it. A device, or a recovery from a slot, takes the key only if all of these hold, and otherwise leaves it out:

  - the box opens for it, and the epoch inside is the one signed;
  - it has no key for that epoch: **a key is never replaced** (`Ring::insert` won't either);
  - the epoch is the one right after the epoch the card is sealed under, and that is the newest it has: epochs go up one at a time, so a key can't skip ahead;
  - the card opens under that, its newest key, names the signer, and is a device's (an id of at most 64 bytes; a slot's is longer, the words' empty);
  - the signature verifies under the card's signing key.

  So the signer was in the account at the recipient's newest epoch. A device removed never was: it has no key for any epoch made after it went, so it can't seal a card under one, and whatever it sends is refused. A key that follows an epoch the device has no key for yet (away across two removals, the later sent first) is held, up to 64, and taken once the one before it has come; the server sends them in order anyway, and sends them all again at each login. The removing device works out the next epoch from its own newest key (`checked_add`), never from a number the server gave: if the server says the account is past it, it doesn't remove until it has caught up.
- **Leaving** (turning sync off) takes the device out without a new key, since a device leaving isn't lost or stolen.
- **Rings.** A device keeps every epoch's key, so what was written before still reads; new records are sealed under the newest. Pairing hands over the whole ring.

**What removal doesn't cover:**

- A device offline when another is removed keeps sealing under the old key until it comes back for the new one. The removed device could read those records if it ever obtained them; it can't from this server.
- The same device, or a slot whose wrapped ring is older than the removal, can't tell the new key from one the removed device makes for that same epoch: both signers were in the account at the newest epoch it has. A server colluding with the removed device could hand it that one first, which gains them no more than keeping the real one from it, as above. Every device that has the key made after a removal refuses anything the removed device sends from then on.
- A server colluding with the device being removed could slip in a device of its own making before the removal, with a card sealed under the old key the removed device knows. The new key would then be sealed to it too. Closing this needs devices to vouch for each other with their signing keys, or safety numbers compared by eye; neither exists yet.
- Records already written stay under their old epoch until compaction re-seals the room's snapshot under the newest.

### A password

Some people expect to sign in on a new device with an email and a password. That's one more way to unwrap the key ring, not a replacement for the above, and it's off unless the server runs with `--passwords` (it needs a server key to keep, and emails to hold).

It uses **OPAQUE** (the `opaque-ke` crate, which builds for wasm): the password never reaches the server, not even hashed. A stolen database allows no precomputed attack; guessing has to start after the theft, one account at a time, through Argon2id, and needs the server's `data/opaque.key` as well.

The password is the weak point against the server itself. The recovery words and a device's keys are too long to guess, but the server holds everything a guess needs, its key and the record, so a hostile server can try passwords offline as fast as it can run Argon2id. So each try is made to cost: Argon2id with 64 MiB of memory, 3 passes and 1 lane (`password::Stretch`), pinned in the code rather than left to the crate's defaults, which an upgrade could change and so make every password set before sign in to nothing. One try takes about a tenth of a second natively and a fifth in a desktop browser (a whole sign-in, measured in Chromium on an Apple laptop), several times that on a slow phone: nothing for a sign-in, a lot for a million guesses. A password is at least 10 letters (`password::MIN_LEN`); a long one, or none, is what keeps the notes safe from a server that wants them.

- **Setting one** (a device already in): OPAQUE registration, relayed. Its `export_key`, which never leaves the device, becomes a secret that works like the recovery words: it wraps a copy of the key ring, its signing key's public half names the password's slot, and its box key is in the slot's card. The server keeps OPAQUE's record and the slot, with the email in the directory to find it by. One email signs in to one account. Changing the password replaces the slot.
- **Signing in** (a new device): OPAQUE login, relayed; an unknown email gets a made-up answer and fails on the device exactly as a wrong password does. Then a recovery signed with the secret. It counts as a try for the rate limit.
- **A device removed** seals the new epoch to the password's slot too, so a password set before still works after.
- **Which email.** The email only names the password: a sign-in finds the account by it. Which emails an account may name its password after is the server's policy (`quota::PasswordEmails`, a `Host` field):
  - `Any` (the default, a server of one's own): any email no other account has. Nobody is mailed, so nothing says it's the person's; on a server of one's own that's fine, since its accounts are invited by hand.
  - `Account` (a hosted service): only the email the account came in by, as the host says (`Admission::email_of`). A hosted signup mails the invite to that email, so it's confirmed. Any other is refused with the one it must be (`On this service, the password signs in with the email you signed up with: ann@example.com`), so nobody can claim someone else's email for their password. `/v1/info` says `"password_email": "account"`, never an email; the account's own devices are told the email in their `Devices` list (`password_email`, empty for any), and Settings shows it instead of a field to type in. Passwords set before the policy stay as they are.
  - `Confirm` (not built): any email, once a code mailed to it comes back. It needs a seam to mail by and a frame for the code between `PasswordSetStart` and `PasswordSetFinish`: the server mails the code on the start, keeps it with the account for ten minutes, and takes the finish only with it.

### Passkeys

On the web, a passkey is one more way in, in the same pattern as the words and the password: a slot on the server that a secret opens. WebAuthn's **PRF extension** supplies the secret: 32 bytes, the same each time the passkey is used with the same salt, computed inside the authenticator from a key that never leaves it.

- **Making one** (a browser already in): `navigator.credentials.create` with a discoverable credential, user verification required, `user.id` the account id, and the PRF salt fixed for good (SHA-256 of `carnet passkey v1`). The PRF output becomes a secret, with its own HKDF label so it never collides with a password's, that wraps a copy of the ring and names a slot by its signing key. The slot's card holds the passkey's name and credential id, sealed. Where a passkey manager gives PRF output only on use, not on creation, the passkey is used once straight away.
- **Signing in** (a new browser): `navigator.credentials.get` with the same salt and no `allowCredentials`, so the browser asks which passkey. Then a recovery signed with the secret. **The server never verifies a WebAuthn assertion:** the signature made with the PRF-derived key is the proof, and the authenticator's user verification (face, fingerprint, PIN) guards the PRF output.
- **Slots.** Each passkey has its own, at most 20 per account; the same passkey added again replaces its slot. A slot id never collides with a device id or the password's. A removal seals the new epoch to every passkey, as to devices.
- **Taking one away** removes its slot and what was sealed to it; it signs nothing in from then on. No new epoch is needed, since the passkey can't reach the server without its slot. The browser tells the passkey manager it's gone (`signalUnknownCredential`, where supported).
- **Where it works.** Browsers with PRF support (current Chrome and Edge, Safari 18+, Firefox 135+). A passkey is bound to the site's domain: the web app at a bare IP address can't make one; at `localhost` or its own domain over HTTPS it can. The desktop app has no WebAuthn: it lists the account's passkeys and removes them, and its removals seal to them.

### Records and files

- **Records.** AES-256-GCM with a random 12-byte IV and the plaintext header as associated data, so the server can't alter a header undetected. Each room has its own key per epoch, HKDF-SHA256(vault key, `"carnet doc v1"`, epoch, room id), so a record the server moves into another room fails to decrypt there. Random 96-bit IVs are safe far beyond any note's update count. They must be random, never a counter.
- **Files.** Each is sealed on its own. Its id is HKDF of the first epoch's vault key over the file's SHA-256, so the same file is one blob in a vault, the id tells the server nothing, and rotating keys doesn't change it. (A removed device could therefore tell whether a file it has is in the vault; nothing more.) The blob is AES-256-GCM under a key for its epoch, with the epoch and id bound in, so it opens only if it's the file its id names: a server swapping blobs is caught. Files are capped at 32 MiB.
- **Randomness** comes in through a `Random` trait: the OS's on a device and on the server, `crypto.getRandomValues` in the browser, a seeded one in the simulation.

**What end-to-end encryption gives up:** server-side search, server-rendered previews, and "reset my password by email". Search runs on the device, in the browser as on the desktop.

## 8. The web app

### Why it's its own app

The web app is a separate app, not the desktop app compiled for the browser. The desktop UI draws everything itself on a canvas, and the hard parts of a web editor (text input, IMEs, selection, accessibility, scrolling on a phone) are exactly what a canvas UI is worst at in a phone browser.

It's also a device like any other: paired by a code, the recovery words, a password or a passkey. It lists notes by folder, searches them, renders them with their pictures, ticks checklists, edits and creates notes, adds pictures, makes, renames, nests and deletes folders, moves notes into them (dragged there, or by a note's "Move to" menu) and pins them, moves notes to Deleted and back or deletes them forever, shows a note's history and restores a version, manages devices, invites and recovery words, all online or offline.

**Folders from the browser.** It has no files, so it changes the vault alone, by the rules the desktop keeps for its files (`carnet_sync::vault`: `make_folder`, `rename_folder`, `move_folder`, `trash_folder`, `move_note`, `set_pinned`; the desktop's store asks the same `folder_name`, `is_inside` and `may_go`): a folder never goes into itself or a folder inside it, nor onto a name another folder there has, and none is called `attachments`; a folder renamed or moved takes its folders and notes with it, as one change; one deleted with notes in it goes to Deleted whole, its notes there (unpinned, each going back to the folder it was in, which comes back with the first put back), and an empty one is just deleted. A note moved has its pictures' paths rewritten from its new folder, sent as the browser's edit after its place (`carnet-web` `relink`), as the desktop rewrites them for its own move; the pictures stay where they are, so a folder moved from the browser is its notes moved one by one. Each desktop then does it to its files when the vault comes (§5).

### What it reuses

Only what must behave identically on every device is shared, compiled to wasm:

- **`carnet-core`:** the Markdown dialect. Colour spans, `==highlights==`, `![alt|right|300](…)` floats and the escaping rules are Carnet's own, and a JS Markdown library would render them differently. A note in the browser is `export::viewer` (real checkboxes with `data-task="n"`, `<mark>` and ink colours as CSS variables). The editor's model, `page.rs` and `history.rs` (caret, selection, every edit, undo), lives here too.
- **`carnet-sync`:** the session, the docs, sealing, pairing. Merge logic exists once.

`crates/carnet-web` (wasm-bindgen) exposes them as sans-I/O objects: `Join` (pair, recover, enroll, password, passkey) and `Vault` (frames in and out, notes, folders, HTML, search, ticking, editing, pictures, devices, invites, history, saved state). The WebSocket, IndexedDB, WebCrypto and the service worker stay in TypeScript, where the browser APIs are.

### The stack: TypeScript, Svelte 5, Vite

| Option | Verdict |
|---|---|
| Server-rendered HTML (htmx and the like) | Out: the server can't render what it can't decrypt, and even pairing needs crypto in the browser. |
| A Rust UI in wasm (Leptos, Dioxus) | One language, but the work here is platform work (service worker, install prompt, IndexedDB, iOS Safari's quirks, accessibility, a virtual list), where Rust's web tooling lags, and it adds hundreds of KB. The Rust that matters is already in the wasm. |
| **Svelte 5 + TypeScript** | Chosen: small, plain DOM code with a tiny runtime, readable by someone who doesn't live in JS. `vite-plugin-pwa` handles the service worker and manifest. |

It's a static single-page app, served by `carnet-server` at `/` (`--web DIR`, with the `.br` and `.gz` copies the build writes), so self-hosters get it from the same binary. Any other path gets `index.html`, except under `/v1/`.

### What the page may do

The app's files (not a host's own pages, not the API) carry a Content-Security-Policy: its own scripts and the wasm (`'wasm-unsafe-eval'`), inline styles (Svelte's, and the notes' colours), pictures only as the page draws them (`blob:`, `data:`), connections only to this server (its `ws(s)://` named too) and to the bucket pictures are in, if elsewhere; no plugins, frames, `<base>` or other form targets. With it, `Referrer-Policy: no-referrer`, and `Cache-Control: no-cache` on the page and the service worker, so a new build is taken at once. Every response is `X-Content-Type-Options: nosniff`; a picture's bytes, served from the app's origin, are also `Content-Disposition: attachment` and `Content-Security-Policy: sandbox`.

A note's link is a link only to `http:`, `https:`, `mailto:` (but not one asking to attach a file, `?attach=`, which some mail apps take from the computer), a relative path or `#…` (`export::safe_href`, read as a browser reads it: case aside, tabs and line breaks dropped); any other (`javascript:`, `data:`, `file:`) is its words alone, in the reader, a copy as HTML and the editor's Open. Links open in a new tab with `noopener noreferrer`. A picture on the web isn't loaded, as on the desktop (it would tell its server who reads the note): its card says so.

A pairing or invite link (`#pair=`, `#invite=`) only fills in the code: Connect is a click, so a link someone else made can't sign a browser into their account unasked. One opened while the page is open fills an empty form only, and never while connecting; with something else typed the page asks first ("Use the code from the link?", `words::linkedCode`), so a link opened again and again can't change a code being typed.

### Writing in the browser

A note opens in CodeMirror 6, but CodeMirror shows only the words: no Markdown mark is ever in its text, so none can be seen, half-typed or half-deleted. Every edit is applied by the desktop's own page model (`carnet_core::page`, in wasm) on the note's document, with the desktop's rules (typing carries the style of the letter before it, Enter continues a list, `- ` and `[] ` make lists, undo takes back a run of typing), and the note is written back by the desktop's own serialiser. So Carnet's dialect is never read or written by a second implementation. How each line looks (heading size, checkbox, bullet, quote, code, colour) also comes from the page model.

A rich-text editor (ProseMirror and the like) was ruled out: it would need its own reader and writer for Carnet's dialect, and any difference would rewrite someone's notes on their first save. Editing raw Markdown with the marks dimmed was tried and dropped: the marks read as leftovers, where the desktop shows none.

- **Sending:** half a second after a pause, and at once when the page is hidden or the editor closes. Each send is the editor's Markdown from the version it started from, merged (`NoteDoc::apply_from`) with whatever arrived meanwhile. A change from another device while editing is merged the same way, with the caret kept.
- **Tables** are edited cell by cell, each cell its own editor over the page model's cell.
- **Pictures** added from the browser are named as the desktop names a pasted one, kept sealed locally until the server has them, then put in the vault. Photos (JPEG, HEIC) are re-encoded as JPEG, at most 2560 px on the long side, with metadata such as location stripped. A picture that doesn't come takes its place as a grey card the size it would be, saying why: one the vault names and the server lacks, "Picture not found" (asked for again when the notes change); one in `attachments/` the vault doesn't name yet, "This picture hasn't reached the server yet"; any other (the user's own, beside a note or outside the notes folder), "This picture is only on the device that has it. Move it into Carnet there to sync it."
- **On phones** the toolbar rides above the keyboard (`visualViewport`).

### Storage, offline and size

- **IndexedDB** holds the identity, the state (cursor and every doc) and fetched pictures, each sealed (§7). A state that won't open is discarded and the server sends everything again. Once paired, the app asks the browser to keep its storage (`navigator.storage.persist`), since Safari can evict a site that isn't installed. On an iPhone it says to add Carnet to the Home Screen before pairing, because Safari and the Home Screen keep separate storage.
- **Offline:** the service worker precaches the app (wasm, fonts, icons). Notes and pictures come from IndexedDB; edits wait in the docs and go when the socket is back.
- **Removed elsewhere,** it forgets the account and its copy, and says so: the whole database deleted (`indexedDB.deleteDatabase`, not its records marked gone) and Carnet's own `carnet.*` settings in `localStorage` with it. The server says so before any login (`Denied`), so it could say it of anyone: with notes or pictures here it may not have, nothing is wiped on its word alone. The page says "This browser was removed from the account.", offers what wasn't sent as a ZIP of Markdown files and pictures, and forgets the browser only when asked. Which notes the server may not have is kept, sealed, as it changes (`Vault::unsent`), so a reload, when the session knows nothing yet, still knows.
- **What the server says** (a refusal, a reason, a signup's answer) is shown as its own, not Carnet's: "The server says: …", on one line, with no control or format character (Unicode's Cf: the ones that join, hide, tag or turn text around), at most 200 letters (`session::server_says`, `words::serverSays`), on the desktop too. Only the few refusals the apps know word for word (`FULL`, `READ_ONLY`, `TOO_BIG`; in joining, `pair::NO_SLOT` and an invite run out, `invite::RUN_OUT`, `invite::RUN_OUT_FIRST`) are said as Carnet's; this device's own words come apart (`Event::Failed`), never by how the server's begin. A plan's name is cleaned the same way, and a signup page is shown only if it's plainly a web page (`https://`, or `http://` from a server reached so; `link::shown_page`, `words::shownPage`).
- **Pictures fetched** are read as they come and stopped past what a picture may be (32 MiB, sealed): by the stated length before any is read, else as soon as it goes past.
- **Size:** the wasm is about 2.3 MB, about 590 KB with Brotli, which the server serves precompressed. The editor's code loads with the first edit.
- **Ten thousand notes** open in about a second; a tick redraws in under 100 ms and a search keystroke in about 30 ms. Only the rows in sight are in the page.

## 9. History and repair

Two needs, one tool:

- **A person wants an older version of a note back:** a paste gone wrong, a list cleared by mistake, a merge that came out odd (§3).
- **Sync itself needs putting right:** a bad snapshot that got past the check, or a buggy device that wrote nonsense.

Both come from the same data. Every device holds each note's full history in its doc. Everything below runs on the device; the server only ever sees ciphertext and the request to roll back. History works with sync off too, since the replica always exists (§4).

### What history records

- **When:** every change carries its time.
- **How:** each commit carries a message: `import`, `save`, `other app`, `restore`.
- **Who:** with sync on, the message also names the device that wrote it (`save by 0a1b…`), since a run's Loro peer is fresh each time. The list shows "Edited here", "Edited on Laptop" (by the device's name), or "Edited" for changes from before sync was on.
- **Versions, not keystrokes.** A writer's commits from the same origin within 10 minutes of the first are one change: a sitting of writing is one entry.

### A note's history

On the desktop it opens from the note's menu (History). The library column becomes the version list; the page shows the selected version read-only, with **Restore**, **Restore as a copy** and **Done**. The web app has the same list, marked previews and Restore.

- **The list:** newest first, each version with its time, where it was made, and words added and removed (counted as bags of words, so a moved paragraph counts as neither).
- **What changed is marked** (`carnet_core::compare`): words added since the version before are washed with the green highlighter; removed words are put back where they were, struck through in red; a block whose words stayed but which changed otherwise (a box ticked, a heading level) gets the plain highlighter. These are the note's own styles, drawn in each theme's colours. **Compare with now** marks what a restore would bring back and take away.
- **Previews never touch the live note.** They're built on `fork_at(version)`, so typing and syncing carry on underneath.
- **Restore** is `revert_to(version)`: Loro writes *new* operations that make the note equal that version. It's an edit like any other: it syncs, merges with whatever happens meanwhile, and is itself in the history, so a restore can be undone. Nothing is ever removed from history.
- **Restore as a copy** makes a new note beside it with that version's text.
- **Known limit:** text copied from a marked preview carries the marks. Restore gives the version's own text.

### Sync repair: Settings → Sync → Notes on the server

It lists each note the server holds that's worth a look, by title, in plain words with an icon per line. The vault is listed too, as "Folders and note names"; a room this device has no copy of yet, as "A note not on this device yet" (how many there are is said first, never counted as up to date), and it is never offered **Replace with this device's copy**: there is nothing here to replace the server's with.

| Shown | Comes from |
|---|---|
| up to date, or how many edits each side has that the other hasn't | version vectors |
| packed into one copy (the base snapshot): when and by which device, and saves since; or kept as N saves | the server's bases |
| whether another device has checked the packed copy | the base's check state |
| lately: packing undone, a copy replaced, a packed copy found wrong | the server's log |
| when it was checked ("Checked 2 minutes ago"), or that the notes changed since ("checking again") | when the server answered, against the session's count of changes kept and taken in |

What it shows is the server's answer when asked, so it's asked again on its own: whenever the account's notes change as this device sees them (a save of its own kept, another device's taken in, a room removed), once they have been quiet for two seconds. What the account keeps ("… kept on the server") is asked again the same way.

Each note's ways to fix it, each confirmed before it runs:

- **Undo packing** (`RollBack`): reinstate the previous base and the deltas the snapshot set aside, and drop the snapshot, while they're within their 30 days. Any of the account's devices can do it; it's logged for every device to see.
- **Replace with this device's copy** (`Repair`): when the server's state is wrong and its old records are past 30 days, a device whose own copy is good uploads a snapshot of it. The server takes it as the room's new base, whatever the old base held, and sets the old one aside for 30 days as usual.

**Two guards make repair rarely needed:**

- **Fork first.** A device never imports a downloaded snapshot straight into its note. It opens it on its own, checks that its version vector matches the header and that it reads as a note, and only then merges it. A bad snapshot is caught before it touches the local copy, and the local copy stays good enough to repair from.
- **Large removals are kept.** A change from another device that empties a note, or removes more than half of it, is applied, because it's a real edit and every copy must converge. But the text from just before goes to `.trash/` as `… (before sync).md` and shows in Deleted. The last resort stays where it has always been: plain files.

## 10. Code layout

```
Cargo.toml              the workspace
src/                    the desktop app (kui); src/sync.rs, src/files.rs, src/store_owned.rs (whose a
                        file is, Move into Carnet, the sweep) and the store's sync parts
crates/carnet-core/     the Markdown model, the editor's page and history, export, compare: no UI, builds for wasm
crates/carnet-sync/     a device's side: note and vault docs, the replica, crypto, %ELO records, the wire,
                        the session, pairing, passwords, passkeys, compatibility; with the `net` feature,
                        the desktop's socket and HTTP clients
crates/carnet-server/   the hub, storage (memory, SQLite, Postgres), pictures (files, S3), limits, the binary
crates/carnet-web/      wasm-bindgen bindings over core and sync for the web app: Join, Vault, the editor
web/                    the web app (Svelte 5, TypeScript, Vite), served by carnet-server at /
fuzz/                   cargo-fuzz targets (§12)
```

`carnet-sync`'s core does no I/O: the session, docs, sealing, pairing and the protocol state machine are bytes in, bytes out. Drivers move the bytes: the desktop's socket thread (`net`), the web app's TypeScript, and the test simulation. The same holds for the server's hub, which `server.rs` drives over real sockets and `tests/sim.rs` drives directly.

## 11. Hosting it for others

`carnet-server` as shipped is everything a self-hoster needs: accounts by invite, sync, files, backup, metrics. It's also a library, so a hosted service (public signup, plans, billing, a landing page) can be built as a separate program on top of it without forking. Nothing in this repository says `if hosted`: the server exposes seams, each with the self-hosted behaviour as its default.

**The entry point** is `carnet_server::serve_hosted(config, listener, metrics, host, stop)`. `serve` and `serve_with` call it with `Host::default()`; `serve_started` takes a `Start` besides (`keep_untracked_files`: start with no accounts beside files of accounts the store doesn't have, which is otherwise refused, §6). A `Host` provides:

| Field | Self-hosted default | What a host can do |
|---|---|---|
| `routes` | none | merge its own axum routes beside the API. A `/` among them replaces the web app's page there (a landing page); the web app is still served everywhere else, and unknown `/v1/` paths stay 404. The web app's service worker answers only `/` from its cache, so a host's pages (a signup at `/start`) reach the server even once the app is installed. |
| `quota: Arc<dyn Quota>` | `Unlimited` | per account, a byte limit (notes and files, as sealed), whether it may write, a plan name and an end date (`quota::Limit`); and the ceiling for its notes (`notes_ceiling`, 4 GiB unless said) |
| `admission: Arc<dyn Admission>` | `ByInvite` | refuse an invite before it's taken (`admit`, by its hash: refused, the device is told as of an invite never made, and the invite stays); be told which invite made which account (`admitted`), and when an account is deleted or removed (`removed`); say the email an account came in by (`email_of`) |
| `signup: Option<String>` | none | a URL where newcomers make an account. Set, the server lists the `signup` feature (in `/v1/info` and every greeting) and `/v1/info` returns the URL; the apps then offer "Make an account" in their connect steps, which asks `POST /v1/signup` (a route the host adds) to mail an invite |
| `passwords` | off | accept email and password sign-in (§7) |
| `password_emails` | `Any` | which email a password may sign in as (§7): any, or `Account`, only the one `admission.email_of` says |
| `first_invite` | on | print the owner's invite on a start with no accounts |
| `store` | SQLite files in `data/` | any `Storage`, e.g. `postgres::Postgres` (feature `postgres`) |
| `blobs` | files in `data/blobs` | any `Blobs`, e.g. `s3::S3Blobs` (feature `s3`), with pre-signed links |
| `ready` | none | receive a `HubHandle` once the hub runs |

**Its other listeners.** `server::serve_app(listener, router, stop)` serves a router as the server serves its own, each request's head in 30 s; a host's admin address can use it. Its routes merged into `routes` have the API's 30 s for a request.

**Running work on the hub.** `HubHandle::run(|hub, outs| …)` runs a closure on the hub's thread, one thing at a time like everything else there: for example, minting an invite for a new signup with `server::invite(store, false)` (never an owner's invite), or reading an account's usage. A signup flow can then hand the person the invite as a link to the web app, `https://server/#invite=ABCD-…`, which the web app redeems on open.

**Quotas and read-only accounts.** The file worker gives no upload link for a new file once the account is at its limit, and the device is told once, in plain words (`quota::FULL`, the same words as `carnet_sync::session::FULL`, so the apps know them). The device holds the files it offered back, and offers them again as soon as a `Usage` / `UsageIs` says there's room (`Usage::has_room`: writable, and under the limit), the account is writable again, or it logs in anew; failing all those, every five minutes, since room can be made without a word (files deleted on another device). The apps take "full" back with it. What's counted against the limit is what's kept, notes and files, and each link to put out and not yet used, at the length the device said, until its file is kept or no put by the link can still be coming: a link must be used within its five minutes, but a put begun then may take ten more (`BLOB_TIMEOUT`), so its room is held for both (`links::PUT_TAKES`). So a link is given while what's kept and what's on its way is under the limit, and one file may take the account over, never two, however many are offered at once or however often, slow puts begun on one round of links not freeing room for the next. A bucket's put begun in time runs as long as the bucket lets it, which this server can't cut short. At most 1,024 links to put are out for an account with a limit; past that a file offered gets no answer, and the device offers it again later. A note's text isn't refused at the limit: only past the account's ceiling for notes (`Quota::notes_ceiling`), or a room past what it holds uncompacted (§6), both far above what notes take. An account that may not write (`writable: false`, e.g. a lapsed plan) is read-only: rooms are joined with the Loro Protocol's read permission, the session sends nothing to them and raises `Event::ReadOnly` once (the apps say it once, in Settings' account card and the web's status line, not again as an error; a file offered meanwhile is held back as when full, `quota::READ_ONLY`, without that said too), and edits wait on the device until the account is writable again. `Usage` / `UsageIs` let a device show what's kept and the limit; on a self-hosted server, just what's kept. At or over the limit, the apps say so beside it, with what it means: new files stay on the device until there's room, and notes still sync. Changes the server isn't known to keep yet (offline, read only, or on their way: `Session::waiting`) are counted, and the apps say "Waiting to send N changes" rather than "Up to date".

**A limit changed is told at once.** When a host changes what its `Quota` says of an account (a plan set or ended, a suspension put on or lifted), it calls `HubHandle::limit_changed(account)` (`Hub::limit_changed` on the hub's thread), once its quota answers anew. Every socket of the account logged in is sent a `UsageIs`, unasked. A device told it can't write treats every room as read only from then on (`Event::ReadOnly`), without a write sent to be refused. A device read only until then joins its read-only rooms again, is let write, sends what waited (`Event::Writable`), and the apps take the read-only words back; the plan's line in Settings is the new one. A device from before this, told so, shows the new line and writes again at its next login. Without the call, a device learns at its next write or login.

**What stays the same.** A hosted server runs the same hub, the same protocol and the same end-to-end encryption; the seams pass account ids and invite hashes, never content. A host keeps its own data (emails, plans) in its own storage, which can be the same Postgres database in tables of its own.

## 12. How it's tested, and what it doesn't cover

`cargo test` runs everything below except the fuzzers; CI also builds the wasm and type-checks and builds the web app.

**Property tests** (proptest; failures shrink to the smallest input and are kept in `proptest-regressions/` to be replayed first):

- `carnet-core`: nothing panics on any text (styling, cutting, typing, comparing); the read–write round trip settles; styling a stretch of words always styles it.
- `carnet-sync`: a note keeps every byte; copies converge in any order of arrival, duplicates included; a merge with one side unchanged is the other side; edits to separate lines both survive; every version reads and restores; the replica remembers across reopens.
- The store, as a model: random edits, saves, edits from another app, renames behind its back, restarts, a lost replica and the trash. No typed word is lost, a saved note's file equals its doc's text, and a note keeps its key across an outside rename and a restart.

**Fuzzing** (`fuzz/`, cargo-fuzz on nightly; CI runs each target nightly on a growing corpus):

| Target | Fed | Holds |
|---|---|---|
| `markdown` | any text as a note file | read, written, read again and rendered without a panic |
| `inline` | any line, and any edit at any caret | styling, colour, links, cut, type, split and join never panic |
| `compare` | any two notes | no panic; a note compared with itself is marked nowhere |
| `note_doc` | any saves, merges, restores and reloads; any bytes as a doc or as another copy's changes | saves and restores leave exactly their text; garbage is an error, not a panic |
| `replica` | any index and doc files | opens and is used without a panic |
| `wire` | any bytes off a socket: a frame, a record, a container, a version vector | no panic, no allocation beyond what the bytes could hold; a frame that decodes re-encodes identically |
| `hub` | any frames from several sockets, opened and closed at any time, a device logging in, sockets drained of a room's records a small part at a time | the hub never panics |

The `wire` target found that the Loro Protocol crate's decoders reserve memory for whatever count they read before reading the items: four bytes could ask for 4 GB, so any client could take a server down. Every count from a socket is now checked against the bytes that follow before the crate sees it.

**The simulation** (`crates/carnet-server/tests/sim.rs`). Four devices of one account, three notes, the hub, and a network that delivers one message at a time, in random order across sockets (in order within one, as TCP does). Every step is one of: type, deliver a message, drop a socket with whatever was in flight, reconnect, restart a device (notes kept, session lost), or restart the server (sockets dropped, storage kept). Now and then one device removes another; rooms compact at a low threshold so snapshots are written, checked and built on throughout. Everything random comes from one seed, so a failure names its seed and replays exactly. When every device is back and every message delivered:

- every device has every note, with the same text;
- every word anyone typed is there;
- no typed word appears anywhere in what the server stored;
- a removed device holds no word typed after its removal, and opens no record sealed under a later epoch.

200 seeds run on every `cargo test` (more with `SIM_SEEDS=N`), alternating in-memory and SQLite storage (and Postgres when `CARNET_TEST_POSTGRES` is set). Each safety check (coverage on compaction, shutting out a removed device) has been confirmed by removing it and watching the simulation fail.

**Integration tests** (`crates/carnet-server/tests/`, on memory and SQLite): logins and authorization frame by frame (`auth.rs`), accounts never seeing each other's records (`accounts_never_meet`), pairing and removal, new epochs' keys signed and a removed device's refused, held until the key before them comes, no epoch overflowing, and no key sealed to a card under an older epoch, whatever the server hands back, while a device away across a removal is still in the next (`rekey.rs`), recovery, passwords (guesses at one slowed from any address among them), passkeys, invites, compaction and repair (a snapshot past a record's size among them, in fragments both ways), pictures, deletion, quotas, what an account may hold and ask at once (`caps.rs`: rooms that hold nothing joined only so many, across its sockets), compatibility across protocol versions, and the binary end to end over real sockets (`server.rs`), including that neither the database nor its WAL holds a word of the notes, that the data folder and what's in it are the server's user's alone, that a deleted account's put link puts nothing and pictures no account owns are swept, and that another site's page opens no socket; a socket's own limits (`sockets.rs`), a room past what may wait for it coming whole, a part at a time.

**Whole apps** (`src/app_tests.rs`, `src/sync_tests.rs`): desktop apps against the server's own code on a real socket: enrolling by invite, pairing, live edits into an open note, moves, pins, trash and folders following across devices, pictures, deletion, removal, passwords. A randomized vault test (`VAULT_SEEDS=N`) has two devices make, move, pin, rename, trash and restore notes and folders, apart and together, then checks both end with the same vault, files and notes. `crates/carnet-web/tests/viewer.rs` does the same for the web app's Rust against a desktop-like session, including a 10,000-note state.

### Known limitations

- **Text merges** can leave visible oddities in rare concurrent edits (§3). Nothing is lost.
- **Removal** doesn't protect against a server colluding with a device to slip in one of its own before that device is removed, nor against a device or slot behind by an epoch being handed the removed device's key for it first; and offline devices keep sealing under the old key until they reconnect (§7, "What removal doesn't cover").
- **Files:** a file changed in place keeps its path, and the vault takes the new one, but a device that already has the old file keeps it. Files over 32 MiB don't sync. A file typed into a note one letter at a time may be missed when deciding what's still in use; its id is kept in the vault (`-<id>`) so it can be fetched again within 30 days, and the sweep's own copy waits in `.trash/attachments/`. A device with sync set up but offline never sweeps. A file swept on one device while another device's note shows it (its text not yet come, a week on) is offered again by that device, and kept. The pictures beside notes stay on their device: the web shows a card in their place.
- **Invites:** a used invite's code stays in the vault until it runs out (7 days) while the server lists no other, never shown.
- **A first sync past 10,000 notes** takes one connection per 10,000, made one after another by the app (§6, `MAX_NEW_ROOMS`).
- **Folders:** a folder trashed whole on one device arrives as its notes, one by one, in the other's trash. A title literally equal to another note's clash name ("Plan 2" beside two "Plan"s) can number differently on two devices.
- **Web editing:** a picture's width and float can't be set from the browser; Android input methods are untested. On an iPhone, Edit doesn't raise the keyboard by itself (iOS raises it only on a tap).
- **Markdown:** punctuation is ASCII's, so other scripts' punctuation and emoji count as letters where CommonMark counts them as punctuation.

### Not built

- Sharing with other people: a read-only link to one note (its key in the URL fragment, never sent to the server) or a vault shared by several accounts (the vault key wrapped for each member). The design leaves room for both; neither exists.
- Devices vouching for each other, or safety numbers, to close the colluding-server gap above.
- Splitting the recovery code across people (SLIP-39).
- Syncing settings.

## 13. Design decisions

- **End-to-end encryption from the start.** Retrofitting it changes the server's storage, the protocol and every client; built in, it costs little, and it means a server, ours or anyone's, is trusted with availability only (§7).
- **Loro, through `LoroText::update` on the Markdown text.** The editor stays snapshot-based, other apps' edits arrive as text anyway, and the character diff merges better and keeps less history than the line diff (§3).
- **The Loro Protocol's wire format, Carnet's own room logic and feed.** The format is good and lets the server index encrypted records by header; the existing server and client implementations weren't fit for encrypted rooms. The feed tells a device about rooms it doesn't have yet (§6).
- **No outbox.** On join, the server says what it holds and the device sends the rest. Nothing besides the notes and a cursor needs to survive a crash (§5).
- **One database file per account** on the default SQLite store: isolation by construction rather than by every query remembering its `WHERE account =` (§6).
- **Opt-in, never prompts.** Sync is for those who ask for it (§1).
- **Accounts by invite on a self-hosted server.** No email, no signup form to abuse; the first code is printed at first start and the owner makes more from the app (§6).
- **Auth by device keys and pairing; passwords and passkeys optional.** Signatures over a fresh nonce leave nothing to steal from the server or a log. Passwords use OPAQUE so the server never sees one; passkeys use PRF so the server never verifies anything but a signature (§7).
- **Recovery is 128 random bits as 12 words**, not a password: no slow hash needed, nothing to guess (§7).
- **Compaction is asked for by the server** when one caught-up device has the room to itself, under a lease; what a snapshot replaces is kept 30 days, and any device can roll it back (§6, §9).
- **Files over HTTP, by short-lived signed links, for every server.** One path for self-hosted and object storage alike; links rather than tokens, because a link is one file, one direction, five minutes (§6).
- **Delete forever is final, history included; files get 30 days.** A note is deleted because someone asked and confirmed; a file is dropped by inference (no note shows it), so the server waits and a device that still wants it keeps it (§6).
- **The web app is its own app** (Svelte 5 + TypeScript), sharing only `carnet-core` and `carnet-sync` through wasm, and editing through the desktop's own page model so the Markdown dialect has one implementation (§8).
- **TLS belongs to the proxy.** Reverse proxies already do certificates well; the server stays plain HTTP (§6).
- **The server is a library with seams for hosting** rather than a fork or feature flags (§11).

## 14. Compatibility

Apps update when their owners let them, and a self-hosted server when its owner gets round to it, so an app and a server of different builds meet every day. What decides whether they can talk is the **protocol**: one number per build (`carnet_sync::compat::PROTOCOL`), raised whenever what goes over the socket changes in a way an older peer would read wrongly or not at all.

**The window.** A build talks to peers as old as `compat::OLDEST`. Peers older than `compat::DEPRECATED` are still served, and told to update. The three numbers hold `OLDEST ≤ DEPRECATED ≤ PROTOCOL`.

**Who says what.**

- The app puts its protocol in the socket's address: `/v1/sync?protocol=2`. None means 0: builds from before protocol numbers.
- The server's challenge carries its **greeting**: its protocol, its `OLDEST` and `DEPRECATED`, and its **features** by name (`compat::feature`: `passwords`, `passkeys`, `account-delete`, `remove`, `usage`, and `signup` where anyone may make an account by email). It's on every challenge, so a device joining by an invite, a code, the words, a password or a passkey reads it too, before it says anything.
- `/v1/info` says the same, for a page to read before any socket opens. The desktop's connect step asks it first (`info::checked`), and what stops it is said: a name the DNS has no address for, "Domain not found: notes.example.com. Check the address; a new name can take a few minutes to appear." (`Problem::DomainNotFound`, read from the lookup's error, `http::domain_not_found`); nothing answering (refused, timed out, offline), "Couldn't reach that server…"; something not Carnet; or a build it can't talk to. In the browser a failed `fetch` can't tell them apart, so it says only that the server couldn't be reached.
- **App too old:** the server sends the greeting, then `Denied` with the reason `Outdated`, and closes. The app says "Update Carnet to keep syncing" and retries now and then.
- **Server too old:** a greeting older than the app's `OLDEST`, or none at all. The app sends nothing and says the server needs updating.
- **Deprecated:** the app syncs as ever, and Settings asks for an update.
- **Features:** an app leaves out what its server doesn't list (passkeys, deleting the account), rather than asking and being refused.
- `Denied` carries a reason code the app acts on (`wire::Deny`: device removed, account removed, outdated, other), so its wording can change without breaking an app that reads it.

**Changes that need no new protocol.** Each side skips a frame whose tag it doesn't know (`Carnet::Unknown`), and ignores whatever follows the fields it knows. So:

- A new frame is safe: an old peer skips it. A new app that needs it from a server should check the server's features first.
- A new field is safe **at the end** of a frame: an old reader ignores it, and a new reader takes the default when it's missing (see how `Challenge` reads a greeting and `Denied` its reason).
- A tag is never reused: a retired frame's tag goes in `RETIRED` (`wire.rs` tests).

**Changes that do need one.** A field changed, moved or removed, or a frame or behaviour an old peer would misread: raise `PROTOCOL`, and keep the old peer working (a server can speak down to an app by the protocol in its address) until `OLDEST` passes it.

**Held by tests.**

- `protocol/N.txt` in `carnet-sync` holds one of every frame as protocol N's builds write them.
- Every fixture of every protocol must still decode as a frame this build knows.
- This protocol's frames must encode byte for byte as its fixture says. A change fails with instructions: raise `PROTOCOL`, keep the old fixture, and bless the new one (`CARNET_BLESS=1 cargo test -p carnet-sync wire`).
- Every tag the decoder knows needs a sample, so a new frame can't skip the fixture.
- `tests/compat.rs` in both crates covers both sides: an app reading greetings, removals and later frames, and a running server greeting an app from before protocols and skipping a later one's frames.

**The protocols so far.**

- **1:** the first.
- **2:** a new epoch's key goes signed by the device that made it, and a device takes no other (§7, "Signed keys"). `Revoke`'s keys, `Rekey` and `Recovered`'s keys are laid out as before; what they hold is a signed key, not a bare box. A protocol-1 app would read none of them (and lose every new epoch), and its own unsigned keys are refused, so the two can't share an account. Before its release `BlobOffer` gained each picture's sealed length at its end (§6, pictures): the old form still reads, with none, and gets no link. Also before it, a snapshot past a message came to go in the Loro Protocol's own fragments, both ways, and a room's records a part at a time (§6, the wire): no Carnet frame changed (`protocol/2.txt` is as it was), but a protocol-2 app from before takes no fragments, so misses a base past a message, and compacts no note past 192 KiB. `OLDEST` and `DEPRECATED` are 2 as well: no release with sync ever spoke 1, so it is dropped at once rather than over two releases. A protocol-1 app is turned away with `Outdated`, and a protocol-2 app won't talk to a protocol-1 server. Keys sealed by protocol 1 and still on a server (from a build before release) are refused; a device that missed one is paired in again with a code.

**Policy.**

- A protocol is supported for **12 months after the release that replaced it**, or two protocols back, whichever is longer.
- Dropping one takes two releases. First raise `DEPRECATED` past it, so its apps ask for an update while still syncing. A release or more later, raise `OLDEST`.
- Before raising `OLDEST`, check the server's metrics. `carnet_sockets_by_protocol` shows how many logged-in sockets still speak each protocol. `carnet_sockets_by_app` and `carnet_logins_by_app_total` show which app and version they're on: the app adds `&app=desktop-macos&version=0.1.4` (or `web`, `web-installed`) to its address. At most 64 app–version pairs are counted apart; the rest are counted as `other`.

**What's stored is versioned separately from the wire.** The server's SQLite and Postgres layouts carry their own schema version and upgrade themselves when a newer server opens them. Notes are Loro documents, which keep their own compatibility. The apps' own stores follow one rule: **no stamp means layout 1; an older one is brought up a step at a time; a newer one is refused and left as it is.**

- **The desktop's replica** (`carnet_sync::replica`) keeps its layout in `replica/<id>/version` (`replica::LAYOUT`, now 1). It covers everything in that folder (§4). A replica without the file is from before stamps and is stamped 1; a new one is stamped as it's made. An older one runs the steps in `replica::UPGRADES` in order, one per layout. Each step must be safe to run again after a crash, and the stamp moves on only after its step is done (written beside, flushed, renamed). A newer one doesn't open (`replica::Newer`) and nothing in it is touched. The notes still open from their files, without history or sync, and the app says "These notes' history was written by a newer Carnet. Update Carnet to open it."
- **The web app's IndexedDB** has the browser's own version (`VERSION` in `web/src/lib/db.ts`): `onupgradeneeded` runs the steps in `UPGRADES` from the version found, in the one transaction the browser gives it. A database a newer Carnet opened gives a `VersionError`, and the page stops and says "The notes kept in this browser were saved by a newer Carnet. Reload the page to get it." The page updates itself through its service worker, so a newer Carnet has been in this browser and the reload brings it. If another tab with a newer Carnet upgrades the database while this one is open, this one stops the same way at its next save.
- **The vault's bytes** in IndexedDB (`Vault.save`) start with a header: `CRS` and the layout's digit (`notes::STATE`, now 1). An older layout goes through `STATE_UPGRADES` as it opens and is saved in the new layout at the next save. A newer one is refused (`state_refusal`), and the page says so rather than starting over. Bytes that don't read at all start again, and the server sends everything back.
- **The vault doc** gains maps as Carnet does (`invites` and `files`, beside `notes`, `folders` and `pictures`): an older app reads only the maps it knows, and Loro merges and keeps the rest untouched, so a new map needs no protocol and no stamp. An older app shows no code for an invite (as before), and doesn't forget one used; a newer device does, when it next lists them.
- **Not stamped:** `sync.txt` and `settings.txt` are `key = value` lines, and a reader skips keys it doesn't know. An older app that rewrites one drops the keys it doesn't know, so a new key must be one an older app can lose. The keychain entry (signing key, then the ring) would need a stamp before its format changes.
