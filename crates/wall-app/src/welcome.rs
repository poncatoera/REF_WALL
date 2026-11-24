//! The first-time walkthrough: a few welcome slides over a dimmed board,
//! then tips that point at the real controls (tool rail, boards button,
//! File menu, zoom). People opening a share link get one short card
//! instead. Skippable at any point; Help > Welcome tour plays it again.
//!
//! Shells remember that it was seen through [`App::ui_setting`].

use wall_render::{BoxInstance, Renderer, ViewUniform};

use crate::panel::{self, PanelBox, TextRun};
use crate::{scene, theme, App, Key};

/// Which walkthrough is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Slides, then tips on the board.
    Full,
    /// Someone who opened a share link: one card (`view`: they can't edit).
    Guest { view: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Welcome {
    pub kind: Kind,
    /// Slide (or tip, once `tips`) shown.
    pub step: usize,
    /// Past the slides: pointing at controls.
    pub tips: bool,
}

/// Bits of what was seen, kept in the settings.
pub const SEEN_FULL: u8 = 1;
pub const SEEN_GUEST: u8 = 2;

struct Slide {
    title: &'static str,
    body: &'static str,
    /// Said instead of `body` on the web (no desktop-only features).
    web_body: Option<&'static str>,
}

const SLIDES: [Slide; 4] = [
    Slide {
        title: "Welcome to Wall",
        body: "One endless board for your references: pictures, videos, notes and sketches, side by side. Zoom out to see everything, zoom in to study a detail.",
        web_body: None,
    },
    Slide {
        title: "Bring your references in",
        body: "Drop pictures or videos onto the board, or paste them. With the Wall extension, right-click any picture in your browser and choose Add to Wall.",
        web_body: Some("Drop pictures or videos onto the board, or paste them straight from your clipboard."),
    },
    Slide {
        title: "Arrange and study",
        body: "Press A to tidy a mess into rows. Select a video to study it: K plays, comma and full stop step a frame, I and O loop a part, M adds a marker.",
        web_body: None,
    },
    Slide {
        title: "Boards, and the people you share with",
        body: "Keep boards inside boards and find them all with S. Share sends a link: people open the board in their browser, and can comment and react.",
        web_body: Some("Keep boards inside boards and find them all with S. Everything stays in this browser until you share it."),
    },
];

/// What a tip points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Rail,
    BoardsButton,
    FileMenu,
    Zoom,
}

struct Tip {
    target: Target,
    title: &'static str,
    body: &'static str,
}

const TIPS: [Tip; 4] = [
    Tip { target: Target::Rail, title: "Your tools", body: "Notes, text, to-dos, arrows and drawing. Click one, or drag it onto the board. Drag the dots at the top to move this bar." },
    Tip { target: Target::BoardsButton, title: "All your boards", body: "Opens the panel with every board and your library. S does the same." },
    Tip { target: Target::FileMenu, title: "File", body: "Import, share this board, and go back to an earlier version." },
    Tip { target: Target::Zoom, title: "Zoom", body: "Scroll to zoom and drag empty board to move around. F fits the whole board on screen." },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Next,
    Back,
    Skip,
    /// One of the dots: go to that slide.
    Dot(usize),
    /// The card itself (takes the click, does nothing).
    Card,
}

#[derive(Default)]
pub struct Layout {
    /// The card (clicks here never reach the board).
    pub card: [f32; 4],
    /// True when the whole window is covered (slides): every click is ours.
    pub modal: bool,
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], Hit)>,
}

impl Layout {
    fn bx(&mut self, rect: [f32; 4], fill: [f32; 4], stroke: [f32; 4], radius: f32, stroke_w: f32) {
        self.boxes.push(PanelBox { rect, fill, stroke, radius, stroke_w });
    }
    fn fill(&mut self, rect: [f32; 4], fill: [f32; 4], radius: f32) {
        self.bx(rect, fill, [0.0; 4], radius, 0.0);
    }
    fn text(&mut self, text: impl Into<String>, at: [f32; 2], size: f32, color: [f32; 4]) {
        self.texts.push(TextRun { text: text.into(), at, size, color });
    }
    pub fn hit_at(&self, p: [f32; 2]) -> Option<Hit> {
        self.hits.iter().rev().find(|(r, _)| panel::inside(*r, p)).map(|(_, h)| *h)
    }
}

type Tw<'a> = &'a mut dyn FnMut(&str, f32) -> f32;

