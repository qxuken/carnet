import { describe, expect, it } from 'vitest';
import { afterDelete, deleteLabel, deletedWords, folderTargets, followed, leaf, mayDrop, movedWords, namingAfter, noteTargets, parentOf, placeWords } from './folders';

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
});
