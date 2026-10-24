// A note's menu, in the library and over the note: open it in a tab of
// its own (the desktop's "Open in new window"), pin it, move it to a
// folder, move it to Deleted, as the desktop's note menu has them.

import { sync, type Note } from './sync.svelte';
import type { MenuItem } from './menu';
import { movedWords, noteTargets } from './folders';

/// Pinned, it's said so: the desktop says it too.
export function pin(n: Note) {
  if (sync.setPinned(n.key, !n.pinned)) sync.say(n.pinned ? 'Unpinned' : 'Pinned');
}

/// The note into `to` ("" for the top), said as the desktop says it.
export function moveNote(n: Note, to: string) {
  if (sync.moveNote(n.key, to)) sync.say(movedWords(n.title, to));
}

/// Where the note may go: out of its folder, then every other folder.
export const moveItems = (n: Note): MenuItem[] => noteTargets(sync.folders, n.folder).map((t) => ({ label: t.label, run: () => moveNote(n, t.to) }));

/// To Deleted, Undo away; then `gone`, if given (the note's page leaves).
export function trash(n: Note, gone?: () => void) {
  if (!sync.setTrashed(n.key, true)) return;
  sync.say('Moved to Deleted', () => sync.setTrashed(n.key, false));
  gone?.();
}

/// The address that opens the note `key` (App.svelte reads `#note/<key>`),
/// from the page at `at`: its path and query kept, its hash the note's.
export const noteAddress = (key: string, at: { origin: string; pathname: string; search: string }) => `${at.origin}${at.pathname}${at.search}#note/${key}`;

/// The note in a tab of its own, by its address: the same notes, kept in
/// this browser, open there too.
export function openInTab(n: Note) {
  window.open(noteAddress(n.key, location), '_blank');
}

export function noteMenu(n: Note, gone?: () => void): MenuItem[] {
  const items: MenuItem[] = [
    { label: 'Open in new tab', run: () => openInTab(n) },
    { label: n.pinned ? 'Unpin' : 'Pin to top', icon: 'star', run: () => pin(n) },
  ];
  const targets = moveItems(n);
  if (targets.length) items.push({ label: 'Move to', icon: 'folder', items: targets });
  items.push({ label: 'Move to Deleted', icon: 'trash', danger: true, run: () => trash(n, gone) });
  return items;
}
