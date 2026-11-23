//! Cloud sync, the app's part: what's in memory that may have changed, and
//! merging what came down. Files and the network are the shell's.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;
use wall_format::{Board, BoardItem, ContentHash, ItemKind, ItemSidecar, MediaKind};
use wall_sync::{Incoming, Meta, State};

use crate::sidebar::BoardMeta;
use crate::{App, AppError, Camera, Document, Picture};

impl App {
    /// Boards in memory: the open one and those visited. None while a drag
    /// or typing is going on, when they hold half-finished edits.
    pub fn sync_boards(&self) -> Option<Vec<Board>> {
        if self.busy_editing() {
            return None;
        }
        Some(std::iter::once(&self.doc.board).chain(self.parked.values().map(|(d, _)| &d.board)).cloned().collect())
    }

    /// A number that changes with any in-memory board's items, name or place.
    pub fn sync_stamp(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut docs: Vec<_> = std::iter::once(&self.doc).chain(self.parked.values().map(|(d, _)| d)).map(|d| (d.board.id, d.version, &d.board.name, d.board.parent)).collect();
        docs.sort();
        docs.hash(&mut h);
        h.finish()
    }

    /// Merges what came from the cloud. Returns the boards whose name and
    /// place stay as they are here (changed here, not sent yet), or None if
    /// it can't be done right now (a drag or typing is going on).
    pub fn sync_apply(&mut self, inc: &Incoming, state: &State) -> Option<BTreeSet<Uuid>> {
        if self.busy_editing() {
            return None;
        }
        let mut kept = BTreeSet::new();
        for m in inc.boards.iter().filter(|m| !m.deleted) {
            let local = self.sync_read(m.id);
            if !state.takes_meta(m, local.as_ref().map(Meta::of).as_ref()) {
                kept.insert(m.id);
                continue;
            }
            let mut b = local.unwrap_or_else(|| wall_sync::new_board(m));
            b.name = m.name.clone();
            b.parent = m.parent;
            self.sync_write(b);
        }
        let mut by_board: BTreeMap<Uuid, Vec<&BoardItem>> = BTreeMap::new();
        for (b, i) in &inc.items {
            by_board.entry(*b).or_default().push(i);
        }
        for (id, items) in by_board {
            // Items of a board that isn't here (say, deleted since) have no place.
            let Some(mut b) = self.sync_read(id) else { continue };
            let mut changed = false;
            for i in items {
                changed |= b.merge_item(i.clone());
            }
            if changed {
                self.sync_write(b);
            }
        }
        for id in inc.boards.iter().filter(|m| m.deleted).map(|m| m.id).chain(inc.out_of_reach.iter().copied()) {
            self.sync_remove(id);
        }
        for sc in &inc.sidecars {
            // Only details of files that are here; the rest come with the file.
            let Some(local) = self.sync_sidecar(&sc.hash) else { continue };
            let here = serde_json::to_value(&local).ok();
            if here.is_none() || state.sidecars.get(&sc.hash) == here.as_ref() {
                self.sync_put_sidecar(sc.clone());
            }
        }
        let live: BTreeSet<Uuid> = self.doc.board.live_items().map(|i| i.id).collect();
        self.selection.retain(|id| live.contains(id));
        self.sync_load_pictures();
        self.scene.invalidate();
        self.needs_redraw = true;
        Some(kept)
    }

    /// A picture or video file has been stored in the library: show it.
    pub fn sync_media_arrived(&mut self, sc: ItemSidecar) {
        self.sync_put_sidecar(sc);
        self.sync_load_pictures();
        self.needs_redraw = true;
    }

    // ---- without a library folder (the web, opened by a share link) ----

    /// Details of pictures and videos, whose files come when they're shown.
    pub fn sync_know(&mut self, sidecars: &[ItemSidecar]) {
        for s in sidecars {
            self.sidecars.insert(s.hash.clone(), s.clone());
        }
        self.needs_redraw = true;
    }

    /// Every picture's and video's details.
    pub fn sync_sidecars(&self) -> Vec<ItemSidecar> {
        self.sidecars.values().cloned().collect()
    }

