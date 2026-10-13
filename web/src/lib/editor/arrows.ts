// Where the arrows go in the editor, decided apart from CodeMirror and the
// page's drawing, so it can be tested on its own (arrows.test.ts). page.ts
// asks, then measures where it must and moves the caret.

/// The editor's lines as the arrows see them: each line's ends, and what
/// it is (page.ts `Look.k`). Lines count from 1, as CodeMirror's do.
export interface Lines {
  count: number;
  line(n: number): { from: number; to: number };
  kind(n: number): string | undefined;
  lineAt(pos: number): number;
}

export type Sel = { anchor: number; head: number };

/// One thing to the eye: chosen whole by an arrow, then passed.
export const isAtom = (kind: string | undefined) => kind === 'divider' || kind === 'image';

/// The divider or picture line a selection covers whole, and nothing more.
export function chosenAtom(lines: Lines, sel: Sel): number | null {
  const from = Math.min(sel.anchor, sel.head);
  const to = Math.max(sel.anchor, sel.head);
  if (from === to) return null;
  const n = lines.lineAt(from);
  const l = lines.line(n);
  return isAtom(lines.kind(n)) && from === l.from && to === l.to ? n : null;
}

/// ← and → over a divider or a picture, as the page's arrows go (page.rs
/// `left`, `right`): an arrow onto it chooses it whole, and the next is
/// past it. Not a place before it and one after. `null`: CodeMirror's
/// own move.
export function across(lines: Lines, sel: Sel, right: boolean): Sel | null {
  const chosen = chosenAtom(lines, sel);
  if (chosen !== null) return past(lines, chosen, right);
  if (sel.anchor !== sel.head) return null;
  const n = lines.lineAt(sel.head);
  const line = lines.line(n);
  // The caret on a picture's line: it's drawn chosen already, so the
  // next is past it.
  if (lines.kind(n) === 'image') return past(lines, n, right);
  let k = n;
  if (lines.kind(n) !== 'divider') {
    if (sel.head !== (right ? line.to : line.from)) return null;
    k = right ? n + 1 : n - 1;
    if (k < 1 || k > lines.count || !isAtom(lines.kind(k))) return null;
  }
  const to = lines.line(k);
  return right ? { anchor: to.from, head: to.to } : { anchor: to.to, head: to.from };
}

/// The caret past line `n`: at the start of the line after it, or the end
/// of the one before; at its own edge at the note's.
function past(lines: Lines, n: number, right: boolean): Sel {
  const line = lines.line(n);
  const k = right ? n + 1 : n - 1;
  const at = k < 1 || k > lines.count ? (right ? line.to : line.from) : right ? lines.line(k).from : lines.line(k).to;
  return { anchor: at, head: at };
}

/// What ↑ or ↓ does: `null`, CodeMirror's own move; `at`, the caret put
/// there; `sel`, a divider or a picture chosen whole; `line`, onto that
/// line at the caret's column, measured (`leaving`: only once the caret
/// is on its own line's first row, going up, or its last, going down;
/// within its line, CodeMirror's).
export type Vertical = null | { at: number } | { sel: Sel } | { line: number; leaving: boolean };

export function vertical(lines: Lines, sel: Sel, down: boolean, extend: boolean): Vertical {
  const chosen = extend ? null : chosenAtom(lines, sel);
  if (!extend && chosen === null && sel.anchor !== sel.head) return null;
  const n = chosen ?? lines.lineAt(sel.head);
  const k = down ? n + 1 : n - 1;
  if (k < 1 || k > lines.count) {
    // Off a chosen divider or picture at the note's edge: the caret at
    // its edge.
    if (chosen !== null) {
      const l = lines.line(n);
      return { at: down ? l.to : l.from };
    }
    return null;
  }
  const kind = lines.kind(k);
  const to = lines.line(k);
  // Onto a divider or a picture (which CodeMirror's own motion passes
  // over when it is at an edge, measured as nothing, the words beside
  // it): chosen whole, as ← and → choose it (`across`), the head on the
  // side it was come to from; the next press is past it. Extending, to
  // its near edge. Onto a table: into its cell (page.ts).
  if (isAtom(kind) && !extend) return { sel: down ? { anchor: to.from, head: to.to } : { anchor: to.to, head: to.from } };
  if (kind === 'image' || kind === 'table') return { at: to.from };
  if (kind === 'divider') return { at: down ? to.from : to.to };
  // Off a picture, a divider or a table's line: onto the next, measured.
  const here = lines.kind(n);
  const off = chosen !== null || here === 'image' || here === 'divider' || here === 'table';
  return { line: k, leaving: !off };
}

/// A row of drawn text: its top and bottom.
export type Row = { top: number; bottom: number };

/// Whether a caret drawn at `caret` is on `row`.
export const onRow = (caret: Row, row: Row) => {
  const mid = (caret.top + caret.bottom) / 2;
  return mid >= row.top && mid <= row.bottom;
};

/// The side a caret put at `pos` keeps to (CodeMirror's `assoc`) so it is
/// drawn on `row`, `coords` measuring a place from one side or the other.
/// Where a wrapped line breaks, one place is both the end of a row and
/// the start of the next: a caret put at the start of a row and drawn —
/// and measured by the next ↑ — at the end of the row above would skip
/// that row.
export function sideOn(pos: number, row: Row, coords: (pos: number, side: 1 | -1) => Row | null): 1 | -1 {
  const after = coords(pos, 1);
  return after && onRow(after, row) ? 1 : -1;
}
