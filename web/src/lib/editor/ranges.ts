// Where a style's run is drawn on a line of the editor, kept apart from
// CodeMirror so it can be tested on its own (ranges.test.ts).

/// A run `a..b` of a line's words (`Look.r`, in UTF-16 from the line's
/// start) as a range of the editor's text: held to the line, and `null`
/// when nothing of it is left there. The looks can trail the text for a
/// moment — a change CodeMirror made itself (a Mac's Ctrl+K, which cuts
/// to the line's end) is drawn before the page's looks for it come — and
/// a mark decoration may never be empty: CodeMirror stops the plugin
/// that gave one, and every style goes with it.
export function markRange(from: number, length: number, a: number, b: number): [number, number] | null {
  const s = from + Math.max(0, Math.min(a, length));
  const e = from + Math.max(0, Math.min(b, length));
  return e > s ? [s, e] : null;
}
