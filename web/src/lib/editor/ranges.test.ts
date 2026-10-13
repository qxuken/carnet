import { describe, expect, it } from 'vitest';
import { markRange } from './ranges';

describe('markRange', () => {
  it('draws a run where it is on its line', () => {
    expect(markRange(10, 8, 2, 5)).toEqual([12, 15]);
    expect(markRange(10, 8, 2, 20)).toEqual([12, 18]);
  });
  // The finding: a coloured word selected whole and cut by Ctrl+K left
  // its run past the line's new end, and CodeMirror's "Mark decorations
  // may not be empty" took every style away until the note was opened
  // again.
  it('gives nothing for a run past the line, or of no length', () => {
    expect(markRange(10, 4, 4, 9)).toBeNull();
    expect(markRange(10, 4, 6, 9)).toBeNull();
    expect(markRange(10, 8, 3, 3)).toBeNull();
    expect(markRange(10, 0, 0, 5)).toBeNull();
  });
});
