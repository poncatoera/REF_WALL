//! Drawing on the board (D, or the rail's pen): the options bar at the top
//! (pen, pencil, brush, marker, eraser; colours; size), the size ring at
//! the pointer, the line being drawn, and rubbing out.

use uuid::Uuid;
use wall_format::{BoardItem, Frame, ItemKind};
use wall_render::{BoxInstance, CurveInstance, Renderer, ViewUniform};

use crate::brush::{self, Brush};
use crate::menu::BAR;
use crate::panel::{self, PanelBox, TextRun};
use crate::{scene, theme, toolbar, App, Gesture};

const H: f32 = 40.0;
const CHIP_H: f32 = 28.0;
const DOT: f32 = 20.0;

#[derive(Debug, Clone, PartialEq)]
pub enum BarHit {
    Brush(Brush),
    Color(usize),
    Smaller,
    Bigger,
    Done,
}

#[derive(Default)]
pub struct BarLayout {
    pub rect: [f32; 4],
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], BarHit)>,
}

impl BarLayout {
    pub fn hit_at(&self, p: [f32; 2]) -> Option<Option<BarHit>> {
        if !panel::inside(self.rect, p) {
            return None;
        }
        Some(self.hits.iter().find(|(r, _)| panel::inside(*r, p)).map(|(_, h)| h.clone()))
    }
}

/// The options bar, centred over the free part of the window.
/// `left` and `right` bound the free part (the left panel and the right
/// side panel); when one row doesn't fit there, the colours and size go on
/// a second row.
pub fn bar_layout(pen: &brush::PenState, hover: [f32; 2], viewport: [f32; 2], left: f32, right: f32, text_width: &mut dyn FnMut(&str, f32) -> f32) -> BarLayout {
    let t = theme::t();
    let mut l = BarLayout::default();
    let chip_w: Vec<f32> = brush::ALL.iter().map(|b| text_width(b.label(), 12.5) + 22.0).collect();
    let size_label = format!("{:.0} px", pen.size());
    let size_w = 26.0 + text_width("000 px", 12.5) + 26.0;
    let done_w = text_width("Done", 12.5) + 22.0;
    let gap = 6.0;
    let sep = 14.0;
    let row1 = chip_w.iter().sum::<f32>() + gap * 4.0;
    let row2 = 8.0 * (DOT + gap) - gap + sep + size_w + sep + done_w;
    let right = right.min(viewport[0]);
    let one_row = 8.0 + row1 + sep + row2 + 8.0;
    let two_rows = one_row > right - left - 16.0;
    let w = if two_rows { 8.0 + row1.max(row2) + 8.0 } else { one_row };
    let h = if two_rows { 2.0 * H - 8.0 } else { H };
    let x0 = (left + (right - left - w) / 2.0).max(left + 8.0);
    let y0 = BAR + 8.0;
    l.rect = [x0, y0, w, h];
    l.boxes.push(PanelBox { rect: l.rect, fill: t.panel_bg, stroke: t.line, radius: 10.0, stroke_w: 1.0 });
    let mut cy = y0 + (H - CHIP_H) / 2.0;
    let mut row_y = y0;
    let mut x = x0 + 8.0;
    for (b, cw) in brush::ALL.iter().zip(&chip_w) {
        let r = [x, cy, *cw, CHIP_H];
        let on = pen.brush == *b;
        let fill = if on { t.chip } else if panel::inside(r, hover) { t.field } else { [0.0; 4] };
        l.boxes.push(PanelBox { rect: r, fill, stroke: [0.0; 4], radius: 7.0, stroke_w: 0.0 });
        l.texts.push(TextRun { text: b.label().into(), at: [x + 11.0, cy + 18.5], size: 12.5, color: if on { t.accent } else { t.ink } });
        l.hits.push((r, BarHit::Brush(*b)));
        x += cw + gap;
    }
    if two_rows {
        x = x0 + 8.0;
        row_y += H - 8.0;
        cy += H - 8.0;
    } else {
        x += sep - gap;
    }
    for (k, c) in brush::COLORS.iter().enumerate() {
        let r = [x, row_y + (H - DOT) / 2.0, DOT, DOT];
        let rgb = hex(c);
        let on = pen.color.eq_ignore_ascii_case(c);
        if on {
            l.boxes.push(PanelBox { rect: [r[0] - 3.0, r[1] - 3.0, DOT + 6.0, DOT + 6.0], fill: [0.0; 4], stroke: t.accent, radius: DOT / 2.0 + 3.0, stroke_w: 2.0 });
        }
        l.boxes.push(PanelBox { rect: r, fill: [rgb[0], rgb[1], rgb[2], 1.0], stroke: t.line, radius: DOT / 2.0, stroke_w: 1.0 });
        l.hits.push((r, BarHit::Color(k)));
        x += DOT + gap;
    }
    x += sep - gap;
    let minus = [x, cy, 24.0, CHIP_H];
    let plus = [x + size_w - 24.0, cy, 24.0, CHIP_H];
    for (r, s, hit) in [(minus, "−", BarHit::Smaller), (plus, "+", BarHit::Bigger)] {
        if panel::inside(r, hover) {
            l.boxes.push(PanelBox { rect: r, fill: t.field, stroke: [0.0; 4], radius: 7.0, stroke_w: 0.0 });
        }
        l.texts.push(TextRun { text: s.into(), at: [r[0] + 7.0, cy + 19.5], size: 15.0, color: t.ink });
        l.hits.push((r, hit));
    }
    let lw = text_width(&size_label, 12.5);
    l.texts.push(TextRun { text: size_label, at: [x + (size_w - lw) / 2.0, cy + 18.5], size: 12.5, color: t.dim });
    x += size_w + sep;
    let r = [x, cy, done_w, CHIP_H];
    l.boxes.push(PanelBox { rect: r, fill: if panel::inside(r, hover) { t.field } else { [0.0; 4] }, stroke: t.line, radius: 7.0, stroke_w: 1.0 });
    l.texts.push(TextRun { text: "Done".into(), at: [x + 11.0, cy + 18.5], size: 12.5, color: t.ink });
    l.hits.push((r, BarHit::Done));
    l
}