/// Muted picture colours for the drawings on the slides.
const SWATCH: [[f32; 3]; 6] = [[0.86, 0.42, 0.33], [0.93, 0.68, 0.33], [0.42, 0.66, 0.50], [0.33, 0.58, 0.74], [0.50, 0.47, 0.82], [0.80, 0.45, 0.62]];

fn swatch(i: usize) -> [f32; 4] {
    let c = SWATCH[i % SWATCH.len()];
    let k = if theme::is_light() { 0.92 } else { 0.78 };
    [c[0] * k, c[1] * k, c[2] * k, 1.0]
}

fn button(l: &mut Layout, r: [f32; 4], label: &str, primary: bool, hover: [f32; 2], hit: Hit, tw: Tw) {
    let t = theme::t();
    let over = panel::inside(r, hover);
    let (fill, stroke, ink) = if primary { (t.accent, [0.0; 4], t.accent_ink) } else { (if over { t.hover } else { [0.0; 4] }, t.line, t.ink) };
    l.bx(r, fill, stroke, theme::radius::CONTROL, if primary { 0.0 } else { 1.0 });
    let w = tw(label, 13.0);
    l.text(label, [r[0] + (r[2] - w) / 2.0, r[1] + r[3] / 2.0 + 4.5], 13.0, ink);
    l.hits.push((r, hit));
}

/// A key cap: "K", "S", "Ctrl V".
fn key_cap(l: &mut Layout, at: [f32; 2], label: &str, tw: Tw) -> f32 {
    let t = theme::t();
    let w = tw(label, 11.5).max(9.0) + 12.0;
    l.bx([at[0], at[1], w, 20.0], t.chip, t.line, theme::radius::CONTROL, 1.0);
    l.text(label, [at[0] + 6.0, at[1] + 14.0], 11.5, t.ink);
    w
}

/// A picture on a drawing: a coloured block with a soft sky band, so it
/// reads as an image rather than a button.
fn picture(l: &mut Layout, r: [f32; 4], i: usize) {
    let c = swatch(i);
    l.fill(r, c, 2.0);
    let light = [c[0] + (1.0 - c[0]) * 0.25, c[1] + (1.0 - c[1]) * 0.25, c[2] + (1.0 - c[2]) * 0.25, 1.0];
    l.fill([r[0], r[1], r[2], r[3] * 0.45], light, 2.0);
    // A sun / subject.
    let d = r[3].min(r[2]) * 0.28;
    l.fill([r[0] + r[2] * 0.66, r[1] + r[3] * 0.16, d, d], [0.5, 0.5, 0.5, 0.5], d);
}

/// A note card with a few lines of "text".
fn note(l: &mut Layout, r: [f32; 4], lines: usize) {
    let t = theme::t();
    l.bx(r, t.card_fill, t.card_stroke, 4.0, 1.0);
    for k in 0..lines {
        let w = if k + 1 == lines { r[2] * 0.45 } else { r[2] - 20.0 };
        l.fill([r[0] + 10.0, r[1] + 12.0 + k as f32 * 10.0, w, 4.0], t.text_done, 2.0);
    }
}

