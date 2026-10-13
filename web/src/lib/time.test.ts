import { describe, expect, it } from 'vitest';
import { ago, dateWords, left, plausible } from './time';

// A server's times are said as times, or as unknown: never "Invalid Date",
// whatever number comes (as the desktop's `epoch_time`).
describe('times from a server', () => {
  it('says a real day and a real while', () => {
    expect(dateWords(1791763200)).toMatch(/2026/);
    expect(ago(Date.now() / 1000 - 120)).toBe('2 minutes ago');
    expect(left(Date.now() / 1000 + 3 * 86400)).toBe('good for 3 more days');
  });
  it('says an impossible one as unknown', () => {
    for (const t of [Number.MAX_SAFE_INTEGER, 1.8446744073709552e19, Infinity, NaN, -5]) {
      expect(plausible(t)).toBe(false);
      expect(dateWords(t)).toBeNull();
      expect(ago(t)).toBe('at an unknown time');
      expect(left(t)).toBe('good for a long while');
    }
  });
});
