// A drawn note's pictures, fetched: each `<img data-picture="<path>">`
// the wasm left in it (no `src`, so the browser asks for nothing first)
// becomes the picture, kept here or asked of the server. Links open in a
// new tab.
//
// Until a picture comes, its place is a box the size it will be laid out
// at (the page's width, or the width the note gives it; a picture's
// shape, 4:3, as no record says its own), the same box in the reader and
// the editor, before and after a reload, so the page doesn't jump when it
// comes; one a while
// coming (fetched from the server) says it's downloading, as the desktop
// does. One that doesn't come says so in that same box, and is asked for
// again when the notes change (a device may have sent it since), and
// now and then besides: 30 s on, then twice as long each time, every 10
// minutes at most — one sent with no note changed comes all the same. Carnet's
// own, before the account's vault has come, is still on its way: "not
// found" only once the server says it hasn't it. One on the web isn't loaded
// (as the desktop doesn't, and the page's CSP wouldn't): its box says so.

import { NOTES_CHANGED, NOT_ON_SERVER, sync } from './sync.svelte';
import warning from '../icons/warning.svg?raw';
import cloud from '../icons/cloud.svg?raw';
import cloudOff from '../icons/cloud-off.svg?raw';
import picture from '../icons/picture.svg?raw';

/// Why a picture on the web isn't asked for: it isn't loaded.
export const ON_THE_WEB = 'on the web';
/// What a picture's box says while it's on its way here (the desktop's
/// words).
export const DOWNLOADING = 'Downloading…';
/// How long a picture is asked for before its box says it's on its way:
/// one kept here comes sooner, and is never announced.
export const SAY_COMING = 300;
const onTheWeb = (src: string) => /^https?:/i.test(src.trim());

export function pictures(box: HTMLElement) {
  for (const img of box.querySelectorAll<HTMLImageElement>('img[data-picture]')) {
    const path = img.dataset.picture!;
    delete img.dataset.picture;
    img.dataset.path = path;
    unshown(img, path);
    show(img, path, () => (onTheWeb(path) ? Promise.reject(new Error(ON_THE_WEB)) : sync.picture(path)));
  }
  for (const a of box.querySelectorAll('a')) {
    a.target = '_blank';
    a.rel = 'noopener noreferrer';
  }
}

/// `img` (not yet drawn, its `src` unset) shown when `ask` gives its URL;
/// until then, and if it never does, a box in its place.
export function show(img: HTMLImageElement, name: string, ask: () => Promise<string>) {
  const place = placeholder(img);
  img.replaceWith(place);
  const shown = (url: string) => {
    waiting.delete(place);
    img.src = url;
    if (place.isConnected) place.replaceWith(img);
  };
  const asked = changes;
  let settled = false;
  const coming = setTimeout(() => {
    if (!settled && place.isConnected) fill(place, DOWNLOADING, name, true, cloud);
  }, SAY_COMING);
  const done = () => {
    settled = true;
    clearTimeout(coming);
  };
  ask().then((url) => (done(), shown(url)), (e: unknown) => {
    done();
    if (!place.isConnected) return;
    const [said, icon] = why(e, name);
    fill(place, said, name, true, icon);
    if (e instanceof Error && e.message === ON_THE_WEB) return;
    waiting.set(place, () => ask().then(shown));
    later();
    // The notes changed while it was asked for (the vault came to name
    // it, say): asked again now, not at the next change.
    if (changes !== asked) askAgain();
  });
}

/// Why a picture didn't come, in words, by what was said of it and where
/// it is. Only Carnet's own pictures, the ones in the attachments folder,
/// sync: any other a note shows (beside it, or outside the notes folder)
/// is the user's, and stays on the device that has it, until it's moved
/// into Carnet there (docs/SYNC.md §4).
function why(e: unknown, path: string): [string, string] {
  if (e instanceof Error && e.message === NOT_ON_SERVER) return ['Picture not found', warning];
  if (e instanceof Error && e.message === ON_THE_WEB) return ['A picture on the web, not loaded', picture];
  // A path from the notes folder (the reader's), or as the note writes it
  // (the editor's: `../attachments/…` from a folder).
  if (/^(\.\.\/)*attachments\//i.test(path)) {
    // The account's vault not come yet: it may well name it.
    if (sync.vaultPending?.()) return [DOWNLOADING, cloud];
    return ['This picture hasn’t reached the server yet', cloud];
  }
  return ['Not on this device', cloudOff];
}