/// The drawing at the top of slide `i`, inside `r`.
fn drawing(l: &mut Layout, i: usize, r: [f32; 4], web: bool, tw: Tw) {
    let t = theme::t();
    l.fill(r, t.background, theme::radius::PANEL);
    // Grid dots, like the board.
    let mut y = r[1] + 14.0;
    while y < r[1] + r[3] - 6.0 {
        let mut x = r[0] + 14.0;
        while x < r[0] + r[2] - 6.0 {
            l.fill([x, y, 2.0, 2.0], t.grid_dot, 1.0);
            x += 24.0;
        }
        y += 24.0;
    }
    let (x, y, w, h) = (r[0], r[1], r[2], r[3]);
    match i {
        0 => {
            // A moodboard: pictures of many sizes, a note, a group.
            l.bx([x + 300.0, y + 26.0, 220.0, 150.0], [0.0; 4], t.accent, 6.0, 1.0);
            l.text("Lighting refs", [x + 308.0, y + 22.0], 11.0, t.accent);
            picture(l, [x + 30.0, y + 30.0, 120.0, 80.0], 0);
            picture(l, [x + 160.0, y + 30.0, 70.0, 100.0], 1);
            picture(l, [x + 30.0, y + 120.0, 88.0, 62.0], 3);
            picture(l, [x + 128.0, y + 140.0, 100.0, 66.0], 4);
            picture(l, [x + 312.0, y + 40.0, 92.0, 64.0], 2);
            picture(l, [x + 414.0, y + 40.0, 94.0, 64.0], 5);
            picture(l, [x + 312.0, y + 114.0, 196.0, 52.0], 1);
            note(l, [x + 240.0, y + 188.0, 108.0, 46.0], 3);
            note(l, [x + 362.0, y + 196.0, 116.0, 38.0], 2);
        }
        1 => {
            // Pictures dropping onto the board.
            let zone = [x + w / 2.0 - 150.0, y + 60.0, 300.0, 150.0];
            l.bx(zone, t.accent_soft, t.accent, 8.0, 1.5);
            l.text("Drop here", [zone[0] + (zone[2] - tw("Drop here", 13.0)) / 2.0, zone[1] + zone[3] - 18.0], 13.0, t.accent_text);
            picture(l, [zone[0] + 40.0, zone[1] + 24.0, 90.0, 64.0], 3);
            picture(l, [zone[0] + 150.0, zone[1] + 18.0, 110.0, 76.0], 0);
            // One still on its way in, tilted a little by a lifted shadow.
            l.fill([x + w / 2.0 + 124.0, y + 22.0, 74.0, 52.0], t.shadow, 3.0);
            picture(l, [x + w / 2.0 + 118.0, y + 14.0, 74.0, 52.0], 4);
            let mut cx = x + 26.0;
            let row = y + h - 32.0;
            for (k, label) in [("Ctrl V", "paste")].iter().chain(if web { [].iter() } else { [("Right-click", "Add to Wall")].iter() }) {
                cx += key_cap(l, [cx, row], k, tw) + 6.0;
                l.text(*label, [cx, row + 14.0], 11.5, t.dim);
                cx += tw(label, 11.5) + 18.0;
            }
        }
        2 => {
            // Tidy rows on the left, a video being studied on the right.
            let mut px = x + 26.0;
            let widths = [56.0, 40.0, 70.0, 48.0, 62.0, 44.0];
            for (k, pw) in widths.iter().enumerate() {
                let (row, col_x) = if k < 3 { (0.0, px) } else { (54.0, px) };
                picture(l, [col_x, y + 34.0 + row, *pw, 46.0], k);
                px += pw + 6.0;
                if k == 2 {
                    px = x + 26.0;
                }
            }
            let mut cx = x + 26.0;
            let row = y + 150.0;
            cx += key_cap(l, [cx, row], "A", tw) + 6.0;
            l.text("tidy into rows", [cx, row + 14.0], 11.5, t.dim);
            // Video.
            let v = [x + 250.0, y + 26.0, 250.0, 140.0];
            picture(l, v, 3);
            l.fill([v[0] + v[2] / 2.0 - 16.0, v[1] + v[3] / 2.0 - 16.0, 32.0, 32.0], [0.0, 0.0, 0.0, 0.45], 16.0);
            l.fill([v[0] + v[2] / 2.0 - 4.0, v[1] + v[3] / 2.0 - 8.0, 4.0, 16.0], [1.0; 4], 1.0);
            l.fill([v[0] + v[2] / 2.0 + 3.0, v[1] + v[3] / 2.0 - 8.0, 4.0, 16.0], [1.0; 4], 1.0);
            // Scrub bar with an in/out range, markers and the playhead.
            let bar = [v[0], v[1] + v[3] + 10.0, v[2], 4.0];
            l.fill(bar, t.line, 2.0);
            l.fill([bar[0] + 60.0, bar[1], 110.0, 4.0], t.accent_soft, 2.0);
            l.fill([bar[0] + 60.0, bar[1] - 4.0, 2.0, 12.0], t.accent, 1.0);
            l.fill([bar[0] + 170.0, bar[1] - 4.0, 2.0, 12.0], t.accent, 1.0);
            for m in [92.0, 140.0] {
                l.fill([bar[0] + m - 3.0, bar[1] - 9.0, 6.0, 6.0], t.accent_text, 3.0);
            }
            l.fill([bar[0] + 118.0, bar[1] - 6.0, 3.0, 16.0], t.ink, 1.5);
            let mut cx = v[0];
            let row = bar[1] + 18.0;
            for k in ["K", ",", ".", "I", "O", "M"] {
                cx += key_cap(l, [cx, row], k, tw) + 5.0;
            }
        }
        _ => {
            // A board with boards inside, and people on it.
            let b = [x + 30.0, y + 30.0, 210.0, 150.0];
            l.bx(b, t.card_fill, t.card_stroke, 6.0, 1.0);
            l.text("Project", [b[0] + 12.0, b[1] + 22.0], 12.0, t.ink);
            for k in 0..3 {
                let c = [b[0] + 12.0 + k as f32 * 64.0, b[1] + 38.0, 56.0, 96.0];
                l.bx(c, t.panel_bg, t.line, 4.0, 1.0);
                picture(l, [c[0] + 6.0, c[1] + 6.0, 44.0, 30.0], k * 2);
                picture(l, [c[0] + 6.0, c[1] + 40.0, 44.0, 30.0], k * 2 + 1);
                l.fill([c[0] + 6.0, c[1] + 80.0, 30.0, 4.0], t.text_done, 2.0);
            }
            // A shared board with people's cursors, a comment and a stamp.
            let s = [x + 280.0, y + 30.0, 230.0, 150.0];
            l.bx(s, t.card_fill, t.card_stroke, 6.0, 1.0);
            picture(l, [s[0] + 14.0, s[1] + 14.0, 120.0, 84.0], 5);
            picture(l, [s[0] + 142.0, s[1] + 14.0, 74.0, 84.0], 2);
            let people = [("Mia", 3usize), ("Jon", 1), ("Paul", 4)];
            for (k, (name, c)) in people.iter().enumerate() {
                let col = crate::presence::pick(Some(*c as u8)).unwrap_or(t.accent);
                let cx = s[0] + 40.0 + k as f32 * 64.0;
                let cy = s[1] + 108.0 + (k % 2) as f32 * 12.0;
                l.fill([cx, cy, 4.0, 14.0], col, 2.0);
                let nw = tw(name, 10.5) + 10.0;
                l.fill([cx + 6.0, cy + 10.0, nw, 16.0], col, 8.0);
                l.text(*name, [cx + 11.0, cy + 22.0], 10.5, [1.0; 4]);
            }
            // Comment pin.
            l.fill([s[0] + 120.0, s[1] + 4.0, 22.0, 22.0], t.accent, 11.0);
            l.text("2", [s[0] + 127.5, s[1] + 19.5], 11.0, t.accent_ink);
            let mut cx = x + 30.0;
            let row = y + h - 32.0;
            cx += key_cap(l, [cx, row], "S", tw) + 6.0;
            l.text("all boards", [cx, row + 14.0], 11.5, t.dim);
            cx += tw("all boards", 11.5) + 18.0;
            cx += key_cap(l, [cx, row], "B", tw) + 6.0;
            l.text("new board", [cx, row + 14.0], 11.5, t.dim);
            if !web {
                cx += tw("new board", 11.5) + 18.0;
                cx += key_cap(l, [cx, row], "Share", tw) + 6.0;
                l.text("top right", [cx, row + 14.0], 11.5, t.dim);
            }
        }
    }
}

