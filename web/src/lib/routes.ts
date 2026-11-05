// The app's hash addresses keep browser Back working between notes and
// sections. A section is still URL-encoded as written in the Markdown.
export type Route =
  | { kind: 'list' }
  | { kind: 'note'; key: string; editing: boolean; fresh: boolean; section: string | null }
  | { kind: 'history'; key: string }
  | { kind: 'settings' };

export function readRoute(hash: string): Route {
  if (hash === '#settings') return { kind: 'settings' };
  const m = /^#note\/([0-9a-f]+)(?:\/(history|edit|new))?(?:\?section=(.*))?$/.exec(hash);
  if (m?.[2] === 'history') return { kind: 'history', key: m[1] };
  if (m) return { kind: 'note', key: m[1], editing: m[2] === 'edit' || m[2] === 'new', fresh: m[2] === 'new', section: m[3] === undefined ? null : new URLSearchParams(`section=${m[3]}`).get('section') };
  return { kind: 'list' };
}

export function noteAddress(key: string, href: string): string {
  const at = href.indexOf('#');
  const section = at >= 0 ? href.slice(at + 1) : '';
  return `#note/${key}${section ? `?section=${encodeURIComponent(section)}` : ''}`;
}
