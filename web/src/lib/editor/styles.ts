// The words' styles the toolbar and the keys switch, as the desktop has
// them: each one's number for `PageEditor.style` (crates/carnet-web), the
// flag `looks` and `style_here` give words that have it, its icon and
// name, and its key as CodeMirror spells it. Code has a key, no button.

export type Style = { which: number; flag: number; icon: string; label: string; key: string; tool: boolean };

export const STYLES: Style[] = [
  { which: 0, flag: 1, icon: 'bold', label: 'Bold', key: 'Mod-b', tool: true },
  { which: 1, flag: 2, icon: 'italic', label: 'Italic', key: 'Mod-i', tool: true },
  { which: 2, flag: 4, icon: 'strike', label: 'Strikethrough', key: 'Shift-Mod-x', tool: true },
  { which: 4, flag: 64, icon: 'underline', label: 'Underline', key: 'Mod-u', tool: true },
  { which: 3, flag: 8, icon: 'code', label: 'Code', key: 'Mod-e', tool: false },
];

/// The style a key pressed with ⌘ (Ctrl) switches, `key` as `keydown`
/// names it and Shift held or not: what a table's cell, which CodeMirror's
/// keymap does not reach, reads. None for any other.
export function styleOfKey(key: string, shift: boolean): number | null {
  const spec = `${shift ? 'Shift-' : ''}Mod-${key.toLowerCase()}`;
  return STYLES.find((s) => s.key === spec)?.which ?? null;
}
