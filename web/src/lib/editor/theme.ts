// The editor in the note's own look: the reading view's type and colours
// (styles.css `.note`), so going from reading to writing moves nothing.
// Each line is drawn from what the page says it is (page.ts `Look`).

import { EditorView } from '@codemirror/view';

export const theme = EditorView.theme({
  '&': { color: 'var(--ink)', backgroundColor: 'transparent', font: '400 17px/1.65 var(--sans)' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'inherit', lineHeight: 'inherit', overflow: 'visible' },
  '.cm-content': { padding: '0', caretColor: 'var(--accent)' },
  '.cm-line': { padding: '0' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)', borderLeftWidth: '2px' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection': { backgroundColor: 'var(--selection) !important' },
  // A block's space above, as the reader spaces them; items close.
  '.cm-block': { paddingTop: '12px !important' },
  '.cm-bullet.cm-block, .cm-number.cm-block, .cm-todo.cm-block': { paddingTop: '3px !important' },
  '.cm-title': { font: '600 32px/1.25 var(--serif)', paddingBottom: '6px !important' },
  '.cm-h1': { font: '600 26px/1.3 var(--serif)', paddingTop: '18px !important' },
  '.cm-h2': { font: '600 24px/1.3 var(--serif)', paddingTop: '16px !important' },
  '.cm-h3': { font: '600 19px/1.35 var(--serif)', paddingTop: '12px !important' },
  '.cm-ph': { color: 'var(--faint)', pointerEvents: 'none' },
  // A list item: its mark in the gutter, its words after it, indented.
  '.cm-bullet, .cm-number, .cm-todo': { paddingLeft: 'calc(30px + var(--indent, 0) * 24px) !important', textIndent: '-30px' },
  '.cm-dot, .cm-num': { display: 'inline-block', width: '30px', textIndent: '0', color: 'var(--accent)' },
  '.cm-dot': { textAlign: 'center' },
  '.cm-num': { textAlign: 'right', paddingRight: '8px', boxSizing: 'border-box', color: 'var(--muted)' },
  '.cm-box': {
    appearance: 'none', width: '18px', height: '18px', margin: '0 12px 0 0', verticalAlign: '-3px', cursor: 'pointer',
    border: '1.6px solid var(--border-strong)', borderRadius: '50%', background: 'var(--card)', textIndent: '0',
  },
  '.cm-box:checked': { background: 'var(--accent)', borderColor: 'var(--accent)' },
  '.cm-box:checked::after': {
    content: "''", display: 'block', width: '5px', height: '9px', margin: '1px 0 0 5px',
    border: 'solid var(--on-accent)', borderWidth: '0 2px 2px 0', transform: 'rotate(45deg)',
  },
  '.cm-ticked': { color: 'var(--muted)', textDecoration: 'line-through', textDecorationColor: 'var(--faint)' },
  '.cm-quote': { borderLeft: '3px solid var(--border-strong)', paddingLeft: '14px !important', color: 'var(--muted)' },
  '.cm-code, .cm-front': { font: '14px/1.6 ui-monospace, Menlo, monospace', backgroundColor: 'var(--sunken)', paddingLeft: '12px !important', paddingRight: '12px !important' },
  '.cm-front': { color: 'var(--muted)' },
  '.cm-b': { fontWeight: '700' },
  '.cm-i': { fontStyle: 'italic' },
  '.cm-s': { textDecoration: 'line-through' },
  '.cm-c': { font: '0.85em ui-monospace, Menlo, monospace', backgroundColor: 'var(--sunken)', borderRadius: '4px', padding: '1px 4px' },
  '.cm-l': { color: 'var(--accent-ink)', textDecoration: 'underline', textDecorationColor: 'color-mix(in srgb, var(--accent-ink) 40%, transparent)' },
  '.cm-marked': { borderRadius: '3px' },
  '.cm-rule': { display: 'inline-block', width: '100%', borderTop: '1px solid var(--border-strong)', verticalAlign: 'middle' },
  '.cm-picture': { display: 'block', maxWidth: '100%', height: 'auto', borderRadius: 'var(--radius)', background: 'var(--sunken)', margin: '4px 0' },
  '.cm-picture.waiting': { minHeight: '120px', minWidth: '160px' },
  '.cm-picture-box': { display: 'block' },
  // A picture at an edge: at that edge of its line, and with no width
  // given at most 45% of the text's, as the desktop sizes it.
  '.cm-float-right .cm-picture-box': { textAlign: 'right' },
  '.cm-float-right .cm-picture': { marginLeft: 'auto' },
  '.cm-float-left:not(.cm-sized) .cm-picture, .cm-float-right:not(.cm-sized) .cm-picture': { maxWidth: '45%' },
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
  '.cm-tablewrap': { display: 'block', overflowX: 'auto' },
  '.cm-tablewrap table': { borderCollapse: 'collapse', margin: '4px 0', font: '400 15px/1.5 var(--sans)' },
  '.cm-tablewrap th, .cm-tablewrap td': { border: '1px solid var(--border-strong)', padding: '6px 10px', textAlign: 'left', verticalAlign: 'top', minWidth: '3em', outline: 'none', caretColor: 'var(--accent)' },
  '.cm-tablewrap th:focus, .cm-tablewrap td:focus': { boxShadow: 'inset 0 0 0 2px var(--accent-soft)' },
  '.cm-tablewrap th': { background: 'var(--sunken)', color: 'var(--muted)', fontWeight: '600', fontSize: '14px' },
});
