// Runs the server locally (workerd, nothing on any Cloudflare account) with a
// throwaway database and a key made for this run, and checks what each kind
// of link can and can't do. Usage: npm test
import { spawn, spawnSync } from "node:child_process";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { gunzipSync } from "node:zlib";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(here);
const port = 8795;
const base = `http://127.0.0.1:${port}/api/`;
const owner = randomBytes(32).toString("base64url");
const data = mkdtempSync(join(tmpdir(), "wall-server-"));
const sha = (t) => createHash("sha256").update(t).digest("hex");

const server = spawn(
  process.execPath,
  [join(root, "node_modules/wrangler/bin/wrangler.js"), "dev", "--local", "--ip", "127.0.0.1", "--port", String(port), "--persist-to", data, "--var", `OWNER_HASH:${sha(owner)}`, "--var", "MAX_MEDIA_BYTES:6000000", "--show-interactive-dev-session=false"],
  { cwd: root, env: { ...process.env, WRANGLER_SEND_METRICS: "false", CI: "1" }, stdio: ["ignore", "pipe", "pipe"] },
);
let log = "";
server.stdout.on("data", (d) => (log += d));
server.stderr.on("data", (d) => (log += d));

async function call(key, path, { method = "GET", json, body } = {}) {
  const headers = {};
  if (key) headers.authorization = `Bearer ${key}`;
  if (json !== undefined) body = JSON.stringify(json);
  const r = await fetch(base + path, { method, headers, body, duplex: "half" });
  const type = r.headers.get("content-type") || "";
  return { status: r.status, headers: r.headers, data: method !== "HEAD" && type.includes("json") ? await r.json() : Buffer.from(await r.arrayBuffer()) };
}
const sync = (key, json) => call(key, "sync", { method: "POST", json });
const board = (id, parent = null, name = "B", more = {}) => ({ version: 1, id, name, ...(parent ? { parent } : {}), created_ms: 1, updated_ms: 1, ...more });
const note = (id, rev, text, by = "pc") => ({ id, frame: { x: 0, y: 0, w: 1, h: 1 }, type: "note", text, rev, updated_ms: rev, updated_by: by });
const picture = (id, hash, rev = 1) => ({ id, frame: { x: 0, y: 0, w: 1, h: 1 }, type: "media", hash, rev, updated_ms: rev, updated_by: "pc" });
const details = (hash, more = {}) => ({ hash, kind: "image", ext: "png", width: 1, height: 1, bytes: 1, added_ms: 1, tags: [], ...more });
const hashOf = (bytes) => createHash("sha256").update(bytes).digest("hex"); // any 64 hex digits do here

const tests = [];
const test = (name, f) => tests.push([name, f]);

const A = randomUUID(), B = randomUUID(), C = randomUUID(), PRIVATE = randomUUID();
const n1 = randomUUID(), n2 = randomUUID(), n3 = randomUUID(), p1 = randomUUID(), p2 = randomUUID();
const pub = randomBytes(2_300_000), priv = randomBytes(700_000);
const hPub = hashOf(pub), hPriv = hashOf(priv);
let view, edit, whole;

test("nothing without a key, nothing with a wrong one", async () => {
  assert.equal((await call(null, "me")).status, 401);
  assert.equal((await call(randomBytes(32).toString("base64url"), "me")).status, 403);
  assert.equal((await sync(randomBytes(32).toString("base64url"), { since: 0 })).status, 403);
  assert.equal((await call("short", "me")).status, 401);
  // The stored hash is not a key.
  assert.equal((await call(sha(owner), "me")).status, 403);
  assert.equal((await call(owner, "me")).data.role, "owner");
});

test("push, pull, and only what changed since", async () => {
  const r = await sync(owner, { since: 0, boards: [board(A), board(B, A), board(PRIVATE)], items: [{ board: A, item: note(n1, 1, "hello") }, { board: PRIVATE, item: note(n2, 1, "secret") }] });
  assert.equal(r.status, 200);
  assert.equal(r.data.boards.length, 3);
  assert.equal(r.data.items.length, 2);
  assert.equal(r.data.more, false);
  const again = await sync(owner, { since: r.data.seq });
  assert.deepEqual([again.data.boards.length, again.data.items.length, again.data.seq], [0, 0, r.data.seq]);
  // Sending the same thing again changes nothing (free-plan writes are counted).
  const same = await sync(owner, { since: r.data.seq, boards: [board(A)], items: [{ board: A, item: note(n1, 1, "hello") }] });
  assert.equal(same.data.seq, r.data.seq);
});

