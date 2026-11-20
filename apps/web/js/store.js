// Where the web version keeps boards and media: one IndexedDB object store
// of key -> value ("boards" -> JSON text, "media/<hash>" -> Uint8Array).
const DB = "wall";
const STORE = "kv";
let opened;

function db() {
  if (!opened) {
    opened = new Promise((resolve, reject) => {
      const req = indexedDB.open(DB, 1);
      req.onupgradeneeded = () => req.result.createObjectStore(STORE);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }
  return opened;
}

export async function storeGet(key) {
  const d = await db();
  return new Promise((resolve, reject) => {
    const req = d.transaction(STORE).objectStore(STORE).get(key);
    req.onsuccess = () => resolve(req.result === undefined ? null : req.result);
    req.onerror = () => reject(req.error);
  });
}

function toBase64(bytes) {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  }
  return btoa(s);
}

function fromBase64(text) {
  const s = atob(text);
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

async function all() {
  const d = await db();
  return new Promise((resolve, reject) => {
    const store = d.transaction(STORE).objectStore(STORE);
    const keys = store.getAllKeys();
    const values = store.getAll();
    values.onsuccess = () => resolve(keys.result.map((k, i) => [k, values.result[i]]));
    values.onerror = () => reject(values.error);
  });
}

// Everything saved, as one JSON text: { wall_backup: 1, entries: {...} }.
export async function storeExport() {
  const entries = {};
  for (const [k, v] of await all()) {
    entries[k] = typeof v === "string" ? { text: v } : { base64: toBase64(new Uint8Array(v.buffer || v)) };
  }
  return JSON.stringify({ wall_backup: 1, entries });
}

// True if `text` is a backup made by storeExport.
export function isBackup(text) {
  return text.startsWith('{"wall_backup":1');
}

// Replaces everything saved with a backup.
export async function storeImport(text) {
  const backup = JSON.parse(text);
  if (backup.wall_backup !== 1) throw new Error("not a Wall backup");
  const d = await db();
  await new Promise((resolve, reject) => {
    const tx = d.transaction(STORE, "readwrite");
    const store = tx.objectStore(STORE);
    store.clear();
    for (const [k, v] of Object.entries(backup.entries)) {
      store.put(v.text !== undefined ? v.text : fromBase64(v.base64), k);
    }
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error);
  });
}

export async function storePut(key, value) {
  const d = await db();
  return new Promise((resolve, reject) => {
    const tx = d.transaction(STORE, "readwrite");
    tx.objectStore(STORE).put(value, key);
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error);
  });
}
