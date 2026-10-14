import { describe, expect, it } from 'vitest';
import library from './screens/Library.svelte?raw';

/// A rule's declarations in `css`, by its selector, inside `within` (an
/// at-rule's head) when given.
function rule(css: string, selector: string, within?: string): string {
  const scope = within ? css.slice(css.indexOf(within)) : css;
  const at = scope.indexOf(`${selector} {`);
  expect(at, `${selector} in ${within ?? 'the styles'}`).toBeGreaterThanOrEqual(0);
  return scope.slice(at, scope.indexOf('}', at));
}

const px = (decls: string, prop: string) => Number(new RegExp(`${prop}:\\s*(-?\\d+)px`).exec(decls)?.[1]);

describe('the folder chevron', () => {
  // The finding: on a phone the chevron was a 22×26 target. On a touch
  // screen or a phone's width its reach is 44×44 at least, inside a row a
  // finger's height, the chevron itself (and so the folder's icon) where
  // it was.
  it('is a 44px target on a touch screen, and moves nothing', () => {
    const touch = '@media (pointer: coarse), (max-width: 600px)';
    const chevron = rule(library, '.chevron');
    const [w, h] = [px(chevron, 'width'), px(chevron, 'height')];
    const reach = rule(library, '.chevron::before', touch);
    const [dy, dx] = /inset:\s*-(\d+)px\s+-(\d+)px/.exec(reach)!.slice(1).map(Number);
    expect(w + 2 * dx).toBeGreaterThanOrEqual(44);
    expect(h + 2 * dy).toBeGreaterThanOrEqual(44);
    expect(px(rule(library, '.frow', touch), 'min-height')).toBeGreaterThanOrEqual(h + 2 * dy);
    // Past its right edge no further than the folder's icon starts (the
    // name's button's own padding, 4px, and the icon's 16): never over
    // the name.
    expect(dx).toBeLessThanOrEqual(4 + 16);
    expect(rule(library, '.chevron', touch)).not.toMatch(/width|margin|padding/);
  });
});
