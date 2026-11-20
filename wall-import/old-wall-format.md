# The old Wall (Electron) library format

How the old Electron app ("Wall", earlier named "RefLoop", source in
`D:\reference_tool`) stores a library on disk, for writing an importer into
the new `.wall` format. Read from the source (file:line references below)
and checked against Paul's real library, read-only. Examples here are made
up; no real titles, links or pictures are copied.

## 1. Where the library lives

- Settings file: `%APPDATA%\<app name>\settings.json` (Electron `userData`,
  `app/lib/settings.js:47`). The app was renamed, so there are two:
  `%APPDATA%\RefLoop\settings.json` and `%APPDATA%\Wall\settings.json`.
- Library folder: `settings.libraryRoot`. Default
  `<Documents>\Wall Library` (`settings.js:6`). The environment variable
  `REFLOOP_LIBRARY` overrides it, for test instances
  (`settings.js:6`, `settings.js:53`).
- Gotcha: settings are merged as `{...defaults, ...file}` (`settings.js:49`),
  so an empty `"libraryRoot": ""` in the file wins over the default.
- **On this PC:** the real library is `D:\Pon\Documents\RefLoop Library`
  (from the RefLoop settings). The Wall settings file has an empty
  `libraryRoot`. Settings also hold API keys (`anthropicKey*`, `arenaToken`,
  `figmaKey`); an importer must not read or copy those.

### What the real library contains (20 Nov 2026)

| | |
|---|---|
| Boards | 4, all top level (no sub-boards) |
| Items | 64: 32 video, 10 image, 7 label, 5 note, 4 frame (group), 4 todo, 1 arrow, 1 palette |
| Items per board | 2, 6, 16, 40 |
| Boards with a reel | 2 |
| Trash | 17 entries (`.trash/trash.json`), 29 files, 114 MB |
| Media on boards | 32 mp4 (682 MB), 17 jpg, 8 png, 8 webp |
| Sidecars | 45 `*.json` next to media |
| Cache | 45 thumbnails, 1 link preview |
| Total | 185 files, 809 MB |

Many videos are over the new 20 MB limit (682 MB / 32 ≈ 21 MB average),
so the importer has to run them through the new conversion rules.
Item `status` values found: `ready`, `error` (failed downloads have no file).

## 2. Folder layout

```
<library root>/
  library.json                 everything: boards and items (one file)
  <Board folder>/              one folder per board, named after the board
    <file>.<ext>               media files of that board's items
    <file>.<ext>.json          sidecar: copy of the item's metadata
    <Sub board folder>/        sub-boards nest in their parent's folder
  .cache/
    library.backup.json        copy of library.json from the last good load
    thumbs/<itemId>.jpg        thumbnails (safe to rebuild)
    links/<itemId>.<ext>       preview images of link cards
  .trash/
    trash.json                 list of trashed items (see 3.4)
    <file>.<ext>[.json]        trashed media and sidecars
```

`library.js:18-20, 28, 100-110, 217-219, 282-301`. Folder names come from
`safeName(board name)`, made unique with " 2", " 3" (`library.js:101-110`).
Media paths in `library.json` are relative to the root, with Windows
separators (e.g. `Board A\clip.mp4`).

## 3. Schemas

All times are Unix **milliseconds** unless noted; ids are 10 lowercase hex
characters (`shortId`, e.g. `"5ab6bf55d9"`). JSON is written with 1-space
indents, atomically via `.tmp` + rename (`library.js:66-74`).

### 3.1 `library.json`

```json
{ "version": 1, "boards": [ /* Board */ ], "items": [ /* Item */ ] }
```

Loaded only if `boards` and `items` are arrays, else the backup is used
(`library.js:27-52`). A library always has at least one board (`Inbox` is
created when empty).

### 3.2 Board

| Field | Type | Meaning |
|---|---|---|
| `id` | string | 10-hex id |
| `name` | string | shown name |
| `parentId` | string \| null | parent board for sub-boards |
| `folder` | string | folder relative to the root (`Parent\Child` for sub-boards) |
| `createdAt` | ms | |
| `view` | `{x, y, zoom}` | last camera: screen = world × zoom + (x, y), x/y in screen px (`canvas.js:175, 196`); zoom 0.03–40 |
| `reel` | array, optional | animatic: `[{key, id, in, out, dur}]`, `id` = item id, `in`/`out`/`dur` seconds or null (`library.js:164-175`) |

### 3.3 Item (all kinds)

Common fields (`library.js:225-242`):

