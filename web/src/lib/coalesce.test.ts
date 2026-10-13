import { describe, expect, it } from 'vitest';
import { type Clock, Coalesced } from './coalesce';

/// A clock moved by hand, its timers run as it passes them.
function hand() {
  let t = 0;
  let timers: { at: number; f: () => void; id: number }[] = [];
  let next = 0;
  const clock: Clock = {
    now: () => t,
    set: (f, ms) => {
      timers.push({ at: t + ms, f, id: ++next });
      return next;
    },
    clear: (id) => {
      timers = timers.filter((x) => x.id !== id);
    },
  };
  const pass = (ms: number) => {
    const end = t + ms;
    for (;;) {
      timers.sort((a, b) => a.at - b.at);
      const due = timers[0];
      if (!due || due.at > end) break;
      timers.shift();
      t = Math.max(t, due.at);
      due.f();
    }
    t = Math.max(t, end);
  };
  return { clock, pass, spend: (ms: number) => (t += ms) };
}

describe('Coalesced', () => {
  it('runs at once after a quiet while, then once for every ask in its wait', () => {
    const h = hand();
    let runs = 0;
    const c = new Coalesced(() => runs++, { least: 50, most: 2000, times: 4 }, h.clock);
    c.ask();
    expect(runs).toBe(1);
    for (let i = 0; i < 40; i++) {
      c.ask();
      h.pass(1);
    }
    // 40 ms of asks: all in the 50 ms wait, one run at its end.
    expect(runs).toBe(1);
    expect(c.waiting).toBe(true);
    h.pass(10);
    expect(runs).toBe(2);
    expect(c.waiting).toBe(false);
    // Quiet past the wait: at once again.
    h.pass(100);
    c.ask();
    expect(runs).toBe(3);
  });

  it('waits a few times what the job took, so a long job takes a small share', () => {
    const h = hand();
    let runs = 0;
    // A list of ten thousand notes: 100 ms each time.
    const c = new Coalesced(() => {
      runs++;
      h.spend(100);
    }, { least: 50, most: 2000, times: 4 }, h.clock);
    // Ten thousand frames over 30 s, each asking.
    for (let i = 0; i < 10_000; i++) {
      c.ask();
      h.pass(3);
    }
    h.pass(1000);
    // One run for each 500 ms (100 run, 400 waiting), not one a frame.
    expect(runs).toBeGreaterThan(50);
    expect(runs).toBeLessThan(80);
    expect(c.wait).toBe(400);
  });

  it('never waits past its most, and a cancelled run does not come', () => {
    const h = hand();
    let runs = 0;
    const c = new Coalesced(() => {
      runs++;
      h.spend(5000);
    }, { least: 50, most: 2000, times: 4 }, h.clock);
    c.ask();
    c.ask();
    expect(c.wait).toBe(2000);
    h.pass(2000);
    expect(runs).toBe(2);
    c.ask();
    c.cancel();
    h.pass(10_000);
    expect(runs).toBe(2);
  });

  it('runs on after a job that throws', () => {
    const h = hand();
    let runs = 0;
    const c = new Coalesced(() => {
      runs++;
      throw new Error('drawn wrong');
    }, { least: 50, most: 2000, times: 4 }, h.clock);
    expect(() => c.ask()).toThrow();
    c.ask();
    expect(() => h.pass(60)).toThrow();
    expect(runs).toBe(2);
    expect(c.waiting).toBe(false);
  });
});
