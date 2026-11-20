# The `.wall` library format, version 1

A Wall library is a plain folder of JSON files and media. Any program can
read or write it without Wall: there is no database, no binary index, and
nothing is hidden. The Rust crate `wall-format` reads and writes it, and the
`wall` command-line tool (`crates/wall-cli`) shows how to use it.

## Folder layout

```text
MyRefs.wall/
  wall.json                 manifest
  media/ab/ab12….png        media files, named by content hash, never edited
  items/ab12….json          one sidecar per media file
  boards/<uuid>.json        one file per board
  cache/                    thumbnails and previews; safe to delete
  trash/                    deleted boards and media; nothing is hard-deleted
```

All JSON is UTF-8. Readers must accept a leading byte-order mark, because
Notepad and PowerShell write one. Writers write without it. Files are
written atomically: write to a temporary file in the same folder, then
rename it over the old one.

**Unknown fields must survive.** Every JSON object may carry fields that
this spec doesn't define. A program that reads and rewrites an object keeps
the fields it doesn't understand. That is how other apps can store their
own data next to Wall's.

## `wall.json`

```json
{
  "format": "wall",
  "version": 1,
  "id": "6d0b0b5e-3f6c-4c1e-9a7e-2f0f6f0e8d11",
  "name": "MyRefs",
  "created_ms": 1795132800000
}
```

| Field | Type | Meaning |
|---|---|---|
| `format` | string | Always `"wall"`. Anything else is not a Wall library. |
| `version` | integer | Format version. Readers refuse a version newer than they know. |
| `id` | UUID | Stable id of the library, used by sync. |
| `name` | string | Display name. |
| `created_ms` | integer | Creation time, Unix milliseconds. |

All times in this format are Unix milliseconds, UTC.

## Media files

A picture or video is stored once, at `media/<first two hex>/<hash>.<ext>`.

- `hash` is the BLAKE3 hash of the file's bytes, as 64 lowercase hex
  characters. The same bytes always get the same name, which makes
  duplicates impossible and sync simple.
- `ext` is lowercase, without the dot: `jpg`, `png`, `webp`, `gif`, `mp4`,
  `mov` or `webm`.
- Wall never edits a media file. Cropping and in/out points are stored as
  data on the board or sidecar.

Import rules (applied by Wall before a file is stored; other writers should
follow them so every Wall can open the library):

- Pictures wider than 2048 px are scaled down to 2048 px wide.
- Videos over 20 MB are re-encoded below 20 MB (H.264 in MP4).
- When a file was shrunk, `original` in its sidecar records the original
  size.

## Sidecars: `items/<hash>.json`

What Wall knows about one media file.

```json
{
  "hash": "ab12…",
  "kind": "video",
  "ext": "mp4",
  "width": 1920,
  "height": 1080,
  "bytes": 11400000,
  "duration_ms": 8000,
  "fps": 25.0,
  "added_ms": 1795132800000,
  "title": "night-drive.mp4",
  "source_url": "https://example.com/clip",
  "tags": ["night", "hero shot"],
  "auto_tags": [
    { "tag": "blue", "source": "local:color" },
    { "tag": "landscape", "source": "local:shape" }
  ],
  "colors": ["#1e50dc", "#f08c28"],
  "original": { "width": 3840, "height": 2160, "bytes": 190000000 },
  "range": { "in_ms": 2000, "out_ms": 6500 }
}
```

| Field | Type | Meaning |
|---|---|---|
| `hash` | string | Content hash; matches the file name. |
| `kind` | `"image"` or `"video"` | |
| `ext` | string | Extension of the media file. |
| `width`, `height` | integer | Pixel size of the stored file. |
| `bytes` | integer | Size of the stored file. |
| `duration_ms` | integer, optional | Videos only. |
| `fps` | number, optional | Videos only. |
| `added_ms` | integer | When it entered the library. |
| `title` | string, optional | Shown in the side panel; defaults to the file name. |
| `source_url` | string, optional | Where it came from. |
| `tags` | string array | Tags a person chose. Lowercase. Never written by a machine. |
| `auto_tags` | array, optional | Suggested tags, kept apart from `tags` so re-running a tagger never overwrites a person's choice. `source` says who suggested it: `local:color`, `local:shape` and `local:length` are Wall's own, and `claude` is reserved for Claude tagging. `confidence` (0 to 1) is optional. |
| `colors` | string array | Main colours as `#rrggbb`, most common first. |
| `original` | object, optional | `width`, `height` and `bytes` before Wall shrank the file, and `kept_as` (a hash) when the original was kept too. |
| `range` | object, optional | Video in and out points, `in_ms` and `out_ms`. Playback loops between them. |

## Boards: `boards/<uuid>.json`

```json
{
  "version": 1,
  "id": "0f1e…",
  "name": "Motion refs",
  "parent": "6a7b…",
  "created_ms": 1795132800000,
  "updated_ms": 1795133000000,
  "items": {
    "4c2d…": {
      "id": "4c2d…",
      "frame": { "x": 0, "y": 0, "w": 320, "h": 180, "rotation": 0, "z": 3 },
      "type": "media",
      "hash": "ab12…",
      "crop": { "x": 0.1, "y": 0, "w": 0.8, "h": 1 },
      "rev": 7,
      "updated_ms": 1795133000000,
      "updated_by": "DESKTOP-O3HIT36"
    }
  }
}
```

| Field | Meaning |
|---|---|
| `id` | UUID; matches the file name. |
| `name` | Display name. |
| `parent` | The board this one sits inside, if any. A library has one board without a parent: its home board. |
| `items` | Every item on the board, keyed by item id. |

Board units are roughly pixels at 100% zoom. `x` and `y` grow right and
down. `z` orders items: higher draws on top.

### Items

Every item has `id`, `frame`, `type`, `rev`, `updated_ms` and
`updated_by`, plus fields for its type:

| `type` | Fields | |
|---|---|---|
| `media` | `hash`, `crop` (optional) | A picture or video. `crop` is the visible part in 0–1 texture units. |
| `note` | `text`, `color` (optional) | A card of text. `color` is `#rrggbb`. |
| `label` | `text` | Large free text without a card. |
| `todo` | `title`, `entries`: `[{ "text", "done" }]` | A checklist. |
| `group` | `name`, `color` (optional) | A frame that carries the items inside it when moved. |
| `arrow` | `from`, `to` | Each end is `{ "to": "item", "id": … }` or `{ "to": "point", "x": …, "y": … }`. |
| `board` | `board_id` | A card that opens another board. |

Each type has its own field names on purpose, so one type can never read
another's data by mistake.

### Merging and deleting

Two copies of a board (desktop and web, or two people) merge item by item:

- Every change to an item increases its `rev` by at least one past the
  highest `rev` the writer has seen (a Lamport counter).
- The copy with the higher `rev` wins. On a tie, the larger `updated_by`
  wins, so every copy picks the same one.
- A deleted item stays in the file with `"deleted": true` (a tombstone),
  so a deletion merges like any other change.

Readers hide deleted items. Writers never drop tombstones that other
copies may not have seen yet.

## Trash

- A deleted board's file moves to `trash/<ms>-board-<uuid>.json`.
- A deleted picture or video moves, with its sidecar, into
  `trash/<ms>-media-<hash>/`. Restoring moves them back.
- `<ms>` is the time it was deleted.

## Versioning

New optional fields keep version 1: older readers carry them along
unchanged. A new item type, or any change that older readers would get
wrong, raises `version` in `wall.json` and in the boards that use it.
Readers refuse a board or library with a version newer than they know,
rather than open it and lose data.
