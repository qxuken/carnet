import { EditorSelection, EditorState, type TransactionSpec } from '@codemirror/state';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { initSync, PageEditor } from '../../wasm/carnet_web.js';
import { looks, pageEditor, roomFor } from './page';

// No window here: what pictures.ts listens for, never sent.
vi.hoisted(() => {
  const g = globalThis as unknown as { addEventListener?: () => void };
  g.addEventListener ??= () => {};
});

// The page itself (crates/carnet-web, built by `npm run wasm`), read from
// its file: Node has no fetch for one.
beforeAll(async () => {
  const fs = await import(/* @vite-ignore */ ['node', 'fs'].join(':'));
  initSync({ module: fs.readFileSync(new URL('../../wasm/carnet_web_bg.wasm', import.meta.url)) });
});

/// An editor over `md` with no view: its state, whose changes go through
/// the page as the view's do (page.ts's transaction filter).
function open(md: string) {
  const pe = new PageEditor(md);
  const { extensions } = pageEditor(pe, { pictures: async () => '', edited: () => {}, moved: () => {} });
  let state = EditorState.create({ doc: pe.text(), extensions });
  return {
    pe,
    state: () => state,
    update: (spec: TransactionSpec) => (state = state.update(spec).state),
    line: (n: number) => state.doc.line(n),
  };
}

/// One on a line of its own, one the words run beside.
const PICTURES = ['![](a.png)', '![|left|200](a.png)'];

describe('typed with a picture chosen', () => {
  // The finding: chosen (↓ from the line above, ← from the one below, a
  // click), "Typed" went to the start of "Below." and the picture stayed.
  // The browser's caret can't be in the picture's block: it stands at the
  // start of the line after it, and the browser types there.
  for (const pic of PICTURES) {
    it(`takes its place: ${pic}`, () => {
      for (const ends of [['from', 'to'], ['to', 'from']] as const) {
        const e = open(`# T\n\nAbove.\n\n${pic}\n\nBelow.\n`);
        expect(e.state().field(looks)[2].k).toBe('image');
        const p = e.line(3);
        e.update({ selection: EditorSelection.single(p[ends[0]], p[ends[1]]) });
        e.update({ changes: { from: e.line(4).from, insert: 'Typed' }, userEvent: 'input.type' });
        expect(e.pe.markdown()).toBe('# T\n\nAbove.\n\nTyped\n\nBelow.\n');
        expect(e.state().doc.toString()).toBe('T\nAbove.\nTyped\nBelow.');
        expect(e.state().selection.main.head).toBe(e.line(3).to);
      }
    });
  }

  it('takes its place when typed at the end of the line above', () => {
    const e = open('# T\n\nAbove.\n\n![](a.png)\n\nBelow.\n');
    e.update({ selection: EditorSelection.single(e.line(3).to, e.line(3).from) });
    e.update({ changes: { from: e.line(2).to, insert: 'Typed' }, userEvent: 'input.type' });
    expect(e.pe.markdown()).toBe('# T\n\nAbove.\n\nTyped\n\nBelow.\n');
  });

  it('is bold when Bold was switched on with it chosen', () => {
    const e = open('# T\n\n![](a.png)\n\nBelow.\n');
    e.update({ selection: EditorSelection.single(e.line(2).from, e.line(2).to) });
    expect(e.pe.style(0)).toBe(true);
    e.update({ changes: { from: e.line(3).from, insert: 'T' }, userEvent: 'input.type' });
    const at = e.state().selection.main.head;
    e.update({ changes: { from: at, insert: 'y' }, userEvent: 'input.type', selection: { anchor: at + 1 } });
    expect(e.pe.markdown()).toBe('# T\n\n**Ty**\n\nBelow.\n');
  });

  it('over a divider, which the browser types over itself, as before', () => {
    const e = open('# T\n\n---\n\nBelow.\n');
    const d = e.line(2);
    e.update({ selection: EditorSelection.single(d.from, d.to) });
    e.update({ changes: { from: d.from, to: d.to, insert: 'Typed' }, userEvent: 'input.type' });
    expect(e.pe.markdown()).toBe('# T\n\nTyped\n\nBelow.\n');
  });

  it('with none chosen, goes where it is typed', () => {
    const e = open('# T\n\n![](a.png)\n\nBelow.\n');
    e.update({ selection: { anchor: e.line(3).from } });
    e.update({ changes: { from: e.line(3).from, insert: 'Typed ' }, userEvent: 'input.type' });
    expect(e.pe.markdown()).toBe('# T\n\n![](a.png)\n\nTyped Below.\n');
  });

  // An input method's words: room made as its key goes down, before the
  // composition, which is then in the picture's place.
  it('by an input method, is composed in its place', () => {
    const e = open('# T\n\n![](a.png)\n\nBelow.\n');
    e.update({ selection: EditorSelection.single(e.line(2).to, e.line(2).from) });
    const room = roomFor(e.pe, e.state());
    expect(room).not.toBeNull();
    e.update(room!);
    expect(e.state().doc.toString()).toBe('T\n\nBelow.');
    const at = e.line(2).from;
    expect(e.state().selection.main.head).toBe(at);
    // As CodeMirror reads the composition: "k", made "か", then "ん" after it.
    const compose = (from: number, to: number, insert: string) =>
      e.update({ changes: { from, to, insert }, userEvent: 'input.type.compose', selection: { anchor: from + insert.length } });
    compose(at, at, 'k');
    compose(at, at + 1, 'か');
    compose(at + 1, at + 1, 'ん');
    expect(e.pe.markdown()).toBe('# T\n\nかん\n\nBelow.\n');
    expect(e.state().doc.toString()).toBe('T\nかん\nBelow.');
    expect(roomFor(e.pe, e.state()), 'none chosen now').toBeNull();
  });

  // As the desktop's `a_picture_straight_after_the_title_is_typed_over`:
  // a bare caret put on it from the title (the arrows choose it whole
  // themselves) is taken as choosing it, and typing takes its place.
  it('straight after the title, come to from the title, is chosen and typed over', () => {
    const e = open('# Title\n\n![x|left|220](attachments/x.png)\n\n- [ ] Book\n');
    e.update({ selection: { anchor: e.line(1).to } });
    e.update({ selection: { anchor: e.line(2).from } });
    expect(e.state().selection.main.from).toBe(e.line(2).from);
    expect(e.state().selection.main.to).toBe(e.line(2).to);
    e.update({ changes: { from: e.line(3).from, insert: 'replaced' }, userEvent: 'input.type' });
    expect(e.pe.markdown()).toBe('# Title\n\nreplaced\n\n- [ ] Book\n');
  });

  // The finding: with Bold on, an input method's words came out plain:
  // as it made "k" "か", the bold "k" went with its marks.
  it('in any paragraph, an input method keeps the bold switched on', () => {
    const e = open('# T\n\nAbove.\n\nBelow.\n');
    const end = e.line(2).to;
    e.update({ selection: { anchor: end } });
    expect(e.pe.style(0)).toBe(true);
    const compose = (from: number, to: number, insert: string) =>
      e.update({ changes: { from, to, insert }, userEvent: 'input.type.compose', selection: { anchor: from + insert.length } });
    compose(end, end, 'k');
    compose(end, end + 1, 'か');
    compose(end + 1, end + 1, 'ん');
    expect(e.pe.markdown()).toBe('# T\n\nAbove.**かん**\n\nBelow.\n');
  });
});
