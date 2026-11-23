//! Comments and stamps.
//!
//! A comment is a pin on the board (an item with no size: the pin is the
//! same size on screen at any zoom) holding a thread of messages. Click the
//! pin to read and reply, resolve it to tuck it away. The Comment tool (C)
//! puts a new pin where you click.
//!
//! A stamp is a small reaction ("+1", "♥", "★", "✓", "?", "!") placed with
//! the Stamp tool (E). Stamps are ordinary items: they scale with the board
//! and can be moved or deleted.
//!
//! Someone on a view-only link can still comment and stamp (the server
//! takes those two kinds from them and nothing else).

use uuid::Uuid;
use wall_format::{BoardItem, CommentMessage, Frame, ItemKind};
use wall_render::{BoxInstance, Renderer, ViewUniform};

use crate::menu::BAR;
use crate::panel::{self, PanelBox, TextRun};
use crate::{presence, scene, theme, toolbar, App};

/// The marks a stamp can have, with their colours.
pub const MARKS: [(&str, [f32; 4]); 6] = [
    ("+1", [0.29, 0.52, 0.96, 1.0]),
    ("♥", [0.91, 0.30, 0.38, 1.0]),
    ("★", [0.96, 0.69, 0.16, 1.0]),
    ("✓", [0.24, 0.70, 0.45, 1.0]),
    ("?", [0.58, 0.42, 0.93, 1.0]),
    ("!", [0.95, 0.50, 0.20, 1.0]),
];

/// A stamp's colour by its mark.
pub fn stamp_color(mark: &str) -> [f32; 4] {
    MARKS.iter().find(|(m, _)| *m == mark).map_or([0.5, 0.5, 0.55, 1.0], |(_, c)| *c)
}

/// How big a new stamp is on screen, whatever the zoom.
const STAMP_PX: f32 = 40.0;
/// The pin's size on screen.
const PIN: f32 = 28.0;
const CARD_W: f32 = 300.0;
const MAX_TEXT: usize = 2000;
/// Messages shown at once (the newest).
const SHOWN: usize = 6;