| Field | Type | Meaning |
|---|---|---|
| `id` | string | 10-hex id |
| `boardId` | string | board it is on |
| `kind` | string | see below |
| `createdAt` | ms | |
| `x`, `y` | number \| null | **top-left** corner in world units (`canvas.js:64, 535`). null = not placed yet (gets placed when shown) |
| `w`, `h` | number \| null | size in world units |
| `z` | integer | stacking order, higher on top; new items get max+1 |
| `status` | `"ready"` \| `"downloading"` \| `"error"` | `downloading` is turned into `error` on load |
| `error` | string \| null | |
| `progress` | 0–1 | download progress |
| `tags` | string[] | manual tags (no auto-tags in the old format) |
| `notes` | string | free notes on any item (inspector) |
| `title` | string | |
| `scale` | number | cards only: text/UI scale, 1/zoom at creation clamped to 0.25–8 (`cards.js:45-53`) |
| `color` | string | cards only: `default`, `yellow`, `orange`, `red`, `pink`, `purple`, `blue`, `green` (`cards.js:19-29`) |

World units have no fixed meaning: a picture's `w` is however wide it was
placed (cards are `base size × scale`). Boards in the real library span
roughly ±40,000 units.

#### Media: `kind: "image"` and `"video"`

| Field | Type | Meaning |
|---|---|---|
| `file` | string | path relative to the root |
| `width`, `height` | px | media pixel size |
| `hash` | string | 40-hex (SHA-1 length) of the file, for duplicates |
| `sourceUrl`, `pageUrl`, `domain` | string | where it came from |
| `origin` | object | the capture request: `{mediaType, srcUrl \| linkUrl, pageUrl, title, boardId, section, fromBrowser}` |
| `thumb` | ms | time the thumbnail was made (cache-buster); `noThumb` bool |
| `palette` | `[{hex, share}]` | dominant colours, share 0–1 |
| `seenAt` | ms | last opened |
| `opacity` | 0–1 | |
| `crop` | `{x, y, w, h}` | visible part, fractions 0–1 of the media (`canvas.js:594, 656-664`) |
| `rotate` | degrees | 0 / 90 / 180 / 270 |
| `mirror`, `flipV`, `grayscale` | bool | |
| `strokes` | `[{c, w, pts: [[x, y], …]}]` | pen annotations (`anno.js:7`) |
| `tracks` | `[{c, pts: [{f, x, y}]}]` | tracked points per frame, media pixels (`anno.js:8`, `canvas.js:613-627`) |
| `lockTrack` | index | keeps a tracked point still |
| `aiDesc` | string | Claude description |

Video only:

| Field | Type | Meaning |
|---|---|---|
| `duration` | **seconds** | |
| `fps` | number | |
| `frames` | integer | |
| `loopIn`, `loopOut` | seconds | in/out points |
| `speed` | number | playback rate |
| `pingpong` | bool | loop back and forth |
| `markers` | `[{f, label}]` | key poses by frame number (`inspector.js:368-378`) |
| `onion` | `{on, before, after, step}` | onion-skin settings |
| `section` | `{start, end}` \| null | seconds; only this part of the source was downloaded (`ingest.js:117-125`) |

GIFs are converted to MP4 on import (`ingest.js:640`).

#### Cards

| kind | Fields | Default size × scale (`cards.js:32-43`) |
|---|---|---|
| `note` | `text`, `color` (default `yellow`) | 240 × 120 |
| `label` | `text`, `size` (font px, already × scale) | 220 × 56 |
| `todo` | `title`, `tasks: [{id, text, done}]` (`id` 7 chars) | 260 × 140 |
| `frame` | `title`, `color` (default `blue`): **a group** | 820 × 540 |
| `palette` | `title`, `swatches: [{hex, name?}]` | 400 × 170 |
| `link` | `url`, `title`, `text`, `siteName`, `domain`, `linkImage` (file in `.cache/links`) (`ingest.js:434-445`) | 300 wide |
| `board` | `linkBoard` (board id): opens a sub-board | 240 × 180 |
| `type` | `font`, `ps`, `fstyle`, `text`, `size`, `leading`, `tracking`, `transform`, `mode` | 460 × 240 |
| `sketch` | `strokes` | from the drawing |

Groups (`frame`) don't list their members; anything inside the frame's
rectangle belongs to it, as in the new Wall.

#### Arrows: `kind: "arrow"`

`from`, `to`: either `{ "id": "<item id>" }` (attached; drawn from the
item's centre, clipped at its edge) or `{ "x": …, "y": … }` (free point in
world units). `head`: `"end"` (default) | `"both"` | `"none"`. `color`,
`scale` (line width). `x/y/w/h` are null (`links.js:4, 45-80`). When an
attached item is removed, the end is pinned to where it was
(`links.js:137-146`).

### 3.4 Sidecars and trash

- Sidecar `<file>.json` next to each media file: the `META_KEYS` subset of
  the item (`id, kind, title, sourceUrl, pageUrl, domain, tags, notes,
  createdAt, width, height, duration, fps, frames, hash, loopIn, loopOut,
  speed, pingpong, mirror, strokes, tracks, section, markers, palette,
  aiDesc`) plus `board` (board name) (`library.js:8-12, 262-269`).
  `library.json` is the source of truth; sidecars are a readable copy.
- `.trash/trash.json`: `[{item, file, sidecar, trashedAt, boardName}]`,
  newest first. `item` is the full item record; `file`/`sidecar` are
  absolute paths of the moved files, or null (`library.js:278-301`).

