import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { beforeAll, describe, expect, it, vi } from 'vitest';

// No window here: a store for the setting, as a browser keeps it, with a
// choice made in an earlier visit.
const kept = vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as unknown as { localStorage: Storage }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    removeItem: (k: string) => void m.delete(k),
  } as Storage;
  m.set('carnet.divider', 'wave');
  m.set('carnet.dividerText', '~ ✦ ~');
  return m;
});

import {
  DIVIDERS,
  divider,
  dividerLook,
  dividerPicture,
  dividerText,
  keptScale,
  ornament,
  ownWords,
  setDivider,
  setDividerPicture,
  setDividerText,
  svgSize,
} from './divider';
import { theme } from './editor/theme';

let styles = '';
beforeAll(async () => {
  const fs = await import(/* @vite-ignore */ ['node', 'fs'].join(':'));
  styles = fs.readFileSync(new URL('../styles.css', import.meta.url), 'utf8');
});

describe('how dividers are drawn', () => {
  it('is kept in this browser, three dots when nothing is', () => {
    expect([divider(), dividerText(), dividerPicture()]).toEqual(['wave', '~ ✦ ~', null]);
    setDivider('lozenge');
    expect(kept.get('carnet.divider')).toBe('lozenge');
    setDivider('dots');
    expect(kept.has('carnet.divider')).toBe(false);
    expect(divider()).toBe('dots');
  });

  // The desktop's choices, in its order: five ornaments, then the
  // reader's own.
  it("offers the desktop's ways, each ornament the desktop's own outlines", () => {
    expect(DIVIDERS).toEqual(['dots', 'line', 'diamonds', 'lozenge', 'wave', 'text', 'picture']);
    expect(svgSize(ornament('dots'))).toEqual({ w: 33, h: 3 });
    expect(svgSize(ornament('lozenge'))).toEqual({ w: 96, h: 10 });
    expect(ornament('wave')).toContain('stroke="currentColor"');
    // The reader's own, asked for an ornament: the one they fall back to.
    expect(ornament('text')).toBe(ornament('dots'));
  });

  it('draws an ornament through a mask, in the ink', () => {
    const look = dividerLook('diamonds', '', null);
    expect(look['--divider-w']).toBe('36px');
    expect(look['--divider-mask']).toMatch(/^url\("data:image\/svg\+xml,%3Csvg/);
    expect([look['--divider-image'], look['--divider-ink'], look['--divider-words']]).toEqual(['none', 'currentColor', 'none']);
  });

  it("draws the reader's words as the text's width allows, in quotes CSS reads", () => {
    const look = dividerLook('text', ' say "hi" \\ ', null);
    expect(look['--divider-words']).toBe('"say \\"hi\\" \\\\"');
    expect([look['--divider-w'], look['--divider-mask'], look['--divider-ink']]).toEqual(['100%', 'none', 'transparent']);
  });

  it('draws a picture a line tall, its shape kept', () => {
    const look = dividerLook('picture', '', { src: 'data:image/png;base64,AAAA', w: 300, h: 50 });
    expect(look['--divider-w']).toBe('162px');
    expect(look['--divider-image']).toBe('url("data:image/png;base64,AAAA")');
    expect([look['--divider-mask'], look['--divider-ink'], look['--divider-words']]).toEqual(['none', 'transparent', 'none']);
  });

  // Words not typed, or no picture: three dots, as on the desktop.
  it("is three dots for the reader's own with nothing to draw", () => {
    const dots = dividerLook('dots', '', null);
    expect(dividerLook('text', '   ', null)).toEqual(dots);
    expect(dividerLook('picture', '', null)).toEqual(dots);
  });

  it("keeps the reader's words to a line of forty letters", () => {
    expect(ownWords('a\tb\nc')).toBe('a b c');
    expect([...ownWords('✦'.repeat(50))].length).toBe(40);
    setDividerText('• • •');
    expect([dividerText(), kept.get('carnet.dividerText')]).toEqual(['• • •', '• • •']);
    setDividerText('');
    expect(kept.has('carnet.dividerText')).toBe(false);
  });

  it('keeps a picture picked, and chooses it', () => {
    const p = { src: 'data:image/png;base64,AAAA', w: 2, h: 1 };
    expect(setDividerPicture(p)).toBe(true);
    expect([divider(), dividerPicture()]).toEqual(['picture', p]);
    expect(JSON.parse(kept.get('carnet.dividerPicture') ?? 'null')).toEqual(p);
    expect(kept.get('carnet.divider')).toBe('picture');
  });

  // 1800 × 168 at most, as the desktop keeps one; a photo never blown up,
  // a drawing drawn as large as that.
  it('keeps a picture no bigger than a divider can use', () => {
    expect(keptScale(3600, 168, false)).toBe(0.5);
    expect(keptScale(400, 672, false)).toBe(0.25);
    expect(keptScale(120, 28, false)).toBe(1);
    expect(keptScale(120, 28, true)).toBe(6);
  });

  it("reads an SVG's size from its width and height, or its viewBox", () => {
    expect(svgSize('<svg xmlns="x" width="120px" height="24">')).toEqual({ w: 120, h: 24 });
    expect(svgSize("<svg viewBox='0 0 300 40' width='100%'>")).toEqual({ w: 300, h: 40 });
    expect(svgSize('<svg width="10em" height="2em">')).toBeNull();
    expect(svgSize('<html>')).toBeNull();
  });

  // One source for both: the reader's `hr` and the editor's rule read the
  // same properties.
  it('is read the same by the reader and the editor', () => {
    const state = EditorState.create({ extensions: theme });
    const css = state.facet(EditorView.styleModule).map((m) => m.getRules()).join('\n');
    const hr = /\.note hr \{([^}]*)\}/.exec(styles)?.[1] ?? '';
    const rule = /\.cm-rule\b[^{,]*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    for (const prop of ['--divider-w', '--divider-image', '--divider-ink', '--divider-mask']) {
      expect(hr).toContain(`var(${prop})`);
      expect(rule).toContain(`var(${prop})`);
    }
    expect(styles).toMatch(/\.note hr::after \{ content: var\(--divider-words\); \}/);
    expect(css).toMatch(/\.cm-rule::after[^{]*\{[^}]*content: var\(--divider-words\)/);
    expect(hr).toContain('color: var(--faint)');
    expect(rule).toContain('color: var(--faint)');
  });
});
