import { describe, expect, it, vi } from 'vitest';
import { focusNow } from './focus';

// The finding: the Link sheet's Address field was not focused (the
// browser blocks `autofocus` while the editor has the focus), so the
// address typed replaced the chosen word in the note.
describe('focusNow', () => {
  it('focuses the field and selects its words', () => {
    const node = { focus: vi.fn(), select: vi.fn() };
    focusNow(node as unknown as HTMLInputElement);
    expect(node.focus).toHaveBeenCalledWith({ preventScroll: true });
    expect(node.select).toHaveBeenCalled();
  });
});
