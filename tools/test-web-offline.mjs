// End-to-end check of the web version on an edit link going offline:
// changes made while offline are kept in the browser, survive the tab being
// closed, and reach the server when the link is opened again. Runs the sync
// server locally (as server/test/run.mjs does) and the built web version
// from server/public in headless Chromium (Playwright).
//
// Build first: the web version into server/public (tools/build-server.ps1,
// or cargo build + wasm-bindgen + copy apps/web/www). Then:
//   node tools/test-web-offline.mjs
import { spawn, spawnSync } from "node:child_process";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import assert from "node:assert/strict";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "server");
if (!existsSync(join(root, "public", "pkg"))) throw new Error("build the web version into server/public first");
const port = 8796;
const site = `http://127.0.0.1:${port}/`;
const api = site + "api/";
const owner = randomBytes(32).toString("base64url");
const data = mkdtempSync(join(tmpdir(), "wall-offline-"));
const sha = (t) => createHash("sha256").update(t).digest("hex");
const shots = process.env.SHOTS || "";

const server = spawn(
  process.execPath,
  [join(root, "node_modules/wrangler/bin/wrangler.js"), "dev", "--local", "--ip", "127.0.0.1", "--port", String(port), "--persist-to", data, "--var", `OWNER_HASH:${sha(owner)}`, "--show-interactive-dev-session=false"],
  { cwd: root, env: { ...process.env, WRANGLER_SEND_METRICS: "false", CI: "1" }, stdio: ["ignore", "pipe", "pipe"] },
);
let log = "";
server.stdout.on("data", (d) => (log += d));
server.stderr.on("data", (d) => (log += d));

async function call(key, path, { method = "GET", json } = {}) {
  const r = await fetch(api + path, { method, headers: { authorization: `Bearer ${key}` }, body: json === undefined ? undefined : JSON.stringify(json) });
  return { status: r.status, data: await r.json() };
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, f, ms = 30_000) {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out: ${what}`);
    await sleep(300);
  }
}
const texts = async (board) => (await call(owner, `items?board=${board}`)).data.items.filter((e) => !e.item.deleted).map((e) => e.item.text).filter(Boolean).sort();

const A = randomUUID();
let browser;
let failed = false;
try {
  await until("server", async () => fetch(api + "me").then(() => true, () => false), 60_000);
  await call(owner, "sync", { method: "POST", json: { since: 0, boards: [{ version: 1, id: A, name: "Offline test", created_ms: 1, updated_ms: 1 }], items: [{ board: A, item: { id: randomUUID(), frame: { x: 0, y: 0, w: 300, h: 120 }, type: "note", text: "from the owner", rev: 1, updated_ms: 1, updated_by: "pc" } }] } });
  const edit = (await call(owner, "links", { method: "POST", json: { role: "edit", board: A } })).data;
  const link = `${site}#share=${edit.secret}`;

  browser = await chromium.launch();
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  // No welcome cards in the way.
  await context.addInitScript(() => {
    try {
      localStorage.setItem("wall.name", "Tester");
      localStorage.setItem("wall.welcome", "done");
    } catch {}
  });
  let page = await context.newPage();
  page.on("console", (m) => process.env.VERBOSE && console.log("page:", m.text()));
  await page.goto(link);
  await page.waitForTimeout(4000);
  const note = async (text, x, y) => {
    await page.mouse.dblclick(x, y);
    await page.waitForTimeout(300);
    await page.keyboard.type(text);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);
    await page.mouse.click(1100, 700);
    await page.waitForTimeout(300);
  };

  await note("made online", 300, 600);
  await until("the online note reaches the server", async () => (await texts(A)).includes("made online"));
  console.log("ok    a change made online goes up");

  await context.setOffline(true);
  await note("made offline", 700, 600);
  if (shots) await page.screenshot({ path: join(shots, "offline.png") });
  const kept = await until("the offline note is kept in the browser", () =>
    page.evaluate(async () => {
      const db = await new Promise((res, rej) => {
        const r = indexedDB.open("wall", 1);
        r.onsuccess = () => res(r.result);
        r.onerror = () => rej(r.error);
      });
      const keys = await new Promise((res) => {
        const r = db.transaction("kv").objectStore("kv").getAllKeys();
        r.onsuccess = () => res(r.result);
      });
      const k = keys.find((k) => typeof k === "string" && k.startsWith("unsent/") && !k.includes("/media/"));
      if (!k) return null;
      return new Promise((res) => {
        const r = db.transaction("kv").objectStore("kv").get(k);
        r.onsuccess = () => res(r.result);
      });
    }),
  );
  assert.ok(kept.includes("made offline"), kept);
  assert.ok(!(await texts(A)).includes("made offline"), "not on the server while offline");
  console.log("ok    a change made offline is kept in the browser");

  // The tab is closed while still offline, then the link is opened again.
  await page.close();
  await context.setOffline(false);
  page = await context.newPage();
  await page.goto(link);
  await until("the offline note reaches the server after reopening", async () => (await texts(A)).includes("made offline"));
  console.log("ok    after the tab was closed offline, reopening sends it");
  await until("the kept copy is cleared once sent", () =>
    page.evaluate(async () => {
      const db = await new Promise((res) => {
        const r = indexedDB.open("wall", 1);
        r.onsuccess = () => res(r.result);
      });
      const keys = await new Promise((res) => {
        const r = db.transaction("kv").objectStore("kv").getAllKeys();
        r.onsuccess = () => res(r.result);
      });
      return !keys.some((k) => typeof k === "string" && k.startsWith("unsent/"));
    }),
  );
  console.log("ok    the kept copy is cleared once it has gone up");

  // Offline then back online without closing: it goes up by itself.
  await context.setOffline(true);
  await note("while away", 500, 450);
  await page.waitForTimeout(1500);
  await context.setOffline(false);
  await until("it goes up when the connection is back", async () => (await texts(A)).includes("while away"), 20_000);
  console.log("ok    back online, it goes up without a reload");
  if (shots) await page.screenshot({ path: join(shots, "back-online.png") });
  assert.deepEqual(await texts(A), ["from the owner", "made offline", "made online", "while away"]);
} catch (e) {
  failed = true;
  console.log("FAIL ", String(e.stack || e));
  console.log("--- server log\n" + log.split("\n").slice(-30).join("\n"));
} finally {
  await browser?.close();
  if (process.platform === "win32") spawnSync("taskkill", ["/pid", String(server.pid), "/T", "/F"], { stdio: "ignore" });
  else server.kill();
  await sleep(500);
  try {
    rmSync(data, { recursive: true, force: true, maxRetries: 5 });
  } catch {}
}
process.exit(failed ? 1 : 0);