test("the higher rev wins, then the larger updated_by, in any order", async () => {
  const item = async () => (await call(owner, `items?board=${A}`)).data.items.find((e) => e.item.id === n1).item.text;
  await sync(owner, { since: 0, items: [{ board: A, item: note(n1, 5, "five", "a") }] });
  await sync(owner, { since: 0, items: [{ board: A, item: note(n1, 3, "old", "z") }] });
  assert.equal(await item(), "five");
  await sync(owner, { since: 0, items: [{ board: A, item: note(n1, 5, "tie z", "z") }] });
  assert.equal(await item(), "tie z");
  await sync(owner, { since: 0, items: [{ board: A, item: note(n1, 5, "tie a", "a") }] });
  assert.equal(await item(), "tie z");
});

test("a file goes up in pieces and comes back the same, never as a web page", async () => {
  await sync(owner, { since: 0, items: [{ board: A, item: picture(p1, hPub) }, { board: PRIVATE, item: picture(p2, hPriv) }], sidecars: [details(hPub), details(hPriv)] });
  assert.equal((await call(owner, `media/${hPub}`)).status, 404);
  assert.equal((await call(owner, `media/${hPub}`, { method: "PUT", body: pub })).data.stored, true);
  assert.equal((await call(owner, `media/${hPriv}`, { method: "PUT", body: priv })).data.stored, true);
  assert.equal((await call(owner, `media/${hPub}`, { method: "PUT", body: pub })).data.have, true);
  const r = await call(owner, `media/${hPub}`);
  assert.equal(r.status, 200);
  assert.ok(r.data.equals(pub));
  assert.equal(r.headers.get("content-type"), "application/octet-stream");
  assert.equal(r.headers.get("x-content-type-options"), "nosniff");
  assert.match(r.headers.get("content-security-policy"), /sandbox/);
  assert.equal(r.headers.get("access-control-allow-origin"), null);
});

test("uploads stop at the cap, and a refused one leaves nothing behind", async () => {
  const big = randomBytes(3_500_000);
  const r = await call(owner, `media/${hashOf(big)}`, { method: "PUT", body: big });
  assert.equal(r.status, 507);
  assert.equal((await call(owner, "me")).data.media_bytes, pub.length + priv.length);
  const fits = randomBytes(1000);
  assert.equal((await call(owner, `media/${hashOf(fits)}`, { method: "PUT", body: fits })).status, 200);
});

test("many at once can't slip past the cap together", async () => {
  // About 3.0 MB left; ten files of 0.9 MB: three fit.
  const files = Array.from({ length: 10 }, () => randomBytes(900_000));
  const r = await Promise.all(files.map((f) => call(owner, `media/${hashOf(f)}`, { method: "PUT", body: f })));
  assert.equal(r.filter((x) => x.status === 200).length, 3);
  assert.ok((await call(owner, "me")).data.media_bytes <= 6_000_000);
});

test("only the owner makes, lists and revokes links", async () => {
  view = (await call(owner, "links", { method: "POST", json: { role: "view", board: A, label: "client" } })).data;
  edit = (await call(owner, "links", { method: "POST", json: { role: "edit", board: A } })).data;
  whole = (await call(owner, "links", { method: "POST", json: { role: "view" } })).data;
  for (const k of [view.secret, edit.secret, whole.secret]) {
    assert.equal((await call(k, "links")).status, 403);
    assert.equal((await call(k, "links", { method: "POST", json: { role: "edit" } })).status, 403);
    assert.equal((await call(k, `links/${view.id}`, { method: "DELETE" })).status, 403);
    assert.equal("media_bytes" in (await call(k, "me")).data, false);
  }
  const list = (await call(owner, "links")).data.links;
  assert.equal(list.length, 3);
  assert.ok(list.every((l) => !("secret" in l)), "secrets are shown once, when made");
  assert.equal((await call(owner, "links", { method: "POST", json: { role: "owner" } })).status, 400);
  assert.equal((await call(owner, "links", { method: "POST", json: { role: "view", board: randomUUID() } })).status, 404);
});

test("a board link sees that board and those inside it, nothing else", async () => {
  for (const k of [view.secret, edit.secret]) {
    const r = (await sync(k, { since: 0 })).data;
    assert.deepEqual(r.boards.map((b) => b.id).sort(), [A, B].sort());
    assert.ok(r.items.every((e) => e.board === A || e.board === B));
    assert.deepEqual(r.sidecars.map((s) => s.hash), [hPub]);
    assert.ok(!JSON.stringify(r).includes("secret"));
    assert.ok(!JSON.stringify(r).includes(PRIVATE));
    assert.equal((await call(k, `items?board=${PRIVATE}`)).status, 404);
    assert.equal((await call(k, `media/${hPub}`)).status, 200);
    assert.equal((await call(k, `media/${hPriv}`)).status, 404);
    assert.equal((await call(k, `media/${hPriv}`, { method: "HEAD" })).status, 404);
  }
  const all = (await sync(whole.secret, { since: 0 })).data;
  assert.equal(all.boards.length, 3);
  assert.equal((await call(whole.secret, `media/${hPriv}`)).status, 200);
});

