// A short message at the top of the page, for a few seconds.
let timer;
export function notice(text) {
  let e = document.getElementById("wall-notice");
  if (!e) {
    e = document.createElement("div");
    e.id = "wall-notice";
    e.style.cssText =
      "position:fixed;left:50%;top:64px;transform:translateX(-50%);padding:8px 16px;border-radius:8px;font:13px system-ui,sans-serif;background:#1c1d22f2;color:#e8e9ee;border:1px solid #3a3c44;pointer-events:none;max-width:90vw;white-space:pre-line";
    e.setAttribute("role", "alert");
    document.body.append(e);
  }
  // Several files can fail within a moment: show them together.
  const lines = e.hidden ? [] : e.textContent.split("\n").filter(Boolean);
  if (!lines.includes(text)) lines.push(text);
  e.textContent = lines.slice(-6).join("\n");
  e.hidden = false;
  clearTimeout(timer);
  timer = setTimeout(() => {
    e.hidden = true;
    e.textContent = "";
  }, 9000);
}

// A picture the browser can show but Wall can't read (AVIF, BMP, GIF…), as a
// still PNG. Null if the browser can't either.
export async function toPng(file) {
  let bitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    return null;
  }
  const { width, height } = bitmap;
  if (!width || !height || width > 8192 || height > 8192) return null;
  const c = document.createElement("canvas");
  c.width = width;
  c.height = height;
  c.getContext("2d").drawImage(bitmap, 0, 0);
  bitmap.close();
  const blob = await new Promise((r) => c.toBlob(r, "image/png"));
  return blob ? new Uint8Array(await blob.arrayBuffer()) : null;
}

// A file dropped just beside the board must not replace the page.
for (const kind of ["dragover", "drop"]) {
  document.addEventListener(kind, (e) => e.preventDefault());
}
