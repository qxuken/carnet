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
