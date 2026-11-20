//! Turns the board into GPU instances. Text layouts are cached per item and
//! stored relative to the item, so dragging only offsets them.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

use uuid::Uuid;
use wall_format::{ArrowEnd, ContentHash, ItemKind};
use wall_render::{BoxInstance, CurveInstance, GlyphInstance, ImageId, ImageInstance, Renderer, ViewUniform};
use wall_text::Font;

use crate::doc::Document;

const ACCENT: [f32; 4] = [0.36, 0.62, 1.0, 1.0];
const NOTE_FILL: [f32; 4] = [0.13, 0.135, 0.15, 1.0];
const NOTE_STROKE: [f32; 4] = [0.24, 0.25, 0.28, 1.0];
const TEXT: [f32; 4] = [0.92, 0.92, 0.94, 1.0];

struct CachedText {
    key: (String, u32, u32, u32),
    /// Rebuild that last used this entry; older ones are dropped.
    gen: u64,
    glyphs: Vec<GlyphInstance>,
}

/// Items drawn together: boxes, then pictures, then text. Items in one run
/// never overlap each other, so drawing a run layer by layer looks the same
/// as drawing its items one by one in z order, with three draw calls.
#[derive(Default)]
struct Run {
    boxes: Vec<BoxInstance>,
    images: Vec<(ContentHash, ImageInstance)>,
    curves: Vec<CurveInstance>,
    glyphs: Vec<GlyphInstance>,
    /// Spatial hash of the run's item rects, for the overlap test.
    cells: HashMap<(i32, i32), Vec<[f32; 4]>>,
}

const CELL: f32 = 512.0;

impl Run {
    fn cell_range(r: [f32; 4]) -> impl Iterator<Item = (i32, i32)> {
        let (x0, y0) = ((r[0] / CELL).floor() as i32, (r[1] / CELL).floor() as i32);
        let (x1, y1) = (((r[0] + r[2]) / CELL).floor() as i32, ((r[1] + r[3]) / CELL).floor() as i32);
        (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
    }

    fn overlaps(&self, r: [f32; 4]) -> bool {
        Self::cell_range(r).any(|c| {
            self.cells.get(&c).is_some_and(|rs| {
                rs.iter().any(|o| r[0] < o[0] + o[2] && o[0] < r[0] + r[2] && r[1] < o[1] + o[3] && o[1] < r[1] + r[3])
            })
        })
    }

    fn claim(&mut self, r: [f32; 4]) {
        for c in Self::cell_range(r) {
            self.cells.entry(c).or_default().push(r);
        }
    }
}

#[derive(Default)]
pub struct Scene {
    version: Option<u64>,
    /// Groups: always behind everything else.
    back: Run,
    /// Everything else, back to front.
    runs: Vec<Run>,
    text_cache: HashMap<Uuid, CachedText>,
    /// Counts rebuilds, to drop cache entries of deleted items.
    gen: u64,
    /// Instance counts of the last rebuild, to size buffers up front.
    last_counts: (usize, usize),
}

impl Scene {
    pub fn glyph_count(&self) -> usize {
        self.back.glyphs.len() + self.runs.iter().map(|r| r.glyphs.len()).sum::<usize>()
    }

    /// Draw-order batches.
    #[cfg(test)]
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// Rebuilds instances if the document changed since the last call.
    /// Forces a rebuild on the next update (another board was opened).
    pub fn invalidate(&mut self) {
        self.version = None;
        self.text_cache.clear();
    }

