// Files in notes, as the desktop has them: put in with the paperclip,
// `/file`, a drop or a paste (a picture shown, any other file linked
// among the words, sync.svelte.ts `attach`); opened from a link, the
// bytes handed to the browser (open.svelte.ts); and Markdown files
// dropped on the page, which come in as notes (`importNotes`). What
// they say, and the choices they make, worked out here apart from the
// page (files.test.ts).

/// A file put in a note: a picture, its `src` for the page's picture,
/// or any other file, its `src` and its name for a link among the words
/// (the desktop's `page::insert_links` writes `[name](src)`).
export type Attached = { picture: boolean; name: string; src: string };

/// Whether a file is Markdown by its name (`.md`, `.markdown`): a note,
/// dropped on the page, rather than a file a note keeps. The desktop's
/// `store::is_markdown`.
export const isMarkdownName = (name: string) => /\.(md|markdown)$/i.test(name);

/// The kinds a browser shows in a tab of its own, safely, by a file's
/// name: a PDF in its own viewer, plain text (a `README`, source code,
/// a CSV read as words), sound, film and a photo. Never a page or a
/// drawing (`.html`, `.svg`), whose scripts would run as this page.
const SHOWN: Record<string, string> = {
  pdf: 'application/pdf',
  txt: 'text/plain',
  text: 'text/plain',
  csv: 'text/plain',
  log: 'text/plain',
  md: 'text/plain',
  mp3: 'audio/mpeg',
  m4a: 'audio/mp4',
  aac: 'audio/aac',
  wav: 'audio/wav',
  ogg: 'audio/ogg',
  flac: 'audio/flac',
  mp4: 'video/mp4',
  m4v: 'video/mp4',
  mov: 'video/quicktime',
  webm: 'video/webm',
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
};

/// The type a file named `name` is shown in a tab as; `null` for one
/// that is saved instead, to open in an app of its own, as the desktop
/// opens every file.
export function shownAs(name: string): string | null {
  const kind = /\.([^./\\]+)$/.exec(name)?.[1]?.toLowerCase();
  return (kind && SHOWN[kind]) || null;
}

/// A file's name from its path in the notes folder.
export const fileName = (path: string) => path.split('/').at(-1) ?? path;

/// What the question before a file that could run says of it: the
/// desktop's words (app_open.rs `OPEN_ASK_WHY`).
export const OPEN_ASK_WHY = 'It could run a program on this computer. Open it only if you know where it came from.';

/// The question's own line: "Open “setup.exe”?".
export const openAsk = (name: string) => `Open “${name}”?`;

/// " into “Work”", for a folder; nothing for the top level.
const into = (folder: string) => (folder ? ` into “${folder.split('/').at(-1)}”` : '');

/// What's said of notes brought in, as the desktop says it: "Imported
/// “Plan”", or "Imported 3 notes into “Work”"; nothing for none.
export function importedWords(titles: string[], folder: string): string | null {
  if (!titles.length) return null;
  return titles.length === 1 ? `Imported “${titles[0]}”${into(folder)}` : `Imported ${titles.length} notes${into(folder)}`;
}

/// What's said of files that couldn't come in as notes, the first by
/// name and why: "Couldn't import “Old.md” (and 2 more): it isn't text
/// Carnet can read (not UTF-8)"; nothing for none.
export function notImportedWords(failed: [name: string, why: string][]): string | null {
  const [first] = failed;
  if (!first) return null;
  const more = failed.length > 1 ? ` (and ${failed.length - 1} more)` : '';
  return `Couldn’t import “${first[0]}”${more}: ${first[1].replace(/\.$/, '')}`;
}

/// What's said of files that couldn't go in a note, the first by name:
/// a file too big to sync says so itself; any other couldn't be read (a
/// folder, which a browser hands over as a file it can't read).
export function refusedWords(refused: [name: string, why: string][]): string | null {
  const [first] = refused;
  if (!first) return null;
  const more = refused.length > 1 ? ` (and ${refused.length - 1} more)` : '';
  const why = /32 MB/.test(first[1]) ? first[1].replace(/\.$/, '') : `“${first[0]}” couldn’t be read: only files go in`;
  return `${why}${more}.`;
}

/// Whether a drag carries files from outside the page (the system's, a
/// download), rather than a note or a folder of the library's own.
export const carriesFiles = (types: readonly string[] | DOMStringList | undefined) => !!types && Array.from(types).includes('Files');
