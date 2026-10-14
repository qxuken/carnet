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

import { RETRY_FIRST, RETRY_MOST, show } from './pictures';

/// A picture on the page, not drawn yet; and asks for it, each answered
/// when the test says.
function picture() {
  const img = new on.El();
  img.isConnected = true;
  const answers: { ok: (url: string) => void; no: (e: Error) => void }[] = [];
  const ask = () => new Promise<string>((ok, no) => answers.push({ ok, no }));
  return { img, answers, ask, el: img as unknown as HTMLImageElement };
}


// The finding: a picture said not found was asked for again only when the
// notes changed, so one that reached the server later stayed "not found".
// Now it's asked again a while on, the wait doubling to 10 minutes.
describe('a picture not found', () => {
  it('is asked for again now and then, the wait doubling, until it comes', async () => {
    vi.useFakeTimers();
    const p = picture();
    show(p.el, 'attachments/late.png', p.ask);
    p.answers[0].no(new Error('not on the server'));
    await vi.advanceTimersByTimeAsync(0);
    expect(p.answers.length).toBe(1);
    await vi.advanceTimersByTimeAsync(RETRY_FIRST);
    expect(p.answers.length).toBe(2);
    p.answers[1].no(new Error('not on the server'));
    // Twice as long the next time.
    await vi.advanceTimersByTimeAsync(RETRY_FIRST);
    expect(p.answers.length).toBe(2);
    await vi.advanceTimersByTimeAsync(RETRY_FIRST);
    expect(p.answers.length).toBe(3);
    p.answers[2].no(new Error('not on the server'));
    // Never longer than the most.
    let waited = 0;
    while (p.answers.length < 9) {
      const n = p.answers.length;
      await vi.advanceTimersByTimeAsync(1000);
      waited += 1000;
      if (p.answers.length > n) {
        expect(waited).toBeLessThanOrEqual(RETRY_MOST);
        waited = 0;
        p.answers.at(-1)!.no(new Error('not on the server'));
      }
    }
    // It reached the server: shown.
    await vi.advanceTimersByTimeAsync(RETRY_MOST);
    p.answers.at(-1)!.ok('blob:late');
    await vi.advanceTimersByTimeAsync(0);
    expect(p.img.src).toBe('blob:late');
    expect(p.img.isConnected).toBe(true);
    // None waiting: no more asking.
    const n = p.answers.length;
    await vi.advanceTimersByTimeAsync(RETRY_MOST * 2);
    expect(p.answers.length).toBe(n);
    vi.useRealTimers();
  });
});
