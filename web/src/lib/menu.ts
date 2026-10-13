// A menu at a button (Menu.svelte): its items, and where it goes on the
// screen (menu.test.ts).

/// One of a menu's ways: an action, or a list of them behind it ("Move
/// to"), opened in its place.
export type MenuItem = {
  label: string;
  icon?: string;
  /// Shown, not done: the way it is now ("Pinned").
  checked?: boolean;
  danger?: boolean;
  run?: () => void;
  items?: MenuItem[];
};

/// A list open behind one of a menu's ways: its label and items, and
/// which way of the list before it (by its place there) it's behind.
export type Level = { label: string; items: MenuItem[]; from: number };

/// The list behind `item`, the `n`th of those shown, opened in their
/// place.
export const into = (stack: Level[], item: MenuItem, n: number): Level[] => [...stack, { label: item.label, items: item.items ?? [], from: n }];

/// A step back out of the list shown (Escape, ←, its way back): the
/// lists left open, and the way to focus in the one shown again, the one
/// the list was behind ("Move to"), not its first.
export function back(stack: Level[]): { stack: Level[]; focus: number } {
  return { stack: stack.slice(0, -1), focus: stack.at(-1)?.from ?? 0 };
}

type Box = { left: number; top: number; right: number; bottom: number };

/// Where a menu `w` by `h` goes for a button at `at`, in a window `vw` by
/// `vh`: under it, its right edge at the button's, else above it, if
/// there's more room there; never past the window's edges (8px in), and
/// no taller than the room it has.
export function placeMenu(at: Box, w: number, h: number, vw: number, vh: number): { left: number; top: number; maxHeight: number } {
  const edge = 8;
  const below = vh - at.bottom - edge;
  const above = at.top - edge;
  const left = Math.max(edge, Math.min(at.right - w, vw - w - edge));
  if (h <= below || below >= above) return { left, top: at.bottom + 4, maxHeight: Math.max(120, below - 4) };
  const room = Math.max(120, above - 4);
  return { left, top: Math.max(edge, at.top - 4 - Math.min(h, room)), maxHeight: room };
}
