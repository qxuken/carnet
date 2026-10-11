// A drawn note's pictures, fetched: each `carnet-picture:<path>` the wasm
// left in it becomes the picture, kept here or asked of the server. Links
// open in a new tab.

import { sync } from './sync.svelte';
import warning from '../icons/warning.svg?raw';

export function pictures(box: HTMLElement) {
  for (const img of box.querySelectorAll<HTMLImageElement>('img[src^="carnet-picture:"]')) {
    const path = img.getAttribute('src')!.slice('carnet-picture:'.length);
    img.removeAttribute('src');
    img.classList.add('waiting');
    img.dataset.path = path;
    unshown(img, path);
    sync.picture(path).then(
      (url) => {
        img.src = url;
        img.classList.remove('waiting');
      },
      () => missing(img, path),
    );
  }
  for (const a of box.querySelectorAll('a')) {
    a.target = '_blank';
    a.rel = 'noopener';
  }
}

/// Whether a picture's path or name is an iPhone's HEIC.
export const isHeic = (name: string) => /\.hei[cf]$/i.test(name);

/// A HEIC picture this browser can't draw (Safari can; Chrome and Firefox
/// can't) becomes a small card saying so, not a broken picture.
export function unshown(img: HTMLImageElement, name: string) {
  if (!isHeic(name)) return;
  img.addEventListener('error', () => {
    if (!img.getAttribute('src')) return;
    img.replaceWith(card('This picture is HEIC, which this browser can’t show', name));
  });
}

/// A picture the notes don't have (no device has sent it): a small card
/// saying so, with its name, as the desktop shows one it can't find, not
/// an empty box.
export function missing(img: HTMLImageElement, name: string) {
  img.replaceWith(card('Picture not found', name, true));
}

function card(why: string, path: string, named = false): HTMLElement {
  const name = path.split('/').pop() ?? path;
  const card = document.createElement('span');
  card.className = 'picture-card';
  card.setAttribute('role', 'img');
  card.title = name;
  card.setAttribute('aria-label', named ? `${why}: ${name}` : why);
  const icon = document.createElement('span');
  icon.className = 'picture-card-icon';
  icon.setAttribute('aria-hidden', 'true');
  icon.innerHTML = warning;
  const words = document.createElement('span');
  words.className = 'picture-card-words';
  words.textContent = why;
  if (named) {
    const small = document.createElement('small');
    small.textContent = name;
    words.append(small);
  }
  card.append(icon, words);
  return card;
}
