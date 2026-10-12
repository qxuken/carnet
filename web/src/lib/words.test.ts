import { describe, expect, it } from 'vitest';
import { footer, linkAction, numbered, overflow, waysBack } from './words';

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
