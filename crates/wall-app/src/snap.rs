//! Snapping while moving items: the dragged selection's left, centre and
//! right edges (and top, middle, bottom) snap to the same lines of nearby
//! items, and a guide line shows what it lined up with.

/// Snap targets for one drag, gathered when it starts (nothing else moves
/// during a drag).
#[derive(Debug, Clone)]
pub struct Snap {
    /// Bounds of the dragged items before the drag: x, y, w, h.
    pub bounds: [f64; 4],
    /// Bounds of the items it can snap to.
    pub targets: Vec<[f64; 4]>,
}

/// A guide: a line segment on the board, x0, y0, x1, y1.
pub type Guide = [f64; 4];

fn lines(lo: f64, size: f64) -> [f64; 3] {
    [lo, lo + size / 2.0, lo + size]
}

/// Best correction along one axis: the smallest move (within `threshold`)
/// that puts one of `moving`'s lines on one of a target's lines.
/// Returns the correction and the line it snapped to.
fn best(moving: [f64; 3], targets: impl Iterator<Item = [f64; 3]>, threshold: f64) -> Option<(f64, f64)> {
    let mut found: Option<(f64, f64)> = None;
    for t in targets {
        for m in moving {
            for line in t {
                let d = line - m;
                if d.abs() <= threshold && found.is_none_or(|(b, _)| d.abs() < b.abs()) {
                    found = Some((d, line));
                }
            }
        }
    }
    found
}

impl Snap {
    /// Adjusts a drag `delta` so the moved bounds line up with a target when
    /// within `threshold` (board units), and returns the guides to draw.
    pub fn apply(&self, delta: [f64; 2], threshold: f64) -> ([f64; 2], Vec<Guide>) {
        let b = self.bounds;
        let (x, y) = (b[0] + delta[0], b[1] + delta[1]);
        let sx = best(lines(x, b[2]), self.targets.iter().map(|t| lines(t[0], t[2])), threshold);
        let sy = best(lines(y, b[3]), self.targets.iter().map(|t| lines(t[1], t[3])), threshold);
        let d = [delta[0] + sx.map_or(0.0, |s| s.0), delta[1] + sy.map_or(0.0, |s| s.0)];

        // Guides span the moved bounds and every target touching the line.
        let moved = [b[0] + d[0], b[1] + d[1], b[2], b[3]];
        let mut guides = Vec::new();
        let eps = 1e-6;
        if let Some((_, line)) = sx {
            let hits = self.targets.iter().filter(|t| lines(t[0], t[2]).iter().any(|l| (l - line).abs() < eps));
            let (lo, hi) = hits.fold((moved[1], moved[1] + moved[3]), |(lo, hi), t| (lo.min(t[1]), hi.max(t[1] + t[3])));
            guides.push([line, lo, line, hi]);
        }
        if let Some((_, line)) = sy {
            let hits = self.targets.iter().filter(|t| lines(t[1], t[3]).iter().any(|l| (l - line).abs() < eps));
            let (lo, hi) = hits.fold((moved[0], moved[0] + moved[2]), |(lo, hi), t| (lo.min(t[0]), hi.max(t[0] + t[2])));
            guides.push([lo, line, hi, line]);
        }
        (d, guides)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snap {
        // Dragged 100x50 box at the origin; a target 200 to the right, 300 down.
        Snap { bounds: [0.0, 0.0, 100.0, 50.0], targets: vec![[200.0, 300.0, 80.0, 40.0]] }
    }

    #[test]
    fn left_edge_snaps_to_target_left_edge() {
        let (d, guides) = snap().apply([196.0, 0.0], 6.0);
        assert_eq!(d, [200.0, 0.0]);
        assert_eq!(guides, vec![[200.0, 0.0, 200.0, 340.0]]);
    }

    #[test]
    fn centres_snap_and_both_axes_at_once() {
        // Centre x: 50 + dx vs 240 -> dx 190; bottom 50 + dy vs top 300 -> dy 250.
        let (d, guides) = snap().apply([188.0, 253.0], 6.0);
        assert_eq!(d, [190.0, 250.0]);
        assert_eq!(guides.len(), 2);
    }

    #[test]
    fn nothing_within_threshold_leaves_delta_alone() {
        // x lines 130/180/230 vs 200/240/280: closest is 10 away.
        let (d, guides) = snap().apply([130.0, 120.0], 6.0);
        assert_eq!(d, [130.0, 120.0]);
        assert!(guides.is_empty());
    }

    #[test]
    fn nearest_line_wins() {
        let s = Snap { bounds: [0.0, 0.0, 100.0, 50.0], targets: vec![[203.0, 0.0, 10.0, 10.0], [201.0, 500.0, 10.0, 10.0]] };
        let (d, _) = s.apply([200.0, 1000.0], 6.0);
        assert_eq!(d[0], 201.0);
    }
}
