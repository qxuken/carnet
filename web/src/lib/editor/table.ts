// A table in the editor: its cells written in, each a little editor of
// its own over the page's cell (the desktop's page edits tables cell by
// cell, so Enter goes down a column and adds a row at the bottom, Tab goes
// on to the next cell, a style carries on as in the text). What's typed in
// a cell goes to the page, which draws the cell again; an input method
// composing in a cell is let be, and its words go in when it's done.
//
// A picture among a cell's words (`![](a.png)`) is one letter of them to
// the page: drawn here as the picture, its own width at most the cell's
// (a width the note writes for it is the cell's to decide), fetched as a
// picture on a line of its own is, in a box the caret can't go into,
// which Backspace and Delete take whole. A press chooses it, as ← and →
// do before they pass it (the page's `left`/`right`): chosen, it wears
// the accent's ring and its cell shows no caret.

import { WidgetType, type EditorView } from '@codemirror/view';
import type { PageEditor } from '../../wasm/carnet_web.js';
import { inkCss, washCss } from '../colors';
import { show, unshown } from '../pictures';

export type Run = [number, number, number, number, number];
/// A picture among a cell's words: the place of its letter in them, its
/// `src` as the note has it, its alt text, the width the note writes for
/// it (not drawn by: the cell decides), and whether an Obsidian embed
/// (`![[…]]`) names it.
export type CellPic = [number, string, string, number | null, boolean];
export type Cell = { t: string; r: Run[]; p?: CellPic[] };

export type TableContext = {
  pe: PageEditor;
  /// A picture's `src` (an Obsidian embed's, with `wiki`) as a URL to show.
  pictures: (src: string, wiki: boolean) => Promise<string>;
  view: () => EditorView | null;
  /// The page's change shown (CodeMirror's text and how it's drawn).
  push: () => void;
  edited: () => void;
  moved: () => void;
};

/// A cell's picture: the box it is drawn in, or the one it was drawn in
/// before, kept so a cell drawn again doesn't fetch it again.
function pictureBox(pic: CellPic, ctx: TableContext, kept: Map<string, HTMLElement[]>): HTMLElement {
  const [, src, alt, , wiki] = pic;
  const key = JSON.stringify(pic.slice(1));
  const old = kept.get(key)?.shift();
  if (old) return old;
  const box = document.createElement('span');
  box.className = 'cm-cellpic';
  box.contentEditable = 'false';
  box.dataset.key = key;
  const img = document.createElement('img');
  img.className = 'cm-cellpic-img';
  img.alt = alt;
  img.draggable = false;
  box.append(img);
  unshown(img, src);
  show(img, src, () => ctx.pictures(src, wiki));
  return box;
}

/// Whether `n` is a cell's picture's box.
const isPicture = (n: Node): n is HTMLElement => n instanceof HTMLElement && n.classList.contains('cm-cellpic');

/// `text` drawn into `el` in its runs' styles, its pictures as pictures.
function paint(el: HTMLElement, text: string, runs: Run[], pics: CellPic[], ctx: TableContext) {
  const kept = new Map<string, HTMLElement[]>();
  for (const box of el.querySelectorAll<HTMLElement>('.cm-cellpic')) {
    const k = box.dataset.key ?? '';
    kept.set(k, [...(kept.get(k) ?? []), box]);
  }
  el.textContent = '';
  let at = 0;
  const plain = (to: number) => {
    if (to > at) el.append(text.slice(at, to));
  };
  for (const [a, b, flags, mark, ink] of runs) {
    plain(a);
    if (flags & 32) {
      // Each letter of the run a picture (two side by side are one run).
      for (let k = a; k < b; k++) {
        const pic = pics.find((p) => p[0] === k);
        if (pic) el.append(pictureBox(pic, ctx, kept));
      }
      at = b;
      continue;
    }
    const s = document.createElement('span');
    const c: string[] = [];
    if (flags & 1) c.push('cm-b');
    if (flags & 2) c.push('cm-i');
    if (flags & 4) c.push('cm-s');
    if (flags & 8) c.push('cm-c');
    if (flags & 16) c.push('cm-l');
    if (mark) c.push('cm-marked');
    s.className = c.join(' ');
    if (ink) s.style.color = inkCss(ink);
    if (mark) s.style.backgroundColor = washCss(mark);
    s.textContent = text.slice(a, b);
    el.append(s);
    at = b;
  }
  plain(text.length);
  // An empty cell keeps a line's height for the caret.
  if (!text) el.append(document.createElement('br'));
}

