//! The app logic shared by desktop and web: camera, input, scene.
//! Shells (apps/desktop, apps/web) only create the window or canvas, forward
//! input here and call [`App::render`].

use wall_render::{BoxInstance, GlyphInstance, Renderer, ViewUniform};
use wall_text::Font;

/// Pan and zoom over the infinite board.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Board point at the centre of the screen.
    pub center: [f32; 2],
    /// Screen pixels per board unit.
    pub zoom: f32,
}

pub const MIN_ZOOM: f32 = 0.01;
pub const MAX_ZOOM: f32 = 64.0;

impl Camera {
    pub fn screen_to_board(&self, p: [f32; 2], size: [f32; 2]) -> [f32; 2] {
        [
            self.center[0] + (p[0] - size[0] * 0.5) / self.zoom,
            self.center[1] + (p[1] - size[1] * 0.5) / self.zoom,
        ]
    }

    /// Moves the board with the pointer by a screen-pixel delta.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.center[0] -= dx / self.zoom;
        self.center[1] -= dy / self.zoom;
    }

    /// Zooms by `factor` keeping the board point under `cursor` fixed.
    pub fn zoom_at(&mut self, factor: f32, cursor: [f32; 2], size: [f32; 2]) {
        let before = self.screen_to_board(cursor, size);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.screen_to_board(cursor, size);
        self.center[0] += before[0] - after[0];
        self.center[1] += before[1] - after[1];
    }
}

pub struct App {
    pub camera: Camera,
    boxes: Vec<BoxInstance>,
    font: Font,
    glyphs: Vec<GlyphInstance>,
    dragging: bool,
    last_cursor: Option<[f32; 2]>,
}

/// Wheel input in "lines" (desktop mouse) or pixels (trackpad / browser).
#[derive(Debug, Clone, Copy)]
pub enum Wheel {
    Lines(f32),
    Pixels(f32),
}

const BACKGROUND: [f32; 4] = [0.075, 0.078, 0.086, 1.0];

impl App {
    /// A grid of `cols` × `rows` cards, enough to see performance.
    pub fn demo(cols: u32, rows: u32) -> Self {
        let mut boxes = Vec::with_capacity((cols * rows) as usize);
        for r in 0..rows {
            for c in 0..cols {
                let hue = ((c * 7 + r * 13) % 360) as f32 / 360.0;
                let [cr, cg, cb] = hsv(hue, 0.35, 0.9);
                let w = 160.0 + ((c * 37 + r * 11) % 5) as f32 * 30.0;
                let h = 100.0 + ((c * 17 + r * 29) % 4) as f32 * 40.0;
                boxes.push(BoxInstance {
                    rect: [c as f32 * 320.0, r as f32 * 280.0, w, h],
                    fill: [cr * 0.18, cg * 0.18, cb * 0.18, 1.0],
                    stroke: [cr, cg, cb, 1.0],
                    shape: [12.0, 2.0],
                });
            }
        }
        let mut font = Font::new(wall_text::INTER_REGULAR).expect("bundled font parses");
        let mut glyphs = Vec::new();
        let titles = ["Title sequence", "Logo reveal", "Kinetic type", "Moodboard", "Style frame", "Walk cycle", "Neon", "Grain"];
        for (i, b) in boxes.iter().enumerate() {
            let [x, y, _, _] = b.rect;
            let title = titles[i % titles.len()];
            let color = [0.93, 0.93, 0.95, 1.0];
            add_text(&mut font, &mut glyphs, title, [x + 16.0, y + 34.0], 18.0, color);
            let sub = format!("REF {:04}", i + 1);
            add_text(&mut font, &mut glyphs, &sub, [x + 16.0, y + 56.0], 12.0, [b.stroke[0], b.stroke[1], b.stroke[2], 1.0]);
        }
        add_text(&mut font, &mut glyphs, "Wall", [0.0, -120.0], 96.0, [1.0, 1.0, 1.0, 1.0]);
        add_text(&mut font, &mut glyphs, "Rust + Slang, Slug-style text, SDF cards", [4.0, -70.0], 22.0, [0.6, 0.62, 0.66, 1.0]);

        Self {
            camera: Camera { center: [620.0, 200.0], zoom: 1.0 },
            boxes,
            font,
            glyphs,
            dragging: false,
            last_cursor: None,
        }
    }

    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    pub fn box_count(&self) -> usize {
        self.boxes.len()
    }

