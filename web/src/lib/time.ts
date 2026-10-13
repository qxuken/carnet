// Times as the desktop says them (app_chrome.rs `ago`, `short_ago`).

const since = (secs: number) => Math.max(0, Date.now() / 1000 - secs);

/// The last second of the year 9999: a time from a server past it (or not
/// a number at all) is none it could mean, said as unknown rather than
/// drawn as "Invalid Date" (as the desktop's `epoch_time`).
const LAST = 253402300799;

/// Whether `secs` (seconds since 1970, from a server) is a time at all.
export const plausible = (secs: number) => Number.isFinite(secs) && secs >= 0 && secs <= LAST;

/// A day, as the desktop writes one: "12 October 2026"; `null` for a time
/// that can't be one.
export function dateWords(secs: number): string | null {
  if (!plausible(secs)) return null;
  return new Date(secs * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC' });
}

/// "just now", "5 minutes ago", "3 hours ago", "2 days ago".
export function ago(secs: number): string {
  if (!plausible(secs)) return 'at an unknown time';
  const s = since(secs);
  const unit = (n: number, one: string) => (n === 1 ? `1 ${one} ago` : `${n} ${one}s ago`);
  if (s < 60) return 'just now';
  if (s < 3600) return unit(Math.floor(s / 60), 'minute');
  if (s < 86400) return unit(Math.floor(s / 3600), 'hour');
  if (s < 86400 * 14) return unit(Math.floor(s / 86400), 'day');
  return unit(Math.floor(s / 604800), 'week');
}

/// The library's terse form: "now", "5m", "3h", "2d", "6w".
export function shortAgo(secs: number): string {
  if (!secs) return '';
  const s = since(secs);
  if (s < 60) return 'now';
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 86400 * 14) return `${Math.floor(s / 86400)}d`;
  return `${Math.floor(s / 604800)}w`;
}

/// How long something has left, rounded up as an invite says it (one
/// just made has a week): "good for 7 more days", "good for 3 more hours".
export function left(until: number): string {
  if (!plausible(until)) return 'good for a long while';
  const s = Math.max(0, until - Date.now() / 1000);
  const unit = (n: number, one: string) => (n === 1 ? `good for 1 more ${one}` : `good for ${n} more ${one}s`);
  if (s < 3600) return unit(Math.max(1, Math.ceil(s / 60)), 'minute');
  if (s < 86400) return unit(Math.ceil(s / 3600), 'hour');
  return unit(Math.ceil(s / 86400), 'day');
}