const SLIDE_W: f32 = 560.0;
const PAD: f32 = 24.0;
const DRAW_H: f32 = 250.0;

fn slides_layout(step: usize, web: bool, hover: [f32; 2], viewport: [f32; 2], tw: Tw) -> Layout {
    let t = theme::t();
    let mut l = Layout { modal: true, ..Default::default() };
    // Dim the board and the menu bar behind.
    let dim = if theme::is_light() { [0.10, 0.08, 0.05, 0.38] } else { [0.0, 0.0, 0.0, 0.55] };
    l.fill([0.0, 0.0, viewport[0], viewport[1]], [dim[0] * dim[3], dim[1] * dim[3], dim[2] * dim[3], dim[3]], 0.0);
    let w = SLIDE_W.min(viewport[0] - 16.0);
    let inner = w - 2.0 * PAD;
    let slide = &SLIDES[step.min(SLIDES.len() - 1)];
    let body = if web { slide.web_body.unwrap_or(slide.body) } else { slide.body };
    let lines = panel::wrap_with(tw, body, 14.0, inner);
    let h = PAD + DRAW_H + 30.0 + 24.0 + lines.len() as f32 * 21.0 + 22.0 + 34.0 + PAD;
    let x0 = ((viewport[0] - w) / 2.0).max(8.0);
    let y0 = ((viewport[1] - h) / 2.0).max(8.0);
    let card = [x0, y0, w, h];
    l.card = card;
    l.boxes.push(theme::shadow(card, theme::radius::PANEL));
    l.bx(card, t.panel_bg, t.line, theme::radius::PANEL + 4.0, 1.0);
    l.hits.push((card, Hit::Card));
    let draw = [x0 + PAD, y0 + PAD, inner, DRAW_H];
    // Narrow windows: the drawing is laid out for 512 px; skip it if smaller.
    let mut y = y0 + PAD;
    if inner >= 500.0 {
        drawing(&mut l, step, [draw[0] + (inner - 512.0) / 2.0, draw[1], 512.0, DRAW_H], web, tw);
        y += DRAW_H;
    } else {
        y += 0.0;
    }
    y += 30.0 + 14.0;
    l.text(slide.title, [x0 + PAD, y], 21.0, t.ink);
    y += 28.0;
    for line in lines {
        l.text(line, [x0 + PAD, y], 14.0, t.dim);
        y += 21.0;
    }
    // Footer: dots, Skip, Back, Next.
    let fy = y0 + h - PAD - 34.0;
    let n = SLIDES.len();
    for k in 0..n {
        let r = [x0 + PAD + k as f32 * 16.0, fy + 13.0, 8.0, 8.0];
        l.fill(r, if k == step { t.accent } else { t.line }, 4.0);
        l.hits.push(([r[0] - 4.0, r[1] - 8.0, 16.0, 24.0], Hit::Dot(k)));
    }
    let last = step + 1 >= n;
    let next_label = if last { "Show me around" } else { "Next" };
    let nw = tw(next_label, 13.0) + 36.0;
    let next = [x0 + w - PAD - nw, fy, nw, 34.0];
    button(&mut l, next, next_label, true, hover, Hit::Next, tw);
    let mut bx = next[0];
    if step > 0 {
        let back = [next[0] - 10.0 - 76.0, fy, 76.0, 34.0];
        button(&mut l, back, "Back", false, hover, Hit::Back, tw);
        bx = back[0];
    }
    let skip = "Skip";
    let sw = tw(skip, 13.0);
    let sr = [bx - 18.0 - sw - 12.0, fy, sw + 24.0, 34.0];
    let over = panel::inside(sr, hover);
    l.text(skip, [sr[0] + 12.0, fy + 21.5], 13.0, if over { t.ink } else { t.dim });
    l.hits.push((sr, Hit::Skip));
    l
}

