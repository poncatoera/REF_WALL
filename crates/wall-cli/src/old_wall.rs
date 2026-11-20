//! Importer for the old Electron Wall ("RefLoop") library: one
//! `library.json` with every board and item, media in a folder per board.
//! The format is described in `wall-import/old-wall-format.md`.
//!
//! The old library is only read. Everything goes into a `.wall` library:
//! pictures and videos through the normal import rules (2048 px, 20 MB
//! H.264), cards, groups and arrows as items, sub-boards as boards. Fields
//! the new format has no place for yet are kept in each sidecar under
//! `old_wall`, so nothing is lost.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use uuid::Uuid;
use wall_format::{ArrowEnd, Board, BoardItem, ClipRange, ContentHash, Frame, ItemKind, Look, Library, Rect, TodoEntry};

use crate::Res;

const BY: &str = "old-wall-import";

#[derive(Default, Debug)]
pub struct Report {
    pub boards: usize,
    /// The board the old boards went into, with [`import_into`].
    pub container: Option<Uuid>,
    pub items: BTreeMap<&'static str, usize>,
    pub converted_to_notes: usize,
    pub skipped: Vec<String>,
}

impl Report {
    pub fn text(&self) -> String {
        let total: usize = self.items.values().sum();
        let mut out = format!("imported {} boards, {total} items\n", self.boards);
        for (k, n) in &self.items {
            out.push_str(&format!("  {k:<9} {n}\n"));
        }
        if self.converted_to_notes > 0 {
            out.push_str(&format!("  ({} link, palette, type or sketch cards became notes)\n", self.converted_to_notes));
        }
        for s in &self.skipped {
            out.push_str(&format!("skipped {s}\n"));
        }
        out
    }
}

/// Old colour names to the new palette (`panel::PALETTE`).
fn color(name: Option<&str>) -> Option<String> {
    let hex = match name? {
        "red" => "#e5534b",
        "orange" => "#e8a15d",
        "yellow" => "#e6c84f",
        "green" => "#57ab5a",
        "blue" => "#7aa2f7",
        "purple" | "pink" => "#b083f0",
        s if s.starts_with('#') && s.len() == 7 => s,
        _ => return None,
    };
    Some(hex.into())
}

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// A media path from `library.json` (relative, Windows separators), kept
/// inside the old library.
fn media_file(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.replace('\\', "/");
    let mut p = root.to_path_buf();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            s if s.contains(':') => return None,
            s => p.push(s),
        }
    }
    p.is_file().then_some(p)
}

/// Imports the old library at `old` into the `.wall` library at `new`
/// (created if needed). Returns what was imported.
pub fn import(old: &Path, new: &Path, now: u64) -> Res<Report> {
    import_into(old, new, None, now)
}