fn hex(c: &str) -> [f32; 3] {
    let c = c.trim_start_matches('#');
    let v = |i: usize| u8::from_str_radix(c.get(i..i + 2).unwrap_or("80"), 16).unwrap_or(128) as f32 / 255.0;
    [v(0), v(2), v(4)]
}

/// A drawing item for a line in board units, with its width factors.
pub fn stroke_item(line: &[[f32; 2]], factors: &[f32], width: f32, color: Option<String>, brush: Brush, top_z: f64, on: Option<Uuid>, drawing: Option<Uuid>) -> Option<(Frame, ItemKind)> {
    if line.is_empty() {
        return None;
    }
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in line {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    // Pad so a straight line still has a box with some size.
    let pad = width;
    let (x, y, w, h) = (b[0] - pad, b[1] - pad, b[2] - b[0] + 2.0 * pad, b[3] - b[1] + 2.0 * pad);
    // 4 decimals of the frame: finer than a pixel on any line under 10,000 px.
    let r4 = |v: f32| (v * 10000.0).round() / 10000.0;
    let points = line.iter().map(|p| [r4((p[0] - x) / w), r4((p[1] - y) / h)]).collect();
    let widths = if factors.iter().all(|f| (*f - 1.0).abs() < 1e-3) { Vec::new() } else { factors.iter().map(|f| (f * 1000.0).round() / 1000.0).collect() };
    let brush = (brush != Brush::Pen).then(|| brush.name().to_owned());
    let frame = Frame { x: x as f64, y: y as f64, w: w as f64, h: h as f64, z: top_z + 1.0, ..Default::default() };
    Some((frame, ItemKind::Stroke { points, width, color, brush, widths, on, drawing }))
}

/// A line drawn on a picture, after the picture's frame changed from
/// `from` to `to`: it keeps its place on the picture and scales with it.
pub(crate) fn follow_frame(stroke: &BoardItem, from: Frame, to: Frame) -> BoardItem {
    let (sx, sy) = (to.w / from.w.max(1e-6), to.h / from.h.max(1e-6));
    let mut s = stroke.clone();
    s.frame.x = to.x + (stroke.frame.x - from.x) * sx;
    s.frame.y = to.y + (stroke.frame.y - from.y) * sy;
    s.frame.w = stroke.frame.w * sx;
    s.frame.h = stroke.frame.h * sy;
    if let ItemKind::Stroke { width, .. } = &mut s.kind {
        *width *= ((sx * sy).sqrt()) as f32;
    }
    s
}

/// How close (in screen pixels) a new line must come to an earlier one to
/// join its drawing.
const JOIN_PX: f32 = 40.0;

impl App {
    /// The drawing tool's settings.
    pub fn pen(&self) -> &brush::PenState {
        &self.pen
    }

    /// D, or the rail's pen: drawing on or off.
    pub fn toggle_drawing(&mut self) {
        self.end_edit();
        self.tool = if self.tool == toolbar::Tool::Pen { toolbar::Tool::Select } else { toolbar::Tool::Pen };
        if self.tool == toolbar::Tool::Pen {
            self.selection.clear();
        }
        self.needs_redraw = true;
    }

    pub(crate) fn pen_bar(&mut self) -> Option<BarLayout> {
        if self.tool != toolbar::Tool::Pen {
            return None;
        }
        let left = self.left_inset();
        let right = self.panel_layout().map_or(self.viewport[0], |p| p.rect[0] - 8.0);
        let font = &mut self.font;
        Some(bar_layout(&self.pen, self.cursor, self.viewport, left, right, &mut |s, size| panel::text_width(font, s, size)))
    }

    /// A press on the options bar. True when it was on the bar.
    pub(crate) fn pen_bar_press(&mut self, pos: [f32; 2]) -> bool {
        let Some(hit) = self.pen_bar().and_then(|l| l.hit_at(pos)) else { return false };
        match hit {
            Some(BarHit::Brush(b)) => self.pen.pick(b),
            Some(BarHit::Color(k)) => self.pen.set_color(brush::COLORS[k]),
            Some(BarHit::Smaller) => self.pen.grow(false),
            Some(BarHit::Bigger) => self.pen.grow(true),
            Some(BarHit::Done) => self.tool = toolbar::Tool::Select,
            None => {}
        }
        self.needs_redraw = true;
        true
    }

    /// Keys while drawing: 1–5 brushes, E eraser, [ ] size. True if used.
    pub(crate) fn pen_key(&mut self, c: char) -> bool {
        match c.to_ascii_lowercase() {
            c @ '1'..='5' => self.pen.pick(brush::ALL[c as usize - '1' as usize]),
            'e' => self.pen.toggle_eraser(),
            '[' => self.pen.grow(false),
            ']' => self.pen.grow(true),
            _ => return false,
        }
        self.needs_redraw = true;
        true
    }

    /// Pointer down on the board while drawing. `straight` (Shift) draws a
    /// straight line from here to where the pointer ends up.
    pub(crate) fn start_draw(&mut self, board: [f32; 2], straight: bool) {
        self.selection.clear();
        if self.pen.brush == Brush::Eraser {
            self.gesture = Gesture::Erase { depth: self.doc.undo_depth(), last: board };
            self.erase_along(board, board);
        } else {
            self.gesture = Gesture::Draw { points: vec![board], pressures: vec![1.0], velocity: 0.0, last_ms: self.now_ms, straight };
        }
    }

    /// The pointer moved while drawing or rubbing out.
    pub(crate) fn draw_move(&mut self, board: [f32; 2]) {
        let zoom = self.camera.zoom;
        let now = self.now_ms;
        match &mut self.gesture {
            Gesture::Draw { points, pressures, velocity, last_ms, straight } => {
                if *straight {
                    points.truncate(1);
                    pressures.truncate(1);
                    points.push(board);
                    pressures.push(1.0);
                    return;
                }
                let last = points.last().copied().unwrap_or(board);
                let d_px = ((board[0] - last[0]).powi(2) + (board[1] - last[1]).powi(2)).sqrt() * zoom;
                if d_px < 1.2 {
                    return;
                }
                // Several moves can arrive within one frame; count those as 8 ms.
                let dt = now.saturating_sub(*last_ms).max(8) as f32;
                *last_ms = now;
                *velocity = 0.75 * *velocity + 0.25 * d_px / dt;
                points.push(board);
                pressures.push(brush::speed_pressure(*velocity));
            }
            Gesture::Erase { last, .. } => {
                let from = std::mem::replace(last, board);
                self.erase_along(from, board);
            }
            _ => {}
        }
        self.needs_redraw = true;
    }

    /// Pointer up while drawing or rubbing out.
    pub(crate) fn finish_draw(&mut self, board: [f32; 2], gesture: Gesture) {
        match gesture {
            Gesture::Draw { mut points, mut pressures, straight, .. } => {
                if straight {
                    points.truncate(1);
                    pressures.truncate(1);
                }
                if points.last() != Some(&board) {
                    points.push(board);
                    pressures.push(pressures.last().copied().unwrap_or(1.0));
                }
                self.add_stroke_with(&points, &pressures);
            }
            Gesture::Erase { depth, last } => {
                self.erase_along(last, board);
                // One drag of the eraser is one undo step.
                self.doc.join_undo_dropping_transient(depth);
            }
            _ => {}
        }
        self.needs_redraw = true;
    }

    /// The picture or video a new line was drawn on: the top one holding
    /// at least half of its points.
    pub(crate) fn picture_under(&self, line: &[[f32; 2]]) -> Option<Uuid> {
        let mut media: Vec<&BoardItem> = self.doc.board.live_items().filter(|i| matches!(i.kind, ItemKind::Media { .. })).collect();
        media.sort_by(|a, b| b.frame.z.total_cmp(&a.frame.z));
        media
            .into_iter()
            .find(|m| {
                let f = m.frame;
                let inside = line.iter().filter(|p| p[0] as f64 >= f.x && p[0] as f64 <= f.x + f.w && p[1] as f64 >= f.y && p[1] as f64 <= f.y + f.h).count();
                inside * 2 >= line.len()
            })
            .map(|m| m.id)
    }

    /// The drawing a new line joins: the newest one on the same picture (or
    /// on the board) with a line within [`JOIN_PX`] of it.
    pub(crate) fn drawing_near(&self, line: &[[f32; 2]], on: Option<Uuid>) -> Option<Uuid> {
        let reach = JOIN_PX / self.camera.zoom;
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for p in line {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        let mut best: Option<(f64, Uuid)> = None;
        for i in self.doc.board.live_items() {
            let ItemKind::Stroke { points, drawing: Some(d), on: o, .. } = &i.kind else { continue };
            if *o != on {
                continue;
            }
            let f = i.frame;
            let near_box = (b[0] - reach) as f64 <= f.x + f.w && (b[2] + reach) as f64 >= f.x && (b[1] - reach) as f64 <= f.y + f.h && (b[3] + reach) as f64 >= f.y;
            if !near_box || best.is_some_and(|(z, _)| z >= f.z) {
                continue;
            }
            let pts = scene::stroke_points(points, [f.x as f32, f.y as f32, f.w as f32, f.h as f32]);
            let close = pts.windows(2).any(|w| line.iter().any(|p| scene::segment_distance(*p, w[0], w[1]) <= reach))
                || (pts.len() == 1 && line.iter().any(|p| (p[0] - pts[0][0]).hypot(p[1] - pts[0][1]) <= reach));
            if close {
                best = Some((f.z, *d));
            }
        }
        best.map(|(_, d)| d)
    }

    /// The other lines of the drawing a line belongs to.
    pub(crate) fn drawing_mates(&self, id: &Uuid) -> Vec<Uuid> {
        let Some(ItemKind::Stroke { drawing: Some(d), .. }) = self.doc.item(id).map(|i| &i.kind) else { return Vec::new() };
        self.doc
            .board
            .live_items()
            .filter(|i| i.id != *id && matches!(&i.kind, ItemKind::Stroke { drawing: Some(x), .. } if x == d))
            .map(|i| i.id)
            .collect()
    }

    /// Lines drawn on any of these items.
    pub(crate) fn lines_on(&self, ids: &std::collections::BTreeSet<Uuid>) -> Vec<BoardItem> {
        self.doc.board.live_items().filter(|i| matches!(&i.kind, ItemKind::Stroke { on: Some(o), .. } if ids.contains(o))).cloned().collect()
    }

    /// Adds a line drawn with the current brush, colour and size.
    pub fn add_stroke_with(&mut self, line: &[[f32; 2]], pressures: &[f32]) -> Option<Uuid> {
        let b = if self.pen.brush == Brush::Eraser { Brush::Pen } else { self.pen.brush };
        let width = self.pen.size() / self.camera.zoom;
        let (line, pressures) = brush::resample(line, pressures, brush::point_step(width, self.camera.zoom));
        let factors = brush::width_factors(&line, &pressures, width, b);
        let on = self.picture_under(&line);
        let drawing = Some(self.drawing_near(&line, on).unwrap_or_else(Uuid::new_v4));
        let (frame, kind) = stroke_item(&line, &factors, width, Some(self.pen.color.clone()), b, self.doc.top_z(), on, drawing)?;
        let id = self.doc.add(vec![(frame, kind)])[0];
        self.needs_redraw = true;
        Some(id)
    }

    /// Rubs out the parts of lines the eraser passes over from `a` to `b`
    /// (board points; pointer moves can be far apart): the rest of each
    /// line stays as one or more lines.
    pub(crate) fn erase_along(&mut self, a: [f32; 2], b: [f32; 2]) {
        let r = self.pen.size() / 2.0 / self.camera.zoom;
        let steps = ((b[0] - a[0]).hypot(b[1] - a[1]) / (r * 0.5)).ceil().clamp(1.0, 2000.0) as usize;
        let path: Vec<[f32; 2]> = if a == b {
            vec![b]
        } else {
            (0..=steps).map(|k| k as f32 / steps as f32).map(|t| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]).collect()
        };
        let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
        let hits: Vec<BoardItem> = self
            .doc
            .board
            .live_items()
            .filter(|i| matches!(i.kind, ItemKind::Stroke { .. }))
            .filter(|i| {
                let f = i.frame;
                let m = r as f64;
                hi[0] as f64 >= f.x - m && lo[0] as f64 <= f.x + f.w + m && hi[1] as f64 >= f.y - m && lo[1] as f64 <= f.y + f.h + m
            })
            .cloned()
            .collect();
        for item in hits {
            let ItemKind::Stroke { points, width, color, brush: b, widths, on, drawing } = &item.kind else { continue };
            let f = item.frame;
            let pts = scene::stroke_points(points, [f.x as f32, f.y as f32, f.w as f32, f.h as f32]);
            let factors: Vec<f32> = if widths.len() == pts.len() { widths.clone() } else { vec![1.0; pts.len()] };
            let reach = r + 0.35 * width;
            if !path.iter().any(|p| brush::erase(&pts, &factors, *p, reach).is_some()) {
                continue;
            }
            // Lines with points far apart (older ones) get more first, so
            // only what's under the eraser goes.
            let gap = reach * 0.5;
            let sparse = pts.windows(2).any(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]) > gap);
            let start = if sparse { brush::resample(&pts, &factors, brush::point_step(*width, self.camera.zoom).min(gap)) } else { (pts, factors) };
            let mut parts = vec![start];
            for p in &path {
                parts = parts
                    .into_iter()
                    .flat_map(|(line, fs)| brush::erase(&line, &fs, *p, reach).unwrap_or_else(|| vec![(line, fs)]))
                    .collect();
            }
            self.doc.delete(&[item.id].into());
            let kinds: Vec<(Frame, ItemKind)> = parts
                .iter()
                .filter_map(|(line, fs)| {
                    let (mut frame, kind) = stroke_item(line, fs, *width, color.clone(), Brush::from_name(b.as_deref()), 0.0, *on, *drawing)?;
                    frame.z = f.z; // stays at the same depth
                    Some((frame, kind))
                })
                .collect();
            if !kinds.is_empty() {
                self.doc.add(kinds);
            }
        }
    }

    /// The line being drawn: (solid pieces, see-through pieces).
    pub(crate) fn drawing_preview(&self) -> (Vec<CurveInstance>, Vec<CurveInstance>) {
        let Gesture::Draw { points, pressures, .. } = &self.gesture else { return (Vec::new(), Vec::new()) };
        let b = self.pen.brush;
        let width = self.pen.size() / self.camera.zoom;
        let (points, pressures) = brush::resample(points, pressures, brush::point_step(width, self.camera.zoom));
        let factors = brush::width_factors(&points, &pressures, width, b);
        let a = b.profile().alpha;
        let c = hex(&self.pen.color);
        let curves = scene::pen_curves_varying(&points, &factors, width, [c[0] * a, c[1] * a, c[2] * a, a]);
        if a < 1.0 { (Vec::new(), curves) } else { (curves, Vec::new()) }
    }

    /// The options bar and the size ring at the pointer, in screen pixels.
    pub(crate) fn draw_pen_ui(&mut self, renderer: &mut Renderer) {
        let Some(l) = self.pen_bar() else { return };
        let view = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let mut boxes: Vec<BoxInstance> = l.boxes.iter().map(|b| BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] }).collect();
        let size = self.pen.size();
        let on_ui = panel::inside(l.rect, self.cursor) || self.cursor[1] < BAR;
        if !on_ui && (size >= 5.0 || self.pen.brush == Brush::Eraser) {
            let r = size.max(5.0) / 2.0;
            let c = self.cursor;
            let ink = theme::t().ink;
            boxes.push(BoxInstance { rect: [c[0] - r, c[1] - r, 2.0 * r, 2.0 * r], fill: [0.0; 4], stroke: [ink[0] * 0.7, ink[1] * 0.7, ink[2] * 0.7, 0.7], shape: [r, 1.2] });
        }
        renderer.draw_boxes(&view, &boxes);
        let mut glyphs = Vec::new();
        for t in &l.texts {
            scene::add_text(&mut self.font, &mut glyphs, &t.text, t.at, t.size, t.color);
        }
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        renderer.draw_glyphs(&view, &glyphs);
    }
}

