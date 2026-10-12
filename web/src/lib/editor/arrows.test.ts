import { describe, expect, it } from 'vitest';
import { across, onRow, vertical, type Lines } from './arrows';

/// Lines of these kinds, each with these words (a picture's or a
/// divider's line holds one letter, as the page gives CodeMirror).
function page(lines: [string, string][]): Lines {
  const starts: number[] = [];
  let at = 0;
  for (const [, text] of lines) {
    starts.push(at);
    at += text.length + 1;
  }
  return {
    count: lines.length,
    line: (n) => ({ from: starts[n - 1], to: starts[n - 1] + lines[n - 1][1].length }),
    kind: (n) => lines[n - 1]?.[0],
    lineAt: (pos) => {
      let n = 1;
      while (n < starts.length && starts[n] <= pos) n++;
      return n;
    },
  };
}

// 1 title 0..5, 2 text 6..11, 3 picture 12..13, 4 h2 14..21, 5 divider 22..23, 6 text 24..28
const note = page([
  ['title', 'Title'],
  ['text', 'Words'],
  ['image', '￼'],
  ['h2', 'Heading'],
  ['divider', '￼'],
  ['text', 'After'],
]);
const at = (n: number) => ({ anchor: n, head: n });

describe('across a picture', () => {
  it('chooses it with one press and is past it with the next', () => {
    const first = across(note, at(11), true);
    expect(first).toEqual({ anchor: 12, head: 13 });
    expect(across(note, first!, true)).toEqual(at(14));
    const back = across(note, at(14), false);
    expect(back).toEqual({ anchor: 13, head: 12 });
    expect(across(note, back!, false)).toEqual(at(11));
  });
  it('passes it from a caret on its line, which shows it chosen', () => {
    expect(across(note, at(12), true)).toEqual(at(14));
    expect(across(note, at(12), false)).toEqual(at(11));
  });
  it('leaves the words to CodeMirror', () => {
    expect(across(note, at(8), true)).toBeNull();
    expect(across(note, at(14), true)).toBeNull();
  });
  it('crosses a divider the same', () => {
    expect(across(note, at(21), true)).toEqual({ anchor: 22, head: 23 });
    expect(across(note, { anchor: 22, head: 23 }, true)).toEqual(at(24));
  });
});

describe('vertical', () => {
  it('goes one line at a time from and to a heading, measured', () => {
    expect(vertical(note, at(16), false, false)).toEqual({ at: 12 });
    expect(vertical(note, at(8), true, false)).toEqual({ at: 12 });
    expect(vertical(note, at(25), false, false)).toEqual({ at: 23 });
    expect(vertical(note, at(2), true, false)).toEqual({ line: 2, leaving: true });
    expect(vertical(note, at(8), false, false)).toEqual({ line: 1, leaving: true });
  });
  it('leaves a picture, or a chosen one, for the next line', () => {
    expect(vertical(note, at(12), true, false)).toEqual({ line: 4, leaving: false });
    expect(vertical(note, { anchor: 12, head: 13 }, false, false)).toEqual({ line: 2, leaving: false });
  });
  it('leaves a selection of words to CodeMirror, and extends one', () => {
    expect(vertical(note, { anchor: 6, head: 9 }, true, false)).toBeNull();
    expect(vertical(note, { anchor: 6, head: 9 }, false, true)).toEqual({ line: 1, leaving: true });
  });
  it('stops at the note’s ends', () => {
    expect(vertical(note, at(2), false, false)).toBeNull();
    expect(vertical(note, at(26), true, false)).toBeNull();
  });
});

describe('onRow', () => {
  it('takes the caret by its middle', () => {
    expect(onRow({ top: 40, bottom: 60 }, { top: 38, bottom: 62 })).toBe(true);
    expect(onRow({ top: 70, bottom: 90 }, { top: 38, bottom: 62 })).toBe(false);
  });
});
