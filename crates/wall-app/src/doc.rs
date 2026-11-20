//! The open board plus undo/redo. Every change is recorded as before/after
//! copies of the items it touched, so undo is generic and an undo syncs to
//! other people like any other edit (it bumps `rev`).

use std::collections::BTreeSet;

use uuid::Uuid;
use wall_format::{Board, BoardItem, Frame, ItemKind};

#[derive(Debug, Clone, PartialEq)]
struct ItemChange {
    before: Option<BoardItem>,
    after: BoardItem,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Change {
    items: Vec<ItemChange>,
}

pub struct Document {
    pub board: Board,
    /// Stable id of this device or person, written to `updated_by`.
    pub device: String,
    undo: Vec<Change>,
    redo: Vec<Change>,
    /// Bumped on every change; the scene rebuilds when it moves.
    pub version: u64,
    clock_ms: u64,
}

impl Document {
    pub fn new(board: Board, device: impl Into<String>) -> Self {
        Self { board, device: device.into(), undo: Vec::new(), redo: Vec::new(), version: 0, clock_ms: 0 }
    }

    /// Wall-clock time for `updated_ms`; shells set it each frame.
    pub fn set_clock(&mut self, now_ms: u64) {
        self.clock_ms = now_ms;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn item(&self, id: &Uuid) -> Option<&BoardItem> {
        self.board.items.get(id).filter(|i| !i.deleted)
    }

    pub fn top_z(&self) -> f64 {
        self.board.live_items().map(|i| i.frame.z).fold(0.0, f64::max)
    }

    /// Writes `after` as a newer revision of the item.
    fn stamp(&self, mut item: BoardItem) -> BoardItem {
        let prev_rev = self.board.items.get(&item.id).map_or(0, |i| i.rev);
        item.rev = prev_rev + 1;
        item.updated_ms = self.clock_ms;
        item.updated_by = self.device.clone();
        item
    }

    fn commit(&mut self, afters: Vec<BoardItem>) -> bool {
        if afters.is_empty() {
            return false;
        }
        let mut change = Change::default();
        for after in afters {
            let after = self.stamp(after);
            let before = self.board.items.get(&after.id).cloned();
            self.board.items.insert(after.id, after.clone());
            change.items.push(ItemChange { before, after });
        }
        self.board.updated_ms = self.clock_ms;
        self.undo.push(change);
        self.redo.clear();
        self.version += 1;
        true
    }

    /// Adds new items on top of everything else. Returns their ids.
    pub fn add(&mut self, kinds: Vec<(Frame, ItemKind)>) -> Vec<Uuid> {
        let mut z = self.top_z();
        let items: Vec<BoardItem> = kinds
            .into_iter()
            .map(|(mut frame, kind)| {
                z += 1.0;
                frame.z = z;
                BoardItem { id: Uuid::new_v4(), frame, kind, rev: 0, updated_ms: 0, updated_by: String::new(), deleted: false }
            })
            .collect();
        let ids = items.iter().map(|i| i.id).collect();
        self.commit(items);
        ids
    }

    /// Moves items by (dx, dy) board units as one undoable step.
    pub fn translate(&mut self, ids: &BTreeSet<Uuid>, dx: f64, dy: f64) -> bool {
        if dx == 0.0 && dy == 0.0 {
            return false;
        }
        let afters = ids
            .iter()
            .filter_map(|id| self.item(id).cloned())
            .map(|mut i| {
                i.frame.x += dx;
                i.frame.y += dy;
                i
            })
            .collect();
        self.commit(afters)
    }

    /// Records a move that was already shown live during a drag: `originals`
    /// are the items as they were before the drag started.
    pub fn commit_drag(&mut self, originals: &[BoardItem], dx: f64, dy: f64) -> bool {
        // Put the originals back first so the change records the true "before".
        for o in originals {
            self.board.items.insert(o.id, o.clone());
        }
        let ids = originals.iter().map(|o| o.id).collect();
        let moved = self.translate(&ids, dx, dy);
        if !moved {
            self.version += 1;
        }
        moved
    }

    /// Shows a drag in progress without recording it.
    pub fn preview_offset(&mut self, originals: &[BoardItem], dx: f64, dy: f64) {
        for o in originals {
            let mut i = o.clone();
            i.frame.x += dx;
            i.frame.y += dy;
            self.board.items.insert(i.id, i);
        }
        self.version += 1;
    }

    /// Deletes items (as tombstones, so the deletion syncs).
    pub fn delete(&mut self, ids: &BTreeSet<Uuid>) -> bool {
        let afters = ids
            .iter()
            .filter_map(|id| self.item(id).cloned())
            .map(|mut i| {
                i.deleted = true;
                i
            })
            .collect();
        self.commit(afters)
    }

    /// Re-applies one side of a recorded change as new revisions.
    fn apply_side(&mut self, change: &Change, use_before: bool) {
        for c in change.items.iter().rev() {
            let target = if use_before {
                // Undoing an add turns the item into a tombstone.
                c.before.clone().unwrap_or_else(|| BoardItem { deleted: true, ..c.after.clone() })
            } else {
                c.after.clone()
            };
            let stamped = self.stamp(target);
            self.board.items.insert(stamped.id, stamped);
        }
        self.version += 1;
    }

    pub fn undo(&mut self) -> bool {
        let Some(change) = self.undo.pop() else { return false };
        self.apply_side(&change, true);
        self.redo.push(change);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(change) = self.redo.pop() else { return false };
        self.apply_side(&change, false);
        self.undo.push(change);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(x: f64) -> (Frame, ItemKind) {
        (Frame { x, y: 0.0, w: 100.0, h: 50.0, ..Default::default() }, ItemKind::Note { text: "hi".into(), color: None })
    }

    fn doc() -> Document {
        Document::new(Board::new("b", 0), "dev-a")
    }

    #[test]
    fn add_stacks_on_top_and_stamps() {
        let mut d = doc();
        let ids = d.add(vec![note(0.0), note(10.0)]);
        let a = d.item(&ids[0]).unwrap();
        let b = d.item(&ids[1]).unwrap();
        assert!(b.frame.z > a.frame.z);
        assert_eq!((a.rev, a.updated_by.as_str()), (1, "dev-a"));
    }

    #[test]
    fn move_undo_redo_round_trip() {
        let mut d = doc();
        let ids = d.add(vec![note(0.0)]);
        let set: BTreeSet<_> = ids.iter().copied().collect();
        d.translate(&set, 30.0, 5.0);
        assert_eq!(d.item(&ids[0]).unwrap().frame.x, 30.0);
        assert!(d.undo());
        assert_eq!(d.item(&ids[0]).unwrap().frame.x, 0.0);
        assert!(d.redo());
        assert_eq!(d.item(&ids[0]).unwrap().frame.x, 30.0);
        assert!(d.undo() && d.undo(), "move then add");
        assert!(d.item(&ids[0]).is_none(), "undoing the add hides the item");
        assert!(!d.undo());
        // Every step bumped rev, so the history syncs as normal edits.
        assert!(d.board.items[&ids[0]].rev >= 5);
    }

    #[test]
    fn new_change_clears_redo() {
        let mut d = doc();
        let ids = d.add(vec![note(0.0)]);
        let set: BTreeSet<_> = ids.iter().copied().collect();
        d.translate(&set, 1.0, 0.0);
        d.undo();
        assert!(d.can_redo());
        d.translate(&set, 2.0, 0.0);
        assert!(!d.can_redo());
    }

    #[test]
    fn delete_is_a_tombstone_and_undoable() {
        let mut d = doc();
        let ids = d.add(vec![note(0.0), note(1.0)]);
        let set: BTreeSet<_> = [ids[0]].into();
        d.delete(&set);
        assert!(d.item(&ids[0]).is_none());
        assert!(d.board.items[&ids[0]].deleted);
        assert_eq!(d.board.live_items().count(), 1);
        d.undo();
        assert!(d.item(&ids[0]).is_some());
    }

    #[test]
    fn drag_preview_then_commit_is_one_undo_step() {
        let mut d = doc();
        let ids = d.add(vec![note(0.0)]);
        let originals = vec![d.item(&ids[0]).unwrap().clone()];
        d.preview_offset(&originals, 5.0, 0.0);
        d.preview_offset(&originals, 40.0, 10.0);
        assert_eq!(d.item(&ids[0]).unwrap().frame.x, 40.0);
        d.commit_drag(&originals, 40.0, 10.0);
        assert_eq!(d.item(&ids[0]).unwrap().frame.y, 10.0);
        d.undo();
        assert_eq!(d.item(&ids[0]).unwrap().frame.x, 0.0);
    }
}
