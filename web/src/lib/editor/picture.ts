// A picture selected in the editor, as on the desktop: a ring round it, a
// handle on each side and one at its outer corner, dragged with a mouse
// or a finger to make it wider or narrower. Its shape holds, the width
// stays between the desktop's bounds (`carnet_core::page::picture_width`),
// and it is written in the picture's alt text (`![alt|left|400](src)`)
// when let go; let go back at its own width, no width is written. A press
// on a picture selects it. Where it sits, and sizes from a list, are the
// toolbar's (Editor.svelte).

import { EditorSelection } from '@codemirror/state';
import { EditorView, ViewPlugin, type ViewUpdate } from '@codemirror/view';
import { dragged_width, picture_width, resized_width, type PageEditor } from '../../wasm/carnet_web.js';

/// The picture the caret is on: its line (from 0), where it sits (0 its
/// own line, 1 left of the text, 2 right) and the width it is given.
export type Picked = { line: number; float: number; width: number | null };

export function picked(pe: PageEditor): Picked | null {
  const p = pe.picture_here();
  return p ? { line: p[0], float: p[1], width: p[2] || null } : null;
}

/// What is drawn for the picture on line `line` (from 0): the picture, or
/// the card standing in for one not shown. `null` while it is out of view.
export function drawnPicture(view: EditorView, line: number): HTMLElement | null {
  // Its row's (page.ts `PictureRow`): the row starts on its first
  // picture's line, and has a box for each line from there.
  for (const row of view.contentDOM.querySelectorAll<HTMLElement>('.cm-pics')) {
    const first = rowLine(view, row);
    const boxes = row.querySelectorAll<HTMLElement>('.cm-picture-box');
    if (first < 0 || line < first || line >= first + boxes.length) continue;
    return boxes[line - first].querySelector<HTMLElement>('img.cm-picture, .picture-card');
  }
  return null;
}

/// The line (from 0) of a row's first picture; -1 if it's not shown.
function rowLine(view: EditorView, row: HTMLElement) {
  try {
    return view.state.doc.lineAt(view.posAtDOM(row)).number - 1;
  } catch {
    return -1;
  }
}

/// The line (from 0) of the picture in `box`.
function boxLine(view: EditorView, box: HTMLElement) {
  const row = box.closest<HTMLElement>('.cm-pics');
  const first = row ? rowLine(view, row) : -1;
  return first < 0 ? -1 : first + Number(box.dataset.k ?? 0);
}

/// The words beside a row of pictures at an edge are given 180px at
/// least, as the reader gives them (styles.css `.pictures ~ p`'s
/// `min-width`), and the desktop: with less, the row has none beside
/// it, and the words go below (theme.ts `.cm-alone`). Seen again when
/// a picture comes, or the page's width changes.
const BESIDE_MIN = 180;

export const pictureRoom = ViewPlugin.fromClass(
  class {
    constructor(readonly view: EditorView) {
      view.contentDOM.addEventListener('load', this.check, true);
      this.check();
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.geometryChanged || u.viewportChanged) this.check();
    }
    destroy() {
      this.view.contentDOM.removeEventListener('load', this.check, true);
    }
    check = () => {
      this.view.requestMeasure({
        key: this,
        read: (v) => {
          const text = textWidth(v);
          return [...v.contentDOM.querySelectorAll<HTMLElement>('.cm-pics.cm-float')].map((row) => {
            const boxes = [...row.querySelectorAll<HTMLElement>('.cm-picture-box')];
            const wide = boxes.reduce((a, b) => a + b.getBoundingClientRect().width, 0) + 16 * (boxes.length - 1);
            return [row, text - wide - 24 < BESIDE_MIN] as const;
          });
        },
        write: (rows, v) => {
          let changed = false;
          for (const [row, alone] of rows) {
            if (row.classList.contains('cm-alone') === alone) continue;
            row.classList.toggle('cm-alone', alone);
            changed = true;
          }
          if (changed) v.requestMeasure();
        },
      });
    };
  },
);

/// The width the text has: what a picture may take at most.
export const textWidth = (view: EditorView) => view.contentDOM.clientWidth;

type Run = (f: () => boolean) => (v: EditorView) => boolean;

type Drag = { left: boolean; x0: number; w0: number; w: number; max: number; text: number; img: HTMLImageElement; grip: HTMLElement };