test("a view link can't change anything", async () => {
  for (const k of [view.secret, whole.secret]) {
    assert.equal((await sync(k, { since: 0, items: [{ board: A, item: note(n1, 99, "hacked", "zz") }] })).status, 403);
    assert.equal((await sync(k, { since: 0, boards: [board(A, null, "hacked")] })).status, 403);
    assert.equal((await sync(k, { since: 0, sidecars: [details(hPub, { title: "x" })] })).status, 403);
    assert.equal((await call(k, `media/${hashOf("q")}`, { method: "PUT", body: "q" })).status, 403);
  }
});

test("a view link can comment and stamp, and not change what others wrote", async () => {
  const k = view.secret;
  const c = randomUUID(), st = randomUUID();
  const msg = (by, text) => ({ by, text, at_ms: 1 });
  const comment = (rev, messages, more = {}) => ({ id: c, frame: { x: 5, y: 5, w: 0, h: 0 }, type: "comment", messages, rev, updated_ms: rev, updated_by: "web-1", ...more });
  const ok = async (item) => {
    const r = await sync(k, { since: 0, items: [{ board: A, item }] });
    assert.equal(r.status, 200);
    return r.data.refused;
  };
  assert.deepEqual(await ok(comment(1, [msg("Mia", "Love it")])), []);
  assert.deepEqual(await ok(comment(2, [msg("Mia", "Love it"), msg("Mia", "More of this")])), [], "a reply");
  assert.deepEqual(await ok(comment(3, [msg("Mia", "Love it"), msg("Mia", "More of this")], { resolved: true })), [], "resolved");
  assert.deepEqual(await ok(comment(4, [msg("Mia", "Hate it")])), [c], "can't rewrite a message");
  assert.deepEqual(await ok(comment(4, [])), [c], "can't drop messages");
  assert.deepEqual(await ok(comment(4, [msg("Mia", "Love it"), msg("Mia", "More of this")], { deleted: true })), [c], "can't delete the thread");
  assert.equal((await sync(k, { since: 0, items: [{ board: A, item: note(c, 5, "x") }] })).status, 403, "only comments and stamps");
  // Stamps: put down, moved, taken away.
  const stamp = (rev, more = {}) => ({ id: st, frame: { x: 0, y: 0, w: 40, h: 40 }, type: "stamp", mark: "♥", by: "Mia", rev, updated_ms: rev, updated_by: "web-1", ...more });
  assert.deepEqual(await ok(stamp(1)), []);
  assert.deepEqual(await ok(stamp(2, { frame: { x: 9, y: 9, w: 40, h: 40 } })), []);
  assert.deepEqual(await ok(stamp(3, { deleted: true })), []);
  // Not someone's note turned into a stamp, nor a comment on another board.
  assert.deepEqual(await ok({ ...stamp(100), id: n1 }), [n1]);
  assert.deepEqual(await ok({ ...comment(1, [msg("x", "y")]), id: randomUUID() }).then(() => "same board"), "same board");
  let r = await sync(k, { since: 0, items: [{ board: PRIVATE, item: { ...comment(1, [msg("x", "y")]), id: randomUUID() } }] });
  assert.equal(r.data.refused.length, 1, "only its own board");
  // The owner sees the thread as it was before the refused changes.
  const all = (await sync(owner, { since: 0 })).data.items.map((e) => e.item);
  const got = all.find((i) => i.id === c);
  assert.deepEqual([got.messages.length, got.resolved, !!got.deleted], [2, true, false]);
  assert.equal(all.find((i) => i.id === n1).type, "note");
});

