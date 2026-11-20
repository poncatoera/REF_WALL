//! `wall-cli`: look into and change a `.wall` library without opening Wall.
//!
//! ```text
//! wall-cli info   <library>
//! wall-cli boards <library>
//! wall-cli find   <library> <word>...        titles, tags and suggested tags
//! wall-cli export <library> <board> <folder> copy a board's pictures and videos out
//! wall-cli import <library> <file>... [--board <board>]   pictures and videos (videos need ffmpeg)
//! wall-cli check  <library>                  look for missing or damaged files
//! wall-cli import-old <old Wall library folder> <library>
//!                                            bring in the old Electron Wall's boards
//! ```
//!
//! `<board>` is a board's name or id. The format is described in
//! `docs/wall-format.md`.

use wall_import::media::import_one;
use wall_import::{old_wall, Res};

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use wall_format::{Board, BoardItem, ContentHash, Frame, ItemKind, ItemSidecar, Library, MediaKind};

const USAGE: &str = "usage:
  wall-cli info   <library>
  wall-cli boards <library>
  wall-cli find   <library> <word>...
  wall-cli export <library> <board> <folder>
  wall-cli import <library> <file>... [--board <board>]
  wall-cli check  <library>
  wall-cli import-old <old Wall library folder> <library>";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(out) => print!("{out}"),
        Err(e) => {
            eprintln!("wall-cli: {e}");
            std::process::exit(1);
        }
    }
}