/// Like [`import`]. With `container`, the old boards go into a new board of
/// that name (inside the home board, which is left as it is) instead of
/// onto the home board: for importing into a library that is open in Wall,
/// which then adds a card for it. Its id is in [`Report::container`].
pub fn import_into(old: &Path, new: &Path, container: Option<&str>, now: u64) -> Res<Report> {
    let bytes = std::fs::read(old.join("library.json")).map_err(|e| format!("{}: {e} (is this an old Wall library?)", old.join("library.json").display()))?;
    let lib_json: Value = serde_json::from_slice(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).map_err(|e| format!("library.json: {e}"))?;
    let old_boards = lib_json["boards"].as_array().ok_or("library.json has no boards")?;
    let old_items = lib_json["items"].as_array().ok_or("library.json has no items")?;
    // The nearest folder of `new` that exists says where it will be.
    let new_at = new.ancestors().find_map(|p| std::fs::canonicalize(p).ok());
    if let (Ok(a), Some(b)) = (std::fs::canonicalize(old), new_at) {
        if b.starts_with(&a) {
            return Err("the new library can't be inside the old one".into());
        }
    }
    let lib = if new.join("wall.json").is_file() {
        Library::open(new).map_err(|e| e.to_string())?
    } else {
        let name = new.file_stem().and_then(|n| n.to_str()).unwrap_or("Wall");
        Library::create(new, name, now).map_err(|e| e.to_string())?
    };
    let mut report = Report::default();

    // The home board: the library's own, or a new one.
    let mut existing = Vec::new();
    for id in lib.board_ids().map_err(|e| e.to_string())? {
        existing.push(lib.read_board(id).map_err(|e| e.to_string())?);
    }
    existing.sort_by_key(|b| b.created_ms);
    let mut home = existing.into_iter().find(|b| b.parent.is_none()).unwrap_or_else(|| Board::new("Home", now));
    if let Some(name) = container {
        let mut c = Board::new(name, now);
        c.parent = Some(home.id);
        report.container = Some(c.id);
        home = c;
    }

    // Boards first, so items and board cards can point at them.
    let mut board_ids: HashMap<String, Uuid> = HashMap::new();
    let mut boards: Vec<(Board, Option<String>)> = Vec::new();
    for b in old_boards {
        let mut board = Board::new(text(b, "name").to_owned(), now);
        board.created_ms = b.get("createdAt").and_then(Value::as_u64).unwrap_or(now);
        board.extra.insert("old_wall".into(), json!({ "id": b.get("id"), "view": b.get("view"), "reel": b.get("reel") }));
        board_ids.insert(text(b, "id").to_owned(), board.id);
        boards.push((board, b.get("parentId").and_then(Value::as_str).map(str::to_owned)));
    }
    let item_ids: HashMap<String, Uuid> = old_items.iter().map(|i| (text(i, "id").to_owned(), Uuid::new_v4())).collect();
    let mut by_board: HashMap<Uuid, Vec<BoardItem>> = HashMap::new();
    let mut unplaced: HashMap<Uuid, Vec<BoardItem>> = HashMap::new();
    let mut converted: HashMap<String, ContentHash> = HashMap::new();

    for it in old_items {
        let kind = text(it, "kind");
        let label = format!("{kind} \"{}\"", if text(it, "title").is_empty() { text(it, "id") } else { text(it, "title") });
        let Some(&board) = board_ids.get(text(it, "boardId")) else {
            report.skipped.push(format!("{label}: its board is missing"));
            continue;
        };
        if text(it, "status") == "error" {
            report.skipped.push(format!("{label}: it failed to download in the old Wall"));
            continue;
        }
        let item_kind = match kind {
            "image" | "video" => match import_media(&lib, old, it, now, &mut converted) {
                Ok(k) => k,
                Err(e) => {
                    report.skipped.push(format!("{label}: {e}"));
                    continue;
                }
            },
            "note" => ItemKind::Note { text: text(it, "text").into(), color: color(it.get("color").and_then(Value::as_str).or(Some("yellow"))) },
            "label" => ItemKind::Label { text: text(it, "text").into() },
            "todo" => ItemKind::Todo {
                title: text(it, "title").into(),
                entries: it["tasks"]
                    .as_array()
                    .map(|t| t.iter().map(|e| TodoEntry { text: text(e, "text").into(), done: e["done"].as_bool().unwrap_or(false) }).collect())
                    .unwrap_or_default(),
            },
            "frame" => ItemKind::Group { name: text(it, "title").into(), color: color(it.get("color").and_then(Value::as_str).or(Some("blue"))) },
            "arrow" => {
                let end = |e: &Value| -> Option<ArrowEnd> {
                    match e.get("id").and_then(Value::as_str) {
                        Some(id) => item_ids.get(id).map(|&id| ArrowEnd::Item { id }),
                        None => Some(ArrowEnd::Point { x: num(e, "x")?, y: num(e, "y")? }),
                    }
                };
                match (end(&it["from"]), end(&it["to"])) {
                    (Some(from), Some(to)) => ItemKind::Arrow { from, to },
                    _ => {
                        report.skipped.push(format!("{label}: an end is missing"));
                        continue;
                    }
                }
            }
            "board" => match it.get("linkBoard").and_then(Value::as_str).and_then(|b| board_ids.get(b)) {
                Some(&board_id) => ItemKind::Board { board_id },
                None => {
                    report.skipped.push(format!("{label}: the board it opens is missing"));
                    continue;
                }
            },
            "palette" if it["swatches"].as_array().is_some_and(|w| w.iter().any(|c| c["hex"].is_string())) => {
                let colors = it["swatches"].as_array().unwrap().iter().filter_map(|c| c["hex"].as_str()).filter(|h| h.len() == 7 && h.starts_with('#')).map(str::to_lowercase).collect();
                ItemKind::Palette { colors, name: text(it, "title").to_owned() }
            }
            "link" if !text(it, "url").is_empty() => ItemKind::Link { url: text(it, "url").to_owned(), title: text(it, "title").to_owned() },
            "palette" | "link" | "type" | "sketch" => {
                report.converted_to_notes += 1;
                ItemKind::Note { text: card_text(kind, it), color: None }
            }
            other => {
                report.skipped.push(format!("{label}: unknown kind \"{other}\""));
                continue;
            }
        };
        let name = match &item_kind {
            ItemKind::Media { .. } => kind_name(kind),
            k => kind_name_of(k),
        };
        *report.items.entry(name).or_default() += 1;
        let (w, h) = default_size(kind, it);
        let id = item_ids[text(it, "id")];
        let placed = num(it, "x").zip(num(it, "y"));
        let (x, y) = placed.unwrap_or((0.0, 0.0));
        let item = BoardItem {
            id,
            frame: Frame { x, y, w: num(it, "w").unwrap_or(w), h: num(it, "h").unwrap_or(h), rotation: 0.0, z: num(it, "z").unwrap_or(0.0), text_scale: text_scale(kind, it) },
            kind: item_kind,
            rev: 1,
            updated_ms: now,
            updated_by: BY.into(),
            deleted: false,
        };
        if placed.is_some() || matches!(item.kind, ItemKind::Arrow { .. }) {
            by_board.entry(board).or_default().push(item);
        } else {
            unplaced.entry(board).or_default().push(item);
        }
    }

    // Items the old Wall hadn't placed yet go in a row under the rest.
    for (board, items) in unplaced {
        let placed = by_board.entry(board).or_default();
        let bottom = placed.iter().map(|i| i.frame.y + i.frame.h).fold(0.0, f64::max);
        let mut x = 0.0;
        for mut i in items {
            i.frame.x = x;
            i.frame.y = bottom + 80.0;
            x += i.frame.w + 24.0;
            placed.push(i);
        }
    }

    // Top-level old boards sit inside the home board, as board cards in a
    // row to the right of what is already there; sub-boards inside their
    // parent, with a card unless the old board had one.
    let right = home.live_items().map(|i| i.frame.x + i.frame.w).fold(f64::MIN, f64::max);
    let mut next_x = if right > f64::MIN { right + 120.0 } else { 0.0 };
    let top_z = home.live_items().map(|i| i.frame.z).fold(0.0, f64::max);
    let ids: Vec<Uuid> = boards.iter().map(|(b, _)| b.id).collect();
    for (k, (board, parent)) in boards.iter_mut().enumerate() {
        let parent_id = parent.as_ref().and_then(|p| board_ids.get(p)).copied().filter(|p| *p != board.id && ids.contains(p));
        board.parent = Some(parent_id.unwrap_or(home.id));
        let has_card = by_board.values().flatten().any(|i| matches!(i.kind, ItemKind::Board { board_id } if board_id == board.id));
        if has_card {
            continue;
        }
        let card = |x: f64, y: f64, z: f64| BoardItem {
            id: Uuid::new_v4(),
            frame: Frame { x, y, w: 240.0, h: 180.0, rotation: 0.0, z, text_scale: 1.0 },
            kind: ItemKind::Board { board_id: board.id },
            rev: 1,
            updated_ms: now,
            updated_by: BY.into(),
            deleted: false,
        };
        match parent_id {
            None => {
                let c = card(next_x, 0.0, top_z + 1.0 + k as f64);
                home.items.insert(c.id, c);
                next_x += 280.0;
            }
            Some(p) => {
                let items = by_board.entry(p).or_default();
                let bottom = items.iter().map(|i| i.frame.y + i.frame.h).fold(0.0, f64::max);
                let z = items.iter().map(|i| i.frame.z).fold(0.0, f64::max) + 1.0;
                items.push(card(0.0, bottom + 80.0, z));
            }
        }
    }
    home.updated_ms = now;
    lib.write_board(&home).map_err(|e| e.to_string())?;
    for (mut board, _) in boards {
        for i in by_board.remove(&board.id).unwrap_or_default() {
            board.items.insert(i.id, i);
        }
        board.updated_ms = now;
        lib.write_board(&board).map_err(|e| e.to_string())?;
        report.boards += 1;
    }
    Ok(report)
}

