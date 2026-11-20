//! The dot grid on the board, and switching dark / light mode.

use wall_render::{BoxInstance, Renderer, ViewUniform};

use crate::{theme, App};

/// Most dots drawn in one frame (a 4K window at the densest spacing is
/// about 30,000; past this, skip rather than stall).
const MAX_DOTS: usize = 60_000;

impl App {
    /// Dark or light mode for the whole app. Shells remember the choice.
    pub fn set_light_mode(&mut self, light: bool) {
        if theme::is_light() != light {
            theme::set_light(light);
            // Card and text colours are baked into the scene's instances.
            self.scene.invalidate();
        }
        self.needs_redraw = true;
    }

    pub fn light_mode(&self) -> bool {
        theme::is_light()
    }

    /// Dots on the board.
    pub fn set_grid(&mut self, on: bool) {
        self.show_grid = on;
        self.needs_redraw = true;
    }

    pub fn grid(&self) -> bool {
        self.show_grid
    }

    /// Dots in board space on a grid whose spacing doubles or halves with
    /// zoom so they stay 16–64 px apart, each about 2 px wide on screen.
    pub(crate) fn draw_grid(&self, renderer: &mut Renderer) {
        let dots = grid_dots(self.camera.center, self.camera.zoom, self.viewport);
        if !dots.is_empty() {
            let view = ViewUniform::new(self.camera.center, self.camera.zoom, self.viewport);
            renderer.draw_boxes(&view, &dots);
        }
    }
}

/// The grid's dots for a view, as rounded boxes.
pub fn grid_dots(center: [f32; 2], zoom: f32, viewport: [f32; 2]) -> Vec<BoxInstance> {
    let s = theme::grid_spacing(zoom);
    let (hw, hh) = (viewport[0] / 2.0 / zoom, viewport[1] / 2.0 / zoom);
    let (x0, x1) = (((center[0] - hw) / s).floor() as i64, ((center[0] + hw) / s).ceil() as i64);
    let (y0, y1) = (((center[1] - hh) / s).floor() as i64, ((center[1] + hh) / s).ceil() as i64);
    let count = ((x1 - x0 + 1) * (y1 - y0 + 1)).max(0) as usize;
    if count > MAX_DOTS {
        return Vec::new();
    }
    // A touch smaller where dots are dense, so the grid stays quiet.
    let px = 1.0 / zoom;
    let r = if s * zoom < 24.0 { 0.9 * px } else { 1.1 * px };
    let colour = theme::t().grid_dot;
    let mut out = Vec::with_capacity(count);
    for gy in y0..=y1 {
        for gx in x0..=x1 {
            let (x, y) = (gx as f32 * s, gy as f32 * s);
            out.push(BoxInstance { rect: [x - r, y - r, 2.0 * r, 2.0 * r], fill: colour, stroke: [0.0; 4], shape: [r, 0.0] });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_cover_the_view_at_any_zoom_without_exploding() {
        for zoom in [0.03f32, 0.25, 1.0, 4.0, 40.0] {
            let dots = grid_dots([1234.0, -567.0], zoom, [1920.0, 1080.0]);
            assert!(!dots.is_empty() && dots.len() < 20_000, "zoom {zoom}: {} dots", dots.len());
            // Every dot is on a grid line and about 2 px wide on screen.
            let s = theme::grid_spacing(zoom);
            let d = dots[0];
            let cx = d.rect[0] + d.rect[2] / 2.0;
            assert!(((cx / s) - (cx / s).round()).abs() < 1e-3);
            assert!((d.rect[2] * zoom - 2.0).abs() < 0.5);
        }
    }
}
