// The `/` menu, as the desktop's (carnet-core's slash.rs, through
// `PageEditor`): a line of a paragraph or a quote that is only `/` and a
// word, the caret at its end, offers the blocks the word could mean. Which
// lines offer it, what a word picks and what each command makes are the
// page's; here, its keys while it shows (↑ and ↓ choose, Enter or Tab
// take, Esc closes it for that line) and what each row says. Editor.svelte
// draws it under its line.

import { Prec, type Extension } from '@codemirror/state';
import { keymap } from '@codemirror/view';
import { slash_commands, type PageEditor } from '../../wasm/carnet_web.js';
import { apple } from './links';

/// What a command does past its line: `turn` it into a heading, a list
/// or a quote; put in a `table`, a `divider` or `code`; or ask for a
/// `picture` or a `file`, which go in on the line it leaves empty.
export type Act = 'turn' | 'table' | 'divider' | 'code' | 'picture' | 'file';
export type Command = { name: string; hint: string; act: Act };

/// The menu showing: CodeMirror's line it hangs from, the word typed, and
/// the commands it offers (indices into `commands()`), in their order.
export type Slash = { line: number; word: string; shown: number[] };

let all: Command[] | null = null;
/// Every command, in the menu's order (`slash_commands`).
export const commands = (): Command[] => (all ??= JSON.parse(slash_commands()));

/// Each command's icon (web/src/icons), in the menu's order.
const ICONS = ['h1', 'h2', 'h3', 'bullets', 'numbers', 'todo', 'quote', 'table', 'divider', 'code', 'picture', 'attach'];
export const iconOf = (i: number) => ICONS[i] ?? 'plus';

/// What to type instead, next time, as a row says it: a picture comes by
/// pasting, so its row shows the paste key as this system writes it.
export function hintOf(c: Command, mac = apple()): string {
  return c.act === 'picture' ? (mac ? '⌘V' : 'Ctrl+V') : c.hint;
}

/// The menu where the caret is, if it shows (`PageEditor.slash`).
export function slashHere(pe: PageEditor): Slash | null {
  const s = pe.slash();
  return s ? JSON.parse(s) : null;
}

/// The row highlighted as the menu goes from `was` to `now`: a new word
/// starts it over at the first, as on the desktop; the same word keeps
/// it, within what is shown.
export function follow(was: Slash | null, now: Slash | null, sel: number): number {
  if (!now) return 0;
  if (!was || was.word !== now.word || was.line !== now.line) return 0;
  return Math.min(sel, now.shown.length - 1);
}

/// What a key does while the menu shows, `sel` the row highlighted: the
/// next row (↑ and ↓ go round), the highlighted command taken (Enter,
/// Tab), or the menu closed (Esc). `null`: not the menu's key.
export function slashKey(key: string, s: Slash, sel: number): { sel: number } | { take: number } | 'close' | null {
  const n = s.shown.length;
  switch (key) {
    case 'ArrowUp':
      return { sel: (sel + n - 1) % n };
    case 'ArrowDown':
      return { sel: (sel + 1) % n };
    case 'Enter':
    case 'Tab':
      return { take: s.shown[Math.min(sel, n - 1)] };
    case 'Escape':
      return 'close';
  }
  return null;
}

export type SlashOptions = {
  /// The menu as it shows now, and its highlighted row.
  menu: () => Slash | null;
  sel: () => number;
  /// Another row highlighted.
  choose: (sel: number) => void;
  /// Command `i` taken.
  take: (i: number) => void;
  /// Esc: closed for its line.
  close: () => void;
};

/// The menu's keys, ahead of the page's own while it shows (Enter would
/// break the line, Tab indent it); Shift+Enter and Shift+Tab still the
/// page's, as on the desktop.
export function slashKeys(o: SlashOptions): Extension {
  const bind = (key: string) => ({
    key,
    run: () => {
      const s = o.menu();
      const did = s && slashKey(key, s, o.sel());
      if (!did) return false;
      if (did === 'close') o.close();
      else if ('take' in did) o.take(did.take);
      else o.choose(did.sel);
      return true;
    },
  });
  return Prec.highest(keymap.of(['ArrowUp', 'ArrowDown', 'Enter', 'Tab', 'Escape'].map(bind)));
}
