// Requests to Wall's server, which also serves this page. The link's secret
// goes in a header: never in an address, a referrer or a cookie.
export async function cloudCall(method, path, key, body) {
  const r = await fetch("/api/" + path, {
    method,
    headers: { authorization: "Bearer " + key },
    body: body === undefined || body === null ? undefined : body,
    credentials: "omit",
    referrerPolicy: "no-referrer",
    redirect: "error",
    // Files never change under their name; everything else must be fresh.
    cache: method === "GET" && path.startsWith("media/") ? "default" : "no-store",
  });
  return [r.status, new Uint8Array(await r.arrayBuffer())];
}

// Another link pasted into the same tab.
addEventListener("hashchange", () => {
  if (location.hash.startsWith("#share=")) location.reload();
});

// A line at the bottom of the page: what kind of link this is, or what's wrong.
export function cloudNote(text) {
  let e = document.getElementById("cloud-note");
  if (!e) {
    e = document.createElement("div");
    e.id = "cloud-note";
    e.style.cssText =
      "position:fixed;left:50%;bottom:14px;transform:translateX(-50%);padding:6px 14px;border-radius:8px;font:13px system-ui,sans-serif;background:#1c1d22ee;color:#e8e9ee;border:1px solid #3a3c44;pointer-events:none;max-width:90vw;text-align:center";
    e.setAttribute("role", "status");
    document.body.append(e);
  }
  e.textContent = text;
  e.hidden = !text;
}