fn kind_name(kind: &str) -> &'static str {
    if kind == "video" { "videos" } else { "pictures" }
}

fn kind_name_of(k: &ItemKind) -> &'static str {
    match k {
        ItemKind::Media { .. } => "pictures",
        ItemKind::Note { .. } => "notes",
        ItemKind::Label { .. } => "labels",
        ItemKind::Todo { .. } => "to-dos",
        ItemKind::Group { .. } => "groups",
        ItemKind::Arrow { .. } => "arrows",
        ItemKind::Board { .. } => "boards",
        ItemKind::Link { .. } => "links",
        ItemKind::Stroke { .. } => "drawings",
        ItemKind::Palette { .. } => "palettes",
    }
}

/// How large the old Wall drew a card's text: cards carry `scale` (set
/// when made while zoomed out), labels a font `size` already times scale
/// (40 px at scale 1, against 96 board units per em for new labels).
fn text_scale(kind: &str, it: &Value) -> f64 {
    let s = match kind {
        "image" | "video" | "arrow" => 1.0,
        "label" => num(it, "size").map_or_else(|| num(it, "scale").unwrap_or(1.0), |px| px / 96.0),
        _ => num(it, "scale").unwrap_or(1.0),
    };
    if s.is_finite() && s > 0.0 { s.clamp(0.05, 64.0) } else { 1.0 }
}

