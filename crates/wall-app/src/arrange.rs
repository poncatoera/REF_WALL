//! Tidying the selection into a neat layout: rows in reading order, rows of
//! one height (pictures scaled to fit), tight packing (largest first), or
//! rows sorted by colour. Pictures keep their shape. One undo step, and it
//! syncs like any other edit.

use std::collections::{BTreeSet, HashMap};

use uuid::Uuid;
use wall_format::{BoardItem, Frame, ItemKind};

use crate::{doc, drawing, App};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrange {
    /// Rows in the order they read now (top to bottom, left to right);
    /// sizes kept.
    Rows,
    /// Rows of one height; pictures and videos are scaled so each row is
    /// the same width.
    SameHeight,
    /// As little empty space as possible, biggest first; sizes kept.
    Pack,
    /// Rows of one height, sorted by each picture's main colour.
    Colour,
}

/// One thing being placed: its size, and whether it may be scaled.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Block {
    pub w: f64,
    pub h: f64,
    pub scalable: bool,
}

/// Where a block goes, relative to the layout's top-left, and its scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Place {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

/// Layouts aim for this width-to-height shape (a wide screen).
const SHAPE: f64 = 1.6;

/// Space between neighbours: a little of a typical block's height.
pub(crate) fn gap_for(blocks: &[Block]) -> f64 {
    (median(blocks.iter().map(|b| b.h)) * 0.05).clamp(8.0, 200.0)
}