#[cfg(test)]
mod tests {
    use crate::{Button, Key, Mods};
    use wall_format::{Board, ItemKind};

    use super::*;

    fn app() -> App {
        let mut a = App::new(Board::new("t", 0), "test");
        a.resize([1200.0, 800.0]);
        a.camera = crate::Camera { center: [600.0, 400.0], zoom: 1.0 };
        a.panel_open = false;
        a
    }

    fn drag(a: &mut App, from: [f32; 2], to: [f32; 2], mods: Mods) {
        a.pointer_down(Button::Primary, from, mods);
        for k in 1..=20 {
            let t = k as f32 / 20.0;
            a.set_time(1000 + k * 16);
            a.pointer_move([from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t]);
        }
        a.pointer_up(Button::Primary, to);
    }

    fn strokes(a: &App) -> Vec<BoardItem> {
        let mut v: Vec<BoardItem> = a.doc.board.live_items().filter(|i| matches!(i.kind, ItemKind::Stroke { .. })).cloned().collect();
        v.sort_by(|x, y| x.frame.z.total_cmp(&y.frame.z));
        v
    }

    fn drawing_of(a: &App, id: &Uuid) -> Option<Uuid> {
        match &a.doc.item(id)?.kind {
            ItemKind::Stroke { drawing, .. } => *drawing,
            _ => None,
        }
    }

