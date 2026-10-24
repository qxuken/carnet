import { beforeAll, describe, expect, it, vi } from 'vitest';
import { initSync, PageEditor } from '../../wasm/carnet_web.js';
import { tableKey } from './table';
import { toolSet } from './tools';

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

const NOTE = '# T\n\nWords\n\n- [ ] item\n\n```\ncode\n```\n\n---\n\n![](a.png)\n\n| A |\n| --- |\n| 1 |\n';

describe('the toolbar, as the desktop sets it', () => {
  /// The groups for the caret from line `a` (column `ac`) to `h` (`hc`).
  const at = (a: number, ac: number, h = a, hc = ac) => {
    const pe = new PageEditor(NOTE);
    pe.select(a, ac, h, hc);
    return toolSet(pe.kind_here(), pe.blocks_here());
  };

  it('shows what applies to each kind of block', () => {
    expect(at(1, 2)).toEqual(['turn', 'styles', 'insert', 'moves', 'undo']);
    expect(at(2, 2)).toEqual(['turn', 'list', 'styles', 'insert', 'moves', 'undo']);
    expect(at(3, 2)).toEqual(['moves', 'insert', 'undo']);
    expect(at(4, 0, 4, 1)).toEqual(['moves', 'insert', 'undo']);
    expect(at(5, 0, 5, 1)).toEqual(['place', 'moves', 'insert', 'undo']);
    expect(at(0, 1)).toEqual(['insert']);
  });

  it('in a table, its rows and columns, the styles and a picture', () => {
    const pe = new PageEditor(NOTE);
    pe.cell_select(6, 1, 0, 0, 0);
    expect(toolSet(pe.kind_here(), pe.blocks_here())).toEqual(['table', 'styles', 'clip', 'undo']);
  });

  it('over blocks of different kinds, what applies to any', () => {
    expect(at(1, 2, 2, 2)).toEqual(['turn', 'list', 'styles', 'moves', 'undo']);
    expect(at(3, 1, 4, 1)).toEqual(['turn', 'moves', 'undo']);
  });

  it('ticks a to-do from its tool', () => {
    const pe = new PageEditor(NOTE);
    pe.select(2, 2, 2, 2);
    expect(pe.tick_here()).toBe(1);
    expect(pe.tick()).toBe(true);
    expect(pe.tick_here()).toBe(2);
    pe.select(1, 2, 1, 2);
    expect([pe.tick_here(), pe.initial_here()]).toEqual([0, 1]);
  });
});

describe("a table's keys", () => {
  const key = (key: string, mods: string) => ({ key, altKey: mods.includes('a'), shiftKey: mods.includes('s'), ctrlKey: mods.includes('c'), metaKey: mods.includes('m') });
  it('move its row with Alt+Shift+↑↓ and its column with ⌘ or Ctrl too', () => {
    expect(tableKey(key('ArrowUp', 'as'))).toBe(5);
    expect(tableKey(key('ArrowDown', 'as'))).toBe(6);
    expect(tableKey(key('ArrowLeft', 'asc'))).toBe(7);
    expect(tableKey(key('ArrowRight', 'asm'))).toBe(8);
  });
  it('leave Alt+Shift+←→ to select by word, and move the table with Alt alone', () => {
    expect(tableKey(key('ArrowLeft', 'as'))).toBeNull();
    expect(tableKey(key('ArrowRight', 'as'))).toBeNull();
    expect(tableKey(key('ArrowUp', 'a'))).toBe(-1);
    expect(tableKey(key('ArrowDown', 'a'))).toBe(-2);
    expect(tableKey(key('ArrowUp', 's'))).toBeNull();
    expect(tableKey(key('ArrowUp', 'asc'))).toBeNull();
  });
});