/// The open comment card: an existing thread, or a new one at `at`.
#[derive(Debug, Clone, Default)]
pub struct CommentCard {
    pub id: Option<Uuid>,
    /// Board point of a new comment.
    pub at: [f32; 2],
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CardHit {
    Send,
    Resolve,
    Delete,
    Close,
    Field,
}

#[derive(Default)]
pub struct CommentLayout {
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    /// Pins on screen: (circle, comment id).
    pub pins: Vec<([f32; 4], Uuid)>,
    pub card: Option<[f32; 4]>,
    pub hits: Vec<([f32; 4], CardHit)>,
}

/// "now", "5m", "3h", "2d".
fn ago(now_ms: u64, at_ms: u64) -> String {
    let s = now_ms.saturating_sub(at_ms) / 1000;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

fn initial(name: &str) -> String {
    name.trim().chars().next().map_or("?".into(), |c| c.to_uppercase().collect())
}

pub struct Inputs<'a> {
    /// Live comments: (id, board point, messages, resolved).
    pub comments: Vec<(Uuid, [f32; 2], &'a [CommentMessage], bool)>,
    pub card: Option<&'a CommentCard>,
    pub show_resolved: bool,
    pub me: &'a str,
    pub can_delete: bool,
    pub now_ms: u64,
    pub caret: bool,
    pub hover: [f32; 2],
    pub viewport: [f32; 2],
}

pub fn layout(i: &Inputs, to_screen: &dyn Fn([f32; 2]) -> [f32; 2], width: &mut dyn FnMut(&str, f32) -> f32) -> CommentLayout {
    let t = theme::t();
    let mut l = CommentLayout::default();
    let open = i.card.and_then(|c| c.id);
    let pin = |l: &mut CommentLayout, p: [f32; 2], color: [f32; 4], label: &str, on: bool, faded: bool, width: &mut dyn FnMut(&str, f32) -> f32| {
        let r = [p[0], p[1] - PIN, PIN, PIN];
        let a = if faded { 0.45 } else { 1.0 };
        let fill = [color[0] * a, color[1] * a, color[2] * a, a];
        let ring = if on { t.accent } else { [1.0, 1.0, 1.0, a] };
        // A round head with its bottom-left corner pointing at the spot.
        l.boxes.push(PanelBox { rect: [p[0], p[1] - 11.0, 11.0, 11.0], fill: ring, stroke: [0.0; 4], radius: 1.5, stroke_w: 0.0 });
        l.boxes.push(PanelBox { rect: r, fill, stroke: ring, radius: PIN / 2.0, stroke_w: if on { 2.5 } else { 2.0 } });
        let tw = width(label, 12.5);
        l.texts.push(TextRun { text: label.to_owned(), at: [r[0] + (PIN - tw) / 2.0, r[1] + PIN / 2.0 + 4.5], size: 12.5, color: [1.0, 1.0, 1.0, a] });
        r
    };
    let vp = i.viewport;
    for (id, at, messages, resolved) in &i.comments {
        let on = open == Some(*id);
        if *resolved && !i.show_resolved && !on {
            continue;
        }
        let p = to_screen(*at);
        if p[0] < -PIN || p[1] < BAR || p[0] > vp[0] + PIN || p[1] > vp[1] + PIN {
            continue;
        }
        let first = messages.first().map_or("?", |m| m.by.as_str());
        let label = if messages.len() > 1 { messages.len().to_string() } else { initial(first) };
        let r = pin(&mut l, p, presence::color_of(first), &label, on, *resolved, width);
        l.pins.push((r, *id));
    }
    let Some(card) = i.card else { return l };
    let thread: &[CommentMessage] = card.id.and_then(|id| i.comments.iter().find(|c| c.0 == id)).map_or(&[], |c| c.2);
    let resolved = card.id.and_then(|id| i.comments.iter().find(|c| c.0 == id)).is_some_and(|c| c.3);
    let at = card.id.and_then(|id| i.comments.iter().find(|c| c.0 == id)).map_or(card.at, |c| c.1);
    let p = to_screen(at);
    if card.id.is_none() {
        pin(&mut l, p, presence::color_of(i.me), &initial(i.me), true, false, width);
    }

    // Measure first, then place.
    let pad = 14.0;
    let inner = CARD_W - 2.0 * pad;
    let mut rows: Vec<(TextRun, f32)> = Vec::new(); // text, and its y from the top of the card
    let mut y = 0.0;
    let hidden = thread.len().saturating_sub(SHOWN);
    if hidden > 0 {
        y += 18.0;
        rows.push((TextRun { text: format!("{hidden} earlier"), at: [0.0, 0.0], size: 11.5, color: t.dim }, y));
        y += 6.0;
    }
    for m in &thread[hidden..] {
        y += 18.0;
        let who = panel::fit_with(width, &m.by, 12.5, inner - 50.0);
        let who_w = width(&who, 12.5);
        rows.push((TextRun { text: who, at: [0.0, 0.0], size: 12.5, color: presence::color_of(&m.by) }, y));
        rows.push((TextRun { text: ago(i.now_ms, m.at_ms), at: [who_w + 8.0, 0.0], size: 11.5, color: t.dim }, y));
        for line in panel::wrap_with(width, &m.text, 13.0, inner) {
            y += 18.0;
            rows.push((TextRun { text: line, at: [0.0, 0.0], size: 13.0, color: t.ink }, y));
        }
        y += 8.0;
    }
    let field_lines: Vec<String> = if card.text.is_empty() { vec![String::new()] } else { panel::wrap_with(width, &card.text, 13.0, inner - 16.0) };
    let field_lines: Vec<String> = field_lines.iter().rev().take(5).rev().cloned().collect();
    let field_h = 14.0 + 18.0 * field_lines.len().max(1) as f32;
    let head = 40.0;
    let h = head + y + 8.0 + field_h + 12.0 + 30.0 + pad;
    let x0 = (p[0] + PIN + 10.0).min(vp[0] - CARD_W - 8.0).max(8.0);
    let y0 = (p[1] - PIN).min(vp[1] - h - 8.0).max(BAR + 8.0);
    let c = [x0, y0, CARD_W, h];
    l.card = Some(c);
    l.boxes.push(crate::theme::shadow(c, crate::theme::radius::PANEL));
    l.boxes.push(PanelBox { rect: c, fill: t.panel_bg, stroke: t.line, radius: crate::theme::radius::PANEL, stroke_w: 1.0 });

    // Header: what it is, and its buttons on the right.
    let title = match (card.id, resolved) {
        (None, _) => "New comment",
        (Some(_), true) => "Comment  ·  resolved",
        (Some(_), false) => "Comment",
    };
    l.texts.push(TextRun { text: title.into(), at: [x0 + pad, y0 + 26.0], size: 13.5, color: t.ink });
    let mut bx = x0 + CARD_W - pad - 22.0;
    let close = [bx, y0 + 10.0, 22.0, 22.0];
    l.texts.push(TextRun { text: "×".into(), at: [close[0] + 5.0, close[1] + 16.0], size: 18.0, color: t.dim });
    l.hits.push((close, CardHit::Close));
    let mut small = |l: &mut CommentLayout, label: &str, hit: CardHit, bx: &mut f32| {
        let w = width(label, 12.0) + 16.0;
        *bx -= w + 6.0;
        let r = [*bx, y0 + 10.0, w, 22.0];
        let fill = if panel::inside(r, i.hover) { t.field } else { [0.0; 4] };
        l.boxes.push(PanelBox { rect: r, fill, stroke: t.line, radius: 6.0, stroke_w: 1.0 });
        l.texts.push(TextRun { text: label.into(), at: [r[0] + 8.0, r[1] + 15.5], size: 12.0, color: t.ink });
        l.hits.push((r, hit));
    };
    if card.id.is_some() {
        small(&mut l, if resolved { "Reopen" } else { "Resolve" }, CardHit::Resolve, &mut bx);
        if i.can_delete {
            small(&mut l, "Delete", CardHit::Delete, &mut bx);
        }
    }
    for (mut run, ry) in rows {
        run.at = [x0 + pad + run.at[0], y0 + head + ry - 4.0];
        l.texts.push(run);
    }

    // The field, with the caret after the last line.
    let fy = y0 + head + y + 8.0;
    let field = [x0 + pad, fy, inner, field_h];
    l.boxes.push(PanelBox { rect: field, fill: t.field, stroke: t.accent, radius: 7.0, stroke_w: 1.5 });
    l.hits.push((field, CardHit::Field));
    if card.text.is_empty() {
        let hint = if thread.is_empty() { "Add a comment" } else { "Reply" };
        l.texts.push(TextRun { text: hint.into(), at: [field[0] + 8.0, fy + 20.0], size: 13.0, color: t.dim });
    }
    let mut ly = fy + 20.0;
    let mut last_w = 0.0;
    for line in &field_lines {
        last_w = width(line, 13.0);
        if !line.is_empty() {
            l.texts.push(TextRun { text: line.clone(), at: [field[0] + 8.0, ly], size: 13.0, color: t.ink });
        }
        ly += 18.0;
    }
    if i.caret {
        let cy = ly - 18.0;
        l.boxes.push(PanelBox { rect: [field[0] + 9.0 + last_w, cy - 13.0, 1.5, 16.0], fill: t.ink, stroke: [0.0; 4], radius: 0.0, stroke_w: 0.0 });
    }
    let by = fy + field_h + 12.0;
    l.texts.push(TextRun { text: "Enter to send".into(), at: [x0 + pad, by + 19.0], size: 11.5, color: t.dim });
    let send = [x0 + CARD_W - pad - 70.0, by, 70.0, 30.0];
    let empty = card.text.trim().is_empty();
    let fill = if empty { t.field } else { t.accent };
    l.boxes.push(PanelBox { rect: send, fill, stroke: [0.0; 4], radius: crate::theme::radius::CONTROL, stroke_w: 0.0 });
    let sw = width("Send", 12.5);
    l.texts.push(TextRun { text: "Send".into(), at: [send[0] + (70.0 - sw) / 2.0, send[1] + 19.5], size: 12.5, color: if empty { t.dim } else { t.accent_ink } });
    l.hits.push((send, CardHit::Send));
    l
}

/// The stamp bar: which mark the Stamp tool puts down.
#[derive(Default)]
pub struct StampBar {
    pub rect: [f32; 4],
    pub boxes: Vec<PanelBox>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<([f32; 4], Option<usize>)>,
}

pub fn stamp_bar(picked: usize, hover: [f32; 2], left: f32, right: f32, width: &mut dyn FnMut(&str, f32) -> f32) -> StampBar {
    let t = theme::t();
    let mut b = StampBar::default();
    let cell = 34.0;
    let gap = 6.0;
    let done_w = width("Done", 12.5) + 22.0;
    let w = 8.0 + MARKS.len() as f32 * (cell + gap) + 8.0 + done_w + 8.0;
    let x0 = (left + (right - left - w) / 2.0).max(left + 8.0);
    let y0 = BAR + 8.0;
    b.rect = [x0, y0, w, cell + 12.0];
    b.boxes.push(crate::theme::shadow(b.rect, crate::theme::radius::PANEL));
    b.boxes.push(PanelBox { rect: b.rect, fill: t.panel_bg, stroke: t.line, radius: crate::theme::radius::PANEL, stroke_w: 1.0 });
    let mut x = x0 + 8.0;
    for (k, (mark, color)) in MARKS.iter().enumerate() {
        let r = [x, y0 + 6.0, cell, cell];
        if k == picked {
            b.boxes.push(PanelBox { rect: r, fill: t.chip, stroke: t.accent, radius: 8.0, stroke_w: 1.5 });
        } else if panel::inside(r, hover) {
            b.boxes.push(PanelBox { rect: r, fill: t.field, stroke: [0.0; 4], radius: 8.0, stroke_w: 0.0 });
        }
        let d = [r[0] + 5.0, r[1] + 5.0, cell - 10.0, cell - 10.0];
        b.boxes.push(PanelBox { rect: d, fill: *color, stroke: [0.0; 4], radius: d[2] / 2.0, stroke_w: 0.0 });
        let size = if mark.chars().count() > 1 { 11.0 } else { 14.0 };
        let mw = width(mark, size);
        b.texts.push(TextRun { text: (*mark).into(), at: [d[0] + (d[2] - mw) / 2.0, d[1] + d[3] / 2.0 + size * 0.36], size, color: [1.0; 4] });
        b.hits.push((r, Some(k)));
        x += cell + gap;
    }
    let r = [x + 2.0, y0 + 8.0, done_w, cell - 4.0];
    let fill = if panel::inside(r, hover) { t.field } else { [0.0; 4] };
    b.boxes.push(PanelBox { rect: r, fill, stroke: t.line, radius: 7.0, stroke_w: 1.0 });
    b.texts.push(TextRun { text: "Done".into(), at: [r[0] + 11.0, r[1] + 19.0], size: 12.5, color: t.ink });
    b.hits.push((r, None));
    b
}

impl App {
    pub(crate) fn comment_layout(&mut self) -> CommentLayout {
        let cam = self.camera;
        let vp = self.viewport;
        let to_screen = move |p: [f32; 2]| [(p[0] - cam.center[0]) * cam.zoom + vp[0] * 0.5, (p[1] - cam.center[1]) * cam.zoom + vp[1] * 0.5];
        let comments = self
            .doc
            .board
            .live_items()
            .filter_map(|it| match &it.kind {
                ItemKind::Comment { messages, resolved } => Some((it.id, [it.frame.x as f32, it.frame.y as f32], messages.as_slice(), *resolved)),
                _ => None,
            })
            .collect();
        let me = self.my_name.clone().unwrap_or_else(|| "Me".into());
        let inputs = Inputs {
            comments,
            card: self.comment_card.as_ref(),
            show_resolved: self.tool == toolbar::Tool::Comment,
            me: &me,
            can_delete: !self.view_only,
            now_ms: self.now_ms,
            caret: (self.now_ms / 530) % 2 == 0,
            hover: self.cursor,
            viewport: vp,
        };
        let font = &mut self.font;
        layout(&inputs, &to_screen, &mut |s, size| panel::text_width(font, s, size))
    }

    /// Typing a comment.
    pub(crate) fn comment_typing(&self) -> bool {
        self.comment_card.is_some()
    }

    /// Is a view-only link open here: only comments and stamps are kept.
    pub fn set_view_only(&mut self, view: bool) {
        self.view_only = view;
    }

    pub fn view_only(&self) -> bool {
        self.view_only
    }

    /// A press on the open card. True when it was on the card. A press
    /// elsewhere closes it and goes on.
    pub(crate) fn comment_card_press(&mut self, pos: [f32; 2]) -> bool {
        if self.comment_card.is_none() {
            return false;
        }
        let l = self.comment_layout();
        let Some(c) = l.card else { return false };
        self.needs_redraw = true;
        if !panel::inside(c, pos) {
            // Another pin opens in its place.
            if let Some((_, id)) = l.pins.iter().rev().find(|(r, _)| panel::inside(*r, pos)) {
                self.comment_card = Some(CommentCard { id: Some(*id), ..Default::default() });
                return true;
            }
            self.comment_card = None;
            return false;
        }
        match l.hits.iter().rev().find(|(r, _)| panel::inside(*r, pos)).map(|(_, h)| *h) {
            Some(CardHit::Send) => self.send_comment(),
            Some(CardHit::Close) => self.comment_card = None,
            Some(CardHit::Resolve) => self.toggle_resolved(),
            Some(CardHit::Delete) => {
                if let Some(id) = self.comment_card.take().and_then(|c| c.id) {
                    self.doc.delete(&[id].into());
                }
            }
            Some(CardHit::Field) | None => {}
        }
        true
    }

    /// A press on the board (not on a panel): opens a pin, or puts down a
    /// new comment or a stamp with those tools. True when it did.
    pub(crate) fn comment_board_press(&mut self, pos: [f32; 2], board: [f32; 2]) -> bool {
        let l = self.comment_layout();
        if let Some((_, id)) = l.pins.iter().rev().find(|(r, _)| panel::inside(*r, pos)) {
            self.comment_card = Some(CommentCard { id: Some(*id), ..Default::default() });
            self.end_edit();
            self.needs_redraw = true;
            return true;
        }
        match self.tool {
            toolbar::Tool::Comment => {
                self.selection.clear();
                self.comment_card = Some(CommentCard { id: None, at: board, text: String::new() });
                self.needs_redraw = true;
                true
            }
            toolbar::Tool::Stamp => {
                self.put_stamp(board);
                true
            }
            _ => false,
        }
    }

    fn put_stamp(&mut self, at: [f32; 2]) {
        let s = (STAMP_PX / self.camera.zoom.max(1e-4)) as f64;
        let mark = MARKS[self.stamp_mark.min(MARKS.len() - 1)].0.to_owned();
        let by = self.my_name.clone().unwrap_or_default();
        let frame = Frame { x: at[0] as f64 - s / 2.0, y: at[1] as f64 - s / 2.0, w: s, h: s, ..Default::default() };
        self.doc.add(vec![(frame, ItemKind::Stamp { mark, by })]);
        self.needs_redraw = true;
    }

    fn send_comment(&mut self) {
        let Some(card) = &mut self.comment_card else { return };
        let text = card.text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let by = self.my_name.clone().unwrap_or_else(|| "Me".into());
        let message = CommentMessage { by, text, at_ms: self.now_ms };
        match card.id {
            None => {
                let frame = Frame { x: card.at[0] as f64, y: card.at[1] as f64, w: 0.0, h: 0.0, ..Default::default() };
                let id = self.doc.add(vec![(frame, ItemKind::Comment { messages: vec![message], resolved: false })])[0];
                self.comment_card = Some(CommentCard { id: Some(id), ..Default::default() });
            }
            Some(id) => {
                let Some(mut after) = self.doc.item(&id).cloned() else { return };
                if let ItemKind::Comment { messages, resolved } = &mut after.kind {
                    messages.push(message);
                    *resolved = false;
                }
                self.doc.replace(vec![after]);
                card.text.clear();
            }
        }
        self.needs_redraw = true;
    }

    fn toggle_resolved(&mut self) {
        let Some(id) = self.comment_card.as_ref().and_then(|c| c.id) else { return };
        let Some(mut after): Option<BoardItem> = self.doc.item(&id).cloned() else { return };
        let mut now_resolved = false;
        if let ItemKind::Comment { resolved, .. } = &mut after.kind {
            *resolved = !*resolved;
            now_resolved = *resolved;
        }
        self.doc.replace(vec![after]);
        if now_resolved {
            self.comment_card = None;
        }
        self.needs_redraw = true;
    }

    pub(crate) fn comment_key(&mut self, key: crate::Key) -> bool {
        use crate::Key;
        let Some(card) = &mut self.comment_card else { return false };
        match key {
            Key::Escape => self.comment_card = None,
            Key::Enter => self.send_comment(),
            Key::Backspace => {
                card.text.pop();
            }
            Key::Char(c) if !c.is_control() && card.text.chars().count() < MAX_TEXT => card.text.push(c),
            _ => {}
        }
        self.needs_redraw = true;
        true
    }

    pub(crate) fn comment_text(&mut self, s: &str) -> bool {
        let Some(card) = &mut self.comment_card else { return false };
        for c in s.chars() {
            if card.text.chars().count() >= MAX_TEXT {
                break;
            }
            card.text.push(if c.is_control() { ' ' } else { c });
        }
        self.needs_redraw = true;
        true
    }

    /// The comment open on the card, if any.
    pub fn open_comment(&self) -> Option<Uuid> {
        self.comment_card.as_ref().and_then(|c| c.id)
    }

    fn stamp_bar_layout(&mut self) -> Option<StampBar> {
        if self.tool != toolbar::Tool::Stamp {
            return None;
        }
        let left = self.left_inset();
        let right = self.panel_layout().map_or(self.viewport[0], |p| p.rect[0] - 8.0);
        let font = &mut self.font;
        Some(stamp_bar(self.stamp_mark, self.cursor, left, right, &mut |s, size| panel::text_width(font, s, size)))
    }

    pub(crate) fn stamp_bar_press(&mut self, pos: [f32; 2]) -> bool {
        let Some(b) = self.stamp_bar_layout() else { return false };
        if !panel::inside(b.rect, pos) {
            return false;
        }
        match b.hits.iter().find(|(r, _)| panel::inside(*r, pos)).map(|(_, h)| *h) {
            Some(Some(k)) => self.stamp_mark = k,
            Some(None) => self.tool = toolbar::Tool::Select,
            None => {}
        }
        self.needs_redraw = true;
        true
    }

    /// 1–6 pick a mark while the Stamp tool is on.
    pub(crate) fn stamp_key(&mut self, c: char) -> bool {
        if self.tool != toolbar::Tool::Stamp {
            return false;
        }
        match c {
            '1'..='6' => {
                self.stamp_mark = c as usize - '1' as usize;
                self.needs_redraw = true;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn draw_comments(&mut self, renderer: &mut Renderer) {
        let bar = self.stamp_bar_layout();
        let has_pins = self.doc.board.live_items().any(|i| matches!(i.kind, ItemKind::Comment { .. }));
        if !has_pins && self.comment_card.is_none() && bar.is_none() {
            return;
        }
        if self.comment_card.is_some() {
            self.needs_redraw = true; // the caret blinks
        }
        let l = self.comment_layout();
        let mut boxes: Vec<&PanelBox> = l.boxes.iter().collect();
        let mut texts: Vec<&TextRun> = l.texts.iter().collect();
        if let Some(b) = &bar {
            boxes.extend(&b.boxes);
            texts.extend(&b.texts);
        }
        let view = ViewUniform::new([self.viewport[0] / 2.0, self.viewport[1] / 2.0], 1.0, self.viewport);
        let boxes: Vec<BoxInstance> = boxes.iter().map(|b| BoxInstance { rect: b.rect, fill: b.fill, stroke: b.stroke, shape: [b.radius, b.stroke_w] }).collect();
        renderer.draw_boxes(&view, &boxes);
        let mut glyphs = Vec::new();
        for t in texts {
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
    use crate::{Button, Key, Mods};

    fn app() -> App {
        let mut a = App::new(wall_format::Board::new("B", 0), "test");
        a.resize([1000.0, 700.0]);
        a
    }

    fn press(a: &mut App, p: [f32; 2]) {
        a.pointer_down(Button::Primary, p, Mods::default());
        a.pointer_up(Button::Primary, p);
    }

    fn comments(a: &App) -> Vec<(Vec<String>, bool)> {
        a.doc
            .board
            .live_items()
            .filter_map(|i| match &i.kind {
                ItemKind::Comment { messages, resolved } => Some((messages.iter().map(|m| format!("{}: {}", m.by, m.text)).collect(), *resolved)),
                _ => None,
            })
            .collect()
    }

    fn hit(a: &mut App, h: CardHit) -> [f32; 2] {
        let l = a.comment_layout();
        let r = l.hits.iter().find(|(_, x)| *x == h).expect("on the card").0;
        [r[0] + 3.0, r[1] + 3.0]
    }

    #[test]
    fn a_comment_is_started_with_the_tool_replied_to_and_resolved() {
        let mut a = app();
        a.set_my_name(Some("Mia".into()));
        a.key(Key::Char('c'));
        assert_eq!(a.tool, toolbar::Tool::Comment);
        press(&mut a, [500.0, 400.0]);
        assert!(a.is_editing(), "the card takes the keys");
        a.text_input("Love this light");
        a.key(Key::Char('!'));
        a.key(Key::Enter);
        assert_eq!(comments(&a), vec![(vec!["Mia: Love this light!".to_owned()], false)]);
        let id = a.open_comment().expect("the new thread stays open");

        // A reply from someone else, on its pin.
        a.key(Key::Escape);
        assert!(!a.is_editing());
        a.set_my_name(Some("Paul".into()));
        let pin = a.comment_layout().pins[0];
        assert_eq!(pin.1, id);
        press(&mut a, [pin.0[0] + 10.0, pin.0[1] + 10.0]);
        assert_eq!(a.open_comment(), Some(id));
        a.text_input("Agreed");
        let send = hit(&mut a, CardHit::Send);
        press(&mut a, send);
        assert_eq!(comments(&a)[0].0, ["Mia: Love this light!", "Paul: Agreed"]);
        assert!(a.comment_layout().texts.iter().any(|t| t.text == "2"), "the pin counts the messages");

        // Resolved: the pin goes, but the Comment tool shows it faded.
        let resolve = hit(&mut a, CardHit::Resolve);
        press(&mut a, resolve);
        assert!(comments(&a)[0].1);
        assert!(a.open_comment().is_none());
        assert_eq!(a.comment_layout().pins.len(), 1, "Comment tool still on: resolved pins show");
        a.key(Key::Escape);
        assert_eq!(a.tool, toolbar::Tool::Select);
        assert!(a.comment_layout().pins.is_empty());

        // Undo brings it back open.
        a.key(Key::Undo);
        assert!(!comments(&a)[0].1);
    }

    #[test]
    fn an_empty_comment_isnt_made_and_a_click_away_closes_the_card() {
        let mut a = app();
        a.key(Key::Char('c'));
        press(&mut a, [500.0, 400.0]);
        a.key(Key::Enter);
        assert!(comments(&a).is_empty());
        a.key(Key::Escape);
        a.key(Key::Escape);
        press(&mut a, [300.0, 300.0]);
        a.text_input("hi");
        press(&mut a, [20.0, 680.0]);
        assert!(!a.is_editing());
        assert!(comments(&a).is_empty());
    }

    #[test]
    fn view_links_get_no_delete_button() {
        let mut a = app();
        a.key(Key::Char('c'));
        press(&mut a, [500.0, 400.0]);
        a.text_input("x");
        a.key(Key::Enter);
        assert!(a.comment_layout().hits.iter().any(|(_, h)| *h == CardHit::Delete));
        a.set_view_only(true);
        assert!(!a.comment_layout().hits.iter().any(|(_, h)| *h == CardHit::Delete));
        a.set_view_only(false);
        let del = hit(&mut a, CardHit::Delete);
        press(&mut a, del);
        assert!(comments(&a).is_empty());
    }

    #[test]
    fn the_stamp_tool_puts_down_the_picked_mark_at_screen_size() {
        let mut a = app();
        a.set_my_name(Some("Ben".into()));
        a.key(Key::Char('e'));
        assert_eq!(a.tool, toolbar::Tool::Stamp);
        a.key(Key::Char('2'));
        let bar = a.stamp_bar_layout().unwrap();
        assert!(bar.texts.iter().any(|t| t.text == "★"));
        press(&mut a, [500.0, 400.0]);
        press(&mut a, [600.0, 400.0]);
        let stamps: Vec<_> = a.doc.board.live_items().filter_map(|i| match &i.kind {
            ItemKind::Stamp { mark, by } => Some((mark.clone(), by.clone(), i.frame.w as f32 * a.camera.zoom)),
            _ => None,
        }).collect();
        assert_eq!(stamps.len(), 2, "the tool stays on");
        assert_eq!((stamps[0].0.as_str(), stamps[0].1.as_str()), ("♥", "Ben"));
        assert!((stamps[0].2 - STAMP_PX).abs() < 0.01);
        // A mark from the bar, then Done.
        let (r, _) = bar.hits[2];
        press(&mut a, [r[0] + 3.0, r[1] + 3.0]);
        assert_eq!(a.stamp_mark, 2);
        let (r, _) = *bar.hits.last().unwrap();
        press(&mut a, [r[0] + 3.0, r[1] + 3.0]);
        assert_eq!(a.tool, toolbar::Tool::Select);
    }
}
