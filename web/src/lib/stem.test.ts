import { describe, expect, it } from 'vitest';
import { pastedStem } from './stem';

describe('pastedStem', () => {
  it('names a picture by the local time, as the desktop does', () => {
    // Local time on purpose: the Date is made from local parts.
    expect(pastedStem(new Date(2026, 9, 13, 16, 43, 52))).toBe('pasted-2026-10-13-164352');
    expect(pastedStem(new Date(2026, 0, 2, 3, 4, 5))).toBe('pasted-2026-01-02-030405');
  });
});
