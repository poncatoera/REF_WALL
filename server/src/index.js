// Wall's sync server: one Worker, one library.
//
//   LibraryDO  boards, items, sidecars, share links (one object)
//   MediaDO    picture and video bytes, in 1 MB pieces (8 objects)
//
// Who may do what: a request carries a 256-bit secret as a Bearer token. The
// owner's key matches OWNER_HASH; a share link's secret matches a row in
// `links` (only hashes are stored). A link opens the whole library or one
// board with the boards inside it, to view or to edit, until it's revoked.
//
// Merging is the file format's rule (BoardItem::supersedes): the higher
// `rev` wins, then the larger `updated_by`. Every accepted row gets the next
// `seq`; clients ask for rows after the last seq they have.
//
// History: before a push changes a board, the board as it is is kept as a
// version (at most every 15 minutes per board, thinned like the desktop's:
// all of the last day, hourly for a week, daily for 90 days, weekly after).
// Owners and edit links can list and fetch versions, list deleted boards and
// bring one back (/api/history, /api/deleted, /api/undelete). Bringing a
// version back is done by the app as ordinary item edits.
//
// Live (/api/live, a WebSocket): who else is on a board and where their
// pointer is, and a "poke" after every accepted push so the others pull at
// once instead of at their next poll. The key comes in the first message,
// since a browser can't set headers on a WebSocket. Sockets hibernate, so
// a quiet one costs nothing; the free plan counts 20 messages as a request.
//
// Follow and spotlight ride on the same socket: someone says whose view they
// follow ({t:"follow"}), or that they bring everyone to theirs ({t:"spot"},
// not from view-only links). A person sends their view ({t:"view"}: the
// board area they see) only while followed or leading, and it goes only to
// those following them; when they open another board, those following get a
// {t:"goto"} first, so they can come along.
import { DurableObject } from "cloudflare:workers";

const SHARDS = 8;
const PIECE = 1_000_000; // bytes per media row
const MAX_FILE = 25_000_000;
const MAX_BODY = 4_000_000; // bytes in one JSON request
const MAX_ROW = 900_000; // characters in one item, board or sidecar
const MAX_PUSH = 500; // rows in one push
const PAGE = 500; // rows in one pull
const PAGE_CHARS = 3_000_000;
const PENDING_MS = 60 * 60 * 1000; // an upload that never finished is forgotten
const MAX_LIBRARY_DB = 700_000_000;
const MAX_SOCKETS = 64; // people connected at once
const MAX_STRANGERS = 24; // connections that haven't shown a key yet
const HELLO_MS = 10_000; // and how long they get to
const MAX_MESSAGE = 1000; // characters in one live message
const CURSOR_MS = 80; // pointer updates closer together than this are dropped
const MAX_NAME = 40;
// Versions (see keepVersions): a board as it was, at most this often while
// it changes, gzipped, in rows of PIECE bytes. Not kept once the database is
// this full, so they never crowd out the boards themselves.
const VERSION_EVERY_MS = 15 * 60 * 1000;
const MAX_VERSION = 8_000_000;
const VERSIONS_UNTIL_DB = 500_000_000;
const HOUR = 3_600_000;
const DAY = 24 * HOUR;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const HASH = /^[0-9a-f]{64}$/;
// File kinds Wall stores. The extension ends up in a file name on disk.
const EXT = /^(jpg|jpeg|png|webp|gif|mp4|webm|mov|m4v|mkv|avi)$/;

class Refuse extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

const json = (value, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store", "x-content-type-options": "nosniff" },
  });