    pub fn update(&mut self, doc: &Document, font: &mut Font, board_names: &HashMap<Uuid, String>) {
        if self.version == Some(doc.version) {
            return;
        }
        self.version = Some(doc.version);
        self.back = Run::default();
        self.runs.clear();

        self.gen += 1;
        let gen = self.gen;
        let (last_boxes, last_glyphs) = self.last_counts;

        let mut items: Vec<_> = doc.board.live_items().collect();
        items.sort_by(|a, b| a.frame.z.total_cmp(&b.frame.z));
        for item in items {
            let f = item.frame;
            let mut rect = [f.x as f32, f.y as f32, f.w as f32, f.h as f32];
            let arrow = match &item.kind {
                ItemKind::Arrow { from, to } => match arrow_points(doc, from, to) {
                    Some(pts) => {
                        rect = curve_bounds(pts, ARROW_HEAD);
                        Some(pts)
                    }
                    None => continue, // an end's item is gone
                },
                _ => None,
            };
            let is_group = matches!(item.kind, ItemKind::Group { .. });
            if !is_group {
                // Start a new run if this item overlaps one already in the current run.
                if self.runs.last().is_none_or(|r| r.overlaps(rect)) {
                    let mut run = Run::default();
                    if self.runs.is_empty() {
                        // Most boards are one run: size it like last time so
                        // a drag frame doesn't regrow megabytes of instances.
                        run.boxes.reserve(last_boxes);
                        run.glyphs.reserve(last_glyphs);
                    }
                    self.runs.push(run);
                }
                self.runs.last_mut().unwrap().claim(rect);
            }
            let mut label = None;
            let run = if is_group { &mut self.back } else { self.runs.last_mut().unwrap() };
            match &item.kind {
                ItemKind::Group { name, color } => {
                    let c = parse_hex(color.as_deref()).unwrap_or([0.6, 0.6, 0.65]);
                    run.boxes.push(BoxInstance {
                        rect,
                        fill: [c[0] * 0.06, c[1] * 0.06, c[2] * 0.06, 0.6],
                        stroke: [c[0] * 0.5, c[1] * 0.5, c[2] * 0.5, 0.9],
                        shape: [10.0, 1.5],
                    });
                    label = Some((name.as_str(), [c[0], c[1], c[2], 1.0]));
                }
                ItemKind::Media { hash, crop } => {
                    // Placeholder under the picture, seen until its pixels (or a
                    // video's first frame) arrive.
                    run.boxes.push(BoxInstance { rect, fill: NOTE_FILL, stroke: NOTE_STROKE, shape: [6.0, 1.0] });
                    run.images.push((hash.clone(), ImageInstance {
                        rect,
                        params: [1.0, 6.0, 0.0, 0.0],
                        uv_rect: crop.map_or([0.0, 0.0, 1.0, 1.0], |c| [c.x as f32, c.y as f32, c.w as f32, c.h as f32]),
                    }));
                }
                ItemKind::Note { text, color } => {
                    let tint = parse_hex(color.as_deref());
                    let fill = tint.map(|c| [c[0] * 0.42, c[1] * 0.42, c[2] * 0.42, 1.0]).unwrap_or(NOTE_FILL);
                    let stroke = tint.map(|c| [c[0] * 0.8, c[1] * 0.8, c[2] * 0.8, 1.0]).unwrap_or(NOTE_STROKE);
                    run.boxes.push(BoxInstance { rect, fill, stroke, shape: [8.0, 1.0] });
                    label = Some((text.as_str(), TEXT));
                }
                ItemKind::Label { text } => {
                    label = Some((text.as_str(), [1.0; 4]));
                }
                ItemKind::Todo { title, .. } => {
                    run.boxes.push(BoxInstance { rect, fill: NOTE_FILL, stroke: NOTE_STROKE, shape: [8.0, 1.0] });
                    label = Some((title.as_str(), TEXT));
                }
                ItemKind::Board { board_id } => {
                    run.boxes.push(BoxInstance { rect, fill: NOTE_FILL, stroke: ACCENT, shape: [8.0, 1.5] });
                    // A small stack mark in the corner says "this opens".
                    let px = 10.0;
                    for k in 0..2 {
                        let o = 6.0 * k as f32;
                        run.boxes.push(BoxInstance {
                            rect: [rect[0] + rect[2] - 34.0 + o, rect[1] + rect[3] - 30.0 - o, 22.0, 16.0],
                            fill: NOTE_FILL,
                            stroke: [ACCENT[0] * 0.8, ACCENT[1] * 0.8, ACCENT[2] * 0.8, 1.0],
                            shape: [3.0, 1.0],
                        });
                        let _ = px;
                    }
                    let name = board_names.get(board_id).map_or("Board", |n| n.as_str());
                    label = Some((name, TEXT));
                }
                ItemKind::Arrow { .. } => {
                    if let Some(pts) = arrow {
                        run.curves.push(arrow_curve(pts, ARROW_COLOR, ARROW_WIDTH));
                    }
                }
            }
            if let Some((t, color)) = label {
                item_text(&mut self.text_cache, gen, font, &mut run.glyphs, item.id, t, &item.kind, color, rect);
            }
        }
        self.text_cache.retain(|_, e| e.gen == gen);
        self.last_counts = self.runs.first().map_or((0, 0), |r| (r.boxes.len(), r.glyphs.len()));
    }