/// The card for someone who opened a share link.
fn guest_layout(view: bool, name: &str, hover: [f32; 2], viewport: [f32; 2], tw: Tw) -> Layout {
    let t = theme::t();
    let mut l = Layout { modal: false, ..Default::default() };
    let w = 380.0f32.min(viewport[0] - 16.0);
    let inner = w - 2.0 * 20.0;
    let title = if name.is_empty() { "Welcome to this board".to_owned() } else { format!("Welcome to “{name}”") };
    let body = if view {
        "Scroll to zoom and drag to look around. You can't change the board, but you can leave comments (C) and stamps (E) for whoever shared it."
    } else {
        "Scroll to zoom and drag empty board to look around. You can add and move things here; everyone with the link sees the changes. Comments are C, stamps E."
    };
    let lines = panel::wrap_with(tw, body, 13.0, inner);
    let h = 20.0 + 24.0 + 10.0 + lines.len() as f32 * 19.0 + 16.0 + 32.0 + 20.0;
    // Low in the middle, clear of the name box at the top.
    let x0 = ((viewport[0] - w) / 2.0).max(8.0);
    let y0 = (viewport[1] - h - 72.0).max(crate::menu::BAR + 60.0);
    let card = [x0, y0, w, h];
    l.card = card;
    l.boxes.push(theme::shadow(card, theme::radius::PANEL));
    l.bx(card, t.panel_bg, t.line, theme::radius::PANEL, 1.0);
    l.hits.push((card, Hit::Card));
    let mut y = y0 + 20.0 + 17.0;
    l.text(panel::fit_with(tw, &title, 17.0, inner), [x0 + 20.0, y], 17.0, t.ink);
    y += 12.0;
    for line in lines {
        y += 19.0;
        l.text(line, [x0 + 20.0, y], 13.0, t.dim);
    }
    let ok = [x0 + w - 20.0 - 86.0, y0 + h - 20.0 - 32.0, 86.0, 32.0];
    button(&mut l, ok, "Got it", true, hover, Hit::Next, tw);
    l
}

