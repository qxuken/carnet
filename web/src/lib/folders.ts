// Folders in the library: what the desktop says of them, and which way a
// drag may go. Where a folder may go at all is the vault's rule
// (carnet-sync `vault::may_go`, asked through `sync.folderTargets`): never
// into itself or a folder inside it. This only words it and lays it out
// (folders.test.ts).

/// A folder's own name: the last part of its path.
export const leaf = (path: string) => path.split('/').at(-1) ?? path;

/// The folder a folder is in ("" for the top).
export function parentOf(path: string): string {
  const i = path.lastIndexOf('/');
  return i < 0 ? '' : path.slice(0, i);
}

/// Whether `path` is `folder` or inside it.
export const isInside = (path: string, folder: string) => folder !== '' && (path === folder || path.startsWith(`${folder}/`));

/// Where things went, as the desktop says it: "to “Work”", or "out of its
/// folder" for the top.
export function placeWords(to: string, whose: 'its' | 'their' = 'its'): string {
  return to ? `to “${leaf(to)}”` : `out of ${whose} folder${whose === 'their' ? 's' : ''}`;
}

/// "Moved “Plan” to “Work”": a note or a folder moved.
export const movedWords = (name: string, to: string) => `Moved “${name}” ${placeWords(to)}`;

/// A place in a "Move to" menu: the folder it goes to ("" the top), in
/// words.
export type Target = { to: string; label: string };

/// Where a note in `from` may go, as the desktop's menu lists them: out
/// of its folder (when it's in one), then every other folder, by its path.
export function noteTargets(folders: string[], from: string): Target[] {
  const out: Target[] = from ? [{ to: '', label: 'Out of its folder' }] : [];
  for (const f of folders) if (f !== from) out.push({ to: f, label: f });
  return out;
}

/// Where a folder may go (the vault's list, `sync.folderTargets`), in
/// words: the top as the desktop calls it, "To the top".
export const folderTargets = (targets: string[]): Target[] => targets.map((to) => ({ to, label: to || 'To the top' }));

/// The menu's words for deleting a folder: one with nothing in it is just
/// deleted; any other goes to Deleted whole (the desktop's "Move folder to
/// trash"), Undo away.
export const deleteLabel = (bare: boolean) => (bare ? 'Delete folder' : 'Move folder to Deleted');

/// What the notice says once a folder went: deleted, or to Deleted with
/// how many notes.
export function deletedWords(folder: string, bare: boolean, notes: number): string {
  if (bare) return `Deleted the folder “${leaf(folder)}”`;
  const what = notes === 0 ? '' : notes === 1 ? ' and its note' : ` and its ${notes} notes`;
  return `Moved “${leaf(folder)}”${what} to Deleted`;
}

/// The folder the library shows, once `folder` is now `to`: the same one,
/// or one inside it, followed there (the desktop's `folder_went`).
export function followed(shown: string | null, folder: string, to: string): string | null {
  if (shown === null || !isInside(shown, folder)) return shown;
  return to + shown.slice(folder.length);
}

/// The folder the library shows, once `folder` is gone: every note, if it
/// showed that one or one inside it.
export const afterDelete = (shown: string | null, folder: string) => (shown !== null && isInside(shown, folder) ? null : shown);

/// What is being dragged in the library: a note (and the folder it's in),
/// or a folder (and where the vault lets it go).
export type Drag = { note: string; from: string } | { folder: string; targets: string[] };

/// Whether a drag may be let go over the folder `to` ("" for All notes,
/// the top): a note anywhere but its own folder; a folder only where the
/// vault lets it go.
export function mayDrop(d: Drag | null, to: string): boolean {
  if (!d) return false;
  return 'note' in d ? d.from !== to : d.targets.includes(to);
}

/// Where a new folder's field goes among the folder rows: after the last
/// row of `parent` and the folders inside it (-1: right after All notes,
/// for the top when there are no folders).
export function namingAfter(folders: string[], parent: string): number {
  let at = -1;
  folders.forEach((f, i) => {
    if (parent === '' || isInside(f, parent)) at = i;
  });
  return at;
}

// ---- folding: a folder's subfolders hidden in the list, and back
//
// Which folders are folded is this browser's view of the list, not the
// vault's (the desktop keeps its own in its settings): kept in
// localStorage, by path, and followed when a folder is renamed or moved.