test("an edit link changes its board and nothing outside it", async () => {
  const k = edit.secret;
  let r = await sync(k, { since: 0, items: [{ board: B, item: note(n3, 1, "from client", "client") }] });
  assert.deepEqual([r.status, r.data.refused.length], [200, 0]);
  r = await sync(k, { since: 0, boards: [board(C, B, "client's board")] });
  assert.equal(r.data.refused.length, 0);
  // Not: another board's items, its name, taking it over, moving boards in or out.
  const bad = [
    { items: [{ board: PRIVATE, item: note(n2, 99, "hacked", "zz") }] },
    { boards: [board(PRIVATE, null, "hacked")] },
    { boards: [board(PRIVATE, A, "pulled in")] },
    { boards: [board(B, null, "pushed out")] },
    { boards: [board(B, PRIVATE, "pushed out")] },
    { boards: [board(A, null, "gone", { deleted: true })] },
    { boards: [board(A, PRIVATE)] },
    { boards: [board(randomUUID(), null, "top-level")] },
    { boards: [board(randomUUID(), PRIVATE, "inside private")] },
  ];
  for (const b of bad) {
    r = await sync(k, { since: 0, ...b });
    assert.equal(r.data.refused.length, 1, JSON.stringify(b));
  }
  assert.equal((await call(k, `items?board=${PRIVATE}`)).status, 404);
  const mine = (await sync(owner, { since: 0 })).data;
  assert.equal(mine.boards.find((b) => b.id === PRIVATE).name, "B");
  assert.equal(mine.items.find((e) => e.item.id === n2).item.text, "secret");
});

test("an edit link can't read a private file by naming its hash", async () => {
  const k = edit.secret;
  const r = await sync(k, { since: 0, items: [{ board: A, item: picture(randomUUID(), hPriv) }], sidecars: [details(hPriv, { title: "mine now" })] });
  assert.equal(r.data.refused.length, 2);
  assert.equal((await call(k, `media/${hPriv}`)).status, 404);
  assert.equal((await call(k, `media/${hPriv}?replace=1`, { method: "PUT", body: priv })).status, 403);
});

test("an edit link that has the same file may use it; wrong bytes give nothing", async () => {
  const k = edit.secret;
  // Same length, other bytes: refused, still unreadable, still unusable.
  const fake = Buffer.from(priv);
  fake[123] ^= 1;
  assert.equal((await call(k, `media/${hPriv}`, { method: "PUT", body: fake })).status, 409);
  assert.equal((await call(k, `media/${hPriv}`, { method: "PUT", body: priv.subarray(1) })).status, 409);
  assert.equal((await call(k, `media/${hPriv}`)).status, 404);
  const pic = randomUUID();
  assert.equal((await sync(k, { since: 0, items: [{ board: A, item: picture(pic, hPriv) }] })).data.refused.length, 1);
  // The same bytes: the link may now put the file on its board.
  assert.equal((await call(k, `media/${hPriv}`, { method: "PUT", body: priv })).data.have, true);
  const r = await sync(k, { since: 0, items: [{ board: A, item: picture(pic, hPriv) }], sidecars: [details(hPriv)] });
  assert.equal(r.data.refused.length, 0);
  assert.equal((await call(k, `media/${hPriv}`)).status, 200);
  // Only for that link: the view link of the same board sees it because
  // it's on the board now, the whole-library one always could, a new
  // board link elsewhere can't.
  assert.equal((await call(view.secret, `media/${hPriv}`)).status, 200);
  const other = (await call(owner, "links", { method: "POST", json: { role: "edit", board: C } })).data;
  if (other.secret) assert.equal((await call(other.secret, `media/${hPriv}`)).status, 404);
  // Tidy up for the tests after: the picture goes again.
  await sync(owner, { since: 0, items: [{ board: A, item: { ...picture(pic, hPriv, 2), deleted: true } }] });
});

test("a new file uploaded through a board link may go on its board after", async () => {
  const k = edit.secret;
  const bytes = randomBytes(5000);
  const h = hashOf(bytes);
  assert.equal((await call(k, `media/${h}`, { method: "PUT", body: bytes })).data.stored, true);
  const r = await sync(k, { since: 0, items: [{ board: A, item: picture(randomUUID(), h) }], sidecars: [details(h)] });
  assert.equal(r.data.refused.length, 0, "upload first, then the picture");
});

test("a board can't be put inside itself", async () => {
  for (const b of [board(A, B, "loop"), board(A, C, "loop"), board(B, B, "self")]) {
    const r = await sync(owner, { since: 0, boards: [b] });
    assert.ok(r.status === 400 || r.data.refused.length === 1);
  }
});

test("names that would reach outside the library folder are refused", async () => {
  for (const ext of ["../../x", "png/..", "exe", "", "PNG\u0000", "a".repeat(50)]) {
    assert.equal((await sync(owner, { since: 0, sidecars: [details(hPub, { ext })] })).status, 400, ext);
  }
  assert.equal((await sync(owner, { since: 0, boards: [board("../x")] })).status, 400);
  assert.equal((await sync(owner, { since: 0, items: [{ board: "../x", item: note(n1, 1, "") }] })).status, 400);
  assert.equal((await call(owner, "media/..%2f..%2fx")).status, 404);
});