/// How long `n` is in a cell's words: its text, a picture one letter.
function wordsLength(n: Node): number {
  if (isPicture(n)) return 1;
  if (n.nodeType === Node.TEXT_NODE) return n.textContent?.length ?? 0;
  let len = 0;
  for (const c of n.childNodes) len += wordsLength(c);
  return len;
}

/// UTF-16 offset of DOM point (`node`, `offset`) in `cell`'s words: a
/// point in a picture's box is after it.
export function offsetIn(cell: HTMLElement, node: Node, offset: number): number {
  const r = document.createRange();
  r.setStart(cell, 0);
  try {
    r.setEnd(node, offset);
  } catch {
    return 0;
  }
  return wordsLength(r.cloneContents());
}

/// The DOM point for UTF-16 offset `n` in `cell`'s words: never inside a
/// picture's box, but before or after it.
function pointIn(cell: HTMLElement, n: number): [Node, number] {
  let left = n;
  let last: [Node, number] = [cell, 0];
  const walk = (el: Node): [Node, number] | null => {
    for (const [i, c] of [...el.childNodes].entries()) {
      if (isPicture(c)) {
        if (left === 0) return [el, i];
        left -= 1;
        last = [el, i + 1];
        if (left === 0) return last;
      } else if (c.nodeType === Node.TEXT_NODE) {
        const len = c.textContent?.length ?? 0;
        if (left <= len) return [c, left];
        left -= len;
        last = [c, len];
      } else {
        const found = walk(c);
        if (found) return found;
      }
    }
    return null;
  };
  return walk(cell) ?? last;
}

type Wrap = HTMLElement & { widget?: TableWidget };

export class TableWidget extends WidgetType {
  constructor(
    readonly cells: Cell[][],
    /// Its first row is its header.
    readonly head: boolean,
    readonly line: number,
    readonly ctx: TableContext,
  ) {
    super();
  }

  eq(o: TableWidget) {
    return o.line === this.line && o.head === this.head && JSON.stringify(o.cells) === JSON.stringify(this.cells);
  }

  toDOM() {
    const wrap = document.createElement('span') as Wrap;
    wrap.className = 'cm-tablewrap';
    wrap.appendChild(document.createElement('table'));
    this.fill(wrap);
    bind(wrap);
    return wrap;
  }

  /// Drawn again in place, so a cell being written in keeps the caret.
  updateDOM(dom: HTMLElement) {
    this.fill(dom as Wrap);
    return true;
  }

  fill(wrap: Wrap) {
    wrap.widget = this;
    const table = wrap.querySelector('table')!;
    while (table.rows.length > this.cells.length) table.deleteRow(-1);
    this.cells.forEach((row, r) => {
      const tr = table.rows[r] ?? table.insertRow();
      while (tr.cells.length > row.length) tr.deleteCell(-1);
      row.forEach((cell, c) => {
        let el = tr.cells[c] as HTMLTableCellElement | undefined;
        const tag = r === 0 && this.head ? 'TH' : 'TD';
        if (!el || el.tagName !== tag) {
          const fresh = document.createElement(tag.toLowerCase()) as HTMLTableCellElement;
          fresh.contentEditable = 'true';
          fresh.spellcheck = true;
          if (el) tr.replaceChild(fresh, el);
          else tr.appendChild(fresh);
          el = fresh;
        }
        el.dataset.r = String(r);
        el.dataset.c = String(c);
        const drawn = JSON.stringify(cell);
        // A cell an input method is composing in is left as it is.
        if (el.dataset.drawn !== drawn && el.dataset.composing !== '1') {
          paint(el, cell.t, cell.r, cell.p ?? [], this.ctx);
          el.dataset.drawn = drawn;
          el.dataset.text = cell.t;
        }
      });
    });
  }

