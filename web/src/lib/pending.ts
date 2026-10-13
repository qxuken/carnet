// Words typed and not yet in what this browser keeps: kept apart at once,
// a small record a note, so a reload or a closed tab never loses them.
// What the vault keeps (its state) is written a while after an edit, all
// of it at once; until then, and while an edit waits to be sent, the
// note's words are here too, with the versions they were typed from
// (pending.test.ts).
//
// Opened again, each is made on the vault from the newest of its
// versions the vault knows: as if just sent, so nothing typed is lost.
// Nor made twice: an edit goes to the server only once what's kept here
// holds it (sync.svelte.ts `held`), and what's kept is kept only once
// these records name the versions sent; so an edit not in what's kept
// never left this browser, and one in it is skipped.

/// A note's words as typed: from which versions (the newest first: each
/// the editor sent and got back, the last the one it opened at), and the
/// folder of a new one, which the vault may not have kept yet. `sent`:
/// these words are the vault's at `bases[0]`, waiting only for the vault
/// to be kept.
export type Entry = { key: string; folder: string | null; bases: string[]; md: string; sent: boolean };

/// Where they're kept: a record a note.
export type Store = { put(key: string, e: Entry): Promise<void>; del(key: string): Promise<void> };

/// What `replay` makes them on: the vault's `edit`, `source` and `create`.
export type Notes = {
  edit(key: string, base: string, md: string): unknown;
  source(key: string): unknown[] | undefined | null;
  create(folder: string): string | null | undefined;
};

export class Pending {
  private now = new Map<string, Entry>();
  /// Each note's record being written, and those to write again after.
  private writing = new Map<string, Promise<void>>();
  private again = new Set<string>();

  constructor(private store: Store) {}

  /// `md` typed in note `key` from version `base` (`folder`, for a new
  /// note): kept at once.
  typed(key: string, base: string, md: string, folder: string | null) {
    const e = this.now.get(key) ?? { key, folder, bases: [], md, sent: false };
    if (e.bases[0] !== base) e.bases.unshift(base);
    e.md = md;
    e.sent = false;
    e.folder ??= folder;
    this.now.set(key, e);
    this.write(key);
  }

  /// The words sent: the vault's now, at `version` (merged with what came
  /// meanwhile: `md`).
  sent(key: string, version: string, md: string) {
    const e = this.now.get(key);
    if (!e) return;
    if (e.bases[0] !== version) e.bases.unshift(version);
    e.md = md;
    e.sent = true;
    this.write(key);
  }

  /// The notes with words kept here.
  keys(): string[] {
    return [...this.now.keys()];
  }

  /// The vault kept, each note as it was then (`versions`, taken as the
  /// vault was): words it holds are let go; for the rest, the versions
  /// older than the one kept are no longer needed.
  kept(versions: [string, string | undefined][]) {
    for (const [key, v] of versions) {
      const e = this.now.get(key);
      if (!e || !v) continue;
      const at = e.bases.indexOf(v);
      if (at < 0) continue;
      if (at === 0 && e.sent) this.now.delete(key);
      else e.bases = e.bases.slice(0, at + 1);
      this.write(key);
    }
  }

  /// Whether a record is being written.
  busy(): boolean {
    return this.writing.size > 0;
  }

  /// Every record written as it is now.
  async settled(): Promise<void> {
    while (this.writing.size) await Promise.all(this.writing.values());
  }

  private write(key: string) {
    if (this.writing.has(key)) {
      this.again.add(key);
      return;
    }
    const go = (async () => {
      do {
        this.again.delete(key);
        const e = this.now.get(key);
        try {
          await (e ? this.store.put(key, { ...e, bases: [...e.bases] }) : this.store.del(key));
        } catch {
          // Storage refused (full, say): the next change tries again.
        }
      } while (this.again.has(key));
    })();
    this.writing.set(key, go);
    void go.finally(() => this.writing.delete(key));
  }
}

/// Words kept here before a reload, made on the vault as it was kept:
/// each from the newest of its versions the vault knows (an edit from a
/// version it doesn't know is refused), so what was sent and kept is not
/// made again, and what wasn't is. A new note the vault didn't keep is
/// made again, in its folder. The keys of the notes they're in now.
export function replay(entries: Entry[], notes: Notes): string[] {
  const out: string[] = [];
  for (const e of entries) {
    if (!notes.source(e.key)) {
      if (e.folder === null || !e.md.trim()) continue;
      const key = notes.create(e.folder);
      const base = key ? notes.source(key)?.[1] : undefined;
      if (key && typeof base === 'string' && notes.edit(key, base, e.md)) out.push(key);
      continue;
    }
    if (notes.source(e.key)?.[0] === e.md) continue;
    if (e.bases.some((b) => notes.edit(e.key, b, e.md))) out.push(e.key);
  }
  return out;
}
