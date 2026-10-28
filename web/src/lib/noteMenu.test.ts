import { describe, expect, it } from 'vitest';
import { noteAddress } from './noteMenu';

describe('a note in a tab of its own', () => {
  it('opens by its address, the page’s own kept', () => {
    expect(noteAddress('ab12', { origin: 'https://notes.example', pathname: '/', search: '' })).toBe('https://notes.example/#note/ab12');
    expect(noteAddress('ab12', { origin: 'http://localhost:5173', pathname: '/app/', search: '?x=1' })).toBe('http://localhost:5173/app/?x=1#note/ab12');
  });
});
