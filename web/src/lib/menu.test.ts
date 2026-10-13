import { describe, expect, it } from 'vitest';
import { placeMenu } from './menu';

describe('placeMenu', () => {
  const at = (left: number, top: number, w = 30, h = 30) => ({ left, top, right: left + w, bottom: top + h });

  it('opens under its button, right edges together', () => {
    expect(placeMenu(at(300, 100), 200, 150, 1000, 800)).toEqual({ left: 130, top: 134, maxHeight: 658 });
  });

  it('stays inside a phone’s width', () => {
    expect(placeMenu(at(10, 100), 220, 150, 375, 700).left).toBe(8);
    expect(placeMenu(at(340, 100, 30), 220, 150, 375, 700).left).toBe(147);
  });

  it('opens above when there is more room there', () => {
    const p = placeMenu(at(300, 700), 200, 300, 1000, 760);
    expect(p.top).toBe(396);
    expect(p.maxHeight).toBe(688);
  });

  it('scrolls when it is taller than any room', () => {
    const p = placeMenu(at(300, 300), 200, 2000, 1000, 640);
    expect(p.top).toBe(334);
    expect(p.maxHeight).toBe(298);
  });
});
