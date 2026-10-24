import { describe, expect, it } from 'vitest';
import { shownNotes, stepIn, stepKey, stepNote, type Placed } from './order';

const note = (key: string, folder: string, modified: number, more: Partial<Placed> = {}): Placed => ({ key, folder, modified, trashed: false, pinned: false, ...more });

describe('the notes the library shows', () => {
  const notes = [
    note('a', '', 1),
    note('b', 'Work', 5),
    note('c', 'Work', 3, { pinned: true }),
    note('d', 'Work/Old', 9),
    note('e', 'Work', 7, { trashed: true }),
  ];

  it("are a folder's own, pinned first, the newest next", () => {
    expect(shownNotes(notes, null, 'Work', false).map((n) => n.key)).toEqual(['c', 'b']);
    expect(shownNotes(notes, null, null, false).map((n) => n.key)).toEqual(['c', 'd', 'b', 'a']);
  });

  it("are a search's matches from every folder, or what was deleted", () => {
    expect(shownNotes(notes, new Set(['a', 'd', 'e']), 'Work', false).map((n) => n.key)).toEqual(['d', 'a']);
    expect(shownNotes(notes, null, null, true).map((n) => n.key)).toEqual(['e']);
  });
});

describe('the next and previous note', () => {
  // The desktop's `step_in`, case by case.
  it('steps as the desktop does, round the list only when asked', () => {
    expect(stepIn(3, 1, false, false)).toBe(2);
    expect(stepIn(3, 1, true, false)).toBe(0);
    expect(stepIn(3, 2, false, false)).toBeNull();
    expect(stepIn(3, 0, true, false)).toBeNull();
    expect(stepIn(3, 2, false, true)).toBe(0);
    expect(stepIn(3, 0, true, true)).toBe(2);
    expect(stepIn(3, null, true, false)).toBe(0);
    expect(stepIn(3, null, true, true)).toBe(2);
    expect(stepIn(1, 0, false, true)).toBeNull();
    expect(stepIn(0, null, false, false)).toBeNull();
  });

  it('goes along the order shown and stops at its ends', () => {
    const keys = ['c', 'b', 'a'];
    expect(stepNote(keys, 'c', false)).toBe('b');
    expect(stepNote(keys, 'b', true)).toBe('c');
    expect(stepNote(keys, 'a', false)).toBeNull();
    expect(stepNote(keys, 'c', true)).toBeNull();
    // From none open, or one the list doesn't show: its first.
    expect(stepNote(keys, null, false)).toBe('c');
    expect(stepNote(keys, 'z', true)).toBe('c');
    expect(stepNote([], null, false)).toBeNull();
  });

  const key = (key: string, mods: string) => ({ key, altKey: mods.includes('a'), shiftKey: mods.includes('s'), ctrlKey: mods.includes('c'), metaKey: mods.includes('m') });

  // Not Alt+↑ alone (a block moved), nor with Shift (a table's row).
  it('is Ctrl+Alt+↑ and ↓, ⌘⌥ on a Mac, nothing else held', () => {
    expect(stepKey(key('ArrowUp', 'ca'), false)).toBe('up');
    expect(stepKey(key('ArrowDown', 'ca'), false)).toBe('down');
    expect(stepKey(key('ArrowDown', 'ma'), true)).toBe('down');
    expect(stepKey(key('ArrowDown', 'ma'), false)).toBeNull();
    expect(stepKey(key('ArrowDown', 'ca'), true)).toBeNull();
    expect(stepKey(key('ArrowUp', 'a'), false)).toBeNull();
    expect(stepKey(key('ArrowUp', 'cas'), false)).toBeNull();
    expect(stepKey(key('ArrowLeft', 'ca'), false)).toBeNull();
  });
});
