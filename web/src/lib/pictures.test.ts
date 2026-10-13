import { describe, expect, it, vi } from 'vitest';

// No DOM here: as much of one as a picture's place takes (a box put in
// the picture's place, and words in it), and the window's events.
const on = vi.hoisted(() => {
  const heard = new Map<string, (() => void)[]>();
  class El {
    isConnected = false;
    className = '';
    title = '';
    textContent = '';
    innerHTML = '';
    src = '';
    alt = '';
    style: Record<string, string> = {};
    dataset: Record<string, string> = {};
    attrs = new Map<string, string>();
    children: El[] = [];
    setAttribute(k: string, v: string) {
      this.attrs.set(k, v);
    }
    getAttribute(k: string) {
      return this.attrs.get(k) ?? null;
    }
    addEventListener() {}
    replaceWith(other: El) {
      other.isConnected = this.isConnected;
      this.isConnected = false;
    }
    append(...c: El[]) {
      this.children.push(...c);
    }
    replaceChildren(...c: El[]) {
      this.children = c;
    }
  }
  const g = globalThis as unknown as Record<string, unknown>;
  const made: El[] = [];
  g.document = {
    createElement: () => {
      const el = new El();
      made.push(el);
      return el;
    },
  };
  g.addEventListener = (name: string, f: () => void) => heard.set(name, [...(heard.get(name) ?? []), f]);
  return { El, made, said: (name: string) => (heard.get(name) ?? []).forEach((f) => f()), sync: { pending: false as boolean, vaultPending: (): boolean => false } };
});

vi.mock('./sync.svelte', () => {
  on.sync.vaultPending = () => on.sync.pending;
  return { NOTES_CHANGED: 'notes-changed', NOT_ON_SERVER: 'not on the server', sync: on.sync };
});

import { DOWNLOADING, SAY_COMING, show } from './pictures';

/// A picture on the page, not drawn yet; and asks for it, each answered
/// when the test says.
function picture() {
  const img = new on.El();
  img.isConnected = true;
  const answers: { ok: (url: string) => void; no: (e: Error) => void }[] = [];
  const ask = () => new Promise<string>((ok, no) => answers.push({ ok, no }));
  return { img, answers, ask, el: img as unknown as HTMLImageElement };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe('a picture not yet on the server', () => {
  // The finding: one put in elsewhere kept its "hasn't reached the
  // server" card in an open note after it got there. The vault came to
  // name it while it was being asked for, and the change that says so
  // came before the card was up to be asked again.
  it('is asked for again when the notes changed while it was asked for', async () => {
    const p = picture();
    show(p.el, 'attachments/a.png', p.ask);
    on.said('notes-changed');
    p.answers[0].no(new Error('not in the vault'));
    await tick();
    expect(p.answers.length).toBe(2);
    p.answers[1].ok('blob:a');
    await tick();
    expect(p.img.src).toBe('blob:a');
    expect(p.img.isConnected).toBe(true);
  });

  it('says so in its place until the notes change, then comes', async () => {
    const p = picture();
    show(p.el, 'attachments/b.png', p.ask);
    p.answers[0].no(new Error('not in the vault'));
    await tick();
    expect(p.answers.length).toBe(1);
    expect(p.img.isConnected).toBe(false);
    on.said('notes-changed');
    p.answers[1].ok('blob:b');
    await tick();
    expect(p.img.src).toBe('blob:b');
    expect(p.img.isConnected).toBe(true);
  });
});

/// What the box in a picture's place says now ('' while it's a quiet
/// box), the picture shown at `p` asked last.
function says(from: number) {
  const place = on.made.slice(from).find((e) => e.className === 'picture-card');
  return place?.children[1]?.textContent ?? '';
}

describe('a picture on its way', () => {
  // The finding: on a device taking in the notes by sync, a note's picture
  // said "Picture not found" until it came.
  it('says it is downloading while it comes, then shows', async () => {
    vi.useFakeTimers();
    const p = picture();
    const from = on.made.length;
    show(p.el, 'attachments/c.png', p.ask);
    expect(says(from)).toBe('');
    vi.advanceTimersByTime(SAY_COMING + 1);
    expect(says(from)).toBe(DOWNLOADING);
    p.answers[0].ok('blob:c');
    await vi.runAllTimersAsync();
    expect(p.img.src).toBe('blob:c');
    expect(p.img.isConnected).toBe(true);
    vi.useRealTimers();
  });

  it('says nothing when it comes at once', async () => {
    vi.useFakeTimers();
    const p = picture();
    const from = on.made.length;
    show(p.el, 'attachments/d.png', p.ask);
    p.answers[0].ok('blob:d');
    await vi.runAllTimersAsync();
    expect(says(from)).toBe('');
    expect(p.img.isConnected).toBe(true);
    vi.useRealTimers();
  });

  it('is downloading until the account’s vault has come, and not found only when the server hasn’t it', async () => {
    on.sync.pending = true;
    let p = picture();
    let from = on.made.length;
    show(p.el, 'attachments/e.png', p.ask);
    p.answers[0].no(new Error('not in the vault'));
    await tick();
    expect(says(from)).toBe(DOWNLOADING);
    on.sync.pending = false;
    p = picture();
    from = on.made.length;
    show(p.el, 'attachments/f.png', p.ask);
    p.answers[0].no(new Error('not in the vault'));
    await tick();
    expect(says(from)).toBe('This picture hasn’t reached the server yet');
    p = picture();
    from = on.made.length;
    show(p.el, 'attachments/g.png', p.ask);
    p.answers[0].no(new Error('not on the server'));
    await tick();
    expect(says(from)).toBe('Picture not found');
  });
});
