// A drawn note's pictures, fetched: each `carnet-picture:<path>` the wasm
// left in it becomes the picture, kept here or asked of the server. Links
// open in a new tab.
//
// Until a picture comes, its place is a box the size it will be laid out
// at (the page's width, or the width the note gives it), the same box in
// the reader and the editor, before and after a reload. One that doesn't
// come says so in that same box, and is asked for again when the notes
// change: a device may have sent it since.

import { NOTES_CHANGED, NOT_ON_SERVER, sync } from './sync.svelte';
import warning from '../icons/warning.svg?raw';
import device from '../icons/device.svg?raw';

export function pictures(box: HTMLElement) {
  for (const img of box.querySelectorAll<HTMLImageElement>('img[src^="carnet-picture:"]')) {
    const path = img.getAttribute('src')!.slice('carnet-picture:'.length);
    img.removeAttribute('src');
    img.dataset.path = path;
    unshown(img, path);
    show(img, path, () => sync.picture(path));
  }
  for (const a of box.querySelectorAll('a')) {
    a.target = '_blank';
    a.rel = 'noopener';
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
  ask().then(shown, (e: unknown) => {
    if (!place.isConnected) return;
    const [said, icon] = why(e, name);
    fill(place, said, name, true, icon);
    waiting.set(place, () => ask().then(shown));
  });
}

/// Why a picture didn't come, in words, by what was said of it and where
/// it is. Only Carnet's own pictures, the ones in the attachments folder,
/// sync: any other a note shows (beside it, or outside the notes folder)
/// is the user's, and stays on the device that has it, until it's moved
/// into Carnet there (docs/SYNC.md §4).
function why(e: unknown, path: string): [string, string] {
  if (e instanceof Error && e.message === NOT_ON_SERVER) return ['Picture not found', warning];
  // A path from the notes folder (the reader's), or as the note writes it
  // (the editor's: `../attachments/…` from a folder).
  if (/^(\.\.\/)*attachments\//i.test(path)) return ['This picture hasn’t reached the server yet', device];
  return ['This picture is only on the device that has it. Move it into Carnet there to sync it.', device];
}

/// Pictures not found, still on the page, and how to ask for each again.
const waiting = new Map<HTMLElement, () => Promise<void>>();
addEventListener(NOTES_CHANGED, () => askAgain());

/// Every picture not found asked for again: the notes changed (a device
/// may have sent it, or the vault names it now).
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

/// The box a picture takes until it's drawn: as wide as it will be, the
/// width the note gives it (at most the page's), or the page's.
function placeholder(img: HTMLImageElement): HTMLElement {
  const place = document.createElement('span');
  place.className = 'picture-card';
  place.setAttribute('role', 'img');
  place.setAttribute('aria-label', img.alt ? `${img.alt}: loading` : 'A picture, loading');
  const width = img.style.width || (img.getAttribute('width') ? `${img.getAttribute('width')}px` : '');
  if (width) place.style.width = width;
  return place;
}

/// What a box says of a picture it stands for, with the picture's name.
function fill(place: HTMLElement, why: string, path: string, named = true, glyph = warning) {
  const name = path.split('/').pop() ?? path;
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
