import { EditorSelection, EditorState, type TransactionSpec } from '@codemirror/state';
import { keymap, type EditorView } from '@codemirror/view';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { initSync, PageEditor } from '../../wasm/carnet_web.js';
import { looks, pageEditor } from './page';
import { STYLES, styleOfKey } from './styles';

// No window here: what pictures.ts listens for, never sent.
vi.hoisted(() => {
  const g = globalThis as unknown as { addEventListener?: () => void };
  g.addEventListener ??= () => {};
});

beforeAll(async () => {
  // No table cell focused, as a key's command asks once it runs (set
  // only now: CodeMirror reads a real page's as it loads).
  (globalThis as unknown as { document?: unknown }).document ??= { activeElement: null };
  const fs = await import(/* @vite-ignore */ ['node', 'fs'].join(':'));
  initSync({ module: fs.readFileSync(new URL('../../wasm/carnet_web_bg.wasm', import.meta.url)) });
});

/// An editor over `md`, and a stand-in for its view: enough for a key's
/// command, which reads the state and dispatches.
function open(md: string) {
  const pe = new PageEditor(md);
  const { extensions } = pageEditor(pe, { pictures: async () => '', edited: () => {}, moved: () => {} });
  let state = EditorState.create({ doc: pe.text(), extensions });
  const view = {
    get state() {
      return state;
    },
    dispatch: (spec: TransactionSpec) => (state = state.update(spec).state),
  } as unknown as EditorView;
  const press = (key: string) => {
    const b = state.facet(keymap).flat().find((k) => k.key === key);
    expect(b, key).toBeDefined();
    return b!.run!(view);
  };
  const select = (from: number, to: number) => view.dispatch({ selection: EditorSelection.single(from, to) });
  return { pe, press, select, state: () => state, line: (n: number) => state.doc.line(n) };
}

describe('underline', () => {
  it('is on the toolbar beside Strikethrough, with its icon', async () => {
    const tools = STYLES.filter((s) => s.tool).map((s) => s.label);
    expect(tools).toEqual(['Bold', 'Italic', 'Strikethrough', 'Underline']);
    const fs = await import(/* @vite-ignore */ ['node', 'fs'].join(':'));
    for (const s of STYLES.filter((s) => s.tool)) {
      expect(fs.existsSync(new URL(`../../icons/${s.icon}.svg`, import.meta.url)), s.icon).toBe(true);
    }
  });

  it('is ⌘U (Ctrl+U), in the page and in a table cell', () => {
    expect(STYLES.find((s) => s.label === 'Underline')?.key).toBe('Mod-u');
    expect(styleOfKey('u', false)).toBe(4);
    expect(styleOfKey('U', false)).toBe(4);
    expect(styleOfKey('u', true)).toBeNull();
    expect(styleOfKey('x', true)).toBe(2);
    expect(styleOfKey('b', false)).toBe(0);
    // No two styles share a key.
    expect(new Set(STYLES.map((s) => s.key)).size).toBe(STYLES.length);
  });

  it('pressed on a selection, writes <u> and draws the words underlined, the toolbar on', () => {
    const e = open('# T\n\nmake this loud\n');
    const p = e.line(2);
    e.select(p.from + 10, p.to);
    expect(e.press('Mod-u')).toBe(true);
    expect(e.pe.markdown()).toBe('# T\n\nmake this <u>loud</u>\n');
    expect(e.state().doc.toString()).toBe('T\nmake this loud');
    const runs = e.state().field(looks)[1].r;
    expect(runs.some(([a, b, flags]) => a === 10 && b === 14 && (flags & 64) !== 0)).toBe(true);
    const u = STYLES.find((s) => s.label === 'Underline')!;
    expect(e.pe.style_here() & u.flag).toBe(u.flag);
    e.select(e.line(2).from + 10, e.line(2).to);
    e.press('Mod-u');
    expect(e.pe.markdown()).toBe('# T\n\nmake this loud\n');
    expect(e.pe.style_here() & u.flag).toBe(0);
  });

  it('pressed with nothing selected, underlines what is typed next', () => {
    const e = open('# T\n\nsay\n');
    e.select(e.line(2).to, e.line(2).to);
    e.press('Mod-u');
    expect(e.pe.style_here() & 64).toBe(64);
    expect(e.pe.input(' it')).toBe(true);
    expect(e.pe.markdown()).toBe('# T\n\nsay <u>it</u>\n');
  });

  it('each style the toolbar has gives the words its flag', () => {
    for (const s of STYLES) {
      const e = open('# T\n\nword\n');
      e.select(e.line(2).from, e.line(2).to);
      e.press(s.key);
      expect(e.state().field(looks)[1].r[0][2] & s.flag, s.label).toBe(s.flag);
    }
  });
});
