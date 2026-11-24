# Wall browser extension

Right-click a picture, video or link on any web page and choose **Add to Wall**.
It goes to a board in the **Wall app**, or to **Wall on the web** open in a tab.

Works in Edge and Chrome (Manifest V3).

## Install (until it's in the stores)

In Wall: **File > Install browser extension…** opens the folder, copies its
address and shows these steps.

1. Edge: go to `edge://extensions` (Chrome: `chrome://extensions`).
2. Turn on **Developer mode**.
3. **Load unpacked** and pick this `extension` folder.
4. Pin the Wall icon to the toolbar if you like.

Shortcuts can be changed at `edge://extensions/shortcuts`.

## What it does

| | Wall app | Wall on the web |
|---|---|---|
| Picture (right-click) | ✓ | ✓ |
| Video file on a page | ✓ downloaded by the app | ✓ |
| Video page (YouTube, Vimeo, Instagram, X…) | ✓ with yt-dlp | — |
| Only part of a video (clip start/end) | ✓ | — |
| Screenshot of the page, Alt+Shift+X | ✓ | ✓ |
| Screenshot of an area, Alt+Shift+A | ✓ | ✓ |

- **Add to** in the popup picks the board: the open one, any board of the
  library, or a Wall web tab. The right-click menu offers the same.
- **Clip**: press *Now* at the start and at the end while the video plays
  (or type times like `1:32.5`), then *Add this clip*. The app downloads just
  that part.
- Pictures are fetched by the browser, so pictures you can only see signed in
  still come through. For video sites that need a sign-in (Instagram, X,
  Vimeo, TikTok, Facebook, Threads, Patreon) the browser's sign-in to that one
  site goes to the Wall app on this computer for the download and is deleted
  straight after.
- Each picture remembers the page it came from (Details panel, *Source*).

## How it talks to Wall

- **Wall app**: an HTTP server on `127.0.0.1:47830` (see
  `apps/desktop/src/collect.rs`). It only takes requests from browser
  extensions, never from web pages. Video sites are downloaded with
  [yt-dlp](https://github.com/yt-dlp/yt-dlp), which Wall fetches into
  `%LOCALAPPDATA%\Wall\tools` the first time it's needed (or uses one next to
  `wall.exe`, on PATH, or at `WALL_YTDLP`).
- **Wall on the web**: `webwall.js` hands the file to the page as if it was
  dropped in the middle of the board, so it's saved and shared like any drop.
