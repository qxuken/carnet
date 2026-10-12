// The editor in the note's own look: the reading view's type and colours
// (styles.css `.note`), so going from reading to writing moves nothing.
// Each line is drawn from what the page says it is (page.ts `Look`).

import { EditorView } from '@codemirror/view';

export const theme = EditorView.theme({
  '&': { color: 'var(--ink)', backgroundColor: 'transparent', font: '400 17px/1.65 var(--sans)' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'inherit', lineHeight: 'inherit', overflow: 'visible' },
  '.cm-content': { padding: '0', caretColor: 'var(--accent)' },
  // Each line a block of its own beside a picture at an edge, as the
  // reader's paragraphs are (a column, not wrapped round it).
  '.cm-line': { padding: '0', display: 'flow-root' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)', borderLeftWidth: '2px' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection': { backgroundColor: 'var(--selection) !important' },
  // A block's space above, as the reader spaces them; items close.
  '.cm-block': { paddingTop: '12px !important' },
  '.cm-bullet.cm-block, .cm-number.cm-block, .cm-todo.cm-block': { paddingTop: '4px !important' },
  '.cm-title': { font: '600 32px/1.25 var(--serif)', paddingBottom: '6px !important' },
  // A heading's space above, the desktop's: its own and the block's
  // before it.
  '.cm-h1': { font: '600 26px/1.3 var(--serif)', paddingTop: '34px !important' },
  '.cm-h2': { font: '600 24px/1.3 var(--serif)', paddingTop: '28px !important' },
  '.cm-h3': { font: '600 19px/1.35 var(--serif)', paddingTop: '22px !important' },
  '.cm-ph': { color: 'var(--faint)', pointerEvents: 'none' },
  // A list item: its mark in the gutter, its words after it, indented.
  '.cm-bullet, .cm-number, .cm-todo': { paddingLeft: 'calc(30px + var(--indent, 0) * 24px) !important', textIndent: '-30px' },
  '.cm-dot, .cm-num': { display: 'inline-block', width: '30px', textIndent: '0', color: 'var(--accent)' },
  '.cm-dot': { textAlign: 'center' },
  '.cm-num': { textAlign: 'right', paddingRight: '8px', boxSizing: 'border-box', color: 'var(--muted)' },
  '.cm-box': {
    appearance: 'none', width: '18px', height: '18px', margin: '0 12px 0 0', verticalAlign: '-3px', cursor: 'pointer',
    border: '1.5px solid var(--ring)', borderRadius: '50%', background: 'transparent', textIndent: '0',
  },
  '.cm-box:checked': { background: 'var(--accent)', borderColor: 'var(--accent)' },
  '.cm-box:checked::after': {
    content: "''", display: 'block', width: '5px', height: '9px', margin: '1px 0 0 5px',
    border: 'solid var(--on-accent)', borderWidth: '0 2px 2px 0', transform: 'rotate(45deg)',
  },
  '.cm-ticked': { color: 'var(--muted)', textDecoration: 'line-through', textDecorationColor: 'var(--faint)' },
  '.cm-quote': { borderLeft: '3px solid var(--border-strong)', paddingLeft: '14px !important', color: 'var(--muted)', fontFamily: 'var(--serif)' },
  // A paragraph's dropped first letter (page.ts `ini`), as the desktop and
  // the reader drop it (styles.css `p.initial`): three lines tall in the
  // headings' serif, in the accent unless it has a colour of its own.
  // Floated, sized to reach from the first line's capitals to the third's
  // baseline: `initial-letter` sinks it only two lines where the spaces
  // are kept as written, as an editor keeps them.
  '.cm-initial::first-letter': {
    float: 'left', fontFamily: 'var(--serif)', fontWeight: '400', fontSize: '6em', lineHeight: '0.78',
    margin: '0.02em 0.06em 0 0', color: 'var(--accent-ink)',
  },
  '.cm-initial.cm-own::first-letter': { color: 'inherit' },
  '.cm-code, .cm-front': { font: '14px/1.6 ui-monospace, Menlo, monospace', backgroundColor: 'var(--sunken)', paddingLeft: '12px !important', paddingRight: '12px !important' },
  '.cm-front': { color: 'var(--muted)' },
  '.cm-b': { fontWeight: '700' },
  '.cm-i': { fontStyle: 'italic' },
  '.cm-s': { textDecoration: 'line-through' },
  '.cm-c': { font: '0.85em ui-monospace, Menlo, monospace', backgroundColor: 'var(--sunken)', borderRadius: '4px', padding: '1px 4px' },
  '.cm-l': { color: 'var(--accent-ink)', textDecoration: 'underline', textDecorationColor: 'color-mix(in srgb, var(--accent-ink) 40%, transparent)' },
  '.cm-marked': { borderRadius: '3px' },
  // A divider as the desktop draws it by default (settings `divider =
  // dots`): three dots 3px round, 15px apart, faint, with the desktop's
  // space round them (styles.css `hr` the same).
  '.cm-divider': { textAlign: 'center', paddingTop: '26px !important', paddingBottom: '16px' },
  '.cm-rule': {
    display: 'inline-block', width: '33px', height: '3px', verticalAlign: 'middle', color: 'var(--faint)',
    background: 'radial-gradient(circle at 1.5px 1.5px, currentColor 1.1px, transparent 1.6px) 0 0 / 15px 3px repeat-x',
  },
  // A divider selected whole (page.ts `ruleChosen`): the selection's
  // wash over its line, the dots in the accent.
  '.cm-rule-on': { backgroundColor: 'var(--selection)', borderRadius: 'var(--radius)' },
  '.cm-rule-on .cm-rule': { color: 'var(--accent)' },
  // Pictures (page.ts `PictureRow`), laid out as the reading view lays
  // them out (styles.css `.pictures`). Every space is inside what
  // CodeMirror measures: padding, never a margin, and a line that goes
  // below a picture at an edge clears it inside itself.
  //
  // One on a line of its own: below any at an edge.
  '.cm-pics': { padding: '16px 0 4px' },
  '.cm-pics::before, .cm-clear::before, .cm-divider::before': { content: "''", display: 'block', clear: 'both' },
  '.cm-picture-box': { display: 'block' },
  '.cm-picture': { display: 'block', maxWidth: '100%', height: 'auto', borderRadius: 'var(--radius)', background: 'var(--sunken)' },
  // At an edge, one after another (`--n` of them): a row floated there,
  // in a wrapper of no height, below any row before it; each picture its
  // share of the text's width, and one no width was given at most 45% of
  // it. The lines after it run beside it, each a column of its own, as
  // the reader's blocks do. The space above it is the wrapper's, which
  // the line after it does without: so a row below another is as far
  // below it as in the reader.
  '.cm-pics.cm-float': { padding: '12px 0 0' },
  '.cm-float + .cm-line, .cm-pics-beside + .cm-line': { paddingTop: '0 !important' },
  '.cm-pics.cm-float::before': { content: 'none' },
  '.cm-float .cm-pics-row': {
    '--share': 'calc((100cqi - (var(--n) - 1) * 16px) / var(--n))',
    display: 'flex', gap: '16px', alignItems: 'flex-start', clear: 'both', maxWidth: '100%',
  },
  '.cm-left .cm-pics-row': { float: 'left', margin: '8px 20px 14px 0' },
  '.cm-right .cm-pics-row': { float: 'right', margin: '8px 0 14px 20px' },
  '.cm-float .cm-picture-box': { flex: 'none' },
  '.cm-float .cm-picture': { minWidth: '48px', maxWidth: 'var(--share)' },
  '.cm-float .cm-picture-box:not(.cm-sized) .cm-picture': { maxWidth: 'min(45cqi, var(--share))' },
  // Too wide to leave 180px beside it (picture.ts `pictureRoom`): as wide
  // as the text, its pictures at its edge, and the words below it, as the
  // reader's go. A line that would start beside it (and a divider, which
  // goes below every row) is a plain block, whose words move below inside
  // it, measured; a column moved down would leave a gap CodeMirror can't
  // see.
  '.cm-alone .cm-pics-row': { width: '100%', marginLeft: '0', marginRight: '0' },
  '.cm-right.cm-alone .cm-pics-row': { justifyContent: 'flex-end' },
  '.cm-alone ~ .cm-line, .cm-divider': { display: 'block' },
  // A table never runs beside them (carnet_core::page::runs_beside): it
  // starts below, at its full width, cleared inside its line, measured.
  '.cm-line.cm-table': { display: 'block', paddingTop: '0 !important', paddingBottom: '8px' },
  // No empty line of the caret's above and below it.
  '.cm-line.cm-table > .cm-widgetBuffer': { position: 'absolute' },
  '.cm-table::before': { content: "''", display: 'block', clear: 'both', paddingTop: '20px' },
  // Its place until it comes (styles.css `.picture-card`) the same.
  '.cm-pics .picture-card': { margin: '0' },
  '.cm-float .picture-card': { flex: 'none', maxWidth: 'var(--share)' },
  '.cm-float .cm-picture-box:not(.cm-sized) .picture-card': { width: 'min(45cqi, var(--share))' },
  // Selected: no caret, no wash; its ring says so (picture.ts).
  '&.cm-picture-picked .cm-content': { caretColor: 'transparent' },
  '&.cm-picture-picked .cm-pics ::selection, &.cm-picture-picked .cm-pics::selection': { backgroundColor: 'transparent !important' },
  // The selected picture (picture.ts): a ring 5px out in the accent, a bar
  // on each side and a dot at the outer corner, dragged to resize; under
  // the note's header when scrolled beneath it. A finger gets more room
  // round each handle than a mouse.
  '.cm-picked': { position: 'absolute', zIndex: '0', pointerEvents: 'none' },
  '.cm-picked-ring': { position: 'absolute', inset: '-5px', border: '2px solid var(--accent)', borderRadius: 'var(--radius-card)' },
  '.cm-grip': {
    position: 'absolute', boxSizing: 'border-box', pointerEvents: 'auto', touchAction: 'none',
    background: 'var(--card)', border: '2px solid var(--accent)', borderRadius: '999px',
  },
  '.cm-grip::before': { content: "''", position: 'absolute', inset: '-10px' },
  '.cm-picked:not(.sized) .cm-grip': { display: 'none' },
  '.cm-grip-left, .cm-grip-right': { top: '50%', width: '8px', height: 'min(30px, 50%)', transform: 'translateY(-50%)', cursor: 'ew-resize' },
  '.cm-grip-left': { left: '-9px' },
  '.cm-grip-right': { right: '-9px' },
  '.cm-grip-corner': { right: '-10px', bottom: '-10px', width: '14px', height: '14px', cursor: 'nwse-resize' },
  '.cm-picked.right .cm-grip-corner': { right: 'auto', left: '-10px', cursor: 'nesw-resize' },
  '@media (pointer: coarse)': {
    '.cm-grip::before': { inset: '-16px' },
    '.cm-grip-left, .cm-grip-right': { width: '10px', height: 'min(40px, 50%)' },
    '.cm-grip-corner': { width: '20px', height: '20px', right: '-12px', bottom: '-12px' },
    '.cm-picked.right .cm-grip-corner': { left: '-12px' },
  },
  '.cm-picked-size': {
    display: 'none', position: 'absolute', left: '8px', bottom: '8px', padding: '3px 8px', borderRadius: 'var(--radius)',
    background: 'var(--card)', color: 'var(--ink)', font: '500 12.5px/1.3 var(--sans)', whiteSpace: 'nowrap', boxShadow: '0 1px 4px #0000001f',
  },
  '.cm-picked.dragging .cm-picked-size': { display: 'block' },
  // A table as the desktop draws it (styles.css `.note table` the same):
  // the text's width, its columns sharing it alike; one hairline in the
  // border's colour for the rounded frame and the rules; the header row
  // small and quiet on a wash; the cells roomy, in the words' type.
  '.cm-tablewrap': { display: 'block', overflowX: 'auto' },
  '.cm-tablewrap table': {
    width: '100%', tableLayout: 'fixed', borderCollapse: 'separate', borderSpacing: '0',
    border: '1px solid var(--border)', borderRadius: 'var(--radius)', font: 'inherit',
  },
  '.cm-tablewrap th, .cm-tablewrap td': {
    border: '0', borderTop: '1px solid var(--border)', borderLeft: '1px solid var(--border)', padding: '8px 14px',
    textAlign: 'left', verticalAlign: 'top', outline: 'none', caretColor: 'var(--accent)', overflowWrap: 'anywhere',
  },
  '.cm-tablewrap tr:first-child > *': { borderTop: '0' },
  '.cm-tablewrap tr > :first-child': { borderLeft: '0' },
  '.cm-tablewrap th:focus, .cm-tablewrap td:focus': { boxShadow: 'inset 0 0 0 2px var(--accent-soft)' },
  '.cm-tablewrap th': { background: 'var(--sunken)', color: 'var(--muted)', font: '400 12.5px/18px var(--sans)', padding: '9px 14px' },
  '.cm-tablewrap tr:first-child > :first-child': { borderTopLeftRadius: 'calc(var(--radius) - 1px)' },
  '.cm-tablewrap tr:first-child > :last-child': { borderTopRightRadius: 'calc(var(--radius) - 1px)' },
  // A picture among a cell's words (table.ts): as wide as the note gives
  // it, or its own, under half the text's when it has no width given, as
  // the reader has it; its place until it comes the same. (A cell is as
  // wide as what it holds: a share of it would be a share of nothing.)
  '.cm-cellpic': { display: 'inline-block', verticalAlign: 'top', borderRadius: 'var(--radius)', cursor: 'default' },
  '.cm-cellpic-img': { display: 'block', maxWidth: '45cqi', height: 'auto', borderRadius: 'var(--radius)', background: 'var(--sunken)' },
  '.cm-cellpic-img[width]': { maxWidth: '100cqi' },
  '.cm-cellpic .picture-card': { width: 'min(45cqi, 280px)', minHeight: '72px', margin: '0' },
});