    #[test]
    fn lines_drawn_close_together_are_one_drawing_picked_and_moved_together() {
        let mut a = app();
        a.key(Key::Char('d'));
        drag(&mut a, [300.0, 300.0], [400.0, 300.0], Mods::default());
        drag(&mut a, [300.0, 320.0], [400.0, 320.0], Mods::default()); // 20 px away: joins
        drag(&mut a, [300.0, 600.0], [400.0, 600.0], Mods::default()); // far: its own
        let s = strokes(&a);
        assert_eq!(s.len(), 3);
        let (d0, d1, d2) = (drawing_of(&a, &s[0].id), drawing_of(&a, &s[1].id), drawing_of(&a, &s[2].id));
        assert!(d0.is_some());
        assert_eq!(d0, d1);
        assert_ne!(d0, d2);
        // Pick one line: the whole drawing; drag it: both move.
        a.key(Key::Char('d'));
        let before: Vec<f64> = s.iter().map(|i| i.frame.x).collect();
        drag(&mut a, [350.0, 300.0], [450.0, 300.0], Mods::default());
        assert_eq!(a.selection.len(), 2);
        let after: Vec<f64> = s.iter().map(|i| a.doc.item(&i.id).unwrap().frame.x).collect();
        assert!((after[0] - before[0] - 100.0).abs() < 1.0 && (after[1] - before[1] - 100.0).abs() < 1.0, "{before:?} {after:?}");
        assert!((after[2] - before[2]).abs() < 1e-6, "the far line stays");
        // A copy is a separate drawing.
        a.key(Key::Duplicate);
        let copies: Vec<Uuid> = a.selection.iter().copied().collect();
        assert_eq!(copies.len(), 2);
        assert_eq!(drawing_of(&a, &copies[0]), drawing_of(&a, &copies[1]));
        assert_ne!(drawing_of(&a, &copies[0]), d0);
    }

