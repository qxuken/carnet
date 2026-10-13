import { describe, expect, it } from 'vitest';
import { footer, joinedSaid, linkAction, linkedCode, numbered, overflow, serverSays, serverWords, shownPage, waysBack } from './words';

const base = { status: 'Up to date', changes: 0, pictures: 0, waitShown: false, allSent: false, readOnly: false };

describe('footer', () => {
  it('says offline and what waits as the online line does', () => {
    expect(footer({ ...base, status: 'Offline', changes: 1, waitShown: true }).text).toBe('Offline · Waiting to send 1 change');
    expect(footer({ ...base, changes: 1, waitShown: true }).text).toBe('Waiting to send 1 change');
    expect(footer({ ...base, status: 'Offline', changes: 2, pictures: 1, waitShown: true }).text).toBe('Offline · Waiting to send 2 changes and 1 picture');
  });
  it('is up to date, or all sent', () => {
    expect(footer(base)).toEqual({ ok: true, text: 'Up to date' });
    expect(footer({ ...base, allSent: true })).toEqual({ ok: true, text: 'All sent' });
    expect(footer({ ...base, readOnly: true, changes: 1, waitShown: true }).text).toBe('Read only for now: 1 change waiting');
  });
  it('says why pictures wait, when they cannot go from here', () => {
    const held = "The server's picture storage can't be reached from here, so new pictures stay on this device for now.";
    expect(footer({ ...base, pictures: 1, waitShown: true, held })).toEqual({ ok: false, text: 'Waiting to send 1 picture', why: held });
    // Said at once: it's no passing wait.
    expect(footer({ ...base, pictures: 1, held }).why).toBe(held);
    // None waiting: nothing to say.
    expect(footer({ ...base, held }).why).toBeUndefined();
    expect(footer({ ...base, pictures: 1, waitShown: true }).why).toBeUndefined();
  });
});

describe('waysBack', () => {
  it('lists only the ways the account has', () => {
    expect(waysBack({ recovery: false, password: false, passkey: false })).toBe('To come back, this browser needs a code from another of your devices.');
    expect(waysBack({ recovery: true, password: false, passkey: false })).toBe('To come back, this browser needs a code from another of your devices or the recovery words.');
    expect(waysBack({ recovery: true, password: true, passkey: true })).toBe(
      'To come back, this browser needs a code from another of your devices, a passkey, your email and password or the recovery words.',
    );
    expect(waysBack({ recovery: false, password: true, passkey: false })).toContain('your email and password');
  });
});

describe('numbered', () => {
  it('numbers a name already taken', () => {
    expect(numbered('Passkey from Chrome on Mac', [])).toBe('Passkey from Chrome on Mac');
    expect(numbered('Passkey from Chrome on Mac', ['Passkey from Chrome on Mac'])).toBe('Passkey from Chrome on Mac 2');
    expect(numbered('Passkey from Chrome on Mac', ['Passkey from Chrome on Mac', 'Passkey from Chrome on Mac 2', null])).toBe('Passkey from Chrome on Mac 3');
  });
});

describe('linkAction', () => {
  it('fills a code before an account, and says so after', () => {
    expect(linkAction('#invite=ABCD', 'setup')).toBe('fill');
    expect(linkAction('#pair=1234', 'loading')).toBe('fill');
    expect(linkAction('#pair=1234', 'ready')).toBe('already');
    expect(linkAction('#settings', 'ready')).toBeNull();
    expect(linkAction('', 'setup')).toBeNull();
  });
});

describe('overflow', () => {
  it('says which ends have more past them', () => {
    expect(overflow(0, 500, 500)).toEqual({ start: false, end: false });
    expect(overflow(0, 500, 700)).toEqual({ start: false, end: true });
    expect(overflow(100, 500, 700)).toEqual({ start: true, end: true });
    expect(overflow(200, 500, 700)).toEqual({ start: true, end: false });
  });
});

describe('linkedCode', () => {
  it('fills the form only when nothing is typed and nothing connects', () => {
    expect(linkedCode('47-381095', '', false)).toBe('fill');
    expect(linkedCode('47-381095', '  ', false)).toBe('fill');
    expect(linkedCode('47-381095', '47-381095', false)).toBe('fill');
  });
  it('asks before changing a code being typed', () => {
    expect(linkedCode('47-381095', '12-34', false)).toBe('ask');
    expect(linkedCode('66-000001', '47-381095', false)).toBe('ask');
  });
  it('leaves a code connecting alone, and a link with none', () => {
    expect(linkedCode('47-381095', '', true)).toBe('ignore');
    expect(linkedCode('47-381095', '12-34', true)).toBe('ignore');
    expect(linkedCode(null, '', false)).toBe('ignore');
    expect(linkedCode('', '', false)).toBe('ignore');
  });
});

describe('serverWords', () => {
  it('is one line, with no letter that hides or turns text around', () => {
    expect(serverWords('two\nlines\t and\u202eevil\u200b ')).toBe('two lines andevil');
    const sly = 'a\u061cb\u200b\u200ec\u2060\u2061\u2064\u206fd\ufeffe\u{e0001}\u{e0041}\u{e007f}f\u00adg\ufff9h';
    expect(serverWords(sly)).toBe('abcdefgh');
    expect(serverWords('caf\u00e9 \u{1f600}')).toBe('caf\u00e9 \u{1f600}');
  });
  it('is cut short', () => {
    const long = serverWords('\u00e9'.repeat(1000));
    expect([...long].length).toBe(201);
    expect(long.endsWith('…')).toBe(true);
  });
  it("is said as the server's own", () => {
    expect(serverSays('only the owner can')).toBe('The server says: only the owner can');
    expect(serverSays('\u200b \u202e')).toBe('');
  });
});

describe('shownPage', () => {
  it("shows a web page of the server's", () => {
    expect(shownPage('https://x.co/start', 'https:')).toBe('https://x.co/start');
    expect(shownPage('http://127.0.0.1:8787/join', 'http:')).toBe('http://127.0.0.1:8787/join');
  });
  it('shows nothing else', () => {
    for (const bad of ['javascript:alert(1)', 'data:text/html,hi', 'http://x.co/start', 'https://x.co/\u202eevil', 'https://u@x.co/', 'ftp://x.co', 'https://x.co/a b', 'not a link', '']) {
      expect(shownPage(bad, 'https:'), bad).toBe(null);
    }
    expect(shownPage(null, 'https:')).toBe(null);
    expect(shownPage(undefined, 'https:')).toBe(null);
  });
});

describe('joinedSaid', () => {
  // The finding: Settings said "A new device is in: it's in the list
  // above." long after. Said while the device is listed, and forgotten
  // once Settings is left having said it (Settings.svelte).
  it('says a device in by the code shown, while it is in the list', () => {
    const list = [{ id: 'aa' }, { id: 'bb' }];
    expect(joinedSaid('bb', list)).toBe(true);
    expect(joinedSaid('bb', [{ id: 'aa' }])).toBe(false);
    expect(joinedSaid('bb', null)).toBe(false);
    expect(joinedSaid(null, list)).toBe(false);
  });
});
