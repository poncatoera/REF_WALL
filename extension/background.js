// Wall browser extension: sends pictures, videos, video pages and
// screenshots to a Wall board, in the Wall app (over 127.0.0.1) or in a
// Wall web page open in a tab.
//
// The app's side is apps/desktop/src/collect.rs. The web page's side is
// webwall.js, which hands files to the page as if they were dropped on it.

const APP = 'http://127.0.0.1:47830';
const CONTEXTS = ['image', 'video', 'link', 'page'];
// The biggest file handed to a web Wall tab (the page shrinks big videos).
const WEB_MAX = 300 * 1024 * 1024;
// Where Wall on the web lives (same list as webwall.js in manifest.json).
const WEB_WALL_URLS = ['https://wall.poncatoera.workers.dev/*', 'http://127.0.0.1/*', 'http://localhost/*'];

// ---- the Wall app ---------------------------------------------------------

async function appCall(path, init) {
  const res = await fetch(APP + path, init);
  const json = await res.json().catch(() => ({}));
  if (!res.ok || json.error) throw new Error(json.error || `Wall answered ${res.status}`);
  return json;
}

async function appStatus() {
  try {
    const ctl = new AbortController();
    const t = setTimeout(() => ctl.abort(), 1500);
    const s = await appCall('/status', { signal: ctl.signal });
    clearTimeout(t);
    return s;
  } catch {
    return null;
  }
}

const query = (o) =>
  Object.entries(o)
    .filter(([, v]) => v != null && v !== '')
    .map(([k, v]) => `${k}=${encodeURIComponent(v)}`)
    .join('&');

async function appFile(blob, name, meta) {
  return appCall(`/file?${query({ name, board: meta.board, page: meta.page, title: meta.title })}`, { method: 'POST', body: blob });
}

async function appLink(link, meta) {
  return appCall('/link', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ ...link, board: meta.board, page: meta.page, title: meta.title }),
  });
}

// ---- Wall web pages in tabs -----------------------------------------------

async function webTabs() {
  const tabs = await chrome.tabs.query({ url: WEB_WALL_URLS });
  const out = [];
  for (const t of tabs) {
    try {
      const r = await chrome.tabs.sendMessage(t.id, { type: 'wall-ping' });
      if (r?.wall) out.push({ id: t.id, windowId: t.windowId, title: r.board || t.title || 'Wall' });
    } catch {
      // not a Wall page
    }
  }
  return out;
}