test("bad requests are refused whole: nothing half-saved", async () => {
  const before = (await call(owner, "me")).data.seq;
  const id = randomUUID();
  const r = await sync(owner, { since: 0, items: [{ board: A, item: note(id, 1, "first") }, { board: A, item: { id: "nope" } }] });
  assert.equal(r.status, 400);
  assert.equal((await call(owner, "me")).data.seq, before);
  assert.equal((await call(owner, "sync", { method: "POST", body: "{not json" })).status, 400);
  assert.equal((await sync(owner, { since: 0, items: Array(501).fill({ board: A, item: note(n1, 1, "") }) })).status, 413);
  assert.equal((await sync(owner, { since: 0, items: [{ board: A, item: note(id, 1, "x".repeat(950_000)) }] })).status, 413);
});

test("long lists come in pages with nothing missing", async () => {
  const L = randomUUID();
  const ids = Array.from({ length: 1300 }, () => randomUUID());
  await sync(owner, { since: 0, boards: [board(L, null, "long")] });
  for (let k = 0; k < ids.length; k += 400) await sync(owner, { since: 1e9, items: ids.slice(k, k + 400).map((id) => ({ board: L, item: note(id, 1, "n") })) });
  const got = new Set();
  let since = 0, pages = 0, more = true;
  while (more) {
    const r = (await sync(owner, { since })).data;
    r.items.forEach((e) => got.add(e.item.id));
    assert.ok(r.seq > since || !r.more);
    [since, more] = [r.seq, r.more];
    assert.ok(++pages < 20);
  }
  assert.ok(pages >= 3 && ids.every((id) => got.has(id)));
  const one = new Set();
  let after = "";
  for (more = true; more; ) {
    const r = (await call(owner, `items?board=${L}&after=${after}`)).data;
    r.items.forEach((e) => one.add(e.item.id));
    [after, more] = [r.after, r.more];
  }
  assert.equal(one.size, 1300);
});

test("a board moved into or out of a share shows up and goes away", async () => {
  const M = randomUUID(), m1 = randomUUID();
  await sync(owner, { since: 0, boards: [board(M, PRIVATE, "moving")], items: [{ board: M, item: note(m1, 1, "x") }] });
  let r = (await sync(view.secret, { since: 0 })).data;
  assert.ok(!r.scope.includes(M));
  const since = r.seq;
  await sync(owner, { since: 0, boards: [board(M, A, "moving")] });
  r = (await sync(view.secret, { since, known: r.scope })).data;
  assert.ok(r.scope.includes(M) && r.boards.some((b) => b.id === M));
  assert.equal((await call(view.secret, `items?board=${M}`)).data.items.length, 1);
  await sync(owner, { since: 0, boards: [board(M, PRIVATE, "moving")] });
  r = (await sync(view.secret, { since: r.seq, known: r.scope })).data;
  assert.ok(!r.scope.includes(M));
  assert.equal((await call(view.secret, `items?board=${M}`)).status, 404);
});

test("deleting the last picture that uses a file closes the file to the link", async () => {
  assert.equal((await call(view.secret, `media/${hPub}`)).status, 200);
  await sync(owner, { since: 0, items: [{ board: A, item: { ...picture(p1, hPub, 2), deleted: true } }] });
  assert.equal((await call(view.secret, `media/${hPub}`)).status, 404);
  await sync(owner, { since: 0, items: [{ board: A, item: picture(p1, hPub, 3) }] });
  assert.equal((await call(view.secret, `media/${hPub}`)).status, 200);
});

