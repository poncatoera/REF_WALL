//! The tool rail down the left of the board: pick the arrow tool, or click
//! or drag a note, text, to-do list, group, board or picture onto the board.

use crate::menu::BAR;
use crate::panel::{inside, PanelBox, TextRun};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Pick, move and resize (the normal pointer).
    Select,
    Note,
    Text,
    Todo,
    Group,
    /// While on, a drag on the board draws an arrow (as Alt-drag does).
    Arrow,
    Board,
    /// Pictures or videos from disk.
    Import,
}

impl Tool {
    /// Tools that make something when clicked or dragged onto the board.
    pub fn makes_item(self) -> bool {
        matches!(self, Tool::Note | Tool::Text | Tool::Todo | Tool::Group | Tool::Board)
    }
}

/// The rail's tools with their tooltips, top to bottom. `None` is a gap.
pub const TOOLS: &[Option<(Tool, &str)>] = &[
    Some((Tool::Select, "Select  ·  Esc")),
    None,
    Some((Tool::Note, "Note  ·  double-click the board")),
    Some((Tool::Text, "Text")),
    Some((Tool::Todo, "To-do list  ·  T")),
    Some((Tool::Group, "Group")),
    Some((Tool::Arrow, "Arrow  ·  or Alt-drag")),
    Some((Tool::Board, "Board  ·  B")),
    None,
    Some((Tool::Import, "Pictures or videos…")),
];

const X: f32 = 10.0;
const W: f32 = 44.0;
const BUTTON: f32 = 36.0;
const STEP: f32 = 40.0;
const GAP: f32 = 9.0;

#[derive(Default)]
pub struct Layout {
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], Tool)>,
    /// The whole rail; clicks inside it never reach the board.
    pub rect: [f32; 4],
}

impl Layout {
    pub fn tool_at(&self, p: [f32; 2]) -> Option<Tool> {
        self.hits.iter().find(|(r, _)| inside(*r, p)).map(|(_, t)| *t)
    }

    pub fn contains(&self, p: [f32; 2]) -> bool {
        inside(self.rect, p)
    }

    fn rect(&mut self, rect: [f32; 4], fill: [f32; 4], stroke: [f32; 4], radius: f32) {
        self.boxes.push(PanelBox { rect, fill, stroke, radius, stroke_w: if stroke[3] > 0.0 { 1.4 } else { 0.0 } });
    }

    fn fill(&mut self, rect: [f32; 4], color: [f32; 4], radius: f32) {
        self.rect(rect, color, [0.0; 4], radius);
    }

    fn outline(&mut self, rect: [f32; 4], color: [f32; 4], radius: f32) {
        self.rect(rect, [0.0; 4], color, radius);
    }
}

/// Height of the rail.
fn height() -> f32 {
    TOOLS.iter().map(|t| if t.is_some() { STEP } else { GAP }).sum::<f32>() + 4.0
}

/// The rail with `active` highlighted, a tooltip for the tool under
/// `hover`, and `dragging` (a tool being dragged out) drawn at the pointer.
/// `text_width` measures a tooltip's text at 12 px.
pub fn layout(active: Tool, hover: [f32; 2], dragging: Option<Tool>, text_width: &mut dyn FnMut(&str) -> f32) -> Layout {
    let t = theme::t();
    let mut l = Layout::default();
    let top = BAR + 10.0;
    l.rect = [X, top, W, height()];
    l.rect(l.rect, t.panel_bg, t.line, 10.0);
    let mut y = top + 4.0;
    let mut tip = None;
    for entry in TOOLS {
        let Some((tool, label)) = entry else {
            l.fill([X + 10.0, y + GAP / 2.0 - 0.5, W - 20.0, 1.0], t.line, 0.0);
            y += GAP;
            continue;
        };
        let r = [X + 4.0, y + 2.0, BUTTON, BUTTON];
        let on = *tool == active;
        let over = inside(r, hover);
        if on {
            l.fill(r, t.chip, 7.0);
        } else if over {
            l.fill(r, t.field, 7.0);
        }
        let ink = if on { t.accent } else { t.ink };
        icon(&mut l, *tool, [r[0] + BUTTON / 2.0, r[1] + BUTTON / 2.0], ink);
        l.hits.push((r, *tool));
        if over && dragging.is_none() {
            tip = Some((*label, r));
        }
        y += STEP;
    }
    if let Some((label, r)) = tip {
        let w = text_width(label) + 20.0;
        let b = [X + W + 8.0, r[1] + 6.0, w, 24.0];
        l.rect(b, t.menu_drop, t.line, 6.0);
        l.texts.push(TextRun { text: label.into(), at: [b[0] + 10.0, b[1] + 16.0], size: 12.0, color: t.ink });
    }
    if let Some(tool) = dragging.filter(|_| !inside(l.rect, hover)) {
        // A ghost of the card under the pointer while dragging it out.
        let [w, h] = ghost_size(tool);
        let g = [hover[0] - 20.0, hover[1] - 20.0, w, h];
        let mut fill = t.card_fill;
        fill[3] = 0.75;
        l.rect(g, fill, t.accent, 8.0);
        icon(&mut l, tool, [g[0] + 22.0, g[1] + 20.0], t.dim);
    }
    l
}

