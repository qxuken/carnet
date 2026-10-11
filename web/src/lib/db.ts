// A tiny key-value store over IndexedDB: what the viewer keeps in this
// browser (its keys, sealed, and its notes, sealed).
//
// One connection, opened on first use and kept. The browser may close it
// (Safari does, for a page left in the background, and another tab
// upgrading the database asks it to): then the next call opens another.

const NAME = 'carnet';
const STORE = 'kv';

/// The database's layout: its stores, and what their keys hold. Raised
/// when a release changes either, with a step in UPGRADES from the one
/// before. (What the vault keeps under `state` has a layout of its own,
/// in its bytes: carnet-web's `STATE`.)
const VERSION = 1;

/// The steps, oldest first: UPGRADES[n] takes the database from version n
/// to n + 1 (0 is none at all). The browser runs them in one transaction,
/// so a step cut short is undone, never half done.
const UPGRADES: ((db: IDBDatabase, tx: IDBTransaction) => void)[] = [(db) => db.createObjectStore(STORE)];

/// A newer Carnet's database: this one can't read it, and doesn't touch
/// it. The page updates itself (its service worker), so a reload is the
/// way to the newer one. As carnet-web says it of the vault's bytes.
export class Newer extends Error {
  constructor() {
    super('The notes kept in this browser were saved by a newer Carnet. Reload the page to get it.');
  }
}

let opened: Promise<IDBDatabase> | null = null;

function open(): Promise<IDBDatabase> {
  opened ??= new Promise<IDBDatabase>((resolve, reject) => {
    const req = indexedDB.open(NAME, VERSION);
    req.onupgradeneeded = (e) => {
      for (let v = e.oldVersion; v < VERSION; v++) UPGRADES[v](req.result, req.transaction!);
    };
    req.onsuccess = () => {
      const db = req.result;
      const forget = () => {
        opened = null;
      };
      db.onclose = forget;
      db.onversionchange = () => {
        db.close();
        forget();
      };
      resolve(db);
    };
    req.onerror = () => {
      opened = null;
      reject(req.error?.name === 'VersionError' ? new Newer() : req.error);
    };
  });
  return opened;
}

async function run<T>(mode: IDBTransactionMode, f: (s: IDBObjectStore) => IDBRequest<T> | void): Promise<T> {
  let db = await open();
  let tx: IDBTransaction;
  try {
    tx = db.transaction(STORE, mode);
  } catch {
    // Closed under us since it was opened: once more, on a new one.
    opened = null;
    db = await open();
    tx = db.transaction(STORE, mode);
  }
  return new Promise((resolve, reject) => {
    const req = f(tx.objectStore(STORE));
    tx.oncomplete = () => resolve(req ? req.result : (undefined as T));
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error);
  });
}

export const get = <T>(key: string) => run<T | undefined>('readonly', (s) => s.get(key));
export const set = (key: string, value: unknown) => run('readwrite', (s) => s.put(value, key));
export const keys = async () => (await run('readonly', (s) => s.getAllKeys())).map(String);
/// Each key gone, in one transaction.
export const delMany = (keys: string[]) =>
  run<void>('readwrite', (s) => {
    for (const k of keys) s.delete(k);
  });
export const clear = () => run('readwrite', (s) => s.clear());