async function sha256(text) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function newSecret() {
  const b = crypto.getRandomValues(new Uint8Array(32));
  return btoa(String.fromCharCode(...b)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

const library = (env) => env.LIBRARY.get(env.LIBRARY.idFromName("library"));
const shard = (env, hash) => env.MEDIA.get(env.MEDIA.idFromName(`media-${parseInt(hash[0], 16) % SHARDS}`));

async function bearer(request) {
  const h = request.headers.get("authorization") || "";
  const m = /^Bearer ([A-Za-z0-9_-]{20,200})$/.exec(h);
  if (!m) throw new Refuse(401, "no key");
  return sha256(m[1]);
}

async function body(request) {
  if (Number(request.headers.get("content-length") || 0) > MAX_BODY) throw new Refuse(413, "too much at once");
  const text = await request.text();
  if (text.length > MAX_BODY) throw new Refuse(413, "too much at once");
  try {
    return JSON.parse(text);
  } catch {
    throw new Refuse(400, "not JSON");
  }
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    try {
      // Keys never over plain http (except on this computer, for tests).
      if (url.protocol !== "https:" && !["127.0.0.1", "localhost"].includes(url.hostname)) throw new Refuse(403, "use https");
      if (!url.pathname.startsWith("/api/")) return new Response("Not found", { status: 404 });
      const path = url.pathname.slice(5);
      if (path === "live") {
        if (request.method !== "GET" || (request.headers.get("upgrade") || "").toLowerCase() !== "websocket") throw new Refuse(426, "a WebSocket only");
        return await library(env).fetch(request);
      }
      const who = await bearer(request);
      const media = /^media\/([0-9a-f]{64})$/.exec(path);
      if (media) return await mediaRequest(request, env, who, media[1], url);
      const lib = library(env);
      const call = async (name, ...args) => {
        const r = await lib[name](who, ...args);
        if (r.error) throw new Refuse(r.status, r.error);
        return json(r);
      };
      if (path === "sync" && request.method === "POST") return await call("sync", await body(request));
      if (path === "items" && request.method === "GET") return await call("boardItems", url.searchParams.get("board"), url.searchParams.get("after") || "");
      if (path === "me" && request.method === "GET") return await call("me");
      if (path === "history" && request.method === "GET") {
        const at = url.searchParams.get("at");
        if (at === null) return await call("versions", url.searchParams.get("board"));
        const r = await lib.version(who, url.searchParams.get("board"), at);
        if (r.error) throw new Refuse(r.status, r.error);
        return new Response(r.bytes, { headers: { "content-type": "application/gzip", "cache-control": "no-store", "x-content-type-options": "nosniff" } });
      }
      if (path === "deleted" && request.method === "GET") return await call("deletedBoards");
      if (path === "undelete" && request.method === "POST") return await call("undelete", await body(request));
      if (path === "links" && request.method === "GET") return await call("listLinks");
      if (path === "links" && request.method === "POST") return await call("makeLink", await body(request));
      const link = /^links\/([0-9a-f-]{36})$/.exec(path);
      if (link && request.method === "DELETE") return await call("revokeLink", link[1]);
      return json({ error: "not found" }, 404);
    } catch (e) {
      if (e instanceof Refuse) return json({ error: e.message }, e.status);
      console.error(e);
      return json({ error: "server error" }, 500);
    }
  },
};

async function mediaRequest(request, env, who, hash, url) {
  const lib = library(env);
  if (request.method === "GET" || request.method === "HEAD") {
    const ok = await lib.mayRead(who, hash);
    if (ok.error) throw new Refuse(ok.status, ok.error);
    const headers = {
      // Never a type a browser would run: these bytes come from people.
      "content-type": "application/octet-stream",
      "content-disposition": "attachment",
      "x-content-type-options": "nosniff",
      "content-security-policy": "sandbox; default-src 'none'",
      "cache-control": "private, max-age=31536000, immutable",
      "content-length": String(ok.size),
    };
    if (request.method === "HEAD") return new Response(null, { headers });
    return new Response(await shard(env, hash).read(hash), { headers });
  }
  if (request.method === "PUT") {
    const size = Number(request.headers.get("content-length"));
    if (!Number.isInteger(size) || size <= 0) throw new Refuse(411, "size missing");
    if (size > MAX_FILE) throw new Refuse(413, "file too big");
    const replace = url.searchParams.get("replace") === "1";
    const r = await lib.reserve(who, hash, size, replace);
    if (r.error) throw new Refuse(r.status, r.error);
    if (r.have) return json({ have: true });
    if (r.prove) {
      const bytes = new Uint8Array(await request.arrayBuffer());
      const stored = await shard(env, hash).read(hash);
      // Built in: a loop over 20 MB takes longer than the free plan allows.
      const same = bytes.byteLength === r.size && stored.byteLength === r.size && crypto.subtle.timingSafeEqual(bytes, stored);
      if (!same) throw new Refuse(409, "not the same file");
      await lib.grant(who, hash);
      return json({ have: true });
    }
    const bytes = await request.arrayBuffer();
    if (bytes.byteLength !== size) {
      await lib.release(hash, r.ticket);
      throw new Refuse(400, "size mismatch");
    }
    try {
      await shard(env, hash).write(hash, bytes);
    } catch (e) {
      await lib.release(hash, r.ticket);
      throw e;
    }
    await lib.commit(who, hash, r.ticket);
    return json({ stored: true });
  }
  throw new Refuse(405, "not allowed");
}

const no = (status, error) => ({ status, error });

// A name others will see: one line, no control characters, not too long.
// One of the 8 colours people can pick (an index), or null for "by id".
// What a view link may put on a board.
const MARKS = new Set(["comment", "stamp"]);

// Revs at or past this are dropped by every copy (see wall-sync).
const MAX_REV = 2 ** 52;
// How far past a board's highest rev a link's item may go.
const REV_STEP = 1_000_000;
const STAMP_MARKS = new Set(["+1", "♥", "★", "✓", "?", "!"]);
const MAX_STAMP = 5000; // board units; a stamp is 40 px on screen when made
const MAX_COORD = 1e9;

const onlyKeys = (o, keys) => o && typeof o === "object" && !Array.isArray(o) && Object.keys(o).every((k) => keys.includes(k));
const shortText = (v, max) => typeof v === "string" && v.length <= max;
const num = (v) => typeof v === "number" && Number.isFinite(v) && Math.abs(v) <= MAX_COORD;
const ITEM_KEYS = ["id", "frame", "type", "rev", "updated_ms", "updated_by", "deleted"];
const FRAME_KEYS = ["x", "y", "w", "h", "rotation", "z", "text_scale"];

function frameOk(f, stamp) {
  if (!onlyKeys(f, FRAME_KEYS) || !FRAME_KEYS.filter((k) => k in f).every((k) => num(f[k]))) return false;
  if (!num(f.x) || !num(f.y)) return false;
  // A comment is a point; a stamp a small square.
  return stamp ? f.w > 0 && f.h > 0 && f.w <= MAX_STAMP && f.h <= MAX_STAMP : (f.w ?? 0) === 0 && (f.h ?? 0) === 0;
}

function messageOk(m, person) {
  return onlyKeys(m, ["by", "text", "at_ms"]) && shortText(m.by, 40) && shortText(m.text, 2000) && Number.isSafeInteger(m.at_ms) && m.at_ms >= 0 && (!person || m.by === person);
}

// A view link may start a comment, reply to one, resolve or reopen it, and
// put down, move or take away stamps. It can't change what anyone wrote,
// move a pin, delete or bring back a thread, or turn something else into a
// comment. A link made for one person writes under that person's name.
function viewMayWrite(old, i, person) {
  if (!MARKS.has(i.type) || (old && old.type !== i.type)) return false;
  if (old?.deleted) return false;
  if (i.type === "stamp") {
    if (!onlyKeys(i, [...ITEM_KEYS, "mark", "by"]) || !STAMP_MARKS.has(i.mark) || !frameOk(i.frame, true)) return false;
    if (!old && i.deleted) return false;
    // Only its maker's name, and only when it's made.
    const by = i.by ?? "";
    return old ? by === (old.by ?? "") : shortText(by, 40) && (!person || by === person);
  }
  if (!onlyKeys(i, [...ITEM_KEYS, "messages", "resolved"]) || !Array.isArray(i.messages) || i.messages.length > 500) return false;
  if (i.deleted || ("resolved" in i && typeof i.resolved !== "boolean")) return false;
  if (!old) return frameOk(i.frame, false) && i.messages.length > 0 && i.messages.every((m) => messageOk(m, person));
  if (!onlyKeys(i.frame, FRAME_KEYS) || FRAME_KEYS.some((k) => (old.frame?.[k] ?? null) !== (i.frame?.[k] ?? null))) return false;
  const was = Array.isArray(old.messages) ? old.messages : [];
  const now = i.messages;
  return now.length >= was.length && was.every((m, k) => JSON.stringify(m) === JSON.stringify(now[k])) && now.slice(was.length).every((m) => messageOk(m, person));
}

// Which of these version times to drop (newest first wins each bucket).
function thin(times, now) {
  const seen = new Set();
  const out = [];
  for (const t of [...times].sort((a, b) => b - a)) {
    const age = now - t;
    if (age < DAY) continue;
    const bucket = age < 7 * DAY ? `h${Math.floor(t / HOUR)}` : age < 90 * DAY ? `d${Math.floor(t / DAY)}` : `w${Math.floor(t / (7 * DAY))}`;
    if (seen.has(bucket)) out.push(t);
    else seen.add(bucket);
  }
  return out;
}

async function gzip(text) {
  const stream = new Blob([text]).stream().pipeThrough(new CompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

// A connection id as the server makes them (see fetch), or null.
function cleanId(v) {
  return typeof v === "string" && /^[0-9a-f-]{1,20}$/.test(v) ? v : null;
}

function cleanColor(v) {
  return Number.isInteger(v) && v >= 0 && v < 8 ? v : null;
}

// "Guest" (or the default given) for none, or one called "Guest" itself.
function cleanName(v, fallback = "Guest") {
  const s = typeof v === "string" ? v.replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029\u202a-\u202e\u2066-\u2069]/g, "").trim() : "";
  const name = [...s].slice(0, MAX_NAME).join("");
  return name && name.toLowerCase() !== "guest" ? name : fallback;
}

export class LibraryDO extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    ctx.blockConcurrencyWhile(async () => {
      this.sql.exec(`
        CREATE TABLE IF NOT EXISTS counter (id INTEGER PRIMARY KEY CHECK (id = 1), seq INTEGER NOT NULL);
        INSERT OR IGNORE INTO counter VALUES (1, 0);
        CREATE TABLE IF NOT EXISTS boards (id TEXT PRIMARY KEY, parent TEXT, deleted INTEGER NOT NULL, json TEXT NOT NULL, seq INTEGER NOT NULL) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS items (board TEXT NOT NULL, id TEXT NOT NULL, rev INTEGER NOT NULL, updated_by TEXT NOT NULL, hash TEXT, deleted INTEGER NOT NULL, json TEXT NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (board, id)) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS items_seq ON items (seq);
        CREATE TABLE IF NOT EXISTS sidecars (hash TEXT PRIMARY KEY, json TEXT NOT NULL, seq INTEGER NOT NULL) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS media (hash TEXT PRIMARY KEY, size INTEGER NOT NULL, done INTEGER NOT NULL, ticket TEXT NOT NULL, at INTEGER NOT NULL) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS links (id TEXT PRIMARY KEY, secret TEXT NOT NULL UNIQUE, role TEXT NOT NULL, board TEXT, label TEXT NOT NULL, created INTEGER NOT NULL, revoked INTEGER NOT NULL) WITHOUT ROWID;
        -- Files a board link has shown it has (uploaded, or sent the same
        -- bytes as a file already here), so it may put them on its boards.
        CREATE TABLE IF NOT EXISTS grants (link TEXT NOT NULL, hash TEXT NOT NULL, PRIMARY KEY (link, hash)) WITHOUT ROWID;
        -- Versions of boards: gzipped board JSON in pieces; seq is the
        -- board's newest item seq then, so an unchanged board isn't kept twice.
        CREATE TABLE IF NOT EXISTS versions (board TEXT NOT NULL, at INTEGER NOT NULL, part INTEGER NOT NULL, seq INTEGER NOT NULL, data BLOB NOT NULL, PRIMARY KEY (board, at, part)) WITHOUT ROWID;
        -- When each deleted board was deleted.
        CREATE TABLE IF NOT EXISTS deleted_at (board TEXT PRIMARY KEY, at INTEGER NOT NULL) WITHOUT ROWID;
      `);
      // Added later: the person a link is for ("Mia"); empty for anyone.
      if (!this.rows("SELECT name FROM pragma_table_info('links') WHERE name = 'person'").length) {
        this.sql.exec("ALTER TABLE links ADD COLUMN person TEXT NOT NULL DEFAULT ''");
      }
    });
    // Hashes a board-only link may read, kept while nothing changes.
    this.readable = new Map();
    // When a version of each board was last looked at (see keepVersions).
    this.taken = new Map();
    // Keep-alives are answered without waking the object (and aren't billed).
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  // ---- live: presence and pokes -----------------------------------------

  async fetch(request) {
    // Connections that never show a key must not keep people out.
    const now = Date.now();
    let people = 0;
    let strangers = 0;
    for (const ws of this.ctx.getWebSockets()) {
      const a = ws.deserializeAttachment();
      if (a && a.who) people += 1;
      else if (!a || now - (a.opened || 0) > HELLO_MS) ws.close(4001, "say hello with a key");
      else strangers += 1;
    }
    if (people >= MAX_SOCKETS || strangers >= MAX_STRANGERS) return new Response("too many people at once", { status: 503 });
    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server);
    // id: what others see; who/board/name once it said hello.
    server.serializeAttachment({ id: crypto.randomUUID().slice(0, 13), who: null, opened: now });
    return new Response(null, { status: 101, webSocket: client });
  }

  // The sockets that said hello, with what they said.
  live() {
    const out = [];
    for (const ws of this.ctx.getWebSockets()) {
      const a = ws.deserializeAttachment();
      if (a && a.who) out.push([ws, a]);
    }
    return out;
  }

  send(ws, msg) {
    try {
      ws.send(JSON.stringify(msg));
    } catch {
      // Closing already.
    }
  }

  // To everyone else on `board`.
  tell(board, msg, except) {
    if (!board) return;
    for (const [ws, a] of this.live()) {
      if (ws !== except && a.board === board) this.send(ws, msg);
    }
  }

  peer(a) {
    return { id: a.id, name: a.name, color: a.color ?? null, x: a.x ?? null, y: a.y ?? null, view: a.view ?? null, follow: a.follow ?? null, spot: !!a.spot };
  }

  // What someone who gave no name is called: "Guest (Nike team)" for a
  // link labelled so.
  guestName(access) {
    const label = access.link ? this.rows("SELECT label FROM links WHERE id = ?", access.link)[0]?.label : "";
    const l = cleanName(label, "");
    return l ? [...`Guest (${l})`].slice(0, MAX_NAME).join("") : "Guest";
  }

  // May `access` look at `board`?
  reaches(access, board) {
    if (!UUID.test(board)) return false;
    const scope = this.scope(access);
    return scope ? scope.has(board) : true;
  }

  async webSocketMessage(ws, text) {
    if (typeof text !== "string" || text.length > MAX_MESSAGE) return ws.close(1009, "too big");
    let m;
    try {
      m = JSON.parse(text);
    } catch {
      return ws.close(1003, "not JSON");
    }
    if (!m || typeof m !== "object") return;
    const a = ws.deserializeAttachment();
    if (!a.who) {
      if (m.t !== "hello" || typeof m.key !== "string" || !/^[A-Za-z0-9_-]{20,200}$/.test(m.key)) return ws.close(4001, "say hello with a key");
      const who = await sha256(m.key);
      const access = this.access(who);
      if (!access) return ws.close(4003, "this link doesn't work (any more)");
      a.who = who;
      a.guest = this.guestName(access);
      // A link made for one person carries their name; they can't change it.
      const person = access.link ? cleanName(this.rows("SELECT person FROM links WHERE id = ?", access.link)[0]?.person, "") : "";
      a.fixed = !!person;
      a.name = person || cleanName(m.name, a.guest);
      a.color = cleanColor(m.color);
      a.board = null;
      a.at = 0;
      a.vat = 0;
      a.view = null;
      // Following someone or leading carries over a reconnect.
      a.follow = cleanId(m.follow);
      a.spot = !!m.spot && access.role !== "view";
      ws.serializeAttachment(a);
      this.send(ws, { t: "welcome", id: a.id, seq: this.seq(), ...(a.fixed ? { name: a.name } : {}) });
      if (typeof m.board === "string") return this.join(ws, a, access, m.board);
      return;
    }
    const access = this.access(a.who);
    if (!access) return ws.close(4003, "this link doesn't work (any more)");
    if (m.t === "board" && typeof m.board === "string") return this.join(ws, a, access, m.board);
    if (m.t === "name") {
      if (!a.fixed && "name" in m) a.name = cleanName(m.name, a.guest || "Guest");
      if ("color" in m) a.color = cleanColor(m.color);
      ws.serializeAttachment(a);
      return this.tell(a.board, { t: "peer", peer: this.peer(a) }, ws);
    }
    if (m.t === "cursor") {
      const now = Date.now();
      if (now - a.at < CURSOR_MS) return;
      const ok = (v) => typeof v === "number" && Number.isFinite(v) && Math.abs(v) < 1e9;
      const off = m.x === null || m.y === null;
      if (!off && !(ok(m.x) && ok(m.y))) return;
      a.x = off ? null : m.x;
      a.y = off ? null : m.y;
      a.at = now;
      ws.serializeAttachment(a);
      return this.tell(a.board, { t: "cursor", id: a.id, x: a.x, y: a.y }, ws);
    }
    if (m.t === "view") {
      const now = Date.now();
      if (now - (a.vat || 0) < CURSOR_MS) return;
      const ok = (v) => typeof v === "number" && Number.isFinite(v) && Math.abs(v) < 1e9;
      if (![m.x, m.y, m.w, m.h].every(ok) || m.w <= 0 || m.h <= 0) return;
      a.view = [m.x, m.y, m.w, m.h];
      a.vat = now;
      ws.serializeAttachment(a);
      const msg = { t: "view", id: a.id, x: m.x, y: m.y, w: m.w, h: m.h };
      for (const [w, o] of this.watchers(a, ws)) this.send(w, msg);
      return;
    }
    if (m.t === "follow") {
      a.follow = cleanId(m.id);
      if (a.follow === a.id) a.follow = null;
      ws.serializeAttachment(a);
      return this.tell(a.board, { t: "peer", peer: this.peer(a) }, ws);
    }
    if (m.t === "spot") {
      // Clients can follow, but not pull everyone along.
      if (access.role === "view") return;
      a.spot = !!m.on;
      ws.serializeAttachment(a);
      return this.tell(a.board, { t: "peer", peer: this.peer(a) }, ws);
    }
  }

  // Those on a's board who see through a's eyes: following a, or everyone
  // while a leads.
  watchers(a, except) {
    if (!a.board) return [];
    return this.live().filter(([w, o]) => w !== except && o.board === a.board && (a.spot || o.follow === a.id));
  }

  // Moves a socket to `board` (or nowhere, if it may not look at it).
  join(ws, a, access, board) {
    if (a.board === board) return;
    const next = this.reaches(access, board) ? board : null;
    if (a.board && next) {
      // Those following come along, if their link reaches that board too.
      for (const [w, o] of this.watchers(a, ws)) {
        const theirs = this.access(o.who);
        if (theirs && this.reaches(theirs, next)) this.send(w, { t: "goto", id: a.id, board: next });
      }
    }
    if (a.board) this.tell(a.board, { t: "leave", id: a.id }, ws);
    a.board = next;
    a.x = a.y = null;
    a.view = null;
    ws.serializeAttachment(a);
    if (!a.board) return this.send(ws, { t: "peers", board, peers: [] });
    const peers = this.live().filter(([w, o]) => w !== ws && o.board === a.board).map(([, o]) => this.peer(o));
    this.send(ws, { t: "peers", board: a.board, peers });
    this.tell(a.board, { t: "peer", peer: this.peer(a) }, ws);
  }

  async webSocketClose(ws) {
    const a = ws.deserializeAttachment();
    if (a && a.board) this.tell(a.board, { t: "leave", id: a.id }, ws);
    try {
      ws.close(1000, "bye");
    } catch {
      // Already closed.
    }
  }

  async webSocketError(ws) {
    return this.webSocketClose(ws);
  }

  // Boards moved or went: nobody stays on one their link no longer reaches.
  recheck() {
    for (const [ws, a] of this.live()) {
      if (!a.board) continue;
      const access = this.access(a.who);
      if (access && this.reaches(access, a.board)) continue;
      this.tell(a.board, { t: "leave", id: a.id }, ws);
      this.send(ws, { t: "peers", board: a.board, peers: [] });
      a.board = null;
      a.x = a.y = null;
      ws.serializeAttachment(a);
    }
  }

  // After rows were accepted: everyone pulls now.
  poke() {
    const msg = { t: "poke", seq: this.seq() };
    for (const [ws] of this.live()) this.send(ws, msg);
  }

  rows(q, ...args) {
    return this.sql.exec(q, ...args).toArray();
  }

  seq() {
    return this.rows("SELECT seq FROM counter")[0].seq;
  }

  // { role: "owner" | "edit" | "view", board: root board or null for all, link: id }
  access(who) {
    const owner = this.env.OWNER_HASH;
    if (owner && HASH.test(owner) && who === owner) return { role: "owner", board: null, link: null };
    const l = this.rows("SELECT id, role, board, person FROM links WHERE secret = ? AND revoked = 0", who)[0];
    return l ? { role: l.role, board: l.board, link: l.id, person: l.person || "" } : null;
  }

  // The boards a link reaches: null for all, else a Set of ids.
  scope(a) {
    if (!a.board) return null;
    const children = new Map();
    const live = new Set();
    for (const b of this.rows("SELECT id, parent FROM boards WHERE deleted = 0")) {
      live.add(b.id);
      if (!children.has(b.parent)) children.set(b.parent, []);
      children.get(b.parent).push(b.id);
    }
    const set = new Set();
    const todo = live.has(a.board) ? [a.board] : [];
    while (todo.length) {
      const id = todo.pop();
      if (set.has(id)) continue;
      set.add(id);
      todo.push(...(children.get(id) || []));
    }
    return set;
  }

  // Media hashes on the boards in `scope`.
  hashes(a, scope) {
    if (!scope) return null;
    const key = `${a.board}`;
    const seq = this.seq();
    const kept = this.readable.get(key);
    if (kept && kept.seq === seq) return kept.set;
    const set = new Set();
    for (const id of scope) {
      for (const r of this.sql.exec("SELECT hash FROM items WHERE board = ? AND deleted = 0 AND hash IS NOT NULL", id)) set.add(r.hash);
    }
    if (this.readable.size > 50) this.readable.clear();
    this.readable.set(key, { seq, set });
    return set;
  }

  async me(who) {
    const a = this.access(who);
    if (!a) return no(403, "this link doesn't work (any more)");
    const used = this.used();
    return { role: a.role, board: a.board, seq: this.seq(), ...(a.role === "owner" ? { media_bytes: used, max_media_bytes: this.max() } : {}) };
  }

  max() {
    return Math.min(Number(this.env.MAX_MEDIA_BYTES) || 0, 4_000_000_000);
  }

  used() {
    return this.rows("SELECT COALESCE(SUM(size), 0) AS n FROM media WHERE done = 1 OR at > ?", Date.now() - PENDING_MS)[0].n;
  }

  // Push and pull in one go. `since` is the last seq the client has.
  async sync(who, req) {
    const a = this.access(who);
    if (!a) return no(403, "this link doesn't work (any more)");
    if (!req || typeof req !== "object") return no(400, "bad request");
    const since = Number.isSafeInteger(req.since) && req.since >= 0 ? req.since : 0;
    const boards = Array.isArray(req.boards) ? req.boards : [];
    const items = Array.isArray(req.items) ? req.items : [];
    const sidecars = Array.isArray(req.sidecars) ? req.sidecars : [];
    const pushing = boards.length + items.length + sidecars.length;
    if (pushing > MAX_PUSH) return no(413, "too much at once");
    // A view link may only comment and stamp.
    if (a.role === "view" && (boards.length || sidecars.length || items.some((e) => !MARKS.has(e?.item?.type)))) return no(403, "this link can only look");
    if (pushing && this.sql.databaseSize > MAX_LIBRARY_DB) return no(507, "the library is full");
    let refused = [];
    if (pushing) {
      // Boards about to change: kept as they are first (now and then).
      const touched = new Set();
      for (const e of items) if (typeof e?.board === "string" && UUID.test(e.board)) touched.add(e.board);
      await this.keepVersions(touched);
      try {
        const before = this.seq();
        refused = this.ctx.storage.transactionSync(() => this.accept(a, boards, items, sidecars));
        if (this.seq() !== before) {
          if (boards.length) this.recheck();
          this.poke();
        }
      } catch (e) {
        if (e instanceof Refuse) return no(e.status, e.message);
        throw e;
      }
    }
    const scope = this.scope(a);
    const reads = this.hashes(a, scope);
    const out = { seq: since, more: false, boards: [], items: [], sidecars: [], refused };
    // Everything after `since`, oldest first, in pages.
    const found = [];
    for (const r of this.sql.exec("SELECT id, json, seq FROM boards WHERE seq > ? ORDER BY seq LIMIT ?", since, PAGE + 1)) {
      found.push({ seq: r.seq, kind: "boards", ok: !scope || scope.has(r.id), json: r.json });
    }
    for (const r of this.sql.exec("SELECT board, json, seq FROM items WHERE seq > ? ORDER BY seq LIMIT ?", since, PAGE + 1)) {
      found.push({ seq: r.seq, kind: "items", ok: !scope || scope.has(r.board), board: r.board, json: r.json });
    }
    for (const r of this.sql.exec("SELECT hash, json, seq FROM sidecars WHERE seq > ? ORDER BY seq LIMIT ?", since, PAGE + 1)) {
      found.push({ seq: r.seq, kind: "sidecars", ok: !reads || reads.has(r.hash), json: r.json });
    }
    found.sort((x, y) => x.seq - y.seq);
    let chars = 0;
    let n = 0;
    for (const f of found) {
      if (n >= PAGE || (n > 0 && chars + f.json.length > PAGE_CHARS)) {
        out.more = true;
        break;
      }
      n += 1;
      chars += f.json.length;
      out.seq = f.seq;
      if (!f.ok) continue;
      const value = JSON.parse(f.json);
      out[f.kind].push(f.kind === "items" ? { board: f.board, item: value } : value);
    }
    if (!out.more) out.seq = Math.max(out.seq, this.seq());
    // A board-only link learns which boards it has, so it can fetch one that
    // was moved in and drop one that was moved out.
    if (scope) {
      out.scope = [...scope];
      // Details of pictures that became visible without changing themselves.
      const known = new Set(Array.isArray(req.known) ? req.known : []);
      for (const id of scope) {
        if (known.has(id) || out.boards.some((b) => b.id === id)) continue;
        const b = this.rows("SELECT json FROM boards WHERE id = ?", id)[0];
        if (b) out.boards.push(JSON.parse(b.json));
      }
    }
    return out;
  }

  accept(a, boards, items, sidecars) {
    const refused = [];
    const next = () => {
      this.sql.exec("UPDATE counter SET seq = seq + 1");
      return this.seq();
    };
    let scope = this.scope(a);
    for (const b of boards) {
      if (!b || !UUID.test(b.id) || (b.parent != null && !UUID.test(b.parent)) || b.parent === b.id || typeof b.name !== "string") throw new Refuse(400, "bad board");
      const meta = { ...b, items: undefined };
      const text = JSON.stringify(meta);
      if (text.length > MAX_ROW) throw new Refuse(413, "board too big");
      const old = this.rows("SELECT parent, deleted, json FROM boards WHERE id = ?", b.id)[0];
      if (scope) {
        // Inside the shared board only: new boards under it, renames, moves
        // within it. The shared board itself can't be moved or deleted.
        const inside = old ? scope.has(b.id) : true;
        const root = b.id === a.board;
        const parentOk = root ? (b.parent ?? null) === (old?.parent ?? null) : b.parent != null && scope.has(b.parent);
        if (!inside || !parentOk || (root && b.deleted)) {
          refused.push(b.id);
          continue;
        }
      }
      if (old && old.json === text) continue;
      // A board inside itself would vanish from every list.
      const seen = new Set([b.id]);
      for (let up = b.parent ?? null; up; up = this.rows("SELECT parent FROM boards WHERE id = ?", up)[0]?.parent ?? null) {
        if (seen.has(up)) {
          up = null;
          seen.add("loop");
          break;
        }
        seen.add(up);
      }
      if (seen.has("loop")) {
        refused.push(b.id);
        continue;
      }
      this.sql.exec("INSERT INTO boards VALUES (?, ?, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET parent = excluded.parent, deleted = excluded.deleted, json = excluded.json, seq = excluded.seq", b.id, b.parent ?? null, b.deleted ? 1 : 0, text, next());
      if (b.deleted && !old?.deleted) this.sql.exec("INSERT OR REPLACE INTO deleted_at VALUES (?, ?)", b.id, Date.now());
      if (!b.deleted && old?.deleted) this.sql.exec("DELETE FROM deleted_at WHERE board = ?", b.id);
      if (scope) scope = this.scope(a);
    }
    const topRev = new Map(); // board -> highest rev on it, for links
    for (const e of items) {
      const i = e && e.item;
      if (!i || !UUID.test(e.board) || !UUID.test(i.id) || !Number.isSafeInteger(i.rev) || i.rev < 0) throw new Refuse(400, "bad item");
      if (scope && !scope.has(e.board)) {
        refused.push(i.id);
        continue;
      }
      // Every copy drops revs from 2^52 up, and nothing could ever replace
      // such an item. A link's revs may only run a little ahead of the
      // board's, so it can't make an item nobody else can change.
      let tooFar = i.rev >= MAX_REV || (typeof i.updated_by === "string" && i.updated_by.length > 200);
      if (!tooFar && a.role !== "owner") {
        if (!topRev.has(e.board)) topRev.set(e.board, this.rows("SELECT COALESCE(MAX(rev), 0) AS r FROM items WHERE board = ?", e.board)[0].r);
        tooFar = i.rev > topRev.get(e.board) + REV_STEP;
      }
      if (tooFar) {
        refused.push(i.id);
        continue;
      }
      const by = typeof i.updated_by === "string" ? i.updated_by : "";
      const hash = i.type === "media" && HASH.test(i.hash) ? i.hash : null;
      const old = this.rows("SELECT rev, updated_by, hash, json FROM items WHERE board = ? AND id = ?", e.board, i.id)[0];
      if (old && !(i.rev > old.rev || (i.rev === old.rev && by > old.updated_by))) continue;
      if (a.role === "view" && !viewMayWrite(old && JSON.parse(old.json), i, a.person)) {
        refused.push(i.id);
        continue;
      }
      // A link mustn't reach a file by naming its hash in a new item: it may
      // only use files it can already read or that it uploads itself.
      if (scope && hash && hash !== old?.hash && !this.mayUse(a, scope, hash)) {
        refused.push(i.id);
        continue;
      }
      const text = JSON.stringify(i);
      if (text.length > MAX_ROW) throw new Refuse(413, "item too big");
      if (a.role !== "owner") topRev.set(e.board, Math.max(topRev.get(e.board) ?? 0, i.rev));
      this.sql.exec("INSERT INTO items VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (board, id) DO UPDATE SET rev = excluded.rev, updated_by = excluded.updated_by, hash = excluded.hash, deleted = excluded.deleted, json = excluded.json, seq = excluded.seq", e.board, i.id, i.rev, by, hash, i.deleted ? 1 : 0, text, next());
    }
    for (const s of sidecars) {
      if (!s || !HASH.test(s.hash) || !EXT.test(s.ext)) throw new Refuse(400, "bad picture details");
      if (scope && !this.mayUse(a, scope, s.hash)) {
        refused.push(s.hash);
        continue;
      }
      const text = JSON.stringify(s);
      if (text.length > MAX_ROW) throw new Refuse(413, "details too big");
      const old = this.rows("SELECT json FROM sidecars WHERE hash = ?", s.hash)[0];
      if (old && old.json === text) continue;
      this.sql.exec("INSERT INTO sidecars VALUES (?, ?, ?) ON CONFLICT (hash) DO UPDATE SET json = excluded.json, seq = excluded.seq", s.hash, text, next());
    }
    return refused;
  }

  // May this link put `hash` on a board? Yes if it can read it already, or
  // the server doesn't have it yet (then the link has to upload it).
  mayUse(a, scope, hash) {
    if (this.hashes(a, scope).has(hash) || this.granted(a, hash)) return true;
    return !this.rows("SELECT 1 FROM media WHERE hash = ? AND done = 1", hash)[0];
  }

  // Every item of one board, for a board a link has just come to see.
  async boardItems(who, board, after) {
    const a = this.access(who);
    if (!a) return no(403, "this link doesn't work (any more)");
    if (!UUID.test(board || "")) return no(400, "bad board");
    const scope = this.scope(a);
    if (scope && !scope.has(board)) return no(404, "no such board here");
    const reads = this.hashes(a, scope);
    const out = { items: [], sidecars: [], after: "", more: false };
    let chars = 0;
    for (const r of this.sql.exec("SELECT id, hash, json FROM items WHERE board = ? AND id > ? ORDER BY id LIMIT ?", board, after, PAGE + 1)) {
      if (out.items.length >= PAGE || (out.items.length && chars + r.json.length > PAGE_CHARS)) {
        out.more = true;
        break;
      }
      chars += r.json.length;
      out.items.push({ board, item: JSON.parse(r.json) });
      out.after = r.id;
      if (r.hash && (!reads || reads.has(r.hash))) {
        const s = this.rows("SELECT json FROM sidecars WHERE hash = ?", r.hash)[0];
        if (s) out.sidecars.push(JSON.parse(s.json));
      }
    }
    return out;
  }

  async mayRead(who, hash) {
    const a = this.access(who);
    if (!a) return no(403, "this link doesn't work (any more)");
    const reads = this.hashes(a, this.scope(a));
    const m = this.rows("SELECT size FROM media WHERE hash = ? AND done = 1", hash)[0];
    // The same answer whether it's missing or merely not theirs.
    if (!m || (reads && !reads.has(hash))) return no(404, "no such file here");
    return { size: m.size };
  }

  async reserve(who, hash, size, replace) {
    const a = this.access(who);
    if (!a) return no(403, "this link doesn't work (any more)");
    if (a.role === "view") return no(403, "this link can only look");
    if (replace && a.role !== "owner") return no(403, "only the owner can replace a file");
    const old = this.rows("SELECT size, done, at FROM media WHERE hash = ?", hash)[0];
    if (old && old.done && !replace) {
      // A board link that can't see this file must send the same bytes
      // before it may use it: naming a hash gives nothing away.
      const reads = this.hashes(a, this.scope(a));
      if (reads && !reads.has(hash) && !this.granted(a, hash)) return size === old.size ? { prove: true, size: old.size } : no(409, "not the same file");
      return { have: true };
    }
    if (old && !old.done && old.at > Date.now() - 60_000) return no(409, "being uploaded already");
    const used = this.used() - (old && (old.done || old.at > Date.now() - PENDING_MS) ? old.size : 0);
    if (used + size > this.max()) return no(507, "the cloud space Wall may use is full");
    const ticket = crypto.randomUUID();
    this.sql.exec("INSERT INTO media VALUES (?, ?, 0, ?, ?) ON CONFLICT (hash) DO UPDATE SET size = excluded.size, done = 0, ticket = excluded.ticket, at = excluded.at", hash, size, ticket, Date.now());
    return { ticket };
  }

  async commit(who, hash, ticket) {
    this.sql.exec("UPDATE media SET done = 1 WHERE hash = ? AND ticket = ?", hash, ticket);
    return this.grant(who, hash);
  }

  // The link `who` has this file: it may put it on its boards (reading it
  // still needs a picture on one of them).
  async grant(who, hash) {
    const a = this.access(who);
    if (!a || !a.link) return;
    this.sql.exec("INSERT OR IGNORE INTO grants VALUES (?, ?)", a.link, hash);
  }

  granted(a, hash) {
    return !!a.link && !!this.rows("SELECT 1 FROM grants WHERE link = ? AND hash = ?", a.link, hash)[0];
  }

  async release(hash, ticket) {
    this.sql.exec("DELETE FROM media WHERE hash = ? AND ticket = ? AND done = 0", hash, ticket);
  }

  // ---- history ------------------------------------------------------------

  // Keeps each of these boards as it is now as a version, if its last one
  // was looked at VERSION_EVERY_MS ago. Item rows are joined as they're
  // stored (not parsed), so this costs little CPU.
  async keepVersions(boards) {
    const now = Date.now();
    if (this.sql.databaseSize > VERSIONS_UNTIL_DB) return;
    for (const id of boards) {
      if (now - (this.taken.get(id) || 0) < VERSION_EVERY_MS) continue;
      const b = this.rows("SELECT json FROM boards WHERE id = ? AND deleted = 0", id)[0];
      if (!b) continue;
      const parts = [];
      let seq = 0;
      for (const r of this.sql.exec("SELECT id, json, seq FROM items WHERE board = ? AND deleted = 0", id)) {
        parts.push(`${JSON.stringify(r.id)}:${r.json}`);
        if (r.seq > seq) seq = r.seq;
      }
      // A new or empty board: nothing to go back to yet.
      if (!parts.length) continue;
      if (this.taken.size > 2000) this.taken.clear();
      this.taken.set(id, now);
      const last = this.rows("SELECT seq FROM versions WHERE board = ? ORDER BY at DESC LIMIT 1", id)[0];
      if (last && last.seq === seq) continue;
      const meta = JSON.parse(b.json);
      const head = { version: 1, id, name: typeof meta.name === "string" ? meta.name : "", ...(meta.parent ? { parent: meta.parent } : {}), created_ms: Number(meta.created_ms) || 0, updated_ms: now };
      const text = `${JSON.stringify(head).slice(0, -1)},"items":{${parts.join(",")}}}`;
      const gz = await gzip(text);
      if (gz.length > MAX_VERSION) continue;
      this.ctx.storage.transactionSync(() => {
        for (let k = 0; k * PIECE < gz.length; k++) this.sql.exec("INSERT OR REPLACE INTO versions VALUES (?, ?, ?, ?, ?)", id, now, k, seq, gz.subarray(k * PIECE, (k + 1) * PIECE));
        const times = this.rows("SELECT DISTINCT at FROM versions WHERE board = ?", id).map((r) => r.at);
        for (const t of thin(times, now)) this.sql.exec("DELETE FROM versions WHERE board = ? AND at = ?", id, t);
      });
    }
  }

  // Owners and edit links only, on boards they reach.
  historyAccess(who, board) {
    const a = this.access(who);
    if (!a) return [null, no(403, "this link doesn't work (any more)")];
    if (a.role === "view") return [null, no(403, "this link can only look")];
    if (board !== undefined && (!UUID.test(board || "") || !this.reaches(a, board))) return [null, no(404, "no such board here")];
    return [a, null];
  }

  async versions(who, board) {
    const [, err] = this.historyAccess(who, board);
    if (err) return err;
    return { times: this.rows("SELECT DISTINCT at FROM versions WHERE board = ? ORDER BY at DESC", board).map((r) => r.at) };
  }

  async version(who, board, at) {
    const [, err] = this.historyAccess(who, board);
    if (err) return err;
    const parts = this.rows("SELECT data FROM versions WHERE board = ? AND at = ? ORDER BY part", board, Number(at)).map((r) => new Uint8Array(r.data));
    if (!parts.length) return no(404, "no such version");
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let k = 0;
    for (const p of parts) {
      out.set(p, k);
      k += p.length;
    }
    return { bytes: out };
  }

  // Deleted boards this access could bring back: those whose parent it
  // reaches (or, for the whole library, whose parent is still there). Boards
  // deleted inside another deleted one come back with it and are counted.
  restorable(a) {
    const scope = this.scope(a);
    const all = this.rows("SELECT b.id, b.parent, b.json, COALESCE(d.at, 0) AS at FROM boards b LEFT JOIN deleted_at d ON d.board = b.id WHERE b.deleted = 1");
    const gone = new Map(all.map((r) => [r.id, r]));
    const out = [];
    for (const r of all) {
      const ok = scope ? r.parent != null && scope.has(r.parent) : r.parent == null || !gone.has(r.parent);
      if (!ok) continue;
      out.push({ ...r, inside: this.deletedInside(r, gone) });
    }
    return out;
  }

  // Deleted boards inside `top` deleted at about the same time (within a
  // minute; any time if that isn't known).
  deletedInside(top, gone) {
    const inside = [];
    const todo = [top.id];
    while (todo.length) {
      const p = todo.pop();
      for (const r of gone.values()) {
        if (r.parent !== p || inside.includes(r) || r.id === top.id) continue;
        if (top.at && r.at && Math.abs(r.at - top.at) > 60_000) continue;
        inside.push(r);
        todo.push(r.id);
      }
    }
    return inside;
  }

  async deletedBoards(who) {
    const [a, err] = this.historyAccess(who);
    if (err) return err;
    const list = this.restorable(a)
      .sort((x, y) => y.at - x.at)
      .slice(0, 100)
      .map((r) => {
        const meta = JSON.parse(r.json);
        const items = this.rows("SELECT COUNT(*) AS n FROM items WHERE board = ? AND deleted = 0", r.id)[0].n;
        return { id: r.id, name: typeof meta.name === "string" ? meta.name : "", parent: r.parent, deleted_ms: r.at, items, inside: r.inside.length };
      });
    return list;
  }

  // Brings a deleted board back, with the boards deleted inside it and its
  // card on the board it was on. Its items get new seqs so everyone pulls
  // them again.
  async undelete(who, req) {
    const [a, err] = this.historyAccess(who);
    if (err) return err;
    const id = req?.board;
    if (!UUID.test(id || "")) return no(400, "bad board");
    const top = this.restorable(a).find((r) => r.id === id);
    if (!top) return no(404, "that board can't be brought back from here");
    const now = Date.now();
    this.ctx.storage.transactionSync(() => {
      const next = () => {
        this.sql.exec("UPDATE counter SET seq = seq + 1");
        return this.seq();
      };
      for (const r of [top, ...top.inside]) {
        const meta = JSON.parse(r.json);
        delete meta.deleted;
        this.sql.exec("UPDATE boards SET deleted = 0, json = ?, seq = ? WHERE id = ?", JSON.stringify(meta), next(), r.id);
        this.sql.exec("DELETE FROM deleted_at WHERE board = ?", r.id);
        for (const i of this.rows("SELECT id FROM items WHERE board = ?", r.id)) this.sql.exec("UPDATE items SET seq = ? WHERE board = ? AND id = ?", next(), r.id, i.id);
      }
      // Its card, deleted with it, comes back as a newer revision.
      if (top.parent) {
        const card = this.rows("SELECT id, rev, json FROM items WHERE board = ? AND deleted = 1 AND instr(json, ?) > 0 ORDER BY rev DESC LIMIT 1", top.parent, `"board_id":"${id}"`)[0];
        if (card) {
          const i = JSON.parse(card.json);
          if (i.type === "board" && i.board_id === id) {
            delete i.deleted;
            i.rev = card.rev + 1;
            i.updated_ms = now;
            i.updated_by = "~restore";
            this.sql.exec("UPDATE items SET rev = ?, updated_by = ?, deleted = 0, json = ?, seq = ? WHERE board = ? AND id = ?", i.rev, i.updated_by, JSON.stringify(i), next(), top.parent, card.id);
          }
        }
      }
    });
    this.readable.clear();
    this.recheck();
    this.poke();
    return { restored: [id, ...top.inside.map((r) => r.id)] };
  }

  async listLinks(who) {
    if (this.access(who)?.role !== "owner") return no(403, "owner only");
    return { links: this.rows("SELECT id, role, board, label, person, created FROM links WHERE revoked = 0 ORDER BY created") };
  }

  async makeLink(who, req) {
    if (this.access(who)?.role !== "owner") return no(403, "owner only");
    const role = req?.role;
    const board = req?.board ?? null;
    if (role !== "view" && role !== "edit") return no(400, "view or edit");
    if (board !== null && !UUID.test(board)) return no(400, "bad board");
    if (board !== null && !this.rows("SELECT 1 FROM boards WHERE id = ? AND deleted = 0", board)[0]) return no(404, "sync that board first");
    const secret = newSecret();
    const id = crypto.randomUUID();
    const person = cleanName(req.person, "");
    this.sql.exec(
      "INSERT INTO links (id, secret, role, board, label, created, revoked, person) VALUES (?, ?, ?, ?, ?, ?, 0, ?)",
      id, await sha256(secret), role, board, String(req.label || "").slice(0, 100), Date.now(), person,
    );
    return { id, secret, role, board, person };
  }

  async revokeLink(who, id) {
    if (this.access(who)?.role !== "owner") return no(403, "owner only");
    const secret = this.rows("SELECT secret FROM links WHERE id = ?", id)[0]?.secret;
    this.sql.exec("UPDATE links SET revoked = 1 WHERE id = ?", id);
    this.sql.exec("DELETE FROM grants WHERE link = ?", id);
    // Whoever is connected with it is gone too.
    for (const [ws, a] of this.live()) {
      if (secret && a.who === secret) {
        if (a.board) this.tell(a.board, { t: "leave", id: a.id }, ws);
        ws.serializeAttachment({ id: a.id, who: null });
        ws.close(4003, "this link doesn't work any more");
      }
    }
    return { revoked: true };
  }
}

export class MediaDO extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    ctx.blockConcurrencyWhile(async () => {
      this.sql.exec("CREATE TABLE IF NOT EXISTS pieces (hash TEXT NOT NULL, n INTEGER NOT NULL, bytes BLOB NOT NULL, PRIMARY KEY (hash, n)) WITHOUT ROWID");
    });
  }

  async write(hash, bytes) {
    this.ctx.storage.transactionSync(() => {
      this.sql.exec("DELETE FROM pieces WHERE hash = ?", hash);
      for (let n = 0; n * PIECE < bytes.byteLength; n++) {
        this.sql.exec("INSERT INTO pieces VALUES (?, ?, ?)", hash, n, bytes.slice(n * PIECE, (n + 1) * PIECE));
      }
    });
  }

  async read(hash) {
    const parts = this.sql.exec("SELECT bytes FROM pieces WHERE hash = ? ORDER BY n", hash).toArray().map((r) => new Uint8Array(r.bytes));
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let at = 0;
    for (const p of parts) {
      out.set(p, at);
      at += p.length;
    }
    return out;
  }
}
