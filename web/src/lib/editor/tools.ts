// Which of the toolbar's groups show, for what the caret is in: the
// desktop's sets (src/app_chrome.rs `ToolSet`), so a tool that does nothing
// where the caret is never shows, and one that does but can't just now (a
// row moved up from under the header) is greyed by Editor.svelte. The
// groups' own tools are Editor.svelte's.

/// What a block becomes (its kind, a paragraph's dropped first letter);
/// a list's in and out and a to-do's tick; the words' styles; what to put
/// in (a table, a divider, pictures and files); the block's own moves (up,
/// down, away); a table's rows and columns; a picture's place and size; a
/// file's, on a line of files (open it, copy it, take it out); and Undo,
/// the web's own, for a phone with no keys.
export type Group = 'turn' | 'list' | 'styles' | 'insert' | 'clip' | 'moves' | 'table' | 'place' | 'file' | 'undo';

/// The groups for `kind` (`PageEditor.kind_here`), in the strip's order:
/// a set's own first, then the styles, what to put in and the moves, in
/// the same order wherever they apply, so what sets share stays together
/// as the caret goes between them. `holds` (`blocks_here`) says what a mix
/// of blocks has: 1 one to turn, 2 a list item, 4 words to style.
export function toolSet(kind: string, holds: number): Group[] {
  switch (kind) {
    // The title: what to put in, at the note's caret.
    case 'title':
      return ['insert'];
    case 'words':
      return ['turn', 'styles', 'insert', 'moves', 'undo'];
    case 'list':
      return ['turn', 'list', 'styles', 'insert', 'moves', 'undo'];
    // A picture goes into a cell among its words; a table or a divider
    // would go after the table, not where the caret is.
    case 'table':
      return ['table', 'styles', 'clip', 'undo'];
    case 'code':
    case 'divider':
      return ['moves', 'insert', 'undo'];
    case 'picture':
      return ['place', 'moves', 'insert', 'undo'];
    // A file on a line of files, chosen or the caret beside it: its own,
    // the line's moves (up and down: Remove is the file's), what to put in.
    case 'file':
      return ['file', 'moves', 'insert', 'undo'];
  }
  // A selection over blocks of different kinds: what applies to any.
  const out: Group[] = [];
  if (holds & 1) out.push('turn');
  if (holds & 2) out.push('list');
  if (holds & 4) out.push('styles');
  out.push('moves', 'undo');
  return out;
}

/// The toolbar's names for its sets, as a screen reader says them: the
/// desktop's (`ToolSet::label`).
export const SET_NAMES: Record<string, string> = {
  title: 'Insert tools',
  words: 'Text tools',
  list: 'List tools',
  table: 'Table tools',
  code: 'Code block tools',
  divider: 'Divider tools',
  picture: 'Picture tools',
  file: 'File tools',
  blocks: 'Block tools',
};

/// Of a block's kinds, those a list's set offers: its own and back to a
/// paragraph (each kind's tool, pressed again, is that). A heading or a
/// quote is a paragraph's turn, there once it is one; so is its first
/// letter.
export const LIST_TURNS = new Set(['Checklist', 'Bulleted list', 'Numbered list']);
