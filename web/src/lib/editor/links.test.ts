import { beforeAll, describe, expect, it, vi } from 'vitest';
import { initSync, link_hand, link_opens, PageEditor } from '../../wasm/carnet_web.js';

// No window here: a store for the setting, as a browser keeps it.
const kept = vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as unknown as { localStorage: Storage }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    removeItem: (k: string) => void m.delete(k),
  } as Storage;
  m.set('carnet.openLinks', 'double-click');
  return m;
});

import { linkOpen, linkOpenWords, pressed, setLinkOpen } from './links';

beforeAll(async () => {
  const fs = await import(/* @vite-ignore */ ['node', 'fs'].join(':'));
  initSync({ module: fs.readFileSync(new URL('../../wasm/carnet_web_bg.wasm', import.meta.url)) });
});

describe('how links open', () => {
  it('is kept in this browser, ⌘-click when none is', () => {
    expect(linkOpen()).toBe('double-click');
    setLinkOpen('click');
    expect(linkOpen()).toBe('click');
    expect(kept.get('carnet.openLinks')).toBe('click');
    setLinkOpen('cmd-click');
    expect(linkOpen()).toBe('cmd-click');
    expect(kept.has('carnet.openLinks')).toBe(false);
    expect(['Click', 'Double-click']).toEqual([linkOpenWords('click'), linkOpenWords('double-click')]);
    expect(['⌘-click', 'Ctrl-click']).toContain(linkOpenWords('cmd-click'));
  });

  // 0: no; 1: at the press; 2: once let go where it was pressed.
  it('opens by the press the setting names, and no other', () => {
    expect(link_opens('cmd-click', 1, true, false)).toBe(1);
    expect(link_opens('cmd-click', 1, false, false)).toBe(0);
    expect(link_opens('click', 1, false, false)).toBe(2);
    expect(link_opens('click', 1, true, false)).toBe(0);
    expect(link_opens('click', 1, false, true)).toBe(0);
    expect(link_opens('double-click', 1, false, false)).toBe(0);
    expect(link_opens('double-click', 2, false, false)).toBe(1);
    expect(link_opens('double-click', 1, true, false)).toBe(1);
  });

  it('shows a hand just when a press would open', () => {
    expect([link_hand('cmd-click', false, false), link_hand('cmd-click', true, false)]).toEqual([false, true]);
    expect([link_hand('click', false, false), link_hand('click', true, false), link_hand('click', false, true)]).toEqual([true, false, false]);
    expect(link_hand('double-click', false, false)).toBe(true);
  });

  // "see docs run": docs at 4..8, run (a script's) at 9..12.
  it('finds the link under a press: inside its words; never one that would run', () => {
    const pe = new PageEditor('# T\n\nsee [docs](https://a.test) [run](javascript:alert(1))\n');
    expect(pe.text().split('\n')[1]).toBe('see docs run');
    expect(pe.link_under(1, 6, 'click')).toBe('https://a.test');
    expect(pe.link_under(1, 8, 'click')).toBeUndefined();
    expect(pe.link_under(1, 8, 'cmd-click')).toBe('https://a.test');
    expect(pe.link_under(1, 2, 'click')).toBeUndefined();
    expect(pe.link_under(1, 11, 'click')).toBeUndefined();
    expect(pe.link_under(1, 11, 'cmd-click')).toBeUndefined();
  });

  // The finding: in Double-click, the second press of a triple-click
  // opened the link and the third chose its line. The third goes nowhere.
  it('takes a triple-click on a link for the double-click that opened it', () => {
    const link = (): string | null => 'https://a.test';
    let opened = false;
    const press = (n: number, url = link) => {
      const does = pressed('double-click', n, false, false, url, opened);
      opened = does === 'open' || (does === 'ignore' && opened);
      return does;
    };
    expect([press(1), press(2), press(3), press(4)]).toEqual([null, 'open', 'ignore', 'ignore']);
    // Not on a link: three presses choose the line, as anywhere.
    expect([press(1, () => null), press(2, () => null), press(3, () => null)]).toEqual([null, null, null]);
    expect(pressed('click', 1, false, false, link, false)).toBe('release');
    expect(pressed('click', 3, false, false, link, true)).toBe(null);
  });
});