    #[test]
    fn lines_drawn_on_a_picture_move_resize_copy_and_delete_with_it() {
        let mut a = app();
        let frame = Frame { x: 200.0, y: 200.0, w: 400.0, h: 300.0, ..Default::default() };
        let pic = a.doc.add(vec![(frame, ItemKind::Media { hash: wall_format::ContentHash::of(b"pic"), crop: None, look: Default::default() })])[0];
        a.key(Key::Char('d'));
        drag(&mut a, [300.0, 300.0], [500.0, 400.0], Mods::default()); // on the picture
        drag(&mut a, [700.0, 600.0], [800.0, 650.0], Mods::default()); // off it
        let s = strokes(&a);
        let on = |a: &App, id: &Uuid| match &a.doc.item(id).unwrap().kind {
            ItemKind::Stroke { on, .. } => *on,
            _ => None,
        };
        assert_eq!(on(&a, &s[0].id), Some(pic));
        assert_eq!(on(&a, &s[1].id), None);
        a.key(Key::Char('d'));
        // Move the picture (grab it away from the line).
        drag(&mut a, [220.0, 480.0], [270.0, 480.0], Mods::default());
        assert_eq!(a.selection, [pic].into());
        assert!((a.doc.item(&s[0].id).unwrap().frame.x - s[0].frame.x - 50.0).abs() < 1.0);
        assert!((a.doc.item(&s[1].id).unwrap().frame.x - s[1].frame.x).abs() < 1e-6);
        // Resize it to double: the line doubles too, and stays on it.
        let f = a.doc.item(&pic).unwrap().frame;
        let line = a.doc.item(&s[0].id).unwrap().frame;
        let corner = [(f.x + f.w) as f32, (f.y + f.h) as f32];
        drag(&mut a, corner, [corner[0] + f.w as f32, corner[1] + f.h as f32], Mods::default());
        let f2 = a.doc.item(&pic).unwrap().frame;
        let line2 = a.doc.item(&s[0].id).unwrap().frame;
        assert!((f2.w / f.w - 2.0).abs() < 0.02, "{f2:?}");
        assert!((line2.w / line.w - 2.0).abs() < 0.02 && ((line2.x - f2.x) / (line.x - f.x) - 2.0).abs() < 0.02);
        // One undo puts both back.
        a.key(Key::Undo);
        assert_eq!(a.doc.item(&pic).unwrap().frame.w, f.w);
        assert!((a.doc.item(&s[0].id).unwrap().frame.w - line.w).abs() < 1e-6);
        // A copy of the picture brings a copy of the line, on the copy.
        a.selection = [pic].into();
        a.key(Key::Duplicate);
        let copy = *a.selection.iter().find(|id| matches!(a.doc.item(id).unwrap().kind, ItemKind::Media { .. })).unwrap();
        assert!(strokes(&a).iter().any(|l| on(&a, &l.id) == Some(copy)));
        // Deleting the picture deletes its line.
        a.selection = [pic].into();
        a.key(Key::Delete);
        assert!(a.doc.item(&s[0].id).is_none_or(|i| i.deleted));
        assert!(a.doc.item(&s[1].id).is_some_and(|i| !i.deleted));
    }

