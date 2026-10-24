// A link to a file in the notes opens it, as the desktop's does
// (app_open.rs): its bytes from the vault, kept here or fetched, handed to
// the browser, which shows a PDF, words, sound, film or a photo in a tab
// of its own and saves any other kind, to open in an app of its own. One
// that could run a program as it opens (`doc::could_run`: a program, an
// installer, a script, a shortcut) is asked about first, in the
// desktop's words, Cancel what Enter, Esc or a press outside does
// (OpenAsk.svelte).

import { could_run } from '../wasm/carnet_web.js';
import { sync } from './sync.svelte';
import { fileName, shownAs } from './files';

class Opener {
  /// The file asked about before it opens, by its path from the notes
  /// folder; none while nothing is asked.
  asking = $state<string | null>(null);

  /// A link in a note in `folder` followed: a file the vault has (or one
  /// on its way there from here) opened, and true; a link to anything
  /// else, false, the browser's to follow.
  link(folder: string, href: string): boolean {
    const path = sync.linkedFile(folder, href);
    if (!path) return false;
    if (could_run(fileName(path))) this.asking = path;
    else void this.open(path);
    return true;
  }

  /// The one asked about, opened all the same.
  anyway() {
    const path = this.asking;
    this.asking = null;
    if (path) void this.open(path);
  }

  cancel() {
    this.asking = null;
  }

  /// The file at `path` handed to the browser: shown in a tab of its own
  /// when it's a kind a browser shows safely, else saved under its name.
  /// A tab the browser won't open (a press long ago, as it counts them)
  /// is a save instead.
  async open(path: string) {
    const name = fileName(path);
    let blob: Blob;
    try {
      blob = await sync.file(path);
    } catch {
      sync.say(sync.vaultPending() ? `“${name}” isn’t here yet: it comes once the notes are in.` : `Couldn’t open “${name}”: it isn’t on the server yet. It comes once the device that has it is online.`);
      return;
    }
    const type = shownAs(name);
    const url = URL.createObjectURL(type ? new Blob([blob], { type }) : blob);
    // Long enough for the tab to read it, or the save to begin.
    setTimeout(() => URL.revokeObjectURL(url), 120_000);
    // Not `noopener`, which makes `open` say null whether a tab opened or
    // not; the tab let go of this page at once instead.
    const tab = type ? window.open(url, '_blank') : null;
    if (tab) {
      tab.opener = null;
      return;
    }
    const a = document.createElement('a');
    a.href = url;
    a.download = name;
    a.click();
  }
}

export const opener = new Opener();