fn median(v: impl Iterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = v.collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// The row width that makes blocks of this total area come out about
/// `SHAPE` wide, never narrower than the widest block.
fn target_width(blocks: &[Block], gap: f64) -> f64 {
    let area: f64 = blocks.iter().map(|b| (b.w + gap) * (b.h + gap)).sum();
    let widest = blocks.iter().map(|b| b.w).fold(0.0, f64::max);
    (area * SHAPE).sqrt().max(widest)
}

/// Left to right, wrapping at `width`; each row as tall as its tallest block.
pub(crate) fn rows(blocks: &[Block], gap: f64, width: f64) -> Vec<Place> {
    let (mut x, mut y, mut row_h) = (0.0, 0.0, 0.0f64);
    blocks
        .iter()
        .map(|b| {
            if x > 0.0 && x + b.w > width + 0.5 {
                (x, y, row_h) = (0.0, y + row_h + gap, 0.0);
            }
            let p = Place { x, y, scale: 1.0 };
            x += b.w + gap;
            row_h = row_h.max(b.h);
            p
        })
        .collect()
}

/// Rows of one height that fill `width` exactly (the last row may fall
/// short). Only scalable blocks change size; others sit in the row as they are.
pub(crate) fn same_height(blocks: &[Block], gap: f64, width: f64) -> Vec<Place> {
    let ideal = median(blocks.iter().filter(|b| b.scalable).map(|b| b.h));
    if ideal <= 0.0 {
        return rows(blocks, gap, width);
    }
    // Height that makes blocks[a..b] exactly `width` wide.
    let fit = |a: usize, b: usize| -> f64 {
        let row = &blocks[a..b];
        let fixed: f64 = row.iter().filter(|b| !b.scalable).map(|b| b.w).sum::<f64>() + gap * (row.len() - 1) as f64;
        let aspect: f64 = row.iter().filter(|b| b.scalable).map(|b| b.w / b.h.max(1e-6)).sum();
        if aspect <= 0.0 {
            return ideal;
        }
        ((width - fixed) / aspect).max(ideal * 0.25)
    };
    let mut places = Vec::with_capacity(blocks.len());
    let (mut start, mut y) = (0, 0.0);
    while start < blocks.len() {
        // Add blocks until the row would have to shrink below the ideal
        // height; keep the last one if that lands closer to it.
        let mut end = start + 1;
        while end < blocks.len() && fit(start, end + 1) >= ideal {
            end += 1;
        }
        if end < blocks.len() && (fit(start, end + 1) - ideal).abs() < (fit(start, end) - ideal).abs() {
            end += 1;
        }
        let last = end == blocks.len();
        let mut h = fit(start, end);
        if last {
            h = h.min(ideal);
        }
        let mut x = 0.0;
        let mut row_h: f64 = 0.0;
        for b in &blocks[start..end] {
            let scale = if b.scalable { h / b.h.max(1e-6) } else { 1.0 };
            places.push(Place { x, y, scale });
            x += b.w * scale + gap;
            row_h = row_h.max(b.h * scale);
        }
        y += row_h + gap;
        start = end;
    }
    places
}

/// Skyline packing at `width`: each block goes where it sits highest
/// (then furthest left). Callers sort blocks biggest first.
pub(crate) fn pack(blocks: &[Block], gap: f64, width: f64) -> Vec<Place> {
    // The skyline: (x, top y, width) segments covering 0..width.
    let mut sky: Vec<(f64, f64, f64)> = vec![(0.0, 0.0, f64::INFINITY)];
    blocks
        .iter()
        .map(|b| {
            let (w, h) = (b.w + gap, b.h + gap);
            let mut best: Option<(f64, f64)> = None;
            for i in 0..sky.len() {
                let x = sky[i].0;
                if x > 0.0 && x + b.w > width + 0.5 {
                    break;
                }
                let mut y: f64 = 0.0;
                for s in &sky[i..] {
                    if s.0 >= x + w - 1e-9 {
                        break;
                    }
                    y = y.max(s.1);
                }
                if best.is_none_or(|(bx, by)| y < by - 1e-9 || (y <= by + 1e-9 && x < bx)) {
                    best = Some((x, y));
                }
            }
            let (x, y) = best.unwrap_or((0.0, sky.iter().map(|s| s.1).fold(0.0, f64::max)));
            // Raise the skyline over x..x+w to y+h.
            let (x1, top) = (x + w, y + h);
            let mut next = Vec::with_capacity(sky.len() + 2);
            for &(sx, sy, sw) in &sky {
                let sx1 = sx + sw;
                if sx1 <= x || sx >= x1 {
                    next.push((sx, sy, sw));
                    continue;
                }
                if sx < x {
                    next.push((sx, sy, x - sx));
                }
                if sx1 > x1 {
                    next.push((x1, sy, sx1 - x1));
                }
            }
            next.push((x, top, w));
            next.sort_by(|a, b| a.0.total_cmp(&b.0));
            // Merge neighbours at the same height.
            sky.clear();
            for s in next {
                match sky.last_mut() {
                    Some(l) if (l.1 - s.1).abs() < 1e-9 && (l.0 + l.2 - s.0).abs() < 1e-9 => l.2 += s.2,
                    _ => sky.push(s),
                }
            }
            Place { x, y, scale: 1.0 }
        })
        .collect()
}

/// Bounding size of a layout.
fn extent(blocks: &[Block], places: &[Place]) -> (f64, f64) {
    blocks.iter().zip(places).fold((0.0, 0.0), |(w, h), (b, p)| (w.max(p.x + b.w * p.scale), h.max(p.y + b.h * p.scale)))
}

/// Packs at a few widths and keeps the one with the least wasted area,
/// preferring a wide-screen shape.
pub(crate) fn pack_best(blocks: &[Block], gap: f64) -> Vec<Place> {
    let base = target_width(blocks, gap);
    let widest = blocks.iter().map(|b| b.w).fold(0.0, f64::max);
    let mut best: Option<(f64, Vec<Place>)> = None;
    for k in [0.7, 0.8, 0.9, 1.0, 1.1, 1.25, 1.4] {
        let width = (base * k).max(widest);
        let places = pack(blocks, gap, width);
        let (w, h) = extent(blocks, &places);
        let shape = (w / h.max(1e-6)) / SHAPE;
        let cost = w * h * (1.0 + 0.3 * shape.ln().abs());
        if best.as_ref().is_none_or(|(c, _)| cost < *c) {
            best = Some((cost, places));
        }
    }
    best.map(|(_, p)| p).unwrap_or_default()
}

/// Lays out `blocks` (already in the wanted order).
pub(crate) fn layout(how: Arrange, blocks: &[Block]) -> Vec<Place> {
    let gap = gap_for(blocks);
    match how {
        Arrange::Rows => rows(blocks, gap, target_width(blocks, gap)),
        Arrange::SameHeight | Arrange::Colour => {
            // Width from the blocks as they'll be: scaled to one height.
            let ideal = median(blocks.iter().filter(|b| b.scalable).map(|b| b.h));
            let sized: Vec<Block> = blocks
                .iter()
                .map(|b| if b.scalable && ideal > 0.0 { Block { w: b.w * ideal / b.h.max(1e-6), h: ideal, ..*b } } else { *b })
                .collect();
            same_height(blocks, gap, target_width(&sized, gap))
        }
        Arrange::Pack if blocks.len() > 400 => rows(blocks, gap, target_width(blocks, gap)),
        Arrange::Pack => pack_best(blocks, gap),
    }
}

/// Things that are tidied as blocks. Lines, arrows, comments and stamps
/// stay where they are (lines drawn on a picture go with it).
fn is_block(kind: &ItemKind) -> bool {
    !matches!(kind, ItemKind::Stroke { .. } | ItemKind::Arrow { .. } | ItemKind::Comment { .. } | ItemKind::Stamp { .. })
}

/// Hue (0–1, greys after colours by lightness) for sorting by colour.
fn colour_key(hex: Option<&str>) -> (u8, f64) {
    let Some([r, g, b, _]) = hex.and_then(crate::panel::parse_hex) else { return (2, 0.0) };
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max - min < 0.08 {
        return (1, 1.0 - l as f64);
    }
    let d = max - min;
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (0, h as f64 / 6.0)
}

impl App {
    /// Tidies the selected items into a neat layout, starting at the
    /// selection's top-left corner. One undo step.
    pub fn arrange(&mut self, how: Arrange) -> bool {
        let selected: Vec<BoardItem> = self.selection.iter().filter_map(|id| self.doc.item(id).cloned()).collect();
        // Groups carry what's inside them; those items aren't blocks themselves.
        let mut inside: HashMap<Uuid, Vec<BoardItem>> = HashMap::new();
        let mut in_a_group: BTreeSet<Uuid> = BTreeSet::new();
        for g in selected.iter().filter(|i| matches!(i.kind, ItemKind::Group { .. })) {
            let f = g.frame;
            let ids = self.items_in([f.x as f32, f.y as f32, (f.x + f.w) as f32, (f.y + f.h) as f32]);
            let items: Vec<BoardItem> = ids.iter().filter(|id| **id != g.id).filter_map(|id| self.doc.item(id).cloned()).collect();
            in_a_group.extend(items.iter().map(|i| i.id));
            inside.insert(g.id, items);
        }
        let mut units: Vec<BoardItem> = selected.into_iter().filter(|i| is_block(&i.kind) && !in_a_group.contains(&i.id)).collect();
        if units.len() < 2 {
            return false;
        }
        let x0 = units.iter().map(|i| i.frame.x).fold(f64::MAX, f64::min);
        let y0 = units.iter().map(|i| i.frame.y).fold(f64::MAX, f64::min);
        self.order_units(&mut units, how);
        let blocks: Vec<Block> = units
            .iter()
            .map(|i| Block { w: i.frame.w.max(1.0), h: i.frame.h.max(1.0), scalable: matches!(i.kind, ItemKind::Media { .. }) })
            .collect();
        let places = layout(how, &blocks);

        let mut afters: Vec<BoardItem> = Vec::new();
        for (unit, p) in units.iter().zip(&places) {
            let from = unit.frame;
            let to = Frame { x: x0 + p.x, y: y0 + p.y, w: from.w * p.scale, h: from.h * p.scale, ..from };
            if from == to {
                continue;
            }
            let (dx, dy) = (to.x - from.x, to.y - from.y);
            let mut moved = unit.clone();
            moved.frame = to;
            afters.push(moved);
            // Lines drawn on it, and everything in a group (with their lines).
            let carried = inside.get(&unit.id).cloned().unwrap_or_default();
            for c in &carried {
                let mut c = c.clone();
                doc::shift(&mut c, dx, dy);
                afters.push(c);
            }
            let mut owners: BTreeSet<Uuid> = carried.iter().map(|c| c.id).collect();
            owners.insert(unit.id);
            for line in self.lines_on(&owners) {
                if afters.iter().any(|a| a.id == line.id) {
                    // A line that's itself in a group: moved above, but it
                    // should follow its picture instead.
                    afters.retain(|a| a.id != line.id);
                }
                let owner = match &line.kind {
                    ItemKind::Stroke { on: Some(o), .. } => *o,
                    _ => continue,
                };
                let after = if owner == unit.id {
                    drawing::follow_frame(&line, from, to)
                } else {
                    let mut l = line.clone();
                    doc::shift(&mut l, dx, dy);
                    l
                };
                afters.push(after);
            }
        }
        // An item in two selected groups moves once.
        let mut seen = BTreeSet::new();
        afters.retain(|a| seen.insert(a.id));
        self.needs_redraw = true;
        self.doc.replace(afters)
    }

    /// Puts blocks in the order the layout should place them.
    fn order_units(&self, units: &mut [BoardItem], how: Arrange) {
        match how {
            Arrange::Rows | Arrange::SameHeight => reading_order(units),
            Arrange::Pack => units.sort_by(|a, b| b.frame.h.total_cmp(&a.frame.h).then(b.frame.w.total_cmp(&a.frame.w))),
            Arrange::Colour => {
                reading_order(units);
                let key = |i: &BoardItem| match &i.kind {
                    ItemKind::Media { hash, .. } => colour_key(self.sidecars.get(hash).and_then(|s| s.colors.first()).map(String::as_str)),
                    ItemKind::Note { color, .. } | ItemKind::Group { color, .. } => colour_key(color.as_deref()),
                    _ => (2, 0.0),
                };
                units.sort_by(|a, b| {
                    let (ka, kb) = (key(a), key(b));
                    ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
                });
            }
        }
    }
}

/// Top to bottom in bands, left to right within a band: the order a person
/// reads the items in now, so tidying keeps things roughly where they were.
fn reading_order(units: &mut [BoardItem]) {
    let band = median(units.iter().map(|i| i.frame.h)) * 0.5;
    units.sort_by(|a, b| (a.frame.y + a.frame.h / 2.0).total_cmp(&(b.frame.y + b.frame.h / 2.0)));
    let mut start = 0;
    while start < units.len() {
        let top = units[start].frame.y + units[start].frame.h / 2.0;
        let mut end = start + 1;
        while end < units.len() && units[end].frame.y + units[end].frame.h / 2.0 - top < band {
            end += 1;
        }
        units[start..end].sort_by(|a, b| a.frame.x.total_cmp(&b.frame.x));
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(w: f64, h: f64) -> Block {
        Block { w, h, scalable: true }
    }

    fn overlaps(blocks: &[Block], places: &[Place], gap: f64) -> bool {
        for i in 0..blocks.len() {
            for j in i + 1..blocks.len() {
                let (a, p) = (blocks[i], places[i]);
                let (c, q) = (blocks[j], places[j]);
                let sep = p.x + a.w * p.scale + gap <= q.x + 1e-6
                    || q.x + c.w * q.scale + gap <= p.x + 1e-6
                    || p.y + a.h * p.scale + gap <= q.y + 1e-6
                    || q.y + c.h * q.scale + gap <= p.y + 1e-6;
                if !sep {
                    return true;
                }
            }
        }
        false
    }

    fn mixed() -> Vec<Block> {
        (0..30).map(|k| b(100.0 + (k * 37 % 200) as f64, 80.0 + (k * 53 % 240) as f64)).collect()
    }

    #[test]
    fn every_layout_keeps_gaps_and_never_overlaps() {
        let blocks = mixed();
        let gap = gap_for(&blocks);
        for how in [Arrange::Rows, Arrange::SameHeight, Arrange::Pack, Arrange::Colour] {
            let places = layout(how, &blocks);
            assert_eq!(places.len(), blocks.len());
            assert!(!overlaps(&blocks, &places, gap - 1e-6), "{how:?}");
            assert!(places.iter().all(|p| p.x >= 0.0 && p.y >= 0.0));
        }
    }

    #[test]
    fn same_height_rows_are_one_height_and_one_width() {
        let blocks = mixed();
        let gap = gap_for(&blocks);
        let places = layout(Arrange::SameHeight, &blocks);
        let mut by_row: Vec<(f64, f64, f64)> = Vec::new(); // y, height, right edge
        for (bl, p) in blocks.iter().zip(&places) {
            let (h, r) = (bl.h * p.scale, p.x + bl.w * p.scale);
            match by_row.iter_mut().find(|row| (row.0 - p.y).abs() < 1e-6) {
                Some(row) => {
                    assert!((row.1 - h).abs() < 1e-6, "one height per row");
                    row.2 = row.2.max(r);
                }
                None => by_row.push((p.y, h, r)),
            }
        }
        assert!(by_row.len() > 2);
        let w = by_row[0].2;
        for row in &by_row[..by_row.len() - 1] {
            assert!((row.2 - w).abs() < 1e-3, "full rows are the same width: {} vs {w}", row.2);
        }
        assert!(by_row.last().unwrap().2 <= w + 1e-3);
        assert!(!overlaps(&blocks, &places, gap - 1e-6));
    }

    #[test]
    fn packing_is_tighter_than_rows() {
        let mut blocks = mixed();
        blocks.sort_by(|a, b| b.h.total_cmp(&a.h));
        let area = |p: &[Place]| {
            let (w, h) = extent(&blocks, p);
            w * h
        };
        let gap = gap_for(&blocks);
        assert!(area(&pack_best(&blocks, gap)) < area(&rows(&blocks, gap, target_width(&blocks, gap))));
    }

    #[test]
    fn layouts_come_out_wide() {
        let blocks: Vec<Block> = (0..20).map(|_| b(100.0, 100.0)).collect();
        for how in [Arrange::Rows, Arrange::SameHeight, Arrange::Pack] {
            let (w, h) = extent(&blocks, &layout(how, &blocks));
            assert!(w > h && w < h * 3.0, "{how:?}: {w} x {h}");
        }
    }

    #[test]
    fn colours_sort_by_hue_then_greys() {
        assert!(colour_key(Some("#ff0000")) < colour_key(Some("#00ff00")));
        assert!(colour_key(Some("#00ff00")) < colour_key(Some("#0000ff")));
        assert!(colour_key(Some("#0000ff")) < colour_key(Some("#ffffff")));
        assert!(colour_key(Some("#808080")) < colour_key(None));
    }

    fn media(a: &mut App, x: f64, y: f64, w: f64, h: f64) -> Uuid {
        let f = Frame { x, y, w, h, ..Default::default() };
        let hash = wall_format::ContentHash::of(format!("{x}{y}").as_bytes());
        a.doc.add(vec![(f, ItemKind::Media { hash, crop: None, look: Default::default() })])[0]
    }

    #[test]
    fn tidying_is_one_undo_step_and_lines_and_group_contents_follow() {
        let mut a = App::new(wall_format::Board::new("t", 0), "test");
        let p1 = media(&mut a, 500.0, 40.0, 200.0, 100.0);
        let p2 = media(&mut a, 0.0, 400.0, 100.0, 300.0);
        let g = a.doc.add(vec![(Frame { x: 900.0, y: 900.0, w: 300.0, h: 300.0, ..Default::default() }, ItemKind::Group { name: "g".into(), color: None })])[0];
        let inner = media(&mut a, 950.0, 950.0, 50.0, 50.0);
        let line = a
            .doc
            .add(vec![(
                Frame { x: 520.0, y: 60.0, w: 20.0, h: 20.0, ..Default::default() },
                ItemKind::Stroke { points: vec![[0.0, 0.0], [1.0, 1.0]], width: 2.0, color: None, brush: None, widths: vec![], on: Some(p1), drawing: None },
            )])[0];
        let before = a.doc.board.clone();
        a.selection = [p1, p2, g].into();
        assert!(a.arrange(Arrange::SameHeight));
        let f = |id| a.doc.item(&id).unwrap().frame;
        // Starts at the selection's top-left; pictures keep their shape.
        assert_eq!((f(p1).y.min(f(p2).y).min(f(g).y), f(p1).x.min(f(p2).x).min(f(g).x)), (40.0, 0.0));
        assert!((f(p1).w / f(p1).h - 2.0).abs() < 1e-9 && (f(p2).w / f(p2).h - 1.0 / 3.0).abs() < 1e-9);
        // The line keeps its place on the picture; the group keeps its size and contents.
        let (l, p) = (f(line), f(p1));
        let s = p.w / 200.0;
        assert!((l.x - (p.x + 20.0 * s)).abs() < 1e-6 && (l.w - 20.0 * s).abs() < 1e-6);
        assert_eq!((f(g).w, f(g).h), (300.0, 300.0));
        assert_eq!((f(inner).x - f(g).x, f(inner).y - f(g).y), (50.0, 50.0));
        a.doc.undo();
        assert_eq!(a.doc.board.items.values().map(|i| i.frame).collect::<Vec<_>>(), before.items.values().map(|i| i.frame).collect::<Vec<_>>());
    }

    #[test]
    fn one_item_or_lines_alone_do_nothing() {
        let mut a = App::new(wall_format::Board::new("t", 0), "test");
        let p1 = media(&mut a, 0.0, 0.0, 10.0, 10.0);
        a.selection = [p1].into();
        assert!(!a.arrange(Arrange::Rows));
        assert!(!a.doc.can_undo() || a.doc.undo_depth() == 1);
    }
}