    #[test]
    fn crossing_solid_lines_over_a_picture_draw_in_one_batch_but_markers_and_notes_split() {
        let mut a = app();
        let frame = Frame { x: 100.0, y: 100.0, w: 800.0, h: 500.0, ..Default::default() };
        a.doc.add(vec![(frame, ItemKind::Media { hash: wall_format::ContentHash::of(b"pic"), crop: None, look: Default::default() })]);
        a.key(Key::Char('d'));
        for k in 0..50 {
            let y = 150.0 + (k % 10) as f32 * 30.0;
            drag(&mut a, [150.0 + k as f32 * 5.0, y], [700.0, 550.0 - y / 2.0], Mods::default());
        }
        a.scene.update(&a.doc, &mut a.font, &a.board_names);
        assert_eq!(a.scene.run_count(), 1, "pen lines crossing each other and the picture");
        // Two crossing marker lines must blend separately: two batches more at most.
        a.key(Key::Char('4'));
        drag(&mut a, [200.0, 200.0], [600.0, 400.0], Mods::default());
        drag(&mut a, [200.0, 400.0], [600.0, 200.0], Mods::default());
        a.scene.update(&a.doc, &mut a.font, &a.board_names);
        assert_eq!(a.scene.run_count(), 2, "the second marker crosses the first");
        // A pen line over a marker goes above it: a new batch.
        a.key(Key::Char('1'));
        drag(&mut a, [400.0, 150.0], [400.0, 450.0], Mods::default());
        a.scene.update(&a.doc, &mut a.font, &a.board_names);
        assert_eq!(a.scene.run_count(), 3);
    }

