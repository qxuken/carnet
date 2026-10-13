import { describe, expect, it } from 'vitest';
import { across, onRow, sideOn, vertical, type Lines, type Row } from './arrows';

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
  it('goes one line at a time onto words, measured', () => {
    expect(vertical(note, at(2), true, false)).toEqual({ line: 2, leaving: true });
    expect(vertical(note, at(8), false, false)).toEqual({ line: 1, leaving: true });
  });
  it('chooses a divider or a picture whole, as ← and → do, and the next is past it', () => {
    // ↑ from the heading, ↓ from the words: the picture, each from its side.
    expect(vertical(note, at(16), false, false)).toEqual({ sel: { anchor: 13, head: 12 } });
    expect(vertical(note, at(16), false, false)).toEqual({ sel: across(note, at(14), false) });
    expect(vertical(note, at(8), true, false)).toEqual({ sel: { anchor: 12, head: 13 } });
    // ↓ from the heading, ↑ from below: the divider.
    expect(vertical(note, at(16), true, false)).toEqual({ sel: across(note, at(21), true) });
    expect(vertical(note, at(25), false, false)).toEqual({ sel: { anchor: 23, head: 22 } });
    // Chosen, the next press is past it, onto the line beyond.
    expect(vertical(note, { anchor: 22, head: 23 }, true, false)).toEqual({ line: 6, leaving: false });
    expect(vertical(note, { anchor: 23, head: 22 }, false, false)).toEqual({ line: 4, leaving: false });
    // Shift extends to its near edge, not round it.
    expect(vertical(note, at(25), false, true)).toEqual({ at: 23 });
    expect(vertical(note, at(16), true, true)).toEqual({ at: 22 });
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

// The finding: ↑ from the start of a wrapped row skipped a row — a
// heading's first, a paragraph's third. A paragraph wrapped every 10
// letters, rows 20 high: place 20 ends row 1 (from its left side) and
// starts row 2 (from its right).
describe('sideOn', () => {
  const rowOf = (n: number): Row => ({ top: n * 20, bottom: n * 20 + 20 });
  const coords = (pos: number, side: 1 | -1) => rowOf(pos % 10 === 0 && side < 0 && pos > 0 ? pos / 10 - 1 : Math.floor(pos / 10));
  it('keeps a caret at the start of a row on that row', () => {
    expect(sideOn(20, rowOf(2), coords)).toBe(1);
  });
  it('and one at the end of a row on that one', () => {
    expect(sideOn(20, rowOf(1), coords)).toBe(-1);
  });
  it('either way in the middle of a row', () => {
    expect(onRow(coords(25, sideOn(25, rowOf(2), coords)), rowOf(2))).toBe(true);
  });
});

// A code block's lines are its words alone (its fences are never among
// the editor's lines, editor.rs `relay`): ↓ from its last line is the
// next block's line, ↑ from its first the one before, measured as any.
describe('vertical by code', () => {
  // 1 title 0..1, 2 text 2..7, 3 code 8..14, 4 code 15..21, 5 text 22..27
  const code = page([
    ['title', 'T'],
    ['text', 'Above'],
    ['code', 'let x;'],
    ['code', 'let y;'],
    ['text', 'Below'],
  ]);
  it('leaves the code from its last line down and its first line up', () => {
    expect(vertical(code, at(21), true, false)).toEqual({ line: 5, leaving: true });
    expect(vertical(code, at(8), false, false)).toEqual({ line: 2, leaving: true });
  });
  it('comes into it at its last line up and its first line down', () => {
    expect(vertical(code, at(22), false, false)).toEqual({ line: 4, leaving: true });
    expect(vertical(code, at(2), true, false)).toEqual({ line: 3, leaving: true });
  });
});