  ignoreEvent() {
    return true;
  }

  destroy(dom: HTMLElement) {
    (dom as Wrap).widget = undefined;
  }
}

/// A table's cell as the page names it: CodeMirror's line, its row and
/// its column; and the cell.
export type CellAt = { cell: HTMLTableCellElement; line: number; r: number; c: number };

/// The table's cell `el` is in, if it is in one.
export function cellAt(el: Element | null): CellAt | null {
  const cell = cellOf(el);
  const w = (cell?.closest('.cm-tablewrap') as Wrap | null)?.widget;
  if (!cell || !w) return null;
  return { cell, line: w.line, r: Number(cell.dataset.r), c: Number(cell.dataset.c) };
}

/// Where a cell's picture is in its words: the UTF-16 offset of its letter.
export function pictureAt(cell: HTMLElement, box: HTMLElement): number {
  const parent = box.parentNode;
  return parent ? offsetIn(cell, parent, [...parent.childNodes].indexOf(box)) : 0;
}

/// The UTF-16 offset in `cell`'s words nearest the point (`x`, `y`);
/// `undefined` when the browser can't say.
export function offsetAt(cell: HTMLElement, x: number, y: number): number | undefined {
  const doc = document as Document & { caretRangeFromPoint?: (x: number, y: number) => Range | null };
  const p = doc.caretPositionFromPoint?.(x, y);
  const [node, offset] = p ? [p.offsetNode, p.offset] : (() => {
    const r = doc.caretRangeFromPoint?.(x, y);
    return r ? [r.startContainer, r.startOffset] : [null, 0];
  })();
  if (!node || !cell.contains(node)) return undefined;
  // In a picture's box: before it or after it, by its half.
  const box = (node instanceof Element ? node : node.parentElement)?.closest<HTMLElement>('.cm-cellpic');
  if (box && cell.contains(box)) {
    const at = pictureAt(cell, box);
    const r = box.getBoundingClientRect();
    return x < r.left + r.width / 2 ? at : at + 1;
  }
  return offsetIn(cell, node, offset);
}

/// The cell `el` is, if it is one.
const cellOf = (el: EventTarget | Node | null): HTMLTableCellElement | null =>
  el instanceof Node ? ((el instanceof Element ? el : el.parentElement)?.closest('td, th') as HTMLTableCellElement | null) : null;

/// The page's caret put where the browser's is, in `cell`.
function tell(w: TableWidget, cell: HTMLTableCellElement, range?: StaticRange | Range) {
  const sel = getSelection();
  let a = 0;
  let h = 0;
  if (range) {
    a = offsetIn(cell, range.startContainer, range.startOffset);
    h = offsetIn(cell, range.endContainer, range.endOffset);
  } else if (sel && sel.anchorNode && cell.contains(sel.anchorNode)) {
    a = offsetIn(cell, sel.anchorNode, sel.anchorOffset);
    h = sel.focusNode && cell.contains(sel.focusNode) ? offsetIn(cell, sel.focusNode, sel.focusOffset) : a;
  }
  w.ctx.pe.cell_select(w.line, Number(cell.dataset.r), Number(cell.dataset.c), a, h);
  mark(w.ctx);
}