/// A tip next to `target` (screen rect), with a ring around the target.
fn tip_layout(step: usize, target: [f32; 4], web: bool, hover: [f32; 2], viewport: [f32; 2], tw: Tw) -> Layout {
    let t = theme::t();
    let mut l = Layout { modal: false, ..Default::default() };
    let tip = &TIPS[step.min(TIPS.len() - 1)];
    let ring = [target[0] - 4.0, target[1] - 4.0, target[2] + 8.0, target[3] + 8.0];
    // A soft halo, then a crisp accent ring.
    l.bx([ring[0] - 4.0, ring[1] - 4.0, ring[2] + 8.0, ring[3] + 8.0], [0.0; 4], t.accent_soft, theme::radius::PANEL + 6.0, 4.0);
    l.bx(ring, [0.0; 4], t.accent, theme::radius::PANEL + 2.0, 2.0);
    let w = 290.0f32.min(viewport[0] - 16.0);
    let inner = w - 2.0 * 18.0;
    let body = if web && tip.target == Target::FileMenu { "Import pictures and videos, and go back to an earlier version." } else { tip.body };
    let lines = panel::wrap_with(tw, body, 13.0, inner);
    let h = 18.0 + 20.0 + 6.0 + lines.len() as f32 * 19.0 + 16.0 + 30.0 + 16.0;
    // Right of the target if it fits, else left, else below.
    let gap = 16.0;
    let (mut x0, mut y0) = if ring[0] + ring[2] + gap + w <= viewport[0] - 8.0 && ring[3] < viewport[1] * 0.5 || ring[0] + ring[2] + gap + w <= viewport[0] - 8.0 && ring[2] < 80.0 {
        (ring[0] + ring[2] + gap, ring[1])
    } else if ring[0] - gap - w >= 8.0 {
        (ring[0] - gap - w, ring[1] + ring[3] - h)
    } else {
        (ring[0], ring[1] + ring[3] + gap)
    };
    // A tall target (the rail): line the card up with its middle.
    if ring[3] > h * 1.5 {
        y0 = ring[1] + (ring[3] - h) / 2.0;
    }
    x0 = x0.clamp(8.0, (viewport[0] - w - 8.0).max(8.0));
    y0 = y0.clamp(crate::menu::BAR + 8.0, (viewport[1] - h - 8.0).max(crate::menu::BAR + 8.0));
    let card = [x0, y0, w, h];
    l.card = card;
    l.boxes.push(theme::shadow(card, theme::radius::PANEL));
    l.bx(card, t.panel_bg, t.accent, theme::radius::PANEL, 1.0);
    l.hits.push((card, Hit::Card));
    let mut y = y0 + 18.0 + 15.0;
    l.text(tip.title, [x0 + 18.0, y], 15.0, t.ink);
    let count = format!("{} of {}", step + 1, TIPS.len());
    l.text(count.clone(), [x0 + w - 18.0 - tw(&count, 11.5), y - 1.0], 11.5, t.dim);
    y += 6.0;
    for line in lines {
        y += 19.0;
        l.text(line, [x0 + 18.0, y], 13.0, t.dim);
    }
    let last = step + 1 >= TIPS.len();
    let label = if last { "Done" } else { "Next" };
    let bw = tw(label, 13.0) + 30.0;
    let next = [x0 + w - 18.0 - bw, y0 + h - 16.0 - 30.0, bw, 30.0];
    button(&mut l, next, label, true, hover, Hit::Next, tw);
    if !last {
        let s = "Skip tips";
        let sr = [x0 + 8.0, next[1], tw(s, 12.5) + 20.0, 30.0];
        let over = panel::inside(sr, hover);
        l.text(s, [sr[0] + 10.0, sr[1] + 19.5], 12.5, if over { t.ink } else { t.dim });
        l.hits.push((sr, Hit::Skip));
    }
    l
}

impl App {
    /// Starts the walkthrough for a newcomer, unless they've seen it. Shells
    /// call this once the settings are restored (not for share links).
    pub fn welcome_if_new(&mut self) {
        if self.welcome_seen & SEEN_FULL == 0 {
            self.start_welcome(Kind::Full);
        }
    }