    #[test]
    fn the_bar_stays_clear_of_the_side_panel_in_a_narrow_window() {
        let mut a = app();
        a.resize([800.0, 700.0]);
        a.panel_open = true;
        a.key(Key::Char('d'));
        let panel = a.panel_layout().unwrap().rect;
        let bar = a.pen_bar().unwrap();
        assert!(bar.rect[0] + bar.rect[2] <= panel[0], "bar {:?} panel {:?}", bar.rect, panel);
        assert!(bar.rect[3] > H, "two rows when one doesn't fit");
        for (r, _) in &bar.hits {
            assert!(r[0] >= bar.rect[0] && r[0] + r[2] <= bar.rect[0] + bar.rect[2] && r[1] + r[3] <= bar.rect[1] + bar.rect[3], "{r:?} inside {:?}", bar.rect);
        }
        // Wide window: one row.
        a.resize([1800.0, 900.0]);
        assert_eq!(a.pen_bar().unwrap().rect[3], H);
    }

    #[test]
    fn d_draws_with_the_chosen_brush_colour_and_size_then_the_eraser_cuts_it() {
        let mut a = app();
        a.key(Key::Char('d'));
        assert_eq!(a.tool, toolbar::Tool::Pen);
        // 4 = marker; a colour from the bar.
        a.key(Key::Char('4'));
        let bar = a.pen_bar().unwrap();
        let blue = bar.hits.iter().find(|(_, h)| *h == BarHit::Color(5)).unwrap().0;
        a.pointer_down(Button::Primary, [blue[0] + 5.0, blue[1] + 5.0], Mods::default());
        a.pointer_up(Button::Primary, [blue[0] + 5.0, blue[1] + 5.0]);
        assert_eq!(a.pen.color, brush::COLORS[5]);
        drag(&mut a, [300.0, 400.0], [700.0, 400.0], Mods::default());
        let s = strokes(&a);
        assert_eq!(s.len(), 1);
        let ItemKind::Stroke { width, color, brush: b, .. } = &s[0].kind else { unreachable!() };
        assert_eq!((*width, color.as_deref(), b.as_deref()), (22.0, Some(brush::COLORS[5]), Some("marker")));

        // The eraser through the middle leaves two lines, as one undo step.
        a.key(Key::Char('e'));
        drag(&mut a, [500.0, 300.0], [500.0, 500.0], Mods::default());
        assert_eq!(strokes(&a).len(), 2);
        a.key(Key::Undo);
        assert_eq!(strokes(&a).len(), 1, "one undo brings the whole line back");
        // E again: back to the marker.
        a.key(Key::Char('e'));
        assert_eq!(a.pen.brush, Brush::Marker);
    }

