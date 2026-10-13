import { describe, expect, it } from 'vitest';

const files = import.meta.glob(['./**/*.svelte'], { query: '?raw', import: 'default', eager: true }) as Record<string, string>;

/// Each field's tag in the screens, with the file it's in.
function fields(): [string, string][] {
  const out: [string, string][] = [];
  for (const [file, src] of Object.entries(files)) {
    for (const m of src.matchAll(/<(input|textarea|select)\b[^>]*>/g)) out.push([file, m[0].replace(/\s+/g, ' ')]);
  }
  return out;
}

describe('fields', () => {
  // The finding: the library's search field had only its placeholder,
  // which a screen reader may not say, and which goes once words are in.
  it('each says what it is for, to a screen reader too', () => {
    const all = fields();
    expect(all.some(([f, tag]) => f.endsWith('Library.svelte') && tag.includes('type="search"'))).toBe(true);
    for (const [file, tag] of all) {
      const named = /\saria-label(ledby)?=/.test(tag) || /\saria-hidden="true"/.test(tag);
      expect(named, `${file}: ${tag}`).toBe(true);
    }
  });
});
