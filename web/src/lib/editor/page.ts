// CodeMirror over the desktop's own page (crates/carnet-web `PageEditor`):
// it shows the note's words alone, a line for each line of the note, and
// every edit goes to the page, whose rules are the desktop's. No mark is
// in CodeMirror's text, so none can be seen or typed half of.
//
// Typing (and the input method's composing, and a phone's Backspace and
// Enter, which come as input rather than keys) is let through, then made
// on the page too; when the page's words come out otherwise (a style's
// marks completed, `- ` turned into a list), the change becomes the page's.
// While the input method composes, nothing is changed under it.

import { Annotation, EditorSelection, EditorState, StateEffect, StateField, type Extension, type Range, type Transaction, type TransactionSpec } from '@codemirror/state';
import { Decoration, EditorView, ViewPlugin, WidgetType, keymap, type DecorationSet, type ViewUpdate } from '@codemirror/view';
import type { PageEditor } from '../../wasm/carnet_web.js';
import { HIGHLIGHT, inkCss, washCss } from '../colors';
import { missing, unshown } from '../pictures';
import { TableWidget, place, type Cell, type TableContext } from './table';
import { pictureTools } from './picture';

/// How a line is drawn (`PageEditor.looks`).
export type Look = {
  k: string;
  f: boolean;
  i: number;
  d: boolean;
  n: number;
  r: [number, number, number, number, number][];
  src: string;
  /// The `src` is an Obsidian embed's, a name to look for.
  wk: boolean;
  alt: string;
  w: number | null;
  /// A picture's place: 0 its own line, 1 left of the text, 2 right.
  fl: number;
  cells: Cell[][];
};

export const TURN = { text: 0, h1: 1, h2: 2, h3: 3, bullet: 4, number: 5, todo: 6, quote: 7 } as const;
export { HIGHLIGHT };

/// Ours: a change the page made, not to be made on it again.
const own = Annotation.define<boolean>();
const setLooks = StateEffect.define<Look[]>();
export const looks = StateField.define<Look[]>({
  create: () => [],
  update(v, tr) {
    for (const e of tr.effects) if (e.is(setLooks)) return e.value;
    return v;
  },
});

/// `now` made `next`: one change, from where they first differ to where
/// they last do.
function diff(now: string, next: string) {
  if (now === next) return [];
  let a = 0;
  while (a < now.length && a < next.length && now[a] === next[a]) a++;
  let b = 0;
  while (b < now.length - a && b < next.length - a && now[now.length - 1 - b] === next[next.length - 1 - b]) b++;
  return [{ from: a, to: now.length - b, insert: next.slice(a, next.length - b) }];
}

/// Line and column (from 0) of `pos` in `doc`.
function lc(doc: { lineAt(p: number): { number: number; from: number } }, pos: number): [number, number] {
  const l = doc.lineAt(pos);
  return [l.number - 1, pos - l.from];
}

/// The page's selection as offsets in `text`.
function selectionIn(text: string, caret: Uint32Array | number[]) {
  const starts = [0];
  for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10) starts.push(i + 1);
  const at = (line: number, col: number) => Math.min(text.length, (starts[Math.min(line, starts.length - 1)] ?? 0) + col);
  return EditorSelection.single(at(caret[0], caret[1]), at(caret[2], caret[3]));
}

/// What the page is now, as one transaction from `startDoc`.
function pageSpec(pe: PageEditor, startDoc: string): TransactionSpec {
  const text = pe.text();
  return {
    changes: diff(startDoc, text),
    selection: selectionIn(text, pe.caret()),
    effects: setLooks.of(JSON.parse(pe.looks())),
    annotations: own.of(true),
    scrollIntoView: true,
  };
}

/// The page's selection set from CodeMirror's.
function tell(pe: PageEditor, state: { doc: EditorState['doc']; selection: EditorSelection }) {
  const r = state.selection.main;
  const [al, ac] = lc(state.doc, r.anchor);
  const [hl, hc] = lc(state.doc, r.head);
  pe.select(al, ac, hl, hc);
}

