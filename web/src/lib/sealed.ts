// What the viewer keeps is sealed (AES-GCM) under a key the browser made
// and will not hand out: kept in IndexedDB as a CryptoKey that cannot be
// exported. A copy of the stored bytes alone opens nothing; only this
// browser, on this site, can open them (docs/SYNC.md §7, "Where the keys
// live").

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

export async function put(name: string, bytes: Uint8Array): Promise<void> {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const sealed = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, await wrapKey(), new Uint8Array(bytes));
  await db.set(name, { iv, sealed });
}

export async function get(name: string): Promise<Uint8Array<ArrayBuffer> | null> {
  const v = await db.get<{ iv: Uint8Array<ArrayBuffer>; sealed: ArrayBuffer }>(name);
  if (!v) return null;
  try {
    return new Uint8Array(await crypto.subtle.decrypt({ name: 'AES-GCM', iv: v.iv }, await wrapKey(), v.sealed));
  } catch {
    return null;
  }
}