/// Whether `path`'s row is out of sight: a folder it is inside is folded.
export function hidden(path: string, folded: ReadonlySet<string>): boolean {
  if (!folded.size) return false;
  for (let i = path.indexOf('/'); i >= 0; i = path.indexOf('/', i + 1)) if (folded.has(path.slice(0, i))) return true;
  return false;
}

/// The folders with folders inside: the ones with a chevron.
export function withSubfolders(folders: readonly string[]): Set<string> {
  const out = new Set<string>();
  for (const f of folders) {
    const p = parentOf(f);
    if (p) out.add(p);
  }
  return out;
}

/// The folders as the list shows them: those folded away left out.
export const inSight = (folders: readonly string[], folded: ReadonlySet<string>) => folders.filter((f) => !hidden(f, folded));

/// `folder` folded (its subfolders hidden) or unfolded (`open`); with
/// `deep` (⌥/Alt), every folder inside it the same. Only a folder with
/// folders inside has anything to fold. A new set; the one given as it was.
export function fold(folded: ReadonlySet<string>, folders: readonly string[], folder: string, open: boolean, deep = false): Set<string> {
  const out = new Set(folded);
  const parents = withSubfolders(folders);
  const these = deep ? folders.filter((f) => isInside(f, folder)) : [folder];
  if (!these.includes(folder)) these.push(folder);
  for (const f of these) {
    if (open) out.delete(f);
    else if (parents.has(f)) out.add(f);
  }
  return out;
}

/// The folders `path` is inside unfolded, so its row is in sight: the
/// folder shown never is out of it.
export function unfoldTo(folded: ReadonlySet<string>, path: string): Set<string> {
  const out = new Set(folded);
  for (let p = parentOf(path); p; p = parentOf(p)) out.delete(p);
  return out;
}

/// The folded folders once `folder` is now `to` (renamed or moved): it
/// and the folders inside it folded as they were.
export function foldsFollow(folded: ReadonlySet<string>, folder: string, to: string): Set<string> {
  const out = new Set<string>();
  for (const f of folded) out.add(isInside(f, folder) ? to + f.slice(folder.length) : f);
  return out;
}

/// The folder shown, once `folder` is folded: the same, unless it was
/// inside it — then the folder folded, so what is shown stays in sight.
export const shownAfterFold = (shown: string | null, folder: string) => (shown !== null && shown !== folder && isInside(shown, folder) ? folder : shown);

/// What ← or → on a folder's row does, as a tree's rows do in Finder: →
/// unfolds a folded one (⌥/Alt: and every folder inside it), or, open
/// already, goes to the first folder inside; ← folds an open one, or goes
/// to the folder it's in. `null`: nothing.
export function arrow(
  key: 'ArrowLeft' | 'ArrowRight',
  alt: boolean,
  folder: string,
  folders: readonly string[],
  folded: ReadonlySet<string>,
): { fold: boolean; deep: boolean } | { go: string } | null {
  const parent = withSubfolders(folders).has(folder);
  const isFolded = folded.has(folder);
  if (key === 'ArrowRight') {
    if (!parent) return null;
    if (isFolded || alt) return { fold: false, deep: alt };
    const first = folders.find((f) => f !== folder && isInside(f, folder));
    return first ? { go: first } : null;
  }
  if (parent && (!isFolded || alt)) return { fold: true, deep: alt };
  const up = parentOf(folder);
  return up ? { go: up } : null;
}

const FOLDED_KEY = 'carnet.foldedFolders';

/// The folders folded in this browser; none when there is no storage (a
/// private window, storage refused) or what is kept can't be read.
export function readFolded(store: Pick<Storage, 'getItem'> | undefined = globalThis.localStorage): Set<string> {
  try {
    const v: unknown = JSON.parse(store?.getItem(FOLDED_KEY) ?? '[]');
    return new Set(Array.isArray(v) ? v.filter((f): f is string => typeof f === 'string' && f !== '') : []);
  } catch {
    return new Set();
  }
}

/// Keeps the folded folders in this browser, when it can: only a
/// convenience, so nothing is said when it can't.
export function keepFolded(folded: ReadonlySet<string>, store: Pick<Storage, 'setItem' | 'removeItem'> | undefined = globalThis.localStorage) {
  try {
    if (!folded.size) store?.removeItem(FOLDED_KEY);
    else store?.setItem(FOLDED_KEY, JSON.stringify([...folded].sort()));
  } catch {
    // Not kept: a private window, storage refused.
  }
}
