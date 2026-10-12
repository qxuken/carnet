// The viewer's side of sync: the socket, and the wasm (crates/carnet-web)
// that does everything else. Frames from the socket go into the wasm;
// frames it has to send go out; what it keeps is kept, sealed.

import init, { Join, Vault, app_version, code_from_link, invite_from, max_sealed, protocol, state_refusal } from '../wasm/carnet_web.js';
import * as sealed from './sealed';
import * as db from './db';
import * as passkey from './passkey';
import { Shown } from './shown';
import { linkAction, numbered } from './words';

export type Note = {
  key: string;
  title: string;
  folder: string;
  pinned: boolean;
  trashed: boolean;
  snippet: string;
  done: number;
  todos: number;
  modified: number;
};

export type Device = { id: string; name: string | null; seen: number; you: boolean; online: boolean };

/// A passkey of the account: `id` names its slot, `credential` is its id
/// in the passkey manager (hex), `made` when it was added.
export type Passkey = { id: string; name: string | null; credential: string; made: number };

/// An invite waiting, and its code as the vault keeps it (null for one made
/// before Carnet kept codes there); an account on the server. The owner's
/// devices see these (docs/SYNC.md §6).
export type Invite = { hash: string; until: number; code: string | null };
export type Account = { id: string; owner: boolean; made: number; seen: number; devices: number; you: boolean };
export type Invites = { owner: boolean; waiting: Invite[]; accounts: Account[] };

/// What the account keeps on the server (sealed bytes), and may: `limit`
/// null on a server of one's own; `until` null for no end.
export type Usage = { notes: number; pictures: number; limit: number | null; writable: boolean; plan: string; until: number | null };

/// `by`: "here", a device's id, or null for a version that didn't say.
export type Past = { version: string; time: number; what: string; by: string | null; added: number; removed: number };

/// A picture to put or fetch, by its link: `body` a put's sealed bytes.
type Transfer = { id: string; url: string; put: boolean; body?: Uint8Array<ArrayBuffer> };

type Said =
  | { devices: Device[] }
  | { code: string }
  | { paired: string }
  | { pairFailed: string }
  | { recoverySet: true }
  | { rekeyed: true }
  | { removed: true; account: boolean }
  | { outdated: string; syncing: boolean }
  | { invites: Invites }
  | { uploaded: string }
  | { usage: Usage }
  | { password: { allowed: boolean; id: string | null; only: string | null } }
  | { passwordSet: true }
  | { passkeys: Passkey[] }
  | { passkeySet: true }
  | { writable: true }
  | { readOnly: true };

const IDENTITY = 'identity';
const STATE = 'state';
const PICTURE = 'picture:';
/// Said on `window` when the notes change: pictures.ts asks again for
/// those not found.
export const NOTES_CHANGED = 'carnet-notes-changed';
/// Why `picture` fails for a picture the vault names and the server
/// hasn't got; any other failure is one the vault doesn't name
/// (pictures.ts says which).
export const NOT_ON_SERVER = 'not on the server';
/// What the server says of pictures offered while the account is full
/// (`carnet_sync::session::FULL`): taken back once there's room.
const FULL = 'This account is full, so new pictures stay on this device for now. Delete some, or move to a bigger plan.';
/// A pairing or invite link opened in a browser that's in an account.
const ALREADY_IN = 'This browser is already in an account, so the link wasn’t used.';
/// Whether the server takes a new picture now (`Usage::has_room`).
const hasRoom = (u: Usage) => u.writable && (u.limit === null || u.notes + u.pictures < u.limit);
/// Pictures put in here and not yet on the server: `[path, id]`s, sealed
/// like the rest (`uploads`).
const UPLOADS = 'uploads';
/// The notes the server may not have as they are here (`Vault::unsent`),
/// sealed: kept so a reload still knows, before the server says.
const UNSENT = 'unsent';
/// The name this browser was given, if one was (sealed like the rest):
/// the browser's own if not.
const NAME = 'name';

/// What this browser is called in the other devices' lists: the browser
/// and the system, "Safari on iPhone", "Chrome on Mac".
function deviceName(): string {
  const ua = navigator.userAgent;
  const on = /iPhone/.test(ua) ? 'iPhone' : /iPad/.test(ua) ? 'iPad' : /Android/.test(ua) ? 'Android' : /Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : '';
  const browser = /Edg\//.test(ua) ? 'Edge' : /Firefox|FxiOS/.test(ua) ? 'Firefox' : /Chrome|CriOS/.test(ua) ? 'Chrome' : /Safari/.test(ua) ? 'Safari' : 'A browser';
  return on ? `${browser} on ${on}` : browser;
}

/// A message as a sentence: capital first, a full stop last.
function sentence(s: string): string {
  const t = s.trim();
  if (!t) return t;
  const c = t.charAt(0).toUpperCase() + t.slice(1);
  return /[.!?]$/.test(c) ? c : `${c}.`;
}