/// A change CodeMirror is making, made on the page. `null` when the
/// page's words agree (CodeMirror's change stands, untouched, so an input
/// method composing is never disturbed); else the page's change instead.
function mirror(pe: PageEditor, tr: Transaction): TransactionSpec | null {
  const start = tr.startState;
  const composing = tr.isUserEvent('input.type.compose');
  const parts: { from: number; to: number; text: string }[] = [];
  tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => parts.push({ from: fromA, to: toA, text: inserted.toString() }));
  const sel = start.selection.main;
  const one = parts.length === 1 ? parts[0] : null;
  tell(pe, start);
  if (one && !composing && tr.isUserEvent('delete.backward') && !one.text && sel.empty && one.to === sel.head) {
    // A phone's Backspace: the page's, which undoes structure first.
    pe.backspace(one.to - one.from > 1);
  } else if (one && !composing && tr.isUserEvent('delete.forward') && !one.text && sel.empty && one.from === sel.head) {
    pe.delete(one.to - one.from > 1);
  } else if (one && !composing && /^\r?\n$/.test(one.text) && one.from === one.to) {
    // A phone's Enter.
    pe.enter(false);
  } else if (one && !composing && one.text.includes('\n')) {
    pe.select(...lc(start.doc, one.from), ...lc(start.doc, one.to));
    pe.paste(one.text);
  } else {
    // The changes from the last, so each earlier place stays where it was.
    for (const p of parts.reverse()) {
      pe.select(...lc(start.doc, p.from), ...lc(start.doc, p.to));
      pe.input(p.text);
    }
  }
  if (composing || pe.text() === tr.newDoc.toString()) return null;
  return { ...pageSpec(pe, start.doc.toString()), userEvent: 'input' };
}

// ------------------------------------------------------------ widgets

class Box extends WidgetType {
  constructor(readonly done: boolean, readonly line: number, readonly act: (line: number) => void) {
    super();
  }
  eq(o: Box) {
    return o.done === this.done && o.line === this.line;
  }
  toDOM() {
    const b = document.createElement('input');
    b.type = 'checkbox';
    b.className = 'cm-box';
    b.checked = this.done;
    b.setAttribute('aria-label', this.done ? 'Done' : 'Not done');
    b.addEventListener('mousedown', (e) => e.preventDefault());
    b.addEventListener('click', (e) => {
      e.preventDefault();
      this.act(this.line);
    });
    return b;
  }
  ignoreEvent() {
    return true;
  }
}

class Marker extends WidgetType {
  constructor(readonly text: string, readonly cls: string) {
    super();
  }
  eq(o: Marker) {
    return o.text === this.text && o.cls === this.cls;
  }
  toDOM() {
    const s = document.createElement('span');
    s.className = this.cls;
    s.textContent = this.text;
    s.setAttribute('aria-hidden', 'true');
    return s;
  }
}

class Placeholder extends WidgetType {
  constructor(readonly text: string) {
    super();
  }
  eq(o: Placeholder) {
    return o.text === this.text;
  }
  toDOM() {
    const s = document.createElement('span');
    s.className = 'cm-ph';
    s.textContent = this.text;
    return s;
  }
}

class Rule extends WidgetType {
  eq() {
    return true;
  }
  toDOM() {
    const s = document.createElement('span');
    s.className = 'cm-rule';
    return s;
  }
}

/// A picture's `src` (an Obsidian embed's, with `wiki`) as a URL to show.
export type Pictures = (src: string, wiki: boolean) => Promise<string>;

class Picture extends WidgetType {
  constructor(readonly src: string, readonly wiki: boolean, readonly alt: string, readonly width: number | null, readonly pictures: Pictures) {
    super();
  }
  eq(o: Picture) {
    return o.src === this.src && o.wiki === this.wiki && o.width === this.width && o.alt === this.alt;
  }
  toDOM() {
    const img = document.createElement('img');
    img.className = 'cm-picture waiting';
    img.alt = this.alt;
    if (this.width) img.style.width = `${this.width}px`;
    // In a box of its own, for a card saying why it isn't shown (not
    // found, or one this browser can't draw) to take its place in.
    const box = document.createElement('span');
    box.className = 'cm-picture-box';
    box.append(img);
    unshown(img, this.src);
    this.pictures(this.src, this.wiki).then(
      (url) => {
        img.src = url;
        img.classList.remove('waiting');
      },
      () => missing(img, this.src),
    );
    return box;
  }
  /// Another width or alt text for the same picture (it was resized):
  /// the one shown kept, not fetched and drawn again.
  updateDOM(dom: HTMLElement, _view: EditorView, from: Picture) {
    if (from.src !== this.src || from.wiki !== this.wiki) return false;
    const img = dom.querySelector<HTMLImageElement>('img.cm-picture');
    if (img) {
      img.style.width = this.width ? `${this.width}px` : '';
      img.alt = this.alt;
    }
    return true;
  }
}

