//! Several pictures imported together (one drop, one pick in the file
//! dialog): one undo step, laid out in rows instead of on top of each other.

use uuid::Uuid;

use crate::App;

impl App {
    /// Lays `ids` out in rows centred on `centre` (or, the first time, on
    /// where the first one landed), selects them all, and makes everything
    /// since undo depth `undo_from` one undo step. Returns the centre used,
    /// for the next call with the same batch (files can arrive one by one).
    pub fn arrange_imported(&mut self, ids: &[Uuid], undo_from: usize, centre: Option<(f64, f64)>) -> Option<(f64, f64)> {
        let mut centre = centre;
        if ids.len() > 1 {
            let sizes: Vec<_> = ids.iter().filter_map(|id| self.doc.item(id)).map(|i| (i.frame.w, i.frame.h)).collect();
            if sizes.len() == ids.len() {
                let c = *centre.get_or_insert_with(|| self.doc.item(&ids[0]).map(|i| (i.frame.x + i.frame.w / 2.0, i.frame.y + i.frame.h / 2.0)).unwrap_or_default());
                let unplaced = grid_layout(&sizes, (0.0, 0.0));
                let (w, h) = unplaced.iter().fold((0.0f64, 0.0f64), |(w, h), r| (w.max(r[0] + r[2]), h.max(r[1] + r[3])));
                for (id, r) in ids.iter().zip(grid_layout(&sizes, (c.0 - w / 2.0, c.1 - h / 2.0))) {
                    self.set_item_frame(*id, r);
                }
            }
            self.selection = ids.iter().copied().collect();
        }
        self.doc.join_undo_since(undo_from);
        self.needs_redraw = true;
        centre
    }
}

/// Rows of about square-root-of-n items, each row as tall as its tallest,
/// 24 board units apart, from `origin` (top-left).
pub fn grid_layout(sizes: &[(f64, f64)], origin: (f64, f64)) -> Vec<[f64; 4]> {
    const GAP: f64 = 24.0;
    let per_row = (sizes.len() as f64).sqrt().ceil().max(1.0) as usize;
    let mut out = Vec::with_capacity(sizes.len());
    let mut y = origin.1;
    for row in sizes.chunks(per_row) {
        let mut x = origin.0;
        let mut tallest: f64 = 0.0;
        for &(w, h) in row {
            out.push([x, y, w, h]);
            x += w + GAP;
            tallest = tallest.max(h);
        }
        y += tallest + GAP;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Board;

    #[test]
    fn dropped_pictures_go_in_rows_without_overlapping() {
        let sizes = [(400.0, 300.0), (200.0, 500.0), (300.0, 300.0), (100.0, 100.0), (640.0, 360.0)];
        let r = grid_layout(&sizes, (10.0, 20.0));
        assert_eq!(r[0], [10.0, 20.0, 400.0, 300.0]);
        assert_eq!(r[1][0], 10.0 + 400.0 + 24.0);
        // Three to a row; the second row starts under the tallest of the first.
        assert_eq!(r[3][..2], [10.0, 20.0 + 500.0 + 24.0]);
        for (i, a) in r.iter().enumerate() {
            for b in &r[i + 1..] {
                let apart = a[0] + a[2] <= b[0] || b[0] + b[2] <= a[0] || a[1] + a[3] <= b[1] || b[1] + b[3] <= a[1];
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn a_batch_is_one_undo_step_and_stays_centred() {
        let mut a = App::new(Board::new("b", 0), "t");
        a.resize([800.0, 600.0]);
        let from = a.doc.undo_depth();
        let ids: Vec<_> = (0..4).map(|k| a.place_picture(vec![k as u8; 4 * 40 * 30], 40, 30)).collect();
        let c = a.arrange_imported(&ids[..2], from, None);
        let c2 = a.arrange_imported(&ids, from, c);
        assert_eq!(c, c2);
        assert_eq!(a.selection.len(), 4);
        assert_eq!(a.doc.undo_depth(), from + 1);
        a.doc.undo();
        assert_eq!(a.item_count(), 0);
    }
}
