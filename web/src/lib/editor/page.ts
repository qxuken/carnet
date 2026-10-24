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
import { dropFor, linesOf } from '../initial';
import { Decoration, EditorView, ViewPlugin, WidgetType, keymap, type DecorationSet, type ViewUpdate } from '@codemirror/view';
import type { PageEditor } from '../../wasm/carnet_web.js';
import { HIGHLIGHT, inkCss, washCss } from '../colors';
import { show, unshown } from '../pictures';
import { TableWidget, mark, place, type Cell, type TableContext } from './table';
import { pictureCarry, pictureRoom, pictureTools } from './picture';
import * as arrows from './arrows';
import { markRange } from './ranges';
import { STYLES } from './styles';
import type { Lines } from './arrows';

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
  /// A table's first row is its header.
  hd: boolean;
  /// A paragraph's first line whose first letter drops: 1, or 2 when the
  /// letter has a colour of its own.
  ini: number;
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

/// The editor's lines, as the arrows see them (arrows.ts).
function linesIn(state: EditorState): Lines {
  const all = state.field(looks);
  const doc = state.doc;
  return {
    count: doc.lines,
    line: (n) => doc.line(n),
    kind: (n) => all[n - 1]?.k,
    lineAt: (pos) => doc.lineAt(pos).number,
  };
}

/// A change CodeMirror is making, made on the page. `null` when the
/// page's words agree (CodeMirror's change stands, untouched, so an input
/// method composing is never disturbed); else the page's change instead.
export function mirror(pe: PageEditor, tr: Transaction): TransactionSpec | null {
  const start = tr.startState;
  const composing = tr.isUserEvent('input.type.compose');
  const parts: { from: number; to: number; text: string }[] = [];
  tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => parts.push({ from: fromA, to: toA, text: inserted.toString() }));
  const sel = start.selection.main;
  const one = parts.length === 1 ? parts[0] : null;
  tell(pe, start);
  const typed = parts.map((p) => p.text).join('');
  if (!composing && typed && tr.isUserEvent('input.type') && arrows.chosenAtom(linesIn(start), sel) !== null) {
    // Typed with a picture or a divider chosen: the words take its place,
    // as on the desktop. A picture is a block of its own, which the
    // browser's caret can't be in: it stands at the start of the line
    // after it (or the end of the one before), and the browser types
    // there; the page's selection is the picture, chosen.
    if (typed.includes('\n')) pe.paste(typed);
    else pe.input(typed);
    return { ...pageSpec(pe, start.doc.toString()), userEvent: 'input' };
  }
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

