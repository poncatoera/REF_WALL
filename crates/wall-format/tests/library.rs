use uuid::Uuid;
use wall_format::*;

fn item(id: Uuid, rev: u64, by: &str, x: f64) -> BoardItem {
    BoardItem {
        id,
        frame: Frame { x, y: 0.0, w: 100.0, h: 100.0, ..Default::default() },
        kind: ItemKind::Note { text: format!("rev {rev}"), color: None },
        rev,
        updated_ms: rev * 10,
        updated_by: by.into(),
        deleted: false,
    }
}

#[test]
fn create_open_and_reject_second_create() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Refs.wall");
    let lib = Library::create(&root, "Refs", 1).unwrap();
    assert_eq!(lib.manifest().version, FORMAT_VERSION);
    let again = Library::open(&root).unwrap();
    assert_eq!(again.manifest().id, lib.manifest().id);
    assert!(matches!(Library::create(&root, "x", 2), Err(Error::AlreadyExists(_))));
}

#[test]
fn open_rejects_non_library_and_newer_versions() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(Library::open(dir.path()), Err(Error::NotALibrary(_))));

    let lib = Library::create(dir.path(), "x", 1).unwrap();
    let path = lib.root().join("wall.json");
    let text = std::fs::read_to_string(&path).unwrap().replace("\"version\": 1", "\"version\": 99");
    std::fs::write(&path, text).unwrap();
    assert!(matches!(Library::open(dir.path()), Err(Error::TooNew { found: 99, .. })));
}

#[test]
fn import_dedupes_by_content() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::create(dir.path(), "x", 1).unwrap();
    let a = lib.import_bytes(b"hello", "JPG").unwrap();
    let b = lib.import_bytes(b"hello", ".jpg").unwrap();
    assert!(matches!(a, ImportOutcome::Added(_)));
    assert!(matches!(b, ImportOutcome::Duplicate(_)));
    assert_eq!(a.hash(), b.hash());
    let path = lib.media_path(a.hash(), "jpg");
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    assert!(path.starts_with(lib.root().join("media").join(a.hash().shard())));
    assert!(lib.import_bytes(b"x", "../evil").is_err());
}

#[test]
fn sidecar_round_trip_keeps_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::create(dir.path(), "x", 1).unwrap();
    let hash = lib.import_bytes(b"img", "png").unwrap().hash().clone();
    let sidecar = ItemSidecar {
        hash: hash.clone(),
        kind: MediaKind::Image,
        ext: "png".into(),
        width: 2048,
        height: 1024,
        bytes: 3,
        duration_ms: None,
        fps: None,
        added_ms: 5,
        title: None,
        source_url: Some("https://example.com/a".into()),
        tags: vec!["neon".into()],
        auto_tags: vec![AutoTag { tag: "pink".into(), source: "local:color".into(), confidence: None }],
        colors: vec!["#ff00aa".into()],
        original: Some(OriginalInfo { width: 4096, height: 2048, bytes: 9_000_000, kept_as: None }),
        range: None,
        extra: Default::default(),
    };
    lib.write_sidecar(&sidecar).unwrap();

    // Another app adds its own field.
    let path = lib.root().join("items").join(format!("{hash}.json"));
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["blender_asset"] = serde_json::json!({"collection": "Refs"});
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();

    let read = lib.read_sidecar(&hash).unwrap();
    assert_eq!(read.extra["blender_asset"]["collection"], "Refs");
    lib.write_sidecar(&read).unwrap();
    assert_eq!(lib.read_sidecar(&hash).unwrap(), read);
    assert!(lib.has_media(&hash));
}

#[test]
fn board_round_trip_list_and_trash() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::create(dir.path(), "x", 1).unwrap();
    let mut board = Board::new("Moodboard", 1);
    let note = item(Uuid::new_v4(), 1, "a", 0.0);
    board.merge_item(note.clone());
    let arrow = BoardItem {
        kind: ItemKind::Arrow { from: ArrowEnd::Item { id: note.id }, to: ArrowEnd::Point { x: 5.0, y: 6.0 } },
        ..item(Uuid::new_v4(), 1, "a", 0.0)
    };
    board.merge_item(arrow);
    lib.write_board(&board).unwrap();

    assert_eq!(lib.read_board(board.id).unwrap(), board);
    assert_eq!(lib.board_ids().unwrap(), vec![board.id]);
    lib.trash_board(board.id, 99).unwrap();
    assert!(lib.board_ids().unwrap().is_empty());
}

#[test]
fn item_json_is_flat_and_tagged() {
    let it = item(Uuid::nil(), 3, "dev", 1.5);
    let v = serde_json::to_value(&it).unwrap();
    assert_eq!(v["type"], "note");
    assert_eq!(v["text"], "rev 3");
    assert!(v.get("deleted").is_none(), "false tombstone flag is omitted");
}

#[test]
fn merge_is_order_independent() {
    let id = Uuid::new_v4();
    let edits = [item(id, 2, "alice", 10.0), item(id, 3, "bob", 20.0), item(id, 3, "carol", 30.0), item(id, 1, "dan", 40.0)];

    let mut results = Vec::new();
    for order in [[0, 1, 2, 3], [3, 2, 1, 0], [2, 0, 3, 1], [1, 3, 0, 2]] {
        let mut b = Board::new("b", 0);
        b.id = Uuid::nil();
        for i in order {
            b.merge_item(edits[i].clone());
        }
        results.push(b.items[&id].clone());
    }
    assert!(results.iter().all(|r| r == &results[0]));
    assert_eq!(results[0].updated_by, "carol", "same rev: larger device id wins");

    // Deletion syncs like an edit and hides the item.
    let mut b = Board::new("b", 0);
    b.merge_item(edits[1].clone());
    b.merge_item(BoardItem { deleted: true, ..item(id, 4, "alice", 0.0) });
    assert_eq!(b.live_items().count(), 0);
    assert!(!b.merge_item(edits[2].clone()), "older edit can't resurrect it");
}