function build(view: EditorView, pictures: Pictures, toggle: (line: number) => void, tables: TableContext): { deco: DecorationSet; atomic: DecorationSet } {
  const all = view.state.field(looks);
  const doc = view.state.doc;
  const out: Range<Decoration>[] = [];
  const atomic: Range<Decoration>[] = [];
  if (all.length !== doc.lines) return { deco: Decoration.none, atomic: Decoration.none };
  for (const { from, to } of view.visibleRanges) {
    for (let n = doc.lineAt(from).number; n <= doc.lineAt(to).number; n++) {
      const line = doc.line(n);
      const look = all[n - 1];
      const cls = [`cm-${look.k}`];
      if (look.f && n > 1) cls.push('cm-block');
      if (look.d) cls.push('cm-ticked');
      // A picture at an edge: there on its line (the words beside it are
      // the reading view's), at most 45% wide with no width given.
      if (look.k === 'image' && look.fl) cls.push(look.fl === 2 ? 'cm-float-right' : 'cm-float-left');
      if (look.k === 'image' && look.w) cls.push('cm-sized');
      const attrs: Record<string, string> = { class: cls.join(' ') };
      if (look.i) attrs.style = `--indent:${look.i}`;
      out.push(Decoration.line({ attributes: attrs }).range(line.from));
      if (look.k === 'title' && !line.length) out.push(Decoration.widget({ widget: new Placeholder('Untitled'), side: 1 }).range(line.from));
      if (look.k === 'todo') out.push(Decoration.widget({ widget: new Box(look.d, n - 1, toggle), side: -1 }).range(line.from));
      if (look.k === 'bullet') out.push(Decoration.widget({ widget: new Marker(['•', '◦', '▪'][look.i % 3], 'cm-dot'), side: -1 }).range(line.from));
      if (look.k === 'number') out.push(Decoration.widget({ widget: new Marker(`${look.n}.`, 'cm-num'), side: -1 }).range(line.from));
      if (look.k === 'divider' || look.k === 'image' || look.k === 'table') {
        const w = look.k === 'divider' ? new Rule() : look.k === 'image' ? new Picture(look.src, look.wk, look.alt, look.w, pictures) : new TableWidget(look.cells, n - 1, tables);
        const d = Decoration.replace({ widget: w });
        out.push(d.range(line.from, line.to));
        atomic.push(d.range(line.from, line.to));
      }
      for (const [a, b, flags, mark, ink] of look.r) {
        const c: string[] = [];
        if (flags & 1) c.push('cm-b');
        if (flags & 2) c.push('cm-i');
        if (flags & 4) c.push('cm-s');
        if (flags & 8) c.push('cm-c');
        if (flags & 16) c.push('cm-l');
        let style = '';
        if (ink) style += `color:${inkCss(ink)};`;
        if (mark) style += `background-color:${washCss(mark)};`;
        if (mark) c.push('cm-marked');
        const spec: { class: string; attributes?: Record<string, string> } = { class: c.join(' ') };
        if (style) spec.attributes = { style };
        if (b > a) out.push(Decoration.mark(spec).range(line.from + a, line.from + Math.min(b, line.length)));
      }
    }
  }
  return { deco: Decoration.set(out, true), atomic: Decoration.set(atomic, true) };
}

export type PageOptions = {
  pictures: Pictures;
  /// The page changed: to be sent.
  edited: () => void;
  /// The caret moved or the page changed: for the toolbar.
  moved: () => void;
};

