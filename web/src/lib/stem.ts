/// A picture put in here is named as the desktop names a pasted one: when,
/// in this device's own time, `pasted-2026-10-13-164352`.
export function pastedStem(now: Date): string {
  const two = (n: number) => String(n).padStart(2, '0');
  return `pasted-${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}-${two(now.getHours())}${two(now.getMinutes())}${two(now.getSeconds())}`;
}