    #[test]
    fn a_fast_eraser_misses_nothing_and_leaves_no_hidden_pieces() {
        let mut a = app();
        a.toggle_drawing();
        a.key(Key::Char('4'));
        for y in [300.0, 350.0, 400.0] {
            drag(&mut a, [300.0, y], [700.0, y], Mods::default());
        }
        // A straight line of only its two ends, as older boards have.
        let frame = Frame { x: 300.0, y: 450.0, w: 400.0, h: 1.0, ..Default::default() };
        a.doc.add(vec![(frame, ItemKind::Stroke { points: vec![[0.0, 0.5], [1.0, 0.5]], width: 4.0, color: None, brush: None, widths: Vec::new(), on: None, drawing: None })]);
        let before = a.doc.board.items.len();
        a.key(Key::Char('e'));
        // Two moves, far apart, across all four.
        a.pointer_down(Button::Primary, [500.0, 250.0], Mods::default());
        a.pointer_move([500.0, 360.0]);
        a.pointer_move([500.0, 520.0]);
        a.pointer_up(Button::Primary, [500.0, 520.0]);
        assert_eq!(strokes(&a).len(), 8, "each line in two");
        assert_eq!(a.doc.board.items.len(), before + 8, "only the four originals (deleted) and the eight pieces");
        let longest = strokes(&a).iter().map(|i| i.frame.w).fold(0.0, f64::max);
        // 200 to the middle, less the reach (15 + 0.35 × 22), plus the
        // frame's padding of one line width (22) at each end.
        assert!((longest - (200.0 - 22.7 + 44.0)).abs() < 8.0, "only what was under the eraser went ({longest})");
        a.key(Key::Undo);
        assert_eq!(strokes(&a).len(), 4);
        a.key(Key::Redo);
        assert_eq!(strokes(&a).len(), 8);
        // Back and forth over the same lines leaves nothing hidden either.
        let before = a.doc.board.items.len();
        a.pointer_down(Button::Primary, [420.0, 280.0], Mods::default());
        for k in 0..30 {
            a.pointer_move([420.0 + 2.0 * k as f32, if k % 2 == 0 { 480.0 } else { 280.0 }]);
        }
        a.pointer_up(Button::Primary, [480.0, 480.0]);
        let live_new = a.doc.board.items.len() - before;
        assert!(live_new <= 8, "{live_new} items more after 30 moves");
    }

    #[test]
    fn the_pen_tapers_and_shift_draws_straight() {
        let mut a = app();
        a.toggle_drawing();
        drag(&mut a, [300.0, 300.0], [700.0, 420.0], Mods::default());
        let s = strokes(&a);
        let ItemKind::Stroke { widths, .. } = &s[0].kind else { unreachable!() };
        assert!(widths.first().unwrap() < &0.3 && widths.last().unwrap() < &0.3, "tapered ends");
        drag(&mut a, [300.0, 600.0], [700.0, 650.0], Mods { shift: true, ..Default::default() });
        let straight = strokes(&a).into_iter().find(|i| i.frame.y > 500.0).unwrap();
        let ItemKind::Stroke { points, widths, .. } = &straight.kind else { unreachable!() };
        let pts = scene::stroke_points(points, [straight.frame.x as f32, straight.frame.y as f32, straight.frame.w as f32, straight.frame.h as f32]);
        assert!(pts.iter().all(|p| scene::segment_distance(*p, [300.0, 600.0], [700.0, 650.0]) < 0.1), "on the straight line");
        assert!(widths.iter().fold(0.0f32, |m, w| m.max(*w)) > 0.95, "full width between its tapered ends");
        a.key(Key::Escape);
        assert_eq!(a.tool, toolbar::Tool::Select);
    }
}