/// The editor over `pe`, and `run`, which makes a page command and shows
/// what it did.
export function pageEditor(pe: PageEditor, o: PageOptions) {
  let view: EditorView | null = null;

  /// The caret is in a table's cell (which has the browser's focus).
  const inCell = () => !!document.activeElement?.closest('.cm-tablewrap');

  /// A command on the page, from CodeMirror's selection (or the cell's,
  /// which the page has already); what it did, shown.
  const run = (f: () => boolean) => (v: EditorView) => {
    if (!inCell()) tell(pe, v.state);
    if (f()) {
      v.dispatch(pageSpec(pe, v.state.doc.toString()));
      o.edited();
      if (pe.cell_caret()) place(tables);
    }
    return true;
  };

  const tables: TableContext = {
    pe,
    view: () => view,
    push: () => view?.dispatch({ ...pageSpec(pe, view.state.doc.toString()), scrollIntoView: false }),
    edited: () => o.edited(),
    moved: () => o.moved(),
  };

  /// The page shown again as it is (after a reload from elsewhere).
  const show = (v: EditorView) => v.dispatch({ ...pageSpec(pe, v.state.doc.toString()), scrollIntoView: false });

  const toggle = (line: number) => view && run(() => pe.toggle(line))(view);

  const plugin = ViewPlugin.fromClass(
    class {
      deco: DecorationSet;
      atomic: DecorationSet;
      constructor(v: EditorView) {
        view = v;
        ({ deco: this.deco, atomic: this.atomic } = build(v, o.pictures, toggle, tables));
      }
      update(u: ViewUpdate) {
        if (u.docChanged || u.viewportChanged || u.transactions.some((t) => t.effects.some((e) => e.is(setLooks)))) {
          ({ deco: this.deco, atomic: this.atomic } = build(u.view, o.pictures, toggle, tables));
        }
        if (u.selectionSet || u.docChanged) o.moved();
      }
    },
    {
      decorations: (v) => v.deco,
      provide: (p) => EditorView.atomicRanges.of((v) => v.plugin(p)?.atomic ?? Decoration.none),
    },
  );

  /// How the page draws itself now, shown (after a change CodeMirror
  /// made itself, which the page agreed with).
  const relook = () => view?.dispatch({ effects: setLooks.of(JSON.parse(pe.looks())), annotations: own.of(true) });

  const extensions: Extension[] = [
    looks.init(() => JSON.parse(pe.looks())),
    plugin,
    pictureTools(pe, run),
    EditorState.transactionFilter.of((tr) => {
      if (tr.annotation(own)) return tr;
      if (!tr.docChanged) {
        if (tr.selection) {
          tell(pe, { doc: tr.newDoc, selection: tr.selection });
          // Onto a table: into its cell, the last row's when come up to
          // from below.
          const at = pe.cell_caret();
          if (at) {
            const was = tr.startState.doc.lineAt(tr.startState.selection.main.head).number - 1;
            const rows = tr.state.field(looks)[at[0]]?.cells.length ?? 0;
            if (was > at[0] && rows) pe.cell_select(at[0], rows - 1, 0, 0, 0);
            queueMicrotask(() => place(tables));
          }
        }
        return tr;
      }
      const instead = mirror(pe, tr);
      o.edited();
      if (instead) return instead;
      queueMicrotask(relook);
      return tr;
    }),
    keymap.of([
      { key: 'Enter', run: run(() => pe.enter(false)) },
      { key: 'Shift-Enter', run: run(() => pe.enter(true)) },
      { key: 'Backspace', run: run(() => pe.backspace(false)) },
      { key: 'Mod-Backspace', run: run(() => pe.backspace(true)) },
      { key: 'Alt-Backspace', run: run(() => pe.backspace(true)) },
      { key: 'Delete', run: run(() => pe.delete(false)) },
      { key: 'Mod-Delete', run: run(() => pe.delete(true)) },
      { key: 'Alt-Delete', run: run(() => pe.delete(true)) },
      { key: 'Tab', run: run(() => pe.tab(false)) },
      { key: 'Shift-Tab', run: run(() => pe.tab(true)) },
      { key: 'Mod-b', run: run(() => pe.style(0)) },
      { key: 'Mod-i', run: run(() => pe.style(1)) },
      { key: 'Shift-Mod-x', run: run(() => pe.style(2)) },
      { key: 'Mod-e', run: run(() => pe.style(3)) },
      { key: 'Shift-Mod-h', run: run(() => pe.tint(false, HIGHLIGHT)) },
      { key: 'Mod-z', run: run(() => pe.undo()) },
      { key: 'Shift-Mod-z', run: run(() => pe.redo()) },
      { key: 'Mod-y', run: run(() => pe.redo()) },
      // The desktop's: a picture to the left or right of the text, or
      // back on its own line from there.
      { key: 'Mod-Alt-l', run: (v) => !!pe.picture_here() && run(() => pe.set_picture_float(1, true))(v) },
      { key: 'Mod-Alt-r', run: (v) => !!pe.picture_here() && run(() => pe.set_picture_float(2, true))(v) },
    ]),
    EditorView.domEventHandlers({
      copy(e, v) {
        tell(pe, v.state);
        const md = pe.copy();
        if (md == null || !e.clipboardData) return false;
        e.clipboardData.setData('text/plain', md);
        const html = pe.copy_html();
        if (html) e.clipboardData.setData('text/html', html);
        e.preventDefault();
        return true;
      },
      cut(e, v) {
        tell(pe, v.state);
        const md = pe.copy();
        if (md == null || !e.clipboardData) return false;
        e.clipboardData.setData('text/plain', md);
        e.preventDefault();
        run(() => pe.input(''))(v);
        return true;
      },
      paste(e, v) {
        const s = e.clipboardData?.getData('text/plain');
        if (!s) return false;
        e.preventDefault();
        run(() => pe.paste(s))(v);
        return true;
      },
    }),
  ];
  return { extensions, run, show };
}
