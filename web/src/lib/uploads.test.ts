import { describe, expect, it } from 'vitest';
import { type Entry, Uploads } from './uploads';

/// A list kept as the browser's database keeps it: each read and write
/// a moment later, as IndexedDB's are.
function kept(start: Entry[] = []) {
  let list = start;
  const later = () => new Promise((r) => setTimeout(r, Math.random() * 4));
  return {
    keep: {
      read: async () => {
        await later();
        return list.map((e) => [...e] as Entry);
      },
      write: async (l: Entry[]) => {
        await later();
        list = l;
      },
    },
    now: () => list,
  };
}

describe('Uploads', () => {
  it('keeps a picture said sent at once as sent, not waiting', async () => {
    // A fast server: its "sent" comes while the picture is still being
    // kept here. Asked in that order, the list ends empty.
    const k = kept();
    const up = new Uploads(k.keep);
    const added = up.add('attachments/a.png', 'aa');
    const sent = up.sent('aa');
    await Promise.all([added, sent]);
    expect(k.now()).toEqual([]);
  });

  it('loses no change made at once', async () => {
    const k = kept([['attachments/old.png', 'o']]);
    const up = new Uploads(k.keep);
    await Promise.all([up.add('attachments/a.png', 'a'), up.add('attachments/b.png', 'b'), up.sent('o'), up.add('attachments/c.png', 'c'), up.sent('b')]);
    expect(k.now()).toEqual([
      ['attachments/a.png', 'a'],
      ['attachments/c.png', 'c'],
    ]);
    // The same picture at two names: both go once it's sent.
    await Promise.all([up.add('attachments/d.png', 'd'), up.add('attachments/d-2.png', 'd'), up.add('attachments/d.png', 'd')]);
    expect(await up.sent('d')).toEqual([
      ['attachments/a.png', 'a'],
      ['attachments/c.png', 'c'],
    ]);
  });

  it('forgets on reload what is not on its way after all', async () => {
    // `sent` lost (the page closed first), bytes gone, and one still
    // waiting: only that one is kept, handed to the vault again.
    const k = kept([
      ['attachments/in.png', 'in'],
      ['attachments/nobytes.png', 'gone'],
      ['attachments/waits.png', 'w'],
      ['attachments/big.png', 'big'],
    ]);
    const up = new Uploads(k.keep);
    const handed: string[] = [];
    const going: string[] = [];
    const list = await up.resume(
      async (id) => (id === 'gone' ? null : new Uint8Array([1])),
      (path, _bytes) => {
        handed.push(path);
        // The vault names `in` already: nothing to send. `big` is over
        // what a picture may be.
        const id = { 'attachments/in.png': 'in', 'attachments/waits.png': 'w' }[path];
        if (id === 'w') going.push(id);
        return id;
      },
      () => going,
    );
    expect(handed).toEqual(['attachments/in.png', 'attachments/waits.png', 'attachments/big.png']);
    expect(list).toEqual([['attachments/waits.png', 'w']]);
    expect(k.now()).toEqual([['attachments/waits.png', 'w']]);
  });

  // The finding: after a reload offline, the browser that put a picture
  // in said it "hasn't reached the server yet" instead of showing its own
  // copy. The vault doesn't name it until it's on the server; this list
  // does, by the id its kept bytes go by.
  it('names the picture on its way to a path, the last put in there', async () => {
    const k = kept([
      ['attachments/a.png', 'a1'],
      ['attachments/b.png', 'b'],
    ]);
    const up = new Uploads(k.keep);
    expect(await up.going('attachments/a.png')).toBe('a1');
    expect(await up.going('attachments/c.png')).toBeUndefined();
    // Put in again there before the first went: the later one.
    void up.add('attachments/a.png', 'a2');
    expect(await up.going('attachments/a.png')).toBe('a2');
    // On the server: on its way no more (the vault names it now).
    void up.sent('b');
    expect(await up.going('attachments/b.png')).toBeUndefined();
  });
});