    pub fn pointer_down(&mut self) {
        self.dragging = true;
    }

    pub fn pointer_up(&mut self) {
        self.dragging = false;
    }

    /// Cursor position in physical pixels.
    pub fn pointer_move(&mut self, p: [f32; 2]) {
        if let (true, Some(last)) = (self.dragging, self.last_cursor) {
            self.camera.pan(p[0] - last[0], p[1] - last[1]);
        }
        self.last_cursor = Some(p);
    }

    pub fn wheel(&mut self, w: Wheel, size: [f32; 2]) {
        let steps = match w {
            Wheel::Lines(l) => l,
            Wheel::Pixels(p) => p / 100.0,
        };
        let cursor = self.last_cursor.unwrap_or([size[0] * 0.5, size[1] * 0.5]);
        self.camera.zoom_at(1.15f32.powf(steps), cursor, size);
    }

    pub fn render(&mut self, renderer: &mut Renderer, size_px: [u32; 2]) {
        if self.font.atlas.dirty {
            renderer.upload_glyph_data(&self.font.atlas.curve_texture(), &self.font.atlas.band_texture());
            self.font.atlas.dirty = false;
        }
        let size = [size_px[0] as f32, size_px[1] as f32];
        renderer.begin_frame(size_px, BACKGROUND);
        let view = ViewUniform::new(self.camera.center, self.camera.zoom, size);
        renderer.draw_boxes(&view, &self.boxes);
        renderer.draw_glyphs(&view, &self.glyphs);
    }
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

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
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
    fn zoom_keeps_point_under_cursor() {
        let size = [1000.0, 800.0];
        let mut cam = Camera { center: [50.0, -20.0], zoom: 1.3 };
        let cursor = [870.0, 120.0];
        let before = cam.screen_to_board(cursor, size);
        cam.zoom_at(2.5, cursor, size);
        let after = cam.screen_to_board(cursor, size);
        assert!((before[0] - after[0]).abs() < 1e-3 && (before[1] - after[1]).abs() < 1e-3);
        assert!((cam.zoom - 3.25).abs() < 1e-5);
    }

    #[test]
    fn text_quads_sit_on_the_baseline() {
        let mut font = Font::new(wall_text::INTER_REGULAR).unwrap();
        let mut out = Vec::new();
        add_text(&mut font, &mut out, "Hx", [100.0, 200.0], 50.0, [1.0; 4]);
        assert_eq!(out.len(), 2);
        // 'H' sits on the baseline: its bottom edge is at y = 200.
        let h = out[0].rect;
        assert!((h[1] + h[3] - 200.0).abs() < 0.5, "{h:?}");
        assert!(h[0] >= 100.0 && h[0] < 110.0);
        assert!(out[1].rect[0] > h[0] + h[2]);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut cam = Camera { center: [0.0, 0.0], zoom: 1.0 };
        cam.zoom_at(1e9, [0.0, 0.0], [10.0, 10.0]);
        assert_eq!(cam.zoom, MAX_ZOOM);
    }

    #[test]
    fn drag_moves_board_with_pointer() {
        let mut app = App::demo(2, 2);
        app.camera.zoom = 2.0;
        let start = app.camera.center;
        app.pointer_move([100.0, 100.0]);
        app.pointer_down();
        app.pointer_move([140.0, 80.0]);
        assert_eq!(app.camera.center, [start[0] - 20.0, start[1] + 10.0]);
        app.pointer_up();
        app.pointer_move([0.0, 0.0]);
        assert_eq!(app.camera.center, [start[0] - 20.0, start[1] + 10.0]);
    }
}