/// The socket, with the protocol this build speaks (a server that no
/// longer takes it says so, `compat`), and which app this is, for the
/// server's counts: the web app, installed or in a tab.
function socketUrl(): string {
  const installed = matchMedia('(display-mode: standalone)').matches;
  const app = installed ? 'web-installed' : 'web';
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/v1/sync?protocol=${protocol()}&app=${app}&version=${encodeURIComponent(app_version())}`;
}

class Sync {
  phase = $state<'loading' | 'setup' | 'ready'>('loading');
  /// In plain words, for the page to say.
  status = $state('');
  /// Changes (a note's, or the library's) the server isn't known to keep
  /// yet: offline, read only, or on their way.
  waiting = $state(0);
  /// Pictures put in here the server hasn't got yet: on their way, or
  /// held back (the account full, or read only).
  picturesWaiting = $state(0);
  error = $state<string | null>(null);
  joining = $state(false);
  // Replaced whole, never changed in place: raw, so ten thousand notes
  // aren't each wrapped to watch for changes.
  notes = $state.raw<Note[]>([]);
  folders = $state.raw<string[]>([]);
  account = $state('');
  /// Bumped when notes change, so an open note draws itself again.
  version = $state(0);
  /// The account's devices, once asked for; `null` until they come.
  devices = $state<Device[] | null>(null);
  /// What the account keeps on the server, asked with the devices.
  usage = $state<Usage | null>(null);
  /// The account can't write for now (a plan lapsed, a suspension), as
  /// the server said: the status line and Settings say so, once.
  readOnly = $state(false);
  /// The server will stop taking this app soon: why, for Settings.
  updateSoon = $state<string | null>(null);
  /// What the server said it can do (`compat::feature`), as it greeted
  /// this browser last: what Settings offers.
  features = $state<string[]>([]);
  /// With the devices: whether the server takes passwords, the email the
  /// account's password signs in as, and the only one it may here (a
  /// hosted service: the email signed up with), if the server says one.
  password = $state<{ allowed: boolean; id: string | null; only: string | null } | null>(null);
  /// Counts each password set (or taken away), for a screen to follow.
  passwordSets = $state(0);
  /// With the devices: the account's passkeys, the oldest first.
  passkeys = $state<Passkey[] | null>(null);
  /// Whether this browser can make and use passkeys Carnet can: known
  /// once started.
  passkeySupport = $state<passkey.Support>('no');
  /// A passkey being made here, until the server keeps it.
  addingPasskey = $state(false);
  /// A pairing open here: the code for the new device to type.
  code = $state<string | null>(null);
  /// The device that came in by the last code, by id.
  paired = $state<string | null>(null);
  /// Recovery words just made: shown once, kept nowhere, when the server
  /// has said it keeps the code.
  words = $state<string | null>(null);
  /// Recovery words made, the server not yet saying it keeps them: never
  /// shown before, so words it didn't take aren't written down.
  wordsWaiting = $state<string | null>(null);
  /// Whether the account has a recovery code, by the last list of
  /// devices; `null` until one has come.
  recovery = $state<boolean | null>(null);
  /// Whether this account is the server's owner's, and its invites and
  /// accounts, once asked for; `null` until they come.
  invites = $state<Invites | null>(null);
  /// The invite just made: its code, shown until put away.
  invite = $state<string | null>(null);
  /// A line said for a moment at the foot of the page, and what its
  /// button undoes, if it has one.
  notice = $state<string | null>(null);
  noticeUndo = $state<(() => void) | null>(null);
  private noticeTimer: ReturnType<typeof setTimeout> | undefined;
  /// A pairing code from the link this page was opened by (a QR code),
  /// for the pair screen to use.
  linked = $state<string | null>(null);
  /// Removed from the account, as the server says, with notes or pictures
  /// here it may not have: why, for the page to say, until this browser
  /// is forgotten. Nothing is wiped on the server's word alone.
  removed = $state<string | null>(null);
  /// Then: how many notes, and pictures, weren't sent.
  unsentCounts = $state<{ notes: number; pictures: number }>({ notes: 0, pictures: 0 });

  private vault: Vault | null = null;
  private join: Join | null = null;
  private ws: WebSocket | null = null;
  private wait = 1000;
  private retryTimer: ReturnType<typeof setTimeout> | undefined;
  private saveTimer: ReturnType<typeof setTimeout> | undefined;
  private pictures = new Shown();
  private wanted = new Map<string, { path: string; done: (url: string) => void; fail: (e: Error) => void }[]>();
  private pruneTimer: ReturnType<typeof setTimeout> | undefined;
  private wantDevices = false;
  /// The passkey a sign-in is using, by its credential id: forgotten in
  /// the passkey manager if the server has no slot for it.
  private signingInWith: Uint8Array<ArrayBuffer> | null = null;
  /// The notes the server may not have, as last kept (`UNSENT`).
  private unsent: string[] = [];

  async start() {
    try {
      await init();
    } catch {
      this.error = "Carnet couldn't load. Check the connection and reload the page.";
      return;
    }
    void passkey.support().then((s) => (this.passkeySupport = s));
    // Opened by a pairing or invite link; or one opened in this tab later.
    const linked = this.takeLink();
    addEventListener('hashchange', () => this.takeLink());
    // Back online: connected again at once, not when the wait is up.
    addEventListener('online', () => this.reconnect());
    try {
      await this.open();
    } catch (e) {
      if (e instanceof db.Newer) this.halt(e.message);
      else this.error = "Carnet couldn't open the notes kept in this browser. Reload the page to try again.";
    }
    if (linked && this.phase === 'ready') this.say(ALREADY_IN);
  }

  /// A pairing link (a QR code from another device), or a new account's
  /// (from a signup's email): its code taken off the address, so it isn't
  /// kept in the history, and filled in to connect with a click; never
  /// connected unasked. In an account already, said so. Whether it was one.
  private takeLink(): boolean {
    const act = linkAction(location.hash, this.phase);
    if (!act) return false;
    const href = location.href;
    const pairing = location.hash.startsWith('#pair=');
    history.replaceState(null, '', location.pathname + location.search);
    if (act === 'already') this.say(ALREADY_IN);
    else this.linked = (pairing ? code_from_link(href) : invite_from(href)) ?? null;
    return true;
  }

  /// Connected again now, the wait between tries begun afresh: the
  /// network is back.
  private reconnect() {
    this.wait = 1000;
    if (!this.vault || this.join) return;
    if (this.ws?.readyState === WebSocket.OPEN) return;
    clearTimeout(this.retryTimer);
    this.connect();
  }

  /// What this browser keeps, opened, and the socket.
  private async open() {
    const me = await sealed.get(IDENTITY);
    if (!me) {
      this.phase = 'setup';
      return;
    }
    this.linked = null;
    const state = (await sealed.get(STATE)) ?? new Uint8Array();
    // Kept by a newer Carnet: left as it is, not started again over.
    const refused = state_refusal(state);
    if (refused) {
      this.halt(refused);
      return;
    }
    try {
      this.vault = Vault.open(location.origin, me, await myName(), state);
    } catch {
      // The copy kept here is unreadable: start it again, and the server
      // sends everything back.
      try {
        this.vault = Vault.open(location.origin, me, await myName(), new Uint8Array());
      } catch (e) {
        this.error = String(e);
        this.phase = 'setup';
        return;
      }
    }
    this.account = this.vault.account();
    this.unsent = readList(await sealed.get(UNSENT));
    this.refresh();
    this.phase = 'ready';
    await this.resumeUploads();
    this.connect();
  }

  /// Pictures put in here before a reload and not yet sent: sent again.
  private async resumeUploads() {
    const list = (await uploads()).filter(([, id]) => id);
    for (const [path, id] of list) {
      const bytes = await sealed.get(PICTURE + id);
      if (bytes && this.vault) this.vault.upload(path, bytes);
    }
  }

  /// A picture from this device put in for a note in `folder`: kept here
  /// (sealed, as fetched ones are), shown at once, sent to the server and
  /// then the vault. Its `src`, as the note links it.
  async addPicture(folder: string, file: File): Promise<string> {
    if (!this.vault) throw new Error('not open');
    const { bytes, ext, type } = await pictureFile(file);
    const now = new Date();
    const two = (n: number) => String(n).padStart(2, '0');
    // As the desktop names a pasted picture: when, in UTC.
    const stem = `pasted-${now.getUTCFullYear()}-${two(now.getUTCMonth() + 1)}-${two(now.getUTCDate())}-${two(now.getUTCHours())}${two(now.getUTCMinutes())}${two(now.getUTCSeconds())}`;
    const path = this.vault.picture_free(stem, ext);
    const id = this.vault.upload(path, bytes);
    if (!id) throw new Error('That picture is over 32 MB, too big to sync.');
    this.flush();
    await sealed.put(PICTURE + id, bytes);
    await setUploads([...(await uploads()), [path, id]]);
    this.pictures.add(path, new Blob([bytes], { type }));
    const depth = folder ? folder.split('/').length : 0;
    return '../'.repeat(depth) + path;
  }

  private connect() {
    this.ws?.close();
    this.status = 'Connecting…';
    const ws = new WebSocket(socketUrl());
    ws.binaryType = 'arraybuffer';
    this.ws = ws;
    ws.onopen = () => {
      this.wait = 1000;
      this.updateSoon = null;
      this.vault?.connected();
    };
    ws.onmessage = (e) => {
      if (e.data instanceof ArrayBuffer) this.frame(new Uint8Array(e.data));
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.vault?.disconnected();
      // Words the server may or may not have kept: never shown.
      if (this.wordsWaiting) {
        this.wordsWaiting = null;
        this.error = 'The connection dropped before the server said it kept the new recovery code. Make one again once online.';
      }
      if (this.join) {
        this.joining = false;
        this.error ??= "Couldn't reach the server. Check the connection and try again.";
        this.dropJoin();
        return;
      }
      this.status = navigator.onLine ? 'Offline: trying again' : 'Offline';
      clearTimeout(this.retryTimer);
      this.retryTimer = setTimeout(() => this.connect(), this.wait);
      this.wait = Math.min(this.wait * 2, 60_000);
    };
  }

  private flush() {
    const out = this.join ? this.join.take_out() : this.vault?.take_out();
    for (const f of out ?? []) this.ws?.send(f);
    // After every edit and every frame: what the server isn't known to
    // keep yet.
    this.waiting = this.vault?.waiting() ?? 0;
    this.picturesWaiting = this.vault?.pictures_waiting() ?? 0;
    // What the server may not have, kept as it changes: after a reload,
    // before the server has said what it keeps, this is what knows.
    if (this.vault && !this.removed) {
      const now = this.vault.unsent(this.unsent);
      if (now.join() !== this.unsent.join()) {
        this.unsent = now;
        void sealed.put(UNSENT, new TextEncoder().encode(JSON.stringify(now)));
      }
    }
  }

  private frame(bytes: Uint8Array) {
    if (this.join) {
      this.join.receive(bytes);
      this.flush();
      const failed = this.join.failed();
      if (failed && this.join.passkey_gone() && this.signingInWith) passkey.forget(this.signingInWith);
      if (failed) {
        this.error = sentence(failed);
        this.joining = false;
        this.dropJoin();
        this.ws?.close();
        return;
      }
      const me = this.join.identity();
      const challenge = this.join.challenge();
      if (me && challenge) {
        const fresh = this.join.is_new_account();
        this.dropJoin();
        void sealed.put(IDENTITY, me);
        this.vault = Vault.open(location.origin, me, deviceName(), new Uint8Array());
        this.account = this.vault.account();
        this.vault.connected();
        this.vault.receive(challenge);
        this.flush();
        this.joining = false;
        this.linked = null;
        this.phase = 'ready';
        // Asked once it's in: that the browser not clear this site's
        // storage to make room (Safari, a site not installed).
        void navigator.storage?.persist?.();
        if (fresh) this.say('Your account is ready. In Settings, make recovery words, for when no device of yours is left.');
      }
      return;
    }
    const v = this.vault;
    if (!v) return;
    v.receive(bytes);
    this.after(v);
    if (!this.vault) return;
    const ready = v.ready();
    // Just logged in, with a screen waiting for the devices: asked now.
    if (ready && this.status !== 'Up to date' && this.wantDevices) {
      this.listDevices();
      this.listInvites();
    }
    this.status = ready ? 'Up to date' : 'Connecting…';
  }

  /// What a frame, or a picture's transfer, left to do: frames sent, notes
  /// drawn again, what was said, pictures come, transfers begun.
  private after(v: Vault) {
    this.flush();
    if (v.take_changed().length) {
      this.refresh();
      this.save();
    }
    for (const why of v.take_errors()) {
      this.error = sentence(why);
      // A passkey refused (one too many, say): not being added now.
      this.addingPasskey = false;
    }
    for (const said of JSON.parse(v.take_said()) as Said[]) this.said(said);
    if (!this.vault) return;
    this.recovery = v.recovery() ?? null;
    const features = ['passkeys', 'account-delete', 'passwords'].filter((f) => v.server_has(f));
    if (features.join() !== this.features.join()) this.features = features;
    for (const [id, data] of v.take_pictures() as [string, Uint8Array<ArrayBuffer>][]) {
      // Kept for offline: a picture's id names its bytes, so a kept copy
      // never goes stale.
      void sealed.put(PICTURE + id, data);
      for (const w of this.wanted.get(id) ?? []) {
        w.done(this.pictures.add(w.path, new Blob([data])));
      }
      this.wanted.delete(id);
    }
    // Asked for and not come (the server hasn't it yet, say): the page
    // says so, and asks again when the notes change (pictures.ts).
    for (const id of v.take_missing()) {
      for (const w of this.wanted.get(id) ?? []) w.fail(new Error(NOT_ON_SERVER));
      this.wanted.delete(id);
    }
    for (const t of v.take_transfers() as Transfer[]) void this.transfer(t);
  }

  /// A picture put or fetched over HTTP, by the short-lived link the
  /// server gave (its sealed bytes only: they open only here). A fetch
  /// is read as it comes, and stopped past what a picture may be.
  private async transfer(t: Transfer) {
    let bytes: Uint8Array | undefined;
    let error: string | undefined;
    try {
      const res = t.put
        ? await fetch(t.url, { method: 'PUT', body: t.body, headers: { 'Content-Type': 'application/octet-stream' } })
        : await fetch(t.url, { cache: 'no-store' });
      if (!res.ok) error = `HTTP ${res.status}`;
      else if (!t.put) bytes = await capped(res, max_sealed());
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
    const v = this.vault;
    if (!v) return;
    v.transferred(t.id, t.put, bytes, error);
    this.after(v);
  }

  private said(s: Said) {
    if ('devices' in s) this.devices = s.devices;
    else if ('usage' in s) {
      this.usage = s.usage;
      // Room again (a plan raised): the pictures held back are on their
      // way (notes.rs), and "full" goes.
      const u = s.usage;
      if (hasRoom(u) && this.error === FULL) this.error = null;
      if (!u.writable) this.readOnly = true;
    }
    else if ('password' in s) this.password = s.password;
    else if ('passwordSet' in s) {
      this.passwordSets++;
      this.listDevices();
    }
    else if ('passkeys' in s) this.passkeys = s.passkeys;
    else if ('passkeySet' in s) this.addingPasskey = false;
    else if ('writable' in s) {
      // Writable again (a plan renewed, a suspension lifted): the
      // read-only words go; what waited is on its way.
      this.readOnly = false;
      if (this.usage) this.usage = { ...this.usage, writable: true };
    } else if ('readOnly' in s) this.readOnly = true;
    else if ('invites' in s) this.invites = s.invites;
    else if ('code' in s) {
      this.code = s.code;
      this.paired = null;
    } else if ('paired' in s) {
      this.code = null;
      this.paired = s.paired;
      this.listDevices();
    } else if ('pairFailed' in s) {
      this.code = null;
      this.error = sentence(`Pairing ended: ${s.pairFailed}`);
    } else if ('recoverySet' in s) {
      this.words = this.wordsWaiting;
      this.wordsWaiting = null;
      // The list says the account has one now.
      this.listDevices();
    } else if ('rekeyed' in s) {
      // A device was removed: the new key kept at once, or a reload
      // would lose what it opens.
      if (this.vault) void sealed.put(IDENTITY, this.vault.identity());
      this.devices = null;
      if (this.wantDevices) this.listDevices();
    } else if ('uploaded' in s) {
      const id = s.uploaded;
      void uploads().then((list) => setUploads(list.filter(([, i]) => i !== id)));
    } else if ('outdated' in s) {
      // Too old, this app or the server: nothing syncs until one is
      // updated. Or soon too old: it goes on, and Settings says so.
      if (s.syncing) this.updateSoon = s.outdated;
      else this.error = s.outdated;
    } else if ('removed' in s) {
      const why = this.deleting
        ? 'The account is deleted, with everything the server kept of it.'
        : s.account
          ? 'This account isn’t on the server any more: it was deleted, or removed by the server’s owner.'
          : 'This browser was removed from the account.';
      const deleting = this.deleting;
      this.deleting = false;
      void this.turnedAway(why, deleting);
    }
  }

  private refresh() {
    if (!this.vault) return;
    this.notes = JSON.parse(this.vault.notes());
    this.folders = this.vault.folders();
    this.version++;
    // Pictures not found may be there now.
    dispatchEvent(new Event(NOTES_CHANGED));
    this.prune();
  }

  /// Pictures kept here that no note shows any more, dropped, a while
  /// after the notes last changed.
  private prune() {
    clearTimeout(this.pruneTimer);
    this.pruneTimer = setTimeout(async () => {
      if (!this.vault) return;
      const named = new Set(this.vault.picture_ids());
      // Not yet in the vault, still kept: those on their way.
      for (const [, id] of await uploads()) named.add(id);
      const gone = (await db.keys()).filter((k) => k.startsWith(PICTURE) && !named.has(k.slice(PICTURE.length)));
      if (gone.length) await db.delMany(gone);
    }, 10_000);
  }

  /// Kept a moment after the last change, sealed.
  private save() {
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => void this.keepNow(), 1500);
  }

  /// Comes into the account by a code shown on another device.
  pair(code: string) {
    this.begin(() => Join.pair(code));
  }

  /// Comes in by an email and password set on another device.
  signIn(email: string, password: string) {
    this.begin(() => Join.password(location.origin, email, password));
  }

  /// Sets (or changes) the account's password: `passwordSets` counts up
  /// when it's done.
  setPassword(email: string, password: string): boolean {
    if (!this.vault) return false;
    this.error = null;
    try {
      this.vault.set_password(email, password);
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
      return false;
    }
    this.flush();
    return true;
  }

  /// The account's deletion, asked from here.
  private deleting = false;

  /// The whole account deleted: the server's goodbye takes this browser
  /// back to the first screen, saying so.
  deleteAccount() {
    if (!this.vault) return;
    this.deleting = true;
    this.vault.delete_account();
    this.flush();
  }

  dropPassword() {
    this.vault?.drop_password();
    this.flush();
  }

  /// A new passkey for the account, made here: the browser asks which
  /// passkey manager keeps it, and for a fingerprint, a face or a PIN. In
  /// the list once the server keeps it.
  async addPasskey() {
    if (!this.vault?.ready() || this.addingPasskey) return;
    this.error = null;
    this.addingPasskey = true;
    try {
      const have = (this.passkeys ?? []).map((p) => passkey.fromHex(p.credential)).filter((c) => c.length);
      const made = await passkey.make(new Uint8Array(this.vault.account_id()), `Carnet notes (${this.account})`, have);
      try {
        this.vault.add_passkey(numbered(`Passkey from ${deviceName()}`, (this.passkeys ?? []).map((p) => p.name)), made.credential, made.prf);
      } finally {
        made.prf.fill(0);
      }
      this.flush();
    } catch (e) {
      this.addingPasskey = false;
      this.error = sentence(e instanceof Error ? e.message : String(e));
    }
  }

  /// A passkey taken away: it lets no device in from now. The passkey
  /// manager is told, where the browser can.
  removePasskey(p: Passkey) {
    this.error = null;
    try {
      this.vault?.remove_passkey(p.id);
      this.flush();
      if (p.credential) passkey.forget(passkey.fromHex(p.credential));
    } catch (e) {
      this.error = sentence(e instanceof Error ? e.message : String(e));
    }
  }

  /// Comes into the account by a passkey made on another device (or this
  /// browser, before it forgot): the browser asks which.
  async signInWithPasskey() {
    this.error = null;
    this.joining = true;
    let used;
    try {
      used = await passkey.use();
    } catch (e) {
      this.joining = false;
      this.error = sentence(e instanceof Error ? e.message : String(e));
      return;
    }
    this.signingInWith = used.credential;
    this.begin(() => Join.passkey(location.origin, used.prf));
    // The wasm has its own copy, and wipes it once used.
    used.prf.fill(0);
  }

  /// Makes a new account by an invite: this browser its first device.
  enroll(code: string) {
    this.begin(() => Join.enroll(code));
  }

  /// Comes in by a code: an invite makes a new account, any other is
  /// from a device already in.
  enter(code: string) {
    if (invite_from(code)) this.enroll(code);
    else this.pair(code);
  }

  /// Comes into the account by the twelve words.
  recover(words: string) {
    this.begin(() => Join.recover(location.origin, words));
  }

  private begin(make: () => Join) {
    this.error = null;
    this.dropJoin();
    try {
      this.join = make();
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
      this.joining = false;
      return;
    }
    this.joining = true;
    this.connect();
  }

  html(key: string): string {
    return this.vault?.html(key) ?? '';
  }

  toggle(key: string, n: number) {
    if (this.vault?.toggle_task(key, n)) {
      this.flush();
      this.refresh();
      this.save();
    }
  }

  search(query: string): Set<string> {
    return new Set(this.vault?.search(query) ?? []);
  }

  /// Where a picture a note in `folder` shows is, as a path from the
  /// notes folder: its `src`, and whether an Obsidian embed names it.
  picturePath(folder: string, src: string, wiki: boolean): string | undefined {
    return this.vault?.picture_path(folder, src, wiki);
  }

  /// The picture at `path` (from the notes folder), as a URL to show:
  /// one kept while it's on screen, else made again from the copy kept
  /// for offline, else fetched.
  picture(path: string): Promise<string> {
    const have = this.pictures.get(path);
    if (have) return Promise.resolve(have);
    const id = this.vault?.picture_id(path);
    if (!id) return Promise.reject(new Error('not in the vault'));
    return sealed.get(PICTURE + id).then(
      (kept) =>
        new Promise((done, fail) => {
          if (kept) {
            return done(this.pictures.add(path, new Blob([kept])));
          }
          if (!this.vault?.get_picture(path)) return fail(new Error('not in the vault'));
          this.flush();
          const list = this.wanted.get(id) ?? [];
          list.push({ path, done, fail });
          this.wanted.set(id, list);
        }),
    );
  }

  /// The account's devices, asked for (and again as they change, while
  /// a screen shows them).
  listDevices() {
    this.wantDevices = true;
    if (!this.vault?.ready()) return;
    this.vault.list_devices();
    this.vault.usage();
    this.flush();
  }

  /// Stops listing devices: the screen showing them closed.
  doneWithDevices() {
    this.wantDevices = false;
  }

  /// Takes another device out of the account.
  removeDevice(id: string) {
    this.error = null;
    try {
      this.vault?.remove_device(id);
      this.flush();
    } catch (e) {
      this.error = sentence(e instanceof Error ? e.message : String(e));
    }
  }

  /// What this browser is called in the others' lists, changed: kept here,
  /// and told at once. A blank name gives it back the browser's own.
  async rename(name: string) {
    const chosen = name.split(/\s+/).filter(Boolean).join(' ').slice(0, 64);
    if (chosen) await sealed.put(NAME, new TextEncoder().encode(chosen));
    else await db.delMany([NAME]);
    this.vault?.rename(chosen || deviceName());
    this.flush();
  }

  /// Whether this account is the server's owner's, and if so its invites
  /// and the accounts: they come in `invites`.
  listInvites() {
    if (!this.vault?.ready()) return;
    this.vault.list_invites();
    this.flush();
  }

  /// A new invite, for someone's own account on this server: its code in
  /// `invite`, until put away.
  makeInvite() {
    this.error = null;
    this.invite = this.vault?.make_invite() ?? null;
    this.flush();
    // Its code is in the vault now: kept, so a reload still shows it.
    this.save();
  }

  inviteDone() {
    this.invite = null;
  }

  /// An invite not used yet, taken back.
  dropInvite(hash: string) {
    this.error = null;
    this.vault?.drop_invite(hash);
    if (this.invites?.waiting.some((w) => w.hash === hash && w.code === this.invite)) this.invite = null;
    this.flush();
    this.save();
  }

  /// Another person's account taken off the server, with all it keeps.
  removeAccount(id: string) {
    this.error = null;
    this.vault?.remove_account(id);
    this.flush();
  }

  /// Opens a pairing: the code comes in `code`.
  pairOpen() {
    this.error = null;
    this.paired = null;
    this.vault?.pair_open();
    this.flush();
  }

  pairCancel() {
    this.vault?.pair_cancel();
    this.flush();
    this.code = null;
  }

  /// Whether the account has a recovery code a new one would replace.
  hasRecovery(): boolean {
    return this.vault?.has_recovery() ?? false;
  }

  /// A new recovery code: its words in `words` once the server keeps it,
  /// until `wordsDone`.
  makeRecovery() {
    this.error = null;
    this.words = null;
    this.wordsWaiting = this.vault?.make_recovery() ?? null;
    this.flush();
  }

  wordsDone() {
    this.words = null;
  }

  /// Says `text` for a few seconds, with Undo if `undo` is given.
  say(text: string, undo: (() => void) | null = null) {
    this.notice = text;
    this.noticeUndo = undo;
    clearTimeout(this.noticeTimer);
    this.noticeTimer = setTimeout(() => {
      this.notice = null;
      this.noticeUndo = null;
    }, undo ? 6000 : 4000);
  }

  /// A note's Markdown and its version, for the editor.
  source(key: string): [string, string] | null {
    return (this.vault?.source(key) as [string, string] | undefined) ?? null;
  }

  /// `text`, typed from the note at `base`: sent, and the merged text and
  /// its version back.
  edit(key: string, base: string, text: string): [string, string] | null {
    const r = this.vault?.edit(key, base, text) as [string, string] | undefined;
    if (!r) return null;
    this.flush();
    this.refresh();
    this.save();
    return r;
  }

  /// A new note in `folder`: its key. It's sent with its first words.
  create(folder: string): string | null {
    return this.vault?.create(folder) ?? null;
  }

  /// A new note left blank, dropped.
  discard(key: string) {
    this.vault?.discard(key);
  }

  /// The note to Deleted, or back, on every device.
  setTrashed(key: string, trashed: boolean): boolean {
    if (!this.vault?.set_trashed(key, trashed)) return false;
    this.flush();
    this.refresh();
    this.save();
    return true;
  }

  /// A note in Deleted, deleted forever: its history too, on every device,
  /// and the pictures only it showed.
  deleteForever(key: string): boolean {
    if (!this.vault?.delete_forever(key)) return false;
    this.flush();
    this.refresh();
    this.save();
    return true;
  }

  /// What's kept, kept now: the page is going out of sight (on a phone,
  /// perhaps for good).
  async keepNow() {
    clearTimeout(this.saveTimer);
    if (!this.vault) return;
    // Both taken now: the vault may be gone by the time the first is kept.
    const state = this.vault.save();
    const me = this.vault.identity();
    try {
      await sealed.put(STATE, state);
      await sealed.put(IDENTITY, me);
    } catch (e) {
      // A newer Carnet, in another tab, took the database on.
      if (e instanceof db.Newer) this.halt(e.message);
      else throw e;
    }
  }

  /// What this browser keeps is a newer Carnet's: this page stops before
  /// it writes over any of it, and says to reload. The newer Carnet is
  /// what the service worker has, or is fetching now.
  private halt(why: string) {
    const ws = this.ws;
    this.ws = null;
    ws?.close();
    clearTimeout(this.saveTimer);
    clearTimeout(this.pruneTimer);
    this.dropVault();
    this.error = why;
    this.phase = 'loading';
    void navigator.serviceWorker?.getRegistration().then((r) => r?.update()).catch(() => {});
  }

  history(key: string): Past[] {
    return JSON.parse(this.vault?.history(key) ?? '[]');
  }

  pastHtml(key: string, version: string): string {
    return this.vault?.past_html(key, version) ?? '';
  }

  /// The note made what it was at `version`, on every device.
  restore(key: string, version: string): boolean {
    if (!this.vault?.restore(key, version)) return false;
    this.flush();
    this.refresh();
    this.save();
    return true;
  }

  /// This browser leaves the account: taken out of it on the server, then
  /// its keys and its copy of the notes forgotten. The notes stay on the
  /// other devices.
  async forget() {
    if (this.vault?.ready()) {
      try {
        this.vault.remove_device(this.vault.device());
        this.flush();
        // A moment for the message to go before the socket closes.
        for (let i = 0; i < 20 && (this.ws?.bufferedAmount ?? 0) > 0; i++) await new Promise((r) => setTimeout(r, 50));
      } catch {
        // Offline: it is forgotten here all the same; the other devices
        // can remove it from their lists.
      }
    }
    await this.leave(null);
  }

  /// Removed from the account, as the server says (`Denied`, which it
  /// sends before any login, so it could say it of anyone): with nothing
  /// here it may not have, everything kept here goes ([`leave`]). With
  /// notes or pictures not sent, nothing is wiped on its word alone: the
  /// page says so, offers them to download, then forgets this browser
  /// when asked ([`forgetRemoved`]).
  private async turnedAway(why: string, deleting: boolean) {
    const v = this.vault;
    if (deleting || !v) return this.leave(why);
    const pictures = await uploads();
    if (this.vault !== v) return;
    const notes = v.unsent(this.unsent);
    if (!notes.length && !pictures.length && !v.pictures_waiting()) return this.leave(why);
    const ws = this.ws;
    this.ws = null;
    ws?.close();
    this.unsent = notes;
    await this.keepNow();
    this.unsentCounts = { notes: v.unsent_notes_count(notes), pictures: pictures.length };
    this.removed = why;
  }

  /// The notes and pictures not sent, as one ZIP file, downloaded.
  async downloadUnsent() {
    const v = this.vault;
    if (!v) return;
    const paths: string[] = [];
    const files: Uint8Array[] = [];
    for (const [path, id] of await uploads()) {
      const bytes = await sealed.get(PICTURE + id);
      if (bytes) {
        paths.push(path);
        files.push(bytes);
      }
    }
    if (this.vault !== v) return;
    const zip = v.unsent_zip(this.unsent, paths, files);
    const url = URL.createObjectURL(new Blob([new Uint8Array(zip)], { type: 'application/zip' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = 'Carnet notes not sent.zip';
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 60_000);
  }

  /// Removed, and what wasn't sent dealt with: this browser forgotten.
  forgetRemoved() {
    void this.leave(this.removed);
  }

  /// The vault let go of, its keys wiped from the wasm's memory.
  private dropVault() {
    const v = this.vault;
    this.vault = null;
    v?.free();
  }

  /// The joining let go of, likewise.
  private dropJoin() {
    const j = this.join;
    this.join = null;
    j?.free();
  }

  /// Everything kept here dropped, and back to the start: the database
  /// deleted whole, and the page's own settings in this browser.
  private async leave(why: string | null) {
    const ws = this.ws;
    this.ws = null;
    ws?.close();
    clearTimeout(this.retryTimer);
    clearTimeout(this.saveTimer);
    clearTimeout(this.pruneTimer);
    this.dropVault();
    this.dropJoin();
    try {
      await db.destroy();
    } catch {
      // Not deleted (the browser refused): every record cleared instead.
      await db.clear().catch(() => {});
    }
    sealed.drop();
    try {
      for (const k of Object.keys(localStorage)) if (k.startsWith('carnet.')) localStorage.removeItem(k);
    } catch {
      // No storage to clear: a private window, storage refused.
    }
    this.unsent = [];
    this.removed = null;
    this.unsentCounts = { notes: 0, pictures: 0 };
    this.pictures.clear();
    this.notes = [];
    this.folders = [];
    this.devices = null;
    this.passkeys = null;
    this.addingPasskey = false;
    this.invites = null;
    this.invite = null;
    this.code = null;
    this.words = null;
    this.wordsWaiting = null;
    this.recovery = null;
    this.usage = null;
    this.readOnly = false;
    this.status = '';
    this.error = why;
    this.phase = 'setup';
  }
}

export const sync = new Sync();

/// A list kept as JSON, sealed; none for one not kept, or not a list.
function readList(kept: Uint8Array | null): string[] {
  try {
    const v = kept ? JSON.parse(new TextDecoder().decode(kept)) : [];
    return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : [];
  } catch {
    return [];
  }
}

/// The pictures put in here and not yet on the server (`UPLOADS`). Kept
/// unsealed by an earlier Carnet: read once, and sealed.
async function uploads(): Promise<[string, string][]> {
  const kept = await sealed.get(UPLOADS);
  if (kept) {
    try {
      return JSON.parse(new TextDecoder().decode(kept)) as [string, string][];
    } catch {
      return [];
    }
  }
  const before = await db.get<unknown>(UPLOADS);
  if (!Array.isArray(before)) return [];
  const list = before.filter((e): e is [string, string] => Array.isArray(e) && typeof e[0] === 'string' && typeof e[1] === 'string');
  await setUploads(list);
  return list;
}

const setUploads = (list: [string, string][]) => sealed.put(UPLOADS, new TextEncoder().encode(JSON.stringify(list)));

/// An answer's body, read as it comes, refused past `most` bytes: by its
/// stated length before any is read, else as soon as it goes past.
async function capped(res: Response, most: number): Promise<Uint8Array> {
  const said = Number(res.headers.get('Content-Length'));
  if (said > most) {
    void res.body?.cancel();
    throw new Error('an answer past what a picture may be');
  }
  if (!res.body) return new Uint8Array(await res.arrayBuffer());
  const reader = res.body.getReader();
  const parts: Uint8Array[] = [];
  let got = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    got += value.length;
    if (got > most) {
      void reader.cancel();
      throw new Error('an answer past what a picture may be');
    }
    parts.push(value);
  }
  const out = new Uint8Array(got);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

/// What this browser is called: the name it was given, or its own.
async function myName(): Promise<string> {
  const kept = await sealed.get(NAME);
  return kept ? new TextDecoder().decode(kept) : deviceName();
}

/// A picture file as the desktop takes it. PNG, GIF and WebP (a
/// screenshot, a drawing) go as they are. A photo (JPEG, or an iPhone's
/// HEIC) is drawn again: the right way up, at most `PHOTO` pixels on its
/// long side, and with nothing the camera wrote in it, such as where it
/// was taken.
async function pictureFile(file: File): Promise<{ bytes: Uint8Array<ArrayBuffer>; ext: string; type: string }> {
  const kept: Record<string, string> = { 'image/png': 'png', 'image/gif': 'gif', 'image/webp': 'webp' };
  const ext = kept[file.type];
  if (ext) return { bytes: new Uint8Array(await file.arrayBuffer()), ext, type: file.type };
  const img = await createImageBitmap(file, { imageOrientation: 'from-image' });
  const k = Math.min(1, PHOTO / Math.max(img.width, img.height));
  const canvas = document.createElement('canvas');
  canvas.width = Math.round(img.width * k);
  canvas.height = Math.round(img.height * k);
  const g = canvas.getContext('2d')!;
  g.imageSmoothingQuality = 'high';
  g.drawImage(img, 0, 0, canvas.width, canvas.height);
  img.close();
  const blob = await new Promise<Blob | null>((ok) => canvas.toBlob(ok, 'image/jpeg', 0.88));
  if (!blob) throw new Error('That picture couldn’t be read.');
  return { bytes: new Uint8Array(await blob.arrayBuffer()), ext: 'jpg', type: 'image/jpeg' };
}

/// A photo's long side at most, in pixels: sharp across a wide window on
/// a high-density screen.
const PHOTO = 2560;
