import { EditorState, type TransactionSpec } from '@codemirror/state';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { file_line, initSync, PageEditor } from '../../wasm/carnet_web.js';
import { looks, pageEditor, tilesOf, type Look } from './page';

// No window here: what pictures.ts listens for, never sent.
vi.hoisted(() => {
  const g = globalThis as unknown as { addEventListener?: () => void };
  g.addEventListener ??= () => {};
});

beforeAll(async () => {
  const fs = await import(/* @vite-ignore */ ['node', 'fs'].join(':'));
  initSync({ module: fs.readFileSync(new URL('../../wasm/carnet_web_bg.wasm', import.meta.url)) });
});

function open(md: string) {
  const pe = new PageEditor(md);
  const { extensions } = pageEditor(pe, { pictures: async () => '', edited: () => {}, moved: () => {} });
  let state = EditorState.create({ doc: pe.text(), extensions });
  return {
    pe,
    state: () => state,
    update: (spec: TransactionSpec) => (state = state.update(spec).state),
    line: (n: number) => state.doc.line(n),
    look: (n: number): Look => state.field(looks)[n - 1],
  };
}

const LINE = '[report.pdf](attachments/report.pdf) [README](attachments/README)';

describe('a line of files', () => {
  // The desktop's rule (carnet-core `doc::file_line`), through the wasm.
  it('is links to files the note keeps and nothing else', () => {
    expect(JSON.parse(file_line(LINE))).toEqual([
      ['report.pdf', 'attachments/report.pdf', 'PDF'],
      ['README', 'attachments/README', 'File'],
    ]);
    for (const words of ['See [report.pdf](attachments/report.pdf).', '[p.png](attachments/p.png)', '[x](https://x.test/a.pdf)', '**[a.pdf](attachments/a.pdf)**']) {
      expect(file_line(words), words).toBe('null');
    }
  });

  it('is a tile for each, each one letter of the words, where its tile goes', () => {
    const e = open(`# T\n\n${LINE}\n\nSee [report.pdf](attachments/report.pdf) here.\n`);
    expect(e.line(2).text).toBe('[ [');
    expect(e.look(2).tl).toEqual([
      [0, 'report.pdf', 'attachments/report.pdf', 'PDF'],
      [2, 'README', 'attachments/README', 'File'],
    ]);
    // A link in a sentence is a link.
    expect(e.line(3).text).toBe('See report.pdf here.');
    expect(e.look(3).tl).toEqual([]);
    // In its letter's place; chosen when the selection is that letter.
    const from = e.line(2).from;
    const tiles = tilesOf(e.look(2), from, { from: from + 2, to: from + 3 });
    expect(tiles.map((t) => [t.from - from, t.to - from, t.widget.name, t.widget.on])).toEqual([
      [0, 1, 'report.pdf', false],
      [2, 3, 'README', true],
    ]);
  });

  it('loses a tile whole to Backspace, and keeps a row of its own when typed beside', () => {
    const e = open(`# T\n\n${LINE}\n`);
    e.update({ selection: { anchor: e.line(2).to } });
    // The page's Backspace (page.ts's keymap runs it), shown.
    e.pe.select(1, 3, 1, 3);
    expect(e.pe.backspace(false)).toBe(true);
    expect(e.pe.markdown()).toBe('# T\n\n[report.pdf](attachments/report.pdf)\n');
    expect(e.pe.undo()).toBe(true);
    const e2 = open(`# T\n\n${LINE}\n`);
    const end = e2.line(2).to;
    e2.update({ selection: { anchor: end } });
    // Typed after the last tile, as CodeMirror hands it over: a paragraph
    // of its own under the line, the space before it nothing; the tiles
    // stay tiles, and the caret is on the new line.
    e2.update({ changes: { from: end, insert: ' x' }, userEvent: 'input.type', selection: { anchor: end + 2 } });
    expect(e2.pe.markdown()).toBe(`# T\n\n${LINE}\n\nx\n`);
    expect(e2.state().doc.line(2).text).toBe('[ [');
    expect(e2.state().doc.line(3).text).toBe('x');
    expect(e2.look(2).tl.length).toBe(2);
    expect(e2.state().selection.main.head).toBe(e2.line(3).to);
    // Before the first: a paragraph above it.
    const e3 = open(`# T\n\n${LINE}\n`);
    const start = e3.line(2).from;
    e3.update({ selection: { anchor: start } });
    e3.update({ changes: { from: start, insert: 'y' }, userEvent: 'input.type', selection: { anchor: start + 1 } });
    expect(e3.pe.markdown()).toBe(`# T\n\ny\n\n${LINE}\n`);
  });

  it('pastes a tile copied as a row of its own, never among the words', () => {
    const e = open(`# T\n\n${LINE}\n\nSome words here\n`);
    e.pe.select(1, 0, 1, 1);
    const md = e.pe.tile_copy();
    expect(md).toBe('[report.pdf](attachments/report.pdf)');
    e.pe.select(2, 4, 2, 4);
    expect(e.pe.paste(md!)).toBe(true);
    expect(e.pe.markdown()).toBe(`# T\n\n${LINE}\n\nSome words here\n\n${md}\n`);
  });

  it('goes in on a line of its own when a file is picked', () => {
    const pe = new PageEditor('# T\n\nsee here\n');
    pe.select(1, 3, 1, 3);
    expect(pe.insert_links(['plan.pdf'], ['attachments/plan.pdf'])).toBe(true);
    expect(pe.markdown()).toBe('# T\n\nsee here\n\n[plan.pdf](attachments/plan.pdf)\n');
  });
});
