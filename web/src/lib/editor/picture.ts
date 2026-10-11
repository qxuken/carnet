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
  if (line + 1 > view.state.doc.lines) return null;
  let node: Node;
  try {
    node = view.domAtPos(view.state.doc.line(line + 1).from).node;
  } catch {
    return null;
  }
  const el = node.nodeType === 1 ? (node as HTMLElement) : node.parentElement;
  return el?.closest('.cm-line')?.querySelector<HTMLElement>('img.cm-picture, .picture-card') ?? null;
}

/// The width the text has: what a picture may take at most.
export const textWidth = (view: EditorView) => view.contentDOM.clientWidth;

type Run = (f: () => boolean) => (v: EditorView) => boolean;

type Drag = { left: boolean; x0: number; w0: number; w: number; max: number; img: HTMLImageElement; grip: HTMLElement };

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
        const box = (e.target as HTMLElement).closest?.('.cm-picture-box');
        if (!box || !this.view.contentDOM.contains(box)) return;
        e.preventDefault();
        const line = this.view.state.doc.lineAt(this.view.posAtDOM(box));
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
        this.drag = { left, x0: e.clientX, w0, w: w0, max: textWidth(this.view), img, grip };
        // Past what a picture at an edge has with no width given.
        img.style.maxWidth = '100%';
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
          const fit = picture_width(d.img.naturalWidth, p.width ?? 0, d.max, d.max, p.float !== 0)[1];
          const width = resized_width(d.w, fit);
          run(() => pe.set_picture_width(width))(this.view);
        }
        this.place();
      }
    },
  );
}
