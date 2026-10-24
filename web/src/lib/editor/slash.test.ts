import { EditorState, type TransactionSpec } from '@codemirror/state';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { initSync, PageEditor } from '../../wasm/carnet_web.js';
import { pageEditor } from './page';
import { commands, follow, hintOf, slashHere, slashKey } from './slash';

// No window here: what pictures.ts listens for, never sent.
vi.hoisted(() => {
  const g = globalThis as unknown as { addEventListener?: () => void };
  g.addEventListener ??= () => {};
});

// The page itself (crates/carnet-web, built by `npm run wasm`).
beforeAll(async () => {
  const fs = await import(/* @vite-ignore */ ['node', 'fs'].join(':'));
  initSync({ module: fs.readFileSync(new URL('../../wasm/carnet_web_bg.wasm', import.meta.url)) });
});

/// An editor over `md` with no view, the caret at the end of line `n`
/// (from 1); `type` types as a keyboard does, through the page.
function open(md: string, n: number) {
  const pe = new PageEditor(md);
  const { extensions } = pageEditor(pe, { pictures: async () => '', edited: () => {}, moved: () => {} });
  let state = EditorState.create({ doc: pe.text(), extensions });
  const update = (spec: TransactionSpec) => (state = state.update(spec).state);
  update({ selection: { anchor: state.doc.line(n).to } });
  const type = (s: string) => {
    for (const ch of s) {
      const at = state.selection.main.head;
      update({ changes: { from: at, insert: ch }, selection: { anchor: at + ch.length }, userEvent: 'input.type' });
    }
  };
  return { pe, type, state: () => state };
}

const names = (pe: PageEditor) => slashHere(pe)?.shown.map((i) => commands()[i].name);

describe('the / menu', () => {
  it('offers what the word picks, as the desktop does', () => {
    const e = open('# T\n\nIntro\n\n\n', 3);
    e.type('/ta');
    expect(slashHere(e.pe)).toMatchObject({ line: 2, word: 'ta' });
    expect(names(e.pe)).toEqual(['Table', 'Checklist']);
    e.type('x');
    expect(slashHere(e.pe)).toBeNull();
  });

  it('is not offered after words, nor in the title', () => {
    const e = open('# T\n\nIntro\n', 2);
    e.type(' /ta');
    expect(slashHere(e.pe)).toBeNull();
    const t = open('# T\n\nIntro\n', 1);
    t.type('/ta');
    expect(slashHere(t.pe)).toBeNull();
  });

  it('makes the line a heading, the caret on it', () => {
    const e = open('# T\n\nIntro\n\n\n', 3);
    e.type('/h2');
    expect(e.pe.run_slash(slashHere(e.pe)!.shown[0])).toBe(true);
    expect(e.pe.markdown()).toMatch(/^# T\n\nIntro\n\n## *\n/);
    expect(Array.from(e.pe.caret())).toEqual([2, 0, 2, 0]);
  });

  it('closes for its line on Esc, and comes back once the line is no command', () => {
    const e = open('# T\n\n\n', 2);
    e.type('/h');
    expect(e.pe.slash_close()).toBe(true);
    expect(slashHere(e.pe)).toBeNull();
    e.type('2');
    expect(slashHere(e.pe)).toBeNull();
    e.type(' ');
    expect(slashHere(e.pe)).toBeNull();
    e.type('/');
    expect(slashHere(e.pe)).toBeNull();
  });

  it('is on a line Shift+Enter made', () => {
    const e = open('# T\n\nhello\n', 2);
    expect(e.pe.enter(true)).toBe(true);
    expect(e.pe.text()).toBe('T\nhello\n');
    expect(Array.from(e.pe.caret())).toEqual([2, 0, 2, 0]);
    expect(e.pe.slash()).toBeUndefined();
    e.pe.input('/');
    e.pe.input('q');
    expect(names(e.pe)).toEqual(['Quote']);
  });
});

describe('its keys and rows', () => {
  const menu = { line: 1, word: 'h', shown: [0, 1, 2, 8] };
  it('go round with the arrows, and take or close', () => {
    expect(slashKey('ArrowDown', menu, 3)).toEqual({ sel: 0 });
    expect(slashKey('ArrowUp', menu, 0)).toEqual({ sel: 3 });
    expect(slashKey('Enter', menu, 1)).toEqual({ take: 1 });
    expect(slashKey('Tab', menu, 3)).toEqual({ take: 8 });
    expect(slashKey('Escape', menu, 0)).toBe('close');
    expect(slashKey('a', menu, 0)).toBeNull();
  });

  it('start over at the first for a new word', () => {
    expect(follow(menu, { ...menu, shown: [0, 1, 2] }, 2)).toBe(2);
    expect(follow(menu, { ...menu, shown: [0, 1] }, 3)).toBe(1);
    expect(follow(menu, { ...menu, word: 'he' }, 2)).toBe(0);
    expect(follow(null, menu, 2)).toBe(0);
  });

  it('say what to type next time, a picture its paste key', () => {
    const [h1, , , bullets, , , , table, divider, , picture, file] = commands();
    expect([h1.hint, bullets.hint, divider.hint, table.hint, file.hint]).toEqual(['#', '-', '---', '', '']);
    expect([hintOf(picture, true), hintOf(picture, false)]).toEqual(['⌘V', 'Ctrl+V']);
    expect(commands().map((c) => c.act)).toEqual(['turn', 'turn', 'turn', 'turn', 'turn', 'turn', 'turn', 'table', 'divider', 'code', 'picture', 'file']);
  });
});