function toBase64(buf) {
  const bytes = new Uint8Array(buf);
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

async function webFile(tabId, blob, name) {
  if (blob.size > WEB_MAX) throw new Error('That file is too big for Wall on the web');
  const data = toBase64(await blob.arrayBuffer());
  const r = await chrome.tabs.sendMessage(tabId, { type: 'wall-add', name, mime: blob.type, data });
  if (!r?.ok) throw new Error(r?.error || 'The Wall page did not take it');
}

// ---- getting the bytes ----------------------------------------------------

const EXT_OF = { 'image/png': 'png', 'image/jpeg': 'jpg', 'image/webp': 'webp', 'image/gif': 'gif', 'image/avif': 'avif', 'image/bmp': 'bmp', 'image/svg+xml': 'svg', 'video/mp4': 'mp4', 'video/webm': 'webm', 'video/quicktime': 'mov' };
const IMAGE_LINK = /\.(png|jpe?g|webp|gif|avif|bmp)(\?|#|$)/i;
const VIDEO_LINK = /\.(mp4|m4v|mov|webm)(\?|#|$)/i;

function fileName(url, blob, fallback) {
  let base = fallback;
  try {
    const u = new URL(url);
    if (u.protocol.startsWith('http')) base = decodeURIComponent(u.pathname.split('/').pop() || '') || fallback;
  } catch {}
  base = base.replace(/[\\/:*?"<>|]/g, '_').slice(0, 90);
  const ext = EXT_OF[blob.type];
  if (ext && !base.toLowerCase().endsWith(`.${ext}`) && !(ext === 'jpg' && /\.jpeg$/i.test(base))) base = `${base.replace(/\.[a-z0-9]{2,5}$/i, '')}.${ext}`;
  return base;
}

// The page itself fetches blob: URLs and pictures only it may load.
async function fetchInPage(tab, frameId, url) {
  const [res] = await chrome.scripting.executeScript({
    target: { tabId: tab.id, frameIds: [frameId ?? 0] },
    args: [url],
    func: async (u) => {
      try {
        const b = await (await fetch(u)).blob();
        return await new Promise((ok) => {
          const r = new FileReader();
          r.onload = () => ok(r.result);
          r.onerror = () => ok(null);
          r.readAsDataURL(b);
        });
      } catch {
        // Not fetchable: draw the picture that's on screen instead.
        const img = [...document.images].find((i) => i.currentSrc === u || i.src === u);
        if (!img || !img.naturalWidth) return null;
        try {
          const c = document.createElement('canvas');
          c.width = img.naturalWidth;
          c.height = img.naturalHeight;
          c.getContext('2d').drawImage(img, 0, 0);
          return c.toDataURL('image/png');
        } catch {
          return null; // a tainted canvas
        }
      }
    },
  });
  if (!res?.result) throw new Error("Couldn't get that picture from the page");
  return (await fetch(res.result)).blob();
}

async function fetchBlob(url, tab, frameId) {
  if (/^https?:|^data:/.test(url)) {
    try {
      const r = await fetch(url, { credentials: 'include' });
      if (r.ok) {
        const b = await r.blob();
        if (b.size) return b;
      }
    } catch {}
  }
  return fetchInPage(tab, frameId, url);
}

// ---- where things go --------------------------------------------------------

// { kind: 'app', board: null | id } or { kind: 'web', tab: id }
async function savedDest() {
  const { dest } = await chrome.storage.local.get('dest');
  return dest || { kind: 'app', board: null };
}

async function resolveDest(dest) {
  if (dest.kind === 'web') {
    const tabs = await webTabs();
    const tab = tabs.find((t) => t.id === dest.tab) || tabs[0];
    if (!tab) throw new Error('Open Wall on the web in a tab first');
    return { kind: 'web', tab: tab.id, name: tab.title };
  }
  return dest;
}

// ---- sending ------------------------------------------------------------------

function flash(text, color, tabId) {
  chrome.action.setBadgeBackgroundColor({ color });
  chrome.action.setBadgeText({ text, tabId });
  setTimeout(() => chrome.action.setBadgeText({ text: '', tabId }), 2500);
}

const LOGIN_SITES = ['vimeo.com', 'instagram.com', 'x.com', 'twitter.com', 'tiktok.com', 'facebook.com', 'threads.net', 'patreon.com'];

function loginSiteOf(url) {
  try {
    const host = new URL(url).hostname;
    return LOGIN_SITES.find((d) => host === d || host.endsWith(`.${d}`)) || null;
  } catch {
    return null;
  }
}

// The site's sign-in, in the cookies.txt form yt-dlp reads, for posts only
// signed-in people see. Only ever sent to the Wall app on 127.0.0.1, which
// deletes it once the download is done.
async function cookiesFor(url) {
  const site = loginSiteOf(url);
  if (!site) return undefined;
  const list = await chrome.cookies.getAll({ domain: site });
  if (!list.length) return undefined;
  const rows = list.map((c) =>
    [
      c.hostOnly ? c.domain : c.domain.startsWith('.') ? c.domain : `.${c.domain}`,
      c.hostOnly ? 'FALSE' : 'TRUE',
      c.path,
      c.secure ? 'TRUE' : 'FALSE',
      c.session ? 0 : Math.floor(c.expirationDate || 0),
      c.name,
      c.value,
    ].join('\t'),
  );
  return `# Netscape HTTP Cookie File\n${rows.join('\n')}\n`;
}

/**
 * Sends one thing. `what`:
 *   { file: Blob, name }                       a picture or video file
 *   { fileUrl, frameId }                        a picture/video at a URL
 *   { videoPage, referer, section }             a page with a video on it
 */
async function send(what, meta, destIn, tab) {
  try {
    const dest = await resolveDest(destIn || (await savedDest()));
    let board;
    if (dest.kind === 'web') {
      let blob = what.file;
      let name = what.name;
      if (!blob && what.fileUrl) {
        blob = await fetchBlob(what.fileUrl, tab, what.frameId);
        name = fileName(what.fileUrl, blob, 'picture');
      }
      if (!blob) throw new Error('Wall on the web takes pictures and video files; send video pages to the Wall app');
      await webFile(dest.tab, blob, name);
      board = dest.name;
    } else {
      const m = { ...meta, board: dest.board || undefined };
      let r;
      if (what.file) r = await appFile(what.file, what.name, m);
      else if (what.fileUrl && !what.video && !VIDEO_LINK.test(what.fileUrl)) {
        // A picture: fetched here, with this browser's sign-in to the site.
        const blob = await fetchBlob(what.fileUrl, tab, what.frameId);
        r = await appFile(blob, fileName(what.fileUrl, blob, 'picture'), m);
      } else {
        // Videos and video pages: the app downloads them (and cuts clips).
        const url = what.fileUrl || what.videoPage;
        const link = { url, referer: what.referer || (what.fileUrl ? meta.page : undefined), cookies: await cookiesFor(url).catch(() => undefined) };
        if (what.section) Object.assign(link, { start: what.section.start, end: what.section.end });
        r = await appLink(link, m);
      }
      board = r.board;
    }
    flash('✓', '#3ddc97', tab?.id);
    return { ok: true, board };
  } catch (e) {
    flash('!', '#ff4d5e', tab?.id);
    const text = String(e?.message || e);
    return { ok: false, error: /Failed to fetch|NetworkError/.test(text) ? 'The Wall app is not open' : text };
  } finally {
    rebuildMenus();
  }
}

// ---- clip marks: the part of a video you want ----------------------------------

const markKey = (tabId) => `marks:${tabId}`;

async function getMarks(tab) {
  const m = (await chrome.storage.session.get(markKey(tab.id)))[markKey(tab.id)];
  return m && m.url === tab.url ? m : null;
}

async function setMark(tab, which, value) {
  const cur = (await getMarks(tab)) || { url: tab.url, start: null, end: null };
  cur[which] = value;
  if (cur.start != null && cur.end != null && cur.end <= cur.start) cur[which === 'start' ? 'end' : 'start'] = null;
  await chrome.storage.session.set({ [markKey(tab.id)]: cur });
  return cur;
}

async function clearMarks(tab) {
  await chrome.storage.session.remove(markKey(tab.id));
  chrome.action.setBadgeText({ text: '', tabId: tab.id });
  chrome.action.setTitle({ tabId: tab.id, title: 'Wall' });
}

// The current time of the biggest video on the page (any frame).
async function videoTime(tab) {
  const res = await chrome.scripting.executeScript({
    target: { tabId: tab.id, allFrames: true },
    func: () => {
      const vids = [...document.querySelectorAll('video')].filter((v) => v.readyState > 0);
      if (!vids.length) return null;
      const v = vids.sort((a, b) => b.clientWidth * b.clientHeight - a.clientWidth * a.clientHeight)[0];
      return { t: v.currentTime, area: v.clientWidth * v.clientHeight };
    },
  });
  const best = res.map((r) => r.result).filter(Boolean).sort((a, b) => b.area - a.area)[0];
  return best ? Math.round(best.t * 100) / 100 : null;
}

const clock = (s) => `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, '0')}`;

async function markNow(tab, which) {
  try {
    const t = await videoTime(tab);
    if (t == null) {
      flash('?', '#ff4d5e', tab.id);
      return { ok: false, error: 'No video found on this page' };
    }
    const m = await setMark(tab, which, t);
    chrome.action.setBadgeBackgroundColor({ color: '#6ea8ff', tabId: tab.id });
    chrome.action.setBadgeText({ text: which === 'start' ? 'IN' : 'OUT', tabId: tab.id });
    chrome.action.setTitle({ tabId: tab.id, title: `Wall clip ${m.start != null ? clock(m.start) : '…'} to ${m.end != null ? clock(m.end) : '…'}` });
    return { ok: true, marks: m };
  } catch (e) {
    return { ok: false, error: e.message };
  }
}

async function sectionFor(tab) {
  const m = await getMarks(tab);
  if (!m || (m.start == null && m.end == null)) return null;
  return { start: m.start || 0, end: m.end ?? undefined };
}

// ---- actions ------------------------------------------------------------------

const metaOf = (tab) => ({ page: tab?.url, title: tab?.title });

async function sendPage(tab, dest) {
  const section = await sectionFor(tab);
  const r = await send({ videoPage: tab.url, section }, metaOf(tab), dest, tab);
  if (r.ok && section) await clearMarks(tab);
  return r;
}

const shotName = (tab) => `${(tab.title || 'Screenshot').replace(/[\\/:*?"<>|]/g, '_').slice(0, 60)}.png`;

async function screenshot(tab, dest) {
  const dataUrl = await chrome.tabs.captureVisibleTab(tab.windowId, { format: 'png' });
  const file = await (await fetch(dataUrl)).blob();
  return send({ file, name: shotName(tab) }, metaOf(tab), dest, tab);
}

// Drag out a rectangle on the page, then the visible tab is captured and cut.
async function screenshotArea(tab, dest) {
  let rect;
  try {
    const [res] = await chrome.scripting.executeScript({ target: { tabId: tab.id }, func: pickArea });
    rect = res?.result;
  } catch {
    return { ok: false, error: "Wall can't take screenshots of this page" };
  }
  if (!rect) return { ok: false, error: 'Cancelled' };
  // Let the overlay disappear before the capture.
  await new Promise((r) => setTimeout(r, 120));
  const dataUrl = await chrome.tabs.captureVisibleTab(tab.windowId, { format: 'png' });
  const bmp = await createImageBitmap(await (await fetch(dataUrl)).blob());
  const k = bmp.width / rect.vw;
  const [x, y, w, h] = [rect.x * k, rect.y * k, rect.w * k, rect.h * k].map(Math.round);
  const canvas = new OffscreenCanvas(Math.max(1, w), Math.max(1, h));
  canvas.getContext('2d').drawImage(bmp, x, y, w, h, 0, 0, w, h);
  const file = await canvas.convertToBlob({ type: 'image/png' });
  return send({ file, name: shotName(tab) }, metaOf(tab), dest, tab);
}

// Runs in the page: a dimmed overlay to drag a rectangle on. Esc cancels.
function pickArea() {
  return new Promise((done) => {
    const o = document.createElement('div');
    o.style.cssText = 'position:fixed;inset:0;z-index:2147483647;cursor:crosshair;background:rgba(10,12,16,.35)';
    const box = document.createElement('div');
    box.style.cssText = 'position:fixed;border:1px solid #6ea8ff;background:rgba(110,168,255,.12);box-shadow:0 0 0 99999px rgba(10,12,16,.35);display:none';
    const tip = document.createElement('div');
    tip.textContent = 'Drag over the part to add to Wall · Esc to cancel';
    tip.style.cssText = 'position:fixed;left:50%;top:16px;transform:translateX(-50%);padding:6px 12px;border-radius:6px;background:#1c1d22;color:#e8e9ee;font:13px system-ui,sans-serif;pointer-events:none';
    o.append(box, tip);
    document.documentElement.append(o);
    let start = null;
    const finish = (r) => {
      o.remove();
      removeEventListener('keydown', key, true);
      done(r);
    };
    const key = (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        finish(null);
      }
    };
    addEventListener('keydown', key, true);
    o.addEventListener('pointerdown', (e) => {
      start = [e.clientX, e.clientY];
      o.setPointerCapture(e.pointerId);
      o.style.background = 'transparent';
      tip.remove();
    });
    const rectOf = (e) => ({ x: Math.min(start[0], e.clientX), y: Math.min(start[1], e.clientY), w: Math.abs(e.clientX - start[0]), h: Math.abs(e.clientY - start[1]) });
    o.addEventListener('pointermove', (e) => {
      if (!start) return;
      const r = rectOf(e);
      Object.assign(box.style, { display: 'block', left: `${r.x}px`, top: `${r.y}px`, width: `${r.w}px`, height: `${r.h}px` });
    });
    o.addEventListener('pointerup', (e) => {
      const r = rectOf(e);
      finish(r.w > 4 && r.h > 4 ? { ...r, vw: innerWidth } : null);
    });
  });
}

// Instagram/X feeds: the right-clicked video is a blob on the feed page, so
// ask the content script which post was clicked.
async function postLinkFor(tab, info) {
  try {
    return await chrome.tabs.sendMessage(tab.id, { type: 'post-link' }, { frameId: info?.frameId ?? 0 });
  } catch {
    return null;
  }
}

async function sendClicked(info, tab, dest) {
  const inFrame = info.frameUrl && info.frameUrl !== info.pageUrl;
  const meta = { page: info.pageUrl || tab?.url, title: tab?.title };
  const kind = info.mediaType || (info.linkUrl ? 'link' : 'page');
  if (kind === 'image') return send({ fileUrl: info.srcUrl, frameId: info.frameId }, meta, dest, tab);
  if (kind === 'link') {
    if (IMAGE_LINK.test(info.linkUrl)) return send({ fileUrl: info.linkUrl, frameId: info.frameId }, meta, dest, tab);
    if (VIDEO_LINK.test(info.linkUrl)) return send({ fileUrl: info.linkUrl, video: true, frameId: info.frameId }, meta, dest, tab);
    return send({ videoPage: info.linkUrl }, { page: info.linkUrl, title: info.selectionText || undefined }, dest, tab);
  }
  const section = await sectionFor(tab);
  let r;
  if (kind === 'video') {
    if (/^https?:/.test(info.srcUrl || '')) {
      r = await send({ fileUrl: info.srcUrl, video: true, frameId: info.frameId, section, referer: info.pageUrl }, meta, dest, tab);
    } else {
      // A streamed video (blob:): its page, or the post it's in on a feed.
      let page = inFrame ? info.frameUrl : meta.page;
      const post = await postLinkFor(tab, info);
      if (post) page = post;
      r = await send({ videoPage: page, section, referer: inFrame ? info.pageUrl : undefined }, { ...meta, page }, dest, tab);
    }
  } else {
    r = await send({ videoPage: meta.page, section }, meta, dest, tab);
  }
  if (r.ok && section) await clearMarks(tab);
  return r;
}

// ---- right-click menu ---------------------------------------------------------
// MV3 can't change a menu as it opens, so it's rebuilt at start, every
// minute, after each send and when a tab finishes loading.

let building = null;
let lastShape = '';

async function rebuildMenus() {
  if (building) return building;
  building = (async () => {
    const [app, web] = await Promise.all([appStatus(), webTabs()]);
    const shape = JSON.stringify([app && app.boards, app && app.current, web.map((t) => [t.id, t.title])]);
    if (shape === lastShape) return;
    lastShape = shape;
    await chrome.contextMenus.removeAll();
    const add = (o) => chrome.contextMenus.create({ contexts: CONTEXTS, ...o });
    add({ id: 'root', title: 'Add to Wall' });
    if (!app && !web.length) {
      add({ id: 'off', parentId: 'root', title: 'Open the Wall app or Wall on the web first', enabled: false });
      return;
    }
    if (app) {
      const cur = app.boards.find((b) => b.id === app.current);
      add({ id: 'app:', parentId: 'root', title: `Open board${cur ? `: ${cur.name}` : ''}` });
      if (app.boards.length > 1) {
        add({ id: 'app-boards', parentId: 'root', title: 'Another board' });
        for (const b of app.boards.slice(0, 60)) add({ id: `app:${b.id}`, parentId: 'app-boards', title: `${'    '.repeat(b.depth)}${b.name}` });
      }
    }
    for (const t of web) add({ id: `web:${t.id}`, parentId: 'root', title: `Wall on the web: ${t.title}`, contexts: ['image', 'video', 'link', 'page'] });
    add({ id: 'sep', parentId: 'root', type: 'separator', contexts: ['page', 'video'] });
    add({ id: 'shot', parentId: 'root', title: 'Screenshot of the page', contexts: ['page', 'video'] });
    add({ id: 'shot-area', parentId: 'root', title: 'Screenshot of an area…', contexts: ['page', 'video'] });
  })().finally(() => (building = null));
  return building;
}

function destOf(menuId) {
  if (menuId.startsWith('app:')) return { kind: 'app', board: menuId.slice(4) || null };
  if (menuId.startsWith('web:')) return { kind: 'web', tab: Number(menuId.slice(4)) };
  return null;
}

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const id = String(info.menuItemId);
  if (id === 'shot') return screenshot(tab);
  if (id === 'shot-area') return screenshotArea(tab);
  const dest = destOf(id);
  if (!dest) return;
  await chrome.storage.local.set({ dest });
  return sendClicked(info, tab, dest);
});

chrome.commands.onCommand.addListener(async (cmd, tab) => {
  if (!tab) [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (cmd === 'send-page') sendPage(tab);
  if (cmd === 'screenshot') screenshot(tab);
  if (cmd === 'screenshot-area') screenshotArea(tab);
  if (cmd === 'mark-in') markNow(tab, 'start');
  if (cmd === 'mark-out') markNow(tab, 'end');
});

// The popup.
chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  if (!String(sender.url || '').startsWith(chrome.runtime.getURL(''))) return false; // only the popup
  (async () => {
    if (msg.type === 'state') {
      const [app, web, dest] = await Promise.all([appStatus(), webTabs(), savedDest()]);
      return reply({ app, web, dest });
    }
    if (msg.type === 'set-dest') {
      await chrome.storage.local.set({ dest: msg.dest });
      return reply(null);
    }
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (msg.type === 'send-page') return reply(await sendPage(tab));
    if (msg.type === 'screenshot') return reply(await screenshot(tab));
    if (msg.type === 'screenshot-area') return reply(await screenshotArea(tab));
    if (msg.type === 'marks') return reply(await getMarks(tab));
    if (msg.type === 'mark') return reply(await markNow(tab, msg.which));
    if (msg.type === 'set-marks') {
      await setMark(tab, 'start', msg.start);
      return reply(await setMark(tab, 'end', msg.end));
    }
    if (msg.type === 'clear-marks') {
      await clearMarks(tab);
      return reply(null);
    }
    reply(null);
  })();
  return true;
});

chrome.runtime.onInstalled.addListener(() => {
  chrome.alarms.create('refresh', { periodInMinutes: 1 });
  rebuildMenus();
});
chrome.runtime.onStartup.addListener(rebuildMenus);
chrome.alarms.onAlarm.addListener((a) => a.name === 'refresh' && rebuildMenus());
chrome.tabs.onUpdated.addListener((_id, change) => change.status === 'complete' && rebuildMenus());
chrome.tabs.onRemoved.addListener(() => rebuildMenus());