/// Size of a card the old Wall stored without one: base size × scale.
fn default_size(kind: &str, it: &Value) -> (f64, f64) {
    let s = num(it, "scale").unwrap_or(1.0);
    let (w, h) = match kind {
        "note" => (240.0, 120.0),
        "label" => (220.0, 56.0),
        "todo" => (260.0, 140.0),
        "frame" => (820.0, 540.0),
        "palette" => (400.0, 170.0),
        "board" => (240.0, 180.0),
        "type" => (460.0, 240.0),
        "image" | "video" => {
            let (pw, ph) = (num(it, "width").unwrap_or(320.0), num(it, "height").unwrap_or(180.0));
            return (320.0, 320.0 * ph / pw.max(1.0));
        }
        _ => (300.0, 160.0),
    };
    (w * s, h * s)
}

/// What a card the new Wall can't show yet says, as note text.
fn card_text(kind: &str, it: &Value) -> String {
    let title = text(it, "title");
    let body = match kind {
        "link" => [text(it, "title"), text(it, "text"), text(it, "url")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"),
        "palette" => {
            let sw: Vec<String> = it["swatches"]
                .as_array()
                .map(|a| a.iter().map(|s| [text(s, "hex"), text(s, "name")].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ")).collect())
                .unwrap_or_default();
            format!("{}{}", if title.is_empty() { String::new() } else { format!("{title}\n") }, sw.join("\n"))
        }
        "type" => format!("{}{}", text(it, "text"), if text(it, "font").is_empty() { String::new() } else { format!("\n({})", text(it, "font")) }),
        _ => format!("Sketch{}", if title.is_empty() { String::new() } else { format!(": {title}") }),
    };
    if body.trim().is_empty() { format!("({kind} card from the old Wall)") } else { body }
}

/// Imports one picture or video and carries its metadata into the sidecar.
/// `done` maps an old file (its old content hash, else its path) to the
/// media it became, so a clip on several boards is converted only once.
fn import_media(lib: &Library, old: &Path, it: &Value, now: u64, done: &mut HashMap<String, ContentHash>) -> Res<ItemKind> {
    let file = media_file(old, text(it, "file")).ok_or_else(|| format!("its file {} is missing", text(it, "file")))?;
    let key = match text(it, "hash") {
        "" => file.to_string_lossy().into_owned(),
        h => format!("hash:{h}"),
    };
    let hash = match done.get(&key) {
        Some(h) => h.clone(),
        None => {
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_owned();
            let (h, _, _) = crate::media::import_one(lib, &file, &name, now)?;
            done.insert(key, h.clone());
            h
        }
    };
    let mut sc = lib.read_sidecar(&hash).map_err(|e| e.to_string())?;
    let title = text(it, "title");
    if !title.is_empty() {
        sc.title = Some(title.into());
    }
    if let Some(t) = it["tags"].as_array() {
        for tag in t.iter().filter_map(Value::as_str).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
            if !sc.tags.contains(&tag) {
                sc.tags.push(tag);
            }
        }
    }
    let url = [text(it, "sourceUrl"), text(it, "pageUrl")].into_iter().find(|s| s.starts_with("http"));
    if let (None, Some(u)) = (&sc.source_url, url) {
        sc.source_url = Some(u.into());
    }
    if let Some(p) = it["palette"].as_array() {
        let old: Vec<String> = p.iter().filter_map(|c| c["hex"].as_str()).filter(|h| h.len() == 7 && h.starts_with('#')).map(str::to_lowercase).collect();
        if !old.is_empty() && sc.colors.is_empty() {
            sc.colors = old;
        }
    }
    if let (Some(a), Some(b)) = (num(it, "loopIn"), num(it, "loopOut")) {
        let (a, b) = ((a * 1000.0).max(0.0) as u64, (b * 1000.0).max(0.0) as u64);
        let dur = sc.duration_ms.unwrap_or(u64::MAX);
        if b > a && a < dur {
            sc.range = Some(ClipRange { in_ms: a, out_ms: b.min(dur) });
        }
    }
    // Markers are frame numbers in the old Wall; labels stay in extra below.
    if let (Some(list), Some(fps)) = (it["markers"].as_array(), sc.fps.filter(|f| *f > 0.0)) {
        let mut ms: Vec<u64> = list.iter().filter_map(|m| m["f"].as_f64()).filter(|f| *f >= 0.0).map(|f| (f / fps as f64 * 1000.0).round() as u64).collect();
        ms.sort_unstable();
        ms.dedup();
        sc.markers = ms;
    }
    // What the new format has no field for yet, kept for later.
    let mut kept = Map::new();
    for k in ["notes", "markers", "speed", "pingpong", "onion", "strokes", "tracks", "lockTrack", "section", "aiDesc", "opacity", "origin", "id"] {
        if let Some(v) = it.get(k).filter(|v| !v.is_null() && *v != &json!("") && *v != &json!([])) {
            kept.insert(k.into(), v.clone());
        }
    }
    if !kept.is_empty() {
        sc.extra.insert("old_wall".into(), Value::Object(kept));
    }
    lib.write_sidecar(&sc).map_err(|e| e.to_string())?;
    let rotated = num(it, "rotate").is_some_and(|r| r != 0.0);
    let crop = it.get("crop").filter(|_| !rotated).and_then(|c| {
        let r = Rect { x: num(c, "x")?, y: num(c, "y")?, w: num(c, "w")?, h: num(c, "h")? };
        let full = r.x <= 0.0 && r.y <= 0.0 && r.w >= 1.0 && r.h >= 1.0;
        (r.w > 0.0 && r.h > 0.0 && !full).then_some(r)
    });
    let turns = num(it, "rotate").map_or(0, |r| ((r / 90.0).round() as i64).rem_euclid(4) as u8);
    let flag = |k: &str| it[k].as_bool().unwrap_or(false);
    let look = Look { turns, mirror: flag("mirror"), flip_v: flag("flipV"), grayscale: flag("grayscale") };
    Ok(ItemKind::Media { hash, crop, look })
}
