// The notes the library shows, in the order it draws them, and the next
// and previous note along them: Ctrl+Alt+↓ and ↑ (⌘⌥ on a Mac), as the
// desktop's (app.rs `step_note`). Its Ctrl+Tab, which goes round the
// list, is the browser's own here; the arrows stop at the list's ends,
// as the desktop's do.

/// What the library needs of a note to place it.
export type Placed = { key: string; folder: string; trashed: boolean; pinned: boolean; modified: number };

/// The notes the library shows: what was deleted (`trash`), or the
/// folder's own (`folder`, null for every note), or a search's matches
/// (`hits`) from every folder; pinned first, the newest next.
export function shownNotes<N extends Placed>(notes: N[], hits: Set<string> | null, folder: string | null, trash: boolean): N[] {
  const list = notes.filter(
    (n) =>
      n.trashed === trash &&
      (hits ? hits.has(n.key) : true) &&
      // A folder's own notes, as the desktop lists them; its subfolders'
      // are theirs.
      (hits || folder === null || n.folder === folder),
  );
  return list.sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.modified - a.modified);
}

/// Where a step from row `at` of a list of `len` comes to, as the
/// desktop's `step_in`: the next, or the one before going `up`, round to
/// the other end with `wrap`; from outside the list (`null`) its first,
/// or its last going back round. `null` for an end without `wrap`, an
/// empty list, or a step round to where it was.
export function stepIn(len: number, at: number | null, up: boolean, wrap: boolean): number | null {
  if (len === 0) return null;
  const last = len - 1;
  let to: number | null;
  if (at === null) to = up && wrap ? last : 0;
  else if (up) to = at > 0 ? at - 1 : wrap ? last : null;
  else to = at < last ? at + 1 : wrap ? 0 : null;
  return to === at ? null : to;
}

/// The note a step goes to from `current` along `keys` (the library's
/// order): the one below, or above going `up`, stopping at the ends.
export function stepNote(keys: string[], current: string | null, up: boolean): string | null {
  const at = current === null ? -1 : keys.indexOf(current);
  const to = stepIn(keys.length, at < 0 ? null : at, up, false);
  return to === null ? null : keys[to];
}

/// Which way a key goes along the notes: Ctrl+Alt+↑ up, ↓ down (⌘⌥ on a
/// Mac, `apple`), nothing else held. `null` for any other key.
export function stepKey(e: { key: string; altKey: boolean; shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }, apple: boolean): 'up' | 'down' | null {
  if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return null;
  const mod = apple ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || !e.altKey || e.shiftKey) return null;
  return e.key === 'ArrowUp' ? 'up' : 'down';
}