/// The picture the page has chosen in a cell (its selection that picture
/// and nothing more) ringed, and its cell's caret hidden; any other ring
/// taken off.
export function mark(ctx: TableContext) {
  const view = ctx.view();
  if (!view) return;
  let box: HTMLElement | undefined;
  let cell: HTMLTableCellElement | undefined;
  const at = ctx.pe.cell_caret();
  if (at) {
    const [line, r, c, a, h] = Array.from(at);
    if (Math.abs(a - h) === 1) {
      const wrap = Array.from(view.contentDOM.querySelectorAll<Wrap>('.cm-tablewrap')).find((w) => w.widget?.line === line);
      cell = wrap?.querySelector('table')?.rows[r]?.cells[c] as HTMLTableCellElement | undefined;
      box = cell ? [...cell.querySelectorAll<HTMLElement>('.cm-cellpic')].find((b) => pictureAt(cell!, b) === Math.min(a, h)) : undefined;
    }
  }
  for (const b of view.contentDOM.querySelectorAll<HTMLElement>('.cm-cellpic-chosen')) if (b !== box) b.classList.remove('cm-cellpic-chosen');
  for (const c of view.contentDOM.querySelectorAll<HTMLElement>('.cm-cell-picked')) if (!box || c !== cell) c.classList.remove('cm-cell-picked');
  if (box && cell) {
    box.classList.add('cm-cellpic-chosen');
    cell.classList.add('cm-cell-picked');
  }
}

/// After the page changed: shown, and the caret back where the page has
/// it (a cell, or CodeMirror's text).
export function settle(ctx: TableContext) {
  ctx.push();
  ctx.edited();
  place(ctx);
  ctx.moved();
}

/// The caret where the page has it.
export function place(ctx: TableContext) {
  const view = ctx.view();
  if (!view) return;
  mark(ctx);
  const at = ctx.pe.cell_caret();
  if (!at) {
    view.focus();
    return;
  }
  const [line, r, c, a, h] = Array.from(at);
  const wrap = Array.from(view.contentDOM.querySelectorAll<Wrap>('.cm-tablewrap')).find((w) => w.widget?.line === line);
  const cell = wrap?.querySelector('table')?.rows[r]?.cells[c] as HTMLTableCellElement | undefined;
  if (!cell) return;
  if (document.activeElement !== cell) cell.focus({ preventScroll: false });
  const sel = getSelection();
  if (!sel) return;
  const [an, ao] = pointIn(cell, a);
  const [hn, ho] = pointIn(cell, h);
  sel.setBaseAndExtent(an, ao, hn, ho);
}