    pub fn draw(&self, renderer: &mut Renderer, view: &ViewUniform, textures: &HashMap<&ContentHash, ImageId>) {
        for run in std::iter::once(&self.back).chain(&self.runs) {
            renderer.draw_boxes(view, &run.boxes);
            let images: Vec<(ImageId, ImageInstance)> =
                run.images.iter().filter_map(|(h, inst)| textures.get(h).map(|id| (*id, *inst))).collect();
            renderer.draw_images(view, &images);
            renderer.draw_curves(view, &run.curves);
            renderer.draw_glyphs(view, &run.glyphs);
        }
    }
}

/// Appends an item's text, from cache when text, style and width are unchanged.
#[allow(clippy::too_many_arguments)]
fn item_text(cache: &mut HashMap<Uuid, CachedText>, gen: u64, font: &mut Font, out: &mut Vec<GlyphInstance>, id: Uuid, text: &str, kind: &ItemKind, color: [f32; 4], rect: [f32; 4]) {
        let Some(style) = TextStyle::of(kind, rect[2]) else { return };
        // Mark the entry used before anything else, so retain keeps it.
        let dims = (style.size.to_bits(), rect[2].to_bits(), rect[3].to_bits());
        let entry = cache.entry(id).or_insert_with(|| CachedText { key: (String::new(), 0, 0, 0), gen, glyphs: Vec::new() });
        entry.gen = gen;
        // Compare before building a key: this runs for every item on every
        // rebuild (each drag frame), and copying the text would allocate.
        let same = entry.key.0 == text && (entry.key.1, entry.key.2, entry.key.3) == dims;
        if !same {
            let key = (text.to_owned(), dims.0, dims.1, dims.2);
            entry.glyphs.clear();
            let max_lines = style.max_lines(rect[3]);
            for (n, range) in style.lines(font, text, max_lines).into_iter().enumerate() {
                let line = text[range].trim_end();
                add_text(font, &mut entry.glyphs, line, [style.at[0], style.at[1] + n as f32 * style.line_height()], style.size, color);
            }
            entry.key = key;
        }
        out.extend(entry.glyphs.iter().map(|g| {
        let mut g = *g;
        g.rect[0] += rect[0];
        g.rect[1] += rect[1];
        g
    }));
}

pub const ARROW_COLOR: [f32; 4] = [0.62, 0.64, 0.7, 1.0];
pub const ARROW_WIDTH: f32 = 2.5;
pub const ARROW_HEAD: f32 = 14.0;
/// Space between an attached arrow end and its item.
const ARROW_GAP: f32 = 8.0;

/// Where an arrow end sits: a free point, or an item's centre.
fn end_anchor(doc: &Document, end: &ArrowEnd) -> Option<([f32; 2], Option<[f32; 4]>)> {
    match end {
        ArrowEnd::Point { x, y } => Some(([*x as f32, *y as f32], None)),
        ArrowEnd::Item { id } => {
            let f = doc.item(id)?.frame;
            let r = [f.x as f32, f.y as f32, f.w as f32, f.h as f32];
            Some(([r[0] + r[2] / 2.0, r[1] + r[3] / 2.0], Some(r)))
        }
    }
}

/// Where a ray from a rect's centre towards `toward` leaves the rect, plus a gap.
fn exit_point(r: [f32; 4], toward: [f32; 2]) -> [f32; 2] {
    let c = [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0];
    let d = [toward[0] - c[0], toward[1] - c[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-6);
    let sx = if d[0].abs() > 1e-6 { r[2] / 2.0 / d[0].abs() } else { f32::MAX };
    let sy = if d[1].abs() > 1e-6 { r[3] / 2.0 / d[1].abs() } else { f32::MAX };
    let s = sx.min(sy);
    [c[0] + d[0] * s + d[0] / len * ARROW_GAP, c[1] + d[1] * s + d[1] / len * ARROW_GAP]
}

/// An arrow's start and end on the board; attached ends stop at their
/// item's edge. None if an attached item no longer exists.
pub fn arrow_points(doc: &Document, from: &ArrowEnd, to: &ArrowEnd) -> Option<[[f32; 2]; 2]> {
    let (a, ra) = end_anchor(doc, from)?;
    let (b, rb) = end_anchor(doc, to)?;
    let a2 = ra.map_or(a, |r| exit_point(r, b));
    let b2 = rb.map_or(b, |r| exit_point(r, a));
    Some([a2, b2])
}

/// A straight arrow as a curve instance (control point in the middle).
pub fn arrow_curve(p: [[f32; 2]; 2], color: [f32; 4], width: f32) -> CurveInstance {
    let mid = [(p[0][0] + p[1][0]) / 2.0, (p[0][1] + p[1][1]) / 2.0];
    CurveInstance { p01: [p[0][0], p[0][1], mid[0], mid[1]], p2w: [p[1][0], p[1][1], width / 2.0, ARROW_HEAD], color }
}

fn curve_bounds(p: [[f32; 2]; 2], pad: f32) -> [f32; 4] {
    let (x0, y0) = (p[0][0].min(p[1][0]) - pad, p[0][1].min(p[1][1]) - pad);
    let (x1, y1) = (p[0][0].max(p[1][0]) + pad, p[0][1].max(p[1][1]) + pad);
    [x0, y0, x1 - x0, y1 - y0]
}

/// Distance from a point to a segment.
pub fn segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (pa, ba) = ([p[0] - a[0], p[1] - a[1]], [b[0] - a[0], b[1] - a[1]]);
    let h = ((pa[0] * ba[0] + pa[1] * ba[1]) / (ba[0] * ba[0] + ba[1] * ba[1]).max(1e-12)).clamp(0.0, 1.0);
    ((pa[0] - ba[0] * h).powi(2) + (pa[1] - ba[1] * h).powi(2)).sqrt()
}

/// Selection outlines and the marquee, sized in screen pixels.
pub fn overlays(doc: &Document, selection: &BTreeSet<Uuid>, marquee: Option<[f32; 4]>, zoom: f32) -> Vec<BoxInstance> {
    let px = 1.0 / zoom;
    let mut out: Vec<BoxInstance> = selection
        .iter()
        .filter_map(|id| doc.item(id))
        .filter(|i| !matches!(i.kind, ItemKind::Arrow { .. }))
        .map(|i| {
            let f = i.frame;
            BoxInstance {
                rect: [f.x as f32 - 3.0 * px, f.y as f32 - 3.0 * px, f.w as f32 + 6.0 * px, f.h as f32 + 6.0 * px],
                fill: [0.0; 4],
                stroke: ACCENT,
                shape: [6.0 * px, 2.0 * px],
            }
        })
        .collect();
    if let Some(m) = marquee {
        out.push(BoxInstance {
            rect: [m[0], m[1], m[2] - m[0], m[3] - m[1]],
            fill: [ACCENT[0] * 0.12, ACCENT[1] * 0.12, ACCENT[2] * 0.12, 0.12],
            stroke: [ACCENT[0] * 0.8, ACCENT[1] * 0.8, ACCENT[2] * 0.8, 0.8],
            shape: [0.0, px],
        });
    }
    out
}

/// Where and how an item's text is laid out, relative to the item's frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// First baseline, relative to the item's top-left.
    pub at: [f32; 2],
    /// Board units per em.
    pub size: f32,
    /// Wrap width in board units, or None for one line per paragraph.
    pub wrap: Option<f32>,
    /// Enter adds a line break (notes) instead of finishing the edit.
    pub multiline: bool,
}

