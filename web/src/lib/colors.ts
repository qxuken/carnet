// The colours words can have, as the desktop has them (src/design.rs
// `tint`): ten hues, each written in the file as its light-theme colour
// so any Markdown viewer shows it, and drawn here in the colour each
// theme has for it (CSS `light-dark`, the page's `color-scheme`). A
// colour from another app is drawn as written; in the dark, a highlighter
// of one is sunk most of the way into the page.

export type Hue = { name: string; ink: number; wash: number; inkDark: number; washDark: number };

const hue = (name: string, ink: number, wash: number, inkDark: number, washDark: number): Hue => ({
  name,
  ink,
  wash: wash * 256 + 0xcc,
  inkDark,
  washDark,
});

export const HUES: Hue[] = [
  hue('Grey', 0x857c76, 0xe6e1dc, 0xb8afa8, 0x3b3633),
  hue('Brown', 0x9a6f4f, 0xefdcc8, 0xd0a27c, 0x4b3828),
  hue('Red', 0xc0553f, 0xf8cfc4, 0xf0907a, 0x5c2d26),
  hue('Orange', 0xc4762f, 0xf9dbb6, 0xf0a866, 0x5c3d20),
  hue('Yellow', 0xa98a1c, 0xf8eba6, 0xe3c45e, 0x52461c),
  hue('Green', 0x5f8f4a, 0xd8eac0, 0x9cc884, 0x30462a),
  hue('Teal', 0x3a8c86, 0xc4e8e1, 0x72c7bd, 0x1f4642),
  hue('Blue', 0x5279b3, 0xcddcf3, 0x94b5ec, 0x263856),
  hue('Purple', 0x8d68ad, 0xe2d5f0, 0xc2a0e2, 0x403256),
  hue('Pink', 0xbb5a85, 0xf5cfe0, 0xec95ba, 0x562c40),
];

/// The plain highlighter, `==…==`.
export const HIGHLIGHT = 1;

/// A hue's words' colour, as a file writes it (0xRRGGBBAA).
export const inkOf = (h: Hue) => h.ink * 256 + 0xff;

const rgb = (c: number) => `#${c.toString(16).padStart(6, '0')}`;
const rgba = (c: number) => `#${(c >>> 0).toString(16).padStart(8, '0')}`;

/// Words' colour `c` (0xRRGGBBAA, as the file has it), for CSS.
export function inkCss(c: number): string {
  const h = HUES.find((h) => inkOf(h) === c);
  return h ? `light-dark(${rgb(h.ink)}, ${rgb(h.inkDark)})` : rgba(c);
}

/// A highlighter's wash `c` (0xRRGGBBAA, 1 the plain one), for CSS.
export function washCss(c: number): string {
  if (c === HIGHLIGHT) return 'var(--highlight)';
  const h = HUES.find((h) => h.wash === c);
  if (h) return `light-dark(${rgba(h.wash)}, ${rgb(h.washDark)})`;
  return `light-dark(${rgba(c)}, color-mix(in srgb, ${rgb(c >>> 8)} 20%, var(--page)))`;
}

/// A colour as CSS has it (`#c0553f`, `#fcefa1cc`), as 0xRRGGBBAA.
export function parse(css: string): number | null {
  const m = /^#([0-9a-f]{6})([0-9a-f]{2})?$/i.exec(css.trim());
  return m ? parseInt(m[1] + (m[2] ?? 'ff'), 16) : null;
}

/// The reader's colours (export::viewer writes them as `--ink` and
/// `--mark`, as the file has them) drawn as the desktop draws them.
export function tints(box: HTMLElement) {
  for (const el of box.querySelectorAll<HTMLElement>('[style*="--ink"]')) {
    const c = parse(el.style.getPropertyValue('--ink'));
    if (c !== null) el.style.color = inkCss(c);
  }
  for (const el of box.querySelectorAll<HTMLElement>('[style*="--mark"]')) {
    const c = parse(el.style.getPropertyValue('--mark'));
    if (c !== null) el.style.backgroundColor = washCss(c);
  }
}
