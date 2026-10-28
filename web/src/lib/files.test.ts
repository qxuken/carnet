import { beforeAll, describe, expect, it } from 'vitest';
import { could_run, import_names, initSync, is_picture_name, welcome_markdown } from '../wasm/carnet_web.js';
import { carriesFiles, importedWords, isMarkdownName, notImportedWords, OPEN_ASK_WHY, openAsk, refusedWords, shownAs } from './files';

// The page itself (crates/carnet-web, built by `npm run wasm`), read from
// disk: the rules are carnet-core's, the desktop's.
beforeAll(async () => {
  const fs = await import(/* @vite-ignore */ ['node', 'fs'].join(':'));
  initSync({ module: fs.readFileSync(new URL('../wasm/carnet_web_bg.wasm', import.meta.url)) });
});

describe('files put in a note', () => {
  it('go in as pictures by a picture’s name, any other linked', () => {
    for (const p of ['photo.JPG', 'shot.png', 'drawing.svg', 'IMG_1.heic']) expect(is_picture_name(p), p).toBe(true);
    for (const f of ['plan.pdf', 'README', 'main.rs', 'setup.exe', 'page.html']) expect(is_picture_name(f), f).toBe(false);
  });

  it('ask before one that could run opens, in the desktop’s words', () => {
    for (const f of ['setup.exe', 'run.BAT', 'x.ps1', 'Tool.app', 'site.url', 'invoice.pdf.exe', 'run.exe.']) expect(could_run(f), f).toBe(true);
    for (const f of ['plan.pdf', 'README', 'main.rs', 'page.html', 'sums.xlsm', 'note.md']) expect(could_run(f), f).toBe(false);
    expect(openAsk('setup.exe')).toBe('Open “setup.exe”?');
    expect(OPEN_ASK_WHY).toBe('It could run a program on this computer. Open it only if you know where it came from.');
  });

  it('show in a tab only what a browser shows safely, and save the rest', () => {
    expect(shownAs('Plan (v2).PDF')).toBe('application/pdf');
    expect(shownAs('attachments/README.txt')).toBe('text/plain');
    expect(shownAs('talk.m4a')).toBe('audio/mp4');
    // A page and a drawing would run their scripts as this page: saved.
    for (const f of ['page.html', 'logo.svg', 'feed.xml', 'README', 'sums.xlsx', 'setup.exe']) expect(shownAs(f), f).toBe(null);
  });

  it('say what couldn’t go in, the first by name', () => {
    expect(refusedWords([])).toBe(null);
    expect(refusedWords([['Film.mov', '“Film.mov” is over 32 MB, too big to sync.']])).toBe('“Film.mov” is over 32 MB, too big to sync.');
    expect(refusedWords([['Photos', 'NotReadableError'], ['b', 'x']])).toBe('“Photos” couldn’t be read: only files go in (and 1 more).');
  });
});

describe('Markdown files dropped', () => {
  it('are notes by their names', () => {
    for (const f of ['Plan.md', 'notes.MARKDOWN']) expect(isMarkdownName(f), f).toBe(true);
    for (const f of ['plan.pdf', 'md', 'Plan.md.txt']) expect(isMarkdownName(f), f).toBe(false);
  });

  it('bring the files they link to by name', () => {
    expect(import_names('![](img/Sea%20View.png) [plan](../plan.pdf) [web](https://e.com/a.png) [[hill.jpg|300]] [other](Other.md)')).toEqual([
      'hill.jpg',
      'plan.pdf',
      'sea view.png',
    ]);
  });

  it('are said as the desktop says them', () => {
    expect(importedWords([], '')).toBe(null);
    expect(importedWords(['Plan'], '')).toBe('Imported “Plan”');
    expect(importedWords(['Plan', 'Trip'], 'Work/Trips')).toBe('Imported 2 notes into “Trips”');
    expect(notImportedWords([])).toBe(null);
    expect(notImportedWords([['Old.md', 'it isn’t text Carnet can read (not UTF-8)'], ['Big.md', 'too big']])).toBe(
      'Couldn’t import “Old.md” (and 1 more): it isn’t text Carnet can read (not UTF-8)',
    );
  });

  it('are files from outside, not a note or a folder dragged', () => {
    expect(carriesFiles(['Files'])).toBe(true);
    expect(carriesFiles(['text/plain'])).toBe(false);
    expect(carriesFiles(undefined)).toBe(false);
  });
});

describe('the welcome note', () => {
  it('is the desktop’s, its keys as this browser’s system writes them', () => {
    const pc = welcome_markdown(false);
    expect(pc.startsWith('# Welcome to Carnet\n\n')).toBe(true);
    expect(pc).toContain('press Ctrl+B for bold');
    expect(pc).toContain('| /file | any file, a tile on its own line |');
    expect(welcome_markdown(true)).toContain('press ⌘B for bold');
  });
});
