// What the page says, and a few small choices it makes, worked out apart
// from the screens so they can be tested on their own (words.test.ts).

/// "1 change", "3 pictures"; nothing for none.
export const count = (n: number, one: string) => (n === 0 ? null : n === 1 ? `1 ${one}` : `${n} ${one}s`);

export type Footer = {
  /// The socket's state in words (`sync.status`).
  status: string;
  changes: number;
  pictures: number;
  /// What waits has waited long enough to be said.
  waitShown: boolean;
  /// All of it sent a moment ago.
  allSent: boolean;
  readOnly: boolean;
  /// Why the pictures wait, when it won't mend by itself (`sync.picturesHeld`).
  held?: string | null;
};

/// What the socket's state says once it's down: the browser offline, or
/// online and the server not answering.
export const offlineStatus = (online: boolean) => (online ? 'Offline: trying again' : 'Offline');

/// The footer's line: up to date only when nothing waits to be sent;
/// else how many changes and pictures wait, and, offline or read only,
/// why. (The desktop's Settings says it in the same words.) Pictures
/// that can't go from here: why too, under it (`why`). Its icon: a check
/// when all's well, a cloud struck through offline, else a clock.
export function footer(f: Footer): { ok: boolean; text: string; icon: string; why?: string } {
  const line = footerLine(f);
  const icon = line.ok ? 'check' : f.status.startsWith('Offline') ? 'cloud-off' : 'clock';
  return f.held && f.pictures > 0 ? { ...line, icon, why: f.held } : { ...line, icon };
}

function footerLine(f: Footer): { ok: boolean; text: string } {
  const parts = f.waitShown ? [count(f.changes, 'change'), count(f.pictures, 'picture')].filter((p) => p !== null) : [];
  const ok = f.status === 'Up to date';
  if (!parts.length && ok && f.allSent && !f.readOnly) return { ok: true, text: 'All sent' };
  if (!parts.length) return ok && f.readOnly ? { ok: false, text: 'Read only for now' } : { ok, text: f.status };
  const changes = parts.join(' and ');
  if (ok && f.readOnly) return { ok: false, text: `Read only for now: ${changes} waiting` };
  if (ok) return { ok: false, text: `Waiting to send ${changes}` };
  return { ok: false, text: `${f.status} · Waiting to send ${changes}` };
}

/// The ways back in for a browser that forgets the account, as the
/// account has them: a code from another device always.
export function waysBack(has: { recovery: boolean; password: boolean; passkey: boolean }): string {
  const ways = ['a code from another of your devices'];
  if (has.passkey) ways.push('a passkey');
  if (has.password) ways.push('your email and password');
  if (has.recovery) ways.push('the recovery words');
  const list = ways.length === 1 ? ways[0] : `${ways.slice(0, -1).join(', ')} or ${ways.at(-1)}`;
  return `To come back, this browser needs ${list}.`;
}

/// `name`, or with a number after it when one of `taken` has it already:
/// "Passkey from Chrome on Mac 2".
export function numbered(name: string, taken: (string | null)[]): string {
  const have = new Set(taken);
  if (!have.has(name)) return name;
  let n = 2;
  while (have.has(`${name} ${n}`)) n++;
  return `${name} ${n}`;
}

/// What to tell someone given invite `code` to the server at `origin`:
/// where it goes on the web (a link that fills it in) and in the app, in
/// the words each shows. The desktop says the same
/// (`carnet_sync::invite::instructions`).
export function inviteSteps(origin: string, code: string): string {
  const o = origin.replace(/\/+$/, '');
  return `Give them this code. On the web, they open ${o}/#invite=${code} (or ${o}, then Enter the code). In the Carnet app: Settings → Sync, Cloud sync setup, the server’s address ${o}, then Enter the code.`;
}

/// What a link opened in this tab asks for: a pairing code or an invite
/// to fill in while not yet in an account, or to say this browser is in
/// one already. `null`: not such a link.
export function linkAction(hash: string, phase: 'loading' | 'setup' | 'ready'): 'fill' | 'already' | null {
  if (!/^#(pair|invite)=/.test(hash)) return null;
  return phase === 'ready' ? 'already' : 'fill';
}

/// Which ends of a row that scrolls sideways have more past them.
export function overflow(scrollLeft: number, clientWidth: number, scrollWidth: number): { start: boolean; end: boolean } {
  return { start: scrollLeft > 1, end: scrollLeft + clientWidth < scrollWidth - 1 };
}

/// What a pairing or invite link opened while this page is open does to
/// the code form. `fill`: nothing is typed (or the same code is), and
/// nothing is connecting. `ask`: something else is typed, so a link opened
/// again and again can't change it unasked; the page asks first. `ignore`:
/// connecting now, or no code.
export function linkedCode(link: string | null, typed: string, joining: boolean): 'fill' | 'ask' | 'ignore' {
  if (!link || joining) return 'ignore';
  const now = typed.trim();
  return now === '' || now === link ? 'fill' : 'ask';
}

/// Unicode's format characters (Cf: those that join, hide, tag or turn
/// text around), and controls: dropped from the server's words.
const FORMAT = /\p{Cf}/gu;
const CONTROL = /\p{Cc}/gu;

/// Words the server sent, made fit to show: on one line, no letter that
/// draws nothing or turns the text around, and at most 200 letters (as
/// `session::server_words` has it). Format characters go first, as one
/// (U+FEFF) would otherwise count as a space.
export function serverWords(words: string): string {
  const line = words
    .replace(FORMAT, '')
    .split(/\s+/)
    .map((w) => w.replace(CONTROL, ''))
    .filter(Boolean)
    .join(' ');
  const letters = [...line];
  return letters.length > 200 ? letters.slice(0, 200).join('') + '…' : line;
}

/// The server's words, said as its own and not Carnet's: "The server
/// says: …"; nothing for none.
export function serverSays(words: string): string {
  const line = serverWords(words);
  return line ? `The server says: ${line}` : '';
}

/// A page the server names to be shown (its signup page), as written, if
/// it's plainly a web page: `https://`, or `http://` when this page is
/// itself served so; printable ASCII alone, no user in it. Anything else
/// (`javascript:`, a `data:` page, letters that turn the line around):
/// `null`, not shown.
export function shownPage(page: string | null | undefined, here: string): string | null {
  if (!page || !/^[\x21-\x7e]+$/.test(page)) return null;
  let url: URL;
  try {
    url = new URL(page);
  } catch {
    return null;
  }
  const secure = url.protocol === 'https:';
  const plain = url.protocol === 'http:' && here === 'http:';
  if (!(secure || plain) || url.username || url.password || !url.hostname) return null;
  return page;
}

/// Whether Settings says "A new device is in": one came in by the code it
/// showed (`paired`), and is in the device list above (one that left
/// since isn't). Said until Settings is left: seen then, and forgotten
/// (`sync.paired` cleared), not said again each time Settings opens.
export const joinedSaid = (paired: string | null, devices: { id: string }[] | null) => !!paired && !!devices?.some((d) => d.id === paired);
