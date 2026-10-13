import { describe, expect, it } from 'vitest';
import { type Entry, Pending, replay, type Notes } from './pending';

/// Records kept as IndexedDB keeps them: each write a moment later. What
/// they hold survives the page (a reload is a new `Pending` over them).
function kept() {
  const records = new Map<string, Entry>();
  let puts = 0;
  const later = () => new Promise((r) => setTimeout(r, Math.random() * 3));
  return {
    store: {
      put: async (key: string, e: Entry) => {
        await later();
        puts++;
        records.set(key, JSON.parse(JSON.stringify(e)));
      },
      del: async (key: string) => {
        await later();
        records.delete(key);
      },
    },
    entries: () => [...records.values()],
    puts: () => puts,
  };
}

/// A vault as last kept on disk: each note's versions (a version's text),
/// and the one it's at. An edit from a version it doesn't know is
/// refused, as `NoteDoc::apply_from` refuses one; from an older one, its
/// change made beside those since, as a merge does.
function vault(notes: Record<string, [string, string][]>): Notes & { text(key: string): string | undefined } {
  const docs = new Map(Object.entries(notes).map(([k, vs]) => [k, { versions: new Map(vs), at: vs[vs.length - 1][0] }]));
  let n = 0;
  return {
    source: (key) => {
      const d = docs.get(key);
      return d ? [d.versions.get(d.at)!, d.at] : null;
    },
    create: () => {
      const key = `new${++n}`;
      docs.set(key, { versions: new Map([['v0', '']]), at: 'v0' });
      return key;
    },
    edit: (key, base, md) => {
      const d = docs.get(key);
      if (!d || !d.versions.has(base)) return null;
      const now = d.versions.get(d.at)!;
      const was = d.versions.get(base)!;
      // From an older version: what was added since it, kept after.
      const merged = base === d.at ? md : md + now.slice(was.length);
      const v = `${d.at}+`;
      d.versions.set(v, merged);
      d.at = v;
      return [merged, v];
    },
    text: (key) => {
      const d = docs.get(key);
      return d?.versions.get(d.at);
    },
  };
}

describe('words typed, then a reload', () => {
  // The finding: words typed within about 2 s of a reload were lost: the
  // editor sends them after 500 ms, and the vault is kept 1.5 s after
  // that. They're kept apart at once now, and made again on opening.
  it('are made again on the vault as it was kept, when neither sent nor kept', async () => {
    const k = kept();
    const p = new Pending(k.store);
    p.typed('n', 'v1', 'Hello', null);
    p.typed('n', 'v1', 'Hello world', null);
    await p.settled();
    // Reloaded: the vault on disk is at v1, as it was opened.
    const disk = vault({ n: [['v1', 'Hel']] });
    expect(replay(k.entries(), disk)).toEqual(['n']);
    expect(disk.text('n')).toBe('Hello world');
  });

  it('sent and not kept: made from the version kept, once', async () => {
    const k = kept();
    const p = new Pending(k.store);
    p.typed('n', 'v1', 'Hello', null);
    p.sent('n', 'v2', 'Hello');
    p.typed('n', 'v2', 'Hello there', null);
    await p.settled();
    expect(k.entries()[0].bases).toEqual(['v2', 'v1']);
    // The vault kept before the send: v2 unknown to it.
    const disk = vault({ n: [['v1', 'Hel']] });
    replay(k.entries(), disk);
    expect(disk.text('n')).toBe('Hello there');
  });

  it('sent and kept, then more: made from the version kept, not twice', async () => {
    const k = kept();
    const p = new Pending(k.store);
    p.typed('n', 'v1', 'Hello', null);
    p.sent('n', 'v2', 'Hello');
    p.kept([['n', 'v2']]);
    await p.settled();
    expect(k.entries(), 'all of it kept: let go').toEqual([]);
    p.typed('n', 'v2', 'Hello again', null);
    await p.settled();
    const disk = vault({ n: [['v1', 'Hel'], ['v2', 'Hello']] });
    replay(k.entries(), disk);
    expect(disk.text('n')).toBe('Hello again');
  });

  it('sent and kept, the record not told yet: not made twice', async () => {
    const k = kept();
    const p = new Pending(k.store);
    p.typed('n', 'v1', 'Hello', null);
    await p.settled();
    // Kept with the edit in it (v2); this record still at v1.
    const disk = vault({ n: [['v1', 'Hel'], ['v2', 'Hello']] });
    expect(replay(k.entries(), disk)).toEqual([]);
    expect(disk.text('n')).toBe('Hello');
  });

  it('with words from another device kept since: both stay', async () => {
    const k = kept();
    const p = new Pending(k.store);
    p.typed('n', 'v1', 'Mine. ', null);
    await p.settled();
    // Kept on disk: another device's words, after v1.
    const disk = vault({ n: [['v1', ''], ['v1r', 'Theirs.']] });
    replay(k.entries(), disk);
    expect(disk.text('n')).toBe('Mine. Theirs.');
  });

  it('in a new note the vault never kept: a note again, in its folder', async () => {
    const k = kept();
    const p = new Pending(k.store);
    p.typed('fresh', 'v0', '# Trip\n\nPack the tent', 'Plans');
    await p.settled();
    const disk = vault({});
    const made = replay(k.entries(), disk);
    expect(made).toEqual(['new1']);
    expect(disk.text('new1')).toBe('# Trip\n\nPack the tent');
  });

  it('are written at once, the latest last, each note on its own', async () => {
    const k = kept();
    const p = new Pending(k.store);
    for (const w of ['a', 'ab', 'abc', 'abcd']) p.typed('n', 'v1', w, null);
    p.typed('m', 'w1', 'other', null);
    await p.settled();
    expect(k.puts()).toBeLessThanOrEqual(3);
    expect(Object.fromEntries(k.entries().map((e) => [e.key, e.md]))).toEqual({ n: 'abcd', m: 'other' });
  });
});
