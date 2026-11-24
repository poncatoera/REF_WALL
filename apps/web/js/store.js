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

// Changes kept for share links while offline (see cloud.rs); not part of
// this browser's own boards, so backups leave them out and keep them.
const UNSENT = "unsent/";

// ---- backups: one .zip file -------------------------------------------------
//
// wall-web-backup/keys.json lists each saved key, whether its value is text,
// and the file inside the zip that holds it. Entries are stored, not
// compressed (pictures and videos don't compress), and the zip is built as a
// Blob of parts, so no giant string is ever made.

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(bytes) {
  let c = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) c = CRC_TABLE[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function dosTime(d) {
  return {
    time: (d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1),
    date: ((d.getFullYear() - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate(),
  };
}

function zip(files) {
  const enc = new TextEncoder();
  const parts = [];
  const central = [];
  let offset = 0;
  const { time, date } = dosTime(new Date());
  for (const [name, data] of files) {
    const n = enc.encode(name);
    const crc = crc32(data);
    if (offset + 30 + n.length + data.length > 0xfffffff0) throw new Error("too much for one backup file (over 4 GB)");
    const h = new DataView(new ArrayBuffer(30));
    h.setUint32(0, 0x04034b50, true);
    h.setUint16(4, 20, true);
    h.setUint16(6, 0x0800, true);
    h.setUint16(8, 0, true);
    h.setUint16(10, time, true);
    h.setUint16(12, date, true);
    h.setUint32(14, crc, true);
    h.setUint32(18, data.length, true);
    h.setUint32(22, data.length, true);
    h.setUint16(26, n.length, true);
    h.setUint16(28, 0, true);
    parts.push(h.buffer, n, data);
    const c = new DataView(new ArrayBuffer(46));
    c.setUint32(0, 0x02014b50, true);
    c.setUint16(4, 20, true);
    c.setUint16(6, 20, true);
    c.setUint16(8, 0x0800, true);
    c.setUint16(10, 0, true);
    c.setUint16(12, time, true);
    c.setUint16(14, date, true);
    c.setUint32(16, crc, true);
    c.setUint32(20, data.length, true);
    c.setUint32(24, data.length, true);
    c.setUint16(28, n.length, true);
    c.setUint32(42, offset, true);
    central.push(c.buffer, n);
    offset += 30 + n.length + data.length;
  }
  const cdSize = central.reduce((a, p) => a + p.byteLength, 0);
  const e = new DataView(new ArrayBuffer(22));
  e.setUint32(0, 0x06054b50, true);
  e.setUint16(8, files.length, true);
  e.setUint16(10, files.length, true);
  e.setUint32(12, cdSize, true);
  e.setUint32(16, offset, true);
  return new Blob([...parts, ...central, e.buffer], { type: "application/zip" });
}

// Everything saved in this browser, as a .zip Blob.
export async function storeExport() {
  const enc = new TextEncoder();
  const keys = [];
  const files = [];
  let i = 0;
  for (const [k, v] of await all()) {
    if (typeof k !== "string" || k.startsWith(UNSENT)) continue;
    const text = typeof v === "string";
    const safe = k.replace(/[^A-Za-z0-9._-]+/g, "_");
    const file = `wall-web-backup/${String(i++).padStart(5, "0")}-${safe}${text ? ".json" : ""}`;
    keys.push({ key: k, text, file });
    files.push([file, text ? enc.encode(v) : new Uint8Array(v.buffer || v)]);
  }
  files.unshift(["wall-web-backup/keys.json", enc.encode(JSON.stringify({ wall_backup: 2, keys }))]);
  return zip(files);
}

// True if `text` is an old (JSON) backup made by this page.
export function isBackup(text) {
  return text.startsWith('{"wall_backup":1');
}

// The entries of a zip File (stored ones only), by name, as Blobs.
async function unzip(file) {
  const tailLen = Math.min(file.size, 65557);
  const tail = new DataView(await file.slice(file.size - tailLen).arrayBuffer());
  let at = -1;
  for (let i = tailLen - 22; i >= 0; i--) {
    if (tail.getUint32(i, true) === 0x06054b50) { at = i; break; }
  }
  if (at < 0) throw new Error("not a zip file");
  const count = tail.getUint16(at + 10, true);
  const cdSize = tail.getUint32(at + 12, true);
  const cdStart = tail.getUint32(at + 16, true);
  const cd = new DataView(await file.slice(cdStart, cdStart + cdSize).arrayBuffer());
  const dec = new TextDecoder();
  const out = new Map();
  let p = 0;
  for (let k = 0; k < count; k++) {
    if (cd.getUint32(p, true) !== 0x02014b50) throw new Error("damaged backup");
    const method = cd.getUint16(p + 10, true);
    const crc = cd.getUint32(p + 16, true);
    const size = cd.getUint32(p + 24, true);
    const nl = cd.getUint16(p + 28, true), el = cd.getUint16(p + 30, true), cl = cd.getUint16(p + 32, true);
    const off = cd.getUint32(p + 42, true);
    const name = dec.decode(new Uint8Array(cd.buffer, cd.byteOffset + p + 46, nl));
    p += 46 + nl + el + cl;
    if (method !== 0) throw new Error(`${name} is compressed`);
    const lh = new DataView(await file.slice(off, off + 30).arrayBuffer());
    const start = off + 30 + lh.getUint16(26, true) + lh.getUint16(28, true);
    out.set(name, { blob: file.slice(start, start + size), crc });
  }
  return out;
}

// Replaces everything saved (but changes kept for share links) with a
// backup: a .zip from storeExport, or an older .json one.
export async function storeImportFile(file) {
  let entries;
  const head = new Uint8Array(await file.slice(0, 2).arrayBuffer());
  if (head[0] === 0x50 && head[1] === 0x4b) {
    const files = await unzip(file);
    const list = files.get("wall-web-backup/keys.json");
    if (!list) throw new Error("not a Wall backup");
    const keys = JSON.parse(await list.blob.text());
    if (keys.wall_backup !== 2) throw new Error("not a Wall backup");
    entries = [];
    for (const { key, text, file: name } of keys.keys) {
      const f = files.get(name);
      if (!f) throw new Error(`${name} is missing from the backup`);
      const bytes = new Uint8Array(await f.blob.arrayBuffer());
      if (crc32(bytes) !== f.crc) throw new Error(`${name} is damaged`);
      entries.push([key, text ? new TextDecoder().decode(bytes) : bytes]);
    }
  } else {
    const backup = JSON.parse(await file.text());
    if (backup.wall_backup !== 1) throw new Error("not a Wall backup");
    entries = Object.entries(backup.entries).map(([k, v]) => [k, v.text !== undefined ? v.text : fromBase64(v.base64)]);
  }
  const d = await db();
  await new Promise((resolve, reject) => {
    const tx = d.transaction(STORE, "readwrite");
    const store = tx.objectStore(STORE);
    const keys = store.getAllKeys();
    keys.onsuccess = () => {
      for (const k of keys.result) {
        if (!(typeof k === "string" && k.startsWith(UNSENT))) store.delete(k);
      }
      for (const [k, v] of entries) store.put(v, k);
    };
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

// Removes every key that starts with `prefix`.
export async function storeDelete(prefix) {
  const d = await db();
  return new Promise((resolve, reject) => {
    const tx = d.transaction(STORE, "readwrite");
    const store = tx.objectStore(STORE);
    const keys = store.getAllKeys();
    keys.onsuccess = () => {
      for (const k of keys.result) {
        if (typeof k === "string" && k.startsWith(prefix)) store.delete(k);
      }
    };
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error);
  });
}