export function pictureTools(pe: PageEditor, run: Run) {
  return ViewPlugin.fromClass(
    class {
      /// Over the picture, its size: the ring, the handles, the size
      /// while dragged.
      box: HTMLElement;
      size: HTMLElement;
      corner: HTMLElement;
      img: HTMLImageElement | null = null;
      drag: Drag | null = null;

      constructor(readonly view: EditorView) {
        this.box = document.createElement('div');
        this.box.className = 'cm-picked';
        this.box.hidden = true;
        this.box.setAttribute('aria-hidden', 'true');
        const ring = document.createElement('span');
        ring.className = 'cm-picked-ring';
        this.size = document.createElement('span');
        this.size.className = 'cm-picked-size';
        const grip = (cls: string, left: () => boolean) => {
          const g = document.createElement('span');
          g.className = `cm-grip ${cls}`;
          g.title = 'Drag to resize';
          g.addEventListener('pointerdown', (e) => this.down(e, g, left()));
          g.addEventListener('pointermove', (e) => this.move(e));
          g.addEventListener('pointerup', () => this.up(true));
          g.addEventListener('pointercancel', () => this.up(false));
          // Let go unseen (the press taken elsewhere): as it was, never left
          // half dragged.
          g.addEventListener('lostpointercapture', () => this.up(false));
          // Pressed without taking the caret or the keyboard away.
          g.addEventListener('mousedown', (e) => e.preventDefault());
          return g;
        };
        // A picture at the right edge grows to the left: its corner is
        // the bottom-left one.
        this.corner = grip('cm-grip-corner', () => this.box.classList.contains('right'));
        this.box.append(ring, grip('cm-grip-left', () => true), grip('cm-grip-right', () => false), this.corner, this.size);
        view.dom.append(this.box);
        view.contentDOM.addEventListener('mousedown', this.press, true);
        view.contentDOM.addEventListener('dragstart', this.noDrag, true);
        this.place();
      }

      update(u: ViewUpdate) {
        if (u.docChanged || u.selectionSet || u.geometryChanged || u.viewportChanged) this.place();
      }

      destroy() {
        this.view.contentDOM.removeEventListener('mousedown', this.press, true);
        this.view.contentDOM.removeEventListener('dragstart', this.noDrag, true);
        this.box.remove();
      }

      /// A press on a picture (a mouse's, or a tap's): it selected whole.
      press = (e: MouseEvent) => {
        if (e.button !== 0) return;
        const box = (e.target as HTMLElement).closest?.<HTMLElement>('.cm-picture-box');
        if (!box || !this.view.contentDOM.contains(box)) return;
        e.preventDefault();
        const n = boxLine(this.view, box);
        if (n < 0) return;
        const line = this.view.state.doc.line(n + 1);
        this.view.focus();
        this.view.dispatch({ selection: EditorSelection.single(line.from, line.to), userEvent: 'select.pointer' });
      };

      /// Not the browser's own drag of the picture out of the note.
      noDrag = (e: DragEvent) => {
        if ((e.target as HTMLElement).closest?.('.cm-picture-box')) e.preventDefault();
      };

      /// The ring and handles over the selected picture, after the page
      /// has been laid out.
      place() {
        this.view.requestMeasure({
          key: this,
          read: () => {
            if (this.drag) return undefined;
            const p = picked(pe);
            const el = p && drawnPicture(this.view, p.line);
            if (!p || !el) return null;
            return { el, right: p.float === 2 };
          },
          write: (m) => {
            if (m === undefined) return;
            // Selected: no caret beside it, nor a wash over it (theme.ts).
            this.view.dom.classList.toggle('cm-picture-picked', !!m);
            if (!m) {
              this.box.hidden = true;
              this.img = null;
              return;
            }
            const img = m.el instanceof HTMLImageElement && m.el.naturalWidth ? m.el : null;
            this.img = img;
            this.box.classList.toggle('right', m.right);
            // A card (not found, or not shown here): the ring, no handles.
            this.box.classList.toggle('sized', !!img);
            this.box.hidden = false;
            this.over(m.el);
            if (img && !img.complete) img.addEventListener('load', () => this.place(), { once: true });
          },
        });
      }

      /// The box put over `el`.
      over(el: HTMLElement) {
        const r = el.getBoundingClientRect();
        const o = this.view.dom.getBoundingClientRect();
        const s = this.box.style;
        s.left = `${r.left - o.left}px`;
        s.top = `${r.top - o.top}px`;
        s.width = `${r.width}px`;
        s.height = `${r.height}px`;
      }

      down(e: PointerEvent, grip: HTMLElement, left: boolean) {
        const img = this.img;
        if (!img || (e.pointerType === 'mouse' && e.button !== 0)) return;
        e.preventDefault();
        e.stopPropagation();
        grip.setPointerCapture(e.pointerId);
        const w0 = img.getBoundingClientRect().width;
        // The text's width at most; in a row at an edge, the picture's
        // share of it.
        const text = textWidth(this.view);
        const row = img.closest('.cm-pics');
        const n = row?.querySelectorAll('.cm-picture-box').length || 1;
        const max = row?.classList.contains('cm-float') ? Math.floor((text - 16 * (n - 1)) / n) : text;
        this.drag = { left, x0: e.clientX, w0, w: w0, max, text, img, grip };
        // Past what a picture at an edge has with no width given.
        img.style.maxWidth = `${max}px`;
        this.box.classList.add('dragging');
        this.show(w0);
      }

      move(e: PointerEvent) {
        const d = this.drag;
        if (!d) return;
        const dx = d.left ? d.x0 - e.clientX : e.clientX - d.x0;
        d.w = dragged_width(d.w0, dx, d.max);
        d.img.style.width = `${d.w}px`;
        this.over(d.img);
        this.show(d.w);
      }

      /// The size it is, said while dragged.
      show(w: number) {
        const img = this.drag?.img;
        if (!img) return;
        this.size.textContent = `${Math.round(w)} × ${Math.round((w * img.naturalHeight) / img.naturalWidth)}`;
      }

      /// Let go (`keep`), the width written; else as it was.
      up(keep: boolean) {
        const d = this.drag;
        if (!d) return;
        this.drag = null;
        this.box.classList.remove('dragging');
        const p = picked(pe);
        // Drawn as the note has it until the page says otherwise.
        d.img.style.width = p?.width ? `${p.width}px` : '';
        d.img.style.maxWidth = '';
        if (keep && p) {
          const fit = picture_width(d.img.naturalWidth, p.width ?? 0, d.text, d.max, p.float !== 0)[1];
          const width = resized_width(d.w, fit);
          run(() => pe.set_picture_width(width))(this.view);
        }
        this.place();
      }
    },
  );
}
