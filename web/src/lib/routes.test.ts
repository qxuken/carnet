import { describe, expect, it } from 'vitest';
import { noteAddress, readRoute } from './routes';

describe('note links in the app address', () => {
  it('opens a note with its encoded section intact, including on reload or Back', () => {
    for (const section of ['decision', 'some%20section', '%D1%82%D0%B5%D1%81%D1%82', 'a+b', 'a%23b']) {
      expect(readRoute(noteAddress('abcd', `docs/adr/0007.md#${section}`))).toEqual({ kind: 'note', key: 'abcd', editing: false, fresh: false, section });
    }
    expect(readRoute(noteAddress('abcd', 'docs/adr/0007.md'))).toEqual({ kind: 'note', key: 'abcd', editing: false, fresh: false, section: null });
  });

  it('keeps the existing editor, history, settings and library routes', () => {
    expect(readRoute('#note/abcd/edit')).toMatchObject({ kind: 'note', editing: true, fresh: false });
    expect(readRoute('#note/abcd/new')).toMatchObject({ kind: 'note', editing: true, fresh: true });
    expect(readRoute('#note/abcd/history')).toEqual({ kind: 'history', key: 'abcd' });
    expect(readRoute('#settings')).toEqual({ kind: 'settings' });
    expect(readRoute('#')).toEqual({ kind: 'list' });
    expect(readRoute('#not-an-app-route')).toEqual({ kind: 'list' });
  });
});