// A live connection: collects what the server says; `next` waits for a
// message that matches.
function live(key, board, name) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/api/live`);
  const got = [];
  const waiters = [];
  let closed = null;
  const check = () => {
    for (const w of [...waiters]) {
      const i = got.findIndex(w.pred);
      if (i >= 0) {
        waiters.splice(waiters.indexOf(w), 1);
        w.resolve(got.splice(i, 1)[0]);
      }
    }
  };
  ws.addEventListener("message", (e) => {
    got.push(JSON.parse(e.data));
    check();
  });
  ws.addEventListener("close", (e) => {
    closed = e.code;
    got.push({ t: "closed", code: e.code });
    check();
  });
  ws.addEventListener("open", () => key !== undefined && ws.send(JSON.stringify({ t: "hello", key, board, name })));
  return {
    ws,
    send: (m) => ws.send(typeof m === "string" ? m : JSON.stringify(m)),
    next: (pred, ms = 5000) =>
      new Promise((resolve, reject) => {
        const w = { pred, resolve };
        waiters.push(w);
        check();
        setTimeout(() => reject(new Error("waited for a message: " + pred + "\n got: " + JSON.stringify(got))), ms);
      }),
    nothing: async (pred, ms = 400) => {
      await new Promise((r) => setTimeout(r, ms));
      assert.equal(got.filter(pred).length, 0, "unexpected: " + JSON.stringify(got.filter(pred)));
    },
    get closed() {
      return closed;
    },
    close: () => ws.close(),
  };
}

test("live: people on a board see each other and their pointers, and nobody else does", async () => {
  const paul = live(owner, A, "Paul");
  const w = await paul.next((m) => m.t === "welcome");
  assert.ok(w.id && Number.isInteger(w.seq));
  assert.deepEqual((await paul.next((m) => m.t === "peers")).peers, []);
  const mia = live(edit.secret, A, "  Mia‮\u0007  ");
  const seen = await mia.next((m) => m.t === "peers");
  assert.deepEqual(seen.peers.map((p) => p.name), ["Paul"]);
  const joined = await paul.next((m) => m.t === "peer");
  assert.equal(joined.peer.name, "Mia", "names are one clean line");
  // An edit link for A can't watch the private board, and isn't shown there.
  const sneak = live(edit.secret, PRIVATE, "Sneak");
  assert.deepEqual((await sneak.next((m) => m.t === "peers")).peers, []);
  const owner2 = live(owner, PRIVATE, "Owner on private");
  assert.deepEqual((await owner2.next((m) => m.t === "peers")).peers, [], "the sneak isn't on the private board");

  mia.send({ t: "cursor", x: 12.5, y: -3 });
  const c = await paul.next((m) => m.t === "cursor");
  assert.deepEqual([c.id, c.x, c.y], [joined.peer.id, 12.5, -3]);
  await owner2.nothing((m) => m.t === "cursor");
  mia.send({ t: "cursor", x: "1", y: 2 });
  mia.send({ t: "cursor", x: null, y: null });
  const off = await paul.next((m) => m.t === "cursor");
  assert.deepEqual([off.x, off.y], [null, null], "bad numbers are dropped; null means off the board");

  // A push pokes everyone, so they pull now.
  await sync(owner, { since: 0, items: [{ board: A, item: note(randomUUID(), 1, "poke") }] });
  const poke = await mia.next((m) => m.t === "poke");
  assert.equal(poke.seq, (await call(owner, "me")).data.seq);
  await paul.next((m) => m.t === "poke");

  // Moving to another board: the old one hears they left.
  mia.send({ t: "board", board: B });
  assert.equal((await paul.next((m) => m.t === "leave")).id, joined.peer.id);
  mia.close();
  for (const x of [paul, sneak, owner2]) x.close();
});

test("live: no key, a wrong key or junk is shut out", async () => {
  const none = live(undefined);
  await new Promise((r) => none.ws.addEventListener("open", r));
  none.send({ t: "cursor", x: 1, y: 1 });
  assert.equal((await none.next((m) => m.t === "closed")).code, 4001);
  const wrong = live(randomBytes(32).toString("base64url"), A, "x");
  assert.equal((await wrong.next((m) => m.t === "closed")).code, 4003);
  const junk = live(owner, A, "x");
  await junk.next((m) => m.t === "peers");
  junk.send("x".repeat(5000));
  assert.equal((await junk.next((m) => m.t === "closed")).code, 1009);
  const r = await fetch(`http://127.0.0.1:${port}/api/live`);
  assert.equal(r.status, 426, "plain requests are turned away");
});

test("live: connections without a key can't keep people out", async () => {
  const opened = await Promise.all(
    Array.from({ length: 40 }, () => new Promise((r) => {
      const ws = new WebSocket(`ws://127.0.0.1:${port}/api/live`);
      ws.addEventListener("open", () => r(ws));
      ws.addEventListener("error", () => r(null));
    })),
  );
  const held = opened.filter(Boolean);
  assert.ok(held.length <= 24 && held.length > 0, `${held.length} strangers`);
  // Their time to show a key runs out when the next person comes.
  await new Promise((r) => setTimeout(r, 11_000));
  const paul = live(owner, A, "Paul");
  await paul.next((m) => m.t === "welcome");
  const guest = live(edit.secret, A, "Guest");
  await guest.next((m) => m.t === "welcome");
  await paul.next((m) => m.t === "peer");
  for (const c of [paul, guest]) c.close();
  held.forEach((ws) => ws.close());
  await new Promise((r) => setTimeout(r, 300));
});

