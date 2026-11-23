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
    /// While on, drags on the board draw pen lines (stays on until Esc).
    Pen,
    Board,
    /// A web link card.
    Link,
    /// A colour palette card (from the selected pictures' colours).
    Palette,
    /// While on, a click on the board starts a comment there (C).
    Comment,
    /// While on, clicks on the board put down stamps (E).
    Stamp,
    /// Pictures or videos from disk.
    Import,
}

impl Tool {
    /// Tools that make something when clicked or dragged onto the board.
    pub fn makes_item(self) -> bool {
        matches!(self, Tool::Note | Tool::Text | Tool::Todo | Tool::Group | Tool::Board | Tool::Link | Tool::Palette)
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
    Some((Tool::Pen, "Draw  ·  D, Esc to stop")),
    Some((Tool::Board, "Board  ·  B")),
    Some((Tool::Link, "Link  ·  or paste an address")),
    Some((Tool::Palette, "Palette  ·  from the selected pictures")),
    None,
    Some((Tool::Comment, "Comment  ·  C")),
    Some((Tool::Stamp, "Stamp a reaction  ·  E")),
    None,
    Some((Tool::Import, "Pictures or videos…")),
];

const W: f32 = 44.0;
const BUTTON: f32 = 36.0;
const STEP: f32 = 40.0;
const GAP: f32 = 9.0;
/// The dotted handle the rail is dragged by.
const GRIP: f32 = 16.0;
/// How close to the top or bottom edge the rail turns horizontal.
const EDGE: f32 = 64.0;

/// Where the rail floats, which way it runs, and whether it's folded away
/// to its handle and one button. Shells keep it between runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Top-left corner on screen.
    pub at: [f32; 2],
    pub horizontal: bool,
    pub collapsed: bool,
}

impl Default for Placement {
    fn default() -> Self {
        Placement { at: [10.0, BAR + 10.0], horizontal: false, collapsed: false }
    }
}

impl Placement {
    /// "x,y,h,c" for settings files.
    pub fn to_setting(&self) -> String {
        format!("{:.0},{:.0},{},{}", self.at[0], self.at[1], self.horizontal as u8, self.collapsed as u8)
    }

    pub fn from_setting(s: &str) -> Option<Self> {
        let v: Vec<&str> = s.split(',').collect();
        let [x, y, h, c] = v[..] else { return None };
        Some(Placement { at: [x.parse().ok()?, y.parse().ok()?], horizontal: h == "1", collapsed: c == "1" })
    }

    /// The placement with its handle dragged to `pointer`. Near the top or
    /// bottom edge it turns horizontal, elsewhere vertical.
    pub fn dragged(&self, pointer: [f32; 2], viewport: [f32; 2]) -> Self {
        let horizontal = pointer[1] < BAR + EDGE || pointer[1] > viewport[1] - EDGE;
        // The middle of the handle, from the rail's corner.
        let grab = if horizontal { [2.0 + GRIP / 2.0, W / 2.0] } else { [W / 2.0, 2.0 + GRIP / 2.0] };
        Placement { at: [pointer[0] - grab[0], pointer[1] - grab[1]], horizontal, collapsed: self.collapsed }
    }
}

#[derive(Default)]
pub struct Layout {
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], Tool)>,
    /// The whole rail; clicks inside it never reach the board.
    pub rect: [f32; 4],
    /// The handle to drag it by.
    pub grip: [f32; 4],
    /// The button that folds it away (or opens it again).
    pub fold: [f32; 4],
    /// The button that opens the boards panel on the left.
    pub side: [f32; 4],
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

/// Length of the rail along its run.
fn length(collapsed: bool) -> f32 {
    let tools = if collapsed { 0.0 } else { TOOLS.iter().map(|t| if t.is_some() { STEP } else { GAP }).sum::<f32>() + GAP };
    GRIP + STEP + tools + STEP + 4.0
}

/// The rail's rectangle on screen: where it was put, kept inside the
/// window and clear of `left_inset` (an open side panel on the left).
pub fn rect(place: &Placement, viewport: [f32; 2], left_inset: f32) -> [f32; 4] {
    let len = length(place.collapsed);
    let (w, h) = if place.horizontal { (len, W) } else { (W, len) };
    let min_x = left_inset + 10.0;
    let x = place.at[0].min(viewport[0] - w - 10.0).max(min_x);
    let y = place.at[1].min(viewport[1] - h - 10.0).max(BAR + 6.0);
    [x, y, w, h]
}

