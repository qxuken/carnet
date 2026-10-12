// How far a paragraph's first letter drops, as the desktop has it
// (app_page.rs `drop_lines`): as many lines as the paragraph has of its
// own, three at most, and none for a paragraph of one line, whose letter
// is then drawn as the rest of its words are.

const SIZES = ['drop2', 'drop3', 'drop-none'];

/// The drop for a paragraph of `lines` lines.
export function dropFor(lines: number): string {
  const n = Math.min(3, lines);
  return n >= 2 ? `drop${n}` : 'drop-none';
}

/// The lines `el`'s words take: the distinct tops of its words' boxes.
/// A letter already dropped is a box taller than a line, not a line of
/// its own: left out.
export function linesOf(el: HTMLElement): number {
  const line = parseFloat(getComputedStyle(el).lineHeight) || 27;
  const r = document.createRange();
  r.selectNodeContents(el);
  const tops = new Set<number>();
  for (const rect of r.getClientRects()) {
    if (rect.height > 0 && rect.height <= line * 1.2) tops.add(Math.round(rect.top));
  }
  return Math.max(tops.size, 1);
}

/// Each of `els` given the drop its lines call for. Whether any changed:
/// a caller that measures its layout (the editor) measures again.
export function dropLetters(els: Iterable<HTMLElement>): boolean {
  let changed = false;
  for (const el of els) {
    const was = SIZES.find((c) => el.classList.contains(c));
    el.classList.remove(...SIZES);
    el.classList.add('drop-measure');
    const now = dropFor(linesOf(el));
    el.classList.remove('drop-measure');
    el.classList.add(now);
    if (now === was) continue;
    changed = true;
    // Chromium lays a first letter's `initial-letter` out once, and not
    // again for a class come after: the paragraph laid out anew.
    el.style.display = 'none';
    void el.offsetHeight;
    el.style.display = '';
  }
  return changed;
}