impl TextStyle {
    pub fn of(kind: &ItemKind, width: f32) -> Option<Self> {
        Some(match kind {
            ItemKind::Note { .. } | ItemKind::Todo { .. } | ItemKind::Board { .. } => TextStyle { at: [14.0, 30.0], size: 16.0, wrap: Some(width - 28.0), multiline: true },
            ItemKind::Group { .. } => TextStyle { at: [12.0, 30.0], size: 20.0, wrap: None, multiline: false },
            ItemKind::Label { .. } => TextStyle { at: [0.0, 96.0], size: 96.0, wrap: None, multiline: false },
            _ => return None,
        })
    }

    pub fn line_height(&self) -> f32 {
        self.size * 1.35
    }

    /// Lines that fit in a frame of this height (at least one).
    pub fn max_lines(&self, height: f32) -> usize {
        ((height - self.at[1]) / self.line_height() + 1.0).floor().max(1.0) as usize
    }

    /// Byte ranges of the laid-out lines.
    pub fn lines(&self, font: &mut Font, text: &str, max_lines: usize) -> Vec<Range<usize>> {
        match self.wrap {
            Some(w) => wrap_ranges(font, text, w / self.size, max_lines),
            None => {
                let mut out = Vec::new();
                let mut start = 0;
                for para in text.split('\n') {
                    out.push(start..start + para.len());
                    start += para.len() + 1;
                }
                out.truncate(max_lines);
                out
            }
        }
    }

