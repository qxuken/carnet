// The pictures notes show, as object URLs: each holds its picture's bytes
// in memory for as long as it lives, so only the ones on screen are kept,
// and a few seen lately (a note glanced away from and back). The rest are
// let go; asked for again, a picture is read back from what's kept for
// offline (or fetched), as it was the first time.

/// Off screen, how many are kept, and how many bytes they may hold.
const MOST = 24;
const BUDGET = 24 << 20;
/// A URL just handed out may not be in its `<img>` yet: kept this long
/// regardless, in milliseconds.
const GRACE = 10_000;

type Kept = { url: string; bytes: number; used: number };

/// Every object URL the page shows: an element's `src` or `href`. A URL
/// let go under an element still showing it would break the picture, so
/// these never go.
function onScreen(): Set<string> {
  const urls = new Set<string>();
  for (const el of document.querySelectorAll<HTMLElement>('[src^="blob:"], [href^="blob:"]')) {
    const url = el.getAttribute('src') ?? el.getAttribute('href');
    if (url) urls.add(url);
  }
  return urls;
}

export class Shown {
  private kept = new Map<string, Kept>();
  private timer: ReturnType<typeof setTimeout> | undefined;

  /// The URL a picture is shown by, if one is kept.
  get(path: string): string | undefined {
    const k = this.kept.get(path);
    if (!k) return undefined;
    k.used = Date.now();
    return k.url;
  }

  /// A URL for a picture's bytes: the one kept, if there is one, else a
  /// new one. Those off screen are looked over a moment later.
  add(path: string, blob: Blob): string {
    const have = this.get(path);
    if (have) return have;
    // A browser sniffs a photo's kind from its bytes, but shows an SVG
    // only when told it is one. In an `<img>`, an SVG's scripts never run.
    if (!blob.type && /\.svg$/i.test(path)) blob = new Blob([blob], { type: 'image/svg+xml' });
    const url = URL.createObjectURL(blob);
    this.kept.set(path, { url, bytes: blob.size, used: Date.now() });
    this.later(2000);
    return url;
  }

  private later(ms: number) {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => this.prune(), ms);
  }

  /// Lets go of the pictures off screen, past the few seen latest.
  prune(now = Date.now(), shown = onScreen()) {
    let n = 0;
    let bytes = 0;
    const off: [string, Kept][] = [];
    for (const [path, k] of this.kept) {
      if (shown.has(k.url)) k.used = now;
      else off.push([path, k]);
    }
    off.sort((a, b) => b[1].used - a[1].used);
    let again = false;
    for (const [path, k] of off) {
      n += 1;
      bytes += k.bytes;
      if (n <= MOST && bytes <= BUDGET) continue;
      // Too new to tell: looked at again once it isn't.
      if (now - k.used < GRACE) {
        again = true;
        continue;
      }
      URL.revokeObjectURL(k.url);
      this.kept.delete(path);
    }
    if (again) this.later(GRACE);
  }

  /// Every URL let go: leaving the account.
  clear() {
    clearTimeout(this.timer);
    for (const k of this.kept.values()) URL.revokeObjectURL(k.url);
    this.kept.clear();
  }
}