test("live: a board moved out of a share takes its guests off it", async () => {
  const M = randomUUID();
  await sync(owner, { since: 0, boards: [board(M, A, "moves away")] });
  const paul = live(owner, M, "Paul");
  await paul.next((m) => m.t === "welcome");
  const guest = live(edit.secret, M, "Guest");
  await guest.next((m) => m.t === "welcome");
  await paul.next((m) => m.t === "peer");
  await sync(owner, { since: 0, boards: [board(M, PRIVATE, "moves away")] });
  await paul.next((m) => m.t === "leave");
  paul.send({ t: "cursor", x: 5, y: 5 });
  guest.send({ t: "cursor", x: 9, y: 9 });
  await new Promise((r) => setTimeout(r, 500));
  await guest.nothing((m) => m.t === "cursor");
  await paul.nothing((m) => m.t === "cursor");
  paul.close();
  guest.close();
  await new Promise((r) => setTimeout(r, 300));
});

test("live: no name gives the link's label; names can change", async () => {
  const temp = (await call(owner, "links", { method: "POST", json: { role: "view", board: A, label: "Nike team" } })).data;
  const paul = live(owner, A, "Paul");
  await paul.next((m) => m.t === "peers");
  const guest = live(temp.secret, A, "");
  const p = await paul.next((m) => m.t === "peer");
  assert.equal(p.peer.name, "Guest (Nike team)");
  await guest.next((m) => m.t === "peers");
  guest.send({ t: "name", name: "Mia" });
  assert.equal((await paul.next((m) => m.t === "peer")).peer.name, "Mia");
  guest.send({ t: "name", name: "guest" });
  assert.equal((await paul.next((m) => m.t === "peer")).peer.name, "Guest (Nike team)");
  await call(owner, `links/${temp.id}`, { method: "DELETE" });
  paul.close();
});

test("live: a link made for one person carries their name; colours are kept", async () => {
  const mias = (await call(owner, "links", { method: "POST", json: { role: "view", board: A, label: "x", person: "  Mia  " } })).data;
  assert.equal(mias.person, "Mia");
  assert.ok((await call(owner, "links")).data.links.some((l) => l.id === mias.id && l.person === "Mia"));
  const paul = live(owner, A, "Paul");
  await paul.next((m) => m.t === "peers");
  const mia = live(mias.secret, A, "Someone else");
  mia.ws.addEventListener("open", () => {});
  const w = await mia.next((m) => m.t === "welcome");
  assert.equal(w.name, "Mia", "told the name the link carries");
  const p = await paul.next((m) => m.t === "peer");
  assert.deepEqual([p.peer.name, p.peer.color], ["Mia", null]);
  await mia.next((m) => m.t === "peers");
  mia.send({ t: "name", name: "Not Mia", color: 3 });
  const q = await paul.next((m) => m.t === "peer");
  assert.deepEqual([q.peer.name, q.peer.color], ["Mia", 3], "the name stays, the colour changes");
  mia.send({ t: "name", color: 99 });
  assert.equal((await paul.next((m) => m.t === "peer")).peer.color, null);
  await call(owner, `links/${mias.id}`, { method: "DELETE" });
  paul.close();
});

test("live: revoking a link disconnects whoever uses it", async () => {
  const temp = (await call(owner, "links", { method: "POST", json: { role: "edit", board: A } })).data;
  const paul = live(owner, A, "Paul");
  await paul.next((m) => m.t === "peers");
  const guest = live(temp.secret, A, "Guest");
  const p = await paul.next((m) => m.t === "peer");
  await guest.next((m) => m.t === "peers");
  await call(owner, `links/${temp.id}`, { method: "DELETE" });
  assert.equal((await guest.next((m) => m.t === "closed")).code, 4003);
  assert.equal((await paul.next((m) => m.t === "leave")).id, p.peer.id);
  paul.close();
});

test("a revoked link stops at once; a deleted board's link shows nothing", async () => {
  await call(owner, `links/${view.id}`, { method: "DELETE" });
  assert.equal((await sync(view.secret, { since: 0 })).status, 403);
  assert.equal((await call(view.secret, `media/${hPub}`)).status, 403);
  assert.equal((await call(edit.secret, "me")).status, 200, "other links go on");
  await sync(owner, { since: 0, boards: [board(A, null, "B", { deleted: true })] });
  const r = (await sync(edit.secret, { since: 0 })).data;
  assert.deepEqual([r.scope, r.boards, r.items, r.sidecars], [[], [], [], []]);
  assert.equal((await call(edit.secret, `media/${hPub}`)).status, 404);
});

