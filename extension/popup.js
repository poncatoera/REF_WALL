const $ = (id) => document.getElementById(id);
const say = (text, kind) => {
  $('msg').textContent = text;
  $('msg').className = kind || '';
};

// "1:32.5", "92.5" or "1:02:03" to seconds; '' to null.
function parseClock(s) {
  s = String(s || '').trim();
  if (!s) return null;
  const parts = s.split(':').map(Number);
  if (parts.some((n) => !Number.isFinite(n) || n < 0)) return NaN;
  return parts.reduce((acc, n) => acc * 60 + n, 0);
}
const clock = (s) => (s == null ? '' : `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, '0')}`);

let state = null;

function showMarks(m) {
  $('start').value = clock(m?.start ?? null);
  $('end').value = clock(m?.end ?? null);
  updateSendLabel();
}

function updateSendLabel() {
  const clip = $('start').value || $('end').value;
  $('send-label').textContent = clip ? 'Add this clip' : "Add this page's video";
}

const destKey = (d) => (d.kind === 'web' ? `web:${d.tab}` : `app:${d.board || ''}`);
function destFrom(key) {
  if (key.startsWith('web:')) return { kind: 'web', tab: Number(key.slice(4)) };
  return { kind: 'app', board: key.slice(4) || null };
}

function fillDest() {
  const sel = $('dest');
  sel.textContent = '';
  const { app, web } = state;
  if (app) {
    const g = document.createElement('optgroup');
    g.label = 'Wall app';
    const cur = app.boards.find((b) => b.id === app.current);
    g.append(new Option(`Open board${cur ? `: ${cur.name}` : ''}`, 'app:'));
    for (const b of app.boards) g.append(new Option(`${' '.repeat(b.depth)}${b.name}`, `app:${b.id}`));
    sel.append(g);
  }
  if (web.length) {
    const g = document.createElement('optgroup');
    g.label = 'Wall on the web';
    for (const t of web) g.append(new Option(t.title, `web:${t.id}`));
    sel.append(g);
  }
  const want = destKey(state.dest);
  const keys = [...sel.options].map((o) => o.value);
  sel.value = keys.includes(want) ? want : keys[0] || '';
  // What's shown is where things go, even when the saved choice is gone.
  if (sel.value && sel.value !== want) chrome.runtime.sendMessage({ type: 'set-dest', dest: destFrom(sel.value) });
  updateKind();
}

// Video pages and clips need the app (it downloads with yt-dlp).
function updateKind() {
  const web = $('dest').value.startsWith('web:');
  $('clip-row').hidden = web;
  $('send').disabled = !state || web || !state.app;
  $('send').title = web ? 'Wall on the web takes pictures and screenshots; video pages go to the Wall app' : '';
}

function showRecent(jobs) {
  const list = (jobs || []).filter((j) => j.secondsAgo < 3600).slice(0, 6);
  $('recent-box').hidden = !list.length;
  $('recent').textContent = '';
  const words = { added: 'Added', failed: 'Failed', downloading: 'Downloading…', converting: 'Converting…', adding: 'Adding…' };
  for (const j of list) {
    const li = document.createElement('li');
    const name = document.createElement('span');
    name.textContent = j.label;
    name.title = j.error ? `${j.label}\n${j.error}` : j.label;
    const st = document.createElement('span');
    st.textContent = words[j.state] || j.state;
    st.className = j.state;
    li.append(name, st);
    $('recent').append(li);
  }
}

async function refresh() {
  state = await chrome.runtime.sendMessage({ type: 'state' });
  const { app, web } = state;
  const online = !!app || web.length > 0;
  $('dot').className = `dot ${online ? 'ok' : 'off'}`;
  $('status-text').textContent = app ? 'Wall app open' : web.length ? 'Wall on the web open' : 'Wall not open';
  $('shot').disabled = $('shot-area').disabled = !online;
  showRecent(app?.jobs);
  return online;
}

async function init() {
  const online = await refresh();
  fillDest();
  showMarks(await chrome.runtime.sendMessage({ type: 'marks' }));
  if (!online) say('Open the Wall app, or Wall on the web in a tab, then open this again.', 'err');
  // Downloads finish in the app: keep the list fresh while the popup is open.
  setInterval(async () => {
    if (state?.app) {
      const s = await chrome.runtime.sendMessage({ type: 'state' });
      showRecent(s.app?.jobs);
    }
  }, 1500);
}

$('dest').addEventListener('change', () => {
  chrome.runtime.sendMessage({ type: 'set-dest', dest: destFrom($('dest').value) });
  updateKind();
});

for (const which of ['start', 'end']) {
  $(`mark-${which}`).addEventListener('click', async () => {
    const r = await chrome.runtime.sendMessage({ type: 'mark', which });
    if (r?.ok) {
      showMarks(r.marks);
      say('');
    } else say(r?.error || "Couldn't read the video's time", 'err');
  });
  $(which).addEventListener('change', saveTyped);
  $(which).addEventListener('input', updateSendLabel);
}

async function saveTyped() {
  const start = parseClock($('start').value);
  const end = parseClock($('end').value);
  if (Number.isNaN(start) || Number.isNaN(end)) {
    say('Use times like 1:32.5 or 92', 'err');
    return false;
  }
  if (start != null && end != null && end <= start) {
    say('The end must be after the start', 'err');
    return false;
  }
  say('');
  showMarks(await chrome.runtime.sendMessage({ type: 'set-marks', start, end }));
  return true;
}

$('clear-clip').addEventListener('click', async () => {
  await chrome.runtime.sendMessage({ type: 'clear-marks' });
  showMarks(null);
});

async function run(type) {
  if (type === 'send-page' && !(await saveTyped())) return;
  // The area picker draws on the page, so the popup gets out of the way.
  if (type === 'screenshot-area') {
    chrome.runtime.sendMessage({ type });
    window.close();
    return;
  }
  say('Sending…');
  const r = await chrome.runtime.sendMessage({ type });
  if (r?.ok) {
    say(type === 'send-page' ? `Sent to “${r.board}”. The app is downloading it.` : `Added to “${r.board}”.`, 'ok');
    if (type === 'send-page') showMarks(null);
    refresh();
  } else say(r?.error || 'That didn’t work', 'err');
}

$('send').addEventListener('click', () => run('send-page'));
$('shot').addEventListener('click', () => run('screenshot'));
$('shot-area').addEventListener('click', () => run('screenshot-area'));
init();
