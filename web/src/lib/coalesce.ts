// A job asked for again and again (the notes listed after each frame that
// changed one) run soon, but not at every ask: at once after a quiet
// while, else once when its wait is up, however many asks came between.
// The wait is a few times what the job took last, within bounds: a short
// list is drawn almost at once; ten thousand notes coming one by one are
// listed a few times a second, not ten thousand times, and listing them
// takes a small share of the time.

/// The time, and a timer: the page's, or a test's.
export type Clock = {
  now: () => number;
  set: (f: () => void, ms: number) => unknown;
  clear: (timer: unknown) => void;
};

const pageClock: Clock = {
  now: () => performance.now(),
  set: (f, ms) => setTimeout(f, ms),
  clear: (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
};

/// How long after a run the next may start: `times` what the run took,
/// at least `least` and at most `most` milliseconds.
export type Pace = { least: number; most: number; times: number };

export const LISTS: Pace = { least: 50, most: 2000, times: 4 };

export class Coalesced {
  /// When the last run ended, and what it took.
  private ended = -Infinity;
  private took = 0;
  private timer: unknown = undefined;

  constructor(
    private job: () => void,
    private pace: Pace = LISTS,
    private clock: Clock = pageClock,
  ) {}

  /// The wait after the last run.
  get wait(): number {
    return Math.min(this.pace.most, Math.max(this.pace.least, this.took * this.pace.times));
  }

  /// Whether a run waits for its time.
  get waiting(): boolean {
    return this.timer !== undefined;
  }

  /// Run now if its wait is up, else once when it is (one run for every
  /// ask until then).
  ask() {
    if (this.timer !== undefined) return;
    const due = this.ended + this.wait - this.clock.now();
    if (due <= 0) {
      this.run();
      return;
    }
    this.timer = this.clock.set(() => {
      this.timer = undefined;
      this.run();
    }, due);
  }

  /// A run waiting, dropped (the page closing what it ran on).
  cancel() {
    if (this.timer === undefined) return;
    this.clock.clear(this.timer);
    this.timer = undefined;
  }

  private run() {
    const start = this.clock.now();
    try {
      this.job();
    } finally {
      this.ended = this.clock.now();
      this.took = this.ended - start;
    }
  }
}