    /// Someone opened a share link; `view`: a view-only one. Shows them a
    /// short card the first time.
    pub fn welcome_guest(&mut self, view: bool) {
        if self.welcome_seen & SEEN_GUEST == 0 && self.welcome.is_none() {
            self.start_welcome(Kind::Guest { view });
        }
    }

    /// Help > Welcome tour, or a shell's first run.
    pub fn start_welcome(&mut self, kind: Kind) {
        self.welcome_seen |= match kind {
            Kind::Full => SEEN_FULL,
            Kind::Guest { .. } => SEEN_GUEST,
        };
        self.menu_open = None;
        self.welcome = Some(Welcome { kind, step: 0, tips: false });
        self.needs_redraw = true;
    }

    /// Jumps to a slide (or, with `tips`, a tip): screenshots and tests.
    pub fn welcome_show(&mut self, step: usize, tips: bool) {
        let step = step.min(if tips { TIPS.len() } else { SLIDES.len() } - 1);
        self.welcome = Some(Welcome { kind: Kind::Full, step, tips });
        self.needs_redraw = true;
    }

    pub fn welcome_state(&self) -> Option<Welcome> {
        self.welcome
    }

    /// Where a tip's target is on screen right now.
    fn tip_target(&mut self, target: Target) -> [f32; 4] {
        match target {
            Target::Rail => self.rail_layout().rect,
            Target::BoardsButton => self.rail_layout().side,
            Target::FileMenu => {
                let w = panel::text_width(&mut self.font, crate::menu::MENUS[0].title, 13.0) + 20.0;
                [8.0, 3.0, w, crate::menu::BAR - 6.0]
            }
            Target::Zoom => self.zoom_pill_rect(),
        }
    }

    fn welcome_layout(&mut self) -> Option<Layout> {
        let w = self.welcome?;
        let web = !self.desktop;
        let hover = self.cursor;
        let viewport = self.viewport;
        if w.tips {
            let target = self.tip_target(TIPS[w.step.min(TIPS.len() - 1)].target);
            let font = &mut self.font;
            return Some(tip_layout(w.step, target, web, hover, viewport, &mut |s, size| panel::text_width(font, s, size)));
        }
        let name = self.doc.board.name.clone();
        let font = &mut self.font;
        let tw = &mut |s: &str, size: f32| panel::text_width(font, s, size);
        Some(match w.kind {
            Kind::Full => slides_layout(w.step, web, hover, viewport, tw),
            Kind::Guest { view } => guest_layout(view, &name, hover, viewport, tw),
        })
    }

    fn welcome_go(&mut self, hit: Hit) {
        let Some(mut w) = self.welcome else { return };
        self.needs_redraw = true;
        match hit {
            Hit::Card => return,
            Hit::Skip => {
                // Skipping the slides still offers nothing more; skipping
                // tips ends the tour.
                self.welcome = None;
                return;
            }
            Hit::Back => w.step = w.step.saturating_sub(1),
            Hit::Dot(k) => w.step = k,
            Hit::Next => {
                if matches!(w.kind, Kind::Guest { .. }) {
                    self.welcome = None;
                    return;
                }
                let n = if w.tips { TIPS.len() } else { SLIDES.len() };
                if w.step + 1 < n {
                    w.step += 1;
                } else if !w.tips {
                    w.tips = true;
                    w.step = 0;
                    // The rail must be showing to be pointed at.
                    self.rail_place.collapsed = false;
                } else {
                    self.welcome = None;
                    return;
                }
            }
        }
        self.welcome = Some(w);
    }

    /// A press while the walkthrough shows. True when it took the press:
    /// any press during the slides; on a tip, only presses on its card.
    pub(crate) fn welcome_press(&mut self, pos: [f32; 2]) -> bool {
        let Some(l) = self.welcome_layout() else { return false };
        if let Some(hit) = l.hit_at(pos) {
            self.welcome_go(hit);
            return true;
        }
        l.modal
    }

    /// The board shouldn't zoom behind the slides.
    pub(crate) fn welcome_modal(&self) -> bool {
        self.welcome.is_some_and(|w| !w.tips && w.kind == Kind::Full)
    }

