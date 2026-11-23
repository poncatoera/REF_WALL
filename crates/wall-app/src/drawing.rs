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
pub fn bar_layout(pen: &brush::PenState, hover: [f32; 2], viewport: [f32; 2], left: f32, text_width: &mut dyn FnMut(&str, f32) -> f32) -> BarLayout {
    let t = theme::t();
    let mut l = BarLayout::default();
    let chip_w: Vec<f32> = brush::ALL.iter().map(|b| text_width(b.label(), 12.5) + 22.0).collect();
    let size_label = format!("{:.0} px", pen.size());
    let size_w = 26.0 + text_width("000 px", 12.5) + 26.0;
    let done_w = text_width("Done", 12.5) + 22.0;
    let gap = 6.0;
    let sep = 14.0;
    let w = 8.0 + chip_w.iter().sum::<f32>() + gap * 4.0 + sep + 8.0 * (DOT + gap) + sep + size_w + sep + done_w + 8.0;
    let x0 = (left + (viewport[0] - left - w) / 2.0).max(left + 8.0);
    let y0 = BAR + 8.0;
    l.rect = [x0, y0, w, H];
    l.boxes.push(PanelBox { rect: l.rect, fill: t.panel_bg, stroke: t.line, radius: 10.0, stroke_w: 1.0 });
    let cy = y0 + (H - CHIP_H) / 2.0;
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
    x += sep - gap;
    for (k, c) in brush::COLORS.iter().enumerate() {
        let r = [x, y0 + (H - DOT) / 2.0, DOT, DOT];
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
pub fn stroke_item(line: &[[f32; 2]], factors: &[f32], width: f32, color: Option<String>, brush: Brush, top_z: f64) -> Option<(Frame, ItemKind)> {
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
    let points = line.iter().map(|p| [(p[0] - x) / w, (p[1] - y) / h]).collect();
    let widths = if factors.iter().all(|f| (*f - 1.0).abs() < 1e-3) { Vec::new() } else { factors.iter().map(|f| (f * 1000.0).round() / 1000.0).collect() };
    let brush = (brush != Brush::Pen).then(|| brush.name().to_owned());
    let frame = Frame { x: x as f64, y: y as f64, w: w as f64, h: h as f64, z: top_z + 1.0, ..Default::default() };
    Some((frame, ItemKind::Stroke { points, width, color, brush, widths }))
}

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
        let font = &mut self.font;
        Some(bar_layout(&self.pen, self.cursor, self.viewport, left, &mut |s, size| panel::text_width(font, s, size)))
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
            self.gesture = Gesture::Erase { depth: self.doc.undo_depth() };
            self.erase_at(board);
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
            Gesture::Erase { .. } => self.erase_at(board),
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
            Gesture::Erase { depth } => {
                self.erase_at(board);
                // One drag of the eraser is one undo step.
                self.doc.join_undo_since(depth);
            }
            _ => {}
        }
        self.needs_redraw = true;
    }

    /// Adds a line drawn with the current brush, colour and size.
    pub fn add_stroke_with(&mut self, line: &[[f32; 2]], pressures: &[f32]) -> Option<Uuid> {
        let b = if self.pen.brush == Brush::Eraser { Brush::Pen } else { self.pen.brush };
        let width = self.pen.size() / self.camera.zoom;
        let factors = brush::width_factors(line, pressures, width, b);
        let (frame, kind) = stroke_item(line, &factors, width, Some(self.pen.color.clone()), b, self.doc.top_z())?;
        let id = self.doc.add(vec![(frame, kind)])[0];
        self.needs_redraw = true;
        Some(id)
    }

    /// Rubs out the parts of lines under the eraser at a board point: the
    /// rest of each line stays as one or more lines.
    pub(crate) fn erase_at(&mut self, p: [f32; 2]) {
        let r = self.pen.size() / 2.0 / self.camera.zoom;
        let hits: Vec<BoardItem> = self
            .doc
            .board
            .live_items()
            .filter(|i| matches!(i.kind, ItemKind::Stroke { .. }))
            .filter(|i| {
                let f = i.frame;
                let m = r as f64;
                p[0] as f64 >= f.x - m && p[0] as f64 <= f.x + f.w + m && p[1] as f64 >= f.y - m && p[1] as f64 <= f.y + f.h + m
            })
            .cloned()
            .collect();
        for item in hits {
            let ItemKind::Stroke { points, width, color, brush: b, widths } = &item.kind else { continue };
            let f = item.frame;
            let pts = scene::stroke_points(points, [f.x as f32, f.y as f32, f.w as f32, f.h as f32]);
            let factors: Vec<f32> = if widths.len() == pts.len() { widths.clone() } else { vec![1.0; pts.len()] };
            let Some(parts) = brush::erase(&pts, &factors, p, r + 0.35 * width) else { continue };
            self.doc.delete(&[item.id].into());
            let kinds: Vec<(Frame, ItemKind)> = parts
                .iter()
                .filter_map(|(line, fs)| {
                    let (mut frame, kind) = stroke_item(line, fs, *width, color.clone(), Brush::from_name(b.as_deref()), 0.0)?;
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
        let factors = brush::width_factors(points, pressures, width, b);
        let a = b.profile().alpha;
        let c = hex(&self.pen.color);
        let curves = scene::pen_curves_varying(points, &factors, width, [c[0] * a, c[1] * a, c[2] * a, a]);
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
        a.doc.board.live_items().filter(|i| matches!(i.kind, ItemKind::Stroke { .. })).cloned().collect()
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
    fn the_pen_tapers_and_shift_draws_straight() {
        let mut a = app();
        a.toggle_drawing();
        drag(&mut a, [300.0, 300.0], [700.0, 420.0], Mods::default());
        let s = strokes(&a);
        let ItemKind::Stroke { widths, .. } = &s[0].kind else { unreachable!() };
        assert!(widths.first().unwrap() < &0.3 && widths.last().unwrap() < &0.3, "tapered ends");
        drag(&mut a, [300.0, 600.0], [700.0, 650.0], Mods { shift: true, ..Default::default() });
        let lens: Vec<usize> = strokes(&a).iter().map(|i| match &i.kind { ItemKind::Stroke { points, .. } => points.len(), _ => 0 }).collect();
        assert!(lens.contains(&2), "straight: just its two ends ({lens:?})");
        a.key(Key::Escape);
        assert_eq!(a.tool, toolbar::Tool::Select);
    }
}
