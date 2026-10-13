import { describe, expect, it } from 'vitest';
import {
  afterDelete,
  arrow,
  deleteLabel,
  deletedWords,
  fold,
  foldsFollow,
  folderTargets,
  followed,
  hidden,
  inSight,
  keepFolded,
  leaf,
  mayDrop,
  movedWords,
  namingAfter,
  noteTargets,
  parentOf,
  placeWords,
  readFolded,
  shownAfterFold,
  unfoldTo,
  withSubfolders,
} from './folders';

describe('folders', () => {
  it('names a folder by its last part, and the one it is in', () => {
    expect(leaf('Work/Q4')).toBe('Q4');
    expect(leaf('Work')).toBe('Work');
    expect(parentOf('Work/Q4/Notes')).toBe('Work/Q4');
    expect(parentOf('Work')).toBe('');
  });

  it('says where things went as the desktop does', () => {
    expect(placeWords('Work/Q4')).toBe('to “Q4”');
    expect(placeWords('')).toBe('out of its folder');
    expect(placeWords('', 'their')).toBe('out of their folders');
    expect(movedWords('Plan', 'Work')).toBe('Moved “Plan” to “Work”');
    expect(movedWords('Q4', '')).toBe('Moved “Q4” out of its folder');
  });

  it('lists where a note may go: out of its folder, then every other', () => {
    const folders = ['Home', 'Work', 'Work/Q4'];
    expect(noteTargets(folders, 'Work')).toEqual([
      { to: '', label: 'Out of its folder' },
      { to: 'Home', label: 'Home' },
      { to: 'Work/Q4', label: 'Work/Q4' },
    ]);
    expect(noteTargets(folders, '').map((t) => t.to)).toEqual(folders);
    expect(noteTargets([], '')).toEqual([]);
  });

  it('words the folders the vault lets a folder go to', () => {
    expect(folderTargets(['', 'Home'])).toEqual([
      { to: '', label: 'To the top' },
      { to: 'Home', label: 'Home' },
    ]);
  });

  it('says what deleting a folder does, and did', () => {
    expect(deleteLabel(true)).toBe('Delete folder');
    expect(deleteLabel(false)).toBe('Move folder to Deleted');
    expect(deletedWords('Work/Old', true, 0)).toBe('Deleted the folder “Old”');
    expect(deletedWords('Work', false, 1)).toBe('Moved “Work” and its note to Deleted');
    expect(deletedWords('Work', false, 3)).toBe('Moved “Work” and its 3 notes to Deleted');
    expect(deletedWords('Work', false, 0)).toBe('Moved “Work” to Deleted');
  });

  it('follows the folder shown when it, or one it is in, moves or goes', () => {
    expect(followed('Work', 'Work', 'Job')).toBe('Job');
    expect(followed('Work/Q4', 'Work', 'Ideas/Work')).toBe('Ideas/Work/Q4');
    expect(followed('Workshop', 'Work', 'Job')).toBe('Workshop');
    expect(followed(null, 'Work', 'Job')).toBe(null);
    expect(afterDelete('Work/Q4', 'Work')).toBe(null);
    expect(afterDelete('Home', 'Work')).toBe('Home');
    expect(afterDelete('Workshop', 'Work')).toBe('Workshop');
  });

  it('lets a drag go only where it may', () => {
    expect(mayDrop(null, '')).toBe(false);
    expect(mayDrop({ note: 'k', from: 'Work' }, 'Work')).toBe(false);
    expect(mayDrop({ note: 'k', from: 'Work' }, '')).toBe(true);
    expect(mayDrop({ note: 'k', from: '' }, '')).toBe(false);
    // The vault's list for "Work": never itself or inside it.
    const d = { folder: 'Work', targets: ['Home'] };
    expect(mayDrop(d, 'Home')).toBe(true);
    expect(mayDrop(d, 'Work')).toBe(false);
    expect(mayDrop(d, 'Work/Q4')).toBe(false);
    expect(mayDrop(d, '')).toBe(false);
  });

  it('puts a new folder’s field after its parent and what is inside it', () => {
    const folders = ['Home', 'Work', 'Work/Q4', 'Zoo'];
    expect(namingAfter(folders, '')).toBe(3);
    expect(namingAfter(folders, 'Work')).toBe(2);
    expect(namingAfter(folders, 'Home')).toBe(0);
    expect(namingAfter([], '')).toBe(-1);
  });

  const tree = ['Home', 'Work', 'Work/Q4', 'Work/Q4/Notes', 'Work/Old', 'Workshop'];

  it('hides the folders inside a folded one, and only those', () => {
    const folded = new Set(['Work']);
    expect(hidden('Work', folded)).toBe(false);
    expect(hidden('Work/Q4', folded)).toBe(true);
    expect(hidden('Work/Q4/Notes', folded)).toBe(true);
    expect(hidden('Workshop', folded)).toBe(false);
    expect(inSight(tree, folded)).toEqual(['Home', 'Work', 'Workshop']);
    expect(inSight(tree, new Set(['Work/Q4']))).toEqual(['Home', 'Work', 'Work/Q4', 'Work/Old', 'Workshop']);
    expect(inSight(tree, new Set())).toEqual(tree);
    expect([...withSubfolders(tree)].sort()).toEqual(['Work', 'Work/Q4']);
  });

  it('folds and unfolds a folder, and with Alt every folder inside it', () => {
    const one = fold(new Set(), tree, 'Work', false);
    expect([...one]).toEqual(['Work']);
    // Nothing inside: nothing to fold.
    expect(fold(new Set(), tree, 'Home', false).size).toBe(0);
    const deep = fold(new Set(), tree, 'Work', false, true);
    expect([...deep].sort()).toEqual(['Work', 'Work/Q4']);
    expect(fold(deep, tree, 'Work', true).has('Work/Q4')).toBe(true);
    expect(fold(deep, tree, 'Work', true, true).size).toBe(0);
    // The set given is left as it was.
    expect(one.size).toBe(1);
  });

  it('keeps the folder shown in sight', () => {
    const folded = new Set(['Work', 'Work/Q4', 'Home']);
    expect([...unfoldTo(folded, 'Work/Q4/Notes')]).toEqual(['Home']);
    expect([...unfoldTo(folded, 'Work')].sort()).toEqual(['Home', 'Work', 'Work/Q4']);
    expect(shownAfterFold('Work/Q4/Notes', 'Work')).toBe('Work');
    expect(shownAfterFold('Work', 'Work')).toBe('Work');
    expect(shownAfterFold('Workshop', 'Work')).toBe('Workshop');
    expect(shownAfterFold(null, 'Work')).toBe(null);
  });

  it('keeps a folder folded when it is renamed or moved', () => {
    const folded = new Set(['Work', 'Work/Q4', 'Workshop']);
    expect([...foldsFollow(folded, 'Work', 'Ideas/Job')].sort()).toEqual(['Ideas/Job', 'Ideas/Job/Q4', 'Workshop']);
  });

  it('walks the tree with the left and right arrows', () => {
    const none = new Set<string>();
    expect(arrow('ArrowLeft', false, 'Work', tree, none)).toEqual({ fold: true, deep: false });
    expect(arrow('ArrowLeft', false, 'Work', tree, new Set(['Work']))).toBe(null);
    expect(arrow('ArrowLeft', false, 'Work/Old', tree, none)).toEqual({ go: 'Work' });
    expect(arrow('ArrowLeft', true, 'Work', tree, new Set(['Work']))).toEqual({ fold: true, deep: true });
    expect(arrow('ArrowRight', false, 'Work', tree, new Set(['Work']))).toEqual({ fold: false, deep: false });
    expect(arrow('ArrowRight', false, 'Work', tree, none)).toEqual({ go: 'Work/Q4' });
    expect(arrow('ArrowRight', true, 'Work', tree, none)).toEqual({ fold: false, deep: true });
    expect(arrow('ArrowRight', false, 'Home', tree, none)).toBe(null);
  });

  it('keeps what is folded in the browser, and does without storage', () => {
    const kept = new Map<string, string>();
    const store = {
      getItem: (k: string) => kept.get(k) ?? null,
      setItem: (k: string, v: string) => void kept.set(k, v),
      removeItem: (k: string) => void kept.delete(k),
    };
    expect(readFolded(store).size).toBe(0);
    keepFolded(new Set(['Work/Q4', 'Work']), store);
    expect([...readFolded(store)]).toEqual(['Work', 'Work/Q4']);
    keepFolded(new Set(), store);
    expect(kept.size).toBe(0);
    // Something else under the key, or storage that throws: none folded,
    // and nothing thrown.
    kept.set('carnet.foldedFolders', '{"no":1}');
    expect(readFolded(store).size).toBe(0);
    kept.set('carnet.foldedFolders', 'not json');
    expect(readFolded(store).size).toBe(0);
    const refused = {
      getItem: () => {
        throw new Error('denied');
      },
      setItem: () => {
        throw new Error('denied');
      },
      removeItem: () => {
        throw new Error('denied');
      },
    };
    expect(readFolded(refused).size).toBe(0);
    expect(() => keepFolded(new Set(['Work']), refused)).not.toThrow();
  });
});