/// The rail with `active` highlighted, a tooltip for the tool under
/// `hover`, and `dragging` (a tool being dragged out) drawn at the pointer.
/// `text_width` measures a tooltip's text at 12 px.
pub fn layout(
    active: Tool,
    side_open: bool,
    hover: [f32; 2],
    dragging: Option<Tool>,
    place: &Placement,
    viewport: [f32; 2],
    left_inset: f32,
    text_width: &mut dyn FnMut(&str) -> f32,
) -> Layout {
    let t = theme::t();
    let mut l = Layout::default();
    l.rect = rect(place, viewport, left_inset);
    l.boxes.push(theme::shadow(l.rect, theme::radius::PANEL));
    l.rect(l.rect, t.panel_bg, t.line, theme::radius::PANEL);
    let [x0, y0, _, _] = l.rect;
    // Position along the run -> a rectangle across it.
    let horiz = place.horizontal;
    let cell = |pos: f32, len: f32, across: f32| if horiz { [x0 + pos, y0 + (W - across) / 2.0, len, across] } else { [x0 + (W - across) / 2.0, y0 + pos, across, len] };
    let mut pos = 2.0;
    // The handle: six dots.
    l.grip = cell(pos, GRIP, W - 8.0);
    let gc = [l.grip[0] + l.grip[2] / 2.0, l.grip[1] + l.grip[3] / 2.0];
    for i in 0..3 {
        for j in 0..2 {
            let (a, b) = ((i as f32 - 1.0) * 5.0, (j as f32 - 0.5) * 5.0);
            let (dx, dy) = if horiz { (b, a) } else { (a, b) };
            l.fill([gc[0] + dx - 1.2, gc[1] + dy - 1.2, 2.4, 2.4], t.dim, 1.2);
        }
    }
    pos += GRIP;
    let mut tip = None;
    // The boards panel: a frame with a bar down its left side.
    let r = cell(pos + 2.0, BUTTON, BUTTON);
    l.side = r;
    let over = inside(r, hover);
    if side_open {
        l.fill(r, t.accent_soft, theme::radius::CONTROL);
    } else if over {
        l.fill(r, t.hover, theme::radius::CONTROL);
    }
    let ink = if side_open { t.accent_text } else if over { t.ink } else { t.dim };
    let c = [r[0] + BUTTON / 2.0, r[1] + BUTTON / 2.0];
    l.outline([c[0] - 9.0, c[1] - 7.0, 18.0, 14.0], ink, 2.5);
    l.fill([c[0] - 9.0, c[1] - 7.0, 6.0, 14.0], ink, 1.5);
    if over {
        tip = Some(("Boards and library  ·  S".to_string(), r));
    }
    pos += STEP;
    if !place.collapsed {
        for entry in TOOLS {
            let Some((tool, label)) = entry else {
                let r = cell(pos + GAP / 2.0 - 0.5, 1.0, W - 20.0);
                l.fill(r, t.line, 0.0);
                pos += GAP;
                continue;
            };
            let r = cell(pos + 2.0, BUTTON, BUTTON);
            let on = *tool == active;
            let over = inside(r, hover);
            if on {
                l.fill(r, t.accent_soft, theme::radius::CONTROL);
            } else if over {
                l.fill(r, t.hover, theme::radius::CONTROL);
            }
            let ink = if on { t.accent_text } else if over { t.ink } else { t.dim };
            icon(&mut l, *tool, [r[0] + BUTTON / 2.0, r[1] + BUTTON / 2.0], ink);
            l.hits.push((r, *tool));
            if over && dragging.is_none() {
                tip = Some((label.to_string(), r));
            }
            pos += STEP;
        }
        l.fill(cell(pos + GAP / 2.0 - 0.5, 1.0, W - 20.0), t.line, 0.0);
        pos += GAP;
    }
    // Fold away, or open again: the active tool's icon with a corner mark.
    let r = cell(pos + 2.0, BUTTON, BUTTON);
    l.fold = r;
    let over = inside(r, hover);
    if over {
        l.fill(r, t.hover, theme::radius::CONTROL);
    }
    let c = [r[0] + BUTTON / 2.0, r[1] + BUTTON / 2.0];
    if place.collapsed {
        icon(&mut l, active, c, t.accent_text);
        l.fill([r[0] + BUTTON - 9.0, r[1] + BUTTON - 9.0, 5.0, 5.0], t.dim, 2.5);
        if over {
            tip = Some(("Show tools".into(), r));
        }
    } else {
        // A short bar: "fold away".
        let (w, h) = if horiz { (2.0, 14.0) } else { (14.0, 2.0) };
        l.fill([c[0] - w / 2.0, c[1] - h / 2.0, w, h], t.dim, 1.0);
        if over {
            tip = Some(("Fold the tools away".into(), r));
        }
    }
    if inside(l.grip, hover) && dragging.is_none() {
        tip = Some(("Drag to move".into(), l.grip));
    }
    if let Some((label, r)) = tip {
        let w = text_width(&label) + 20.0;
        let b = if horiz {
            // Below the rail, or above it near the bottom of the window.
            let y = if l.rect[1] + W + 40.0 > viewport[1] { l.rect[1] - 32.0 } else { l.rect[1] + W + 8.0 };
            [(r[0] + r[2] / 2.0 - w / 2.0).max(4.0), y, w, 24.0]
        } else {
            let x = if l.rect[0] + W + 8.0 + w > viewport[0] { l.rect[0] - 8.0 - w } else { l.rect[0] + W + 8.0 };
            [x, r[1] + r[3] / 2.0 - 12.0, w, 24.0]
        };
        l.boxes.push(theme::shadow(b, theme::radius::INNER));
        l.rect(b, t.menu_drop, t.line, theme::radius::INNER);
        l.texts.push(TextRun { text: label, at: [b[0] + 10.0, b[1] + 16.0], size: 12.0, color: t.ink });
    }
    if let Some(tool) = dragging.filter(|_| !inside(l.rect, hover)) {
        // A ghost of the card under the pointer while dragging it out.
        let [w, h] = ghost_size(tool);
        let g = [hover[0] - 20.0, hover[1] - 20.0, w, h];
        let mut fill = t.card_fill;
        fill[3] = 0.75;
        l.rect(g, fill, t.accent, crate::theme::radius::CONTROL);
        icon(&mut l, tool, [g[0] + 22.0, g[1] + 20.0], t.dim);
    }
    l
}