/// Pictures not found, still on the page, and how to ask for each again.
const waiting = new Map<HTMLElement, () => Promise<void>>();
/// How many times the notes have changed: a picture asked for across a
/// change is asked again once it's said not found.
let changes = 0;
addEventListener(NOTES_CHANGED, () => {
  changes++;
  askAgain();
});

/// How long a picture not found waits before it's asked for again with
/// nothing changed, the first time; and at most, the wait doubling.
export const RETRY_FIRST = 30_000;
export const RETRY_MOST = 600_000;
let retryWait = RETRY_FIRST;
let retryTimer: ReturnType<typeof setTimeout> | undefined;

/// Pictures not found asked for again a while on, while any are on the
/// page: the wait doubles each time, back to the first once none wait.
function later() {
  if (retryTimer !== undefined) return;
  retryTimer = setTimeout(() => {
    retryTimer = undefined;
    askAgain();
    if (!waiting.size) {
      retryWait = RETRY_FIRST;
      return;
    }
    retryWait = Math.min(retryWait * 2, RETRY_MOST);
    later();
  }, retryWait);
}

/// Every picture not found asked for again: the notes changed (a device
/// may have sent it, or the vault names it now), or a while went by.
export function askAgain() {
  for (const [place, ask] of waiting) {
    if (!place.isConnected) {
      waiting.delete(place);
      continue;
    }
    if (place.dataset.asking) continue;
    place.dataset.asking = '1';
    ask().then(
      () => {},
      () => delete place.dataset.asking,
    );
  }
}

/// Whether a picture's path or name is an iPhone's HEIC.
export const isHeic = (name: string) => /\.hei[cf]$/i.test(name);

/// A HEIC picture this browser can't draw (Safari can; Chrome and Firefox
/// can't) becomes a card saying so, not a broken picture.
export function unshown(img: HTMLImageElement, name: string) {
  if (!isHeic(name)) return;
  img.addEventListener('error', () => {
    if (!img.getAttribute('src')) return;
    const place = placeholder(img);
    fill(place, 'This picture is HEIC, which this browser can’t show', name, false);
    img.replaceWith(place);
  });
}

/// What a picture's box is while the picture may still come: as tall as
/// a picture of its width (`.picture-card.coming`).
const COMING = 'picture-card coming';

/// The box a picture takes until it's drawn: as wide as it will be, the
/// width the note gives it (at most the page's), or the page's; and a
/// picture's height for that width.
function placeholder(img: HTMLImageElement): HTMLElement {
  const place = document.createElement('span');
  place.className = COMING;
  place.setAttribute('role', 'img');
  place.setAttribute('aria-label', img.alt ? `${img.alt}: loading` : 'A picture, loading');
  const width = img.style.width || (img.getAttribute('width') ? `${img.getAttribute('width')}px` : '');
  if (width) place.style.width = width;
  return place;
}

/// What a box says of a picture it stands for, with the picture's name.
function fill(place: HTMLElement, why: string, path: string, named = true, glyph = warning) {
  const name = path.split('/').pop() ?? path;
  // Coming still: the picture's height kept; one that won't, a card.
  place.className = why === DOWNLOADING ? COMING : 'picture-card';
  place.title = name;
  place.setAttribute('aria-label', named ? `${why}: ${name}` : why);
  const icon = document.createElement('span');
  icon.className = 'picture-card-icon';
  icon.setAttribute('aria-hidden', 'true');
  icon.innerHTML = glyph;
  const words = document.createElement('span');
  words.className = 'picture-card-words';
  words.textContent = why;
  if (named) {
    const small = document.createElement('small');
    small.textContent = name;
    words.append(small);
  }
  place.replaceChildren(icon, words);
}
