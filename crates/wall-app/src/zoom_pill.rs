//! The zoom pill at the bottom right: "−  84%  +". The percentage fits the
//! board when clicked.

use crate::menu::Command;
use crate::panel::{inside, text_width, PanelBox, TextRun};
use crate::{scene, theme, App};
use wall_render::{Renderer, ViewUniform};
use wall_text::Font;

const H: f32 = 30.0;
const BTN: f32 = 28.0;
const MARGIN: f32 = 12.0;

#[derive(Default)]
pub(crate) struct Layout {
    pub rect: [f32; 4],
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], Command)>,
}

pub(crate) fn layout(font: &mut Font, zoom: f32, hover: [f32; 2], viewport: [f32; 2]) -> Layout {
    let t = theme::t();
    let label = format!("{}%", (zoom * 100.0).round() as i64);
    // Room for "1000%" so the pill doesn't change size while zooming.
    let mid = text_width(font, "1000%", 12.5) + 12.0;
    let w = 2.0 * BTN + mid + 4.0;
    let r = [viewport[0] - MARGIN - w, viewport[1] - MARGIN - H, w, H];
    let mut l = Layout { rect: r, ..Default::default() };
    l.boxes.push(theme::shadow(r, theme::radius::PANEL));
    l.boxes.push(PanelBox { rect: r, fill: t.panel_bg, stroke: t.line, radius: theme::radius::PANEL, stroke_w: 1.0 });
    let parts = [
        ([r[0] + 2.0, r[1] + 1.0, BTN, H - 2.0], Command::ZoomOut, "−".to_owned()),
        ([r[0] + 2.0 + BTN, r[1] + 1.0, mid, H - 2.0], Command::FitBoard, label),
        ([r[0] + 2.0 + BTN + mid, r[1] + 1.0, BTN, H - 2.0], Command::ZoomIn, "+".to_owned()),
    ];
    for (k, (b, cmd, text)) in parts.into_iter().enumerate() {
        let over = inside(b, hover);
        if over {
            let inner = [b[0] + 2.0, b[1] + 2.0, b[2] - 4.0, b[3] - 4.0];
            l.boxes.push(PanelBox { rect: inner, fill: t.hover, stroke: [0.0; 4], radius: theme::radius::INNER, stroke_w: 0.0 });
        }
        let size = if k == 1 { 12.5 } else { 15.0 };
        let tw = text_width(font, &text, size);
        let ink = if k == 1 || over { t.ink } else { t.dim };
        l.texts.push(TextRun { text, at: [b[0] + (b[2] - tw) / 2.0, b[1] + b[3] / 2.0 + size * 0.36], size, color: ink });
        l.hits.push((b, cmd));
    }
    l
}

impl App {
    fn zoom_pill_layout(&mut self) -> Layout {
        layout(&mut self.font, self.camera.zoom, self.cursor, self.viewport)
    }

    pub(crate) fn zoom_pill_rect(&mut self) -> [f32; 4] {
        self.zoom_pill_layout().rect
    }

    /// A press on the zoom pill. True when it was used.
    pub(crate) fn zoom_pill_press(&mut self, pos: [f32; 2]) -> bool {
        let l = self.zoom_pill_layout();
        if !inside(l.rect, pos) {
            return false;
        }
        if let Some((_, cmd)) = l.hits.iter().find(|(r, _)| inside(*r, pos)) {
            self.run_command(*cmd);
        }
        true
    }

    pub(crate) fn draw_zoom_pill(&mut self, renderer: &mut Renderer) {
        let l = self.zoom_pill_layout();
        let view = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let boxes: Vec<wall_render::BoxInstance> = l.boxes.iter().map(|b| wall_render::BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] }).collect();
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
    use super::*;

    #[test]
    fn pill_sits_bottom_right_and_its_parts_click() {
        let mut f = Font::new(wall_text::INTER_REGULAR).unwrap();
        let l = layout(&mut f, 0.84, [0.0, 0.0], [1200.0, 800.0]);
        assert!(l.rect[0] + l.rect[2] <= 1200.0 - MARGIN + 0.01 && l.rect[1] + l.rect[3] <= 800.0 - MARGIN + 0.01);
        let cmds: Vec<Command> = l.hits.iter().map(|h| h.1).collect();
        assert_eq!(cmds, vec![Command::ZoomOut, Command::FitBoard, Command::ZoomIn]);
        assert!(l.texts.iter().any(|t| t.text == "84%"));
    }
}
