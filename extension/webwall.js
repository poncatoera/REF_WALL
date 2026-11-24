// Runs on Wall on the web: takes pictures and videos from the extension and
// hands them to the page as if they were dropped in the middle of the board.
// Nothing here talks to any server; the page saves and syncs as for a drop.

const canvas = () => document.getElementById('wall');

chrome.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg.type === 'wall-ping') {
    if (!canvas()) return false;
    reply({ wall: true, board: document.title.replace(/\s*[·|-]\s*Wall$/, '') || 'Wall' });
    return false;
  }
  if (msg.type !== 'wall-add') return false;
  const c = canvas();
  if (!c) {
    reply({ ok: false, error: 'This tab is not a Wall board' });
    return false;
  }
  try {
    const bin = atob(msg.data);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    const file = new File([bytes], msg.name || 'picture', { type: msg.mime || '' });
    const dt = new DataTransfer();
    dt.items.add(file);
    const r = c.getBoundingClientRect();
    const at = { clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 };
    c.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt, ...at }));
    c.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt, ...at }));
    reply({ ok: true });
  } catch (e) {
    reply({ ok: false, error: String(e?.message || e) });
  }
  return false;
});
