// Remembers which post was right-clicked on feed pages (Instagram, X), so the
// extension can send that post's link instead of the feed URL.
let lastPost = null;

const SELECTORS = {
  instagram: 'a[href*="/p/"], a[href*="/reel/"], a[href*="/reels/"]',
  x: 'a[href*="/status/"]:has(time), a[href*="/status/"]',
};

function findPost(el) {
  const site = /instagram\.com$/.test(location.hostname) ? 'instagram' : 'x';
  const sel = SELECTORS[site];
  const direct = el.closest(sel);
  if (direct) return direct;
  // Walk up to the post container and take its permalink.
  const container = el.closest('article, [role="dialog"], [role="presentation"]');
  return container?.querySelector(sel) || null;
}

// An open Instagram post (page or pop-up) puts ?img_index=N in the address bar as you
// swipe, which tells the app exactly which slide you're looking at.
const OPEN_POST = /^\/(?:[^/]+\/)?(p|reels?)\/[^/]+/;

document.addEventListener('contextmenu', (e) => {
  if (/instagram\.com$/.test(location.hostname) && OPEN_POST.test(location.pathname)) {
    const u = new URL(location.href);
    const keep = u.searchParams.get('img_index');
    u.search = keep ? `?img_index=${keep}` : '';
    lastPost = u.href;
    return;
  }
  const a = e.target instanceof Element ? findPost(e.target) : null;
  lastPost = a ? new URL(a.getAttribute('href'), location.href).href.split('?')[0] : null;
}, true);

chrome.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg.type === 'post-link') reply(lastPost);
});
