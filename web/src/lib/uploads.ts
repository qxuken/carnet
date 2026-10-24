// The files put in here and not yet on the server, as kept across a
// reload: `[path, id]`s. Every change is made in the order it was asked,
// one after another, so the server's word that a file is in (asked
// the moment it comes) is never kept before the file was, nor lost
// between a read and a write of another change (uploads.test.ts).

export type Entry = [path: string, id: string];

/// Where the list is kept: sealed, in the browser's database.
export type Keep = { read(): Promise<Entry[]>; write(list: Entry[]): Promise<void> };

export class Uploads {
  private last: Promise<unknown> = Promise.resolve();

  constructor(private keep: Keep) {}

  /// `change` made to the list once every change asked before it is
  /// done (it may wait on more); `null` from it leaves the list as it
  /// is. The list after.
  private change(change: (list: Entry[]) => Entry[] | null | Promise<Entry[] | null>): Promise<Entry[]> {
    const next = this.last.then(async () => {
      const list = await this.keep.read();
      const now = await change(list);
      if (now) await this.keep.write(now);
      return now ?? list;
    });
    // One that failed (storage refused) doesn't stop those after it.
    this.last = next.catch(() => {});
    return next;
  }

  /// The list, once the changes asked before are made.
  list(): Promise<Entry[]> {
    return this.change(() => null);
  }

  /// The id of the file on its way to `path`, if one is: the last put
  /// in there. What this browser shows there until the vault names it,
  /// from the copy kept here (after a reload, offline, say).
  async going(path: string): Promise<string | undefined> {
    const list = await this.list();
    for (let n = list.length - 1; n >= 0; n--) if (list[n][0] === path) return list[n][1];
    return undefined;
  }

  /// A file put in at `path`: kept as on its way. Asked before it's
  /// sent, so its "sent" comes after.
  add(path: string, id: string): Promise<Entry[]> {
    return this.change((list) => (list.some(([p, i]) => p === path && i === id) ? null : [...list, [path, id]]));
  }

  /// The server has the file `id`: on its way no more, at any path.
  sent(id: string): Promise<Entry[]> {
    return this.change((list) => (list.some(([, i]) => i === id) ? list.filter(([, i]) => i !== id) : null));
  }

  /// Opened again (a reload): each file handed to `upload` with its
  /// bytes (`upload` says the id it goes as), and kept on the list only
  /// if it's on its way after all: `going()` names it. One whose bytes
  /// aren't kept here any more, or that the vault has already (sent
  /// before the reload, its "sent" lost), goes from the list: it would
  /// never be said sent, and would count as not sent if this browser
  /// were removed.
  resume(bytes: (id: string) => Promise<Uint8Array | null>, upload: (path: string, bytes: Uint8Array) => string | undefined, going: () => string[]): Promise<Entry[]> {
    return this.change(async (list) => {
      const handed: Entry[] = [];
      for (const [path, id] of list) {
        const b = await bytes(id);
        const as = b ? upload(path, b) : undefined;
        if (as) handed.push([path, as]);
      }
      const on = new Set(going());
      const now = handed.filter(([, id]) => on.has(id));
      return now.length === list.length && now.every(([p, i], n) => p === list[n][0] && i === list[n][1]) ? null : now;
    });
  }
}
