import { beforeAll, describe, expect, it } from 'vitest';
import { initSync, picture_width, resized_width } from '../../wasm/carnet_web.js';

beforeAll(async () => {
  const fs = await import(/* @vite-ignore */ ['node', 'fs'].join(':'));
  initSync({ module: fs.readFileSync(new URL('../../wasm/carnet_web_bg.wasm', import.meta.url)) });
});

// What `up` in picture.ts writes when a resize is let go: -1 nothing,
// 0 no width, else the width.
describe('a picture resized', () => {
  // The finding: a handle pressed and let go, no drag, dropped `|240`
  // where the row's share drew the picture narrower (180) than its own.
  it('changes nothing pressed and let go, and keeps a width the room caps', () => {
    const [drawn] = picture_width(240, 240, 180, 180, false);
    const none = picture_width(240, 0, 180, 180, false)[0];
    expect(resized_width(drawn, drawn, 240, none)).toBe(-1);
    expect(resized_width(150, 180, 240, none)).toBe(180);
  });

  it('forgets the width only back at its own, drawn so with none', () => {
    expect(resized_width(250, 300.6, 300, 300)).toBe(0);
    expect(resized_width(300, 250.4, 300, 300)).toBe(250);
    // At an edge, 45% of the text with none: its own width is written.
    const none = picture_width(800, 0, 600, 600, true)[0];
    expect(resized_width(270, 800, 800, none)).toBe(800);
  });
});