    pub(crate) fn welcome_key(&mut self, key: Key) -> bool {
        let Some(w) = self.welcome else { return false };
        let modal = self.welcome_modal();
        match key {
            Key::Escape => {
                self.welcome = None;
                self.needs_redraw = true;
                true
            }
            Key::Enter | Key::Right if modal || matches!(w.kind, Kind::Guest { .. }) => {
                self.welcome_go(Hit::Next);
                true
            }
            Key::Char(' ') if modal => {
                self.welcome_go(Hit::Next);
                true
            }
            Key::Left if modal => {
                self.welcome_go(Hit::Back);
                true
            }
            // Slides cover the board: its shortcuts wait.
            _ => modal,
        }
    }

    pub(crate) fn draw_welcome(&mut self, renderer: &mut Renderer) {
        let Some(l) = self.welcome_layout() else { return };
        let view = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let boxes: Vec<BoxInstance> = l.boxes.iter().map(|b| BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] }).collect();
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
    use crate::{Button, Mods};
    use wall_format::Board;

    fn app() -> App {
        let mut a = App::new(Board::new("Moodboard", 0), "test");
        a.resize([1280.0, 800.0]);
        a
    }

    fn click(a: &mut App, hit: Hit) {
        let l = a.welcome_layout().unwrap();
        let r = l.hits.iter().rev().find(|(_, h)| *h == hit).unwrap_or_else(|| panic!("no {hit:?}")).0;
        let p = [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0];
        a.pointer_down(Button::Primary, p, Mods::default());
        a.pointer_up(Button::Primary, p);
    }

    #[test]
    fn slides_then_tips_then_done_and_seen_once() {
        let mut a = app();
        a.welcome_if_new();
        assert_eq!(a.welcome_state(), Some(Welcome { kind: Kind::Full, step: 0, tips: false }));
        // The board's keys wait while the slides show.
        let before = a.item_count();
        a.key(Key::Char('t'));
        assert_eq!(a.item_count(), before);
        click(&mut a, Hit::Next);
        a.key(Key::Right);
        click(&mut a, Hit::Back);
        assert_eq!(a.welcome_state().unwrap().step, 1);
        click(&mut a, Hit::Dot(3));
        click(&mut a, Hit::Next);
        let w = a.welcome_state().unwrap();
        assert!(w.tips && w.step == 0);
        for _ in 0..TIPS.len() {
            let l = a.welcome_layout().unwrap();
            assert!(!l.modal);
            click(&mut a, Hit::Next);
        }
        assert!(a.welcome_state().is_none());
        // Seen: remembered in the settings and not shown again.
        let s = a.ui_setting();
        let mut b = app();
        b.set_ui_setting(&s);
        b.welcome_if_new();
        assert!(b.welcome_state().is_none());
        // Help > Welcome tour plays it anyway.
        b.run_command(crate::menu::Command::WelcomeTour);
        assert!(b.welcome_state().is_some());
        b.key(Key::Escape);
        assert!(b.welcome_state().is_none());
    }

    #[test]
    fn settings_without_the_flag_still_restore_the_pen() {
        let mut a = app();
        let s = a.ui_setting();
        let (old, _) = s.rsplit_once("|w").unwrap();
        a.set_ui_setting(old);
        a.welcome_if_new();
        assert!(a.welcome_state().is_some(), "an old settings string means not seen");
    }

    #[test]
    fn guests_get_one_card_and_tips_fit_on_screen() {
        let mut a = app();
        a.welcome_guest(true);
        let l = a.welcome_layout().unwrap();
        assert!(l.texts.iter().any(|t| t.text.contains("Moodboard")));
        assert!(!l.modal);
        click(&mut a, Hit::Next);
        assert!(a.welcome_state().is_none());
        a.welcome_guest(true);
        assert!(a.welcome_state().is_none(), "once per browser");

        for vp in [[1280.0, 800.0], [800.0, 600.0], [420.0, 700.0]] {
            let mut a = app();
            a.resize(vp);
            for k in 0..TIPS.len() {
                a.welcome_show(k, true);
                let c = a.welcome_layout().unwrap().card;
                assert!(c[0] >= 0.0 && c[1] >= 0.0 && c[0] + c[2] <= vp[0] + 0.5 && c[1] + c[3] <= vp[1] + 0.5, "tip {k} at {vp:?}: {c:?}");
            }
            for k in 0..SLIDES.len() {
                a.welcome_show(k, false);
                let c = a.welcome_layout().unwrap().card;
                assert!(c[0] >= 0.0 && c[0] + c[2] <= vp[0] + 0.5, "slide {k} at {vp:?}: {c:?}");
            }
        }
    }
}