test("history: a board's earlier version is kept, and only editors may read it", async () => {
  const H = randomUUID(), K = randomUUID(), h1 = randomUUID(), card = randomUUID();
  const cardItem = (rev, deleted = false) => ({ id: card, frame: { x: 0, y: 0, w: 1, h: 1 }, type: "board", board_id: K, rev, updated_ms: rev, updated_by: "pc", ...(deleted ? { deleted: true } : {}) });
  await sync(owner, { since: 0, boards: [board(H)], items: [{ board: H, item: note(h1, 1, "one") }] });
  // The next change keeps the board as it was.
  await sync(owner, { since: 0, items: [{ board: H, item: note(h1, 2, "two") }] });
  const times = (await call(owner, `history?board=${H}`)).data.times;
  assert.equal(times.length, 1);
  const v = await call(owner, `history?board=${H}&at=${times[0]}`);
  assert.equal(v.headers.get("content-type"), "application/gzip");
  const kept = JSON.parse(gunzipSync(v.data).toString());
  assert.deepEqual([kept.id, kept.version, kept.items[h1].text], [H, 1, "one"]);
  // Changes within 15 minutes don't make another.
  await sync(owner, { since: 0, items: [{ board: H, item: note(h1, 3, "three") }] });
  assert.equal((await call(owner, `history?board=${H}`)).data.times.length, 1);
  assert.equal((await call(owner, `history?board=${H}&at=1`)).status, 404);

  const hEdit = (await call(owner, "links", { method: "POST", json: { role: "edit", board: H } })).data.secret;
  const hView = (await call(owner, "links", { method: "POST", json: { role: "view", board: H } })).data.secret;
  assert.equal((await call(hEdit, `history?board=${H}`)).data.times.length, 1);
  assert.equal((await call(hView, `history?board=${H}`)).status, 403, "view links only look at now");
  assert.equal((await call(hEdit, `history?board=${A}`)).status, 404, "not a board it reaches");

  // A board inside H is deleted, with its card: it can be brought back.
  await sync(owner, { since: 0, boards: [board(K, H, "Inner")], items: [{ board: H, item: cardItem(1) }, { board: K, item: note(randomUUID(), 1, "inside") }] });
  await sync(owner, { since: 0, boards: [board(K, H, "Inner", { deleted: true })], items: [{ board: H, item: cardItem(2, true) }] });
  const before = (await call(owner, "me")).data.seq;
  assert.equal((await call(hView, "deleted")).status, 403);
  const listed = (await call(hEdit, "deleted")).data;
  assert.deepEqual(listed.map((d) => [d.id, d.name, d.items, d.inside]), [[K, "Inner", 1, 0]]);
  assert.ok(Date.now() - listed[0].deleted_ms < 60_000);
  assert.equal((await call(hView, "undelete", { method: "POST", json: { board: K } })).status, 403);
  assert.equal((await call(hEdit, "undelete", { method: "POST", json: { board: A } })).status, 404);
  assert.equal((await call(hEdit, "undelete", { method: "POST", json: { board: K } })).status, 200);
  // Everyone pulls it again: the board, its items, and its card, newer.
  const r = (await sync(owner, { since: before })).data;
  assert.deepEqual(r.boards.map((b) => [b.id, !!b.deleted]), [[K, false]]);
  assert.ok(r.items.some((e) => e.board === K && e.item.text === "inside"));
  const back = r.items.find((e) => e.item.id === card).item;
  assert.deepEqual([back.deleted, back.rev], [undefined, 3]);
  assert.deepEqual((await call(hEdit, "deleted")).data, []);
  assert.ok((await sync(hEdit, { since: 0 })).data.scope.includes(K), "the link reaches it again");
});

let failed = 0;
try {
  for (let k = 0; ; k++) {
    try {
      await fetch(base);
      break;
    } catch {
      if (k > 120 || server.exitCode !== null) throw new Error("the local server didn't start:\n" + log);
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  for (const [name, f] of tests) {
    try {
      await f();
      console.log("ok   ", name);
    } catch (e) {
      failed++;
      console.log("FAIL ", name, "\n      ", String(e.stack || e).split("\n").slice(0, 12).join("\n       "));
    }
  }
  if (failed) console.log("--- server log\n" + log.split("\n").filter((l) => !l.startsWith("[wrangler:info]")).slice(-40).join("\n"));
} finally {
  // wrangler starts workerd as a child; end only this run's process tree.
  if (process.platform === "win32") spawnSync("taskkill", ["/pid", String(server.pid), "/T", "/F"], { stdio: "ignore" });
  else server.kill();
  await new Promise((r) => setTimeout(r, 500));
  try {
    rmSync(data, { recursive: true, force: true, maxRetries: 5 });
  } catch {}
}
console.log(failed ? `${failed} of ${tests.length} failed` : `all ${tests.length} passed`);
process.exit(failed ? 1 : 0);