## 4. Made-up example

```json
{
 "version": 1,
 "boards": [
  { "id": "a1b2c3d4e5", "name": "Walk cycles", "parentId": null, "folder": "Walk cycles",
    "createdAt": 1793000000000, "view": { "x": 120, "y": -40, "zoom": 0.25 } }
 ],
 "items": [
  { "id": "0f1e2d3c4b", "boardId": "a1b2c3d4e5", "kind": "video", "createdAt": 1793000100000,
    "x": 0, "y": 0, "w": 960, "h": 540, "z": 3, "status": "ready", "error": null, "progress": 1,
    "tags": ["walk"], "notes": "", "title": "Example clip", "file": "Walk cycles\\example.mp4",
    "width": 1920, "height": 1080, "duration": 6.0, "fps": 24, "frames": 144,
    "loopIn": 1.5, "loopOut": 4.0, "pingpong": false, "markers": [{ "f": 36, "label": "Contact" }],
    "crop": { "x": 0.1, "y": 0, "w": 0.8, "h": 1 },
    "hash": "0123456789abcdef0123456789abcdef01234567",
    "palette": [{ "hex": "#2a3b4c", "share": 0.6 }] },
  { "id": "1a2b3c4d5e", "boardId": "a1b2c3d4e5", "kind": "note", "createdAt": 1793000200000,
    "x": 1000, "y": 0, "w": 240, "h": 120, "z": 4, "status": "ready", "tags": [], "notes": "",
    "title": "", "text": "Heel strike on 9", "color": "yellow", "scale": 1 },
  { "id": "2b3c4d5e6f", "boardId": "a1b2c3d4e5", "kind": "arrow", "createdAt": 1793000300000,
    "x": null, "y": null, "w": null, "h": null, "z": 5, "status": "ready", "tags": [], "notes": "",
    "title": "", "from": { "id": "1a2b3c4d5e" }, "to": { "id": "0f1e2d3c4b" },
    "head": "end", "color": "default", "scale": 1 }
 ]
}
```

## 5. Mapping to the new `.wall` format (suggestions)

| Old | New | Notes |
|---|---|---|
| library root | new library folder | import into a new library; never write into the old one |
| board | `boards/<uuid>.json` | new UUIDs; keep a map old id → new id. `parentId` → nested boards plus a `board` card on the parent (the old app shows sub-boards as `kind:"board"` cards when one exists). `view` → initial camera (guess: centre = (w/2 − x)/zoom, (h/2 − y)/zoom for the window size) |
| media `file` | `media/<shard>/<blake3>.<ext>` + `items/<hash>.json` | run the new import rules (2048 px, 20 MB H.264, keyframe each second). Dedupe by the new hash. Old `hash` is not reused |
| `title`, `tags`, `sourceUrl`/`pageUrl` | sidecar `title`, `tags`, `source_url` | prefer `sourceUrl`, else `pageUrl` |
| `palette[].hex` | sidecar `colors` | |
| `width`, `height`, `duration` (s), `fps` | sidecar `width`, `height`, `duration_ms` (×1000), `fps` | after conversion, from the converted file |
| `loopIn`/`loopOut` | sidecar `range {in_ms, out_ms}` | seconds × 1000 |
| `x, y, w, h, z` | `frame {x, y, w, h, rotation: 0, z}` | same top-left convention; world units carry over 1:1. Items with null x/y: place in a grid after the others |
| `crop {x,y,w,h}` | `media.crop` (0–1) | same meaning, as long as `rotate` is 0. Rotation, mirror, flipV and grayscale have no new equivalent yet |
| `note.text`, `color` | `note {text, color}` | colour names → the new palette (`panel::PALETTE`): yellow `#e6c84f`, orange `#e8a15d`, red `#e5534b`, pink → red or purple (no pink yet), purple `#b083f0`, blue `#7aa2f7`, green `#57ab5a`, default → none |
| `label.text` | `label {text}` | `size` has no new equivalent |
| `todo.title`, `tasks[].text/done` | `todo {title, entries[{text, done}]}` | |
| `frame.title`, `color` | `group {name, color}` | |
| `arrow.from/to` | `arrow {from, to}` | `{id}` → `ArrowEnd::Item` with the new UUID; `{x, y}` → `ArrowEnd::Point`. `head` not kept |
| `board` card | `board {board_id}` | |
| `palette`, `link`, `type`, `sketch` | (none yet) | suggest a note with the text (link: title + URL; palette: hex codes) so nothing is lost silently |
| `notes`, `markers`, `speed`, `pingpong`, `onion`, `strokes`, `tracks`, `section`, `aiDesc` | sidecar `extra` | the new format keeps unknown fields: store them under `extra.old_wall` for later study tools |
| `status: "error"` items | skip | list them in the import report |
| `.trash` | skip (or import into the new trash) | |
| `.cache` | skip | rebuilt by the new app |