fn run(args: &[String]) -> Res<String> {
    let (cmd, rest) = args.split_first().ok_or(USAGE)?;
    if cmd == "import-old" {
        let [old, new] = rest else { return Err(USAGE.into()) };
        return old_wall::import(Path::new(old), Path::new(new), now_ms()).map(|r| r.text());
    }
    let lib_path = rest.first().ok_or(USAGE)?;
    let lib = Library::open(lib_path).map_err(|e| e.to_string())?;
    let rest = &rest[1..];
    match cmd.as_str() {
        "info" => info(&lib),
        "boards" => boards(&lib),
        "find" if !rest.is_empty() => find(&lib, rest),
        "export" if rest.len() == 2 => export(&lib, &rest[0], Path::new(&rest[1])),
        "import" if !rest.is_empty() => {
            let (files, board) = match rest.iter().position(|a| a == "--board") {
                Some(i) => (&rest[..i], Some(rest.get(i + 1).ok_or("--board needs a name or id")?.as_str())),
                None => (rest, None),
            };
            import(&lib, files, board, now_ms())
        }
        "check" => check(&lib),
        _ => Err(USAGE.into()),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn all_boards(lib: &Library) -> Res<Vec<Board>> {
    let mut out = Vec::new();
    for id in lib.board_ids().map_err(|e| e.to_string())? {
        out.push(lib.read_board(id).map_err(|e| e.to_string())?);
    }
    out.sort_by_key(|b| b.created_ms);
    Ok(out)
}

fn sidecars(lib: &Library) -> Res<Vec<ItemSidecar>> {
    Ok(lib.sidecars().map_err(|e| e.to_string())?.into_iter().filter_map(Result::ok).collect())
}

/// A board by name (case-insensitive) or id.
fn pick_board(boards: &[Board], key: &str) -> Res<Board> {
    let k = key.to_lowercase();
    let hits: Vec<&Board> = boards.iter().filter(|b| b.id.to_string() == k || b.name.to_lowercase() == k).collect();
    match hits[..] {
        [b] => Ok(b.clone()),
        [] => Err(format!("no board called \"{key}\"; see `wall-cli boards`")),
        _ => Err(format!("several boards are called \"{key}\"; use its id from `wall-cli boards`")),
    }
}

fn info(lib: &Library) -> Res<String> {
    let m = lib.manifest();
    let boards = all_boards(lib)?;
    let scs = sidecars(lib)?;
    let images = scs.iter().filter(|s| s.kind == MediaKind::Image).count();
    let videos = scs.len() - images;
    let bytes: u64 = scs.iter().map(|s| s.bytes).sum();
    let mut tags: BTreeMap<&str, usize> = BTreeMap::new();
    for s in &scs {
        for t in &s.tags {
            *tags.entry(t).or_default() += 1;
        }
    }
    let mut top: Vec<(&str, usize)> = tags.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let top: Vec<String> = top.iter().take(12).map(|(t, n)| format!("{t} ({n})")).collect();
    Ok(format!(
        "{}\n  format    {} v{}\n  id        {}\n  boards    {}\n  pictures  {images}\n  videos    {videos}\n  media     {:.1} MB\n  tags      {}\n",
        m.name,
        m.format,
        m.version,
        m.id,
        boards.len(),
        bytes as f64 / 1e6,
        if top.is_empty() { "none".into() } else { top.join(", ") }
    ))
}

fn boards(lib: &Library) -> Res<String> {
    let boards = all_boards(lib)?;
    let mut children: HashMap<Option<uuid::Uuid>, Vec<&Board>> = HashMap::new();
    for b in &boards {
        children.entry(b.parent).or_default().push(b);
    }
    fn walk(out: &mut String, children: &HashMap<Option<uuid::Uuid>, Vec<&Board>>, parent: Option<uuid::Uuid>, depth: usize) {
        for b in children.get(&parent).into_iter().flatten() {
            let n = b.live_items().count();
            out.push_str(&format!("{}{}  ({n} items)  {}\n", "  ".repeat(depth), b.name, b.id));
            walk(out, children, Some(b.id), depth + 1);
        }
    }
    let mut out = String::new();
    walk(&mut out, &children, None, 0);
    // Boards whose parent is missing still get listed.
    let known: std::collections::HashSet<_> = boards.iter().map(|b| b.id).collect();
    for b in boards.iter().filter(|b| b.parent.is_some_and(|p| !known.contains(&p))) {
        out.push_str(&format!("{}  (parent missing)  {}\n", b.name, b.id));
    }
    Ok(out)
}

fn find(lib: &Library, words: &[String]) -> Res<String> {
    let words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut hits: Vec<ItemSidecar> = sidecars(lib)?
        .into_iter()
        .filter(|s| {
            let hay: Vec<String> = s.title.iter().chain(&s.tags).cloned().chain(s.auto_tags.iter().map(|t| t.tag.clone())).map(|t| t.to_lowercase()).collect();
            words.iter().all(|w| hay.iter().any(|h| h.contains(w.as_str())))
        })
        .collect();
    hits.sort_by_key(|s| std::cmp::Reverse(s.added_ms));
    let mut out = String::new();
    for s in &hits {
        out.push_str(&format!("{}\t{}\t{}\n", s.title.as_deref().unwrap_or("(untitled)"), s.tags.join(","), lib.media_path(&s.hash, &s.ext).display()));
    }
    out.push_str(&format!("{} found\n", hits.len()));
    Ok(out)
}

/// A file name that is safe on Windows, macOS and Linux.
fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let t = cleaned.trim().trim_end_matches('.').to_owned();
    if t.is_empty() { "untitled".into() } else { t }
}

fn export(lib: &Library, board: &str, dir: &Path) -> Res<String> {
    let board = pick_board(&all_boards(lib)?, board)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut used = std::collections::HashSet::new();
    let mut n = 0;
    let mut items: Vec<&BoardItem> = board.live_items().collect();
    items.sort_by(|a, b| (a.frame.y, a.frame.x).partial_cmp(&(b.frame.y, b.frame.x)).unwrap_or(std::cmp::Ordering::Equal));
    for item in items {
        let ItemKind::Media { hash, .. } = &item.kind else { continue };
        if !used.insert(hash.clone()) {
            continue;
        }
        let Ok(sc) = lib.read_sidecar(hash) else { continue };
        let stem = safe_name(sc.title.as_deref().map(|t| t.rsplit_once('.').map_or(t, |(s, _)| s)).unwrap_or(hash.as_str()));
        let mut name = format!("{stem}.{}", sc.ext);
        let mut k = 2;
        while dir.join(&name).exists() {
            name = format!("{stem} {k}.{}", sc.ext);
            k += 1;
        }
        let from = lib.media_path(hash, &sc.ext);
        std::fs::copy(&from, dir.join(&name)).map_err(|e| format!("{}: {e}", from.display()))?;
        n += 1;
    }
    Ok(format!("exported {n} files from \"{}\" to {}\n", board.name, dir.display()))
}

fn import(lib: &Library, files: &[String], board: Option<&str>, now: u64) -> Res<String> {
    let boards = all_boards(lib)?;
    let mut board = match board {
        Some(key) => pick_board(&boards, key)?,
        None => boards.iter().find(|b| b.parent.is_none()).cloned().ok_or("the library has no home board")?,
    };
    // New items go in a row under everything already on the board.
    let bottom = board.live_items().map(|i| i.frame.y + i.frame.h).fold(0.0f64, f64::max);
    let mut x = 0.0;
    let y = if board.live_items().next().is_some() { bottom + 80.0 } else { 0.0 };
    let top_z = board.live_items().map(|i| i.frame.z).fold(0.0f64, f64::max);
    let top_rev = board.items.values().map(|i| i.rev).max().unwrap_or(0);
    let mut added = Vec::new();
    let mut skipped = Vec::new();
    for (k, file) in files.iter().enumerate() {
        let path = PathBuf::from(file);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_owned();
        let result = import_one(lib, &path, &name, now);
        let (hash, w, h) = match result {
            Ok(v) => v,
            Err(e) => {
                skipped.push(format!("{name}: {e}"));
                continue;
            }
        };
        let width = 320.0;
        let height = width * h as f64 / w.max(1) as f64;
        let id = uuid::Uuid::new_v4();
        board.items.insert(
            id,
            BoardItem {
                id,
                frame: Frame { x, y, w: width, h: height, rotation: 0.0, z: top_z + 1.0 + k as f64, text_scale: 1.0 },
                kind: ItemKind::Media { hash, crop: None },
                rev: top_rev + 1,
                updated_ms: now,
                updated_by: "wall-cli".into(),
                deleted: false,
            },
        );
        x += width + 24.0;
        added.push(name);
    }
    board.updated_ms = board.updated_ms.max(now);
    lib.write_board(&board).map_err(|e| e.to_string())?;
    let mut out = format!("added {} to \"{}\"\n", added.len(), board.name);
    for s in skipped {
        out.push_str(&format!("skipped {s}\n"));
    }
    Ok(out)
}

fn check(lib: &Library) -> Res<String> {
    let mut problems = Vec::new();
    let mut known = std::collections::HashSet::new();
    for sc in lib.sidecars().map_err(|e| e.to_string())? {
        match sc {
            Err(e) => problems.push(format!("unreadable sidecar: {e}")),
            Ok(sc) => {
                let path = lib.media_path(&sc.hash, &sc.ext);
                match std::fs::read(&path) {
                    Err(_) => problems.push(format!("missing media file {}", path.display())),
                    Ok(bytes) if ContentHash::of(&bytes) != sc.hash => problems.push(format!("damaged media file {} (hash differs)", path.display())),
                    Ok(_) => {}
                }
                known.insert(sc.hash);
            }
        }
    }
    let mut n_boards = 0;
    for id in lib.board_ids().map_err(|e| e.to_string())? {
        match lib.read_board(id) {
            Err(e) => problems.push(format!("unreadable board {id}: {e}")),
            Ok(b) => {
                n_boards += 1;
                for i in b.live_items() {
                    if let ItemKind::Media { hash, .. } = &i.kind {
                        if !known.contains(hash) {
                            problems.push(format!("board \"{}\" uses {hash}, which is not in the library", b.name));
                        }
                    }
                }
            }
        }
    }
    let mut out = format!("checked {} media files and {n_boards} boards\n", known.len());
    if problems.is_empty() {
        out.push_str("no problems found\n");
        Ok(out)
    } else {
        for p in &problems {
            out.push_str(&format!("problem: {p}\n"));
        }
        Err(format!("{out}{} problems", problems.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn import_find_export_check() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Refs.wall");
        let lib = Library::create(&root, "Refs", 1).unwrap();
        let mut home = Board::new("Home", 1);
        let mut moods = Board::new("Moods", 2);
        moods.parent = Some(home.id);
        home.updated_ms = 1;
        lib.write_board(&home).unwrap();
        lib.write_board(&moods).unwrap();
        let files = dir.path().join("in");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("sky.png"), png(40, 80, [30, 80, 220])).unwrap();
        std::fs::write(files.join("sun.png"), png(80, 40, [240, 140, 40])).unwrap();
        std::fs::write(files.join("clip.mp4"), b"not really").unwrap();
        let root_s = root.to_str().unwrap();
        let f = |n: &str| files.join(n).to_str().unwrap().to_owned();

        let out = run(&s(&["import", root_s, &f("sky.png"), &f("sun.png"), &f("clip.mp4"), "--board", "moods"])).unwrap();
        assert!(out.starts_with("added 2 to \"Moods\""), "{out}");
        assert!(out.contains("skipped clip.mp4"), "{out}");
        // Importing the same file again stores it once.
        run(&s(&["import", root_s, &f("sky.png")])).unwrap();
        let info = run(&s(&["info", root_s])).unwrap();
        assert!(info.contains("boards    2") && info.contains("pictures  2"), "{info}");

        let tree = run(&s(&["boards", root_s])).unwrap();
        assert!(tree.contains("Home  (1 items)") && tree.contains("\n  Moods  (2 items)"), "{tree}");

        let found = run(&s(&["find", root_s, "blue", "portrait"])).unwrap();
        assert!(found.contains("sky.png") && found.ends_with("1 found\n"), "{found}");

        let out_dir = dir.path().join("out");
        let msg = run(&s(&["export", root_s, "Moods", out_dir.to_str().unwrap()])).unwrap();
        assert!(msg.starts_with("exported 2 files"), "{msg}");
        assert!(out_dir.join("sky.png").is_file() && out_dir.join("sun.png").is_file());

        assert!(run(&s(&["check", root_s])).unwrap().contains("no problems found"));
        // Damage a media file: check reports it.
        let sc = lib.sidecars().unwrap().into_iter().next().unwrap().unwrap();
        std::fs::write(lib.media_path(&sc.hash, &sc.ext), b"oops").unwrap();
        let err = run(&s(&["check", root_s])).unwrap_err();
        assert!(err.contains("damaged media file"), "{err}");
    }

    #[test]
    fn imports_and_converts_videos() {
        let Some(ff) = wall_media::ffmpeg::ffmpeg() else {
            eprintln!("skipped: no ffmpeg");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Clips.wall");
        let lib = Library::create(&root, "Clips", 1).unwrap();
        lib.write_board(&Board::new("Home", 1)).unwrap();
        // A red clip as MPEG-4 Part 2 in .mov, which the rules convert to H.264 MP4.
        let clip = dir.path().join("red.mov");
        let ok = wall_media::ffmpeg::command(ff)
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=0xdc1e1e:s=320x180:d=2:r=25", "-c:v", "mpeg4"])
            .arg(&clip)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let out = run(&s(&["import", root.to_str().unwrap(), clip.to_str().unwrap()])).unwrap();
        assert!(out.starts_with("added 1"), "{out}");
        let sc = lib.sidecars().unwrap().into_iter().next().unwrap().unwrap();
        assert_eq!(sc.kind, MediaKind::Video);
        assert_eq!(sc.ext, "mp4");
        assert_eq!((sc.width, sc.height), (320, 180));
        assert!(sc.duration_ms.unwrap().abs_diff(2000) < 200, "{:?}", sc.duration_ms);
        assert!(sc.original.is_some(), "converted, so the original size is recorded");
        assert!(sc.auto_tags.iter().any(|t| t.tag == "red"), "{:?}", sc.auto_tags);
        let info = run(&s(&["info", root.to_str().unwrap()])).unwrap();
        assert!(info.contains("videos    1"), "{info}");
    }

    #[test]
    fn imports_an_old_wall_library() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("RefLoop Library");
        std::fs::create_dir_all(old.join("Walk cycles").join("Sub")).unwrap();
        std::fs::write(old.join("Walk cycles").join("sky.png"), png(400, 200, [30, 80, 220])).unwrap();
        let lib_json = serde_json::json!({
            "version": 1,
            "boards": [
                { "id": "a1b2c3d4e5", "name": "Walk cycles", "parentId": null, "folder": "Walk cycles", "createdAt": 1, "view": { "x": 0, "y": 0, "zoom": 1 } },
                { "id": "b1b2c3d4e5", "name": "Sub", "parentId": "a1b2c3d4e5", "folder": "Walk cycles\\Sub", "createdAt": 2 }
            ],
            "items": [
                { "id": "0000000001", "boardId": "a1b2c3d4e5", "kind": "image", "x": 10, "y": 20, "w": 400, "h": 200, "z": 3,
                  "status": "ready", "tags": ["Sky", "blue"], "notes": "look at the clouds", "title": "Sky", "file": "Walk cycles\\sky.png",
                  "width": 400, "height": 200, "crop": { "x": 0.1, "y": 0, "w": 0.8, "h": 1 }, "sourceUrl": "https://example.com/sky.png" },
                { "id": "0000000002", "boardId": "a1b2c3d4e5", "kind": "note", "x": 500, "y": 0, "w": 240, "h": 120, "z": 4,
                  "status": "ready", "text": "Heel strike on 9", "color": "orange", "scale": 1 },
                { "id": "0000000003", "boardId": "a1b2c3d4e5", "kind": "arrow", "x": null, "y": null, "w": null, "h": null, "z": 5,
                  "status": "ready", "from": { "id": "0000000002" }, "to": { "x": 0, "y": 0 }, "head": "end" },
                { "id": "0000000004", "boardId": "a1b2c3d4e5", "kind": "todo", "x": null, "y": null, "z": 6, "status": "ready", "scale": 1,
                  "title": "Shots", "tasks": [{ "id": "t1", "text": "Pose", "done": true }, { "id": "t2", "text": "Timing", "done": false }] },
                { "id": "0000000005", "boardId": "b1b2c3d4e5", "kind": "palette", "x": 0, "y": 0, "w": 400, "h": 170, "z": 1, "status": "ready",
                  "title": "Dusk", "swatches": [{ "hex": "#112233" }, { "hex": "#445566", "name": "sea" }] },
                { "id": "0000000006", "boardId": "a1b2c3d4e5", "kind": "video", "x": 0, "y": 0, "w": 10, "h": 10, "z": 1, "status": "error", "title": "Broken" },
                { "id": "0000000007", "boardId": "a1b2c3d4e5", "kind": "image", "x": 0, "y": 0, "w": 10, "h": 10, "z": 1, "status": "ready", "file": "..\\..\\secret.png" },
                { "id": "0000000008", "boardId": "a1b2c3d4e5", "kind": "frame", "x": -40, "y": -40, "w": 900, "h": 400, "z": 0, "status": "ready", "title": "Refs", "color": "pink" }
            ]
        });
        // Old Wall files start with a BOM sometimes.
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(serde_json::to_vec(&lib_json).unwrap());
        std::fs::write(old.join("library.json"), bytes).unwrap();
        let before = std::fs::read(old.join("library.json")).unwrap();

        let new = dir.path().join("Imported.wall");
        let r = old_wall::import(&old, &new, 99).unwrap();
        let text = r.text();
        assert!(text.starts_with("imported 2 boards, 6 items"), "{text}");
        assert!(text.contains("skipped video \"Broken\": it failed to download"), "{text}");
        assert!(text.contains("skipped image \"0000000007\": its file"), "a path out of the library is refused: {text}");
        assert_eq!(r.converted_to_notes, 1);
        assert_eq!(std::fs::read(old.join("library.json")).unwrap(), before, "the old library is untouched");

        let lib = Library::open(&new).unwrap();
        let boards = all_boards(&lib).unwrap();
        let home = boards.iter().find(|b| b.parent.is_none()).unwrap();
        let walk = boards.iter().find(|b| b.name == "Walk cycles").unwrap();
        let sub = boards.iter().find(|b| b.name == "Sub").unwrap();
        assert_eq!(walk.parent, Some(home.id));
        assert_eq!(sub.parent, Some(walk.id));
        assert!(home.live_items().any(|i| i.kind == ItemKind::Board { board_id: walk.id }), "home has a card for the old board");
        assert!(walk.live_items().any(|i| i.kind == ItemKind::Board { board_id: sub.id }), "parent has a card for the sub-board");

        let media = walk.live_items().find_map(|i| match &i.kind { ItemKind::Media { hash, crop } => Some((i, hash, crop)), _ => None }).unwrap();
        assert_eq!((media.0.frame.x, media.0.frame.y, media.0.frame.w, media.0.frame.z), (10.0, 20.0, 400.0, 3.0));
        assert_eq!(media.2.unwrap().x, 0.1);
        let sc = lib.read_sidecar(media.1).unwrap();
        assert_eq!(sc.title.as_deref(), Some("Sky"));
        assert_eq!(sc.tags, vec!["sky", "blue"]);
        assert_eq!(sc.source_url.as_deref(), Some("https://example.com/sky.png"));
        assert_eq!(sc.extra["old_wall"]["notes"], "look at the clouds");

        let note = walk.live_items().find(|i| matches!(i.kind, ItemKind::Note { .. })).unwrap();
        assert_eq!(note.kind, ItemKind::Note { text: "Heel strike on 9".into(), color: Some("#e8a15d".into()) });
        let arrow = walk.live_items().find_map(|i| match &i.kind { ItemKind::Arrow { from, to } => Some((from.clone(), to.clone())), _ => None }).unwrap();
        assert_eq!(arrow.0, wall_format::ArrowEnd::Item { id: note.id });
        assert_eq!(arrow.1, wall_format::ArrowEnd::Point { x: 0.0, y: 0.0 });
        let todo = walk.live_items().find(|i| matches!(i.kind, ItemKind::Todo { .. })).unwrap();
        assert!(todo.frame.y > 200.0, "an unplaced item goes under the rest");
        if let ItemKind::Todo { title, entries } = &todo.kind {
            assert_eq!((title.as_str(), entries.len(), entries[0].done), ("Shots", 2, true));
        }
        assert!(walk.live_items().any(|i| i.kind == ItemKind::Group { name: "Refs".into(), color: Some("#b083f0".into()) }));
        assert!(sub.live_items().any(|i| matches!(&i.kind, ItemKind::Note { text, .. } if text == "Dusk\n#112233\n#445566 sea")));

        // Into a library that is open in Wall: a new board, home untouched.
        let home_before = std::fs::read(new.join("boards").join(format!("{}.json", home.id))).unwrap();
        let r = old_wall::import_into(&old, &new, Some("Old Wall"), 100).unwrap();
        let c = lib.read_board(r.container.unwrap()).unwrap();
        assert_eq!((c.name.as_str(), c.parent), ("Old Wall", Some(home.id)));
        assert!(c.live_items().any(|i| matches!(i.kind, ItemKind::Board { .. })));
        assert_eq!(std::fs::read(new.join("boards").join(format!("{}.json", home.id))).unwrap(), home_before);

        // Refuses to import into the old library itself.
        assert!(old_wall::import(&old, &old.join("x.wall"), 1).is_err());
    }

    #[test]
    fn helpful_errors() {
        assert!(run(&[]).unwrap_err().contains("usage"));
        let dir = tempfile::tempdir().unwrap();
        let err = run(&s(&["info", dir.path().to_str().unwrap()])).unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(safe_name("a/b:c?.jpg"), "a_b_c_.jpg");
        assert_eq!(safe_name(" . "), "untitled");
    }
}
