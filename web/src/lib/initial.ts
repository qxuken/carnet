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

/// The lines `el`'s words take: the distinct tops of its line boxes.
export function linesOf(el: HTMLElement): number {
  const r = document.createRange();
  r.selectNodeContents(el);
  const tops = new Set<number>();
  for (const rect of r.getClientRects()) if (rect.height > 0) tops.add(Math.round(rect.top));
  return tops.size;
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
    if (now !== was) changed = true;
  }
  return changed;
}
