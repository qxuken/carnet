// How a link in a note opens while it's written in, as on the desktop
// (`carnet_core::page::LinkOpen`, chosen in Settings): ⌘-click (Ctrl-click
// elsewhere), a click, or a double-click. The press that opens a link
// leaves the caret where it was; any other places it, so a link's words
// can always be edited: with a click opening, a drag or a Shift-click
// selects, a ⌘-click places the caret, and so does a click at the words'
// very end. Which press opens, and which link is under it, are the page's
// (`link_opens`, `PageEditor.link_under`): only one a page may follow,
// never a `javascript:` one. Over a link a press would open, with the
// keys held now, the pointer is a hand. The reader opens a link on a
// plain click, whatever this says.

import { ViewPlugin, type EditorView } from '@codemirror/view';
import type { EditorSelection } from '@codemirror/state';
import { link_hand, link_ignores, link_opens, type PageEditor } from '../../wasm/carnet_web.js';

export type LinkOpen = 'cmd-click' | 'click' | 'double-click';
export const LINK_OPENS: LinkOpen[] = ['cmd-click', 'click', 'double-click'];
const KEY = 'carnet.openLinks';

/// Whether this is a Mac (or an iPhone, an iPad): ⌘ there, Ctrl elsewhere.
export const apple = () => typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '');

/// The setting's choices in words: "⌘-click" on a Mac, "Ctrl-click" elsewhere.
export const linkOpenWords = (h: LinkOpen) => (h === 'click' ? 'Click' : h === 'double-click' ? 'Double-click' : apple() ? '⌘-click' : 'Ctrl-click');

function load(): LinkOpen {
  try {
    const v = localStorage.getItem(KEY);
    if (v === 'click' || v === 'double-click') return v;
  } catch {
    // Storage refused (a private window): the default.
  }
  return 'cmd-click';
}

let how: LinkOpen = load();

/// How links open now.
export const linkOpen = () => how;

/// How links open, chosen: kept in this browser.
export function setLinkOpen(to: LinkOpen) {
  how = to;
  try {
    if (to === 'cmd-click') localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, to);
  } catch {
    // Not kept: chosen for now all the same.
  }
}

/// Opens an address a page may follow, as the link sheet's Open does.
const openTab = (url: string) => void window.open(url, '_blank', 'noopener,noreferrer');

type Press = { url: string; x: number; y: number; sel: EditorSelection };

/// What a press does to the link under it (`url`, or none), the
/// settings opening links `how`, `clicks` in a row: opens it now, once
/// let go, nothing of its own (`null`: what a press does anywhere), or
/// goes nowhere — a triple-click's third press after a double-click
/// opened the link (`opened`), which would choose its line.
export function pressed(how: LinkOpen, clicks: number, cmd: boolean, shift: boolean, url: () => string | null, opened: boolean): 'open' | 'release' | 'ignore' | null {
  if (link_ignores(how, clicks, opened)) return 'ignore';
  const when = link_opens(how, clicks, cmd, shift);
  if (!when || !url()) return null;
  return when === 1 ? 'open' : 'release';
}

export function linkOpening(pe: PageEditor, open: (url: string) => void = openTab) {
  return ViewPlugin.fromClass(
    class {
      /// A press on a link that opens once let go where it was pressed.
      press: Press | null = null;
      /// The last press opened a link (`pressed`).
      opened = false;
      /// Where the pointer is over the page, for a key pressed or let go.
      at: { x: number; y: number } | null = null;

      constructor(readonly view: EditorView) {
        view.contentDOM.addEventListener('mousedown', this.down, true);
        view.contentDOM.addEventListener('mousemove', this.moved);
        view.contentDOM.addEventListener('mouseleave', this.left);
        addEventListener('mouseup', this.up, true);
        addEventListener('keydown', this.keys);
        addEventListener('keyup', this.keys);
        addEventListener('blur', this.left);
      }

      destroy() {
        this.view.contentDOM.removeEventListener('mousedown', this.down, true);
        this.view.contentDOM.removeEventListener('mousemove', this.moved);
        this.view.contentDOM.removeEventListener('mouseleave', this.left);
        removeEventListener('mouseup', this.up, true);
        removeEventListener('keydown', this.keys);
        removeEventListener('keyup', this.keys);
        removeEventListener('blur', this.left);
        this.view.dom.classList.remove('cm-link-hand');
      }

      /// The address a press at a point, on `target`, would open: a link's
      /// words drawn there, the page saying which and whether it may.
      linkAt(x: number, y: number, target: EventTarget | null): string | null {
        if (!(target instanceof Element) || !target.closest('.cm-l')) return null;
        const pos = this.view.posAtCoords({ x, y });
        if (pos == null) return null;
        const line = this.view.state.doc.lineAt(pos);
        return pe.link_under(line.number - 1, pos - line.from, how) ?? null;
      }

      down = (e: MouseEvent) => {
        this.press = null;
        if (e.button !== 0) return;
        let url: string | null = null;
        const does = pressed(how, e.detail, apple() ? e.metaKey : e.ctrlKey, e.shiftKey, () => (url = this.linkAt(e.clientX, e.clientY, e.target)), this.opened);
        this.opened = does === 'open' || (does === 'ignore' && this.opened);
        if (does === 'open' || does === 'ignore') {
          e.preventDefault();
          e.stopPropagation();
          if (does === 'open' && url) open(url);
          return;
        }
        if (does === 'release' && url) this.press = { url, x: e.clientX, y: e.clientY, sel: this.view.state.selection };
      };

      /// Let go where it was pressed: the link opens, and the caret goes
      /// back where it was. Dragged away, it's a selection.
      up = (e: MouseEvent) => {
        const p = this.press;
        this.press = null;
        if (!p || e.button !== 0 || Math.hypot(e.clientX - p.x, e.clientY - p.y) >= 4) return;
        open(p.url);
        // After CodeMirror's own end of the press, which sets the caret
        // where it was let go.
        setTimeout(() => {
          if (this.view.dom.isConnected) this.view.dispatch({ selection: p.sel, userEvent: 'select.pointer' });
        });
      };

      moved = (e: MouseEvent) => {
        this.at = { x: e.clientX, y: e.clientY };
        this.hand(e.clientX, e.clientY, e.target, apple() ? e.metaKey : e.ctrlKey, e.shiftKey, e.buttons !== 0);
      };

      keys = (e: KeyboardEvent) => {
        if (!this.at || !['Meta', 'Control', 'Shift'].includes(e.key)) return;
        const { x, y } = this.at;
        this.hand(x, y, document.elementFromPoint(x, y), apple() ? e.metaKey : e.ctrlKey, e.shiftKey, false);
      };

      left = () => {
        this.at = null;
        this.view.dom.classList.remove('cm-link-hand');
      };

      /// The pointer a hand over a link a press would open, keys held so.
      hand(x: number, y: number, target: EventTarget | null, cmd: boolean, shift: boolean, pressed: boolean) {
        const on = !pressed && link_hand(how, cmd, shift) && !!this.linkAt(x, y, target);
        this.view.dom.classList.toggle('cm-link-hand', on);
      }
    },
  );
}
