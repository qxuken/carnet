// What the viewer keeps is sealed (AES-GCM) under a key the browser made
// and will not hand out: kept in IndexedDB as a CryptoKey that cannot be
// exported. Page script can't read it, but the browser keeps it with the
// site's data: Chromium and Firefox in the profile's files, where a copy
// of the profile opens it; Safari wrapped by the system's keychain
// (docs/SYNC.md §7, "Where the keys live").
//
// Each record is bound to its name (AES-GCM's associated data): moved
// under another name, one record doesn't open as another.

import * as db from './db';

const WRAP = 'wrap';

let wrap: Promise<CryptoKey> | null = null;

/// The key, read once and kept for the page's life (made, the first time).
function wrapKey(): Promise<CryptoKey> {
  wrap ??= (async () => {
    const have = await db.get<CryptoKey>(WRAP);
    if (have) return have;
    const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
    await db.set(WRAP, key);
    return key;
  })().catch((e) => {
    wrap = null;
    throw e;
  });
  return wrap;
}

/// Forgets the key held: the database was cleared, and a new one is made
/// on next use.
export function drop() {
  wrap = null;
}

/// A record as kept: `bound` when its name is its associated data (all
/// but those sealed before that was so).
type Kept = { iv: Uint8Array<ArrayBuffer>; sealed: ArrayBuffer; bound?: 1 };

const named = (name: string) => new TextEncoder().encode(`carnet sealed v1 ${name}`);

export async function put(name: string, bytes: Uint8Array): Promise<void> {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const sealed = await crypto.subtle.encrypt({ name: 'AES-GCM', iv, additionalData: named(name) }, await wrapKey(), new Uint8Array(bytes));
  await db.set(name, { iv, sealed, bound: 1 } satisfies Kept);
}

/// The record kept as `name`; none if there's none, or it doesn't open
/// as that name's.
export async function get(name: string): Promise<Uint8Array<ArrayBuffer> | null> {
  const v = await db.get<Kept>(name);
  if (!v?.iv || !v.sealed) return null;
  const key = await wrapKey();
  try {
    // (No `additionalData` at all for an unbound one: Chromium refuses
    // the member set to undefined.)
    const params: AesGcmParams = v.bound ? { name: 'AES-GCM', iv: v.iv, additionalData: named(name) } : { name: 'AES-GCM', iv: v.iv };
    const plain = new Uint8Array(await crypto.subtle.decrypt(params, key, v.sealed));
    // Sealed before records were bound to their names: bound now, once.
    if (!v.bound) await put(name, plain);
    return plain;
  } catch {
    return null;
  }
}