function bind(wrap: Wrap) {
  const widget = () => wrap.widget;

  wrap.addEventListener('focusin', (e) => {
    const w = widget();
    const cell = cellOf(e.target);
    if (w && cell) {
      // After the browser has put its caret.
      requestAnimationFrame(() => {
        tell(w, cell);
        w.ctx.moved();
      });
    }
  });
  const follow = () => {
    const w = widget();
    const cell = cellOf(getSelection()?.anchorNode ?? null);
    if (w && cell && wrap.contains(cell) && cell.dataset.composing !== '1') {
      tell(w, cell);
      w.ctx.moved();
    }
  };
  wrap.addEventListener('mouseup', follow);
  wrap.addEventListener('keyup', follow);

  // A press on a cell's picture selects it, whole.
  wrap.addEventListener('mousedown', (e) => {
    const box = e.button === 0 ? (e.target as Element).closest?.('.cm-cellpic') : null;
    const w = widget();
    const cell = cellOf(box ?? null);
    if (!box || !w || !cell || !box.parentNode) return;
    e.preventDefault();
    if (document.activeElement !== cell) cell.focus({ preventScroll: true });
    const i = [...box.parentNode.childNodes].indexOf(box);
    getSelection()?.setBaseAndExtent(box.parentNode, i, box.parentNode, i + 1);
    tell(w, cell);
    w.ctx.moved();
  });

  wrap.addEventListener('compositionstart', (e) => {
    const cell = cellOf(e.target);
    if (cell) cell.dataset.composing = '1';
  });
  wrap.addEventListener('compositionend', (e) => {
    const w = widget();
    const cell = cellOf(e.target);
    if (!w || !cell) return;
    cell.dataset.composing = '';
    // The words the input method left, made on the page: one change
    // between where the cell's words first and last differ.
    const was = cell.dataset.text ?? '';
    const now = cell.textContent ?? '';
    let a = 0;
    while (a < was.length && a < now.length && was[a] === now[a]) a++;
    let b = 0;
    while (b < was.length - a && b < now.length - a && was[was.length - 1 - b] === now[now.length - 1 - b]) b++;
    w.ctx.pe.cell_select(w.line, Number(cell.dataset.r), Number(cell.dataset.c), a, was.length - b);
    w.ctx.pe.input(now.slice(a, now.length - b));
    cell.dataset.drawn = '';
    settle(w.ctx);
  });

  wrap.addEventListener('beforeinput', (e) => {
    const w = widget();
    const cell = cellOf(e.target);
    if (!w || !cell || e.isComposing || e.inputType === 'insertCompositionText') return;
    e.preventDefault();
    const pe = w.ctx.pe;
    const range = e.getTargetRanges()[0];
    tell(w, cell, range);
    const collapsed = !range || (range.startContainer === range.endContainer && range.startOffset === range.endOffset);
    const text = e.data ?? e.dataTransfer?.getData('text/plain') ?? '';
    let done = false;
    switch (e.inputType) {
      case 'insertText':
      case 'insertReplacementText':
        done = pe.input(text);
        break;
      case 'deleteContentBackward':
        // A range given (the word a keyboard took back): that range;
        // else the page's Backspace.
        tell(w, cell);
        done = collapsed || !range ? pe.backspace(false) : (tell(w, cell, range), pe.input(''));
        break;
      case 'deleteWordBackward':
        tell(w, cell);
        done = pe.backspace(true);
        break;
      case 'deleteContentForward':
        tell(w, cell);
        done = pe.delete(false);
        break;
      case 'deleteWordForward':
        tell(w, cell);
        done = pe.delete(true);
        break;
      case 'insertParagraph':
      case 'insertLineBreak':
        tell(w, cell);
        done = pe.enter(false);
        break;
      case 'insertFromPaste':
      case 'insertFromDrop':
        done = pe.paste(text);
        break;
      case 'deleteByCut':
        done = pe.input('');
        break;
    }
    if (done) settle(w.ctx);
  });

  wrap.addEventListener('keydown', (e) => {
    const w = widget();
    const cell = cellOf(e.target);
    if (!w || !cell || e.isComposing) return;
    const pe = w.ctx.pe;
    const mod = e.metaKey || e.ctrlKey;
    const k = e.key.toLowerCase();
    // ← or → beside a picture, or with one chosen: the page's move, which
    // chooses it before passing it. Any other, the browser's.
    if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && !mod && !e.altKey && !e.shiftKey) {
      tell(w, cell);
      if (pe.cell_arrow(e.key === 'ArrowRight')) {
        e.preventDefault();
        place(w.ctx);
        w.ctx.moved();
      }
      return;
    }
    let f: (() => boolean) | null = null;
    if (e.key === 'Tab') f = () => pe.tab(e.shiftKey);
    else if (mod && !e.shiftKey && k === 'b') f = () => pe.style(0);
    else if (mod && !e.shiftKey && k === 'i') f = () => pe.style(1);
    else if (mod && e.shiftKey && k === 'x') f = () => pe.style(2);
    else if (mod && !e.shiftKey && k === 'e') f = () => pe.style(3);
    else if (mod && e.shiftKey && k === 'h') f = () => pe.tint(false, 1);
    else if (mod && !e.shiftKey && k === 'z') f = () => pe.undo();
    else if (mod && e.shiftKey && k === 'z') f = () => pe.redo();
    else if (e.key === 'Escape') {
      // Out of the table, to the line after it.
      f = () => (pe.select(w.line + 1, 0, w.line + 1, 0), true);
    }
    if (!f) return;
    e.preventDefault();
    tell(w, cell);
    if (f()) settle(w.ctx);
  });

  wrap.addEventListener('copy', (e) => {
    const w = widget();
    const cell = cellOf(e.target);
    if (!w || !cell || !e.clipboardData) return;
    tell(w, cell);
    const md = w.ctx.pe.copy();
    if (md == null) return;
    e.clipboardData.setData('text/plain', md);
    e.preventDefault();
  });
}