/// With a picture or a divider chosen, room made in its place for an
/// input method about to compose (`PageEditor.make_room`): an empty
/// paragraph, which the browser's caret is then put in. `null` with none
/// chosen.
export function roomFor(pe: PageEditor, state: EditorState): TransactionSpec | null {
  if (arrows.chosenAtom(linesIn(state), state.selection.main) === null) return null;
  tell(pe, state);
  return pe.make_room() ? pageSpec(pe, state.doc.toString()) : null;
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

/// A picture as the note has it: what its widget draws.
type Pic = { src: string; wiki: boolean; alt: string; width: number | null };

const same = (a: Pic, b: Pic) => a.src === b.src && a.wiki === b.wiki && a.alt === b.alt && a.width === b.width;

/// Each picture's box, and the picture in it (or to be, once it comes:
/// until then its place is a box of the same size, pictures.ts).
const shownIn = new WeakMap<HTMLElement, HTMLImageElement>();

/// The pictures of a row, laid out as the reading view lays them out
/// (styles.css `.pictures`): one on a line of its own, or those at an
/// edge one after another, side by side there. A block of its own: no
/// line of text above or below it to put a caret in. A row at an edge is
/// a float in a wrapper with no height, so CodeMirror measures it as
/// nothing while the lines after it run beside it (theme.ts); the row's
/// later pictures each keep a line of their own, drawn as nothing
/// (`Beside`).
class PictureRow extends WidgetType {
  constructor(readonly pics: Pic[], readonly side: number, readonly pictures: Pictures) {
    super();
  }
  eq(o: PictureRow) {
    return o.side === this.side && o.pics.length === this.pics.length && o.pics.every((p, i) => same(p, this.pics[i]));
  }
  toDOM() {
    const wrap = document.createElement('div');
    const row = document.createElement('div');
    row.className = 'cm-pics-row';
    for (const [k, p] of this.pics.entries()) {
      const img = document.createElement('img');
      img.className = 'cm-picture';
      img.alt = p.alt;
      if (p.width) img.style.width = `${p.width}px`;
      // In a box of its own, for the box it takes until it comes (or a
      // card saying why it isn't shown: not found, or one this browser
      // can't draw) to take its place in: the reader's, the same size.
      const box = document.createElement('span');
      box.className = 'cm-picture-box';
      box.dataset.k = String(k);
      box.classList.toggle('cm-sized', !!p.width);
      box.append(img);
      unshown(img, p.src);
      show(img, p.src, () => this.pictures(p.src, p.wiki));
      shownIn.set(box, img);
      row.append(box);
    }
    wrap.append(row);
    this.dress(wrap);
    return wrap;
  }
  /// The row's edge, and how many share it.
  dress(wrap: HTMLElement) {
    wrap.classList.add('cm-pics');
    wrap.classList.toggle('cm-float', !!this.side);
    wrap.classList.toggle('cm-left', this.side === 1);
    wrap.classList.toggle('cm-right', this.side === 2);
    wrap.style.setProperty('--n', String(this.pics.length));
  }
  /// The same pictures with another width, alt text or edge (resized, or
  /// moved): the ones shown kept, not fetched and drawn again.
  updateDOM(dom: HTMLElement, _view: EditorView, from: PictureRow) {
    if (from.pics.length !== this.pics.length || from.pics.some((p, i) => p.src !== this.pics[i].src || p.wiki !== this.pics[i].wiki)) return false;
    const boxes = dom.querySelectorAll<HTMLElement>('.cm-picture-box');
    if (boxes.length !== this.pics.length) return false;
    for (const [k, p] of this.pics.entries()) {
      const box = boxes[k];
      const width = p.width ? `${p.width}px` : '';
      box.classList.toggle('cm-sized', !!p.width);
      // The picture, drawn or still to come; and its place while it comes.
      const img = shownIn.get(box);
      if (img) {
        img.style.width = width;
        img.alt = p.alt;
      }
      const place = box.querySelector<HTMLElement>('.picture-card');
      if (place) place.style.width = width;
    }
    this.dress(dom);
    return true;
  }
  get estimatedHeight() {
    return this.side ? 0 : -1;
  }
}

/// A picture drawn in the row of the one before it: nothing of its own.
class Beside extends WidgetType {
  eq() {
    return true;
  }
  toDOM() {
    const d = document.createElement('div');
    d.className = 'cm-pics-beside';
    return d;
  }
  get estimatedHeight() {
    return 0;
  }
}

/// At the note's end, below any picture at an edge: the page as tall as
/// what is drawn, the last picture too.
class Clear extends WidgetType {
  eq() {
    return true;
  }
  toDOM() {
    const d = document.createElement('div');
    d.className = 'cm-clear';
    return d;
  }
  get estimatedHeight() {
    return 0;
  }
}

/// The pictures as blocks, each picture's line replaced whole. `null`
/// while the page's looks are not yet the text's.
function pictureBlocks(state: EditorState, pictures: Pictures): DecorationSet | null {
  const all = state.field(looks);
  const doc = state.doc;
  if (all.length !== doc.lines) return null;
  const out: Range<Decoration>[] = [];
  const pic = (l: Look): Pic => ({ src: l.src, wiki: l.wk, alt: l.alt, width: l.w });
  const atEdge = (i: number) => i >= 0 && all[i]?.k === 'image' && !!all[i].fl;
  for (let i = 0; i < all.length; i++) {
    const look = all[i];
    if (look.k !== 'image') continue;
    let widget: WidgetType;
    if (!look.fl) widget = new PictureRow([pic(look)], 0, pictures);
    else if (atEdge(i - 1)) widget = new Beside();
    else {
      // A row: the pictures at an edge from here on, at the first's edge.
      let j = i;
      while (atEdge(j)) j++;
      widget = new PictureRow(all.slice(i, j).map(pic), look.fl, pictures);
    }
    const line = doc.line(i + 1);
    out.push(Decoration.replace({ widget, block: true }).range(line.from, line.to));
  }
  out.push(Decoration.widget({ widget: new Clear(), block: true, side: 1 }).range(doc.length));
  return Decoration.set(out, true);
}

/// A divider the selection takes in whole (an arrow onto it, Backspace
/// on the empty line under it, a click on it): drawn chosen, as the
/// desktop draws it — a band of the selection's wash as wide as the text,
/// the rule in the accent (theme.ts) — so the next Backspace is seen to
/// have a target. Native selection draws nothing on a widget.
const ruleOn = Decoration.line({ class: 'cm-rule-on' });

function chosenRules(state: EditorState): DecorationSet {
  const all = state.field(looks);
  const doc = state.doc;
  const sel = state.selection.main;
  if (sel.empty || all.length !== doc.lines) return Decoration.none;
  const out: Range<Decoration>[] = [];
  for (let n = doc.lineAt(sel.from).number; n <= doc.lineAt(sel.to).number; n++) {
    if (all[n - 1]?.k !== 'divider') continue;
    const line = doc.line(n);
    if (sel.from <= line.from && sel.to >= line.to) out.push(ruleOn.range(line.from));
  }
  return Decoration.set(out);
}

/// Dropped first letters as deep as their paragraphs (lib/initial.ts),
/// given as line decorations, never classes set on CodeMirror's own
/// lines, which it would redraw away. Measured after each change is drawn,
/// the letter as it then drops (a line or so of difference at most, and
/// none for a paragraph of one line, which is what matters).
const initialsMeasured = StateEffect.define<null>();
const initials = ViewPlugin.fromClass(
  class {
    drops = new Map<number, string>();
    deco: DecorationSet = Decoration.none;
    constructor(readonly view: EditorView) {
      this.measure();
    }
    update(u: ViewUpdate) {
      if (u.docChanged) {
        const moved = new Map<number, string>();
        for (const [at, cls] of this.drops) moved.set(u.changes.mapPos(at), cls);
        this.drops = moved;
        this.deco = this.decorate(u.state);
      }
      if (u.transactions.some((t) => t.effects.some((e) => e.is(initialsMeasured)))) this.deco = this.decorate(u.state);
      else if (u.docChanged || u.viewportChanged || u.geometryChanged || u.transactions.some((t) => t.effects.some((e) => e.is(setLooks)))) this.measure();
    }
    decorate(state: EditorState): DecorationSet {
      const out: Range<Decoration>[] = [];
      for (const [at, cls] of [...this.drops].sort((a, b) => a[0] - b[0])) {
        if (at > state.doc.length) continue;
        out.push(Decoration.line({ class: cls }).range(state.doc.lineAt(at).from));
      }
      return Decoration.set(out, true);
    }
    measure() {
      this.view.requestMeasure({
        key: this,
        read: (view) => {
          const out = new Map<number, string>();
          for (const el of view.contentDOM.querySelectorAll<HTMLElement>('.cm-initial')) {
            const at = view.state.doc.lineAt(view.posAtDOM(el)).from;
            out.set(at, dropFor(linesOf(el)));
          }
          return out;
        },
        write: (out, view) => {
          const same = out.size === this.drops.size && [...out].every(([k, v]) => this.drops.get(k) === v);
          if (same) return;
          this.drops = out;
          queueMicrotask(() => view.dispatch({ effects: initialsMeasured.of(null) }));
        },
      });
    }
  },
  { decorations: (v) => v.deco },
);

const ruleChosen = ViewPlugin.fromClass(
  class {
    deco: DecorationSet;
    constructor(v: EditorView) {
      this.deco = chosenRules(v.state);
    }
    update(u: ViewUpdate) {
      if (u.selectionSet || u.docChanged || u.transactions.some((t) => t.effects.some((e) => e.is(setLooks)))) this.deco = chosenRules(u.state);
    }
  },
  { decorations: (v) => v.deco },
);

function build(view: EditorView, toggle: (line: number) => void, tables: TableContext): { deco: DecorationSet; atomic: DecorationSet } {
  const all = view.state.field(looks);
  const doc = view.state.doc;
  const out: Range<Decoration>[] = [];
  const atomic: Range<Decoration>[] = [];
  if (all.length !== doc.lines) return { deco: Decoration.none, atomic: Decoration.none };
  for (const { from, to } of view.visibleRanges) {
    for (let n = doc.lineAt(from).number; n <= doc.lineAt(to).number; n++) {
      const line = doc.line(n);
      const look = all[n - 1];
      // A picture: a block of its own (`pictureBlocks`).
      if (look.k === 'image') continue;
      const cls = [`cm-${look.k}`];
      if (look.f && n > 1) cls.push('cm-block');
      if (look.d) cls.push('cm-ticked');
      if (look.ini) cls.push(look.ini === 2 ? 'cm-initial cm-own' : 'cm-initial');
      const attrs: Record<string, string> = { class: cls.join(' ') };
      if (look.i) attrs.style = `--indent:${look.i}`;
      out.push(Decoration.line({ attributes: attrs }).range(line.from));
      if (look.k === 'title' && !line.length) out.push(Decoration.widget({ widget: new Placeholder('Untitled'), side: 1 }).range(line.from));
      if (look.k === 'todo') out.push(Decoration.widget({ widget: new Box(look.d, n - 1, toggle), side: -1 }).range(line.from));
      if (look.k === 'bullet') out.push(Decoration.widget({ widget: new Marker(['•', '◦', '▪'][look.i % 3], 'cm-dot'), side: -1 }).range(line.from));
      if (look.k === 'number') out.push(Decoration.widget({ widget: new Marker(`${look.n}.`, 'cm-num'), side: -1 }).range(line.from));
      if (look.k === 'divider' || look.k === 'table') {
        const w = look.k === 'divider' ? new Rule() : new TableWidget(look.cells, look.hd, n - 1, tables);
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
        if (flags & 64) c.push('cm-u');
        let style = '';
        if (ink) style += `color:${inkCss(ink)};`;
        if (mark) style += `background-color:${washCss(mark)};`;
        if (mark) c.push('cm-marked');
        const spec: { class: string; attributes?: Record<string, string> } = { class: c.join(' ') };
        if (style) spec.attributes = { style };
        const at = markRange(line.from, line.length, a, b);
        if (at) out.push(Decoration.mark(spec).range(at[0], at[1]));
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
    pictures: o.pictures,
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
        ({ deco: this.deco, atomic: this.atomic } = build(v, toggle, tables));
      }
      update(u: ViewUpdate) {
        if (u.docChanged || u.viewportChanged || u.transactions.some((t) => t.effects.some((e) => e.is(setLooks)))) {
          ({ deco: this.deco, atomic: this.atomic } = build(u.view, toggle, tables));
        }
        if (u.selectionSet || u.docChanged) o.moved();
        // Out of a cell: a picture chosen in it is no longer.
        if (u.selectionSet) mark(tables);
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

  /// The pictures, blocks of their own (`pictureBlocks`).
  const pictureField = StateField.define<DecorationSet>({
    create: (s) => pictureBlocks(s, o.pictures) ?? Decoration.none,
    update(v, tr) {
      if (!tr.docChanged && !tr.effects.some((e) => e.is(setLooks))) return v;
      return pictureBlocks(tr.state, o.pictures) ?? v.map(tr.changes);
    },
    provide: (f) => [EditorView.decorations.from(f), EditorView.atomicRanges.of((v) => v.state.field(f))],
  });

  /// ↑ and ↓ a row at a time: off a line once the caret is on its first
  /// row (or last), onto the next line's nearest row, at the caret's
  /// column. CodeMirror's own motion goes by height, and a heading's
  /// space above it sends it lines too far; and it passes over a picture
  /// at an edge, measured as nothing (the words beside it). Within a
  /// line's own rows, CodeMirror's.
  const vertical = (down: boolean, extend: boolean) => (v: EditorView) => {
    const sel = v.state.selection.main;
    const doc = v.state.doc;
    const plan = arrows.vertical(linesIn(v.state), { anchor: sel.anchor, head: sel.head }, down, extend);
    if (!plan) return false;
    // On `row` (the one measured), the caret kept there: at the start of
    // a wrapped row, not the end of the row above (arrows.ts `sideOn`).
    const go = (head: number, goal?: number, row?: arrows.Row) => {
      const side = row ? arrows.sideOn(head, row, (p, s) => v.coordsAtPos(p, s)) : 0;
      const range = extend ? EditorSelection.range(sel.anchor, head, goal, undefined, side) : EditorSelection.cursor(head, side, undefined, goal);
      v.dispatch({ selection: EditorSelection.create([range]), scrollIntoView: true, userEvent: 'select' });
      return true;
    };
    const here = doc.lineAt(sel.head);
    const caret = v.coordsAtPos(sel.head, sel.assoc || -1);
    const box = v.contentDOM.getBoundingClientRect();
    // The column kept across a picture, a divider or a short line.
    const goal = sel.goalColumn ?? (caret ? caret.left - box.left : undefined);
    if ('at' in plan) return go(plan.at, goal);
    if ('sel' in plan) {
      const range = EditorSelection.range(plan.sel.anchor, plan.sel.head, goal);
      v.dispatch({ selection: EditorSelection.create([range]), scrollIntoView: true, userEvent: 'select' });
      return true;
    }
    if (plan.leaving) {
      // Still within the line's own rows: CodeMirror's.
      const edge = down ? v.coordsAtPos(here.to, -1) : v.coordsAtPos(Math.min(here.from + 1, here.to), here.length > 1 ? 1 : -1);
      if (!caret || !edge) return false;
      if (!arrows.onRow(caret, edge)) return false;
    }
    const to = doc.line(plan.line);
    // The next line's nearest row: its first going down, its last going
    // up (a dropped first letter's line measured by its second letter).
    const row = down ? v.coordsAtPos(Math.min(to.from + 1, to.to), to.length > 1 ? 1 : -1) : v.coordsAtPos(to.to, -1);
    if (!row) return go(down ? to.from : to.to, goal);
    const at = v.posAtCoords({ x: box.left + (goal ?? 0), y: (row.top + row.bottom) / 2 }, false);
    return go(Math.max(to.from, Math.min(to.to, at)), goal, row);
  };

  /// ← and → over a divider or a picture (arrows.ts `across`): an arrow
  /// onto it chooses it whole, and the next is past it.
  const across = (right: boolean) => (v: EditorView) => {
    const sel = v.state.selection.main;
    const to = arrows.across(linesIn(v.state), { anchor: sel.anchor, head: sel.head }, right);
    if (!to) return false;
    v.dispatch({ selection: to, scrollIntoView: true, userEvent: 'select' });
    return true;
  };

  /// Room made over a chosen picture or divider for an input method's
  /// words (`roomFor`), shown.
  const makeRoom = (v: EditorView) => {
    const spec = v.composing ? null : roomFor(pe, v.state);
    if (!spec) return;
    v.dispatch(spec);
    o.edited();
  };

  const extensions: Extension[] = [
    looks.init(() => JSON.parse(pe.looks())),
    pictureField,
    plugin,
    ruleChosen,
    initials,
    pictureTools(pe, run),
    pictureCarry(pe, run, (s, n) => !!s.field(looks)[n]?.f),
    pictureRoom,
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
          // A bare caret the page took for more: down from the title onto
          // a picture or a divider, chosen whole (`Editor::select`).
          const page = selectionIn(tr.newDoc.toString(), pe.caret()).main;
          if (!at && tr.selection.main.empty && !page.empty) return [tr, { selection: EditorSelection.create([page]), annotations: own.of(true), sequential: true }];
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
      { key: 'ArrowRight', run: across(true) },
      { key: 'ArrowLeft', run: across(false) },
      { key: 'ArrowDown', run: vertical(true, false) },
      { key: 'ArrowUp', run: vertical(false, false) },
      { key: 'Shift-ArrowDown', run: vertical(true, true) },
      { key: 'Shift-ArrowUp', run: vertical(false, true) },
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
      // The desktop's: the caret's blocks up or down past their neighbour
      // (in place of CodeMirror's own line moves, which would move a line
      // of words, not a block), and a to-do ticked.
      { key: 'Alt-ArrowUp', shift: run(() => pe.move_blocks(-1)), run: run(() => pe.move_blocks(-1)) },
      { key: 'Alt-ArrowDown', shift: run(() => pe.move_blocks(1)), run: run(() => pe.move_blocks(1)) },
      { key: 'Mod-Enter', run: run(() => pe.tick()) },
      ...STYLES.map((s) => ({ key: s.key, run: run(() => pe.style(s.which)) })),
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
      // An input method about to compose with a picture or a divider
      // chosen (its key down, or a dead key's, which come before the
      // composition): room made in its place first, an empty paragraph
      // the browser's caret is put in, so the words are composed there.
      // Not once the composition has begun: a change under it then
      // leaves the browser composing in the wrong place.
      keydown(e, v) {
        if (e.keyCode === 229 || e.key === 'Process' || e.key === 'Dead') makeRoom(v);
        return false;
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
