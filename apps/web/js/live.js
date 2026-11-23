// The live connection for a page opened by a share link: who else is on the
// board, where their pointers are, and a nudge when something changed so the
// page pulls at once. Rust asks for news each frame (liveTake) and says
// where this person's pointer is (liveCursor).

let key = null;
let ws = null;
let board = null;
let cursor = null; // [x, y] or null, the latest to send
let sentCursor = undefined;
let cursorTimer = null;
let peers = new Map(); // id -> { id, name, x, y }
let changed = false;
let seq = 0; // last change the server announced
let pokeWaiters = [];
let retry = 1000;
let stopped = false;

const NAME_KEY = "wall.name";

function myName() {
  try {
    return localStorage.getItem(NAME_KEY) || "Guest";
  } catch {
    return "Guest";
  }
}

function send(m) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(m));
}

export function liveStart(k) {
  key = k;
  connect();
  askName();
  // A background tab doesn't need pointers; the poll loop slows down too.
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && (!ws || ws.readyState > WebSocket.OPEN)) connect();
  });
}

function connect() {
  if (stopped || !key) return;
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const s = new WebSocket(`${proto}//${location.host}/api/live`);
  ws = s;
  let pinger = null;
  s.onopen = () => {
    retry = 1000;
    s.send(JSON.stringify({ t: "hello", key, board, name: myName() }));
    sentCursor = undefined;
    // Answered by the server without waking it up (not billed).
    pinger = setInterval(() => s.readyState === WebSocket.OPEN && s.send("ping"), 30000);
  };
  s.onmessage = (e) => {
    if (e.data === "pong") return;
    let m;
    try {
      m = JSON.parse(e.data);
    } catch {
      return;
    }
    if (m.t === "welcome" || m.t === "poke") {
      seq = Math.max(seq, m.seq || 0);
      const w = pokeWaiters;
      pokeWaiters = [];
      for (const f of w) f(seq);
    } else if (m.t === "peers") {
      if (m.board !== board) return;
      peers = new Map((m.peers || []).map((p) => [p.id, p]));
      changed = true;
    } else if (m.t === "peer") {
      peers.set(m.peer.id, m.peer);
      changed = true;
    } else if (m.t === "cursor") {
      const p = peers.get(m.id);
      if (p) {
        p.x = m.x;
        p.y = m.y;
        changed = true;
      }
    } else if (m.t === "leave") {
      peers.delete(m.id);
      changed = true;
    }
  };
  s.onclose = (e) => {
    clearInterval(pinger);
    if (ws !== s) return;
    ws = null;
    peers.clear();
    changed = true;
    // A revoked link or a bad key: don't keep knocking.
    if (e.code === 4001 || e.code === 4003) {
      stopped = true;
      return;
    }
    if (document.hidden) return; // reconnects when the tab is looked at again
    setTimeout(connect, retry);
    retry = Math.min(retry * 2, 60000);
  };
}

// The board this page shows now.
export function liveBoard(id) {
  if (id === board) return;
  board = id;
  peers.clear();
  changed = true;
  send({ t: "board", board: id });
}

// This person's pointer on the board, or null; sent at most ~20 times a second.
export function liveCursor(x, y) {
  cursor = x === undefined || x === null ? null : [x, y];
  if (cursorTimer) return;
  flushCursor();
}

function flushCursor() {
  const same = sentCursor !== undefined && JSON.stringify(sentCursor) === JSON.stringify(cursor);
  if (same) {
    cursorTimer = null;
    return;
  }
  sentCursor = cursor;
  send(cursor ? { t: "cursor", x: cursor[0], y: cursor[1] } : { t: "cursor", x: null, y: null });
  cursorTimer = setTimeout(flushCursor, 50);
}

// The people on the board as [{id, name, x, y}], if that changed since last asked.
export function liveTake() {
  if (!changed) return undefined;
  changed = false;
  return JSON.stringify([...peers.values()]);
}

// Waits `ms`, or less if the server says there's a change after `have`.
export function liveWait(ms, have) {
  return new Promise((resolve) => {
    const t = setTimeout(() => resolve(false), ms);
    const f = (s) => {
      if (s > have) {
        clearTimeout(t);
        resolve(true);
      } else {
        pokeWaiters.push(f);
      }
    };
    if (seq > have) f(seq);
    else pokeWaiters.push(f);
  });
}

// Asks once for a name others will see; kept in this browser.
function askName() {
  let had = null;
  try {
    had = localStorage.getItem(NAME_KEY);
  } catch {}
  if (had) return;
  const box = document.createElement("form");
  box.style.cssText =
    "position:fixed;left:50%;transform:translateX(-50%);top:44px;z-index:10;display:flex;gap:8px;align-items:center;padding:10px 12px;border-radius:10px;font:13px system-ui,sans-serif;background:#1c1d22f2;color:#e8e9ee;border:1px solid #3a3c44;box-shadow:0 6px 24px #0006;max-width:calc(100vw - 32px)";
  const label = document.createElement("label");
  label.textContent = "Your name, so others see who's here";
  label.style.cssText = "white-space:nowrap";
  const input = document.createElement("input");
  input.maxLength = 40;
  input.placeholder = "Name";
  input.style.cssText = "width:140px;padding:5px 8px;border-radius:6px;border:1px solid #4a4d57;background:#121316;color:#e8e9ee;font:inherit";
  const ok = document.createElement("button");
  ok.textContent = "OK";
  ok.type = "submit";
  ok.style.cssText = "padding:5px 12px;border-radius:6px;border:0;background:#7aa2f7;color:#0d0e10;font:inherit;font-weight:600;cursor:pointer";
  const later = document.createElement("button");
  later.textContent = "Not now";
  later.type = "button";
  later.style.cssText = "padding:5px 8px;border-radius:6px;border:0;background:transparent;color:#9a9ea6;font:inherit;cursor:pointer";
  label.htmlFor = input.id = "wall-name";
  box.append(label, input, ok, later);
  // Keys typed here are for the field, not for Wall's shortcuts.
  for (const k of ["keydown", "keyup", "keypress"]) box.addEventListener(k, (e) => e.stopPropagation());
  box.addEventListener("submit", (e) => {
    e.preventDefault();
    const name = input.value.trim();
    if (!name) return input.focus();
    try {
      localStorage.setItem(NAME_KEY, name);
    } catch {}
    send({ t: "name", name });
    box.remove();
  });
  later.addEventListener("click", () => box.remove());
  document.body.append(box);
}