/// Rough on-screen size of what a tool makes, for the drag ghost.
fn ghost_size(tool: Tool) -> [f32; 2] {
    match tool {
        Tool::Group => [240.0, 160.0],
        Tool::Board => [220.0, 140.0],
        Tool::Palette => [260.0, 110.0],
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
        Tool::Stamp => glyph(l, "★", c, 17.0, ink),
        Tool::Comment => {
            // A speech bubble.
            l.outline([x - 9.0, y - 8.0, 18.0, 13.0], ink, 6.0);
            l.fill([x - 6.0, y + 3.0, 4.0, 5.0], ink, 0.5);
        }
        Tool::Pen => {
            // A slanted line of dots with a heavier tip.
            for k in 0..5 {
                let o = k as f32 * 3.2;
                l.fill([x + 6.0 - o - 1.2, y - 7.0 + o - 1.2, 2.6, 2.6], ink, 1.3);
            }
            l.fill([x - 9.0, y + 6.0, 3.0, 3.0], ink, 0.5);
        }
        Tool::Palette => {
            for k in 0..3 {
                let a = [1.0, 0.75, 0.5][k];
                l.fill([x - 9.0 + k as f32 * 6.5, y - 7.0, 5.0, 14.0], [ink[0], ink[1], ink[2], ink[3] * a], 1.0);
            }
        }
        Tool::Link => {
            // Two linked rings.
            l.outline([x - 9.0, y - 4.0, 11.0, 8.0], ink, 4.0);
            l.outline([x - 2.0, y - 4.0, 11.0, 8.0], ink, 4.0);
        }
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
        let vp = [1200.0, 900.0];
        let l = layout(Tool::Select, false, [0.0, 0.0], None, &Placement::default(), vp, 0.0, &mut |s| s.len() as f32 * 6.0);
        assert_eq!(l.hits.len(), TOOLS.iter().flatten().count());
        let (r, _) = l.hits[2];
        assert_eq!(l.tool_at([r[0] + 5.0, r[1] + 5.0]), Some(l.hits[2].1));
        assert!(l.contains([11.0, BAR + 12.0]));
        assert!(!l.contains([10.0 + W + 2.0, BAR + 12.0]));
        assert!(l.contains([l.fold[0] + 2.0, l.fold[1] + 2.0]), "the fold button is on the rail");
    }

    #[test]
    fn rail_floats_turns_horizontal_near_edges_folds_and_keeps_clear() {
        let vp = [1200.0, 900.0];
        let p = Placement::default();
        let moved = p.dragged([600.0, 400.0], vp);
        assert_eq!((moved.at, moved.horizontal), ([578.0, 390.0], false));
        let bottom = moved.dragged([600.0, 880.0], vp);
        assert!(bottom.horizontal);
        let r = rect(&bottom, vp, 0.0);
        assert!(r[2] > r[3] && r[1] + r[3] <= vp[1] - 10.0, "horizontal and inside the window: {r:?}");
        // Folded: just the handle, the boards button and the unfold button.
        let folded = Placement { collapsed: true, ..p };
        let l = layout(Tool::Pen, false, [0.0; 2], None, &folded, vp, 0.0, &mut |s| s.len() as f32 * 6.0);
        assert!(l.hits.is_empty() && l.rect[3] < 110.0);
        // An open side panel on the left pushes it over.
        assert_eq!(rect(&p, vp, 260.0)[0], 270.0);
        assert_eq!(Placement::from_setting(&bottom.to_setting()).unwrap().horizontal, true);
        assert_eq!(Placement::from_setting("junk"), None);
    }
}
