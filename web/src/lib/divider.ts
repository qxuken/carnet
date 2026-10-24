// How a divider is drawn, as the desktop's Settings → Dividers has it
// (settings.rs `Divider`): one of five ornaments, or the reader's own
// words or picture. The note still says `---` whichever. Chosen in this
// browser and kept in it, as how links open is (links.ts).
//
// The reader (styles.css `.note hr`) and the editor (theme.ts `.cm-rule`)
// draw it from the same custom properties on the page's root, which
// `showDivider` sets: so a choice made in Settings is on every divider at
// once, read or written. The ornaments are the desktop's own outlines
// (`carnet --icons` writes them, icons.rs `ornament_svg`), drawn through a
// mask in the faint ink; the words in the page's face and that ink; the
// picture as it is, its see-through parts kept. All a line tall, centred,
// never wider than the text.

const svgs = import.meta.glob('../icons/ornament-*.svg', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;

export type Divider = 'dots' | 'line' | 'diamonds' | 'lozenge' | 'wave' | 'text' | 'picture';
/// Every way, in the order Settings offers them.
export const DIVIDERS: Divider[] = ['dots', 'line', 'diamonds', 'lozenge', 'wave', 'text', 'picture'];
/// What a divider of the reader's own is drawn as while it has nothing
/// to draw: no words typed, or no picture kept.
export const FALLBACK: Divider = 'dots';
/// The longest the reader's own words may be, in letters.
export const TEXT_MOST = 40;
/// The most a picture is kept as, in pixels, as the desktop keeps one
/// (`DIVIDER_PICTURE_MOST`): as wide as the widest page's words on a
/// screen of twice the density, four times as tall as a divider is drawn.
export const PICTURE_MOST = { w: 1800, h: 168 };
/// How tall a divider is drawn, in px: a line of the page's words.
export const LINE = 27;
/// An SVG kept as it is up to this size; a bigger one is drawn into a
/// picture, as any other is.
const SVG_KEPT = 256 * 1024;

/// Each way in words, as the desktop's tiles name them.
export const DIVIDER_NAMES: Record<Divider, string> = {
  dots: 'Three dots',
  line: 'A short line',
  diamonds: 'Three diamonds',
  lozenge: 'A diamond between lines',
  wave: 'A wave',
  text: 'Your own words',
  picture: 'Your own picture',
};

/// A picture kept for dividers: its address (a `data:` URL) and its
/// shape, in pixels.
export type Picture = { src: string; w: number; h: number };

const KEY = 'carnet.divider';
const TEXT_KEY = 'carnet.dividerText';
const PICTURE_KEY = 'carnet.dividerPicture';

/// An ornament's outlines as the desktop exports them, in the text's
/// colour; one of the reader's own asked for, the one it falls back to.
export function ornament(d: Divider): string {
  const own = d === 'text' || d === 'picture';
  return svgs[`../icons/ornament-${own ? FALLBACK : d}.svg`] ?? '';
}

/// The size an SVG says it is: its `width` and `height` in px (or no
/// unit), else its `viewBox`'s. `null` when it says neither.
export function svgSize(svg: string): { w: number; h: number } | null {
  const tag = /<svg\b[^>]*>/i.exec(svg)?.[0];
  if (!tag) return null;
  const attr = (name: string) => new RegExp(`\\s${name}\\s*=\\s*["']([^"']*)["']`, 'i').exec(tag)?.[1];
  const px = (v: string | undefined) => (v && /^\s*[\d.]+\s*(px)?\s*$/.test(v) ? parseFloat(v) : NaN);
  const [w, h] = [px(attr('width')), px(attr('height'))];
  if (w > 0 && h > 0) return { w, h };
  const box = attr('viewBox')?.trim().split(/[\s,]+/).map(Number);
  if (box?.length === 4 && box[2] > 0 && box[3] > 0) return { w: box[2], h: box[3] };
  return null;
}

/// The reader's own words as they're kept: on one line, a short one.
export const ownWords = (text: string) => [...text.replace(/[\u0000-\u001f\u007f]/g, ' ')].slice(0, TEXT_MOST).join('');

/// A string as CSS writes one: in quotes, its quotes and backslashes
/// escaped, as `content` reads it.
const cssString = (s: string) => `"${s.replace(/[\\"]/g, (c) => `\\${c}`)}"`;

/// What draws a divider, as the custom properties the reader and the
/// editor read: how wide it is at most (`--divider-w`), what is masked
/// out of the ink (an ornament), the picture drawn instead, the ink (the
/// faint one for an ornament, none otherwise) and the words. Words not
/// typed, or no picture, are the ornament they fall back to.
export function dividerLook(d: Divider, text: string, picture: Picture | null): Record<string, string> {
  const words = text.trim();
  if (d === 'text' && words) {
    return { '--divider-w': '100%', '--divider-mask': 'none', '--divider-image': 'none', '--divider-ink': 'transparent', '--divider-words': cssString(words) };
  }
  if (d === 'picture' && picture && picture.w > 0 && picture.h > 0) {
    const w = Math.round((LINE * picture.w * 100) / picture.h) / 100;
    return { '--divider-w': `${w}px`, '--divider-mask': 'none', '--divider-image': `url(${cssString(picture.src)})`, '--divider-ink': 'transparent', '--divider-words': 'none' };
  }
  const svg = ornament(d);
  const w = svgSize(svg)?.w ?? 0;
  return {
    '--divider-w': `${w}px`,
    '--divider-mask': `url(${cssString(`data:image/svg+xml,${encodeURIComponent(svg)}`)})`,
    '--divider-image': 'none',
    '--divider-ink': 'currentColor',
    '--divider-words': 'none',
  };
}

function load(): { d: Divider; text: string; picture: Picture | null } {
  let d: Divider = FALLBACK;
  let text = '';
  let picture: Picture | null = null;
  try {
    const v = localStorage.getItem(KEY) as Divider | null;
    if (v && DIVIDERS.includes(v)) d = v;
    text = ownWords(localStorage.getItem(TEXT_KEY) ?? '');
    const p = JSON.parse(localStorage.getItem(PICTURE_KEY) ?? 'null') as Picture | null;
    if (p && typeof p.src === 'string' && p.src.startsWith('data:image/') && p.w > 0 && p.h > 0) picture = { src: p.src, w: p.w, h: p.h };
  } catch {
    // Storage refused (a private window), or a picture not read back:
    // three dots, until another is chosen.
  }
  return { d, text, picture };
}

let now = load();

/// How dividers are drawn now, and the reader's own words and picture,
/// kept whichever is chosen.
export const divider = () => now.d;
export const dividerText = () => now.text;
export const dividerPicture = () => now.picture;

/// `f` on this browser's storage; refused (a private window, or full),
/// chosen for now all the same, and said by `false`.
function keep(f: (s: Storage) => void): boolean {
  try {
    f(localStorage);
    return true;
  } catch {
    return false;
  }
}

/// The way chosen: kept in this browser, and drawn.
export function setDivider(to: Divider) {
  now = { ...now, d: to };
  keep((s) => (to === FALLBACK ? s.removeItem(KEY) : s.setItem(KEY, to)));
  showDivider();
}

/// The reader's own words, typed: kept to a line of `TEXT_MOST` letters.
export function setDividerText(text: string) {
  const words = ownWords(text);
  now = { ...now, text: words };
  keep((s) => (words ? s.setItem(TEXT_KEY, words) : s.removeItem(TEXT_KEY)));
  showDivider();
}

/// A picture picked, and chosen: `false` when this browser wouldn't keep
/// it (too big for its storage), shown until the page is closed.
export function setDividerPicture(p: Picture): boolean {
  now = { ...now, d: 'picture', picture: p };
  const kept = keep((s) => {
    s.setItem(PICTURE_KEY, JSON.stringify(p));
    s.setItem(KEY, 'picture');
  });
  showDivider();
  return kept;
}

/// Dividers drawn as chosen: the root's custom properties set.
export function showDivider() {
  if (typeof document === 'undefined') return;
  const root = document.documentElement.style;
  for (const [k, v] of Object.entries(dividerLook(now.d, now.text, now.picture))) root.setProperty(k, v);
}

/// The scale a picture `w` × `h` is kept at: no bigger than a divider
/// can use, its shape kept; a drawing (`vector`) as large as that, a
/// picture never made larger than it is, which would blur it.
export function keptScale(w: number, h: number, vector: boolean): number {
  const fit = Math.min(PICTURE_MOST.w / w, PICTURE_MOST.h / h);
  return vector ? fit : Math.min(1, fit);
}

const isSvg = (f: File) => f.type === 'image/svg+xml' || /\.svgz?$/i.test(f.name);

const asDataUrl = (f: Blob) =>
  new Promise<string>((done, fail) => {
    const r = new FileReader();
    r.onload = () => done(String(r.result));
    r.onerror = () => fail(r.error);
    r.readAsDataURL(f);
  });

/// A file picked for dividers, as it's kept: an SVG small enough as it
/// is; any other picture (or a large drawing) drawn on a canvas no bigger
/// than a divider can use, as a PNG, its see-through parts kept. Throws
/// when the browser can't read it as a picture.
export async function pictureFrom(file: File): Promise<Picture> {
  const vector = isSvg(file);
  let size: { w: number; h: number } | null = null;
  if (vector) {
    size = svgSize(await file.text());
    if (size && file.size <= SVG_KEPT) return { src: await asDataUrl(file.type ? file : new Blob([file], { type: 'image/svg+xml' })), ...size };
  }
  const url = URL.createObjectURL(vector && !file.type ? new Blob([file], { type: 'image/svg+xml' }) : file);
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    const w = size?.w ?? img.naturalWidth;
    const h = size?.h ?? img.naturalHeight;
    if (!(w > 0 && h > 0)) throw new Error('no size');
    const k = keptScale(w, h, vector);
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(w * k));
    canvas.height = Math.max(1, Math.round(h * k));
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('no canvas');
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    return { src: canvas.toDataURL('image/png'), w: canvas.width, h: canvas.height };
  } finally {
    URL.revokeObjectURL(url);
  }
}
