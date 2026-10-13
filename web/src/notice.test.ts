import { describe, expect, it } from 'vitest';
import app from './App.svelte?raw';

/// A rule of App.svelte's style, as `property: value` pairs.
function rule(selector: string): Map<string, string> {
  const css = app.slice(app.indexOf('<style>'));
  const body = css.match(new RegExp(`\\n\\s*${selector.replace('.', '\\.')} \\{([^}]*)\\}`))?.[1] ?? '';
  const out = new Map<string, string>();
  for (const decl of body.replace(/\/\*[\s\S]*?\*\//g, '').split(';')) {
    const at = decl.indexOf(':');
    if (at > 0) out.set(decl.slice(0, at).trim(), decl.slice(at + 1).trim());
  }
  return out;
}

describe('the notice at the bottom (Undo and the like)', () => {
  // The finding: at 375px, deleting a folder's notice went a word or two
  // a line. Placed from the middle (left: 50%, pulled back by half its
  // width), it could be no wider than half the screen.
  it('spans the screen less its margins, as wide as its words', () => {
    const r = rule('.notice');
    expect(r.get('position')).toBe('fixed');
    expect(r.get('left')).toBe('16px');
    expect(r.get('right')).toBe('16px');
    expect(r.get('margin')).toBe('0 auto');
    expect(r.get('width')).toBe('fit-content');
    expect(r.has('transform')).toBe(false);
  });

  it('keeps its Undo on one line', () => {
    expect(rule('.notice button').get('white-space')).toBe('nowrap');
  });
});