    /// What the open board shows that has no file here yet.
    pub fn sync_missing_here(&self) -> Vec<ItemSidecar> {
        let mut seen = BTreeSet::new();
        self.doc
            .board
            .live_items()
            .filter_map(|i| match &i.kind {
                ItemKind::Media { hash, .. } => Some(hash),
                _ => None,
            })
            .filter(|h| !self.pictures.contains_key(*h) && !self.videos.contains_key(*h) && seen.insert((*h).clone()))
            .filter_map(|h| self.sidecars.get(h).cloned())
            .collect()
    }

    /// A file that was fetched. True for a video, which needs a player.
    pub fn sync_media_bytes(&mut self, sc: &ItemSidecar, bytes: &[u8]) -> Result<bool, AppError> {
        self.needs_redraw = true;
        if sc.kind == MediaKind::Video {
            self.videos.insert(sc.hash.clone(), (sc.ext.clone(), sc.width, sc.height));
            return Ok(true);
        }
        // A small file can claim to be a huge picture: limits before decoding.
        let mut r = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| AppError::Other(e.to_string()))?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(300_000_000);
        r.limits(limits);
        let rgba = r.decode()?.to_rgba8();
        let (width, height) = rgba.dimensions();
        self.pictures.insert(sc.hash.clone(), Picture::Pending { rgba: rgba.into_raw(), width, height });
        Ok(false)
    }

    /// Opens the top of what a link reaches, in place of the empty board
    /// the page started with.
    pub fn sync_open_top(&mut self, blank: Uuid) -> bool {
        let top = self.board_meta.iter().filter(|(id, m)| **id != blank && m.parent.is_none_or(|p| !self.board_meta.contains_key(&p))).min_by_key(|(_, m)| m.created_ms).map(|(id, _)| *id);
        let Some(top) = top else { return false };
        self.go_to_board(top);
        self.nav.clear();
        self.parked.remove(&blank);
        self.board_meta.remove(&blank);
        self.board_names.remove(&blank);
        // It can arrive before the window's size is known.
        if self.viewport[0] > 1.0 {
            self.fit_view();
        } else {
            self.fit_on_resize = true;
        }
        self.doc.board.id == top
    }

    fn sync_sidecar(&self, hash: &ContentHash) -> Option<ItemSidecar> {
        self.sidecars.get(hash).cloned().or_else(|| self.library.as_ref()?.read_sidecar(hash).ok())
    }

    fn sync_put_sidecar(&mut self, sc: ItemSidecar) {
        if let Some(lib) = &self.library {
            if let Err(e) = lib.write_sidecar(&sc) {
                eprintln!("wall: could not save details: {e}");
                return;
            }
        }
        self.sidecars.insert(sc.hash.clone(), sc);
    }

    fn sync_load_pictures(&mut self) {
        if let Some(lib) = self.library.take() {
            self.load_pictures(&lib);
            self.library = Some(lib);
        }
    }

    /// A board as it is now: in memory, else from the library.
    fn sync_read(&self, id: Uuid) -> Option<Board> {
        if self.doc.board.id == id {
            return Some(self.doc.board.clone());
        }
        match self.parked.get(&id) {
            Some((d, _)) => Some(d.board.clone()),
            None => self.library.as_ref()?.read_board(id).ok(),
        }
    }

    /// Puts a merged board back where [`Self::sync_read`] found it. The open
    /// board keeps its undo history and is saved with the next autosave.
    fn sync_write(&mut self, b: Board) {
        self.board_names.insert(b.id, b.name.clone());
        let seen = self.board_meta.get(&b.id).map(|m| m.seen_ms);
        let mut meta = BoardMeta::of(&b);
        if let Some(seen) = seen {
            meta.seen_ms = seen;
        }
        self.board_meta.insert(b.id, meta);
        if self.doc.board.id == b.id {
            self.doc.board = b;
            self.doc.version += 1;
            return;
        }
        if let Some(lib) = &self.library {
            if let Err(e) = lib.write_board(&b) {
                eprintln!("wall: could not save board: {e}");
            }
        }
        match self.parked.get_mut(&b.id) {
            Some((d, _)) => {
                d.board = b;
                d.version += 1;
            }
            None if self.library.is_none() => {
                let device = self.doc.device.clone();
                self.parked.insert(b.id, (Document::new(b, device), Camera { center: [0.0, 0.0], zoom: 1.0 }));
            }
            None => {}
        }
    }

    /// Takes a board deleted elsewhere out of the lists; its file goes to
    /// the trash. The last board stays.
    fn sync_remove(&mut self, id: Uuid) {
        if self.sync_read(id).is_none() || self.board_meta.len() < 2 {
            return;
        }
        if self.doc.board.id == id {
            let parent = self.doc.board.parent.filter(|p| self.board_meta.contains_key(p) && *p != id);
            let Some(to) = parent.or_else(|| self.board_meta.iter().filter(|(b, _)| **b != id).min_by_key(|(_, m)| (m.parent.is_some(), m.created_ms)).map(|(b, _)| *b)) else { return };
            self.go_to_board(to);
            if self.doc.board.id == id {
                return;
            }
        }
        self.parked.remove(&id);
        if let Some(lib) = &self.library {
            if let Err(e) = lib.trash_board(id, self.now_ms) {
                eprintln!("wall: could not move board {id} to the trash: {e}");
            }
        }
        self.board_meta.remove(&id);
        self.board_names.remove(&id);
        self.board_order.retain(|b| *b != id);
        self.nav.retain(|b| *b != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Button, Key, Mods};
    use wall_format::{Frame, Library};

    fn incoming(app: &App, name: &str) -> (Incoming, Uuid) {
        let m = Meta { id: Uuid::new_v4(), name: name.into(), parent: Some(app.board_id()), created_ms: 5, deleted: false };
        let id = m.id;
        (Incoming { boards: vec![m], items: vec![(id, note("there", 1, "other"))], ..Default::default() }, id)
    }

    fn note(text: &str, rev: u64, by: &str) -> BoardItem {
        BoardItem { id: Uuid::new_v4(), frame: Frame { x: 5.0, y: 5.0, w: 100.0, h: 50.0, ..Default::default() }, kind: ItemKind::Note { text: text.into(), color: None }, rev, updated_ms: 1, updated_by: by.into(), deleted: false }
    }

    fn open(dir: &std::path::Path) -> App {
        let mut a = App::open_library(&dir.join("L.wall"), "here", 1).unwrap();
        a.resize([1000.0, 800.0]);
        a.set_time(1000);
        a
    }

    #[test]
    fn boards_and_items_from_elsewhere_arrive_and_are_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let home = a.board_id();
        let (mut inc, id) = incoming(&a, "From afar");
        inc.items.push((home, note("on the open board", 1, "other")));
        assert_eq!(a.sync_apply(&inc, &State::default()), Some(BTreeSet::new()));
        assert_eq!(a.item_count(), 1);
        assert_eq!(a.board_names[&id], "From afar");
        a.save_if_dirty().unwrap();
        drop(a);
        let mut b = open(dir.path());
        assert_eq!(b.item_count(), 1);
        b.open_board(id, true);
        assert_eq!((b.board_name().as_str(), b.item_count()), ("From afar", 1));
        // A visited board keeps taking changes, in memory and on disk.
        b.open_board(home, false);
        let more = Incoming { items: vec![(id, note("later", 1, "other"))], ..Default::default() };
        b.sync_apply(&more, &State::default()).unwrap();
        assert_eq!(Library::open(dir.path().join("L.wall")).unwrap().read_board(id).unwrap().live_items().count(), 2);
        b.open_board(id, true);
        assert_eq!(b.item_count(), 2);
    }

    #[test]
    fn nothing_changes_under_a_drag_and_undo_still_works_after() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        a.panel_open = false;
        a.camera = Camera { center: [500.0, 400.0], zoom: 1.0 };
        let mine = a.doc.add(vec![(Frame { x: 400.0, y: 300.0, w: 100.0, h: 100.0, ..Default::default() }, ItemKind::Note { text: "mine".into(), color: None })])[0];
        a.pointer_down(Button::Primary, [450.0, 350.0], Mods::default());
        a.pointer_move([600.0, 350.0]);
        let inc = Incoming { items: vec![(a.board_id(), note("theirs", 1, "other"))], ..Default::default() };
        assert_eq!(a.sync_apply(&inc, &State::default()), None);
        assert!(a.sync_boards().is_none());
        a.pointer_up(Button::Primary, [600.0, 350.0]);
        assert!(a.sync_apply(&inc, &State::default()).is_some());
        assert_eq!(a.item_count(), 2);
        a.key(Key::Undo);
        assert_eq!(a.doc.item(&mine).unwrap().frame.x, 400.0, "my move is undone");
        assert_eq!(a.item_count(), 2, "their note stays");
        // The undo is a change of its own, so it goes up.
        let mine = a.doc.item(&mine).unwrap();
        assert!(mine.rev > 2);
    }

    #[test]
    fn a_board_deleted_elsewhere_goes_to_the_trash_but_never_the_last_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let home = a.board_id();
        let (inc, id) = incoming(&a, "Short-lived");
        a.sync_apply(&inc, &State::default()).unwrap();
        a.open_board(id, true);
        let gone = Incoming { boards: vec![Meta { deleted: true, ..inc.boards[0].clone() }], ..Default::default() };
        a.sync_apply(&gone, &State::default()).unwrap();
        assert_eq!(a.board_id(), home);
        assert!(!a.board_names.contains_key(&id));
        let lib = Library::open(dir.path().join("L.wall")).unwrap();
        assert!(!lib.board_ids().unwrap().contains(&id));
        assert_eq!(std::fs::read_dir(dir.path().join("L.wall/trash")).unwrap().count(), 1);
        let last = Incoming { boards: vec![Meta { id: home, deleted: true, ..Default::default() }], ..Default::default() };
        a.sync_apply(&last, &State::default()).unwrap();
        assert_eq!(a.board_id(), home);
        assert!(lib.board_ids().unwrap().contains(&home));
    }

    #[test]
    fn a_shared_board_opens_without_a_library() {
        let mut a = App::new(Board::new("Wall", 0), "web");
        a.resize([1000.0, 800.0]);
        let blank = a.board_id();
        let top = Meta { id: Uuid::new_v4(), name: "Shared".into(), parent: Some(Uuid::new_v4()), created_ms: 1, deleted: false };
        let inner = Meta { id: Uuid::new_v4(), name: "Inside".into(), parent: Some(top.id), created_ms: 0, deleted: false };
        let mut png = Vec::new();
        image::RgbaImage::new(4, 3).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let hash = ContentHash::of(&png);
        let sc: ItemSidecar = serde_json::from_value(serde_json::json!({"hash": hash, "kind": "image", "ext": "png", "width": 4, "height": 3, "bytes": 9, "added_ms": 1, "tags": []})).unwrap();
        let mut pic = note("", 1, "x");
        pic.kind = ItemKind::Media { hash: hash.clone(), crop: None, look: Default::default() };
        let inc = Incoming { boards: vec![inner.clone(), top.clone()], items: vec![(top.id, pic), (top.id, note("hello", 1, "x"))], sidecars: vec![sc.clone()], ..Default::default() };
        a.sync_apply(&inc, &State::default()).unwrap();
        a.sync_know(&inc.sidecars);
        assert!(a.sync_open_top(blank));
        assert_eq!((a.board_name().as_str(), a.item_count(), a.board_meta.len()), ("Shared", 2, 2));
        assert_eq!(a.sync_missing_here().len(), 1);
        assert!(!a.sync_media_bytes(&sc, &png).unwrap());
        assert!(a.sync_missing_here().is_empty());
        assert!(a.sync_media_bytes(&sc, b"not a picture").is_err());
        // The same before the window's size is known: it fits when it is.
        let mut b = App::new(Board::new("Wall", 0), "web");
        let blank = b.board_id();
        b.sync_apply(&inc, &State::default()).unwrap();
        assert!(b.sync_open_top(blank));
        b.resize([1000.0, 800.0]);
        assert!(b.camera.zoom > 0.9, "not a speck: {}", b.camera.zoom);
        // Out of reach now: the board goes, what's left opens.
        let gone = Incoming { out_of_reach: vec![top.id], ..Default::default() };
        a.sync_apply(&gone, &State::default()).unwrap();
        assert_eq!(a.board_name(), "Inside");
    }

    #[test]
    fn a_name_changed_here_stays_until_its_sent() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let (inc, id) = incoming(&a, "Old name");
        let mut st = State::default();
        a.sync_apply(&inc, &st).unwrap();
        st.merged(&inc, &BTreeSet::new());
        a.rename_board(id, "My name".into());
        let theirs = Incoming { boards: vec![Meta { name: "Their name".into(), ..inc.boards[0].clone() }], ..Default::default() };
        assert_eq!(a.sync_apply(&theirs, &st), Some([id].into()));
        assert_eq!(a.board_names[&id], "My name");
    }
}