/// Rough on-screen size of what a tool makes, for the drag ghost.
fn ghost_size(tool: Tool) -> [f32; 2] {
    match tool {
        Tool::Group => [240.0, 160.0],
        Tool::Board => [220.0, 140.0],
        Tool::Text => [160.0, 40.0],
        _ => [240.0, 72.0],
    }
}

/// Draws a tool's icon centred on `c`, from boxes and glyphs.
fn icon(l: &mut Layout, tool: Tool, c: [f32; 2], ink: [f32; 4]) {
    let [x, y] = c;
    match tool {
        Tool::Select => glyph(l, "↖", c, 19.0, ink),
        Tool::Arrow => glyph(l, "↗", c, 19.0, ink),
        Tool::Text => glyph(l, "T", c, 19.0, ink),
        Tool::Note => {
            l.outline([x - 7.0, y - 8.0, 14.0, 16.0], ink, 2.0);
            l.fill([x - 3.5, y - 3.0, 7.0, 1.4], ink, 0.0);
            l.fill([x - 3.5, y + 0.5, 7.0, 1.4], ink, 0.0);
        }
        Tool::Todo => {
            for k in 0..3 {
                let ry = y - 7.0 + k as f32 * 5.5;
                l.outline([x - 8.0, ry, 3.5, 3.5], ink, 0.8);
                l.fill([x - 2.0, ry + 1.1, 10.0, 1.4], ink, 0.0);
            }
        }
        Tool::Group => {
            l.outline([x - 9.0, y - 7.0, 18.0, 14.0], ink, 2.5);
            l.fill([x - 9.0, y - 7.0, 18.0, 4.0], ink, 1.5);
        }
        Tool::Board => {
            l.outline([x - 9.0, y - 7.0, 18.0, 14.0], ink, 2.5);
            l.outline([x - 4.0, y - 2.5, 8.0, 5.0], ink, 1.0);
        }
        Tool::Import => {
            l.outline([x - 9.0, y - 7.0, 18.0, 14.0], ink, 2.5);
            l.fill([x + 2.0, y - 4.0, 4.0, 4.0], ink, 2.0);
            l.fill([x - 6.0, y + 2.0, 12.0, 2.0], ink, 0.5);
        }
    }
}

/// One glyph roughly centred on `c` (cap height is about 0.7 em).
fn glyph(l: &mut Layout, s: &str, c: [f32; 2], size: f32, ink: [f32; 4]) {
    l.texts.push(TextRun { text: s.into(), at: [c[0] - size * 0.32, c[1] + size * 0.35], size, color: ink });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_one_button_and_gaps_dont() {
        let l = layout(Tool::Select, [0.0, 0.0], None, &mut |s| s.len() as f32 * 6.0);
        assert_eq!(l.hits.len(), TOOLS.iter().flatten().count());
        let (r, _) = l.hits[2];
        assert_eq!(l.tool_at([r[0] + 5.0, r[1] + 5.0]), Some(l.hits[2].1));
        assert!(l.contains([X + 1.0, BAR + 12.0]));
        assert!(!l.contains([X + W + 2.0, BAR + 12.0]));
    }
}