    /// Height a frame needs to show every line of `text`.
    pub fn height_for(&self, font: &mut Font, text: &str) -> f32 {
        let n = self.lines(font, text, usize::MAX).len().max(1);
        self.at[1] + (n - 1) as f32 * self.line_height() + self.size * 0.9
    }

    /// Caret box (x, y, w, h) relative to the item for a byte offset.
    pub fn caret(&self, font: &mut Font, text: &str, max_lines: usize, caret: usize) -> [f32; 4] {
        let lines = self.lines(font, text, max_lines);
        let n = lines.iter().rposition(|r| r.start <= caret).unwrap_or(0);
        let start = lines.get(n).map_or(0, |r| r.start);
        let end = caret.min(text.len()).max(start);
        let x = font.layout_line(&text[start..end]).width * self.size;
        let y = self.at[1] + n as f32 * self.line_height();
        [self.at[0] + x, y - self.size * 0.95, 0.0, self.size * 1.2]
    }

    /// Byte offset nearest to a point relative to the item.
    pub fn hit(&self, font: &mut Font, text: &str, max_lines: usize, p: [f32; 2]) -> usize {
        let lines = self.lines(font, text, max_lines);
        let Some(last) = lines.len().checked_sub(1) else { return 0 };
        let row = ((p[1] - self.at[1] + self.size * 0.95) / self.line_height()).floor().clamp(0.0, last as f32) as usize;
        let r = lines[row].clone();
        let mut best = (f32::MAX, r.start);
        let mut i = r.start;
        loop {
            let x = self.at[0] + font.layout_line(&text[r.start..i]).width * self.size;
            if (x - p[0]).abs() < best.0 {
                best = ((x - p[0]).abs(), i);
            }
            if i >= r.end {
                break;
            }
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
        best.1
    }
}

/// Greedy word wrap to `max_em` (width in em units); returns byte ranges of
/// lines. Line breaks in the text start new lines; an empty paragraph is an
/// empty line. A single word wider than the line gets a line of its own.
pub fn wrap_ranges(font: &mut Font, text: &str, max_em: f32, max_lines: usize) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut base = 0;
    for para in text.split('\n') {
        let mut line: Option<(usize, usize)> = None;
        let mut start = None;
        let mut words = Vec::new();
        for (i, ch) in para.char_indices() {
            match (ch.is_whitespace(), start) {
                (false, None) => start = Some(i),
                (true, Some(s)) => {
                    words.push((s, i));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            words.push((s, para.len()));
        }
        for (ws, we) in words {
            let (ws, we) = (base + ws, base + we);
            line = match line {
                None => Some((ws, we)),
                Some((ls, _)) if font.layout_line(&text[ls..we]).width <= max_em => Some((ls, we)),
                Some((ls, le)) => {
                    out.push(ls..le);
                    if out.len() >= max_lines {
                        return out;
                    }
                    Some((ws, we))
                }
            };
        }
        out.push(line.map_or(base..base, |(ls, le)| ls..le));
        if out.len() >= max_lines {
            return out;
        }
        base += para.len() + 1;
    }
    out
}

/// Greedy word wrap returning the line strings (see [`wrap_ranges`]).
#[cfg(test)]
pub fn wrap_text(font: &mut Font, text: &str, max_em: f32, max_lines: usize) -> Vec<String> {
    wrap_ranges(font, text, max_em, max_lines).into_iter().map(|r| text[r].to_owned()).collect()
}

/// Lays out `text` with its baseline starting at `baseline` (board units,
/// y down) at `size` board units per em, and appends one quad per glyph.
pub fn add_text(font: &mut Font, out: &mut Vec<GlyphInstance>, text: &str, baseline: [f32; 2], size: f32, rgba: [f32; 4]) {
    let line = font.layout_line(text);
    for g in &line.glyphs {
        let [x0, y0, x1, y1] = g.entry.bounds;
        out.push(GlyphInstance {
            rect: [
                baseline[0] + (g.origin[0] + x0) * size,
                baseline[1] - (g.origin[1] + y1) * size,
                (x1 - x0) * size,
                (y1 - y0) * size,
            ],
            em: g.entry.bounds,
            color: rgba,
            data: [g.entry.curve_base as f32, g.entry.band_base as f32, g.entry.bands_h as f32, g.entry.bands_v as f32],
        });
    }
}

fn parse_hex(s: Option<&str>) -> Option<[f32; 3]> {
    let s = s?.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0])
}

