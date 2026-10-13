// A field that takes the keyboard as it shows, given it by hand: the
// `autofocus` attribute is ignored once the page has a focused element
// (the note's editor, which a sheet opens over), so what was typed for
// the field went into the note instead.

/// For `use:focusNow`: `node` focused, and its words selected to be
/// typed over.
export function focusNow(node: HTMLInputElement) {
  node.focus({ preventScroll: true });
  node.select();
}
