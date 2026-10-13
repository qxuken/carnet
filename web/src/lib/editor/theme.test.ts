import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { beforeAll, describe, expect, it } from 'vitest';
import { theme } from './theme';

// The page's own sheet, as written: read from its file (Vite hands a
// test an empty one).
let styles = '';
beforeAll(async () => {
  const fs = await import(/* @vite-ignore */ ['node', 'fs'].join(':'));
  styles = fs.readFileSync(new URL('../../styles.css', import.meta.url), 'utf8');
});

/// The CSS the editor's theme puts on the page.
function rules(): string {
  const state = EditorState.create({ extensions: theme });
  return state.facet(EditorView.styleModule).map((m) => m.getRules()).join('\n');
}

describe('code', () => {
  // As on the desktop, where it is kui's Mono: SF Mono or Menlo, Cascadia
  // Mono or Consolas, DejaVu Sans Mono or Liberation Mono.
  it("is the system's monospace, one stack for the page and the editor", () => {
    const stack = styles.match(/--mono:\s*([^;]+);/)?.[1] ?? '';
    expect(stack.startsWith('ui-monospace')).toBe(true);
    expect(stack.trim().endsWith('monospace')).toBe(true);
    for (const face of ['Menlo', 'Consolas', "'DejaVu Sans Mono'"]) expect(stack).toContain(face);
    expect(styles).toMatch(/\.note code \{ font: [^;]*var\(--mono\)/);
    const css = rules();
    // A block of code (and front matter), and a word of it.
    expect(css).toMatch(/\.cm-code[^{]*\{[^}]*font: 14px\/1\.6 var\(--mono\)/);
    expect(css).toMatch(/\.cm-c\b[^{]*\{[^}]*font: 0\.85em var\(--mono\)/);
    expect(css).not.toMatch(/Menlo/);
  });
});