pub fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_quads_sit_on_the_baseline() {
        let mut font = Font::new(wall_text::INTER_REGULAR).unwrap();
        let mut out = Vec::new();
        add_text(&mut font, &mut out, "Hx", [100.0, 200.0], 50.0, [1.0; 4]);
        assert_eq!(out.len(), 2);
        let h = out[0].rect;
        assert!((h[1] + h[3] - 200.0).abs() < 0.5, "{h:?}");
        assert!(out[1].rect[0] > h[0] + h[2]);
    }

    #[test]
    fn wrap_breaks_on_words_and_caps_lines() {
        let mut font = Font::new(wall_text::INTER_REGULAR).unwrap();
        let lines = wrap_text(&mut font, "Kinetic type on a grid with long tail easing", 8.0, 10);
        assert!(lines.len() >= 2, "{lines:?}");
        assert!(lines.iter().all(|l| font.layout_line(l).width <= 8.0 || !l.contains(' ')));
        assert_eq!(lines.join(" "), "Kinetic type on a grid with long tail easing");
        assert_eq!(wrap_text(&mut font, "a b c d e f g h", 0.5, 3).len(), 3);
    }

    #[test]
    fn line_breaks_and_caret_positions() {
        let mut font = Font::new(wall_text::INTER_REGULAR).unwrap();
        let style = TextStyle::of(&ItemKind::Note { text: String::new(), color: None }, 240.0).unwrap();
        let text = "one\n\nthree";
        let lines = style.lines(&mut font, text, 10);
        assert_eq!(lines, vec![0..3, 4..4, 5..10]);
        let c0 = style.caret(&mut font, text, 10, 0);
        let c_end = style.caret(&mut font, text, 10, text.len());
        assert_eq!(c0[0], style.at[0]);
        assert!((c_end[1] - c0[1] - 2.0 * style.line_height()).abs() < 1e-3);
        // Clicking right of line 3 puts the caret at its end.
        let p = [500.0, style.at[1] + 2.0 * style.line_height() - 4.0];
        assert_eq!(style.hit(&mut font, text, 10, p), text.len());
        assert!(style.height_for(&mut font, text) > style.height_for(&mut font, "one"));
    }

    #[test]
    fn arrow_ends_stop_at_item_edges() {
        let r = [0.0, 0.0, 100.0, 50.0];
        let e = exit_point(r, [500.0, 25.0]);
        assert!((e[0] - (100.0 + ARROW_GAP)).abs() < 1e-3 && (e[1] - 25.0).abs() < 1e-3, "{e:?}");
        let e = exit_point(r, [50.0, -400.0]);
        assert!((e[1] + ARROW_GAP).abs() < 1e-3, "{e:?}");
        assert!((segment_distance([5.0, 3.0], [0.0, 0.0], [10.0, 0.0]) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn hex_colours_parse() {
        assert_eq!(parse_hex(Some("#ff0000")), Some([1.0, 0.0, 0.0]));
        assert_eq!(parse_hex(Some("red")), None);
    }
}
